use atlas_core::{CertaintyClass, EdgeId, EdgeKind, EntityId, EntityKind, Owner, RelPath, RepoName, UnresolvedRef};
use rusqlite::{OptionalExtension, params, params_from_iter};

use crate::codec::from_json;
use crate::error::{StoreError, StoreResult};
use crate::facts::{OwnedUnresolved, parse_owner};
use crate::filter::{CertaintyBounds, EdgeFilter};
use crate::tx::ReadTx;

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeLink {
  pub id: EdgeId,
  pub kind: EdgeKind,
  pub from: EntityId,
  pub to: EntityId,
  pub certainty_class: CertaintyClass,
  pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeEnd {
  Source,
  Target,
}

const LINKS_TO: &str =
  "SELECT id, kind, src, dst, certainty_class, confidence FROM edges WHERE dst = ?1 AND certainty_tier >= ?2 AND (?3 = 0 OR certainty_class <> 'inferred') ORDER BY id";
const LINKS_FROM: &str =
  "SELECT id, kind, src, dst, certainty_class, confidence FROM edges WHERE src = ?1 AND certainty_tier >= ?2 AND (?3 = 0 OR certainty_class <> 'inferred') ORDER BY id";

impl ReadTx<'_> {
  pub fn edge_links_to(&self, id: EntityId, filter: &EdgeFilter) -> StoreResult<Vec<EdgeLink>> {
    self.edge_links(LINKS_TO, id, filter)
  }

  pub fn edge_links_from(&self, id: EntityId, filter: &EdgeFilter) -> StoreResult<Vec<EdgeLink>> {
    self.edge_links(LINKS_FROM, id, filter)
  }

  fn edge_links(&self, sql: &str, id: EntityId, filter: &EdgeFilter) -> StoreResult<Vec<EdgeLink>> {
    let CertaintyBounds { min_tier, exclude_inferred } = filter.bounds();
    let mut statement = self.conn().prepare_cached(sql)?;
    let rows = statement.query_map(params![id.as_i64(), min_tier, exclude_inferred], link_row)?;
    let mut links = Vec::new();
    for row in rows {
      let row = row?;
      if filter.admits_kind(&row.1) {
        links.push(edge_link(row)?);
      }
    }
    Ok(links)
  }

  pub fn edge_links_of_kind(&self, kind: &EdgeKind, filter: &EdgeFilter) -> StoreResult<Vec<EdgeLink>> {
    let CertaintyBounds { min_tier, exclude_inferred } = filter.bounds();
    let mut statement = self.conn().prepare_cached(
      "SELECT id, kind, src, dst, certainty_class, confidence FROM edges WHERE kind = ?1 AND certainty_tier >= ?2 AND (?3 = 0 OR certainty_class <> 'inferred') ORDER BY id",
    )?;
    let rows = statement.query_map(params![kind.as_str(), min_tier, exclude_inferred], link_row)?;
    rows.map(|row| edge_link(row?)).collect()
  }

  pub fn edge_count_to(&self, id: EntityId) -> StoreResult<u64> {
    let count: i64 = self
      .conn()
      .prepare_cached("SELECT count(*) FROM edges WHERE dst = ?1")?
      .query_row([id.as_i64()], |row| row.get(0))?;
    Ok(u64::try_from(count).unwrap_or_default())
  }

  pub fn edge_count_of_kind(&self, kind: &EdgeKind) -> StoreResult<u64> {
    let count: i64 = self
      .conn()
      .prepare_cached("SELECT count(*) FROM edges WHERE kind = ?1")?
      .query_row([kind.as_str()], |row| row.get(0))?;
    Ok(u64::try_from(count).unwrap_or_default())
  }

  pub fn edge_kind_meets_entity_kind(&self, edge: &EdgeKind, end: EdgeEnd, entity: &EntityKind) -> StoreResult<bool> {
    let sql = match end {
      EdgeEnd::Source => "SELECT 1 FROM edges e JOIN entities n ON n.id = e.src WHERE e.kind = ?1 AND n.kind = ?2 LIMIT 1",
      EdgeEnd::Target => "SELECT 1 FROM edges e JOIN entities n ON n.id = e.dst WHERE e.kind = ?1 AND n.kind = ?2 LIMIT 1",
    };
    let found = self
      .conn()
      .prepare_cached(sql)?
      .query_row(params![edge.as_str(), entity.as_str()], |row| row.get::<_, i64>(0))
      .optional()?;
    Ok(found.is_some())
  }

  pub fn has_unresolved_with_name_prefix(&self, prefix: &str) -> StoreResult<bool> {
    let (sql, bounds) = name_range("SELECT 1 FROM unresolved_refs u WHERE u.name >= ?1", "LIMIT 1", 1, prefix);
    let found = self
      .conn()
      .prepare_cached(&sql)?
      .query_row(params_from_iter(bounds), |row| row.get::<_, i64>(0))
      .optional()?;
    Ok(found.is_some())
  }

  pub fn unresolved_in_repo(&self, repo: &RepoName) -> StoreResult<Vec<OwnedUnresolved>> {
    let mut statement = self
      .conn()
      .prepare_cached("SELECT o.path, u.data FROM owners o JOIN unresolved_refs u ON u.owner_id = o.id WHERE o.kind = 'file' AND o.repo = ?1 ORDER BY o.path, u.ordinal")?;
    let rows = statement.query_map([repo.as_str()], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    let mut found = Vec::new();
    for row in rows {
      let (path, data) = row?;
      let path = RelPath::new(&path).map_err(|error| StoreError::corrupt("owner path", error.to_string()))?;
      found.push(OwnedUnresolved {
        owner: Owner::File { repo: repo.clone(), path },
        reference: from_json("unresolved reference", &data)?,
      });
    }
    Ok(found)
  }

  pub fn unresolved_with_name_prefix(&self, prefix: &str) -> StoreResult<Vec<OwnedUnresolved>> {
    let (sql, bounds) = name_range(
      "SELECT o.key, u.data FROM unresolved_refs u JOIN owners o ON o.id = u.owner_id WHERE u.name >= ?1",
      "ORDER BY o.key, u.ordinal",
      1,
      prefix,
    );
    let mut statement = self.conn().prepare_cached(&sql)?;
    let rows = statement.query_map(params_from_iter(bounds), |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    let mut found = Vec::new();
    for row in rows {
      let (owner, data) = row?;
      let reference: UnresolvedRef = from_json("unresolved reference", &data)?;
      if reference.path.last().is_some_and(|name| name.starts_with(prefix)) {
        found.push(OwnedUnresolved {
          owner: parse_owner(&owner)?,
          reference,
        });
      }
    }
    Ok(found)
  }

  pub fn unresolved_in_file_with_name_prefix(&self, repo: &RepoName, path: &RelPath, prefix: &str) -> StoreResult<Vec<UnresolvedRef>> {
    let (sql, bounds) = name_range(
      "SELECT u.data FROM owners o JOIN unresolved_refs u ON u.owner_id = o.id WHERE o.kind = 'file' AND o.repo = ?1 AND o.path = ?2 AND u.name >= ?3",
      "ORDER BY u.ordinal",
      3,
      prefix,
    );
    let values = [repo.as_str().to_owned(), path.as_str().to_owned()].into_iter().chain(bounds);
    let mut statement = self.conn().prepare_cached(&sql)?;
    let rows = statement.query_map(params_from_iter(values), |row| row.get::<_, String>(0))?;
    let mut found = Vec::new();
    for data in rows {
      let reference: UnresolvedRef = from_json("unresolved reference", &data?)?;
      if reference.path.last().is_some_and(|name| name.starts_with(prefix)) {
        found.push(reference);
      }
    }
    Ok(found)
  }
}

type LinkRow = (i64, String, i64, i64, String, Option<f64>);

fn link_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LinkRow> {
  Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
}

fn edge_link((id, kind, from, to, class, confidence): LinkRow) -> StoreResult<EdgeLink> {
  Ok(EdgeLink {
    id: EdgeId::from_i64(id),
    kind: kind.parse().map_err(|error: atlas_core::CoreError| StoreError::corrupt("edge kind", error.to_string()))?,
    from: EntityId::from_i64(from),
    to: EntityId::from_i64(to),
    certainty_class: class
      .parse()
      .map_err(|error: atlas_core::CoreError| StoreError::corrupt("certainty class", error.to_string()))?,
    confidence: confidence.map(|confidence| confidence as f32),
  })
}

fn name_range(select: &str, order: &str, start_param: usize, prefix: &str) -> (String, Vec<String>) {
  match prefix_end(prefix) {
    Some(end) => (format!("{select} AND u.name < ?{} {order}", start_param + 1), vec![prefix.to_owned(), end]),
    None => (format!("{select} {order}"), vec![prefix.to_owned()]),
  }
}

fn prefix_end(prefix: &str) -> Option<String> {
  let mut chars: Vec<char> = prefix.chars().collect();
  while let Some(last) = chars.pop() {
    if let Some(next) = (u32::from(last) + 1..=u32::from(char::MAX)).find_map(char::from_u32) {
      chars.push(next);
      return Some(chars.into_iter().collect());
    }
  }
  None
}
