use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};

use crate::error::{StoreError, StoreLock, StoreResult};
use crate::schema;
use crate::store::{WriterLock, lock_path_for};

const RETRY_PAUSE: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupReport {
  pub pages: u64,
  pub bytes: u64,
}

// The index lock is what an engine holds for a whole run, so the copy never sees a run's earlier batches without its later ones.
pub fn backup_database(source: &Path, destination: &Path, lock_timeout: Duration) -> StoreResult<BackupReport> {
  if fs::symlink_metadata(destination).is_ok() {
    return Err(StoreError::Io {
      path: destination.to_path_buf(),
      source: io::Error::new(io::ErrorKind::AlreadyExists, "the backup destination already exists"),
    });
  }
  let mut index = WriterLock::open(StoreLock::Index, lock_path_for(source, ".index.lock"))?;
  index.hold(lock_timeout, || copy(source, destination, lock_timeout))?
}

fn copy(source: &Path, destination: &Path, timeout: Duration) -> StoreResult<BackupReport> {
  let from = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
  from.busy_timeout(timeout)?;
  schema::ensure_supported(schema::user_version(&from)?)?;
  let mut to = Connection::open(destination)?;
  let pages = step_to_completion(&from, &mut to, source, timeout)?;
  to.close().map_err(|(_, error)| StoreError::Sqlite(error))?;
  let file = File::open(destination).map_err(|source| io_error(destination, source))?;
  file.sync_all().map_err(|source| io_error(destination, source))?;
  let bytes = file.metadata().map_err(|source| io_error(destination, source))?.len();
  Ok(BackupReport { pages, bytes })
}

fn step_to_completion(from: &Connection, to: &mut Connection, source: &Path, timeout: Duration) -> StoreResult<u64> {
  let backup = Backup::new(from, to)?;
  let started = Instant::now();
  loop {
    if backup.step(-1)? == StepResult::Done {
      return Ok(u64::try_from(backup.progress().pagecount).unwrap_or(0));
    }
    let waited = started.elapsed();
    if waited >= timeout {
      return Err(StoreError::LockTimeout {
        lock: StoreLock::Writer,
        path: source.to_path_buf(),
        waited,
      });
    }
    thread::sleep(RETRY_PAUSE);
  }
}

fn io_error(path: &Path, source: io::Error) -> StoreError {
  StoreError::Io { path: path.to_path_buf(), source }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::store::Store;

  fn seeded_store(path: &Path) -> Store {
    let store = Store::open(path).unwrap();
    store.write(|tx| tx.put_blob("engine:meta", 2, b"{\"generation\":7}")).unwrap();
    store
  }

  fn blob(path: &Path) -> Option<Vec<u8>> {
    Store::open(path).unwrap().read(|tx| tx.blob("engine:meta")).unwrap().map(|blob| blob.bytes)
  }

  #[test]
  fn should_copy_a_live_database_including_its_uncheckpointed_writes() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("atlas.db");
    let destination = dir.path().join("copy.db");
    let _open = seeded_store(&source);

    let report = backup_database(&source, &destination, Duration::from_secs(5)).unwrap();

    assert_eq!(blob(&destination), Some(b"{\"generation\":7}".to_vec()));
    assert_eq!(report.bytes, fs::metadata(&destination).unwrap().len());
    assert!(report.pages > 0);
    assert!(!lock_path_for(&destination, "-wal").exists());
  }

  #[test]
  fn should_refuse_to_overwrite_an_existing_destination() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("atlas.db");
    let destination = dir.path().join("copy.db");
    drop(seeded_store(&source));
    fs::write(&destination, b"keep").unwrap();

    let error = backup_database(&source, &destination, Duration::from_secs(5)).unwrap_err();

    assert!(matches!(&error, StoreError::Io { source, .. } if source.kind() == io::ErrorKind::AlreadyExists), "{error}");
    assert_eq!(fs::read(&destination).unwrap(), b"keep");
  }

  #[test]
  fn should_wait_for_the_index_lock_and_time_out_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("atlas.db");
    let destination = dir.path().join("copy.db");
    drop(seeded_store(&source));
    let mut held = WriterLock::open(StoreLock::Index, lock_path_for(&source, ".index.lock")).unwrap();

    let outcome = held
      .hold(Duration::from_secs(5), || backup_database(&source, &destination, Duration::from_millis(100)))
      .unwrap();

    assert!(matches!(outcome, Err(StoreError::LockTimeout { lock: StoreLock::Index, .. })), "{outcome:?}");
    assert!(!destination.exists());
  }

  #[test]
  fn should_not_create_a_missing_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("missing.db");

    let outcome = backup_database(&source, &dir.path().join("copy.db"), Duration::from_secs(1));

    assert!(matches!(outcome, Err(StoreError::Sqlite(_))), "{outcome:?}");
    assert!(!source.exists());
  }
}
