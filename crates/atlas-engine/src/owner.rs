use std::cmp::Ordering;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{
  Certainty, ContentHash, DetectedEdge, Edge, EdgeId, EdgeKind, Entity, EntityId, EntityKey, EntityKind, Evidence, FileFacts, NodeRef, ParseStatus, Producer, RelPath, RepoName,
  Span, SymbolKey, UnresolvedRef,
};
use atlas_project::PackageGraph;
use atlas_resolve::{ProjectSymbolTable, Resolution, resolve_file, resolve_path_in_file};
use atlas_store::OwnedFacts;
use serde_json::{Value, json};

use crate::artifact::{FileArtifact, FileKey};
use crate::error::{EngineError, EngineResult};
use crate::packages::package_key_for_file;

pub(crate) const ENGINE_PRODUCER: &str = "atlas-engine";
pub(crate) const ENGINE_PRODUCER_VERSION: u32 = 1;
pub(crate) const ATTR_GENERATED: &str = "generated";
const RULE_DETECTOR_CANDIDATE: &str = "detector-target-candidate";
const ATTR_SITE_ATTRS: &str = "site_attrs";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SymbolEntry {
  pub id: EntityId,
  pub key: SymbolKey,
  pub kind: EntityKind,
  pub local: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ConditionalEdge {
  pub edge: Edge,
  pub requires: BTreeSet<EntityId>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DraftCounts {
  pub unresolvable_detector_edges: u64,
  pub dangling_detector_edges: u64,
}

#[derive(Debug)]
pub(crate) struct FinalOwner {
  pub key: FileKey,
  pub facts: OwnedFacts,
  pub symbols: Vec<SymbolEntry>,
  pub counts: DraftCounts,
  pub wanted: BTreeSet<EntityId>,
}

#[derive(Debug)]
pub(crate) struct OwnerDraft {
  pub key: FileKey,
  pub entities: BTreeMap<EntityId, Entity>,
  pub edges: BTreeMap<EdgeId, Edge>,
  pub conditional: BTreeMap<EdgeId, ConditionalEdge>,
  pub unresolved: Vec<UnresolvedRef>,
  pub symbols: Vec<SymbolEntry>,
  pub counts: DraftCounts,
}

impl OwnerDraft {
  pub fn wanted(&self) -> BTreeSet<EntityId> {
    self.conditional.values().flat_map(|conditional| conditional.requires.iter().copied()).collect()
  }

  pub fn dangling_count(&self, exists: impl Fn(EntityId) -> bool) -> u64 {
    let dangling = self
      .conditional
      .values()
      .filter(|conditional| !conditional.requires.iter().all(|id| self.entities.contains_key(id) || exists(*id)));
    dangling.count() as u64
  }

  pub fn finalize(mut self, exists: impl Fn(EntityId) -> bool) -> FinalOwner {
    let wanted = self.wanted();
    for conditional in std::mem::take(&mut self.conditional).into_values() {
      let satisfied = conditional.requires.iter().all(|id| self.entities.contains_key(id) || exists(*id));
      match satisfied {
        true => merge_edge(&mut self.edges, conditional.edge),
        false => self.counts.dangling_detector_edges += 1,
      }
    }
    let edges = self
      .edges
      .into_values()
      .map(|mut edge| {
        edge.evidence.sort_by(evidence_order);
        edge.evidence.dedup();
        edge
      })
      .collect();
    let facts = OwnedFacts {
      entities: self.entities.into_values().collect(),
      edges,
      unresolved: self.unresolved,
    };
    FinalOwner {
      key: self.key,
      facts,
      symbols: self.symbols,
      counts: self.counts,
      wanted,
    }
  }
}

pub(crate) fn evidence_order(left: &Evidence, right: &Evidence) -> Ordering {
  (&left.repo, &left.path, &left.span, &left.content_hash, &left.producer, &left.note).cmp(&(
    &right.repo,
    &right.path,
    &right.span,
    &right.content_hash,
    &right.producer,
    &right.note,
  ))
}

pub(crate) fn weaker(left: Certainty, right: Certainty) -> Certainty {
  match left.cmp_strength(&right) {
    Ordering::Greater => right,
    _ => left,
  }
}

pub(crate) fn merge_edge(edges: &mut BTreeMap<EdgeId, Edge>, edge: Edge) {
  match edges.entry(edge.id) {
    Entry::Vacant(slot) => {
      slot.insert(edge);
    }
    Entry::Occupied(slot) => absorb(slot.into_mut(), edge),
  }
}

fn absorb(existing: &mut Edge, edge: Edge) {
  if edge.certainty.cmp_strength(&existing.certainty) == Ordering::Greater {
    existing.certainty = edge.certainty;
  }
  let same_attrs = existing.attrs.iter().filter(|(name, _)| *name != ATTR_SITE_ATTRS).eq(edge.attrs.iter());
  match (existing.attrs.is_empty(), same_attrs || edge.attrs.is_empty()) {
    (true, _) => existing.attrs = edge.attrs,
    (false, true) => {}
    (false, false) => {
      let site = json!({ "span": edge.evidence.first().map(|evidence| evidence.span.start_byte), "attrs": edge.attrs });
      match existing.attrs.get_mut(ATTR_SITE_ATTRS) {
        Some(Value::Array(sites)) => sites.push(site),
        _ => {
          existing.attrs.insert(ATTR_SITE_ATTRS.to_owned(), Value::Array(vec![site]));
        }
      }
    }
  }
  for evidence in edge.evidence {
    if !existing.evidence.contains(&evidence) {
      existing.evidence.push(evidence);
    }
  }
}

pub(crate) fn engine_evidence(repo: &RepoName, path: &RelPath, content_hash: ContentHash) -> Evidence {
  Evidence {
    repo: repo.clone(),
    path: path.clone(),
    span: Span::from_zero_based_rows(0, 0, 0, 0, 0, 0),
    content_hash,
    producer: Producer::new(ENGINE_PRODUCER, ENGINE_PRODUCER_VERSION),
    note: None,
  }
}

fn parse_status_label(status: &ParseStatus) -> &'static str {
  match status {
    ParseStatus::Ok => "ok",
    ParseStatus::Partial { .. } => "partial",
    ParseStatus::Failed { .. } => "failed",
  }
}

fn file_entity(artifact: &FileArtifact) -> Entity {
  let facts = &artifact.facts;
  let key = EntityKey::File {
    repo: facts.file.repo.clone(),
    path: facts.file.path.clone(),
  };
  let mut entity = Entity::new(key, Certainty::Syntactic);
  entity.path = Some(facts.file.path.clone());
  entity.doc = facts.file_doc.clone();
  let attrs = &mut entity.attrs;
  attrs.insert(atlas_core::ATTR_LANGUAGE.to_owned(), json!(facts.file.language.as_str()));
  attrs.insert("parse_status".to_owned(), json!(parse_status_label(&facts.parse_status)));
  match &facts.parse_status {
    ParseStatus::Failed { reason } => {
      attrs.insert("parse_error".to_owned(), json!(reason));
    }
    ParseStatus::Partial { error_count, .. } => {
      attrs.insert("parse_error_count".to_owned(), json!(error_count));
    }
    ParseStatus::Ok => {}
  }
  attrs.insert(ATTR_GENERATED.to_owned(), json!(artifact.generated));
  attrs.insert("size".to_owned(), json!(artifact.size));
  let diagnostics: Vec<Value> = artifact
    .detections
    .iter()
    .flat_map(|output| {
      output
        .diagnostics
        .iter()
        .map(move |diagnostic| json!({ "detector": output.detector, "diagnostic": diagnostic }))
    })
    .collect();
  if !diagnostics.is_empty() {
    attrs.insert("detector_diagnostics".to_owned(), Value::Array(diagnostics));
  }
  entity
}

struct EndpointResolver<'a> {
  facts: &'a FileFacts,
  table: &'a ProjectSymbolTable,
  symbols: &'a [SymbolEntry],
}

