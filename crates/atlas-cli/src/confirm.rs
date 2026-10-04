use std::io::{self, BufRead, IsTerminal, Write};

use crate::error::{CliError, CliResult};

pub fn confirm(what: &str, assume_yes: bool) -> CliResult<()> {
  if assume_yes {
    return Ok(());
  }
  if !io::stdin().is_terminal() {
    return Err(CliError::ConfirmationRequired { what: what.to_owned() });
  }
  let mut stderr = io::stderr().lock();
  let _ = write!(stderr, "{what}? [y/N] ");
  let _ = stderr.flush();
  let mut answer = String::new();
  io::stdin().lock().read_line(&mut answer).map_err(|source| CliError::io("read", "<stdin>", source))?;
  match answer.trim().to_ascii_lowercase().as_str() {
    "y" | "yes" => Ok(()),
    _ => Err(CliError::Declined),
  }
}
