use atlas_core::RunId;
use rusqlite::{OptionalExtension, params};

use crate::error::StoreResult;
use crate::tx::{ReadTx, WriteTx};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedBlob {
  pub name: String,
  pub version: u32,
  pub bytes: Vec<u8>,
  pub updated_run: Option<RunId>,
}

impl WriteTx<'_> {
  pub fn put_blob(&mut self, name: &str, version: u32, bytes: &[u8]) -> StoreResult<()> {
    self
      .conn()
      .prepare_cached(
        "INSERT INTO derived_blobs (name, version, bytes, updated_run) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(name) DO UPDATE SET version = excluded.version, bytes = excluded.bytes, updated_run = excluded.updated_run",
      )?
      .execute(params![name, version, bytes, self.active_run().map(|run| run.0)])?;
    Ok(())
  }

  pub fn delete_blob(&mut self, name: &str) -> StoreResult<bool> {
    let deleted = self.conn().prepare_cached("DELETE FROM derived_blobs WHERE name = ?1")?.execute([name])?;
    Ok(deleted == 1)
  }
}

impl ReadTx<'_> {
  pub fn blob(&self, name: &str) -> StoreResult<Option<DerivedBlob>> {
    let blob = self
      .conn()
      .prepare_cached("SELECT version, bytes, updated_run FROM derived_blobs WHERE name = ?1")?
      .query_row([name], |row| {
        Ok(DerivedBlob {
          name: name.to_owned(),
          version: row.get(0)?,
          bytes: row.get(1)?,
          updated_run: row.get::<_, Option<i64>>(2)?.map(RunId),
        })
      })
      .optional()?;
    Ok(blob)
  }
}