enum Endpoint {
  Known(Vec<(EntityId, Certainty)>),
  Required(EntityId),
  Unresolvable,
}

impl EndpointResolver<'_> {
  fn resolve(&self, node: &NodeRef) -> EngineResult<Endpoint> {
    match node {
      NodeRef::Local(local) => Ok(match self.symbols.get(local.index()) {
        Some(symbol) => Endpoint::Known(vec![(symbol.id, Certainty::Syntactic)]),
        None => Endpoint::Unresolvable,
      }),
      NodeRef::Key(key) => Ok(Endpoint::Required(EntityId::of(key))),
      NodeRef::Unresolved { path, from } => Ok(match resolve_path_in_file(self.facts, path, *from, self.table)? {
        Resolution::Resolved(id, certainty) => Endpoint::Known(vec![(id, certainty)]),
        Resolution::Candidates(candidates) if !candidates.is_empty() => Endpoint::Known(
          candidates
            .into_iter()
            .map(|(id, confidence)| (id, Certainty::heuristic(confidence, RULE_DETECTOR_CANDIDATE)))
            .collect(),
        ),
        Resolution::Candidates(_) | Resolution::Unresolved(_) => Endpoint::Unresolvable,
      }),
    }
  }
}

impl Endpoint {
  fn candidates(&self) -> Option<Vec<(EntityId, Certainty)>> {
    match self {
      Self::Known(targets) => Some(targets.clone()),
      Self::Required(id) => Some(vec![(*id, Certainty::Syntactic)]),
      Self::Unresolvable => None,
    }
  }

