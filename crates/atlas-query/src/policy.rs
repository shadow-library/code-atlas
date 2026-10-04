use atlas_core::{Certainty, EdgeKind};
use atlas_semantic::{included_in_default_results, is_low_confidence};
use atlas_store::StoredInferredFact;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Policy {
  pub min_confidence: f32,
  pub include_stale: bool,
}

impl Policy {
  pub fn with_floor(min_confidence: f32) -> Self {
    Self {
      min_confidence,
      include_stale: false,
    }
  }

  pub fn admits(&self, certainty: &Certainty) -> bool {
    match certainty.confidence() {
      Some(confidence) if !certainty.is_authoritative() => f32::from(confidence) >= self.min_confidence,
      _ => true,
    }
  }

  pub(crate) fn inferred_admission(&self, fact: &StoredInferredFact) -> InferredAdmission {
    if is_low_confidence(fact) {
      return InferredAdmission::LowConfidence;
    }
    if included_in_default_results(fact) {
      return InferredAdmission::Current;
    }
    match self.include_stale {
      true => InferredAdmission::StaleShown,
      false => InferredAdmission::StaleHidden,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InferredAdmission {
  Current,
  StaleShown,
  StaleHidden,
  LowConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
  Forward,
  Reverse,
}

pub(crate) fn phrase(kind: &EdgeKind, direction: Direction, certainty: &Certainty) -> String {
  let label = match direction {
    Direction::Forward => kind.forward_label(),
    Direction::Reverse => kind.reverse_label(),
  };
  hedge(&label, certainty)
}

pub(crate) fn hedge(label: &str, certainty: &Certainty) -> String {
  match certainty {
    Certainty::Heuristic { .. } => format!("possibly {label}"),
    Certainty::Inferred { .. } => format!("likely {label}"),
    _ => label.to_owned(),
  }
}
