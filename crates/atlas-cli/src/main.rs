mod cli;
mod commands;
mod confirm;
mod context;
mod diff;
mod error;
mod json_edit;
mod logging;
mod output;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::Cli;
use crate::error::Exit;

fn main() -> ExitCode {
  let cli = Cli::parse();
  let result = logging::init(cli.log_level.as_deref(), cli.log_format).and_then(|()| commands::run(cli.project, cli.command));
  let exit = match result {
    Ok(outcome) => outcome.exit(),
    Err(error) => {
      error.report();
      error.exit()
    }
  };
  ExitCode::from(Exit::code(exit))
}
