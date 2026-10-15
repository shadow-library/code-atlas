use std::io;
use std::path::PathBuf;

use atlas_core::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProjectError {
  #[error("cannot determine a home directory; set CODE_ATLAS_HOME or HOME")]
  HomeNotFound,
  #[error("{operation} {}: {source}", .path.display())]
  Io {
    operation: &'static str,
    path: PathBuf,
    #[source]
    source: io::Error,
  },
  #[error("invalid registry file {}: {message}", .path.display())]
  RegistryParse { path: PathBuf, message: String },
  #[error("invalid config file {}: {message}", .path.display())]
  ConfigParse { path: PathBuf, message: String },
  #[error("invalid settings for project {project:?}: {message}")]
  InvalidSettings { project: String, message: String },
  #[error("cannot serialize the registry: {0}")]
  RegistrySerialize(String),
  #[error("project {0:?} already exists")]
  ProjectExists(String),
  #[error("project {0:?} not found")]
  ProjectNotFound(String),
  #[error("repo {repo:?} already exists in project {project:?}")]
  DuplicateRepo { project: String, repo: String },
  #[error("repo {repo:?} not found in project {project:?}")]
  RepoNotFound { project: String, repo: String },
  #[error("repo path {} does not exist", .0.display())]
  RepoPathMissing(PathBuf),
  #[error("repo path {} is not a directory", .0.display())]
  RepoPathNotDirectory(PathBuf),
  #[error("repo path {} is not valid UTF-8", .0.display())]
  RepoPathNotUtf8(PathBuf),
  #[error("path {} is already registered in project {project:?} as repo {repo:?}", .path.display())]
  DuplicatePath { project: String, repo: String, path: PathBuf },
  #[error("cannot derive a repo name from {}; pass an explicit name", .0.display())]
  NameUnderivable(PathBuf),
  #[error("invalid glob {pattern:?}: {message}")]
  InvalidGlob { pattern: String, message: String },
  #[error(transparent)]
  Name(#[from] CoreError),
  #[error("{} is inside several projects ({}); pass --project to choose", .cwd.display(), .candidates.join(", "))]
  Ambiguous { cwd: PathBuf, candidates: Vec<String> },
  #[error("{} is not inside any registered project; register it with `catl project create` or pass --project", .cwd.display())]
  NotInProject { cwd: PathBuf },
  #[error("invalid project marker {}: {message}", .path.display())]
  InvalidMarker { path: PathBuf, message: String },
  #[error("project marker {} names unknown project {project:?}", .path.display())]
  MarkerProjectNotFound { path: PathBuf, project: String },
  #[error(
    "{} is in the git worktree at {} of project {project:?}, and config.toml sets [worktrees] mode = \"off\", so Code Atlas does not serve it",
    .cwd.display(),
    .worktree.display()
  )]
  WorktreeIgnored { cwd: PathBuf, worktree: PathBuf, project: String },
}

impl ProjectError {
  pub(crate) fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
    Self::Io {
      operation,
      path: path.into(),
      source,
    }
  }
}
