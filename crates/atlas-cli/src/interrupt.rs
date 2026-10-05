use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use atlas_ide::IdeService;
use tokio::signal::unix::{SignalKind, signal};

use crate::error::{CliError, CliResult, Exit};
use crate::output::note;

pub const INTERRUPTED_EXIT: u8 = 130;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn interrupted() -> bool {
  INTERRUPTED.load(Ordering::SeqCst)
}

pub fn exit_after_signal(exit: Exit) -> u8 {
  match exit {
    Exit::Refused | Exit::RolledBack | Exit::PartiallyApplied => exit.code(),
    Exit::Success | Exit::Failure | Exit::Usage | Exit::Degraded | Exit::NoMatch => INTERRUPTED_EXIT,
  }
}

pub fn watch(ide: Arc<IdeService>) -> CliResult<()> {
  let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(CliError::Signal)?;
  let (mut interrupt, mut terminate) = runtime
    .block_on(async { Ok::<_, std::io::Error>((signal(SignalKind::interrupt())?, signal(SignalKind::terminate())?)) })
    .map_err(CliError::Signal)?;
  std::thread::Builder::new()
    .name("interrupt".to_owned())
    .spawn(move || {
      runtime.block_on(async {
        loop {
          tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
          }
          INTERRUPTED.store(true, Ordering::SeqCst);
          if ide.close_writes() {
            ide.shutdown();
            std::process::exit(i32::from(INTERRUPTED_EXIT));
          }
          note("interrupted while files are being written; the write finishes first so no file is left half written, then the command exits 130, or 5, 6 or 7 if that edit is refused, rolled back or partially applied");
        }
      });
    })
    .map_err(CliError::Signal)?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_keep_the_edit_failure_codes_and_exit_130_otherwise_after_a_deferred_signal() {
    let exits = [
      Exit::Success,
      Exit::Failure,
      Exit::Usage,
      Exit::Degraded,
      Exit::NoMatch,
      Exit::Refused,
      Exit::RolledBack,
      Exit::PartiallyApplied,
    ];

    let codes: Vec<u8> = exits.into_iter().map(exit_after_signal).collect();

    assert_eq!(codes, [130, 130, 130, 130, 130, 5, 6, 7]);
  }
}
