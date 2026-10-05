use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{EditError, EditResult};

const MAX_BACKOFF: Duration = Duration::from_millis(50);

pub struct EditLock {
  path: PathBuf,
  lock: fd_lock::RwLock<File>,
}

impl EditLock {
  pub fn open(path: &Path) -> EditResult<Self> {
    let file = OpenOptions::new()
      .create(true)
      .truncate(false)
      .read(true)
      .write(true)
      .open(path)
      .map_err(|source| EditError::io("open edit lock", path, source))?;
    Ok(Self {
      path: path.to_path_buf(),
      lock: fd_lock::RwLock::new(file),
    })
  }

  pub fn hold<R>(&mut self, timeout: Duration, f: impl FnOnce() -> R) -> EditResult<R> {
    let started = Instant::now();
    let mut backoff = Duration::from_millis(1);
    loop {
      match self.lock.try_write() {
        Ok(_guard) => return Ok(f()),
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
        Err(source) => return Err(EditError::io("lock", &self.path, source)),
      }
      let waited = started.elapsed();
      if waited >= timeout {
        return Err(EditError::LockTimeout { path: self.path.clone(), waited });
      }
      tracing::debug!(path = %self.path.display(), waited_ms = waited.as_millis() as u64, "waiting for another edit to finish");
      thread::sleep(backoff.min(timeout - waited));
      backoff = (backoff * 2).min(MAX_BACKOFF);
    }
  }
}
