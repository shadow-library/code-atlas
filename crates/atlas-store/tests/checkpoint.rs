mod common;

use std::path::{Path, PathBuf};

use atlas_store::{SecondaryIndexes, Store, StoreError};
use common::*;
use rusqlite::Connection;

const BLOB_BYTES: usize = 1024 * 1024;
const DEFAULT_CHECKPOINT_BYTES: u64 = 1_000 * 4096;

fn wal_path(db: &Path) -> PathBuf {
  let mut path = db.as_os_str().to_owned();
  path.push("-wal");
  PathBuf::from(path)
}

fn wal_bytes(db: &Path) -> u64 {
  std::fs::metadata(wal_path(db)).map_or(0, |metadata| metadata.len())
}

fn write_blobs(store: &Store, prefix: &str, count: usize) {
  for index in 0..count {
    let bytes = vec![index as u8; BLOB_BYTES];
    store.write(|tx| tx.put_blob(&format!("{prefix}:{index}"), 1, &bytes)).unwrap();
  }
}

#[test]
fn should_defer_checkpoints_until_bulk_writes_finish() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let store = Store::open(&path).unwrap();
  let bulk = store.bulk_writes().unwrap();

  write_blobs(&store, "bulk", 12);
  let during = wal_bytes(&path);
  let checkpoint = bulk.finish().unwrap().unwrap();

  assert!(during > 12 * BLOB_BYTES as u64, "the WAL must hold every bulk commit, found {during} bytes");
  assert!(!checkpoint.busy);
  assert_eq!(wal_bytes(&path), 0);
  assert_eq!(store.read(|tx| tx.blob("bulk:11")).unwrap().map(|blob| blob.bytes.len()), Some(BLOB_BYTES));
}

#[test]
fn should_restore_automatic_checkpoints_after_bulk_writes() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let store = Store::open(&path).unwrap();
  store.bulk_writes().unwrap().finish().unwrap();

  write_blobs(&store, "after", 12);

  let after = wal_bytes(&path);
  assert!(
    after < DEFAULT_CHECKPOINT_BYTES + 2 * BLOB_BYTES as u64,
    "automatic checkpoints must keep the WAL small, found {after} bytes"
  );
}

#[test]
fn should_checkpoint_only_when_the_outermost_bulk_scope_finishes() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let store = Store::open(&path).unwrap();
  let outer = store.bulk_writes().unwrap();
  let inner = store.bulk_writes().unwrap();
  write_blobs(&store, "nested", 2);

  let inner_checkpoint = inner.finish().unwrap();
  let before_outer = wal_bytes(&path);
  let outer_checkpoint = outer.finish().unwrap();

  assert_eq!(inner_checkpoint, None);
  assert!(before_outer > 2 * BLOB_BYTES as u64);
  assert!(outer_checkpoint.is_some());
  assert_eq!(wal_bytes(&path), 0);
}

#[test]
fn should_restore_the_checkpoint_policy_when_a_bulk_scope_is_dropped() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let store = Store::open(&path).unwrap();
  drop(store.bulk_writes().unwrap());

  write_blobs(&store, "dropped", 12);

  assert!(wal_bytes(&path) < DEFAULT_CHECKPOINT_BYTES + 2 * BLOB_BYTES as u64);
}

#[test]
fn should_refuse_to_start_bulk_writes_inside_a_write() {
  let dir = tempfile::tempdir().unwrap();
  let store = Store::open(dir.path().join("atlas.db")).unwrap();

  let outcome = store.write(|_| store.bulk_writes().map(drop));

  assert!(matches!(outcome, Err(StoreError::Reentrant)));
}

#[test]
fn should_treat_bulk_writes_on_an_in_memory_store_as_a_no_op() {
  let store = Store::open_in_memory().unwrap();

  let checkpoint = store.bulk_writes().unwrap().finish().unwrap();

  assert_eq!(checkpoint, None);
}

fn index_sql(path: &Path) -> Vec<(String, String)> {
  let conn = Connection::open(path).unwrap();
  let mut statement = conn
    .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL ORDER BY name")
    .unwrap();
  statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
}

