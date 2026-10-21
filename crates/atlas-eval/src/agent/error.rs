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
  #[error("batch mixes atlas preamble variants ({0}); runs under different preambles are not comparable, rerun them under one")]
  MixedPreamble(String),
  #[error("--atlas-config {0} is not a directory")]
  AtlasConfigMissing(PathBuf),
  #[error("--atlas-config {0} holds neither config.toml nor detectors/*.toml, so there is nothing to apply")]
  AtlasConfigEmpty(PathBuf),
  #[error("--atlas-config {} does not load: {reason}", dir.display())]
  AtlasConfigRejected { dir: PathBuf, reason: String },
  #[error(
    "scored run(s) {runs} of this batch were produced with another atlas configuration (digest {recorded}) than this invocation ({expected}); rerun them under one or use a new batch"
  )]
  AtlasConfigMismatch { runs: String, recorded: String, expected: String },
  #[error("--atlas-hooks: {reason}")]
  AtlasHooksUnavailable { reason: String },
  #[error(
    "scored atlas run(s) {runs} of this batch were produced with other Code Atlas hooks (digest {recorded}) than this invocation ({expected}); rerun them under one setting or use a new batch"
  )]
  AtlasHooksMismatch { runs: String, recorded: String, expected: String },
  #[error("--atlas-surface forced: {reason}")]
  AtlasSurfaceUnavailable { reason: String },
  #[error(
    "scored atlas run(s) {runs} of this batch were produced under atlas surface {recorded}, this invocation uses {expected}; rerun them under one surface or use a new batch"
  )]
  AtlasSurfaceMismatch { runs: String, recorded: String, expected: String },
  #[error("invalid argument: {0}")]
  InvalidArgument(String),
  #[error("`{path}` reaches outside the tree through the symbolic link `{link}`; a task may only edit files of its own tree")]
  TreePathThroughSymlink { path: String, link: String },
  #[error("{count} task(s) declare a prompt_leak class that differs from the audit; see {}", report.display())]
  LeakClassMismatch { count: usize, report: PathBuf },
}

impl AgentError {
  pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    Self::Io { path: path.into(), source }
  }
}

pub type AgentResult<T> = Result<T, AgentError>;
