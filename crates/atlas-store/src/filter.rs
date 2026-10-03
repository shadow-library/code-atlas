use atlas_core::{CertaintyClass, EdgeKind, EntityKind};

use crate::codec::certainty_tier;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InferredPolicy {
  #[default]
  Include,
  Exclude,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdgeFilter {
  pub kinds: Vec<EdgeKind>,
  pub min_certainty: Option<CertaintyClass>,
  pub inferred: InferredPolicy,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntityFilter {
  pub kinds: Vec<EntityKind>,
  pub min_certainty: Option<CertaintyClass>,
  pub inferred: InferredPolicy,
}

pub(crate) struct CertaintyBounds {
  pub min_tier: i64,
  pub exclude_inferred: bool,
}

impl CertaintyBounds {
  fn new(min_certainty: Option<CertaintyClass>, inferred: InferredPolicy) -> Self {
    Self {
      min_tier: min_certainty.map_or(0, certainty_tier),
      exclude_inferred: inferred == InferredPolicy::Exclude,
    }
  }
}

impl EdgeFilter {
  pub(crate) fn bounds(&self) -> CertaintyBounds {
    CertaintyBounds::new(self.min_certainty, self.inferred)
  }

  pub(crate) fn admits_kind(&self, kind: &str) -> bool {
    self.kinds.is_empty() || self.kinds.iter().any(|candidate| candidate.as_str() == kind)
  }
}

impl EntityFilter {
  pub(crate) fn bounds(&self) -> CertaintyBounds {
    CertaintyBounds::new(self.min_certainty, self.inferred)
  }

  pub(crate) fn admits_kind(&self, kind: &str) -> bool {
    self.kinds.is_empty() || self.kinds.iter().any(|candidate| candidate.as_str() == kind)
  }
}
