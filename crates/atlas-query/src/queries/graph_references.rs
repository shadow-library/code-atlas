use std::collections::{HashMap, HashSet};

use atlas_core::{ATTR_MOCK, ContentHash, Edge, EdgeKind, Entity, EntityKind, Evidence, LanguageFamily, RefKind, RelPath, RepoName, Span, UnresolvedRef};
use atlas_project::Project;
use serde::Serialize;

use crate::QueryEngine;
use crate::engine::risk_reason;
use crate::error::QueryResult;
use crate::graph::Graph;
use crate::model::{Caveat, CertaintyTag, EntityRef, Location, Response, Targeted, UnresolvedSite};
use crate::policy::{Direction, Policy, phrase};
use crate::queries::positions::{Site, VerifiedSource, display_column, name_token};
use crate::queries::symbols::{DeclaredName, SymbolSource, SymbolTarget, declared_name_span, file_symbols, innermost, overload_group, reads_name, source_order, target_candidates};
use crate::queries::traverse::risk_name;

const USAGE_KINDS: [EdgeKind; 8] = [
  EdgeKind::Calls,
  EdgeKind::References,
  EdgeKind::Imports,
  EdgeKind::ReExports,
  EdgeKind::TypeOf,
  EdgeKind::Instantiates,
  EdgeKind::Implements,
  EdgeKind::Extends,
];
const HERITAGE_KINDS: [EdgeKind; 2] = [EdgeKind::Implements, EdgeKind::Extends];
const SNIPPET_CHARS: usize = 160;
const NOT_EXHAUSTIVE: &str = "graph references are not exhaustive: calls through dynamic dispatch, reflection or unindexed code are missing";

#[derive(Debug, Clone, PartialEq)]
pub struct ReferencesRequest {
  pub target: String,
}

