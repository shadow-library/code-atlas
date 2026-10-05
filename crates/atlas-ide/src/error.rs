use std::path::PathBuf;

use atlas_edit::EditError;
use atlas_engine::EngineError;
use atlas_lsp::LspError;
use atlas_query::QueryError;
use atlas_store::StoreError;
use thiserror::Error;

pub const GRAPH_ALTERNATIVE: &str = "retry with source='graph' for tagged graph results";

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IdeError {
  #[error("path {path} is outside every registered repo ({})", repos.join(", "))]
  OutsideProject { path: String, repos: Vec<String> },
  #[error("{server} is unavailable: {reason}{}", alternative_suffix(alternative))]
  Unavailable {
    server: String,
    reason: String,
    alternative: Option<&'static str>,
  },
  #[error(
    "language servers are disabled for project {project} (lsp.enabled = false); enable them with `code-atlas project config {project} --lsp on`{}",
    alternative_suffix(alternative)
  )]
  Disabled { project: String, alternative: Option<&'static str> },
  #[error("{spec:?} is an endpoint; endpoints have no LSP symbol; use atlas_api")]
  EndpointTarget { spec: String },
  #[error("{spec:?} is a {kind}, not a code symbol; give a symbol name, a key or [repo:]path:line:col")]
  NotASymbol { spec: String, kind: String },
  #[error("cannot locate the name of {target} in its file: {reason}")]
  NoNameToken { target: String, reason: String },
  #[error("{file} changed while answering and still differs from what was indexed or sent to the language server; retry")]
  SourceChanged { file: String },
  #[error("unknown repo {repo:?}; registered repos: {}", repos.join(", "))]
  UnknownRepo { repo: String, repos: Vec<String> },
  #[error("no file {path} in any registered repo ({})", repos.join(", "))]
  FileNotFound { path: String, repos: Vec<String> },
  #[error("{path} exists in several repos ({}); prefix it with the repo, e.g. {}:{path}", repos.join(", "), repos.first().map_or("", String::as_str))]
  AmbiguousFile { path: String, repos: Vec<String> },
  #[error("{path} is not a Rust, TypeScript or JavaScript file; no language server covers it")]
  UnsupportedFile { path: String },
  #[error("{path} is not covered by {server}: {reason}")]
  NotCovered { path: String, server: String, reason: String },
  #[error("{file}:{line}:{column} is not a position in the file: {reason}")]
  InvalidPosition { file: String, line: u32, column: u32, reason: String },
  #[error("{0}")]
  InvalidRequest(String),
  #[error("invalid regex {pattern:?}: {reason}")]
  Regex { pattern: String, reason: String },
  #[error("the regex {pattern:?} has {groups} capture groups; it needs exactly one, around the name to look up")]
  RegexGroups { pattern: String, groups: usize },
  #[error("the regex {pattern:?} matches {count} times in {file}; it must match exactly once")]
  RegexMatches { pattern: String, file: String, count: usize },
  #[error("{server} does not support {capability}")]
  Unsupported { server: String, capability: String },
  #[error("{file} could not be opened in {server}: {reason}")]
  NotOpened { file: String, server: String, reason: String },
  #[error("cannot read {}: {source}", path.display())]
  Io { path: PathBuf, source: std::io::Error },
  #[error(transparent)]
  Lsp(#[from] LspError),
  #[error("{source}{}", alternative_suffix(alternative))]
  ServerFailed { source: LspError, alternative: Option<&'static str> },
  #[error(transparent)]
  Query(#[from] QueryError),
  #[error(transparent)]
  Engine(#[from] EngineError),
  #[error(transparent)]
  Store(#[from] StoreError),
  #[error(transparent)]
  Edit(#[from] EditError),
  #[error("interrupted before the write started; nothing was written")]
  Interrupted,
  #[error("{server} is not running; code-atlas does not start it for this")]
  NotRunning { server: String },
  #[error("plan {id} expired after {age_secs}s (plans are kept for {ttl_secs}s); run the dry run again")]
  PlanExpired { id: String, age_secs: u64, ttl_secs: u64 },
  #[error("unknown plan {id}: it was never made by this process, was already applied, or was evicted (the last {capacity} plans are kept); run the dry run again")]
  UnknownPlan { id: String, capacity: usize },
}

pub type IdeResult<T> = Result<T, IdeError>;

fn alternative_suffix(alternative: &Option<&'static str>) -> String {
  alternative.map_or_else(String::new, |alternative| format!("; {alternative}"))
}

impl IdeError {
  pub(crate) fn with_graph_alternative(self) -> Self {
    match self {
      Self::Unavailable { server, reason, .. } => Self::Unavailable {
        server,
        reason,
        alternative: Some(GRAPH_ALTERNATIVE),
      },
      Self::Disabled { project, .. } => Self::Disabled {
        project,
        alternative: Some(GRAPH_ALTERNATIVE),
      },
      Self::Lsp(
        source @ (LspError::Spawn { .. }
        | LspError::CrashLoop { .. }
        | LspError::NotReady { .. }
        | LspError::Unhealthy { .. }
        | LspError::PoolBusy { .. }
        | LspError::PoolShutDown
        | LspError::ServerExited { .. }
        | LspError::Unavailable(_)),
      ) => Self::ServerFailed {
        source,
        alternative: Some(GRAPH_ALTERNATIVE),
      },
      other => other,
    }
  }
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::*;

  #[test]
  fn should_point_server_failures_to_the_graph_and_leave_other_errors_alone() {
    let crash_loop = IdeError::Lsp(LspError::CrashLoop {
      server: "rust-analyzer".to_owned(),
      failures: 3,
      window: Duration::from_secs(600),
      retry_in: Duration::from_secs(60),
      last_failure: "exited".to_owned(),
    })
    .with_graph_alternative();
    let other = IdeError::InvalidRequest("bad".to_owned()).with_graph_alternative();

    assert_eq!(
      crash_loop.to_string(),
      "rust-analyzer failed 3 times within 600s; not restarting it for another 60s; last failure: exited; retry with source='graph' for tagged graph results"
    );
    assert_eq!(other.to_string(), "bad");
  }
}
