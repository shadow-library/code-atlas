use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use rusqlite::{Connection, TransactionBehavior};

use crate::error::{StoreError, StoreResult};
use crate::schema;
use crate::tx::{ReadTx, WriteTx};

const STATEMENT_CACHE_CAPACITY: usize = 128;
// SQLite reads a negative cache_size as KiB rather than pages.
const WRITER_CACHE_KIB: i64 = -65_536;
const READER_CACHE_KIB: i64 = -16_384;
const MAX_LOCK_BACKOFF: Duration = Duration::from_millis(50);

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
}

enum Location {
  File(PathBuf),
  Memory,
}

struct Writer {
  conn: Connection,
  lock: WriterLock,
}

enum WriterLock {
  File { path: PathBuf, lock: fd_lock::RwLock<File> },
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
  Reader,
}

impl Store {
  pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
    Self::open_with(path, StoreOptions::default())
  }

  pub fn open_with(path: impl AsRef<Path>, options: StoreOptions) -> StoreResult<Self> {
    let path = path.as_ref().to_path_buf();
    let lock_path = lock_path_for(&path);
    let lock_file = OpenOptions::new()
      .create(true)
      .truncate(false)
      .read(true)
      .write(true)
      .open(&lock_path)
      .map_err(|source| StoreError::Io { path: lock_path.clone(), source })?;
    let mut lock = WriterLock::File {
      path: lock_path,
      lock: fd_lock::RwLock::new(lock_file),
    };
    let mut conn = Connection::open(&path)?;
    configure(&conn, Role::Writer, options.busy_timeout)?;
    let found = schema::user_version(&conn)?;
    schema::ensure_supported(found)?;
    if found < schema::SCHEMA_VERSION || !is_wal(&conn)? {
      lock.hold(options.lock_timeout, || -> StoreResult<()> {
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
          return Err(StoreError::corrupt("journal mode", format!("SQLite refused WAL mode and stayed in {mode}")));
        }
        schema::migrate(&mut conn)
      })??;
    }
    let readers = (0..options.readers.max(1))
      .map(|_| {
        let reader = Connection::open(&path)?;
        configure(&reader, Role::Reader, options.busy_timeout)?;
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

  fn is_writer_thread(&self, current: ThreadId) -> bool {
    *lock_ignoring_poison(&self.writer_thread) == Some(current)
  }
}

fn lock_path_for(path: &Path) -> PathBuf {
  let mut name = OsString::from(path.as_os_str());
  name.push(".lock");
  PathBuf::from(name)
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
    }
    Role::Reader => {
      conn.pragma_update(None, "cache_size", READER_CACHE_KIB)?;
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
  fn hold<R>(&mut self, timeout: Duration, f: impl FnOnce() -> R) -> StoreResult<R> {
    let (path, lock) = match self {
      Self::InProcess => return Ok(f()),
      Self::File { path, lock } => (path, lock),
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
        return Err(StoreError::LockTimeout { path: path.clone(), waited });
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
