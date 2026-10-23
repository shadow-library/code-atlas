use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

use atlas_core::{ContentHash, EdgeKind, Entity, EntityId, EntityKey, EntityKind, HttpMethod, ParseStatus, RelPath, RepoName, Span};
use atlas_engine::{EngineError, artifact_file_facts};
use atlas_store::EntityFilter;
use serde::Serialize;

use crate::QueryEngine;
use crate::engine::{Snippet, SnippetMode};
use crate::error::{QueryError, QueryResult};
use crate::freshness::Freshness;
use crate::graph::Graph;
use crate::model::{Caveat, EntityRef, Listing, Location, Response, Targeted};
use crate::policy::Policy;
use crate::queries::name_path::{Anchor, NameMatch, NamePath, crate_name, file_module_prefix, qualified_segments};
use crate::target::{SOURCE_EXTENSIONS, TargetSpec, is_test_block, not_found_hint, prefer_definitions};

pub const DEFAULT_MAX_MATCHES: usize = 20;
pub const MAX_MATCHES: usize = 200;
pub const MAX_SYMBOL_DEPTH: u32 = 3;
const MAX_NESTING: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolTarget {
  Key(EntityKey),
  Id(EntityId),
  Position { repo: Option<String>, path: String, line: u32, column: u32 },
  Line { repo: Option<String>, path: String, line: u32 },
  File { repo: Option<String>, path: String },
  NamePath(NamePath),
}

impl SymbolTarget {
  pub fn parse(text: &str) -> QueryResult<Self> {
    let text = text.trim();
    if text.is_empty() {
      return Err(QueryError::invalid("empty target"));
    }
    if let Ok(key) = text.parse::<EntityKey>() {
      return Ok(Self::Key(key));
    }
    if let Ok(id) = text.parse::<EntityId>() {
      return Ok(Self::Id(id));
    }
    if is_endpoint(text) {
      return Err(QueryError::invalid(format!("{text:?} is an endpoint; endpoints have no LSP symbol; use atlas_api")));
    }
    if let Some(located) = parse_located(text)? {
      return Ok(located);
    }
    NamePath::parse(text).map(Self::NamePath)
  }
}

fn is_endpoint(text: &str) -> bool {
  let Some((method, route)) = text.split_once(' ') else { return false };
  let route = route.trim();
  HttpMethod::parse_lenient(method).is_some() && route.starts_with('/') && !route.contains(char::is_whitespace)
}

fn parse_located(text: &str) -> QueryResult<Option<SymbolTarget>> {
  let mut parts: Vec<&str> = text.split(':').collect();
  let mut numbers = Vec::new();
  while numbers.len() < 2 && parts.len() > 1 {
    let Some(number) = parts.last().and_then(|last| last.parse::<u32>().ok()) else { break };
    numbers.insert(0, number);
    parts.pop();
  }
  let (repo, path) = match parts.as_slice() {
    [path] => (None, *path),
    [repo, path] => (Some((*repo).to_owned()), *path),
    _ => return Ok(None),
  };
  let path = path.trim_start_matches("./");
  let has_extension = SOURCE_EXTENSIONS.iter().any(|extension| path.ends_with(extension));
  if path.contains(' ') || path.is_empty() || !(has_extension || (path.contains('/') && !numbers.is_empty())) {
    return Ok(None);
  }
  if numbers.contains(&0) {
    return Err(QueryError::invalid(format!("{text:?}: lines and columns are 1-based")));
  }
  let path = path.to_owned();
  Ok(Some(match numbers.as_slice() {
    [line, column] => SymbolTarget::Position {
      repo,
      path,
      line: *line,
      column: *column,
    },
    [line] => SymbolTarget::Line { repo, path, line: *line },
    _ => SymbolTarget::File { repo, path },
  }))
}

#[derive(Debug, Clone, PartialEq)]
pub struct FindRequest {
  pub pattern: String,
  pub path: Option<String>,
  pub depth: u32,
  pub body: bool,
  pub name_match: NameMatch,
  pub kinds: Vec<EntityKind>,
  pub exclude_kinds: Vec<EntityKind>,
  pub max_matches: usize,
}

