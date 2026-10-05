use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use atlas_core::{
  ATTR_EXTENDS, ATTR_IMPL_OF, ATTR_IMPLEMENTS, ATTR_OVERRIDE_VIA, Edge, EdgeKind, Entity, EntityId, EntityKind, ImplInfo, LanguageFamily, Owner, RelPath, RepoName,
  UnresolvedReason, Visibility,
};
use serde::Serialize;

use crate::QueryEngine;
use crate::dynamic::dynamic_member_calls;
use crate::engine::unresolved_risk;
use crate::error::QueryResult;
use crate::graph::{FileKey, Graph};
use crate::model::{Caveat, EntityRef, Listing, Response};
use crate::policy::Policy;
use crate::queries::common::{DEFAULT_LIMIT, attr_str, attribute_names, is_router_relative, is_test, language, link_caveat, package_key, superseded_fragments};
use crate::queries::inferred_links::{LinkEnd, LinkScan, inferred_links};
use crate::queries::textual::{TextCheck, TextIndex, TextScopes};

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
    for check in &checks {
      let Some(candidate) = candidates.get_mut(check.index) else { continue };
      if let Some(reason) = index.verify(check) {
        candidate.class = UnusedClass::PossiblyUnused;
        candidate.evidence.push(reason);
      }
    }
    caveats.extend(self.problem_files(freshness.run)?);
    let (mut likely, mut possibly): (Vec<UnusedCandidate>, Vec<UnusedCandidate>) = candidates.into_iter().partition(|candidate| candidate.class == UnusedClass::LikelyUnused);
    likely.sort_by_key(|candidate| candidate.entity.sort_key());
    possibly.sort_by_key(|candidate| candidate.entity.sort_key());
    let answer = UnusedAnswer {
      likely_unused: Listing::truncated(likely, request.limit),
      possibly_unused: Listing::truncated(possibly, request.limit),
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
  repo_files: HashMap<RepoName, Vec<RelPath>>,
  macro_files: Option<BTreeSet<FileKey>>,
  dynamic: HashMap<RepoName, DynamicSites>,
  private: Option<&'p BTreeSet<EntityId>>,
  liveness: HashMap<EntityId, bool>,
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
  if wanted(request, &EntityKind::EnvVar) {
    caveats.push(Caveat::MissingFacts {
      detail: "env var definitions (.env, compose, helm) are not indexed; unused env vars cannot be detected".to_owned(),
    });
  }
  Ok(Scanned {
    candidates: scan.candidates,
    checks: scan.checks,
    scopes: TextScopes {
      packages: scan.package_files,
      macro_files: scan.macro_files.unwrap_or_default(),
    },
    excluded: scan.excluded,
    caveats,
  })
}

fn has_usage(graph: &Graph<'_, '_>, id: EntityId, outside: Option<EntityId>) -> QueryResult<bool> {
  let mut overriders: Option<HashSet<EntityId>> = None;
  for edge in graph.links_in(id, &[])? {
    if NON_USAGE.contains(&edge.kind) || Some(edge.from) == outside || edge.from == id {
      continue;
    }
    if edge.kind == EdgeKind::Implements {
      let overriders = match &mut overriders {
        Some(found) => found,
        None => overriders.insert(override_sources(graph, id)?),
      };
      if overriders.contains(&edge.from) {
        continue;
      }
    }
    if let Some(container) = outside
      && graph.parent(edge.from)?.is_some_and(|parent| parent.id == container)
    {
      continue;
    }
    return Ok(true);
  }
  Ok(false)
}

fn used(graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<bool> {
  if has_usage(graph, entity.id, None)? {
    return Ok(true);
  }
  if !matches!(
    entity.kind,
    EntityKind::Class | EntityKind::Struct | EntityKind::Enum | EntityKind::Trait | EntityKind::Interface
  ) {
    return Ok(false);
  }
  for child in graph.children(entity.id)? {
    if has_usage(graph, child.id, Some(entity.id))? {
      return Ok(true);
    }
  }
  Ok(false)
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
    return Ok(Some("trait or interface implementation"));
  }
  let edges = graph.edges_out(entity.id, &[EdgeKind::Implements])?;
  if edges.is_empty() {
    return Ok(None);
  }
  if edges.iter().any(|edge| override_via(edge).is_none()) {
    return Ok(Some("trait or interface implementation"));
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
    false => "trait or interface implementation",
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
  if used(graph, entity)? {
    return Ok(());
  }
  if let Some(reason) = entrypoint(graph, &mut scan.liveness, entity)? {
    *scan.excluded.entry(reason.to_owned()).or_default() += 1;
    return Ok(());
  }
  let mut evidence = vec!["no inbound usage edges at any certainty".to_owned()];
  let mut class = UnusedClass::LikelyUnused;
  for reason in [public_surface(graph, scan, entity)?, runtime_hooks(graph, entity)?].into_iter().flatten() {
    evidence.push(reason);
    class = UnusedClass::PossiblyUnused;
  }
  if let Some(Caveat::UnresolvedRisk { total, by_reason, .. }) = unresolved_risk(graph, entity.id, &entity.name)? {
    let reasons: Vec<String> = by_reason.iter().map(|(reason, count)| format!("{count} {reason}")).collect();
    evidence.push(format!(
      "{} named {:?} could reach it ({})",
      crate::text::counted(total, "unresolved reference"),
      entity.name,
      reasons.join(", ")
    ));
    class = UnusedClass::PossiblyUnused;
  }
  for reason in dynamic_risk(graph, scan, entity)? {
    evidence.push(reason);
    class = UnusedClass::PossiblyUnused;
  }
  if class == UnusedClass::LikelyUnused {
    evidence.push(likely_reason(graph, scan, entity)?);
    match text_check(graph, scan, entity)? {
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

fn text_check(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, entity: &Entity) -> QueryResult<Option<TextCheck>> {
  let (Some(repo), Some(path)) = (entity.repo.clone(), entity.path.clone()) else {
    return Ok(None);
  };
  let package = package_scope(graph, scan, entity)?;
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
  if scan.package_files.contains_key(&id) {
    return Ok(Some(id));
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
  Ok(Some(id))
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

fn dynamic_risk(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, entity: &Entity) -> QueryResult<Vec<String>> {
  let mut reasons = Vec::new();
  if language(entity) != Some(LanguageFamily::Ts) {
    return Ok(reasons);
  }
  let Some(repo) = entity.repo.clone() else { return Ok(reasons) };
  let sites = dynamic_sites(graph, scan, &repo)?;
  if let Some(site) = sites.imports.first()
    && externally_visible(graph, entity)?
  {
    reasons.push(format!(
      "{} dynamic import()/require() calls in the repo (first at {site}) may load its module",
      sites.imports.len()
    ));
  }
  if let Some(Caveat::DynamicMemberCalls { total, .. }) = dynamic_member_calls(graph, entity)? {
    reasons.push(format!(
      "{} could reach this (computed property access)",
      crate::text::counted(total, "dynamic member call")
    ));
  }
  Ok(reasons)
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
