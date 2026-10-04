use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum QualityError {
  #[error("i/o error at {path}: {source}")]
  Io {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
  #[error("cannot parse {path}: {reason}")]
  Parse { path: PathBuf, reason: String },
  #[error("invalid ground truth in {file}: {reason}")]
  InvalidTruth { file: String, reason: String },
  #[error("engine error: {0}")]
  Engine(#[from] atlas_engine::EngineError),
  #[error("store error: {0}")]
  Store(#[from] atlas_store::StoreError),
  #[error("query error: {0}")]
  Query(#[from] atlas_query::QueryError),
  #[error("project error: {0}")]
  Project(#[from] atlas_project::ProjectError),
  #[error("invalid argument: {0}")]
  InvalidArgument(String),
  #[error("scenario `{scenario}` cannot run: {reason}")]
  Scenario { scenario: String, reason: String },
}

impl QualityError {
  pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    Self::Io { path: path.into(), source }
  }

  pub fn parse(path: impl Into<PathBuf>, reason: impl std::fmt::Display) -> Self {
    Self::Parse {
      path: path.into(),
      reason: reason.to_string(),
    }
  }

  pub fn truth(file: impl Into<String>, reason: impl Into<String>) -> Self {
    Self::InvalidTruth {
      file: file.into(),
      reason: reason.into(),
    }
  }

  pub fn scenario(scenario: impl Into<String>, reason: impl Into<String>) -> Self {
    Self::Scenario {
      scenario: scenario.into(),
      reason: reason.into(),
    }
  }
}

pub type QualityResult<T> = Result<T, QualityError>;
