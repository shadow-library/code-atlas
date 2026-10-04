use std::io::{self, IsTerminal};

use tracing_subscriber::EnvFilter;

use crate::cli::LogFormat;
use crate::error::{CliError, CliResult};

const DEFAULT_FILTER: &str = "warn";

pub fn init(level: Option<&str>, format: LogFormat) -> CliResult<()> {
  let filter = match level {
    Some(raw) => EnvFilter::try_new(raw).map_err(|error| CliError::InvalidLogFilter {
      filter: raw.to_owned(),
      message: error.to_string(),
    })?,
    None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER)),
  };
  let builder = tracing_subscriber::fmt().with_env_filter(filter).with_writer(io::stderr);
  let installed = match format {
    LogFormat::Json => builder.json().try_init(),
    LogFormat::Text => builder.with_ansi(io::stderr().is_terminal()).try_init(),
  };
  if let Err(error) = installed {
    tracing::debug!(error = %error, "a tracing subscriber was already installed");
  }
  Ok(())
}
