use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use atlas_ide::IdeService;
use tokio::signal::unix::{SignalKind, signal};

use crate::error::{CliError, CliResult};
use crate::output::note;

pub const INTERRUPTED_EXIT: u8 = 130;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn interrupted() -> bool {
  INTERRUPTED.load(Ordering::SeqCst)
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
          note("interrupted while files are being written; the write finishes first so no file is left half written, then the command exits 130");
        }
      });
    })
    .map_err(CliError::Signal)?;
  Ok(())
}
