use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BenchError {
  #[error("{operation} {path}: {source}")]
  Io { operation: &'static str, path: PathBuf, source: std::io::Error },
  #[error("cannot encode or decode JSON for {context}: {source}")]
  Json { context: String, source: serde_json::Error },
  #[error(transparent)]
  Engine(#[from] atlas_engine::EngineError),
  #[error(transparent)]
  Query(#[from] atlas_query::QueryError),
  #[error(transparent)]
  Project(#[from] atlas_project::ProjectError),
  #[error(transparent)]
  Store(#[from] atlas_store::StoreError),
  #[error(transparent)]
  Core(#[from] atlas_core::CoreError),
  #[error("worker {mode} failed: {detail}")]
  Worker { mode: String, detail: String },
  #[error("cannot read resource usage from the time wrapper: {0}")]
  Usage(String),
  #[error("{count} generated file(s) did not parse cleanly: {sample}")]
  ParseFailures { count: u64, sample: String },
  #[error("invalid argument: {0}")]
  Invalid(String),
  #[error("fetching {corpus} failed: {detail}")]
  Fetch { corpus: String, detail: String },
}

impl BenchError {
  pub fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
    Self::Io {
      operation,
      path: path.to_path_buf(),
      source,
    }
  }

  pub fn json(context: impl Into<String>, source: serde_json::Error) -> Self {
    Self::Json { context: context.into(), source }
  }
}

pub type BenchResult<T> = Result<T, BenchError>;
