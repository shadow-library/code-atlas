use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use rusqlite::{Connection, TransactionBehavior};

use crate::error::{StoreError, StoreLock, StoreResult};
use crate::schema;
use crate::tx::{ReadTx, WriteTx};

const STATEMENT_CACHE_CAPACITY: usize = 128;
// SQLite reads a negative cache_size as KiB rather than pages.
const WRITER_CACHE_KIB: i64 = -65_536;
const READER_CACHE_MAX_KIB: i64 = 16_384;
const READERS_CACHE_BUDGET_KIB: i64 = 524_288;
const MAX_LOCK_BACKOFF: Duration = Duration::from_millis(50);
const DEFAULT_AUTOCHECKPOINT_PAGES: i64 = 1_000;
// Bulk runs commit many batches; checkpointing after each one re-copies and fsyncs the same hot pages, so a run defers it.
const BULK_AUTOCHECKPOINT_PAGES: i64 = 65_536;
const JOURNAL_SIZE_LIMIT_BYTES: i64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct StoreOptions {
  pub readers: usize,
  pub busy_timeout: Duration,
  pub lock_timeout: Duration,
}

impl Default for StoreOptions {
  fn default() -> Self {
    Self {
      readers: 4,
      busy_timeout: Duration::from_secs(5),
      lock_timeout: Duration::from_secs(60),
    }
  }
}

pub struct Store {
  location: Location,
  writer: Mutex<Writer>,
  writer_thread: Mutex<Option<ThreadId>>,
  readers: Readers,
  lock_timeout: Duration,
  busy_timeout: Duration,
  bulk_depth: AtomicUsize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checkpoint {
  pub wal_frames: i64,
  pub checkpointed_frames: i64,
  pub busy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryIndexes {
  Built,
  Deferred,
}

pub struct BulkWrites<'s> {
  store: Option<&'s Store>,
  indexes: SecondaryIndexes,
}

impl BulkWrites<'_> {
  pub fn defer_indexes_while_empty(&mut self) -> StoreResult<SecondaryIndexes> {
    let Some(store) = self.store else { return Ok(self.indexes) };
    if self.indexes == SecondaryIndexes::Deferred {
      return Ok(self.indexes);
    }
    let deferred = store.write(|tx| -> StoreResult<bool> {
      if schema::holds_facts(tx.conn())? {
        schema::create_missing_indexes(tx.conn())?;
        return Ok(false);
      }
      schema::drop_deferrable_indexes(tx.conn())?;
      Ok(true)
    })?;
    if deferred {
      self.indexes = SecondaryIndexes::Deferred;
      tracing::info!("deferred the secondary fact indexes while bulk-loading an empty store");
    }
    Ok(self.indexes)
  }

  pub fn restore_indexes(&mut self) -> StoreResult<()> {
    let Some(store) = self.store else { return Ok(()) };
    if self.indexes == SecondaryIndexes::Built {
      return Ok(());
    }
    store.write(|tx| schema::create_missing_indexes(tx.conn()))?;
    self.indexes = SecondaryIndexes::Built;
    Ok(())
  }

  pub fn finish(mut self) -> StoreResult<Option<Checkpoint>> {
    self.restore_indexes()?;
    match self.store.take() {
      Some(store) => store.end_bulk(),
      None => Ok(None),
    }
  }
}

impl Drop for BulkWrites<'_> {
  fn drop(&mut self) {
    if let Err(error) = self.restore_indexes() {
      tracing::warn!(error = %error, "could not rebuild the deferred indexes; the next open rebuilds them");
    }
    let Some(store) = self.store.take() else { return };
    if let Err(error) = store.end_bulk() {
      tracing::warn!(error = %error, "could not restore the checkpoint policy after bulk writes");
    }
  }
}

enum Location {
  File(PathBuf),
  Memory,
}

struct Writer {
  conn: Connection,
  lock: WriterLock,
}

pub(crate) enum WriterLock {
  File { kind: StoreLock, path: PathBuf, lock: fd_lock::RwLock<File> },
  InProcess,
}

enum Readers {
  Pool(ReaderPool),
  SharedWithWriter,
}

struct ReaderPool {
  idle: Mutex<Vec<Connection>>,
  returned: Condvar,
  holders: Mutex<Vec<ThreadId>>,
}

#[derive(Clone, Copy)]
enum Role {
  Writer,
  Reader { cache_kib: i64 },
}

fn reader_cache_kib(readers: usize) -> i64 {
  let readers = i64::try_from(readers.max(1)).unwrap_or(i64::MAX);
  READER_CACHE_MAX_KIB.min(READERS_CACHE_BUDGET_KIB / readers)
}

