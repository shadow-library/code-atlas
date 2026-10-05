use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use atlas_core::{
  ATTR_EXTENDS, ATTR_IMPL_OF, ATTR_IMPLEMENTS, ATTR_MOCK, ATTR_OVERRIDE_VIA, CertaintyClass, Edge, EdgeKind, Entity, EntityId, EntityKind, ImplInfo, LanguageFamily, Owner,
  RelPath, RepoName, UnresolvedReason, Visibility, is_test_code, is_test_path,
};
use atlas_store::EdgeLink;
use serde::Serialize;

use crate::QueryEngine;
use crate::dynamic::{dynamic_member_calls, dynamic_member_files};
use crate::engine::{risk_reason, unresolved_risk};
use crate::error::QueryResult;
use crate::graph::{FileKey, Graph};
use crate::model::{Caveat, EntityRef, Listing, Response};
use crate::policy::Policy;
use crate::queries::common::{DEFAULT_LIMIT, attr_str, attribute_names, is_router_relative, is_test, language, link_caveat, package_key, superseded_fragments};
use crate::queries::inferred_links::{LinkEnd, LinkScan, inferred_links};
use crate::queries::textual::{TestCode, TextCheck, TextIndex, TextScopes};

const SYMBOL_KINDS: [EntityKind; 10] = [
  EntityKind::Function,
  EntityKind::Method,
  EntityKind::Class,
  EntityKind::Struct,
  EntityKind::Enum,
  EntityKind::Trait,
  EntityKind::Interface,
  EntityKind::TypeAlias,
  EntityKind::Constant,
  EntityKind::Macro,
];

const MAX_HERITAGE_WALK: usize = 32;

const MAX_TEST_SOURCES_SHOWN: usize = 3;

const TRAIT_IMPLEMENTATION: &str = "trait or interface implementation";

const UNCHECKED_DEFINITIONS: &str = "config keys and env vars are not checked for use: their definitions (.env, config files) are not indexed";

const NON_USAGE: [EdgeKind; 8] = [
  EdgeKind::Contains,
  EdgeKind::Defines,
  EdgeKind::CoversFile,
  EdgeKind::RenamedFrom,
  EdgeKind::SimilarTo,
  EdgeKind::EquivalentTo,
  EdgeKind::Exports,
  EdgeKind::ReExports,
];

const ROUTE_DIRECTORIES: [&str; 3] = ["app", "pages", "routes"];

const RUST_REGISTRATION_ATTRIBUTES: [&str; 14] = [
  "no_mangle",
  "export_name",
  "proc_macro",
  "proc_macro_derive",
  "proc_macro_attribute",
  "tokio::main",
  "main",
  "ctor",
  "get",
  "post",
  "put",
  "delete",
  "patch",
  "route",
];

#[derive(Debug, Clone, PartialEq)]
pub struct UnusedRequest {
  pub kinds: Vec<EntityKind>,
  pub repo: Option<String>,
  pub limit: usize,
}