  fn required(&self) -> Option<EntityId> {
    match self {
      Self::Required(id) => Some(*id),
      Self::Known(_) | Self::Unresolvable => None,
    }
  }
}

pub(crate) fn build_owner(artifact: &FileArtifact, table: &ProjectSymbolTable, graph: Option<&PackageGraph>) -> EngineResult<OwnerDraft> {
  let facts = &artifact.facts;
  let repo = &facts.file.repo;
  let path = &facts.file.path;
  let placement = table.placement(repo, path).ok_or_else(|| EngineError::NotIndexed {
    repo: repo.clone(),
    path: path.clone(),
  })?;
  let keys = facts.symbol_keys(&placement.context)?;
  let file = file_entity(artifact);
  let file_id = file.id;
  let mut entities = BTreeMap::new();
  entities.insert(file_id, file);
  let mut symbols = Vec::with_capacity(keys.len());
  for (local, (fact, key)) in facts.symbols.iter().zip(keys).enumerate() {
    let mut entity = Entity::from_symbol(fact, key.clone(), &facts.file);
    if artifact.generated {
      entity.attrs.insert(ATTR_GENERATED.to_owned(), json!(true));
    }
    symbols.push(SymbolEntry {
      id: entity.id,
      key,
      kind: fact.kind.clone(),
      local,
    });
    entities.entry(entity.id).or_insert(entity);
  }
  for output in &artifact.detections {
    for detected in &output.detection.entities {
      let mut entity = Entity::new(detected.key.clone(), detected.certainty.clone()).located(path.clone(), detected.span);
      entity.name = detected.name.clone();
      entity.attrs = detected.attrs.clone();
      entities.entry(entity.id).or_insert(entity);
    }
  }
  let mut edges = BTreeMap::new();
  let mut unresolved = Vec::new();
  if !matches!(facts.parse_status, ParseStatus::Failed { .. }) {
    let resolved = resolve_file(facts, table)?;
    unresolved = resolved.unresolved;
    for edge in resolved.edges.into_iter().chain(resolved.structural) {
      merge_edge(&mut edges, edge);
    }
  }
  let container = graph
    .and_then(|graph| package_key_for_file(graph, path))
    .unwrap_or(EntityKey::Repository { repo: repo.clone() });
  let contains = Edge::new(EdgeKind::Contains, EntityId::of(&container), file_id, "", Certainty::Syntactic).with_evidence(engine_evidence(repo, path, facts.file.content_hash));
  merge_edge(&mut edges, contains);
  let resolver = EndpointResolver { facts, table, symbols: &symbols };
  let mut conditional: BTreeMap<EdgeId, ConditionalEdge> = BTreeMap::new();
  let mut counts = DraftCounts::default();
  for output in &artifact.detections {
    for detected in &output.detection.edges {
      let (source, target) = (resolver.resolve(&detected.from)?, resolver.resolve(&detected.to)?);
      let (Some(sources), Some(targets)) = (source.candidates(), target.candidates()) else {
        counts.unresolvable_detector_edges += 1;
        continue;
      };
      let requires: BTreeSet<EntityId> = source.required().into_iter().chain(target.required()).collect();
      for (from, from_certainty) in &sources {
        for (to, to_certainty) in &targets {
          let edge = detector_edge(detected, output.producer.clone(), facts, *from, *to, weaker(from_certainty.clone(), to_certainty.clone()));
          match requires.is_empty() {
            true => merge_edge(&mut edges, edge),
            false => match conditional.entry(edge.id) {
              Entry::Vacant(slot) => {
                slot.insert(ConditionalEdge { edge, requires: requires.clone() });
              }
              Entry::Occupied(slot) => absorb(&mut slot.into_mut().edge, edge),
            },
          }
        }
      }
    }
  }
  Ok(OwnerDraft {
    key: (repo.clone(), path.clone()),
    entities,
    edges,
    conditional,
    unresolved,
    symbols,
    counts,
  })
}

fn detector_edge(detected: &DetectedEdge, producer: Producer, facts: &FileFacts, from: EntityId, to: EntityId, resolution: Certainty) -> Edge {
  let certainty = weaker(detected.certainty.clone(), resolution);
  let mut edge = Edge::new(detected.kind.clone(), from, to, "", certainty).with_evidence(Evidence {
    repo: facts.file.repo.clone(),
    path: facts.file.path.clone(),
    span: detected.span,
    content_hash: facts.file.content_hash,
    producer,
    note: detected.note.clone(),
  });
  edge.attrs = detected.attrs.clone();
  edge
}