impl ReferencesRequest {
  pub fn new(target: impl Into<String>) -> Self {
    Self { target: target.into() }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceSide {
  Inbound,
  Implemented,
  Heritage,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphReference {
  pub location: Option<Location>,
  pub side: ReferenceSide,
  pub edge_kind: String,
  pub edge_ids: Vec<String>,
  pub phrase: String,
  pub other: EntityRef,
  pub enclosing: Option<EntityRef>,
  pub certainty: CertaintyTag,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub overload: Option<usize>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub mock: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphReferences {
  /// For an overload set this is only the first overload; `overloads` is the real subject.
  pub target: EntityRef,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub overloads: Vec<EntityRef>,
  pub source: SymbolSource,
  pub references: Vec<GraphReference>,
  pub unresolved: Vec<UnresolvedSite>,
  pub unverified_files: Vec<Location>,
  pub caveats: Vec<Caveat>,
}

impl QueryEngine {
  pub fn symbol_references(&self, request: &ReferencesRequest) -> QueryResult<Response<Targeted<GraphReferences>>> {
    let target = SymbolTarget::parse(&request.target)?;
    let mut freshness = self.barrier()?;
    let mut answer = self.references_of(&target, &request.target)?;
    if matches!(&answer, Targeted::Found(found) if !found.unverified_files.is_empty()) {
      tracing::info!(spec = %request.target, "a referencing file changed since indexing; re-running the freshness barrier");
      let later = self.barrier()?;
      freshness.absorb_resync(later);
      answer = self.references_of(&target, &request.target)?;
    }
    Ok(Response { freshness, answer })
  }

  pub fn graph_references(&self, target: &Entity) -> QueryResult<GraphReferences> {
    let project = self.engine().project();
    self.read(Policy::with_floor(0.0), |graph| collect(graph, &project, std::slice::from_ref(target)))
  }

  fn references_of(&self, target: &SymbolTarget, text: &str) -> QueryResult<Targeted<GraphReferences>> {
    let project = self.engine().project();
    let repos = self.repos();
    self.read(Policy::with_floor(0.0), |graph| {
      let (candidates, hint) = target_candidates(graph, &repos, target, text)?;
      Ok(match candidates.len() {
        0 => Targeted::NotFound { spec: text.to_owned(), hint },
        1 => Targeted::Found(collect(graph, &project, &candidates)?),
        _ if is_overload_set(target, &candidates) => Targeted::Found(collect(graph, &project, &candidates)?),
        _ => Targeted::Ambiguous {
          spec: text.to_owned(),
          candidates: candidates.iter().map(EntityRef::of).collect(),
        },
      })
    })
  }
}

fn is_overload_set(target: &SymbolTarget, candidates: &[Entity]) -> bool {
  let SymbolTarget::NamePath(pattern) = target else { return false };
  let Some((first, rest)) = candidates.split_first() else { return false };
  let group = overload_group(first);
  pattern.index.is_none() && rest.iter().all(|candidate| overload_group(candidate) == group)
}

fn collect(graph: &Graph<'_, '_>, project: &Project, targets: &[Entity]) -> QueryResult<GraphReferences> {
  let mut targets = targets.to_vec();
  targets.sort_by(source_order);
  let mut files = SourceFiles::new(project);
  let mut rows = Vec::new();
  for (index, target) in targets.iter().enumerate() {
    let overload = (targets.len() > 1).then_some(index);
    let mut sites = Sites {
      graph,
      files: &mut files,
      target,
      overload,
      seen: HashSet::new(),
      rows: &mut rows,
    };
    for edge in graph.edges_in(target.id, &USAGE_KINDS)? {
      let Some(other) = graph.entity(edge.from)? else { continue };
      sites.at_evidence(&edge, &other, ReferenceSide::Inbound)?;
    }
    for edge in graph.edges_out(target.id, &HERITAGE_KINDS)? {
      let Some(other) = graph.entity(edge.to)? else { continue };
      match is_member_override(&edge, target, &other) {
        true => sites.at_declaration(&edge, &other)?,
        false => sites.at_evidence(&edge, &other, ReferenceSide::Heritage)?,
      }
    }
  }
  let mut references = merge_same_token(rows);
  references.sort_by(|left, right| (&left.location, &left.edge_kind, &left.other.key).cmp(&(&right.location, &right.edge_kind, &right.other.key)));
  let unresolved = unresolved_sites(graph, &mut files, &targets)?;
  let (target, overloads) = match targets.as_slice() {
    [only] => (EntityRef::of(only), Vec::new()),
    [first, ..] => (EntityRef::of(first), targets.iter().map(EntityRef::of).collect()),
    [] => return Err(crate::error::QueryError::invalid("graph references need a target")),
  };
  Ok(GraphReferences {
    target,
    overloads,
    source: SymbolSource::Graph,
    references,
    unresolved,
    unverified_files: files.changed(),
    caveats: vec![Caveat::Note {
      message: NOT_EXHAUSTIVE.to_owned(),
    }],
  })
}

fn is_member_override(edge: &Edge, target: &Entity, other: &Entity) -> bool {
  let is_type = |kind: &EntityKind| {
    matches!(
      kind,
      EntityKind::Class | EntityKind::Struct | EntityKind::Enum | EntityKind::Trait | EntityKind::Interface | EntityKind::TypeAlias
    )
  };
  edge.kind == EdgeKind::Implements && other.name == target.name && !is_type(&target.kind)
}

struct Sites<'s, 'g, 'a, 'c, 'p> {
  graph: &'g Graph<'a, 'c>,
  files: &'s mut SourceFiles<'p>,
  target: &'s Entity,
  overload: Option<usize>,
  seen: HashSet<(atlas_core::EdgeId, RepoName, RelPath, u32, u32)>,
  rows: &'s mut Vec<GraphReference>,
}

impl Sites<'_, '_, '_, '_, '_> {
  fn at_evidence(&mut self, edge: &Edge, other: &Entity, side: ReferenceSide) -> QueryResult<()> {
    if edge.evidence.is_empty() {
      if side == ReferenceSide::Inbound {
        self.rows.push(self.row(edge, other, side, None, None, None));
      }
      return Ok(());
    }
    for evidence in &edge.evidence {
      if !self
        .seen
        .insert((edge.id, evidence.repo.clone(), evidence.path.clone(), evidence.span.start_byte, evidence.span.end_byte))
      {
        continue;
      }
      let file = self.files.load(self.graph, &evidence.repo, &evidence.path)?;
      let token = file.token(evidence, &edge.kind, self.target);
      if side == ReferenceSide::Heritage && inside_declaration(self.target, evidence, token) {
        continue;
      }
      let enclosing = innermost(&file.symbols, |span| span.contains(&evidence.span)).map(EntityRef::of);
      let line = token.map_or(evidence.span.start_line, |token| token.start_line);
      let location = Location {
        repo: evidence.repo.to_string(),
        path: evidence.path.to_string(),
        line: Some(line),
        column: token.as_ref().map(display_column),
      };
      let snippet = file.line_text(line);
      let row = self.row(edge, other, side, Some(location), enclosing, snippet);
      self.rows.push(row);
    }
    Ok(())
  }

  fn at_declaration(&mut self, edge: &Edge, declaration: &Entity) -> QueryResult<()> {
    let token = match (declared_name_span(self.graph, declaration)?, &declaration.repo, &declaration.path) {
      (Ok(declared), Some(repo), Some(path)) => self.files.load(self.graph, repo, path)?.verified_name(&declared, &declaration.name),
      _ => None,
    };
    let location = declaration.repo.as_ref().zip(declaration.path.as_ref()).map(|(repo, path)| Location {
      repo: repo.to_string(),
      path: path.to_string(),
      line: token.map(|token| token.start_line).or(declaration.span.map(|span| span.start_line)),
      column: token.as_ref().map(display_column),
    });
    let row = self.row(edge, declaration, ReferenceSide::Implemented, location, Some(EntityRef::of(declaration)), None);
    self.rows.push(row);
    Ok(())
  }

  fn row(&self, edge: &Edge, other: &Entity, side: ReferenceSide, location: Option<Location>, enclosing: Option<EntityRef>, snippet: Option<String>) -> GraphReference {
    let direction = match side {
      ReferenceSide::Implemented => Direction::Reverse,
      ReferenceSide::Inbound | ReferenceSide::Heritage => Direction::Forward,
    };
    GraphReference {
      location,
      side,
      edge_kind: edge.kind.as_str().to_owned(),
      edge_ids: vec![edge.id.to_string()],
      phrase: phrase(&edge.kind, direction, &edge.certainty),
      other: EntityRef::of(other),
      enclosing,
      certainty: CertaintyTag::of(&edge.certainty),
      overload: self.overload,
      mock: edge.attrs.get(ATTR_MOCK).and_then(serde_json::Value::as_str).map(str::to_owned),
      snippet,
    }
  }
}

fn inside_declaration(target: &Entity, evidence: &Evidence, token: Option<Span>) -> bool {
  let same_file = target.repo.as_ref() == Some(&evidence.repo) && target.path.as_ref() == Some(&evidence.path);
  let site = token.unwrap_or(evidence.span);
  same_file && target.span.is_some_and(|declaration| declaration.contains(&site))
}

type TokenKey = (String, String, u32, u32);

fn token_key(row: &GraphReference) -> Option<TokenKey> {
  let location = row.location.as_ref()?;
  Some((location.repo.clone(), location.path.clone(), location.line?, location.column?))
}

fn strength(row: &GraphReference) -> (bool, bool, u32) {
  let confidence = row.certainty.confidence.map_or(1000, |confidence| (confidence * 1000.0).round() as u32);
  (row.edge_kind != EdgeKind::References.as_str(), row.certainty.authoritative, confidence)
}

fn merge_same_token(rows: Vec<GraphReference>) -> Vec<GraphReference> {
  let mut merged: Vec<GraphReference> = Vec::with_capacity(rows.len());
  let mut at: HashMap<TokenKey, usize> = HashMap::new();
  for row in rows {
    let Some(key) = token_key(&row) else {
      merged.push(row);
      continue;
    };
    let Some(&index) = at.get(&key) else {
      at.insert(key, merged.len());
      merged.push(row);
      continue;
    };
    let kept = &mut merged[index];
    let mut edge_ids = std::mem::take(&mut kept.edge_ids);
    edge_ids.extend(row.edge_ids.iter().cloned());
    edge_ids.sort();
    edge_ids.dedup();
    if strength(&row) > strength(kept) {
      *kept = row;
    }
    kept.edge_ids = edge_ids;
  }
  merged
}

fn unresolved_sites(graph: &Graph<'_, '_>, files: &mut SourceFiles<'_>, targets: &[Entity]) -> QueryResult<Vec<UnresolvedSite>> {
  let Some(first) = targets.first() else { return Ok(Vec::new()) };
  let name = risk_name(first);
  if name.is_empty() {
    return Ok(Vec::new());
  }
  let mut sites = Vec::new();
  for found in graph.tx().unresolved_by_names(&[name])? {
    let reference = found.reference;
    let Some(reason) = targets.iter().find_map(|target| risk_reason(&reference, target.id)) else {
      continue;
    };
    let file = files.load(graph, &reference.repo, &reference.file)?;
    let token = file.unresolved_token(&reference, name);
    sites.push(UnresolvedSite {
      location: Location {
        repo: reference.repo.to_string(),
        path: reference.file.to_string(),
        line: Some(token.map_or(reference.span.start_line, |token| token.start_line)),
        column: token.as_ref().map(display_column),
      },
      path: reference.path.join("."),
      reason: reason.to_owned(),
    });
  }
  sites.sort_by(|left, right| left.location.cmp(&right.location).then_with(|| left.path.cmp(&right.path)));
  sites.dedup();
  Ok(sites)
}

fn edge_kind_of(kind: RefKind) -> EdgeKind {
  match kind {
    RefKind::Call | RefKind::MethodCall => EdgeKind::Calls,
    RefKind::TypeRef | RefKind::Inherit => EdgeKind::TypeOf,
    RefKind::Instantiation => EdgeKind::Instantiates,
    _ => EdgeKind::References,
  }
}

struct VerifiedText {
  text: String,
  family: LanguageFamily,
  hash: ContentHash,
  line_starts: Vec<usize>,
}

enum FileText {
  Verified(VerifiedText),
  Changed,
  Unavailable,
}

struct SourceFile {
  text: FileText,
  symbols: Vec<Entity>,
}

impl SourceFile {
  fn verified(&self) -> Option<&VerifiedText> {
    match &self.text {
      FileText::Verified(verified) => Some(verified),
      FileText::Changed | FileText::Unavailable => None,
    }
  }

  fn token(&self, evidence: &Evidence, kind: &EdgeKind, target: &Entity) -> Option<Span> {
    let verified = self.verified().filter(|verified| verified.hash == evidence.content_hash)?;
    let site = Site::of(kind, evidence.span.text(&verified.text)?, &target.kind);
    name_token(&verified.text, verified.family, &evidence.span, &target.name, site)
  }

  fn unresolved_token(&self, reference: &UnresolvedRef, name: &str) -> Option<Span> {
    let verified = self.verified()?;
    let site = Site::of(&edge_kind_of(reference.kind), reference.span.text(&verified.text)?, &EntityKind::Function);
    name_token(&verified.text, verified.family, &reference.span, name, site)
  }

  fn verified_name(&self, declared: &DeclaredName, name: &str) -> Option<Span> {
    let verified = self.verified()?;
    (verified.hash == declared.hash && reads_name(&verified.text, &declared.span, name)).then_some(declared.span)
  }

  fn line_text(&self, line: u32) -> Option<String> {
    let verified = self.verified()?;
    let index = usize::try_from(line).ok()?.checked_sub(1)?;
    let start = *verified.line_starts.get(index)?;
    let end = verified.line_starts.get(index + 1).copied().unwrap_or(verified.text.len());
    let content = verified.text.get(start..end)?.trim();
    Some(match content.char_indices().nth(SNIPPET_CHARS) {
      Some((cut, _)) => format!("{}…", &content[..cut]),
      None => content.to_owned(),
    })
  }
}

struct SourceFiles<'p> {
  project: &'p Project,
  files: HashMap<(RepoName, RelPath), SourceFile>,
}

impl<'p> SourceFiles<'p> {
  fn new(project: &'p Project) -> Self {
    Self { project, files: HashMap::new() }
  }

