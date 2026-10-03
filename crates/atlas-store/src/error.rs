use std::path::PathBuf;
use std::time::Duration;

use atlas_core::{CoreError, RunId};
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreError {
  #[error("sqlite: {0}")]
  Sqlite(#[from] rusqlite::Error),
  #[error("json: {0}")]
  Json(#[from] serde_json::Error),
  #[error(transparent)]
  Core(#[from] CoreError),
  #[error("i/o on {path}: {source}")]
  Io {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
  #[error("database schema version {found} is newer than the supported version {supported}; refusing to open it")]
  SchemaTooNew { found: i64, supported: i64 },
  #[error("database schema version {0} is invalid")]
  InvalidSchemaVersion(i64),
  #[error("timed out after {waited:?} waiting for the writer lock {path}")]
  LockTimeout { path: PathBuf, waited: Duration },
  #[error("this thread already holds a store transaction; use the ReadTx or WriteTx it was given")]
  Reentrant,
  #[error("index run {active} is already active in this write transaction")]
  RunAlreadyActive { active: RunId },
  #[error("invalid facts for {owner}: {reason}")]
  InvalidFacts { owner: String, reason: String },
  #[error("id collision on {what} {id}: {existing} vs {incoming}")]
  IdCollision {
    what: &'static str,
    id: String,
    existing: String,
    incoming: String,
  },
  #[error("stored {what} is corrupt: {detail}")]
  Corrupt { what: &'static str, detail: String },
  #[error("index run {0} does not exist")]
  RunNotFound(RunId),
  #[error("index run {id} is {status}, not running")]
  RunNotRunning { id: RunId, status: &'static str },
  #[error("inferred facts must carry inferred certainty, got {0}")]
  NotInferred(String),
  #[error("{what} value {value} does not fit in a SQLite integer")]
  OutOfRange { what: &'static str, value: u64 },
}

impl StoreError {
  pub(crate) fn corrupt(what: &'static str, detail: impl Into<String>) -> Self {
    Self::Corrupt { what, detail: detail.into() }
  }
}

pub type StoreResult<T> = Result<T, StoreError>;
