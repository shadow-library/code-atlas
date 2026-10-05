use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::facts::{FileRef, ReceiverHint, RefKind, SymbolFact, Visibility};
use crate::ids::{EdgeId, EntityId, RelPath, RepoName};
use crate::keys::{EntityKey, SymbolKey};
use crate::kinds::{EdgeKind, EntityKind};
use crate::provenance::{Certainty, Evidence, Freshness};
use crate::span::Span;

pub type Attrs = BTreeMap<String, serde_json::Value>;

pub const ATTR_LANGUAGE: &str = "language";
pub const ATTR_MODIFIERS: &str = "modifiers";
pub const ATTR_ATTRIBUTES: &str = "attributes";
pub const ATTR_PARAMS: &str = "params";
pub const ATTR_TEST: &str = "test";
pub const ATTR_IMPL_OF: &str = "impl_of";
pub const ATTR_EXTENDS: &str = "extends";
pub const ATTR_IMPLEMENTS: &str = "implements";
pub const ATTR_OVERRIDE_VIA: &str = "via";

fn insert_attr<T: Serialize>(attrs: &mut Attrs, name: &str, value: &T) {
  if let Ok(json) = serde_json::to_value(value) {
    attrs.insert(name.to_owned(), json);
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
  pub id: EntityId,
  pub key: EntityKey,
  pub kind: EntityKind,
  pub name: String,
  pub repo: Option<RepoName>,
  pub path: Option<RelPath>,
  pub span: Option<Span>,
  pub signature: Option<String>,
  pub doc: Option<String>,
  pub visibility: Option<Visibility>,
  pub certainty: Certainty,
  pub attrs: Attrs,
}

impl Entity {
  pub fn new(key: EntityKey, certainty: Certainty) -> Self {
    Self {
      id: EntityId::of(&key),
      kind: key.kind(),
      name: key.display_name(),
      repo: key.repo().cloned(),
      key,
      path: None,
      span: None,
      signature: None,
      doc: None,
      visibility: None,
      certainty,
      attrs: Attrs::new(),
    }
  }

  pub fn from_symbol(fact: &SymbolFact, key: SymbolKey, file: &FileRef) -> Self {
    let mut entity = Self::new(EntityKey::Symbol(key), Certainty::Syntactic).located(file.path.clone(), fact.span);
    entity.name = fact.name.clone();
    entity.signature = fact.signature.clone();
    entity.doc = fact.doc.clone();
    entity.visibility = Some(fact.visibility.clone());
    let attrs = &mut entity.attrs;
    insert_attr(attrs, ATTR_LANGUAGE, &file.language);
    if !fact.modifiers.is_empty() {
      insert_attr(attrs, ATTR_MODIFIERS, &fact.modifiers);
    }
    if !fact.attributes.is_empty() {
      insert_attr(attrs, ATTR_ATTRIBUTES, &fact.attributes);
    }
    if !fact.params.is_empty() {
      insert_attr(attrs, ATTR_PARAMS, &fact.params);
    }
    if let Some(test) = &fact.test {
      insert_attr(attrs, ATTR_TEST, test);
    }
    if let Some(impl_of) = &fact.impl_of {
      insert_attr(attrs, ATTR_IMPL_OF, impl_of);
    }
    if !fact.extends.is_empty() {
      insert_attr(attrs, ATTR_EXTENDS, &fact.extends);
    }
    if !fact.implements.is_empty() {
      insert_attr(attrs, ATTR_IMPLEMENTS, &fact.implements);
    }
    entity
  }

  pub fn located(mut self, path: RelPath, span: Span) -> Self {
    self.path = Some(path);
    self.span = Some(span);
    self
  }

  pub fn is_consistent(&self) -> bool {
    self.id == EntityId::of(&self.key) && self.kind == self.key.kind() && self.repo.as_ref() == self.key.repo()
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
  pub id: EdgeId,
  pub kind: EdgeKind,
  pub from: EntityId,
  pub to: EntityId,
  pub certainty: Certainty,
  pub evidence: Vec<Evidence>,
  pub attrs: Attrs,
}

impl Edge {
  pub fn new(kind: EdgeKind, from: EntityId, to: EntityId, discriminator: &str, certainty: Certainty) -> Self {
    Self {
      id: EdgeId::of(&kind, from, to, discriminator),
      kind,
      from,
      to,
      certainty,
      evidence: Vec::new(),
      attrs: Attrs::new(),
    }
  }

  pub fn with_evidence(mut self, evidence: Evidence) -> Self {
    self.evidence.push(evidence);
    self
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
#[non_exhaustive]
pub enum UnresolvedReason {
  NotFound,
  Ambiguous { candidates: Vec<EntityId> },
  Dynamic,
  External { package: String },
  Unsupported { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UnresolvedRef {
  pub from: EntityId,
  pub path: Vec<String>,
  pub kind: RefKind,
  pub receiver: Option<ReceiverHint>,
  pub reason: UnresolvedReason,
  pub repo: RepoName,
  pub file: RelPath,
  pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "owner", rename_all = "snake_case")]
pub enum Owner {
  File { repo: RepoName, path: RelPath },
  Project { pass: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferredFact<T> {
  pub fact: T,
  pub freshness: Freshness,
}

impl InferredFact<Edge> {
  pub fn tag(&self) -> String {
    self.fact.certainty.tag_with_freshness(&self.freshness)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::facts::{AttributeFact, LocalSymbolId, Modifier, Modifiers, TestInfo, TestKind};
  use crate::ids::ContentHash;
  use crate::keys::HttpMethod;
  use crate::lang::LanguageFamily;
  use crate::provenance::{Confidence, Producer};

  #[test]
  fn should_build_symbol_entities_with_deterministic_attrs() {
    let file = FileRef::detect(repo(), RelPath::new("src/users.spec.ts").unwrap(), ContentHash::of(b"x")).unwrap();
    let span = Span::from_zero_based_rows(10, 60, 1, 0, 3, 2);
    let name_span = Span::from_zero_based_rows(14, 30, 1, 4, 1, 20);
    let mut fact = SymbolFact::new(
      LocalSymbolId(0),
      EntityKind::Test,
      "should find a user",
      vec!["should find a user".into()],
      span,
      name_span,
      Visibility::Module,
    );
    fact.signature = Some("it('should find a user')".into());
    fact.doc = Some("Finds users.".into());
    fact.modifiers = Modifiers::empty().with(Modifier::Static).with(Modifier::Async);
    fact.attributes.push(AttributeFact {
      path: vec!["Skip".into()],
      args: vec![],
      span: name_span,
    });
    fact.test = Some(TestInfo {
      framework: "jest".into(),
      kind: TestKind::Case,
      display_name: Some("should find a user".into()),
    });
    let key = SymbolKey::new(
      LanguageFamily::Ts,
      repo(),
      "@acme/api",
      vec!["src".into(), "users.spec".into()],
      fact.item_path.clone(),
      EntityKind::Test,
      0,
    )
    .unwrap();

    let entity = Entity::from_symbol(&fact, key.clone(), &file);

    assert!(entity.is_consistent());
    assert_eq!(entity.id, EntityId::of(&EntityKey::Symbol(key)));
    assert_eq!(entity.certainty, Certainty::Syntactic);
    assert_eq!(entity.name, "should find a user");
    assert_eq!(entity.path.as_ref().map(RelPath::as_str), Some("src/users.spec.ts"));
    assert_eq!(entity.span, Some(span));
    assert_eq!(entity.signature.as_deref(), Some("it('should find a user')"));
    assert_eq!(entity.doc.as_deref(), Some("Finds users."));
    assert_eq!(entity.visibility, Some(Visibility::Module));
    assert_eq!(
      entity.attrs.keys().map(String::as_str).collect::<Vec<_>>(),
      [ATTR_ATTRIBUTES, ATTR_LANGUAGE, ATTR_MODIFIERS, ATTR_TEST]
    );
    assert_eq!(entity.attrs[ATTR_MODIFIERS], serde_json::json!(["async", "static"]));
    assert_eq!(entity.attrs[ATTR_LANGUAGE], serde_json::json!("typescript"));
    assert_eq!(
      entity.attrs[ATTR_TEST],
      serde_json::json!({ "framework": "jest", "kind": "case", "display_name": "should find a user" })
    );
    assert_eq!(entity.attrs[ATTR_ATTRIBUTES][0]["path"], serde_json::json!(["Skip"]));
    let again = Entity::from_symbol(&fact, entity.key.as_symbol().unwrap().clone(), &file);
    assert_eq!(serde_json::to_string(&again).unwrap(), serde_json::to_string(&entity).unwrap());
  }

  fn repo() -> RepoName {
    RepoName::new("acme-api").unwrap()
  }

  fn endpoint() -> EntityKey {
    EntityKey::Endpoint {
      repo: repo(),
      method: HttpMethod::Get,
      route: "/users/{id}".into(),
    }
  }

  #[test]
  fn should_derive_entity_identity_from_its_key() {
    let entity = Entity::new(endpoint(), Certainty::Syntactic).located(RelPath::new("src/users.controller.ts").unwrap(), Span::from_zero_based_rows(0, 4, 0, 0, 0, 4));
    assert_eq!(entity.id, EntityId::of(&endpoint()));
    assert_eq!(entity.kind, EntityKind::ApiEndpoint);
    assert_eq!(entity.name, "GET /users/{id}");
    assert_eq!(entity.repo, Some(repo()));
    assert!(entity.is_consistent());
    let mut tampered = entity.clone();
    tampered.kind = EntityKind::Function;
    assert!(!tampered.is_consistent());
  }

  #[test]
  fn should_round_trip_entities_and_edges_through_json() {
    let mut entity = Entity::new(endpoint(), Certainty::Syntactic);
    entity.attrs.insert("framework".into(), serde_json::json!("nestjs"));
    assert_eq!(serde_json::from_str::<Entity>(&serde_json::to_string(&entity).unwrap()).unwrap(), entity);

    let caller = EntityId::from_raw(7);
    let edge = Edge::new(
      EdgeKind::CallsEndpoint,
      caller,
      entity.id,
      "",
      Certainty::heuristic(Confidence::new(0.7).unwrap(), "route-template"),
    )
    .with_evidence(Evidence {
      repo: RepoName::new("acme-web").unwrap(),
      path: RelPath::new("src/api.ts").unwrap(),
      span: Span::from_zero_based_rows(10, 30, 2, 4, 2, 24),
      content_hash: ContentHash::of(b"web"),
      producer: Producer::new("link.api", 1),
      note: Some("GET `${BASE}/users/${id}`".into()),
    });
    assert_eq!(edge.id, EdgeId::of(&EdgeKind::CallsEndpoint, caller, entity.id, ""));
    assert_eq!(serde_json::from_str::<Edge>(&serde_json::to_string(&edge).unwrap()).unwrap(), edge);
  }

  #[test]
  fn should_serialise_unresolved_reasons_and_owners_as_tagged_unions() {
    let ambiguous = UnresolvedReason::Ambiguous {
      candidates: vec![EntityId::from_raw(1)],
    };
    assert_eq!(
      serde_json::to_value(&ambiguous).unwrap(),
      serde_json::json!({ "reason": "ambiguous", "candidates": ["0000000000000001"] })
    );
    let owner = Owner::File {
      repo: repo(),
      path: RelPath::new("src/a.ts").unwrap(),
    };
    assert_eq!(
      serde_json::to_value(&owner).unwrap(),
      serde_json::json!({ "owner": "file", "repo": "acme-api", "path": "src/a.ts" })
    );
    assert_eq!(
      serde_json::from_value::<Owner>(serde_json::json!({ "owner": "project", "pass": "link" })).unwrap(),
      Owner::Project { pass: "link".into() }
    );
  }

  #[test]
  fn should_tag_inferred_edges_with_their_freshness() {
    let certainty = Certainty::Inferred {
      confidence: Confidence::new(0.81).unwrap(),
      provider: "claude-cli".into(),
      model: "m".into(),
      rationale: "same rounding".into(),
    };
    let fact = InferredFact {
      fact: Edge::new(EdgeKind::EquivalentTo, EntityId::from_raw(1), EntityId::from_raw(2), "", certainty),
      freshness: Freshness::Stale { reason: "changed".into() },
    };
    assert_eq!(fact.tag(), "[inferred 0.81 stale]");
  }
}