  fn load(&mut self, graph: &Graph<'_, '_>, repo: &RepoName, path: &RelPath) -> QueryResult<&SourceFile> {
    let key = (repo.clone(), path.clone());
    if !self.files.contains_key(&key) {
      let file = SourceFile {
        text: self.read(graph, repo, path)?,
        symbols: file_symbols(graph, repo, path)?,
      };
      self.files.insert(key.clone(), file);
    }
    Ok(&self.files[&key])
  }

  fn read(&self, graph: &Graph<'_, '_>, repo: &RepoName, path: &RelPath) -> QueryResult<FileText> {
    let Some(record) = graph.tx().file(repo, path)? else { return Ok(FileText::Unavailable) };
    let Some(family) = record.language.map(|language| language.family()) else {
      return Ok(FileText::Unavailable);
    };
    let Some(root) = self.project.repos.iter().find(|config| config.name == *repo).map(|config| &config.path) else {
      return Ok(FileText::Unavailable);
    };
    let bytes = match std::fs::read(root.join(path.as_str())) {
      Ok(bytes) => bytes,
      Err(error) => {
        tracing::debug!(repo = %repo, path = %path, error = %error, "cannot read a referencing file; its columns are omitted");
        return Ok(FileText::Changed);
      }
    };
    Ok(match VerifiedSource::verify(&bytes, &record.content_hash, family) {
      Some(source) => {
        let text = source.text().to_owned();
        let line_starts = std::iter::once(0).chain(text.match_indices('\n').map(|(index, _)| index + 1)).collect();
        FileText::Verified(VerifiedText {
          text,
          family,
          hash: record.content_hash,
          line_starts,
        })
      }
      None if std::str::from_utf8(&bytes).is_err() => FileText::Unavailable,
      None => FileText::Changed,
    })
  }

  fn changed(&self) -> Vec<Location> {
    let mut changed: Vec<Location> = self
      .files
      .iter()
      .filter(|(_, file)| matches!(file.text, FileText::Changed))
      .map(|((repo, path), _)| Location {
        repo: repo.to_string(),
        path: path.to_string(),
        line: None,
        column: None,
      })
      .collect();
    changed.sort();
    changed
  }
}
