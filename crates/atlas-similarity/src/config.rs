use crate::error::SimilarityError;
use crate::minhash::LANES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimilarityConfig {
  pub bands: usize,
  pub rows: usize,
  pub min_tokens: usize,
}

impl Default for SimilarityConfig {
  fn default() -> Self {
    Self {
      bands: 32,
      rows: 4,
      min_tokens: 40,
    }
  }
}

impl SimilarityConfig {
  pub(crate) fn validate(&self) -> Result<(), SimilarityError> {
    if self.bands == 0 || self.rows == 0 {
      return Err(SimilarityError::InvalidConfig {
        reason: "bands and rows must be positive",
      });
    }
    match self.bands.checked_mul(self.rows) {
      Some(lanes) if lanes <= LANES => Ok(()),
      _ => Err(SimilarityError::InvalidConfig {
        reason: "bands * rows must not exceed the 128 signature lanes",
      }),
    }
  }
}
