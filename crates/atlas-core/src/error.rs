use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Error)]
#[non_exhaustive]
pub enum CoreError {
  #[error("invalid relative path {path:?}: {reason}")]
  InvalidRelPath { path: String, reason: &'static str },
  #[error("invalid {what} {value:?}: {reason}")]
  InvalidName { what: &'static str, value: String, reason: &'static str },
  #[error("invalid key {key:?}: {reason}")]
  InvalidKey { key: String, reason: String },
  #[error("confidence must be a number in [0, 1], got {0}")]
  InvalidConfidence(f32),
  #[error("invalid content hash {0:?}")]
  InvalidContentHash(String),
  #[error("invalid entity id {0:?}")]
  InvalidEntityId(String),
  #[error("unknown {what} {value:?}")]
  UnknownVariant { what: &'static str, value: String },
  #[error("no supported language for path {0:?}")]
  UnsupportedPath(String),
  #[error("file facts violate an invariant: {0}")]
  InvalidFacts(String),
}

impl CoreError {
  pub(crate) fn invalid_key(key: &str, reason: impl Into<String>) -> Self {
    Self::InvalidKey {
      key: key.to_owned(),
      reason: reason.into(),
    }
  }
}
