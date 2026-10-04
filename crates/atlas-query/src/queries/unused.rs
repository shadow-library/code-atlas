use std::collections::{BTreeMap, BTreeSet, HashMap};

use atlas_core::{ATTR_IMPL_OF, ATTR_IMPLEMENTS, EdgeKind, Entity, EntityId, EntityKind, ImplInfo, LanguageFamily, Owner, RelPath, RepoName, UnresolvedReason, Visibility};
use atlas_store::{FreshnessPolicy, InferredQuery};
use serde::Serialize;

use crate::QueryEngine;
use crate::dynamic::dynamic_member_calls;
use crate::engine::unresolved_risk;
use crate::error::QueryResult;
use crate::graph::Graph;
use crate::model::{Caveat, EntityRef, Listing, Response};
use crate::policy::Policy;
use crate::queries::common::{DEFAULT_LIMIT, attr_str, attribute_names, is_router_relative, is_test, language, link_caveat, package_key, superseded_fragments};

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
    let (mut candidates, checks, excluded_entrypoints, mut caveats) = self.read(Policy::with_floor(0.0), |graph| unused(graph, request, &private))?;
    let mut texts = TextCache::default();
    for check in &checks {
      let Some(candidate) = candidates.get_mut(check.index) else { continue };
      if let Some(reason) = self.verify_textually(check, &mut texts) {
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

  fn verify_textually(&self, check: &TextCheck, texts: &mut TextCache) -> Option<String> {
    let mut unreadable = Vec::new();
    let mut hits: Vec<String> = Vec::new();
    let mut macro_hits: Vec<String> = Vec::new();
    for (repo, path) in &check.scope {
      let in_macro_file = check.macro_files.contains(&(repo.clone(), path.clone()));
      let Some(text) = texts.get(self, repo, path) else {
        unreadable.push(format!("{repo}:{path}"));
        continue;
      };
      let own_file = *repo == check.repo && *path == check.path;
      for (index, line) in text.lines().enumerate() {
        let number = index as u32 + 1;
        if own_file && (check.span.0..=check.span.1).contains(&number) {
          continue;
        }
        let location = format!("{repo}:{path}:{number}");
        if in_macro_file && call_occurrences(line, &check.name) > 0 {
          macro_hits.push(location.clone());
        }
        let found = match (&check.owner, COMMON_NAMES.contains(&check.name.as_str())) {
          (_, false) => word_occurrences(line, &check.name),
          (Some(owner), true) => qualified_occurrences(line, owner, &check.name, own_file),
          (None, true) => 0,
        };
        if found > 0 {
          hits.push(location);
        }
      }
    }
    if let Some(location) = macro_hits.first() {
      return Some(format!(
        "called by name inside a macro_rules!/include! file ({location}); macro-expanded callers are not indexed"
      ));
    }
    if let Some(location) = hits.first() {
      let form = match COMMON_NAMES.contains(&check.name.as_str()) {
        true => "its qualified name",
        false => "its name",
      };
      return Some(format!(
        "{form} occurs {}x in its package (first at {location}) although no reference was recorded",
        hits.len()
      ));
    }
    if !unreadable.is_empty() {
      return Some(format!(
        "{} files in scope could not be read for a textual check (first: {})",
        unreadable.len(),
        unreadable[0]
      ));
    }
    None
  }
}

#[derive(Default)]
struct TextCache {
  texts: HashMap<(RepoName, RelPath), Option<String>>,
}

impl TextCache {
  fn get(&mut self, query: &QueryEngine, repo: &RepoName, path: &RelPath) -> Option<&str> {
    self
      .texts
      .entry((repo.clone(), path.clone()))
      .or_insert_with(|| query.engine().read_source(repo, path).ok().map(|(text, _)| text))
      .as_deref()
  }
}

struct TextCheck {
  index: usize,
  name: String,
  owner: Option<String>,
  repo: RepoName,
  path: RelPath,
  span: (u32, u32),
  scope: Vec<(RepoName, RelPath)>,
  macro_files: BTreeSet<(RepoName, RelPath)>,
}

const COMMON_NAMES: [&str; 64] = [
  "new",
  "get",
  "set",
  "run",
  "default",
  "from",
  "into",
  "len",
  "is_empty",
  "clone",
  "fmt",
  "build",
  "parse",
  "name",
  "id",
  "with",
  "add",
  "insert",
  "remove",
  "update",
  "find",
  "apply",
  "init",
  "start",
  "stop",
  "next",
  "load",
  "save",
  "read",
  "write",
  "open",
  "close",
  "handle",
  "process",
  "call",
  "execute",
  "create",
  "delete",
  "list",
  "map",
  "value",
  "key",
  "data",
  "to_string",
  "as_str",
  "eq",
  "hash",
  "drop",
  "deref",
  "index",
  "iter",
  "push",
  "pop",
  "contains",
  "reset",
  "clear",
  "validate",
  "check",
  "render",
  "emit",
  "send",
  "toString",
  "then",
  "on",
];

fn is_word(c: char) -> bool {
  c.is_alphanumeric() || c == '_' || c == '$'
}

fn bounded(line: &str, word: &str) -> Vec<usize> {
  if word.is_empty() {
    return Vec::new();
  }
  line
    .match_indices(word)
    .filter(|(start, _)| {
      let before = line[..*start].chars().next_back();
      let after = line[start + word.len()..].chars().next();
      !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
    .map(|(start, _)| start)
    .collect()
}

fn word_occurrences(line: &str, word: &str) -> usize {
  bounded(line, word).len()
}

fn call_occurrences(line: &str, word: &str) -> usize {
  bounded(line, word)
    .into_iter()
    .filter(|start| {
      let before = line[..*start].trim_end();
      let after = line[start + word.len()..].trim_start();
      let definition = before.ends_with("fn") || before.ends_with("function");
      after.starts_with('(') && !definition
    })
    .count()
}

fn qualified_occurrences(line: &str, owner: &str, name: &str, own_file: bool) -> usize {
  let mut receivers = vec![format!("{owner}::"), format!("{owner}.")];
  if own_file {
    receivers.extend(["Self::".to_owned(), "self.".to_owned(), "this.".to_owned()]);
  }
  bounded(line, name)
    .into_iter()
    .filter(|start| receivers.iter().any(|receiver| line[..*start].ends_with(receiver.as_str())))
    .count()
}

#[derive(Default)]
struct Scan<'p> {
  candidates: Vec<UnusedCandidate>,
  checks: Vec<TextCheck>,
  excluded: BTreeMap<String, usize>,
  packages: HashMap<EntityId, PackageRole>,
  package_files: HashMap<EntityId, Vec<(RepoName, RelPath)>>,
  repo_files: HashMap<RepoName, Vec<RelPath>>,
  macro_files: Option<BTreeSet<(RepoName, RelPath)>>,
  dynamic: HashMap<RepoName, DynamicSites>,
  private: Option<&'p BTreeSet<EntityId>>,
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

type Scanned = (Vec<UnusedCandidate>, Vec<TextCheck>, BTreeMap<String, usize>, Vec<Caveat>);

fn unused(graph: &Graph<'_, '_>, request: &UnusedRequest, private: &BTreeSet<EntityId>) -> QueryResult<Scanned> {
  let mut scan = Scan {
    private: Some(private),
    ..Scan::default()
  };
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
    endpoint_candidates(graph, &mut scan, request)?;
    caveats.extend(missing);
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
  Ok((scan.candidates, scan.checks, scan.excluded, caveats))
}

fn has_usage(graph: &Graph<'_, '_>, id: EntityId, outside: Option<EntityId>) -> QueryResult<bool> {
  for edge in graph.edges_in(id, &[])? {
    if NON_USAGE.contains(&edge.kind) || Some(edge.from) == outside || edge.from == id {
      continue;
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

fn entrypoint(graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<Option<&'static str>> {
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
  if !graph.edges_out(entity.id, &[EdgeKind::Implements])?.is_empty() || is_trait_impl_method(entity) {
    return Ok(Some("trait or interface implementation"));
  }
  let attributes = attribute_names(entity);
  match language(entity) {
    Some(LanguageFamily::Rust) if attributes.iter().any(|name| RUST_REGISTRATION_ATTRIBUTES.contains(&name.as_str())) => return Ok(Some("registered by attribute")),
    Some(LanguageFamily::Ts) if !attributes.is_empty() => return Ok(Some("decorator-registered")),
    _ => {}
  }
  if entity.kind == EntityKind::Method
    && let Some(parent) = graph.parent(entity.id)?
    && parent
      .attrs
      .get(ATTR_IMPLEMENTS)
      .is_some_and(|implements| implements.as_array().is_some_and(|list| !list.is_empty()))
  {
    return Ok(Some("method of a class implementing an interface (may be framework-invoked)"));
  }
  Ok(None)
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
  if let Some(reason) = entrypoint(graph, entity)? {
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
    evidence.push(format!("{total} unresolved references named {:?} could reach it ({})", entity.name, reasons.join(", ")));
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
  let mut scope: BTreeSet<(RepoName, RelPath)> = package_scope(graph, scan, entity)?.into_iter().collect();
  scope.insert((repo.clone(), path.clone()));
  let macro_files = macro_files(graph, scan)?.clone();
  scope.extend(macro_files.iter().cloned());
  let owner = match entity.kind {
    EntityKind::Method => graph.parent(entity.id)?.map(|parent| parent.name),
    _ => None,
  };
  Ok(Some(TextCheck {
    index: scan.candidates.len(),
    name: entity.name.clone(),
    owner,
    repo,
    path,
    span: entity.span.map_or((0, 0), |span| (span.start_line, span.end_line)),
    scope: scope.into_iter().collect(),
    macro_files,
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

fn package_scope(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, entity: &Entity) -> QueryResult<Vec<(RepoName, RelPath)>> {
  let Some(key) = package_key(entity) else { return Ok(Vec::new()) };
  let id = EntityId::of(&key);
  if let Some(files) = scan.package_files.get(&id) {
    return Ok(files.clone());
  }
  let mut files = Vec::new();
  if let Some(package) = graph.entity(id)?
    && let (Some(repo), Some(manifest)) = (attr_str(&package, "repo"), attr_str(&package, "manifest"))
    && let Ok(repo) = RepoName::new(repo)
  {
    let directory = manifest.rsplit_once('/').map(|(directory, _)| format!("{directory}/"));
    for path in repo_files(graph, scan, &repo)?.clone() {
      if directory.as_deref().is_none_or(|directory| path.as_str().starts_with(directory)) {
        files.push((repo.clone(), path));
      }
    }
  }
  scan.package_files.insert(id, files.clone());
  Ok(files)
}

fn macro_files<'s>(graph: &Graph<'_, '_>, scan: &'s mut Scan<'_>) -> QueryResult<&'s BTreeSet<(RepoName, RelPath)>> {
  if scan.macro_files.is_none() {
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
  }
  Ok(scan.macro_files.get_or_insert_with(BTreeSet::new))
}

fn dynamic_sites(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, repo: &RepoName) -> QueryResult<DynamicSites> {
  if let Some(sites) = scan.dynamic.get(repo) {
    return Ok(sites.clone());
  }
  let mut sites = DynamicSites::default();
  for path in repo_files(graph, scan, repo)?.clone() {
    let facts = graph.tx().owned_facts(&Owner::File {
      repo: repo.clone(),
      path: path.clone(),
    })?;
    for reference in facts.unresolved.iter().filter(|reference| reference.reason == UnresolvedReason::Dynamic) {
      let head = reference.path.first().map(String::as_str);
      if matches!(head, Some("import" | "require")) {
        sites.imports.push(format!("{repo}:{path}:{}", reference.span.start_line));
      }
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
    reasons.push(format!("{total} dynamic member calls could reach this (computed property access)"));
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

fn endpoint_candidates(graph: &Graph<'_, '_>, scan: &mut Scan<'_>, request: &UnusedRequest) -> QueryResult<()> {
  let superseded = superseded_fragments(graph)?;
  for endpoint in graph.entities_of_kind(&EntityKind::ApiEndpoint)? {
    if !in_repo(request, &endpoint) || !graph.edges_in(endpoint.id, &[EdgeKind::CallsEndpoint])?.is_empty() {
      continue;
    }
    let fragment = is_router_relative(&endpoint);
    if fragment && superseded.contains(&endpoint.key.to_string()) {
      continue;
    }
    let inferred = graph.tx().inferred_facts(&InferredQuery {
      subject: Some(endpoint.id),
      relation: Some(atlas_semantic::RELATION_CALLS_ENDPOINT.to_owned()),
      freshness: FreshnessPolicy::IncludeStale,
    })?;
    if inferred.iter().any(atlas_semantic::included_in_default_results) {
      continue;
    }
    let mut evidence = vec!["no detected consumers in project".to_owned(), "external clients outside the project may exist".to_owned()];
    if fragment {
      evidence.insert(0, "router fragment; mount not detected".to_owned());
    }
    scan.candidates.push(UnusedCandidate {
      entity: EntityRef::of(&endpoint),
      class: UnusedClass::PossiblyUnused,
      evidence,
    });
  }
  Ok(())
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