impl FindRequest {
  pub fn new(pattern: impl Into<String>) -> Self {
    Self {
      pattern: pattern.into(),
      path: None,
      depth: 0,
      body: false,
      name_match: NameMatch::Exact,
      kinds: Vec::new(),
      exclude_kinds: Vec::new(),
      max_matches: DEFAULT_MAX_MATCHES,
    }
  }

  fn validate(&self) -> QueryResult<()> {
    if !(1..=MAX_MATCHES).contains(&self.max_matches) {
      return Err(QueryError::invalid(format!("max_matches must be between 1 and {MAX_MATCHES}")));
    }
    validate_depth(self.depth)?;
    if let Some(kind) = self.kinds.iter().chain(&self.exclude_kinds).find(|kind| !kind.is_code_symbol()) {
      return Err(QueryError::invalid(format!("{kind} is not a symbol kind; find searches code symbols")));
    }
    Ok(())
  }

  fn admits_kind(&self, kind: &EntityKind) -> bool {
    kind.is_code_symbol() && (self.kinds.is_empty() || self.kinds.contains(kind)) && !self.exclude_kinds.contains(kind)
  }
}

fn validate_depth(depth: u32) -> QueryResult<()> {
  match depth <= MAX_SYMBOL_DEPTH {
    true => Ok(()),
    false => Err(QueryError::invalid(format!("depth must be between 0 and {MAX_SYMBOL_DEPTH}"))),
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SymbolNode {
  pub entity: EntityRef,
  pub name: String,
  pub signature: Option<String>,
  pub children: Vec<SymbolNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SymbolMatch {
  pub entity: EntityRef,
  pub name_path: String,
  pub signature: Option<String>,
  pub children: Vec<SymbolNode>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub body: Option<Snippet>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FindAnswer {
  pub pattern: String,
  pub matches: Listing<SymbolMatch>,
  pub caveats: Vec<Caveat>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolSource {
  Graph,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutlineRequest {
  pub file: String,
  pub depth: u32,
}

impl OutlineRequest {
  pub fn new(file: impl Into<String>) -> Self {
    Self { file: file.into(), depth: 1 }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OutlineParse {
  Complete,
  Partial { error_count: u32 },
  Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutlineAnswer {
  pub file: Location,
  pub source: SymbolSource,
  pub parse: OutlineParse,
  pub symbols: Vec<SymbolNode>,
  pub caveats: Vec<Caveat>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NameSpan {
  Found { span: Span },
  SourceChanged,
  Unavailable { reason: String },
}

impl QueryEngine {
  pub fn fresh(&self) -> QueryResult<Freshness> {
    self.barrier()
  }

  pub fn resolve_target(&self, text: &str) -> QueryResult<Targeted<Entity>> {
    let target = SymbolTarget::parse(text)?;
    let repos = self.repos();
    self.read(Policy::with_floor(0.0), |graph| resolve_symbol_target(graph, &repos, &target, text))
  }

  pub fn innermost_symbol(&self, repo: &RepoName, path: &RelPath, line: u32, column: u32) -> QueryResult<Option<Entity>> {
    self.read(Policy::with_floor(0.0), |graph| {
      let symbols = file_symbols(graph, repo, path)?;
      Ok(innermost(&symbols, |span| covers_position(span, line, column)).cloned())
    })
  }

  pub fn name_span(&self, entity: &Entity) -> QueryResult<NameSpan> {
    let declared = match self.read(Policy::with_floor(0.0), |graph| declared_name_span(graph, entity))? {
      Ok(declared) => declared,
      Err(reason) => return Ok(unavailable(reason)),
    };
    let (Some(repo), Some(path)) = (&entity.repo, &entity.path) else {
      return Ok(unavailable("the entity has no file"));
    };
    let (text, hash) = match self.engine().read_source(repo, path) {
      Ok(read) => read,
      Err(EngineError::SourceChanged { .. }) => return Ok(NameSpan::SourceChanged),
      Err(error) => return Ok(unavailable(error.to_string())),
    };
    if hash != declared.hash {
      return Ok(NameSpan::SourceChanged);
    }
    match reads_name(&text, &declared.span, &entity.name) {
      true => Ok(NameSpan::Found { span: declared.span }),
      false => Ok(unavailable(format!("the declared name token in {repo}:{path} does not read {:?}", entity.name))),
    }
  }

  pub fn find(&self, request: &FindRequest) -> QueryResult<Response<FindAnswer>> {
    request.validate()?;
    let pattern = NamePath::parse(&request.pattern)?;
    let mut freshness = self.barrier()?;
    let repos = self.repos();
    let mut answer = self.read(Policy::with_floor(0.0), |graph| find_in(graph, &repos, &pattern, request))?;
    if request.body {
      let mut sources = self.sources(&mut freshness);
      for found in &mut answer.matches.items {
        found.body = Some(sources.snippet(found.entity.entity_id()?, SnippetMode::Body { max_lines: None }, 0)?);
      }
    }
    Ok(Response { freshness, answer })
  }

  pub fn outline(&self, request: &OutlineRequest) -> QueryResult<Response<Targeted<OutlineAnswer>>> {
    validate_depth(request.depth)?;
    let freshness = self.barrier()?;
    let repos = self.repos();
    let answer = self.read(Policy::with_floor(0.0), |graph| outline_of(graph, &repos, request))?;
    Ok(Response { freshness, answer })
  }
}

fn unavailable(reason: impl Into<String>) -> NameSpan {
  NameSpan::Unavailable { reason: reason.into() }
}

pub(crate) struct DeclaredName {
  pub span: Span,
  pub hash: ContentHash,
}

pub(crate) fn declared_name_span(graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<Result<DeclaredName, String>> {
  let (Some(repo), Some(path), Some(span), EntityKey::Symbol(key)) = (&entity.repo, &entity.path, entity.span, &entity.key) else {
    return Ok(Err("the entity is not a symbol with a source span".to_owned()));
  };
  let Some(blob) = graph.tx().blob(&format!("artifact:{repo}:{path}"))? else {
    return Ok(Err(format!("{repo}:{path} has no stored analysis")));
  };
  let facts = match artifact_file_facts(&blob.bytes) {
    Ok(facts) => facts,
    Err(error) => return Ok(Err(format!("the stored analysis of {repo}:{path} cannot be read: {error}"))),
  };
  let declared: Vec<Span> = facts
    .symbols
    .iter()
    .filter(|symbol| symbol.kind == entity.kind && symbol.span == span && symbol.item_path == key.item_path())
    .map(|symbol| symbol.name_span)
    .collect();
  match declared.as_slice() {
    [name_span] => Ok(Ok(DeclaredName {
      span: *name_span,
      hash: facts.file.content_hash,
    })),
    _ => Ok(Err(format!("{} declarations in {repo}:{path} match {}", declared.len(), entity.key))),
  }
}

pub(crate) fn reads_name(text: &str, span: &Span, name: &str) -> bool {
  span.text(text).is_some_and(|token| token.strip_prefix("r#").unwrap_or(token) == name)
}

pub(crate) fn resolve_symbol_target(graph: &Graph<'_, '_>, repos: &[RepoName], target: &SymbolTarget, text: &str) -> QueryResult<Targeted<Entity>> {
  let (mut candidates, hint) = target_candidates(graph, repos, target, text)?;
  Ok(match candidates.len() {
    0 => Targeted::NotFound { spec: text.to_owned(), hint },
    1 => Targeted::Found(candidates.remove(0)),
    _ => Targeted::Ambiguous {
      spec: text.to_owned(),
      candidates: candidates.iter().map(EntityRef::of).collect(),
    },
  })
}

pub(crate) fn target_candidates(graph: &Graph<'_, '_>, repos: &[RepoName], target: &SymbolTarget, text: &str) -> QueryResult<(Vec<Entity>, String)> {
  let (mut candidates, hint) = match target {
    SymbolTarget::Key(key) => (graph.tx().entity_by_key(key)?.into_iter().collect(), format!("no entity has key {key}")),
    SymbolTarget::Id(id) => (graph.entity(*id)?.into_iter().collect(), format!("no entity has id {id}")),
    SymbolTarget::File { repo, path } => (files_named(graph, repos, repo.as_deref(), path)?, format!("no indexed file at {text}")),
    SymbolTarget::Line { repo, path, line } => (
      symbols_at(graph, repos, repo.as_deref(), path, |span| span.start_line <= *line && *line <= span.end_line)?,
      format!("no symbol encloses {text}"),
    ),
    SymbolTarget::Position { repo, path, line, column } => (
      symbols_at(graph, repos, repo.as_deref(), path, |span| covers_position(span, *line, *column))?,
      format!("no symbol encloses {text}"),
    ),
    SymbolTarget::NamePath(pattern) => name_path_candidates(graph, pattern, text)?,
  };
  candidates.sort_by(|left, right| left.key.cmp(&right.key));
  Ok((candidates, hint))
}

fn files_named(graph: &Graph<'_, '_>, repos: &[RepoName], repo: Option<&str>, path: &str) -> QueryResult<Vec<Entity>> {
  let Ok(path) = RelPath::new(path) else { return Ok(Vec::new()) };
  let mut found = Vec::new();
  for candidate in repos.iter().filter(|candidate| repo.is_none_or(|repo| candidate.as_str() == repo)) {
    found.extend(graph.tx().entity_by_key(&EntityKey::File {
      repo: candidate.clone(),
      path: path.clone(),
    })?);
  }
  Ok(found)
}

fn symbols_at(graph: &Graph<'_, '_>, repos: &[RepoName], repo: Option<&str>, path: &str, covers: impl Fn(&Span) -> bool) -> QueryResult<Vec<Entity>> {
  let mut found = Vec::new();
  for file in files_named(graph, repos, repo, path)? {
    let EntityKey::File { repo, path } = &file.key else { continue };
    let symbols = file_symbols(graph, repo, path)?;
    found.extend(innermost(&symbols, &covers).cloned());
  }
  Ok(found)
}

pub(crate) fn file_symbols(graph: &Graph<'_, '_>, repo: &RepoName, path: &RelPath) -> QueryResult<Vec<Entity>> {
  Ok(
    graph
      .tx()
      .entities_in_file(repo, path, &EntityFilter::default())?
      .into_iter()
      .filter(|entity| entity.kind.is_code_symbol() && entity.span.is_some())
      .collect(),
  )
}

pub(crate) fn innermost(symbols: &[Entity], covers: impl Fn(&Span) -> bool) -> Option<&Entity> {
  symbols
    .iter()
    .filter(|entity| entity.span.as_ref().is_some_and(&covers))
    .min_by_key(|entity| (entity.span.map_or(u32::MAX, |span| span.end_byte - span.start_byte), entity.id))
}

fn covers_position(span: &Span, line: u32, column: u32) -> bool {
  let point = (line, column.saturating_sub(1));
  (span.start_line, span.start_col) <= point && point < (span.end_line, span.end_col)
}

fn named(graph: &Graph<'_, '_>, pattern: &NamePath) -> QueryResult<Vec<Entity>> {
  let candidates = graph.tx().entities_by_name(pattern.name(), &EntityFilter::default())?;
  let matched = matching(graph, pattern, NameMatch::Exact, candidates.into_iter().filter(|entity| entity.kind.is_code_symbol()))?;
  Ok(match pattern.index {
    Some(index) => select_overload(matched, index),
    None => matched,
  })
}

pub(crate) fn symbols_named(graph: &Graph<'_, '_>, text: &str) -> QueryResult<Vec<Entity>> {
  let Ok(pattern) = NamePath::parse(text) else { return Ok(Vec::new()) };
  let mut found = named(graph, &pattern)?;
  if let Some(literal) = pattern.literal_fallback().filter(|_| found.is_empty()) {
    found = named(graph, &literal)?;
  }
  found.retain(|entity| !is_test_block(entity));
  found.sort_by(source_order);
  found.dedup_by_key(|entity| entity.id);
  Ok(found)
}

fn name_path_candidates(graph: &Graph<'_, '_>, pattern: &NamePath, text: &str) -> QueryResult<(Vec<Entity>, String)> {
  let spec = TargetSpec::Name {
    qualifier: pattern.qualifier().to_vec(),
    name: pattern.name().to_owned(),
  };
  let hint = not_found_hint(graph, &spec, text)?;
  let found = prefer_definitions(named(graph, pattern)?);
  let (found, name) = match pattern.literal_fallback().filter(|_| found.is_empty()) {
    Some(literal) => (prefer_definitions(named(graph, &literal)?), literal.name().to_owned()),
    None => (found, pattern.name().to_owned()),
  };
  let only_test_blocks = !found.is_empty() && found.iter().all(is_test_block);
  if !only_test_blocks || name.contains(char::is_whitespace) {
    return Ok((found, hint));
  }
  let mut withheld: Vec<String> = found.iter().map(|entity| format!("{} {}", entity.kind, entity.key)).collect();
  withheld.sort();
  Ok((Vec::new(), format!("{hint}; {} is named exactly {name:?}, pass its key to target it", withheld.join(", "))))
}

fn matching(graph: &Graph<'_, '_>, pattern: &NamePath, rule: NameMatch, candidates: impl Iterator<Item = Entity>) -> QueryResult<Vec<Entity>> {
  let mut matched = Vec::new();
  for entity in candidates {
    let qualified = qualified_segments(&entity);
    let accepted = match pattern.anchor {
      Anchor::Relative => pattern.matches_qualified(&qualified, crate_name(&entity).as_deref(), rule),
      Anchor::Absolute => {
        let item_path = entity.key.as_symbol().map_or(&[][..], |symbol| symbol.item_path());
        pattern.matches_qualified(&qualified, None, rule) && pattern.may_match_in_file(item_path) && pattern.matches_in_file(&in_file_segments(graph, &entity)?, rule)
      }
    };
    if accepted {
      matched.push(entity);
    }
  }
  Ok(matched)
}

pub(crate) type OverloadGroup = (Option<RepoName>, Option<RelPath>, Vec<String>, EntityKind);

pub(crate) fn overload_group(entity: &Entity) -> OverloadGroup {
  (entity.repo.clone(), entity.path.clone(), qualified_segments(entity), entity.kind.clone())
}

fn select_overload(matched: Vec<Entity>, index: usize) -> Vec<Entity> {
  let mut siblings: BTreeMap<OverloadGroup, Vec<Entity>> = BTreeMap::new();
  for entity in matched {
    siblings.entry(overload_group(&entity)).or_default().push(entity);
  }
  siblings
    .into_values()
    .filter_map(|mut group| {
      group.sort_by(source_order);
      (index < group.len()).then(|| group.swap_remove(index))
    })
    .collect()
}

pub(crate) fn source_order(left: &Entity, right: &Entity) -> Ordering {
  let place = |entity: &Entity| (entity.repo.clone(), entity.path.clone(), entity.span.map(|span| span.start_byte));
  place(left).cmp(&place(right)).then_with(|| left.key.cmp(&right.key))
}

pub(crate) fn in_file_segments(graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<Vec<String>> {
  let qualified = qualified_segments(entity);
  let mut top = entity.clone();
  for _ in 0..MAX_NESTING {
    let Some(parent) = same_file_parent(graph, &top)? else { break };
    top = parent;
  }
  Ok(match qualified.strip_prefix(file_module_prefix(&top)) {
    Some(rest) if !rest.is_empty() => rest.to_vec(),
    _ => match &entity.key {
      EntityKey::Symbol(symbol) if !symbol.item_path().is_empty() => symbol.item_path().to_vec(),
      _ => vec![entity.name.clone()],
    },
  })
}

fn same_file_parent(graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<Option<Entity>> {
  let mut parents = Vec::new();
  for link in graph.links_in(entity.id, &[EdgeKind::Contains])? {
    let Some(parent) = graph.entity(link.from)? else { continue };
    if parent.kind.is_code_symbol() && parent.repo == entity.repo && parent.path == entity.path && parent.id != entity.id {
      parents.push(parent);
    }
  }
  Ok(parents.into_iter().min_by(|left, right| left.key.cmp(&right.key)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathScope {
  repo: Option<RepoName>,
  path: String,
}

impl PathScope {
  fn parse(text: &str, repos: &[RepoName]) -> QueryResult<Self> {
    let text = text.trim();
    let (repo, rest) = match text.split_once(':') {
      Some((repo, rest)) => match repos.iter().find(|candidate| candidate.as_str() == repo) {
        Some(found) => (Some(found.clone()), rest),
        None => return Err(QueryError::invalid(format!("unknown repo {repo:?} in path {text:?}"))),
      },
      None => (None, text),
    };
    let path = rest.trim().trim_start_matches("./").trim_end_matches('/');
    let path = if path == "." { "" } else { path };
    if path.starts_with('/') || path.split('/').any(|segment| segment == "..") {
      return Err(QueryError::invalid(format!("path {text:?} must be relative to a repo root")));
    }
    Ok(Self { repo, path: path.to_owned() })
  }

  fn covers(&self, repo: &RepoName, path: &str) -> bool {
    let inside = self.path.is_empty() || path == self.path || path.strip_prefix(self.path.as_str()).is_some_and(|rest| rest.starts_with('/'));
    inside && self.repo.as_ref().is_none_or(|wanted| wanted == repo)
  }

  fn files(&self, graph: &Graph<'_, '_>, repos: &[RepoName]) -> QueryResult<HashSet<(RepoName, RelPath)>> {
    let mut files = HashSet::new();
    for repo in repos.iter().filter(|repo| self.repo.as_ref().is_none_or(|wanted| wanted == *repo)) {
      for record in graph.tx().files_in_repo(repo)? {
        if self.covers(&record.repo, record.path.as_str()) {
          files.insert((record.repo, record.path));
        }
      }
    }
    Ok(files)
  }
}

fn find_in(graph: &Graph<'_, '_>, repos: &[RepoName], pattern: &NamePath, request: &FindRequest) -> QueryResult<FindAnswer> {
  let mut caveats = Vec::new();
  let files = match &request.path {
    Some(text) => {
      let files = PathScope::parse(text, repos)?.files(graph, repos)?;
      if files.is_empty() {
        caveats.push(Caveat::Note {
          message: format!("no indexed file matches the path {text:?}"),
        });
      }
      Some(files)
    }
    None => None,
  };
  let in_scope = |entity: &Entity| match (&files, &entity.repo, &entity.path) {
    (None, _, _) => true,
    (Some(files), Some(repo), Some(path)) => files.contains(&(repo.clone(), path.clone())),
    _ => false,
  };
  let search = |pattern: &NamePath| -> QueryResult<Vec<Entity>> {
    let mut seen = HashSet::new();
    let admitted = candidates(graph, pattern, request, files.as_ref())?
      .into_iter()
      .filter(|entity| request.admits_kind(&entity.kind) && in_scope(entity) && seen.insert(entity.id));
    let matched = matching(graph, pattern, request.name_match, admitted)?;
    Ok(match pattern.index {
      Some(index) => select_overload(matched, index),
      None => matched,
    })
  };
  let mut matched = search(pattern)?;
  if let Some(fallback) = pattern.literal_fallback()
    && matched.is_empty()
  {
    matched = search(&fallback)?;
  }
  matched.sort_by(|left, right| is_test_block(left).cmp(&is_test_block(right)).then_with(|| source_order(left, right)));
  let listing = Listing::truncated(matched, request.max_matches);
  let mut items = Vec::with_capacity(listing.items.len());
  for entity in &listing.items {
    items.push(SymbolMatch {
      entity: EntityRef::of(entity),
      name_path: in_file_segments(graph, entity)?.join("/"),
      signature: entity.signature.clone(),
      children: symbol_nodes(graph, entity, request.depth, &ChildScope::Anywhere)?,
      body: None,
    });
  }
  Ok(FindAnswer {
    pattern: request.pattern.trim().to_owned(),
    matches: Listing { total: listing.total, items },
    caveats,
  })
}

fn candidates(graph: &Graph<'_, '_>, pattern: &NamePath, request: &FindRequest, files: Option<&HashSet<(RepoName, RelPath)>>) -> QueryResult<Vec<Entity>> {
  let mut found = Vec::new();
  match (request.name_match, files) {
    (NameMatch::Exact, _) => found = graph.tx().entities_by_name(pattern.name(), &EntityFilter::default())?,
    (NameMatch::Substring, Some(files)) => {
      for (repo, path) in files {
        found.extend(graph.tx().entities_in_file(repo, path, &EntityFilter::default())?);
      }
    }
    (NameMatch::Substring, None) => {
      for kind in EntityKind::BUILTIN.iter().filter(|kind| request.admits_kind(kind)) {
        let of_kind = graph.tx().entities_by_kind(kind, &EntityFilter::default())?;
        found.extend(of_kind.into_iter().filter(|entity| entity.name.contains(pattern.name())));
      }
    }
  }
  Ok(found)
}

enum ChildScope<'s> {
  Anywhere,
  Within(&'s HashSet<EntityId>),
}

impl ChildScope<'_> {
  fn admits(&self, entity: &Entity) -> bool {
    match self {
      Self::Anywhere => true,
      Self::Within(ids) => ids.contains(&entity.id),
    }
  }
}

fn symbol_nodes(graph: &Graph<'_, '_>, parent: &Entity, depth: u32, scope: &ChildScope<'_>) -> QueryResult<Vec<SymbolNode>> {
  if depth == 0 {
    return Ok(Vec::new());
  }
  let mut children: Vec<Entity> = graph
    .children(parent.id)?
    .into_iter()
    .filter(|child| child.kind.is_code_symbol() && child.id != parent.id && scope.admits(child))
    .collect();
  children.sort_by(source_order);
  nodes_of(graph, &children, depth - 1, scope)
}

fn nodes_of(graph: &Graph<'_, '_>, entities: &[Entity], depth: u32, scope: &ChildScope<'_>) -> QueryResult<Vec<SymbolNode>> {
  entities
    .iter()
    .map(|entity| {
      Ok(SymbolNode {
        entity: EntityRef::of(entity),
        name: entity.name.clone(),
        signature: entity.signature.clone(),
        children: symbol_nodes(graph, entity, depth, scope)?,
      })
    })
    .collect()
}

fn outline_of(graph: &Graph<'_, '_>, repos: &[RepoName], request: &OutlineRequest) -> QueryResult<Targeted<OutlineAnswer>> {
  let scope = PathScope::parse(&request.file, repos)?;
  let mut files = files_named(graph, repos, scope.repo.as_ref().map(RepoName::as_str), &scope.path)?;
  files.sort_by(|left, right| left.key.cmp(&right.key));
  let file = match files.len() {
    0 => {
      return Ok(Targeted::NotFound {
        spec: request.file.clone(),
        hint: format!("no indexed file at {}", request.file),
      });
    }
    1 => files.remove(0),
    _ => {
      return Ok(Targeted::Ambiguous {
        spec: request.file.clone(),
        candidates: files.iter().map(EntityRef::of).collect(),
      });
    }
  };
  let EntityKey::File { repo, path } = &file.key else {
    return Err(QueryError::invalid(format!("{} is not a file", file.key)));
  };
  let mut symbols = file_symbols(graph, repo, path)?;
  symbols.sort_by(source_order);
  let ids: HashSet<EntityId> = symbols.iter().map(|symbol| symbol.id).collect();
  let mut top = Vec::new();
  for symbol in &symbols {
    let mut parents = graph.links_in(symbol.id, &[EdgeKind::Contains])?.into_iter();
    if !parents.any(|link| link.from != symbol.id && ids.contains(&link.from)) {
      top.push(symbol.clone());
    }
  }
  let parse = match graph.tx().file(repo, path)?.map(|record| record.parse_status) {
    Some(ParseStatus::Partial { error_count, .. }) => OutlineParse::Partial { error_count },
    Some(ParseStatus::Failed { reason }) => OutlineParse::Failed { reason },
    Some(ParseStatus::Ok) | None => OutlineParse::Complete,
  };
  Ok(Targeted::Found(OutlineAnswer {
    file: Location {
      repo: repo.to_string(),
      path: path.to_string(),
      line: None,
      column: None,
    },
    source: SymbolSource::Graph,
    parse,
    symbols: nodes_of(graph, &top, request.depth, &ChildScope::Within(&ids))?,
    caveats: Vec::new(),
  }))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_parse_positions_lines_files_and_name_paths() {
    assert_eq!(
      SymbolTarget::parse("acme-api:src/users/users.service.ts:18:9").unwrap(),
      SymbolTarget::Position {
        repo: Some("acme-api".into()),
        path: "src/users/users.service.ts".into(),
        line: 18,
        column: 9
      }
    );
    assert_eq!(
      SymbolTarget::parse("src/money.rs:4").unwrap(),
      SymbolTarget::Line {
        repo: None,
        path: "src/money.rs".into(),
        line: 4
      }
    );
    assert_eq!(
      SymbolTarget::parse("./src/client.ts").unwrap(),
      SymbolTarget::File {
        repo: None,
        path: "src/client.ts".into()
      }
    );
    assert_eq!(
      SymbolTarget::parse("UsersService/findOne").unwrap(),
      SymbolTarget::NamePath(NamePath::parse("UsersService/findOne").unwrap())
    );
    assert_eq!(SymbolTarget::parse("/AcmeClient").unwrap(), SymbolTarget::NamePath(NamePath::parse("/AcmeClient").unwrap()));
    assert_eq!(
      SymbolTarget::parse("src/users/findOne:3").unwrap(),
      SymbolTarget::Line {
        repo: None,
        path: "src/users/findOne".into(),
        line: 3
      }
    );
  }

  #[test]
  fn should_parse_keys_before_anything_else() {
    let key = "ts:acme-api:acme-api:src/users/users.service::UsersService::findOne#method";

    assert_eq!(SymbolTarget::parse(key).unwrap(), SymbolTarget::Key(key.parse().unwrap()));
  }

  #[test]
  fn should_refuse_endpoints_and_zero_based_positions() {
    let endpoint = SymbolTarget::parse("GET /users/:id").unwrap_err().to_string();

    assert_eq!(endpoint, "invalid query: \"GET /users/:id\" is an endpoint; endpoints have no LSP symbol; use atlas_api");
    assert!(SymbolTarget::parse("src/a.ts:0:1").is_err());
    assert!(SymbolTarget::parse("src/a.ts:3:0").is_err());
  }

  #[test]
  fn should_cover_a_position_up_to_but_excluding_the_span_end() {
    let span = Span {
      start_byte: 10,
      end_byte: 40,
      start_line: 2,
      start_col: 4,
      end_line: 4,
      end_col: 1,
    };

    assert!(!covers_position(&span, 2, 4));
    assert!(covers_position(&span, 2, 5));
    assert!(covers_position(&span, 4, 1));
    assert!(!covers_position(&span, 4, 2));
    assert!(!covers_position(&span, 1, 30));
  }

  #[test]
  fn should_scope_paths_to_files_and_directories() {
    let repos = vec![RepoName::new("acme-api").unwrap(), RepoName::new("acme-sdk").unwrap()];
    let api = &repos[0];
    let sdk = &repos[1];

    let directory = PathScope::parse("acme-api:src/users/", &repos).unwrap();
    assert!(directory.covers(api, "src/users/users.service.ts"));
    assert!(!directory.covers(api, "src/users2/x.ts"));
    assert!(!directory.covers(sdk, "src/users/users.service.ts"));

    let file = PathScope::parse("./src/client.ts", &repos).unwrap();
    assert!(file.covers(sdk, "src/client.ts"));
    assert!(!file.covers(sdk, "src/client.tsx"));

    assert!(PathScope::parse("acme-sdk:", &repos).unwrap().covers(sdk, "src/index.ts"));
    assert!(PathScope::parse("nope:src", &repos).is_err());
    assert!(PathScope::parse("/abs/src", &repos).is_err());
    assert!(PathScope::parse("src/../x", &repos).is_err());
  }
}