impl Store {
  pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
    Self::open_with(path, StoreOptions::default())
  }

  pub fn open_with(path: impl AsRef<Path>, options: StoreOptions) -> StoreResult<Self> {
    let path = path.as_ref().to_path_buf();
    let mut lock = WriterLock::open(StoreLock::Writer, lock_path_for(&path, ".lock"))?;
    let mut conn = Connection::open(&path)?;
    configure(&conn, Role::Writer, options.busy_timeout)?;
    let found = schema::user_version(&conn)?;
    schema::ensure_supported(found)?;
    if found < schema::SCHEMA_VERSION || !is_wal(&conn)? {
      // An older binary checks the schema only when it opens, so the upgrade waits for any run it has in flight.
      let mut index = WriterLock::open(StoreLock::Index, lock_path_for(&path, ".index.lock"))?;
      index.hold(options.lock_timeout, || {
        lock.hold(options.lock_timeout, || -> StoreResult<()> {
          let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
          if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::corrupt("journal mode", format!("SQLite refused WAL mode and stayed in {mode}")));
          }
          schema::migrate(&mut conn).map_err(|source| StoreError::Migration {
            path: path.clone(),
            from: found,
            to: schema::SCHEMA_VERSION,
            source: Box::new(source),
          })
        })
      })???;
    }
    if !schema::missing_indexes(&conn)?.is_empty() {
      // Missing indexes while another process indexes are its deferred bulk load; that run, or the next one, restores them.
      let mut index = WriterLock::open(StoreLock::Index, lock_path_for(&path, ".index.lock"))?;
      match index.try_hold(|| lock.hold(options.lock_timeout, || restore_indexes(&mut conn)))? {
        Some(restored) => {
          restored??;
        }
        None => tracing::info!(path = %path.display(), "secondary indexes are missing while another process indexes; leaving them to its run"),
      }
    }
    let reader_role = Role::Reader {
      cache_kib: reader_cache_kib(options.readers),
    };
    let readers = (0..options.readers.max(1))
      .map(|_| {
        let reader = Connection::open(&path)?;
        configure(&reader, reader_role, options.busy_timeout)?;
        Ok(reader)
      })
      .collect::<StoreResult<Vec<_>>>()?;
    Ok(Self {
      location: Location::File(path),
      writer: Mutex::new(Writer { conn, lock }),
      writer_thread: Mutex::new(None),
      readers: Readers::Pool(ReaderPool {
        idle: Mutex::new(readers),
        returned: Condvar::new(),
        holders: Mutex::new(Vec::new()),
      }),
      lock_timeout: options.lock_timeout,
      busy_timeout: options.busy_timeout,
      bulk_depth: AtomicUsize::new(0),
    })
  }

  pub fn open_in_memory() -> StoreResult<Self> {
    let mut conn = Connection::open_in_memory()?;
    configure(&conn, Role::Writer, Duration::ZERO)?;
    schema::migrate(&mut conn)?;
    Ok(Self {
      location: Location::Memory,
      writer: Mutex::new(Writer {
        conn,
        lock: WriterLock::InProcess,
      }),
      writer_thread: Mutex::new(None),
      readers: Readers::SharedWithWriter,
      lock_timeout: Duration::ZERO,
      busy_timeout: Duration::ZERO,
      bulk_depth: AtomicUsize::new(0),
    })
  }

  pub fn path(&self) -> Option<&Path> {
    match &self.location {
      Location::File(path) => Some(path),
      Location::Memory => None,
    }
  }

  pub fn write<T, E>(&self, f: impl FnOnce(&mut WriteTx<'_>) -> Result<T, E>) -> Result<T, E>
  where
    E: From<StoreError>,
  {
    let current = thread::current().id();
    if self.is_writer_thread(current) {
      return Err(StoreError::Reentrant.into());
    }
    let mut writer = lock_ignoring_poison(&self.writer);
    let _active = ActiveWriter::enter(&self.writer_thread, current);
    let Writer { conn, lock } = &mut *writer;
    lock.hold(self.lock_timeout, || run_write(conn, f)).map_err(E::from)?
  }

  pub fn read<T, E>(&self, f: impl FnOnce(&ReadTx<'_>) -> Result<T, E>) -> Result<T, E>
  where
    E: From<StoreError>,
  {
    match &self.readers {
      Readers::Pool(pool) => {
        let mut lease = pool.lease()?;
        run_read(lease.connection(), f)
      }
      Readers::SharedWithWriter => {
        if self.is_writer_thread(thread::current().id()) {
          return Err(StoreError::Reentrant.into());
        }
        let mut writer = lock_ignoring_poison(&self.writer);
        run_read(&mut writer.conn, f)
      }
    }
  }

  pub fn bulk_writes(&self) -> StoreResult<BulkWrites<'_>> {
    if self.bulk_depth.fetch_add(1, Ordering::SeqCst) == 0
      && let Err(error) = self.on_writer(|conn| Ok(conn.pragma_update(None, "wal_autocheckpoint", BULK_AUTOCHECKPOINT_PAGES)?))
    {
      self.bulk_depth.fetch_sub(1, Ordering::SeqCst);
      return Err(error);
    }
    Ok(BulkWrites {
      store: Some(self),
      indexes: SecondaryIndexes::Built,
    })
  }

  fn end_bulk(&self) -> StoreResult<Option<Checkpoint>> {
    if self.bulk_depth.fetch_sub(1, Ordering::SeqCst) != 1 {
      return Ok(None);
    }
    let checkpoint = self.on_writer(|conn| {
      conn.pragma_update(None, "wal_autocheckpoint", DEFAULT_AUTOCHECKPOINT_PAGES)?;
      // Without a busy wait, TRUNCATE degrades to PASSIVE whenever a reader or writer holds the WAL, so it never stalls them.
      conn.busy_timeout(Duration::ZERO)?;
      let checkpoint = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok(Checkpoint {
          busy: row.get::<_, i64>(0)? != 0,
          wal_frames: row.get(1)?,
          checkpointed_frames: row.get(2)?,
        })
      });
      conn.busy_timeout(self.busy_timeout)?;
      Ok(checkpoint?)
    })?;
    if let Some(checkpoint) = &checkpoint {
      tracing::debug!(
        wal_frames = checkpoint.wal_frames,
        checkpointed_frames = checkpoint.checkpointed_frames,
        busy = checkpoint.busy,
        "checkpointed after bulk writes"
      );
    }
    Ok(checkpoint)
  }

  fn on_writer<T>(&self, f: impl FnOnce(&Connection) -> StoreResult<T>) -> StoreResult<Option<T>> {
    if let Location::Memory = self.location {
      return Ok(None);
    }
    if self.is_writer_thread(thread::current().id()) {
      return Err(StoreError::Reentrant);
    }
    let writer = lock_ignoring_poison(&self.writer);
    f(&writer.conn).map(Some)
  }

  fn is_writer_thread(&self, current: ThreadId) -> bool {
    *lock_ignoring_poison(&self.writer_thread) == Some(current)
  }
}

