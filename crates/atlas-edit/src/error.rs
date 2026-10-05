use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EditError {
  #[error("edit is stale: {path}")]
  Stale { path: String },
}

pub type EditResult<T> = Result<T, EditError>;
