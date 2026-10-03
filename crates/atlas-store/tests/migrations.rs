mod common;

use atlas_store::{SCHEMA_VERSION, Store, StoreError};
use common::*;
use rusqlite::Connection;

fn user_version(conn: &Connection) -> i64 {
  conn.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap()
}

fn journal_mode(conn: &Connection) -> String {
  conn.query_row("PRAGMA journal_mode", [], |row| row.get(0)).unwrap()
}

fn tables(conn: &Connection) -> Vec<String> {
  conn
    .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
    .unwrap()
    .query_map([], |row| row.get(0))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

#[test]
fn should_migrate_a_version_zero_database_to_the_current_schema() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let raw = Connection::open(&path).unwrap();
  assert_eq!(user_version(&raw), 0);
  drop(raw);

  Store::open(&path).unwrap();

  let raw = Connection::open(&path).unwrap();
  assert_eq!(user_version(&raw), SCHEMA_VERSION);
  assert_eq!(journal_mode(&raw), "wal");
  assert_eq!(
    tables(&raw),
    [
      "derived_blobs",
      "edge_owners",
      "edges",
      "entities",
      "entity_owners",
      "files",
      "index_runs",
      "inferred_evidence",
      "inferred_facts",
      "inferred_subjects",
      "llm_cache",
      "llm_calls",
      "owners",
      "run_fact_deltas",
      "unresolved_refs",
    ]
  );
}

#[test]
fn should_refuse_a_newer_schema_without_touching_the_file() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let raw = Connection::open(&path).unwrap();
  raw.execute_batch("CREATE TABLE future (x INTEGER)").unwrap();
  raw.pragma_update(None, "user_version", SCHEMA_VERSION + 1).unwrap();
  drop(raw);

  let opened = Store::open(&path);

  match opened {
    Err(StoreError::SchemaTooNew { found, supported }) => {
      assert_eq!(found, SCHEMA_VERSION + 1);
      assert_eq!(supported, SCHEMA_VERSION);
    }
    other => panic!("expected SchemaTooNew, got {:?}", other.err()),
  }
  let raw = Connection::open(&path).unwrap();
  assert_eq!(user_version(&raw), SCHEMA_VERSION + 1);
  assert_eq!(journal_mode(&raw), "delete");
  assert_eq!(tables(&raw), ["future"]);
}

#[test]
fn should_reopen_a_current_database_and_keep_its_facts() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let store = Store::open(&path).unwrap();
  replace(&store, &file_owner("src/lib.rs"), &facts(vec![entity("kept")], vec![]));
  drop(store);

  let reopened = Store::open(&path).unwrap();

  assert_eq!(read(&reopened, |tx| tx.entity(id("kept"))), Some(entity("kept")));
  assert_eq!(reopened.path(), Some(path.as_path()));
  assert!(path.with_file_name("atlas.db.lock").exists());
}