pub(crate) fn lock_path_for(path: &Path, suffix: &str) -> PathBuf {
  let mut name = OsString::from(path.as_os_str());
  name.push(suffix);
  PathBuf::from(name)
}

impl WriteTx<'_> {
  pub fn restore_missing_indexes(&mut self) -> StoreResult<usize> {
    schema::create_missing_indexes(self.conn())
  }
}

fn restore_indexes(conn: &mut Connection) -> StoreResult<usize> {
  let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
  let created = schema::create_missing_indexes(&tx)?;
  tx.commit()?;
  Ok(created)
}

fn lock_ignoring_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
  mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn is_wal(conn: &Connection) -> StoreResult<bool> {
  let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
  Ok(mode.eq_ignore_ascii_case("wal"))
}

fn configure(conn: &Connection, role: Role, busy_timeout: Duration) -> StoreResult<()> {
  conn.busy_timeout(busy_timeout)?;
  conn.pragma_update(None, "foreign_keys", "ON")?;
  conn.pragma_update(None, "synchronous", "NORMAL")?;
  conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE_CAPACITY);
  match role {
    Role::Writer => {
      conn.pragma_update(None, "cache_size", WRITER_CACHE_KIB)?;
      // replace_owner's savepoint copies every page it first touches into a temp-store sub-journal.
      conn.pragma_update(None, "temp_store", "MEMORY")?;
      conn.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT_BYTES)?;
    }
    Role::Reader { cache_kib } => {
      conn.pragma_update(None, "cache_size", -cache_kib)?;
      conn.pragma_update(None, "query_only", "ON")?;
    }
  }
  Ok(())
}

fn discard_open_transaction(conn: &Connection) -> StoreResult<()> {
  if conn.is_autocommit() {
    return Ok(());
  }
  tracing::warn!("rolling back a transaction left open by an earlier panic");
  conn.execute_batch("ROLLBACK")?;
  Ok(())
}

