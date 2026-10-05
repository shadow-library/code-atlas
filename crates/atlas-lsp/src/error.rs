use std::fmt;
use std::time::Duration;

use thiserror::Error;

use crate::position::PositionError;
use crate::process::ProcessExit;
use crate::protocol::ResponseError;
use crate::uri::UriError;

#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum LspError {
  #[error("failed to start language server {program}: {reason}")]
  Spawn { program: String, reason: String },
  #[error("language server protocol violation: {message}{}", tail_suffix(stderr_tail))]
  Protocol { message: String, stderr_tail: String },
  #[error("language server exited ({cause}){}", tail_suffix(stderr_tail))]
  ServerExited { cause: Disconnect, stderr_tail: String },
  #[error("{method} timed out after {}s", after.as_secs_f64())]
  Timeout { method: String, after: Duration },
  #[error("{method} failed: {} (code {})", error.message, error.code)]
  Response { method: String, error: ResponseError },
  #[error("cannot decode the {method} result: {message}")]
  Decode { method: String, message: String },
  #[error("cannot encode the {method} params: {message}")]
  Encode { method: String, message: String },
  #[error("language server chose position encoding {encoding:?}, which code-atlas does not support")]
  UnsupportedEncoding { encoding: String },
  #[error(
    "{server} was not ready after {}s ({status}); check it with `code-atlas lsp check`, and on a large workspace raise lsp.ready_timeout_secs in the project config",
    waited.as_secs_f64()
  )]
  NotReady { server: String, waited: Duration, status: String },
  #[error("{server} reports an unhealthy workspace: {message}; fix the workspace (rust-analyzer needs `cargo metadata` to succeed) and check it with `code-atlas lsp check`")]
  Unhealthy { server: String, message: String },
  #[error("{server} failed {failures} times within {}s; not restarting it for another {}s; last failure: {last_failure}", window.as_secs(), retry_in.as_secs())]
  CrashLoop {
    server: String,
    failures: usize,
    window: Duration,
    retry_in: Duration,
    last_failure: String,
  },
  #[error("invalid language server unit: {reason}")]
  InvalidUnit { reason: String },
  #[error("the language server pool is shut down")]
  PoolShutDown,
  #[error("all {max_servers} language server slots stayed in use for {}s; retry when a running operation finishes", waited.as_secs_f64())]
  PoolBusy { max_servers: usize, waited: Duration },
  #[error("{server} could not be brought in sync with the files on disk for an exact operation: {reason}; retry once the server settles")]
  SyncIncomplete { server: String, reason: String },
  #[error(transparent)]
  Uri(#[from] UriError),
  #[error(transparent)]
  Position(#[from] PositionError),
}

pub type LspResult<T> = Result<T, LspError>;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Disconnect {
  EndOfStream,
  Truncated,
  ReadFailed(String),
  WriteFailed(String),
  ProcessExited(ProcessExit),
  ShutDown,
}

impl fmt::Display for Disconnect {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::EndOfStream => formatter.write_str("its output ended"),
      Self::Truncated => formatter.write_str("its output ended in the middle of a message"),
      Self::ReadFailed(reason) => write!(formatter, "reading its output failed: {reason}"),
      Self::WriteFailed(reason) => write!(formatter, "writing to it failed: {reason}"),
      Self::ProcessExited(exit) => write!(formatter, "the process {exit}"),
      Self::ShutDown => formatter.write_str("it was shut down"),
    }
  }
}

fn tail_suffix(stderr_tail: &str) -> String {
  let tail = stderr_tail.trim();
  match tail.is_empty() {
    true => String::new(),
    false => format!("; stderr tail:\n{tail}"),
  }
}