#[test]
fn should_defer_secondary_indexes_while_loading_an_empty_store_and_rebuild_them_identically() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let store = Store::open(&path).unwrap();
  let original = index_sql(&path);
  let mut bulk = store.bulk_writes().unwrap();

  let state = bulk.defer_indexes_while_empty().unwrap();
  let during = index_sql(&path);
  replace(&store, &file_owner("src/lib.rs"), &facts(vec![entity("a"), entity("b")], vec![calls("a", "b")]));
  bulk.restore_indexes().unwrap();

  assert_eq!(state, SecondaryIndexes::Deferred);
  assert_eq!(during.len(), original.len() - 7);
  assert_eq!(index_sql(&path), original);
  assert_eq!(read(&store, |tx| tx.entities_by_name("a", &Default::default())), vec![entity("a")]);
  assert_eq!(read(&store, |tx| tx.edges_to(id("b"), &Default::default())), vec![calls("a", "b")]);
  bulk.finish().unwrap();
}

#[test]
fn should_keep_indexes_when_the_store_already_holds_facts() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let store = Store::open(&path).unwrap();
  replace(&store, &file_owner("src/lib.rs"), &facts(vec![entity("a")], vec![]));
  let original = index_sql(&path);
  let mut bulk = store.bulk_writes().unwrap();

  let state = bulk.defer_indexes_while_empty().unwrap();

  assert_eq!(state, SecondaryIndexes::Built);
  assert_eq!(index_sql(&path), original);
  bulk.finish().unwrap();
}

#[test]
fn should_rebuild_deferred_indexes_when_bulk_writes_finish_or_drop() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let store = Store::open(&path).unwrap();
  let original = index_sql(&path);

  let mut finished = store.bulk_writes().unwrap();
  finished.defer_indexes_while_empty().unwrap();
  finished.finish().unwrap();
  let after_finish = index_sql(&path);
  let mut dropped = store.bulk_writes().unwrap();
  dropped.defer_indexes_while_empty().unwrap();
  drop(dropped);

  assert_eq!(after_finish, original);
  assert_eq!(index_sql(&path), original);
}

#[test]
fn should_rebuild_indexes_left_deferred_by_a_crashed_process_on_open() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let store = Store::open(&path).unwrap();
  let original = index_sql(&path);
  let mut bulk = store.bulk_writes().unwrap();
  bulk.defer_indexes_while_empty().unwrap();
  replace(&store, &file_owner("src/lib.rs"), &facts(vec![entity("a")], vec![]));
  std::mem::forget(bulk);
  drop(store);

  let reopened = Store::open(&path).unwrap();

  assert_eq!(index_sql(&path), original);
  assert_eq!(read(&reopened, |tx| tx.entities_by_name("a", &Default::default())), vec![entity("a")]);
}

#[test]
fn should_open_without_waiting_when_another_process_indexes_with_deferred_indexes() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let crashed = Store::open(&path).unwrap();
  let original = index_sql(&path);
  let mut bulk = crashed.bulk_writes().unwrap();
  bulk.defer_indexes_while_empty().unwrap();
  std::mem::forget(bulk);
  drop(crashed);
  let mut name = path.as_os_str().to_owned();
  name.push(".index.lock");
  let file = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(name).unwrap();
  let mut indexer = fd_lock::RwLock::new(file);
  let guard = indexer.try_write().unwrap();
  let options = atlas_store::StoreOptions {
    lock_timeout: std::time::Duration::from_secs(30),
    ..atlas_store::StoreOptions::default()
  };

  let started = std::time::Instant::now();
  let during = Store::open_with(&path, options.clone()).unwrap();
  let waited = started.elapsed();
  let while_indexing = index_sql(&path);
  drop(during);
  drop(guard);
  drop(Store::open_with(&path, options).unwrap());

  assert!(waited < std::time::Duration::from_secs(5), "open waited {waited:?} behind the indexer");
  assert_eq!(while_indexing.len(), original.len() - 7);
  assert_eq!(index_sql(&path), original);
}
