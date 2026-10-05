mod cli;
mod commands;
mod confirm;
mod context;
mod diff;
mod error;
mod interrupt;
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
      if !interrupt::interrupted() {
        error.report();
      }
      error.exit()
    }
  };
  match interrupt::interrupted() {
    true => ExitCode::from(interrupt::exit_after_signal(exit)),
    false => ExitCode::from(Exit::code(exit)),
  }
}
