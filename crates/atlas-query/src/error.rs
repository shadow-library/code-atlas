use atlas_engine::EngineError;
use atlas_store::StoreError;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum QueryError {
  #[error(transparent)]
  Engine(#[from] EngineError),
  #[error(transparent)]
  Store(#[from] StoreError),
  #[error("the draft could not be analysed: {0}")]
  Analysis(String),
  #[error("invalid query: {0}")]
  Invalid(String),
  #[error("cannot encode the response as JSON: {0}")]
  Encode(#[from] serde_json::Error),
}

impl QueryError {
  pub(crate) fn invalid(message: impl Into<String>) -> Self {
    Self::Invalid(message.into())
  }
}

pub type QueryResult<T> = Result<T, QueryError>;
