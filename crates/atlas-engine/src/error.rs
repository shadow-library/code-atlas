use std::path::PathBuf;
use std::time::Duration;

use atlas_core::{CoreError, RelPath, RepoName};
use atlas_project::ProjectError;
use atlas_resolve::ResolveError;
use atlas_semantic::SemanticError;
use atlas_similarity::SimilarityError;
use atlas_store::StoreError;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EngineError {
  #[error(transparent)]
  Store(#[from] StoreError),
  #[error(transparent)]
  Project(#[from] ProjectError),
  #[error(transparent)]
  Resolve(#[from] ResolveError),
  #[error(transparent)]
  Core(#[from] CoreError),
  #[error(transparent)]
  Similarity(#[from] SimilarityError),
  #[error(transparent)]
  Semantic(#[from] SemanticError),
  #[error("i/o on {path}: {source}")]
  Io {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
  #[error("stored {what} is unreadable: {detail}")]
  Codec { what: &'static str, detail: String },
  #[error("timed out after {waited:?} waiting for the index lock {path}")]
  IndexLockTimeout { path: PathBuf, waited: Duration },
  #[error("repo {0} is not part of this project")]
  UnknownRepo(RepoName),
  #[error("{repo}:{path} is not indexed")]
  NotIndexed { repo: RepoName, path: RelPath },
  #[error("{repo}:{path} changed on disk since it was indexed; run the freshness barrier first")]
  SourceChanged { repo: RepoName, path: RelPath },
  #[error("file watcher: {0}")]
  Watch(String),
  #[error("project pass name {0:?} is reserved or registered twice")]
  InvalidPass(String),
  #[error("index update panicked: {0}")]
  Panicked(String),
  #[error("project configuration changed; reopen the engine")]
  ProjectConfigChanged,
  #[error("engine serves project {expected}, not {found}")]
  ProjectMismatch { expected: String, found: String },
  #[error("{} has {available} bytes free, but copying the project database needs {needed}", .path.display())]
  InsufficientSpace { path: PathBuf, needed: u64, available: u64 },
}

impl EngineError {
  pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    Self::Io { path: path.into(), source }
  }

  pub(crate) fn codec(what: &'static str, detail: impl ToString) -> Self {
    Self::Codec { what, detail: detail.to_string() }
  }
}

pub type EngineResult<T> = Result<T, EngineError>;
