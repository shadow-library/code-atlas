use rusqlite::{Connection, TransactionBehavior};

use crate::error::{StoreError, StoreResult};
use crate::stored::migrate_v1_rows;

type Migration = fn(&Connection) -> StoreResult<()>;

const MIGRATIONS: &[Migration] = &[create_v1, compact_v2];

pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;
pub const PROJECT_FINGERPRINT_BLOB: &str = "engine:project";
pub const PROJECT_FINGERPRINT_VERSION: u32 = 1;
const SUPERSEDED_FINGERPRINT: &str = "superseded by a store schema upgrade";

fn create_v1(conn: &Connection) -> StoreResult<()> {
  conn.execute_batch(include_str!("schema_v1.sql"))?;
  Ok(())
}

fn compact_v2(conn: &Connection) -> StoreResult<()> {
  conn.execute_batch("ALTER TABLE owners ADD COLUMN content_hash TEXT;")?;
  migrate_v1_rows(conn)
}

// An engine built for the old schema compares its fingerprint with this blob before every run; a value no engine computes makes it
// degrade instead of writing old-format rows, even if it wins the index lock before a current engine adopts the project.
fn supersede_project_fingerprint(conn: &Connection) -> StoreResult<()> {
  conn.execute(
    "UPDATE derived_blobs SET version = ?2, bytes = ?3 WHERE name = ?1",
    rusqlite::params![PROJECT_FINGERPRINT_BLOB, PROJECT_FINGERPRINT_VERSION, SUPERSEDED_FINGERPRINT.as_bytes()],
  )?;
  Ok(())
}

pub(crate) fn user_version(conn: &Connection) -> StoreResult<i64> {
  Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

pub(crate) fn ensure_supported(found: i64) -> StoreResult<()> {
  if found < 0 {
    return Err(StoreError::InvalidSchemaVersion(found));
  }
  if found > SCHEMA_VERSION {
    return Err(StoreError::SchemaTooNew { found, supported: SCHEMA_VERSION });
  }
  Ok(())
}

pub(crate) fn migrate(conn: &mut Connection) -> StoreResult<()> {
  let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
  let found = user_version(&tx)?;
  ensure_supported(found)?;
  if found == SCHEMA_VERSION {
    return Ok(());
  }
  for migration in MIGRATIONS.iter().skip(found as usize) {
    migration(&tx)?;
  }
  if found > 0 {
    supersede_project_fingerprint(&tx)?;
  }
  tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
  tx.commit()?;
  tracing::info!(from = found, to = SCHEMA_VERSION, "migrated store schema");
  Ok(())
}

const DEFERRABLE_INDEXES: [(&str, &str); 7] = [
  ("entities_name", "CREATE INDEX entities_name ON entities(name)"),
  ("entities_file", "CREATE INDEX entities_file ON entities(repo, path)"),
  ("entities_kind", "CREATE INDEX entities_kind ON entities(kind)"),
  ("edges_src", "CREATE INDEX edges_src ON edges(src)"),
  ("edges_dst", "CREATE INDEX edges_dst ON edges(dst)"),
  ("edges_kind", "CREATE INDEX edges_kind ON edges(kind)"),
  ("unresolved_refs_name", "CREATE INDEX unresolved_refs_name ON unresolved_refs(name)"),
];

pub(crate) fn missing_indexes(conn: &Connection) -> StoreResult<Vec<&'static str>> {
  let mut statement = conn.prepare_cached("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)")?;
  let mut missing = Vec::new();
  for (name, create) in DEFERRABLE_INDEXES {
    if !statement.query_row([name], |row| row.get::<_, bool>(0))? {
      missing.push(create);
    }
  }
  Ok(missing)
}

pub(crate) fn create_missing_indexes(conn: &Connection) -> StoreResult<usize> {
  let missing = missing_indexes(conn)?;
  for create in &missing {
    conn.execute_batch(create)?;
  }
  if !missing.is_empty() {
    tracing::info!(created = missing.len(), "built the secondary fact indexes");
  }
  Ok(missing.len())
}

pub(crate) fn holds_facts(conn: &Connection) -> StoreResult<bool> {
  Ok(conn.query_row(
    "SELECT EXISTS(SELECT 1 FROM entity_owners) OR EXISTS(SELECT 1 FROM edge_owners) OR EXISTS(SELECT 1 FROM unresolved_refs)",
    [],
    |row| row.get(0),
  )?)
}

pub(crate) fn drop_deferrable_indexes(conn: &Connection) -> StoreResult<()> {
  for (name, _) in DEFERRABLE_INDEXES {
    conn.execute_batch(&format!("DROP INDEX IF EXISTS {name}"))?;
  }
  Ok(())
}