fn run_write<T, E>(conn: &mut Connection, f: impl FnOnce(&mut WriteTx<'_>) -> Result<T, E>) -> Result<T, E>
where
  E: From<StoreError>,
{
  discard_open_transaction(conn)?;
  let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(StoreError::from)?;
  let outcome = {
    let mut write = WriteTx::new(&tx);
    f(&mut write)
  };
  match outcome {
    Ok(value) => {
      tx.commit().map_err(StoreError::from)?;
      Ok(value)
    }
    Err(error) => {
      if let Err(rollback) = tx.rollback() {
        tracing::error!(error = %rollback, "rollback after a failed write did not complete");
      }
      Err(error)
    }
  }
}

fn run_read<T, E>(conn: &mut Connection, f: impl FnOnce(&ReadTx<'_>) -> Result<T, E>) -> Result<T, E>
where
  E: From<StoreError>,
{
  discard_open_transaction(conn)?;
  let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred).map_err(StoreError::from)?;
  let outcome = f(&ReadTx::new(&tx));
  tx.rollback().map_err(StoreError::from)?;
  outcome
}

impl WriterLock {
  pub(crate) fn open(kind: StoreLock, path: PathBuf) -> StoreResult<Self> {
    let file = OpenOptions::new()
      .create(true)
      .truncate(false)
      .read(true)
      .write(true)
      .open(&path)
      .map_err(|source| StoreError::Io { path: path.clone(), source })?;
    Ok(Self::File {
      kind,
      path,
      lock: fd_lock::RwLock::new(file),
    })
  }

  fn try_hold<R>(&mut self, f: impl FnOnce() -> R) -> StoreResult<Option<R>> {
    let (path, lock) = match self {
      Self::InProcess => return Ok(Some(f())),
      Self::File { path, lock, .. } => (path, lock),
    };
    match lock.try_write() {
      Ok(_guard) => Ok(Some(f())),
      Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => Ok(None),
      Err(source) => Err(StoreError::Io { path: path.clone(), source }),
    }
  }

  pub(crate) fn hold<R>(&mut self, timeout: Duration, f: impl FnOnce() -> R) -> StoreResult<R> {
    let (kind, path, lock) = match self {
      Self::InProcess => return Ok(f()),
      Self::File { kind, path, lock } => (*kind, path, lock),
    };
    let started = Instant::now();
    let mut backoff = Duration::from_millis(1);
    loop {
      match lock.try_write() {
        Ok(_guard) => return Ok(f()),
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
        Err(source) => return Err(StoreError::Io { path: path.clone(), source }),
      }
      let waited = started.elapsed();
      if waited >= timeout {
        return Err(StoreError::LockTimeout {
          lock: kind,
          path: path.clone(),
          waited,
        });
      }
      thread::sleep(backoff.min(timeout - waited));
      backoff = (backoff * 2).min(MAX_LOCK_BACKOFF);
    }
  }
}

struct ActiveWriter<'a> {
  slot: &'a Mutex<Option<ThreadId>>,
}

impl<'a> ActiveWriter<'a> {
  fn enter(slot: &'a Mutex<Option<ThreadId>>, thread: ThreadId) -> Self {
    *lock_ignoring_poison(slot) = Some(thread);
    Self { slot }
  }
}

impl Drop for ActiveWriter<'_> {
  fn drop(&mut self) {
    *lock_ignoring_poison(self.slot) = None;
  }
}

impl ReaderPool {
  fn lease(&self) -> StoreResult<Lease<'_>> {
    let holder = thread::current().id();
    if lock_ignoring_poison(&self.holders).contains(&holder) {
      return Err(StoreError::Reentrant);
    }
    let mut idle = lock_ignoring_poison(&self.idle);
    loop {
      if let Some(conn) = idle.pop() {
        lock_ignoring_poison(&self.holders).push(holder);
        return Ok(Lease {
          pool: self,
          holder,
          conn: Some(conn),
        });
      }
      idle = self.returned.wait(idle).unwrap_or_else(PoisonError::into_inner);
    }
  }
}

struct Lease<'a> {
  pool: &'a ReaderPool,
  holder: ThreadId,
  conn: Option<Connection>,
}

impl Lease<'_> {
  fn connection(&mut self) -> &mut Connection {
    match &mut self.conn {
      Some(conn) => conn,
      None => unreachable!("a lease holds its connection until it is dropped"),
    }
  }
}

impl Drop for Lease<'_> {
  fn drop(&mut self) {
    lock_ignoring_poison(&self.pool.holders).retain(|holder| *holder != self.holder);
    if let Some(conn) = self.conn.take() {
      lock_ignoring_poison(&self.pool.idle).push(conn);
      self.pool.returned.notify_one();
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_bound_the_total_reader_cache_across_every_reader() {
    assert_eq!(reader_cache_kib(0), 16_384);
    assert_eq!(reader_cache_kib(14), 16_384);
    assert_eq!(reader_cache_kib(32), 16_384);
    assert_eq!(reader_cache_kib(64), 8_192);
    assert!(reader_cache_kib(64) * 64 <= READERS_CACHE_BUDGET_KIB);
  }
}
