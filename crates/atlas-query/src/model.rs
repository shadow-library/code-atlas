use std::collections::BTreeMap;
use std::fmt;

use atlas_core::{Certainty, Entity, EntityKey, Freshness as FactFreshness, LanguageFamily};
use serde::Serialize;

use crate::freshness::Freshness;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Location {
  pub repo: String,
  pub path: String,
  pub line: Option<u32>,
  /// 1-based UTF-8 byte column
  #[serde(skip_serializing_if = "Option::is_none")]
  pub column: Option<u32>,
}

impl fmt::Display for Location {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match (self.line, self.column) {
      (Some(line), Some(column)) => write!(f, "{}:{}:{line}:{column}", self.repo, self.path),
      (Some(line), None) => write!(f, "{}:{}:{line}", self.repo, self.path),
      (None, _) => write!(f, "{}:{}", self.repo, self.path),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EntityRef {
  pub key: String,
  pub id: String,
  pub short: String,
  pub kind: String,
  pub location: Option<Location>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub certainty: Option<Box<CertaintyTag>>,
}

impl EntityRef {
  pub fn of(entity: &Entity) -> Self {
    Self {
      key: entity.key.to_string(),
      id: entity.id.to_string(),
      short: short_name(&entity.key),
      kind: entity.kind.as_str().to_owned(),
      location: location_of(entity),
      certainty: non_authoritative(&entity.certainty),
    }
  }

  pub fn tag(&self) -> Option<&str> {
    self.certainty.as_ref().map(|certainty| certainty.tag.as_str())
  }

  pub fn tagged_short(&self) -> String {
    match self.tag() {
      Some(tag) => format!("{} {tag}", self.short),
      None => self.short.clone(),
    }
  }

  pub fn entity_id(&self) -> crate::error::QueryResult<atlas_core::EntityId> {
    self.id.parse().map_err(|_| crate::error::QueryError::invalid(format!("malformed entity id {}", self.id)))
  }

  pub fn repo(&self) -> &str {
    self.location.as_ref().map_or("", |location| location.repo.as_str())
  }

  pub(crate) fn sort_key(&self) -> (String, String, Option<u32>, String) {
    let (repo, path, line) = match &self.location {
      Some(location) => (location.repo.clone(), location.path.clone(), location.line),
      None => (String::new(), String::new(), None),
    };
    (repo, path, line, self.key.clone())
  }
}

pub(crate) fn non_authoritative(certainty: &Certainty) -> Option<Box<CertaintyTag>> {
  Some(CertaintyTag::of(certainty)).filter(|tag| !tag.authoritative).map(Box::new)
}

pub(crate) fn location_of(entity: &Entity) -> Option<Location> {
  let repo = entity.repo.as_ref()?;
  let path = entity.path.as_ref().map(ToString::to_string).or_else(|| match &entity.key {
    EntityKey::File { path, .. } => Some(path.to_string()),
    _ => None,
  })?;
  Some(Location {
    repo: repo.to_string(),
    path,
    line: entity.span.map(|span| span.start_line),
    column: None,
  })
}

pub fn short_name(key: &EntityKey) -> String {
  match key {
    EntityKey::Symbol(symbol) => {
      let separator = match symbol.family() {
        LanguageFamily::Rust => "::",
        _ => ".",
      };
      match symbol.item_path() {
        [] if symbol.module_path().is_empty() => symbol.package().to_owned(),
        [] => symbol.module_path().join("/"),
        items => items.join(separator),
      }
    }
    other => other.display_name(),
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CertaintyTag {
  pub class: String,
  pub confidence: Option<f32>,
  pub freshness: Option<String>,
  pub authoritative: bool,
  pub tag: String,
}

impl CertaintyTag {
  pub fn of(certainty: &Certainty) -> Self {
    Self {
      class: certainty.class().as_str().to_owned(),
      confidence: certainty.confidence().map(f32::from),
      freshness: None,
      authoritative: certainty.is_authoritative(),
      tag: certainty.tag(),
    }
  }

  pub fn inferred(certainty: &Certainty, freshness: &FactFreshness) -> Self {
    let state = match freshness {
      FactFreshness::Current => None,
      FactFreshness::Stale { reason } => Some(format!("stale: {reason}")),
      FactFreshness::NeedsRevalidation { reason } => Some(format!("needs revalidation: {reason}")),
    };
    Self {
      class: certainty.class().as_str().to_owned(),
      confidence: certainty.confidence().map(f32::from),
      freshness: state,
      authoritative: false,
      tag: certainty.tag_with_freshness(freshness),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AttrView {
  pub name: String,
  pub value: serde_json::Value,
  pub owner: String,
  pub certainty: CertaintyTag,
}

impl AttrView {
  pub fn tag(&self) -> String {
    let tag = &self.certainty.tag;
    format!("{}, {}]", &tag[..tag.len().saturating_sub(1)], self.owner)
  }

  pub fn value_text(&self) -> String {
    match &self.value {
      serde_json::Value::String(text) => text.clone(),
      other => other.to_string(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hop {
  pub edge: String,
  pub kind: String,
  pub phrase: String,
  pub entity: String,
  pub certainty: CertaintyTag,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Listing<T> {
  pub total: usize,
  pub items: Vec<T>,
}

impl<T> Listing<T> {
  pub fn truncated(mut items: Vec<T>, limit: usize) -> Self {
    let total = items.len();
    items.truncate(limit);
    Self { total, items }
  }

  pub fn omitted(&self) -> usize {
    self.total.saturating_sub(self.items.len())
  }

  pub fn is_empty(&self) -> bool {
    self.total == 0
  }
}

impl<T> Default for Listing<T> {
  fn default() -> Self {
    Self { total: 0, items: Vec::new() }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnresolvedSite {
  pub location: Location,
  pub path: String,
  pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProblemFile {
  pub location: Location,
  pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MacroGapScope {
  Callees,
  Dependencies,
  Impact,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "caveat", rename_all = "snake_case")]
pub enum Caveat {
  Truncated {
    max_nodes: usize,
  },
  UnresolvedRisk {
    name: String,
    total: usize,
    by_reason: BTreeMap<String, usize>,
    sites: Vec<UnresolvedSite>,
  },
  DynamicMemberCalls {
    total: usize,
    sites: Vec<UnresolvedSite>,
  },
  MacroGeneratedRefs {
    target: String,
    scope: MacroGapScope,
    total: usize,
    sites: Vec<UnresolvedSite>,
  },
  ProblemFiles {
    total: usize,
    files: Vec<ProblemFile>,
  },
  BelowConfidenceFloor {
    hidden: usize,
    floor: f32,
  },
  InferredHidden {
    stale: usize,
    low_confidence: usize,
  },
  MissingFacts {
    detail: String,
  },
  Note {
    message: String,
  },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Targeted<T> {
  Found(T),
  Ambiguous { spec: String, candidates: Vec<EntityRef> },
  NotFound { spec: String, hint: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Response<T> {
  pub freshness: Freshness,
  pub answer: T,
}
