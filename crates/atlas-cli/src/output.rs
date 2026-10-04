use std::io::{self, ErrorKind, Write};

use crate::error::{CliError, CliResult, Exit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
  Complete,
  Degraded,
  Unresolved,
  Failed,
}

impl Outcome {
  pub fn exit(self) -> Exit {
    match self {
      Self::Complete => Exit::Success,
      Self::Degraded => Exit::Degraded,
      Self::Unresolved => Exit::NoMatch,
      Self::Failed => Exit::Failure,
    }
  }

  pub fn degraded_if(condition: bool) -> Self {
    if condition { Self::Degraded } else { Self::Complete }
  }
}

pub fn emit(text: &str) -> CliResult<()> {
  let mut stdout = io::stdout().lock();
  match stdout.write_all(text.as_bytes()).and_then(|()| stdout.flush()) {
    Err(error) if error.kind() == ErrorKind::BrokenPipe => Ok(()),
    other => other.map_err(|source| CliError::io("write", "<stdout>", source)),
  }
}

pub fn emit_line(text: impl AsRef<str>) -> CliResult<()> {
  let mut line = text.as_ref().to_owned();
  line.push('\n');
  emit(&line)
}

pub fn emit_json(value: &impl serde::Serialize) -> CliResult<()> {
  let mut text = serde_json::to_string_pretty(value)?;
  text.push('\n');
  emit(&text)
}

pub fn note(text: impl AsRef<str>) {
  let _ = writeln!(io::stderr().lock(), "{}", text.as_ref());
}
