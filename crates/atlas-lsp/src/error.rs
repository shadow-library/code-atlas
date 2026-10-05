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
