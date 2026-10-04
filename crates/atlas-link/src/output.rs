use std::cmp::Ordering;
use std::collections::BTreeMap;

use atlas_core::{Certainty, Confidence, ContentHash, Edge, EdgeId, Entity, EntityId, Evidence, Producer, RelPath, RepoName, Span};
use serde_json::Value;

use crate::error::LinkResult;
use crate::facts::sort_evidence;
use crate::pass::LinkOutput;

const PRODUCER: &str = "atlas-link";
const PRODUCER_VERSION: u32 = 1;
const ABSENCE_CONFIDENCE: f32 = 0.5;
const RULE_DERIVED: &str = "link-derived";

#[derive(Default)]
pub(crate) struct Output {
  entities: BTreeMap<EntityId, Entity>,
  edges: BTreeMap<EdgeId, Edge>,
}

impl Output {
  pub fn entity(&mut self, entity: Entity) {
    match self.entities.get(&entity.id) {
      Some(existing) if existing.certainty.cmp_strength(&entity.certainty) != Ordering::Less => {}
      _ => {
        self.entities.insert(entity.id, entity);
      }
    }
  }

  pub fn extend_entity(&mut self, entity: &Entity, certainty: Certainty, attrs: impl IntoIterator<Item = (&'static str, Value)>) {
    match self.entities.get_mut(&entity.id) {
      Some(existing) => {
        existing.certainty = weaker(&existing.certainty, &certainty);
        existing.attrs.extend(attrs.into_iter().map(|(name, value)| (name.to_owned(), value)));
      }
      None => {
        self.entities.insert(entity.id, annotated(entity, certainty, attrs));
      }
    }
  }

  pub fn edge(&mut self, mut edge: Edge) {
    let Some(existing) = self.edges.get_mut(&edge.id) else {
      self.edges.insert(edge.id, edge);
      return;
    };
    if edge.certainty.cmp_strength(&existing.certainty) == Ordering::Greater {
      existing.certainty = edge.certainty;
      existing.attrs = std::mem::take(&mut edge.attrs);
    }
    existing.evidence.append(&mut edge.evidence);
  }

  pub fn entity_count(&self) -> usize {
    self.entities.len()
  }

  pub fn finish(self) -> LinkOutput {
    let edges = self
      .edges
      .into_values()
      .map(|mut edge| {
        sort_evidence(&mut edge.evidence);
        edge
      })
      .collect();
    LinkOutput {
      entities: self.entities.into_values().collect(),
      edges,
    }
  }
}

pub(crate) fn evidence(repo: &RepoName, path: &RelPath, span: Span, content_hash: ContentHash, note: Option<String>) -> Evidence {
  Evidence {
    repo: repo.clone(),
    path: path.clone(),
    span,
    content_hash,
    producer: Producer::new(PRODUCER, PRODUCER_VERSION),
    note,
  }
}

pub(crate) fn relabel(source: &Evidence, note: Option<String>) -> Evidence {
  evidence(&source.repo, &source.path, source.span, source.content_hash, note)
}

pub(crate) fn rounded(value: f32) -> f32 {
  (value * 1000.0).round() / 1000.0
}

pub(crate) fn heuristic(value: f32, rule: &str) -> LinkResult<Certainty> {
  let bounded = if value.is_nan() { value } else { rounded(value.clamp(0.01, 0.99)) };
  Ok(Certainty::heuristic(Confidence::new(bounded)?, rule))
}

pub(crate) fn confidence_of(certainty: &Certainty) -> f32 {
  certainty.confidence().map_or(1.0, Confidence::value)
}

pub(crate) fn weaker(left: &Certainty, right: &Certainty) -> Certainty {
  match left.cmp_strength(right) {
    Ordering::Greater => right.clone(),
    _ => left.clone(),
  }
}

pub(crate) fn derived<'a>(ceiling: &Certainty, inputs: impl IntoIterator<Item = &'a Certainty>) -> Certainty {
  let weakest = inputs.into_iter().fold(ceiling, |weakest, input| match input.cmp_strength(weakest) {
    Ordering::Less => input,
    _ => weakest,
  });
  match weakest {
    Certainty::Heuristic { confidence, .. } => Certainty::heuristic(*confidence, RULE_DERIVED),
    other => other.clone(),
  }
}

pub(crate) fn absence(rule: &str) -> LinkResult<Certainty> {
  heuristic(ABSENCE_CONFIDENCE, rule)
}

pub(crate) fn annotated(entity: &Entity, certainty: Certainty, attrs: impl IntoIterator<Item = (&'static str, Value)>) -> Entity {
  let mut copy = entity.clone();
  copy.certainty = certainty;
  copy.attrs.clear();
  for (name, value) in attrs {
    copy.attrs.insert(name.to_owned(), value);
  }
  copy
}

pub(crate) fn attr_str<'a>(attrs: &'a atlas_core::Attrs, name: &str) -> Option<&'a str> {
  attrs.get(name).and_then(Value::as_str)
}
