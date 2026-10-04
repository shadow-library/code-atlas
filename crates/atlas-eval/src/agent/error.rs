use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
  #[error("i/o error at {path}: {source}")]
  Io {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
  #[error("invalid task spec {path}: {reason}")]
  InvalidSpec { path: PathBuf, reason: String },
  #[error("cannot parse {path}: {reason}")]
  Parse { path: PathBuf, reason: String },
  #[error("`{command}` failed: {detail}")]
  Command { command: String, detail: String },
  #[error("no task specs found under {0}")]
  NoTasks(PathBuf),
  #[error("unknown task id `{0}`")]
  UnknownTask(String),
  #[error("invalid argument: {0}")]
  InvalidArgument(String),
}

impl AgentError {
  pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    Self::Io { path: path.into(), source }
  }
}

pub type AgentResult<T> = Result<T, AgentError>;
