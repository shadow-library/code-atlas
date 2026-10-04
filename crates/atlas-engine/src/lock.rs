use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{EngineError, EngineResult};

const MAX_BACKOFF: Duration = Duration::from_millis(50);

pub(crate) struct IndexLock {
  path: PathBuf,
  lock: fd_lock::RwLock<File>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Contention {
  Uncontended,
  Waited,
}

impl IndexLock {
  pub fn open(db_path: &Path) -> EngineResult<Self> {
    let mut name = db_path.as_os_str().to_owned();
    name.push(".index.lock");
    let path = PathBuf::from(name);
    let file = OpenOptions::new()
      .create(true)
      .truncate(false)
      .read(true)
      .write(true)
      .open(&path)
      .map_err(|source| EngineError::io(&path, source))?;
    Ok(Self {
      path,
      lock: fd_lock::RwLock::new(file),
    })
  }

  pub fn try_hold<R>(&mut self, f: impl FnOnce() -> R) -> EngineResult<Option<R>> {
    match self.lock.try_write() {
      Ok(_guard) => Ok(Some(f())),
      Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => Ok(None),
      Err(source) => Err(EngineError::io(&self.path, source)),
    }
  }

  pub fn hold<R>(&mut self, timeout: Duration, f: impl FnOnce(Contention) -> R) -> EngineResult<R> {
    let started = Instant::now();
    let mut backoff = Duration::from_millis(1);
    let mut contention = Contention::Uncontended;
    loop {
      match self.lock.try_write() {
        Ok(_guard) => return Ok(f(contention)),
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
        Err(source) => return Err(EngineError::io(&self.path, source)),
      }
      contention = Contention::Waited;
      let waited = started.elapsed();
      if waited >= timeout {
        return Err(EngineError::IndexLockTimeout { path: self.path.clone(), waited });
      }
      tracing::debug!(path = %self.path.display(), waited_ms = waited.as_millis() as u64, "waiting for another process to finish indexing");
      thread::sleep(backoff.min(timeout - waited));
      backoff = (backoff * 2).min(MAX_BACKOFF);
    }
  }
}
