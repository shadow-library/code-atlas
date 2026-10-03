use atlas_core::CoreError;
use atlas_store::StoreError;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SemanticError {
  #[error(transparent)]
  Store(#[from] StoreError),
  #[error(transparent)]
  Core(#[from] CoreError),
  #[error("invalid semantic task: {0}")]
  InvalidTask(String),
  #[error("invalid LLM provider configuration: {0}")]
  Config(String),
}

pub type SemanticResult<T> = Result<T, SemanticError>;