impl Default for UnusedRequest {
  fn default() -> Self {
    Self {
      kinds: Vec::new(),
      repo: None,
      limit: DEFAULT_LIMIT,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnusedClass {
  LikelyUnused,
  PossiblyUnused,
  TestOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnusedCandidate {
  pub entity: EntityRef,
  pub class: UnusedClass,
  pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnusedAnswer {
  pub likely_unused: Listing<UnusedCandidate>,
  pub possibly_unused: Listing<UnusedCandidate>,
  pub test_only: Listing<UnusedCandidate>,
  pub excluded_entrypoints: BTreeMap<String, usize>,
  pub caveats: Vec<Caveat>,
}

impl QueryEngine {
  pub fn unused(&self, request: &UnusedRequest) -> QueryResult<Response<UnusedAnswer>> {
    let freshness = self.barrier()?;
    let private = self.private_packages()?;
    let Scanned {
      mut candidates,
      checks,
      scopes,
      excluded: excluded_entrypoints,
      mut caveats,
    } = self.read(Policy::with_floor(0.0), |graph| unused(graph, request, &private))?;
    let index = TextIndex::build(self, &checks, &scopes);
    let mut refuted: HashSet<usize> = HashSet::new();
    for check in &checks {
      let Some(candidate) = candidates.get_mut(check.index) else { continue };
      let Some(reason) = index.verify(check) else { continue };
      if candidate.class == UnusedClass::TestOnly {
        refuted.insert(check.index);
        continue;
      }
      candidate.class = UnusedClass::PossiblyUnused;
      candidate.evidence.push(reason);
    }
    let candidates: Vec<UnusedCandidate> = candidates
      .into_iter()
      .enumerate()
      .filter(|(index, _)| !refuted.contains(index))
      .map(|(_, candidate)| candidate)
      .collect();
    caveats.extend(self.problem_files(freshness.run)?);
    let mut likely = Vec::new();
    let mut possibly = Vec::new();
    let mut test_only = Vec::new();
    for candidate in candidates {
      match candidate.class {
        UnusedClass::LikelyUnused => likely.push(candidate),
        UnusedClass::PossiblyUnused => possibly.push(candidate),
        UnusedClass::TestOnly => test_only.push(candidate),
      }
    }
    for group in [&mut likely, &mut possibly, &mut test_only] {
      group.sort_by_key(|candidate| candidate.entity.sort_key());
    }
    let answer = UnusedAnswer {
      likely_unused: Listing::truncated(likely, request.limit),
      possibly_unused: Listing::truncated(possibly, request.limit),
      test_only: Listing::truncated(test_only, request.limit),
      excluded_entrypoints,
      caveats,
    };
    Ok(Response { freshness, answer })
  }

  fn private_packages(&self) -> QueryResult<BTreeSet<EntityId>> {
    let manifests = self.read(Policy::with_floor(0.0), |graph| -> QueryResult<Vec<(EntityId, RepoName, RelPath)>> {
      let mut manifests = Vec::new();
      for package in graph.entities_of_kind(&EntityKind::Package)? {
        let (Some(repo), Some(manifest)) = (attr_str(&package, "repo"), attr_str(&package, "manifest")) else {
          continue;
        };
        if !manifest.ends_with("package.json") {
          continue;
        }
        if let (Ok(repo), Ok(manifest)) = (RepoName::new(repo), RelPath::new(manifest)) {
          manifests.push((package.id, repo, manifest));
        }
      }
      Ok(manifests)
    })?;
    let mut private = BTreeSet::new();
    for (package, repo, manifest) in manifests {
      let Ok((text, _)) = self.engine().read_source(&repo, &manifest) else { continue };
      let flagged = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("private").and_then(serde_json::Value::as_bool))
        .unwrap_or(false);
      if flagged {
        private.insert(package);
      }
    }
    Ok(private)
  }
}

#[derive(Default)]
struct Scan<'p> {
  candidates: Vec<UnusedCandidate>,
  checks: Vec<TextCheck>,
  excluded: BTreeMap<String, usize>,
  packages: HashMap<EntityId, PackageRole>,
  package_files: HashMap<EntityId, Vec<FileKey>>,
  package_file_sets: HashMap<EntityId, HashSet<FileKey>>,
  repo_files: HashMap<RepoName, Vec<RelPath>>,
  macro_files: Option<BTreeSet<FileKey>>,
  dynamic: HashMap<RepoName, DynamicSites>,
  private: Option<&'p BTreeSet<EntityId>>,
  liveness: HashMap<EntityId, bool>,
  tests: TestScope,
}

#[derive(Default, Clone)]
struct DynamicSites {
  imports: Vec<String>,
}

#[derive(Debug, Clone)]
enum PackageRole {
  Library { reason: String },
  Application { name: String },
}

fn wanted(request: &UnusedRequest, kind: &EntityKind) -> bool {
  request.kinds.is_empty() || request.kinds.contains(kind)
}

fn in_repo(request: &UnusedRequest, entity: &Entity) -> bool {
  request
    .repo
    .as_ref()
    .is_none_or(|repo| entity.repo.as_ref().is_some_and(|candidate| candidate.as_str() == repo))
}

struct Scanned {
  candidates: Vec<UnusedCandidate>,
  checks: Vec<TextCheck>,
  scopes: TextScopes,
  excluded: BTreeMap<String, usize>,
  caveats: Vec<Caveat>,
}

fn unused(graph: &Graph<'_, '_>, request: &UnusedRequest, private: &BTreeSet<EntityId>) -> QueryResult<Scanned> {
  let mut scan = Scan {
    private: Some(private),
    ..Scan::default()
  };
  graph.preload_dynamic_sites()?;
  scan.tests = TestScope::build(graph)?;
  for kind in SYMBOL_KINDS.iter().filter(|kind| wanted(request, kind)) {
    for entity in graph.entities_of_kind(kind)? {
      if in_repo(request, &entity) {
        symbol_candidate(graph, &mut scan, &entity)?;
      }
    }
  }
  let mut caveats = Vec::new();
  if wanted(request, &EntityKind::ApiEndpoint) {
    let missing = link_caveat(
      graph,
      EdgeKind::CallsEndpoint,
      "no client->endpoint links (calls_endpoint) exist in this index; every endpoint appears unconsumed",
    )?;
    let mut hidden = LinkScan::default();
    endpoint_candidates(graph, &mut scan, request, &mut hidden)?;
    caveats.extend(missing);
    caveats.extend(hidden.caveats(graph.policy.min_confidence));
  }
  if wanted(request, &EntityKind::AuthScheme) {
    let missing = link_caveat(
      graph,
      EdgeKind::UsesAuth,
      "no client auth usage facts (uses_auth) exist in this index; every auth scheme appears unused",
    )?;
    linked_candidates(
      graph,
      &mut scan,
      request,
      EntityKind::AuthScheme,
      EdgeKind::UsesAuth,
      "no client in the project sends this scheme",
    )?;
    caveats.extend(missing);
  }
  if wanted(request, &EntityKind::Event) {
    linked_candidates(
      graph,
      &mut scan,
      request,
      EntityKind::Event,
      EdgeKind::Subscribes,
      "no subscriber in the project; external consumers may exist",
    )?;
  }
  if wanted(request, &EntityKind::EnvVar) || wanted(request, &EntityKind::ConfigKey) {
    caveats.push(Caveat::Note {
      message: UNCHECKED_DEFINITIONS.to_owned(),
    });
  }
  Ok(Scanned {
    candidates: scan.candidates,
    checks: scan.checks,
    scopes: TextScopes {
      packages: scan.package_files,
      macro_files: scan.macro_files.unwrap_or_default(),
      test_code: scan.tests.code,
    },
    excluded: scan.excluded,
    caveats,
  })
}

struct UsageFilter<'g, 'a, 'b> {
  graph: &'g Graph<'a, 'b>,
  id: EntityId,
  outside: Option<EntityId>,
  overriders: Option<HashSet<EntityId>>,
}

impl<'g, 'a, 'b> UsageFilter<'g, 'a, 'b> {
  fn new(graph: &'g Graph<'a, 'b>, id: EntityId, outside: Option<EntityId>) -> Self {
    Self {
      graph,
      id,
      outside,
      overriders: None,
    }
  }

  fn counts(&mut self, link: &EdgeLink) -> QueryResult<bool> {
    let EdgeLink { kind, from, .. } = link;
    let from = *from;
    if NON_USAGE.contains(kind) || Some(from) == self.outside || from == self.id {
      return Ok(false);
    }
    if *kind == EdgeKind::Implements {
      let overriders = match &mut self.overriders {
        Some(found) => found,
        None => self.overriders.insert(override_sources(self.graph, self.id)?),
      };
      if overriders.contains(&from) {
        return Ok(false);
      }
    }
    if let Some(container) = self.outside
      && self.graph.parent(from)?.is_some_and(|parent| parent.id == container)
    {
      return Ok(false);
    }
    Ok(true)
  }
}

fn has_usage(graph: &Graph<'_, '_>, id: EntityId, outside: Option<EntityId>) -> QueryResult<bool> {
  let mut filter = UsageFilter::new(graph, id, outside);
  for edge in graph.links_in(id, &[])? {
    if filter.counts(&edge)? {
      return Ok(true);
    }
  }
  Ok(false)
}

enum Usage {
  Unreferenced,
  Live,
  TestOnly { sources: Vec<EntityRef>, dispatched_from: Option<String> },
}

#[derive(Default)]
struct Inbound {
  test_sources: Vec<EntityRef>,
  production: Option<String>,
  firm: bool,
  incidental: bool,
}

#[derive(Default)]
struct TestScope {
  modules: Vec<(RepoName, String, Vec<String>)>,
  code: TestCode,
}

impl TestScope {
  fn build(graph: &Graph<'_, '_>) -> QueryResult<Self> {
    let mut scope = Self::default();
    for module in graph.entities_of_kind(&EntityKind::Module)? {
      let Some(symbol) = module.key.as_symbol() else { continue };
      if is_test(&module) {
        scope.modules.push((symbol.repo().clone(), symbol.package().to_owned(), symbol.module_path().to_vec()));
      }
    }
    for module in graph.entities_of_kind(&EntityKind::Module)? {
      if is_test(&module) {
        scope.mark(&module);
      }
    }
    for kind in SYMBOL_KINDS.iter().chain([&EntityKind::Test, &EntityKind::TestSuite]) {
      for entity in graph.entities_of_kind(kind)? {
        if scope.contains(&entity) {
          scope.mark(&entity);
        }
      }
    }
    Ok(scope)
  }

  fn mark(&mut self, entity: &Entity) {
    let (Some(repo), Some(path)) = (entity.repo.clone(), entity.path.clone()) else { return };
    if is_test_path(path.as_str()) {
      self.code.files.insert((repo, path));
      return;
    }
    if let Some(span) = entity.span {
      self.code.ranges.entry((repo, path)).or_default().push((span.start_line, span.end_line));
    }
  }

  fn contains(&self, entity: &Entity) -> bool {
    is_test_code(entity) || self.in_cfg_test_module(entity)
  }

  fn in_cfg_test_module(&self, entity: &Entity) -> bool {
    let Some(symbol) = entity.key.as_symbol() else { return false };
    self
      .modules
      .iter()
      .any(|(repo, package, path)| repo == symbol.repo() && package == symbol.package() && symbol.module_path().starts_with(path))
  }
}

fn from_test_side(graph: &Graph<'_, '_>, tests: &TestScope, link: &EdgeLink) -> QueryResult<Option<Entity>> {
  let Some(source) = graph.entity(link.from)? else { return Ok(None) };
  if tests.contains(&source) {
    return Ok(Some(source));
  }
  let mocked = graph.edge(link.id)?.is_some_and(|edge| edge.attrs.contains_key(ATTR_MOCK));
  Ok(mocked.then_some(source))
}

fn is_own_impl_header(graph: &Graph<'_, '_>, entity: &Entity, link: &EdgeLink) -> QueryResult<bool> {
  if link.kind != EdgeKind::References || language(entity) != Some(LanguageFamily::Rust) {
    return Ok(false);
  }
  let Some(source) = graph.entity(link.from)? else { return Ok(false) };
  Ok(source.kind == EntityKind::File && source.repo == entity.repo && source.path == entity.path)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Targets {
  Own,
  Dispatched,
}

fn inbound(graph: &Graph<'_, '_>, tests: &TestScope, entity: &Entity, targets: &[(EntityId, Option<EntityId>)], kind: Targets) -> QueryResult<Inbound> {
  let mut found = Inbound::default();
  let mut seen: HashSet<EntityId> = HashSet::new();
  for &(id, outside) in targets {
    let mut filter = UsageFilter::new(graph, id, outside);
    for link in graph.links_in(id, &[])? {
      if !filter.counts(&link)? {
        continue;
      }
      if is_own_impl_header(graph, entity, &link)? {
        found.incidental = true;
        continue;
      }
      if let Some(source) = from_test_side(graph, tests, &link)? {
        if seen.insert(source.id) {
          found.test_sources.push(EntityRef::of(&source));
        }
        continue;
      }
      let label = graph.entity(link.from)?.map_or_else(|| link.from.to_string(), |source| EntityRef::of(&source).short);
      found.production.get_or_insert(label);
      if kind == Targets::Own || link.certainty_class != CertaintyClass::Heuristic {
        found.firm = true;
        return Ok(found);
      }
    }
  }
  found.test_sources.sort_by_key(EntityRef::sort_key);
  Ok(found)
}

fn usage(graph: &Graph<'_, '_>, tests: &TestScope, entity: &Entity) -> QueryResult<Usage> {
  let mut own = vec![(entity.id, None)];
  let mut dispatched = Vec::new();
  if is_container(entity) {
    for child in graph.children(entity.id)? {
      match is_trait_impl_method(&child) {
        true => dispatched.push((child.id, Some(entity.id))),
        false => own.push((child.id, Some(entity.id))),
      }
    }
  }
  let direct = inbound(graph, tests, entity, &own, Targets::Own)?;
  if direct.firm {
    return Ok(Usage::Live);
  }
  let through_traits = inbound(graph, tests, entity, &dispatched, Targets::Dispatched)?;
  if through_traits.firm {
    return Ok(Usage::Live);
  }
  if direct.test_sources.is_empty() {
    return Ok(match (through_traits.production, through_traits.test_sources.is_empty()) {
      (Some(_), _) => Usage::Live,
      (None, true) if direct.incidental || through_traits.incidental => Usage::Live,
      (None, true) => Usage::Unreferenced,
      (None, false) => Usage::TestOnly {
        sources: through_traits.test_sources,
        dispatched_from: None,
      },
    });
  }
  let mut sources = direct.test_sources;
  sources.extend(through_traits.test_sources);
  Ok(Usage::TestOnly {
    sources,
    dispatched_from: through_traits.production,
  })
}

fn is_container(entity: &Entity) -> bool {
  matches!(
    entity.kind,
    EntityKind::Class | EntityKind::Struct | EntityKind::Enum | EntityKind::Trait | EntityKind::Interface
  )
}

fn test_only_evidence(sources: &[EntityRef]) -> String {
  let covered = |source: &EntityRef| {
    source.kind == "file"
      && sources.iter().any(|other| {
        other.kind != "file"
          && other
            .location
            .as_ref()
            .zip(source.location.as_ref())
            .is_some_and(|(left, right)| (&left.repo, &left.path) == (&right.repo, &right.path))
      })
  };
  let sources: Vec<&EntityRef> = sources.iter().filter(|source| !covered(source)).collect();
  let shown: Vec<String> = sources
    .iter()
    .take(MAX_TEST_SOURCES_SHOWN)
    .map(|source| match (&source.location, source.kind.as_str()) {
      (Some(location), "file") => location.to_string(),
      (Some(location), _) => format!("{} at {location}", source.short),
      (None, _) => source.short.clone(),
    })
    .collect();
  let more = sources.len().saturating_sub(MAX_TEST_SOURCES_SHOWN);
  let tail = if more > 0 { format!(" and {more} more") } else { String::new() };
  format!("used only by tests: {}{tail}", shown.join(", "))
}

fn entrypoint(graph: &Graph<'_, '_>, liveness: &mut HashMap<EntityId, bool>, entity: &Entity) -> QueryResult<Option<&'static str>> {
  if is_test(entity) {
    return Ok(Some("test"));
  }
  if entity.name == "main" && entity.kind == EntityKind::Function {
    return Ok(Some("main entrypoint"));
  }
  if entity.kind == EntityKind::Method && entity.name == "constructor" {
    return Ok(Some("constructor"));
  }
  if !graph.edges_out(entity.id, &[EdgeKind::ExposesEndpoint])?.is_empty() {
    return Ok(Some("route handler"));
  }
  if entity.visibility == Some(Visibility::ExportedDefault) && entity.path.as_ref().is_some_and(|path| path.segments().any(|segment| ROUTE_DIRECTORIES.contains(&segment))) {
    return Ok(Some("framework route module (default export)"));
  }
  if let Some(reason) = implementation(graph, liveness, entity)? {
    return Ok(Some(reason));
  }
  let attributes = attribute_names(entity);
  match language(entity) {
    Some(LanguageFamily::Rust) if attributes.iter().any(|name| RUST_REGISTRATION_ATTRIBUTES.contains(&name.as_str())) => return Ok(Some("registered by attribute")),
    Some(LanguageFamily::Ts) if !attributes.is_empty() => return Ok(Some("decorator-registered")),
    _ => {}
  }
  if entity.kind == EntityKind::Method
    && let Some(parent) = graph.parent(entity.id)?
    && heritage_count(&parent, ATTR_IMPLEMENTS) > 0
    && has_unresolved_heritage(graph, parent)?
  {
    return Ok(Some("method of a class implementing an interface (may be framework-invoked)"));
  }
  Ok(None)
}

fn implementation(graph: &Graph<'_, '_>, liveness: &mut HashMap<EntityId, bool>, entity: &Entity) -> QueryResult<Option<&'static str>> {
  if is_trait_impl_method(entity) {
    return Ok(Some(TRAIT_IMPLEMENTATION));
  }
  let edges = graph.edges_out(entity.id, &[EdgeKind::Implements])?;
  if edges.is_empty() {
    return Ok(None);
  }
  if edges.iter().any(|edge| override_via(edge).is_none()) {
    return Ok(Some(TRAIT_IMPLEMENTATION));
  }
  let mut live = Vec::new();
  for edge in &edges {
    if is_live(graph, liveness, edge.to)? {
      live.push(edge);
    }
  }
  if live.is_empty() {
    return Ok(None);
  }
  Ok(Some(match live.iter().all(|edge| override_via(edge) == Some("class")) {
    true => "override of a base class member",
    false => TRAIT_IMPLEMENTATION,
  }))
}

fn override_via(edge: &Edge) -> Option<&str> {
  edge.attrs.get(ATTR_OVERRIDE_VIA).and_then(|via| via.as_str())
}

fn override_sources(graph: &Graph<'_, '_>, id: EntityId) -> QueryResult<HashSet<EntityId>> {
  Ok(
    graph
      .edges_in(id, &[EdgeKind::Implements])?
      .into_iter()
      .filter(|edge| override_via(edge).is_some())
      .map(|edge| edge.from)
      .collect(),
  )
}

fn is_live(graph: &Graph<'_, '_>, liveness: &mut HashMap<EntityId, bool>, member: EntityId) -> QueryResult<bool> {
  if let Some(known) = liveness.get(&member) {
    return Ok(*known);
  }
  let mut visited: HashSet<EntityId> = HashSet::new();
  let mut reached_from: HashMap<EntityId, EntityId> = HashMap::new();
  let mut pending = vec![member];
  while let Some(current) = pending.pop() {
    if !visited.insert(current) {
      continue;
    }
    if visited.len() > MAX_HERITAGE_WALK {
      liveness.insert(member, true);
      return Ok(true);
    }
    let live = match liveness.get(&current) {
      Some(known) => *known,
      None => has_usage(graph, current, None)?,
    };
    if live {
      let mut node = Some(current);
      while let Some(on_path) = node {
        liveness.insert(on_path, true);
        node = reached_from.get(&on_path).copied();
      }
      return Ok(true);
    }
    if liveness.contains_key(&current) {
      continue;
    }
    for edge in graph.edges_out(current, &[EdgeKind::Implements])? {
      if override_via(&edge).is_some() && !visited.contains(&edge.to) {
        reached_from.entry(edge.to).or_insert(current);
        pending.push(edge.to);
      }
    }
  }
  liveness.extend(visited.into_iter().map(|node| (node, false)));
  Ok(false)
}

fn heritage_count(entity: &Entity, attr: &str) -> usize {
  entity.attrs.get(attr).and_then(|value| value.as_array()).map_or(0, Vec::len)
}

fn has_unresolved_heritage(graph: &Graph<'_, '_>, class: Entity) -> QueryResult<bool> {
  let mut visited: HashSet<EntityId> = HashSet::new();
  let mut pending = vec![class];
  while let Some(current) = pending.pop() {
    if !visited.insert(current.id) {
      continue;
    }
    if visited.len() > MAX_HERITAGE_WALK || has_unlisted_extends(&current) {
      return Ok(true);
    }
    let declared = heritage_count(&current, ATTR_EXTENDS) + heritage_count(&current, ATTR_IMPLEMENTS);
    let mut resolved = 0;
    for edge in graph.edges_out(current.id, &[EdgeKind::Extends, EdgeKind::Implements])? {
      if let Some(target) = graph.entity(edge.to)?.filter(|target| matches!(target.kind, EntityKind::Class | EntityKind::Interface)) {
        resolved += 1;
        pending.push(target);
      }
    }
    if resolved < declared {
      return Ok(true);
    }
  }
  Ok(false)
}

fn has_unlisted_extends(entity: &Entity) -> bool {
  entity.kind == EntityKind::Class && heritage_count(entity, ATTR_EXTENDS) == 0 && entity.signature.as_deref().is_some_and(declares_extends)
}

fn declares_extends(signature: &str) -> bool {
  let mut depth = 0usize;
  let mut previous = ' ';
  for (index, character) in signature.char_indices() {
    match character {
      '<' | '(' | '[' | '{' => depth += 1,
      '>' if previous == '=' => {}
      '>' | ')' | ']' | '}' => depth = depth.saturating_sub(1),
      'e' if depth == 0 && !(previous.is_alphanumeric() || previous == '_' || previous == '$') => {
        let rest = &signature[index..];
        let ends_word = rest.chars().nth("extends".len()).is_none_or(|next| !(next.is_alphanumeric() || next == '_' || next == '$'));
        if rest.starts_with("extends") && ends_word {
          return true;
        }
      }
      _ => {}
    }
    previous = character;
  }
  false
}

fn is_trait_impl_method(entity: &Entity) -> bool {
  entity
    .attrs
    .get(ATTR_IMPL_OF)
    .and_then(|value| serde_json::from_value::<ImplInfo>(value.clone()).ok())
    .is_some_and(|info| info.trait_ref.is_some())
}

fn symbol_candidate(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, entity: &Entity) -> QueryResult<()> {
  let test_only = match usage(graph, &scan.tests, entity)? {
    Usage::Live => return Ok(()),
    Usage::TestOnly { .. } if scan.tests.contains(entity) => return Ok(()),
    Usage::TestOnly { sources, dispatched_from } => Some((sources, dispatched_from)),
    Usage::Unreferenced => None,
  };
  if let Some(reason) = entrypoint(graph, &mut scan.liveness, entity)? {
    let instantiable = test_only.is_some() && is_container(entity) && reason == TRAIT_IMPLEMENTATION;
    if !instantiable {
      if test_only.is_none() {
        *scan.excluded.entry(reason.to_owned()).or_default() += 1;
      }
      return Ok(());
    }
  }
  if let Some((sources, dispatched_from)) = test_only {
    return test_only_candidate(graph, scan, entity, &sources, dispatched_from);
  }
  let mut evidence = vec!["no inbound usage edges at any certainty".to_owned()];
  let mut class = UnusedClass::LikelyUnused;
  for reason in [public_surface(graph, scan, entity)?, runtime_hooks(graph, entity)?].into_iter().flatten() {
    evidence.push(reason);
    class = UnusedClass::PossiblyUnused;
  }
  if let Some(reason) = unresolved_evidence(graph, entity)? {
    evidence.push(reason);
    class = UnusedClass::PossiblyUnused;
  }
  for reason in dynamic_risk(graph, scan, entity)? {
    evidence.push(reason);
    class = UnusedClass::PossiblyUnused;
  }
  if class == UnusedClass::LikelyUnused {
    evidence.push(likely_reason(graph, scan, entity)?);
    match text_check(graph, scan, entity, false)? {
      Some(check) => scan.checks.push(check),
      None => {
        class = UnusedClass::PossiblyUnused;
        evidence.push("no source location to verify textually".to_owned());
      }
    }
  }
  scan.candidates.push(UnusedCandidate {
    entity: EntityRef::of(entity),
    class,
    evidence,
  });
  Ok(())
}

fn unresolved_evidence(graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<Option<String>> {
  let Some(Caveat::UnresolvedRisk { total, by_reason, .. }) = unresolved_risk(graph, entity.id, &entity.name)? else {
    return Ok(None);
  };
  let reasons: Vec<String> = by_reason.iter().map(|(reason, count)| format!("{count} {reason}")).collect();
  Ok(Some(format!(
    "{} named {:?} could reach it ({})",
    crate::text::counted(total, "unresolved reference"),
    entity.name,
    reasons.join(", ")
  )))
}

struct Reach {
  packages: Vec<EntityId>,
}

impl Reach {
  fn covers(&self, scan: &Scan<'_>, file: &FileKey) -> bool {
    self.packages.is_empty()
      || self
        .packages
        .iter()
        .any(|package| scan.package_file_sets.get(package).is_some_and(|files| files.contains(file)))
  }

  fn production_site(&self, scan: &Scan<'_>, repo: &RepoName, path: &RelPath, line: u32) -> bool {
    let file = (repo.clone(), path.clone());
    self.covers(scan, &file) && !scan.tests.code.contains(&file, line)
  }
}

fn reach_of(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, entity: &Entity) -> QueryResult<Reach> {
  let Some(package) = package_scope(graph, scan, entity)? else {
    return Ok(Reach { packages: Vec::new() });
  };
  if scan.package_files.get(&package).is_none_or(Vec::is_empty) {
    return Ok(Reach { packages: Vec::new() });
  }
  let mut packages = vec![package];
  packages.extend(dependent_scopes(graph, scan, package)?);
  for package in &packages {
    if !scan.package_file_sets.contains_key(package) {
      let files = scan.package_files.get(package).map(|files| files.iter().cloned().collect()).unwrap_or_default();
      scan.package_file_sets.insert(*package, files);
    }
  }
  Ok(Reach { packages })
}

fn production_unresolved(graph: &Graph<'_, '_>, scan: &Scan<'_>, reach: &Reach, entity: &Entity) -> QueryResult<bool> {
  if entity.name.is_empty() {
    return Ok(false);
  }
  for found in graph.tx().unresolved_by_names(&[entity.name.as_str()])? {
    let reference = found.reference;
    if risk_reason(&reference, entity.id).is_some() && reach.production_site(scan, &reference.repo, &reference.file, reference.span.start_line) {
      return Ok(true);
    }
  }
  Ok(false)
}

fn production_dynamic_member_calls(graph: &Graph<'_, '_>, scan: &Scan<'_>, reach: &Reach, entity: &Entity) -> QueryResult<bool> {
  if language(entity) != Some(LanguageFamily::Ts) || entity.key.as_symbol().is_none() || !entity.kind.is_code_symbol() || !graph.may_have_dynamic_sites()? {
    return Ok(false);
  }
  for (repo, path) in dynamic_member_files(graph, entity)? {
    for site in graph.dynamic_sites(&repo, &path)?.iter() {
      if site.location.line.is_some_and(|line| reach.production_site(scan, &repo, &path, line)) {
        return Ok(true);
      }
    }
  }
  Ok(false)
}

fn test_only_candidate(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, entity: &Entity, sources: &[EntityRef], dispatched_from: Option<String>) -> QueryResult<()> {
  let reach = reach_of(graph, scan, entity)?;
  if production_unresolved(graph, scan, &reach, entity)? || production_dynamic_member_calls(graph, scan, &reach, entity)? {
    return Ok(());
  }
  let mut evidence = vec![test_only_evidence(sources)];
  evidence.extend(dispatched_from.map(|caller| format!("its trait methods are called from production code through the trait (e.g. {caller}); that call may dispatch to it")));
  evidence.extend(public_surface(graph, scan, entity)?);
  evidence.extend(dynamic_import_risk(graph, scan, entity)?);
  if let Some(check) = text_check(graph, scan, entity, true)? {
    scan.checks.push(check);
  }
  scan.candidates.push(UnusedCandidate {
    entity: EntityRef::of(entity),
    class: UnusedClass::TestOnly,
    evidence,
  });
  Ok(())
}

fn text_check(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, entity: &Entity, skip_test_code: bool) -> QueryResult<Option<TextCheck>> {
  let (Some(repo), Some(path)) = (entity.repo.clone(), entity.path.clone()) else {
    return Ok(None);
  };
  let package = package_scope(graph, scan, entity)?;
  let dependents = match (skip_test_code, package) {
    (true, Some(package)) => dependent_scopes(graph, scan, package)?,
    _ => Vec::new(),
  };
  macro_files(graph, scan)?;
  let owner = match entity.kind {
    EntityKind::Method => graph.parent(entity.id)?.map(|parent| parent.name),
    _ => None,
  };
  Ok(Some(TextCheck {
    index: scan.candidates.len(),
    name: entity.name.clone(),
    owner,
    file: (repo, path),
    span: entity.span.map_or((0, 0), |span| (span.start_line, span.end_line)),
    package,
    dependents,
    skip_test_code,
  }))
}

fn repo_files<'s>(graph: &Graph<'_, '_>, scan: &'s mut Scan<'_>, repo: &RepoName) -> QueryResult<&'s Vec<RelPath>> {
  if !scan.repo_files.contains_key(repo) {
    let files = graph
      .tx()
      .files_in_repo(repo)?
      .into_iter()
      .filter(|file| file.language.is_some())
      .map(|file| file.path)
      .collect();
    scan.repo_files.insert(repo.clone(), files);
  }
  Ok(&scan.repo_files[repo])
}

fn package_scope(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, entity: &Entity) -> QueryResult<Option<EntityId>> {
  let Some(key) = package_key(entity) else { return Ok(None) };
  let id = EntityId::of(&key);
  load_package_files(graph, scan, id)?;
  Ok(Some(id))
}

fn dependent_scopes(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, package: EntityId) -> QueryResult<Vec<EntityId>> {
  let mut dependents = Vec::new();
  for edge in graph.edges_in(package, &[EdgeKind::DependsOnPackage])? {
    if edge.from != package && graph.entity(edge.from)?.is_some_and(|dependent| dependent.kind == EntityKind::Package) {
      load_package_files(graph, scan, edge.from)?;
      dependents.push(edge.from);
    }
  }
  Ok(dependents)
}

fn load_package_files(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, id: EntityId) -> QueryResult<()> {
  if scan.package_files.contains_key(&id) {
    return Ok(());
  }
  let mut files = Vec::new();
  if let Some(package) = graph.entity(id)?
    && let (Some(repo), Some(manifest)) = (attr_str(&package, "repo"), attr_str(&package, "manifest"))
    && let Ok(repo) = RepoName::new(repo)
  {
    let directory = manifest.rsplit_once('/').map(|(directory, _)| format!("{directory}/"));
    for path in repo_files(graph, scan, &repo)? {
      if directory.as_deref().is_none_or(|directory| path.as_str().starts_with(directory)) {
        files.push((repo.clone(), path.clone()));
      }
    }
  }
  scan.package_files.insert(id, files);
  Ok(())
}

fn macro_files(graph: &Graph<'_, '_>, scan: &mut Scan<'_>) -> QueryResult<()> {
  if scan.macro_files.is_some() {
    return Ok(());
  }
  let mut files = BTreeSet::new();
  for entity in graph.entities_of_kind(&EntityKind::Macro)? {
    if let (Some(repo), Some(path)) = (entity.repo, entity.path) {
      files.insert((repo, path));
    }
  }
  for found in graph.tx().unresolved_by_names(&["include"])? {
    files.insert((found.reference.repo, found.reference.file));
  }
  scan.macro_files = Some(files);
  Ok(())
}

fn dynamic_sites(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, repo: &RepoName) -> QueryResult<DynamicSites> {
  if let Some(sites) = scan.dynamic.get(repo) {
    return Ok(sites.clone());
  }
  let indexed: HashSet<RelPath> = repo_files(graph, scan, repo)?.iter().cloned().collect();
  let mut sites = DynamicSites::default();
  for found in graph.tx().unresolved_in_repo(repo)? {
    let Owner::File { path, .. } = &found.owner else { continue };
    let reference = &found.reference;
    if reference.reason != UnresolvedReason::Dynamic || !indexed.contains(path) {
      continue;
    }
    if matches!(reference.path.first().map(String::as_str), Some("import" | "require")) {
      sites.imports.push(format!("{repo}:{path}:{}", reference.span.start_line));
    }
  }
  scan.dynamic.insert(repo.clone(), sites.clone());
  Ok(sites)
}

fn dynamic_import_risk(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, entity: &Entity) -> QueryResult<Option<String>> {
  if language(entity) != Some(LanguageFamily::Ts) {
    return Ok(None);
  }
  let Some(repo) = entity.repo.clone() else { return Ok(None) };
  let sites = dynamic_sites(graph, scan, &repo)?;
  let Some(site) = sites.imports.first() else { return Ok(None) };
  if !externally_visible(graph, entity)? {
    return Ok(None);
  }
  Ok(Some(format!(
    "{} dynamic import()/require() calls in the repo (first at {site}) may load its module",
    sites.imports.len()
  )))
}

fn dynamic_member_risk(graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<Option<String>> {
  if language(entity) != Some(LanguageFamily::Ts) {
    return Ok(None);
  }
  let Some(Caveat::DynamicMemberCalls { total, .. }) = dynamic_member_calls(graph, entity)? else {
    return Ok(None);
  };
  Ok(Some(format!(
    "{} could reach this (computed property access)",
    crate::text::counted(total, "dynamic member call")
  )))
}

fn dynamic_risk(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, entity: &Entity) -> QueryResult<Vec<String>> {
  Ok(
    [dynamic_import_risk(graph, scan, entity)?, dynamic_member_risk(graph, entity)?]
      .into_iter()
      .flatten()
      .collect(),
  )
}

fn externally_visible(graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<bool> {
  let visible = entity.visibility.as_ref().is_some_and(Visibility::is_externally_visible);
  if !visible || entity.kind != EntityKind::Method {
    return Ok(visible);
  }
  Ok(
    graph
      .parent(entity.id)?
      .is_some_and(|parent| parent.visibility.as_ref().is_some_and(Visibility::is_externally_visible)),
  )
}

fn public_surface(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, entity: &Entity) -> QueryResult<Option<String>> {
  if !externally_visible(graph, entity)? {
    return Ok(None);
  }
  if !graph.edges_in(entity.id, &[EdgeKind::ReExports, EdgeKind::Exports])?.is_empty() {
    return Ok(Some("exported from its package entry point; consumers outside the project may exist".to_owned()));
  }
  let Some(symbol) = entity.key.as_symbol() else { return Ok(None) };
  if symbol.family() == LanguageFamily::Rust {
    return Ok(match symbol.package().contains('/') {
      true => None,
      false => Some(format!("pub item of library crate {}; other crates may use it", symbol.package())),
    });
  }
  match package_role(graph, scan, entity)? {
    Some(PackageRole::Library { reason }) => Ok(Some(format!("public API of a library package ({reason}); consumers outside the project may exist"))),
    _ => Ok(None),
  }
}

fn runtime_hooks(graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<Option<String>> {
  let mut events = Vec::new();
  for edge in graph.edges_out(entity.id, &[EdgeKind::Subscribes])? {
    events.extend(graph.entity(edge.to)?.map(|event| event.name));
  }
  if events.is_empty() {
    return Ok(None);
  }
  Ok(Some(format!("subscribes to {}; a message runtime may invoke it", events.join(", "))))
}

fn likely_reason(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, entity: &Entity) -> QueryResult<String> {
  if !externally_visible(graph, entity)? {
    return Ok("not visible outside its module".to_owned());
  }
  Ok(match package_role(graph, scan, entity)? {
    Some(PackageRole::Application { name }) => format!("exported only within application package {name}"),
    _ => "visible only inside its binary target".to_owned(),
  })
}

fn package_role(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, entity: &Entity) -> QueryResult<Option<PackageRole>> {
  let Some(key) = package_key(entity) else { return Ok(None) };
  let id = EntityId::of(&key);
  if let Some(role) = scan.packages.get(&id) {
    return Ok(Some(role.clone()));
  }
  let Some(package) = graph.entity(id)? else { return Ok(None) };
  let mut dependents = Vec::new();
  for edge in graph.edges_in(id, &[EdgeKind::DependsOnPackage])? {
    if let Some(dependent) = graph.entity(edge.from)?.filter(|dependent| dependent.kind == EntityKind::Package) {
      dependents.push(dependent.name);
    }
  }
  let application = !graph.edges_out(id, &[EdgeKind::PartOfApp])?.is_empty() || scan.private.is_some_and(|private| private.contains(&id));
  let role = match (dependents.is_empty(), application) {
    (false, _) => PackageRole::Library {
      reason: format!("{} is depended on by {}", package.name, dependents.join(", ")),
    },
    (true, false) => PackageRole::Library {
      reason: format!("{} has no detected application", package.name),
    },
    (true, true) => PackageRole::Application { name: package.name.clone() },
  };
  scan.packages.insert(id, role.clone());
  Ok(Some(role))
}

fn endpoint_candidates(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, request: &UnusedRequest, hidden: &mut LinkScan) -> QueryResult<()> {
  let superseded = superseded_fragments(graph)?;
  for endpoint in graph.entities_of_kind(&EntityKind::ApiEndpoint)? {
    if !in_repo(request, &endpoint) || !graph.edges_in(endpoint.id, &[EdgeKind::CallsEndpoint])?.is_empty() {
      continue;
    }
    let fragment = is_router_relative(&endpoint);
    if fragment && superseded.contains(&endpoint.key.to_string()) {
      continue;
    }
    let mut evidence = vec!["no detected consumers in project".to_owned(), "external clients outside the project may exist".to_owned()];
    if fragment {
      evidence.insert(0, "router fragment; mount not detected".to_owned());
    }
    evidence.extend(inferred_consumer_evidence(graph, &endpoint, hidden)?);
    scan.candidates.push(UnusedCandidate {
      entity: EntityRef::of(&endpoint),
      class: UnusedClass::PossiblyUnused,
      evidence,
    });
  }
  Ok(())
}

fn inferred_consumer_evidence(graph: &Graph<'_, '_>, endpoint: &Entity, hidden: &mut LinkScan) -> QueryResult<Vec<String>> {
  let mut evidence = Vec::new();
  for link in inferred_links(graph, endpoint.id, LinkEnd::Endpoint, hidden)? {
    let call = EntityRef::of(&link.call);
    for request in graph.edges_in(link.call.id, &[EdgeKind::MakesRequest])? {
      let Some(caller) = graph.entity(request.from)? else { continue };
      evidence.push(format!("likely consumer {} via {} {}", EntityRef::of(&caller).short, call.short, link.hop.certainty.tag));
    }
  }
  Ok(evidence)
}

fn linked_candidates(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, request: &UnusedRequest, kind: EntityKind, usage: EdgeKind, reason: &str) -> QueryResult<()> {
  for entity in graph.entities_of_kind(&kind)? {
    if !in_repo(request, &entity) || !graph.edges_in(entity.id, std::slice::from_ref(&usage))?.is_empty() {
      continue;
    }
    scan.candidates.push(UnusedCandidate {
      entity: EntityRef::of(&entity),
      class: UnusedClass::PossiblyUnused,
      evidence: vec![reason.to_owned()],
    });
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::declares_extends;

  #[test]
  fn should_find_only_a_top_level_extends_clause_in_a_class_signature() {
    assert!(declares_extends("export class JwtGuard extends AuthGuard('jwt') implements Named"));
    assert!(declares_extends("class Box<F = () => void> extends mixin(Base)"));
    assert!(!declares_extends("class Box<T extends Base<U>> implements Named"));
    assert!(!declares_extends("class Extendsable implements extendsLike"));
    assert!(!declares_extends("class Plain"));
  }
}
