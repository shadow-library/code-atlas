use atlas_core::{ContentHash, Edge, Entity};
use rusqlite::Connection;
use serde::de::DeserializeOwned;

use crate::codec::{canonical_json, from_json};
use crate::error::StoreResult;
use crate::facts::{Assertion, merge_edge, merge_entity, parse_owner};
use crate::tx::ReadTx;

const DIGEST_CONTEXT: &str = "code-atlas 2026-10-04 fact-set digest v1";

struct DigestWriter {
  hasher: blake3::Hasher,
}

impl DigestWriter {
  fn field(&mut self, bytes: &[u8]) {
    self.hasher.update(&(bytes.len() as u64).to_le_bytes());
    self.hasher.update(bytes);
  }

  fn record(&mut self, tag: &str, merged: &str, owners: &[String]) {
    self.field(tag.as_bytes());
    self.field(merged.as_bytes());
    self.hasher.update(&(owners.len() as u64).to_le_bytes());
    for owner in owners {
      self.field(owner.as_bytes());
    }
  }
}

impl ReadTx<'_> {
  pub fn fact_set_digest(&self) -> StoreResult<ContentHash> {
    let conn = self.conn();
    let mut digest = DigestWriter {
      hasher: blake3::Hasher::new_derive_key(DIGEST_CONTEXT),
    };
    for_each_group::<Entity>(
      conn,
      "SELECT eo.entity_id, o.key, eo.data FROM entity_owners eo JOIN owners o ON o.id = eo.owner_id ORDER BY eo.entity_id, o.key",
      "entity",
      |owners, assertions| {
        let Some(merged) = merge_entity(assertions)? else { return Ok(()) };
        digest.record("entity", &canonical_json(&merged.entity)?, owners);
        Ok(())
      },
    )?;
    for_each_group::<Edge>(
      conn,
      "SELECT eo.edge_id, o.key, eo.data FROM edge_owners eo JOIN owners o ON o.id = eo.owner_id ORDER BY eo.edge_id, o.key",
      "edge",
      |owners, assertions| {
        let Some(merged) = merge_edge(assertions)? else { return Ok(()) };
        digest.record("edge", &canonical_json(&merged.edge)?, owners);
        Ok(())
      },
    )?;
    let mut statement = conn.prepare_cached("SELECT o.key, u.data FROM unresolved_refs u JOIN owners o ON o.id = u.owner_id ORDER BY o.key, u.ordinal")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    for row in rows {
      let (owner, data) = row?;
      digest.record("unresolved", &data, &[owner]);
    }
    Ok(ContentHash::from_bytes(*digest.hasher.finalize().as_bytes()))
  }
}

fn for_each_group<T: DeserializeOwned>(conn: &Connection, sql: &str, what: &'static str, mut visit: impl FnMut(&[String], &[Assertion<T>]) -> StoreResult<()>) -> StoreResult<()> {
  let mut statement = conn.prepare_cached(sql)?;
  let mut rows = statement.query([])?;
  let mut current: Option<i64> = None;
  let mut owners = Vec::new();
  let mut assertions = Vec::new();
  while let Some(row) = rows.next()? {
    let id: i64 = row.get(0)?;
    if current.is_some_and(|current| current != id) {
      visit(&owners, &assertions)?;
      owners.clear();
      assertions.clear();
    }
    current = Some(id);
    let owner: String = row.get(1)?;
    let data: String = row.get(2)?;
    assertions.push(Assertion {
      owner: parse_owner(&owner)?,
      fact: from_json(what, &data)?,
    });
    owners.push(owner);
  }
  if current.is_some() {
    visit(&owners, &assertions)?;
  }
  Ok(())
}
