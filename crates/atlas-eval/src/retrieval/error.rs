use std::path::PathBuf;

use thiserror::Error;

use crate::agent::AgentError;

#[derive(Debug, Error)]
pub enum RetrievalError {
  #[error("i/o error at {path}: {source}")]
  Io {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
  #[error("cannot serialise the retrieval report: {0}")]
  Json(#[from] serde_json::Error),
  #[error(transparent)]
  Agent(#[from] AgentError),
  #[error("engine error: {0}")]
  Engine(#[from] atlas_engine::EngineError),
  #[error("store error: {0}")]
  Store(#[from] atlas_store::StoreError),
  #[error("query error: {0}")]
  Query(#[from] atlas_query::QueryError),
  #[error("project error: {0}")]
  Project(#[from] atlas_project::ProjectError),
  #[error("invalid {}: {reason}", path.display())]
  InvalidQueries { path: PathBuf, reason: String },
  #[error("{failed} of {total} tasks could not be scored; see {}", report.display())]
  TasksFailed { failed: usize, total: usize, report: PathBuf },
}

impl RetrievalError {
  pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    Self::Io { path: path.into(), source }
  }
}

pub type RetrievalResult<T> = Result<T, RetrievalError>;
