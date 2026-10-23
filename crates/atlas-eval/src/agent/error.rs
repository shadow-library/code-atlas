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
  #[error("invalid [linked] table in task spec {path}: {source}")]
  InvalidLinked {
    path: PathBuf,
    #[source]
    source: LinkedSpecError,
  },
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
  #[error("incumbent arm (Serena + graphify): {reason}")]
  IncumbentUnavailable { reason: String },
  #[error(
    "scored incumbent run(s) {runs} of this batch were produced with other Serena, graphify or instructions ({recorded}) than this invocation ({expected}); rerun them under one setup or use a new batch"
  )]
  IncumbentMismatch { runs: String, recorded: String, expected: String },
  #[error("batch mixes incumbent setups ({0}); runs under different Serena, graphify or instructions are not comparable, rerun them under one")]
  MixedIncumbent(String),
  #[error("{0} is in use by another atlas-eval process (its batch lock is held); wait for it to finish")]
  BatchInProgress(PathBuf),
  #[error("{flag} {given} differs from the {recorded} the batch was judged with; a rejudge uses the batch's judge settings")]
  JudgeSettingConflict { flag: String, given: String, recorded: String },
  #[error("invalid argument: {0}")]
  InvalidArgument(String),
  #[error("`{path}` reaches outside the tree through the symbolic link `{link}`; a task may only edit files of its own tree")]
  TreePathThroughSymlink { path: String, link: String },
  #[error("{count} task(s) declare a prompt_leak class that differs from the audit; see {}", report.display())]
  LeakClassMismatch { count: usize, report: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LinkedSpecError {
  #[error("`paths` must name at least one directory")]
  NoPaths,
  #[error("path `{0}` names the tree root; a linked path must be a directory below it")]
  TreeRoot(String),
  #[error("path `{0}` must be relative to the tree root, without `..`, `.` or empty segments")]
  InvalidPath(String),
  #[error("path `{0}` is listed twice")]
  Duplicate(String),
  #[error("path `{inner}` lies inside the linked path `{outer}`")]
  Nested { outer: String, inner: String },
  #[error("path `{path}` is or lies inside the strip path `{strip}`, which is removed before the linked paths are hidden")]
  InsideStrip { path: String, strip: String },
  #[error("`atlas_repos` ({}) cannot be combined with [linked] ({}): with linked paths the session tree is one Code Atlas repo", atlas_repos.join(", "), paths.join(", "))]
  WithAtlasRepos { atlas_repos: Vec<String>, paths: Vec<String> },
  #[error("must_change file `{file}` lies inside the linked path `{path}`, which the session cannot change")]
  MustChangeLinked { file: String, path: String },
  #[error("repo `{0}` is not a valid Code Atlas repository name")]
  InvalidRepo(String),
  #[error("repo `{0}` is the name of the session tree's own repository")]
  ReservedRepo(String),
  #[error("patch `{0}` must be a relative path inside the task directory, outside hidden/")]
  PatchLocation(String),
  #[error("patch `{0}` does not exist")]
  PatchMissing(String),
  #[error("patch `{0}` touches no file")]
  PatchEmpty(String),
  #[error("patch `{patch}` touches `{file}`, which lies under no linked path")]
  PatchOutsidePaths { patch: String, file: String },
}

impl AgentError {
  pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    Self::Io { path: path.into(), source }
  }
}

pub type AgentResult<T> = Result<T, AgentError>;
