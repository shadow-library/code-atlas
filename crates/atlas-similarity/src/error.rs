use thiserror::Error;

#[derive(Debug, Error)]
pub enum SimilarityError {
  #[error("invalid similarity config: {reason}")]
  InvalidConfig { reason: &'static str },
  #[error("similarity index capacity exceeded")]
  CapacityExceeded,
  #[error("not a similarity index: bad magic header")]
  BadMagic,
  #[error("unsupported similarity index format version {found}, expected {expected}")]
  UnsupportedVersion { found: u32, expected: u32 },
  #[error("similarity index data is truncated")]
  Truncated,
  #[error("similarity index checksum mismatch")]
  ChecksumMismatch,
  #[error("corrupt similarity index: {reason}")]
  Corrupt { reason: String },
}
