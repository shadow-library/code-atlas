#![allow(dead_code)]

use atlas_core::{
  Certainty, Confidence, ContentHash, Edge, EdgeKind, Entity, EntityId, EntityKey, EntityKind, Evidence, LanguageFamily, Owner, Producer, RefKind, RelPath, RepoName, Span,
  SymbolKey, UnresolvedReason, UnresolvedRef,
};
use atlas_store::{OwnedFacts, Store, StoreError};

pub fn repo() -> RepoName {
  RepoName::new("acme").unwrap()
}

pub fn rel(path: &str) -> RelPath {
  RelPath::new(path).unwrap()
}

pub fn file_owner(path: &str) -> Owner {
  Owner::File { repo: repo(), path: rel(path) }
}

pub fn symbol_key(name: &str) -> EntityKey {
  EntityKey::Symbol(SymbolKey::new(LanguageFamily::Rust, repo(), "acme", vec!["lib".into()], vec![name.into()], EntityKind::Function, 0).unwrap())
}

pub fn span(start: u32) -> Span {
  Span::from_zero_based_rows(start, start + 10, start, 0, start, 10)
}

pub fn entity_in(name: &str, path: &str) -> Entity {
  Entity::new(symbol_key(name), Certainty::Syntactic).located(rel(path), span(1))
}

pub fn entity(name: &str) -> Entity {
  entity_in(name, "src/lib.rs")
}

pub fn id(name: &str) -> EntityId {
  EntityId::of(&symbol_key(name))
}

pub fn evidence(path: &str, start: u32) -> Evidence {
  Evidence {
    repo: repo(),
    path: rel(path),
    span: span(start),
    content_hash: ContentHash::of(path.as_bytes()),
    producer: Producer::new("test", 1),
    note: None,
  }
}

pub fn heuristic(confidence: f32) -> Certainty {
  Certainty::heuristic(Confidence::new(confidence).unwrap(), "name-match")
}

pub fn inferred(confidence: f32) -> Certainty {
  Certainty::Inferred {
    confidence: Confidence::new(confidence).unwrap(),
    provider: "fake".into(),
    model: "m".into(),
    rationale: "same shape".into(),
  }
}

pub fn edge(kind: EdgeKind, from: &str, to: &str, certainty: Certainty, evidence_path: &str) -> Edge {
  Edge::new(kind, id(from), id(to), "", certainty).with_evidence(evidence(evidence_path, 5))
}

pub fn calls(from: &str, to: &str) -> Edge {
  edge(EdgeKind::Calls, from, to, Certainty::Resolved, "src/lib.rs")
}

pub fn unresolved(from: &str, path: &[&str], file: &str) -> UnresolvedRef {
  UnresolvedRef {
    from: id(from),
    path: path.iter().map(|segment| (*segment).to_owned()).collect(),
    kind: RefKind::Call,
    receiver: None,
    reason: UnresolvedReason::NotFound,
    repo: repo(),
    file: rel(file),
    span: span(3),
  }
}

pub fn facts(entities: Vec<Entity>, edges: Vec<Edge>) -> OwnedFacts {
  OwnedFacts {
    entities,
    edges,
    unresolved: Vec::new(),
  }
}

pub fn replace(store: &Store, owner: &Owner, owned: &OwnedFacts) -> atlas_store::FactDelta {
  store.write(|tx| tx.replace_owner(owner, owned)).unwrap()
}

pub fn read<T>(store: &Store, f: impl FnOnce(&atlas_store::ReadTx<'_>) -> Result<T, StoreError>) -> T {
  store.read(f).unwrap()
}

pub fn sorted<T: Ord>(mut items: Vec<T>) -> Vec<T> {
  items.sort();
  items
}
