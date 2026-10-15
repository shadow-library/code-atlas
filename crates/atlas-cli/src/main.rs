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

use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::error::Exit;

const HOOK_COMMAND: &str = "hook";

fn main() -> ExitCode {
  let cli = match Cli::try_parse() {
    Ok(cli) => cli,
    Err(_) if std::env::args_os().nth(1).is_some_and(|argument| argument == HOOK_COMMAND) => return ExitCode::SUCCESS,
    Err(error) => error.exit(),
  };
  if matches!(cli.command, Command::Hook(_)) {
    panic::set_hook(Box::new(|_| {}));
    let _ = panic::catch_unwind(AssertUnwindSafe(|| commands::run(cli.project, cli.command)));
    return ExitCode::SUCCESS;
  }
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
