use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LspError {
  #[error("language server transport failed: {0}")]
  Transport(String),
}

pub type LspResult<T> = Result<T, LspError>;
