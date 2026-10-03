use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};

use atlas_core::{Certainty, Edge, EdgeId, Entity, EntityId, EntityKey, EntityKind, Evidence, Owner, RelPath, RepoName, RunId, UnresolvedRef};
use rusqlite::{Connection, OptionalExtension, ToSql, params};
use serde::{Deserialize, Serialize};

use crate::codec::{certainty_tier, from_json, owner_label, to_json};
use crate::error::{StoreError, StoreResult};
use crate::filter::{CertaintyBounds, EdgeFilter, EntityFilter};
use crate::tx::{ReadTx, WriteTx};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnedFacts {
  pub entities: Vec<Entity>,
  pub edges: Vec<Edge>,
  pub unresolved: Vec<UnresolvedRef>,
}

impl OwnedFacts {
  pub fn is_empty(&self) -> bool {
    self.entities.is_empty() && self.edges.is_empty() && self.unresolved.is_empty()
  }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeCounts {
  pub added: u64,
  pub removed: u64,
  pub changed: u64,
}

impl ChangeCounts {
  pub fn is_empty(&self) -> bool {
    self.added == 0 && self.removed == 0 && self.changed == 0
  }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedCounts {
  pub added: u64,
  pub removed: u64,
}

impl UnresolvedCounts {
  pub fn is_empty(&self) -> bool {
    self.added == 0 && self.removed == 0
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FactDelta {
  pub entities: ChangeCounts,
  pub edges: ChangeCounts,
  pub unresolved: UnresolvedCounts,
  pub created_entities: Vec<EntityId>,
  pub collected_entities: Vec<EntityId>,
}

impl FactDelta {
  pub fn is_empty(&self) -> bool {
    self.entities.is_empty() && self.edges.is_empty() && self.unresolved.is_empty()
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion<T> {
  pub owner: Owner,
  pub fact: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedUnresolved {
  pub owner: Owner,
  pub reference: UnresolvedRef,
}

struct PreparedEntity<'a> {
  entity: &'a Entity,
  id: i64,
  data: String,
}

struct PreparedEdge<'a> {
  edge: &'a Edge,
  id: i64,
  data: String,
}

struct PreparedUnresolved {
  name: String,
  from: i64,
  data: String,
}

struct Prepared<'a> {
  entities: Vec<PreparedEntity<'a>>,
  edges: Vec<PreparedEdge<'a>>,
  unresolved: Vec<PreparedUnresolved>,
}

impl<'a> Prepared<'a> {
  fn new(owner: &Owner, facts: &'a OwnedFacts) -> StoreResult<Self> {
    let invalid = |reason: String| StoreError::InvalidFacts {
      owner: owner_label(owner),
      reason,
    };
    let mut seen = HashSet::with_capacity(facts.entities.len());
    let mut entities = Vec::with_capacity(facts.entities.len());
    for entity in &facts.entities {
      if !entity.is_consistent() {
        return Err(invalid(format!("entity {} does not match its key {}", entity.id, entity.key)));
      }
      if !seen.insert(entity.id) {
        return Err(invalid(format!("entity {} ({}) is asserted twice", entity.id, entity.key)));
      }
      entities.push(PreparedEntity {
        entity,
        id: entity.id.as_i64(),
        data: to_json(entity)?,
      });
    }
    let mut seen = HashSet::with_capacity(facts.edges.len());
    let mut edges = Vec::with_capacity(facts.edges.len());
    for edge in &facts.edges {
      if !seen.insert(edge.id) {
        return Err(invalid(format!("edge {} ({} {} -> {}) is asserted twice", edge.id, edge.kind, edge.from, edge.to)));
      }
      edges.push(PreparedEdge {
        edge,
        id: edge.id.as_i64(),
        data: to_json(edge)?,
      });
    }
    let mut unresolved = facts
      .unresolved
      .iter()
      .map(|reference| {
        Ok(PreparedUnresolved {
          name: reference_name(reference).to_owned(),
          from: reference.from.as_i64(),
          data: to_json(reference)?,
        })
      })
      .collect::<StoreResult<Vec<_>>>()?;
    unresolved.sort_by(|left, right| left.data.cmp(&right.data));
    Ok(Self { entities, edges, unresolved })
  }

  fn is_empty(&self) -> bool {
    self.entities.is_empty() && self.edges.is_empty() && self.unresolved.is_empty()
  }
}

fn reference_name(reference: &UnresolvedRef) -> &str {
  reference.path.last().map_or("", String::as_str)
}

#[derive(PartialEq, Eq)]
enum Presence {
  Present,
  Gone,
}

impl WriteTx<'_> {
  pub fn replace_owner(&mut self, owner: &Owner, facts: &OwnedFacts) -> StoreResult<FactDelta> {
    let prepared = Prepared::new(owner, facts)?;
    let conn = self.conn();
    conn.prepare_cached("SAVEPOINT replace_owner")?.execute([])?;
    let delta = match apply_owner_facts(conn, owner, &prepared, self.active_run()) {
      Ok(delta) => delta,
      Err(error) => {
        if let Err(rollback) = conn.execute_batch("ROLLBACK TO replace_owner; RELEASE replace_owner") {
          tracing::error!(owner = %owner_label(owner), error = %rollback, "rolling back a failed owner replacement did not complete");
        }
        return Err(error);
      }
    };
    conn.prepare_cached("RELEASE replace_owner")?.execute([])?;
    tracing::debug!(
      owner = %owner_label(owner),
      entities_added = delta.entities.added,
      entities_removed = delta.entities.removed,
      entities_changed = delta.entities.changed,
      edges_added = delta.edges.added,
      edges_removed = delta.edges.removed,
      edges_changed = delta.edges.changed,
      "replaced owner facts"
    );
    Ok(delta)
  }

  pub fn remove_owner(&mut self, owner: &Owner) -> StoreResult<FactDelta> {
    self.replace_owner(owner, &OwnedFacts::default())
  }
}

fn apply_owner_facts(conn: &Connection, owner: &Owner, prepared: &Prepared<'_>, active_run: Option<RunId>) -> StoreResult<FactDelta> {
  let owner_id = match find_owner_id(conn, owner)? {
    Some(id) => id,
    None if prepared.is_empty() => return Ok(FactDelta::default()),
    None => insert_owner(conn, owner)?,
  };
  let run = active_run.map(|run| run.0);
  let mut delta = FactDelta::default();
  replace_entities(conn, owner_id, run, &prepared.entities, &mut delta)?;
  replace_edges(conn, owner_id, run, &prepared.edges, &mut delta)?;
  replace_unresolved(conn, owner_id, run, &prepared.unresolved, &mut delta)?;
  if prepared.is_empty() {
    conn.prepare_cached("DELETE FROM owners WHERE id = ?1")?.execute([owner_id])?;
  }
  if let Some(run) = active_run
    && !delta.is_empty()
  {
    crate::runs::record_delta(conn, run, owner, &delta)?;
  }
  Ok(delta)
}

fn find_owner_id(conn: &Connection, owner: &Owner) -> StoreResult<Option<i64>> {
  let key = to_json(owner)?;
  Ok(conn.prepare_cached("SELECT id FROM owners WHERE key = ?1")?.query_row([key], |row| row.get(0)).optional()?)
}

fn insert_owner(conn: &Connection, owner: &Owner) -> StoreResult<i64> {
  let key = to_json(owner)?;
  let (kind, repo, path, pass) = match owner {
    Owner::File { repo, path } => ("file", Some(repo.as_str()), Some(path.as_str()), None),
    Owner::Project { pass } => ("project", None, None, Some(pass.as_str())),
  };
  conn
    .prepare_cached("INSERT INTO owners (key, kind, repo, path, pass) VALUES (?1, ?2, ?3, ?4, ?5)")?
    .execute(params![key, kind, repo, path, pass])?;
  Ok(conn.last_insert_rowid())
}

fn load_owner_rows(conn: &Connection, sql: &str, owner_id: i64) -> StoreResult<HashMap<i64, String>> {
  let mut statement = conn.prepare_cached(sql)?;
  let rows = statement.query_map([owner_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
  Ok(rows.collect::<Result<_, _>>()?)
}

fn sorted_leftovers(previous: HashMap<i64, String>) -> Vec<i64> {
  let mut ids: Vec<i64> = previous.into_keys().collect();
  ids.sort_unstable();
  ids
}

fn replace_entities(conn: &Connection, owner_id: i64, run: Option<i64>, incoming: &[PreparedEntity<'_>], delta: &mut FactDelta) -> StoreResult<()> {
  let mut previous = load_owner_rows(conn, "SELECT entity_id, data FROM entity_owners WHERE owner_id = ?1", owner_id)?;
  for item in incoming {
    match previous.remove(&item.id) {
      Some(data) if data == item.data => {}
      Some(_) => {
        conn
          .prepare_cached("UPDATE entity_owners SET data = ?3, run_id = ?4 WHERE entity_id = ?1 AND owner_id = ?2")?
          .execute(params![item.id, owner_id, item.data, run])?;
        refresh_entity(conn, item.id)?;
        delta.entities.changed += 1;
      }
      None => {
        let created = insert_entity_projection(conn, item.id, item.entity)?;
        conn
          .prepare_cached("INSERT INTO entity_owners (entity_id, owner_id, run_id, data) VALUES (?1, ?2, ?3, ?4)")?
          .execute(params![item.id, owner_id, run, item.data])?;
        if created {
          delta.created_entities.push(item.entity.id);
        } else {
          refresh_entity(conn, item.id)?;
        }
        delta.entities.added += 1;
      }
    }
  }
  for id in sorted_leftovers(previous) {
    conn
      .prepare_cached("DELETE FROM entity_owners WHERE entity_id = ?1 AND owner_id = ?2")?
      .execute(params![id, owner_id])?;
    if refresh_entity(conn, id)? == Presence::Gone {
      delta.collected_entities.push(EntityId::from_i64(id));
    }
    delta.entities.removed += 1;
  }
  delta.created_entities.sort_unstable();
  delta.collected_entities.sort_unstable();
  Ok(())
}

struct EntityProjection<'a> {
  key: String,
  kind: &'a str,
  name: &'a str,
  repo: Option<&'a str>,
  path: Option<&'a str>,
  class: &'static str,
  tier: i64,
  confidence: Option<f64>,
}

impl<'a> EntityProjection<'a> {
  fn of(entity: &'a Entity) -> Self {
    let class = entity.certainty.class();
    Self {
      key: entity.key.to_string(),
      kind: entity.kind.as_str(),
      name: &entity.name,
      repo: entity.repo.as_ref().map(RepoName::as_str),
      path: entity.path.as_ref().map(RelPath::as_str),
      class: class.as_str(),
      tier: certainty_tier(class),
      confidence: entity.certainty.confidence().map(|confidence| f64::from(confidence.value())),
    }
  }
}

fn insert_entity_projection(conn: &Connection, id: i64, entity: &Entity) -> StoreResult<bool> {
  let row = EntityProjection::of(entity);
  let inserted = conn
    .prepare_cached(
      "INSERT INTO entities (id, key, kind, name, repo, path, certainty_class, certainty_tier, confidence) \
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT(id) DO NOTHING",
    )?
    .execute(params![id, row.key, row.kind, row.name, row.repo, row.path, row.class, row.tier, row.confidence])?;
  Ok(inserted == 1)
}

fn refresh_entity(conn: &Connection, id: i64) -> StoreResult<Presence> {
  let assertions = entity_assertions(conn, id)?;
  let Some(winner) = winning_entity(&assertions)? else {
    conn.prepare_cached("DELETE FROM entities WHERE id = ?1")?.execute([id])?;
    return Ok(Presence::Gone);
  };
  let row = EntityProjection::of(winner);
  conn
    .prepare_cached("UPDATE entities SET key = ?2, kind = ?3, name = ?4, repo = ?5, path = ?6, certainty_class = ?7, certainty_tier = ?8, confidence = ?9 WHERE id = ?1")?
    .execute(params![id, row.key, row.kind, row.name, row.repo, row.path, row.class, row.tier, row.confidence])?;
  Ok(Presence::Present)
}

fn strongest<T>(assertions: &[Assertion<T>], certainty: impl Fn(&T) -> &Certainty) -> Option<&Assertion<T>> {
  assertions
    .iter()
    .max_by(|left, right| certainty(&left.fact).cmp_strength(certainty(&right.fact)).then_with(|| right.owner.cmp(&left.owner)))
}

pub(crate) fn winning_entity(assertions: &[Assertion<Entity>]) -> StoreResult<Option<&Entity>> {
  let Some(first) = assertions.first() else { return Ok(None) };
  if let Some(other) = assertions.iter().find(|assertion| assertion.fact.key != first.fact.key) {
    return Err(StoreError::IdCollision {
      what: "entity",
      id: first.fact.id.to_string(),
      existing: first.fact.key.to_string(),
      incoming: other.fact.key.to_string(),
    });
  }
  Ok(strongest(assertions, |entity: &Entity| &entity.certainty).map(|assertion| &assertion.fact))
}

fn replace_edges(conn: &Connection, owner_id: i64, run: Option<i64>, incoming: &[PreparedEdge<'_>], delta: &mut FactDelta) -> StoreResult<()> {
  let mut previous = load_owner_rows(conn, "SELECT edge_id, data FROM edge_owners WHERE owner_id = ?1", owner_id)?;
  for item in incoming {
    match previous.remove(&item.id) {
      Some(data) if data == item.data => {}
      Some(_) => {
        conn
          .prepare_cached("UPDATE edge_owners SET data = ?3, run_id = ?4 WHERE edge_id = ?1 AND owner_id = ?2")?
          .execute(params![item.id, owner_id, item.data, run])?;
        refresh_edge(conn, item.id)?;
        delta.edges.changed += 1;
      }
      None => {
        let created = insert_edge_projection(conn, item.id, item.edge)?;
        conn
          .prepare_cached("INSERT INTO edge_owners (edge_id, owner_id, run_id, data) VALUES (?1, ?2, ?3, ?4)")?
          .execute(params![item.id, owner_id, run, item.data])?;
        if !created {
          refresh_edge(conn, item.id)?;
        }
        delta.edges.added += 1;
      }
    }
  }
  for id in sorted_leftovers(previous) {
    conn
      .prepare_cached("DELETE FROM edge_owners WHERE edge_id = ?1 AND owner_id = ?2")?
      .execute(params![id, owner_id])?;
    refresh_edge(conn, id)?;
    delta.edges.removed += 1;
  }
  Ok(())
}

fn insert_edge_projection(conn: &Connection, id: i64, edge: &Edge) -> StoreResult<bool> {
  let class = edge.certainty.class();
  let inserted = conn
    .prepare_cached(
      "INSERT INTO edges (id, kind, src, dst, certainty_class, certainty_tier, confidence) \
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(id) DO NOTHING",
    )?
    .execute(params![
      id,
      edge.kind.as_str(),
      edge.from.as_i64(),
      edge.to.as_i64(),
      class.as_str(),
      certainty_tier(class),
      edge.certainty.confidence().map(|confidence| f64::from(confidence.value())),
    ])?;
  Ok(inserted == 1)
}

fn refresh_edge(conn: &Connection, id: i64) -> StoreResult<Presence> {
  let assertions = edge_assertions(conn, id)?;
  let Some(merged) = merge_edge(&assertions)? else {
    conn.prepare_cached("DELETE FROM edges WHERE id = ?1")?.execute([id])?;
    return Ok(Presence::Gone);
  };
  let class = merged.certainty.class();
  conn
    .prepare_cached("UPDATE edges SET certainty_class = ?2, certainty_tier = ?3, confidence = ?4 WHERE id = ?1")?
    .execute(params![
      id,
      class.as_str(),
      certainty_tier(class),
      merged.certainty.confidence().map(|confidence| f64::from(confidence.value())),
    ])?;
  Ok(Presence::Present)
}

fn evidence_order(left: &Evidence, right: &Evidence) -> Ordering {
  (&left.repo, &left.path, &left.span, &left.content_hash, &left.producer, &left.note).cmp(&(
    &right.repo,
    &right.path,
    &right.span,
    &right.content_hash,
    &right.producer,
    &right.note,
  ))
}

pub(crate) fn merge_edge(assertions: &[Assertion<Edge>]) -> StoreResult<Option<Edge>> {
  let Some(first) = assertions.first() else { return Ok(None) };
  if let Some(other) = assertions
    .iter()
    .find(|assertion| (&assertion.fact.kind, assertion.fact.from, assertion.fact.to) != (&first.fact.kind, first.fact.from, first.fact.to))
  {
    return Err(StoreError::IdCollision {
      what: "edge",
      id: first.fact.id.to_string(),
      existing: format!("{} {} -> {}", first.fact.kind, first.fact.from, first.fact.to),
      incoming: format!("{} {} -> {}", other.fact.kind, other.fact.from, other.fact.to),
    });
  }
  let Some(winner) = strongest(assertions, |edge: &Edge| &edge.certainty) else {
    return Ok(None);
  };
  let mut merged = winner.fact.clone();
  merged.evidence = assertions.iter().flat_map(|assertion| assertion.fact.evidence.iter().cloned()).collect();
  merged.evidence.sort_by(evidence_order);
  merged.evidence.dedup();
  Ok(Some(merged))
}

fn replace_unresolved(conn: &Connection, owner_id: i64, run: Option<i64>, incoming: &[PreparedUnresolved], delta: &mut FactDelta) -> StoreResult<()> {
  let previous: Vec<String> = {
    let mut statement = conn.prepare_cached("SELECT data FROM unresolved_refs WHERE owner_id = ?1 ORDER BY ordinal")?;
    let rows = statement.query_map([owner_id], |row| row.get(0))?;
    rows.collect::<Result<_, _>>()?
  };
  if previous.len() == incoming.len() && previous.iter().zip(incoming).all(|(old, new)| *old == new.data) {
    return Ok(());
  }
  let (added, removed) = sorted_multiset_difference(&previous, incoming);
  conn.prepare_cached("DELETE FROM unresolved_refs WHERE owner_id = ?1")?.execute([owner_id])?;
  let mut insert = conn.prepare_cached("INSERT INTO unresolved_refs (owner_id, ordinal, name, src, run_id, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
  for (ordinal, item) in incoming.iter().enumerate() {
    insert.execute(params![owner_id, ordinal as i64, item.name, item.from, run, item.data])?;
  }
  delta.unresolved.added += added;
  delta.unresolved.removed += removed;
  Ok(())
}

fn sorted_multiset_difference(previous: &[String], incoming: &[PreparedUnresolved]) -> (u64, u64) {
  let (mut old, mut new) = (0, 0);
  let (mut added, mut removed) = (0, 0);
  while old < previous.len() && new < incoming.len() {
    match previous[old].cmp(&incoming[new].data) {
      Ordering::Equal => {
        old += 1;
        new += 1;
      }
      Ordering::Less => {
        removed += 1;
        old += 1;
      }
      Ordering::Greater => {
        added += 1;
        new += 1;
      }
    }
  }
  (added + (incoming.len() - new) as u64, removed + (previous.len() - old) as u64)
}

pub(crate) fn parse_owner(key: &str) -> StoreResult<Owner> {
  from_json("owner", key)
}

fn entity_assertions(conn: &Connection, id: i64) -> StoreResult<Vec<Assertion<Entity>>> {
  let mut statement = conn.prepare_cached("SELECT o.key, eo.data FROM entity_owners eo JOIN owners o ON o.id = eo.owner_id WHERE eo.entity_id = ?1")?;
  let rows = statement.query_map([id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
  rows
    .map(|row| {
      let (owner, data) = row?;
      Ok(Assertion {
        owner: parse_owner(&owner)?,
        fact: from_json("entity", &data)?,
      })
    })
    .collect()
}

fn edge_assertions(conn: &Connection, id: i64) -> StoreResult<Vec<Assertion<Edge>>> {
  let mut statement = conn.prepare_cached("SELECT o.key, eo.data FROM edge_owners eo JOIN owners o ON o.id = eo.owner_id WHERE eo.edge_id = ?1")?;
  let rows = statement.query_map([id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
  rows
    .map(|row| {
      let (owner, data) = row?;
      Ok(Assertion {
        owner: parse_owner(&owner)?,
        fact: from_json("edge", &data)?,
      })
    })
    .collect()
}

fn sorted_by_owner<T>(mut assertions: Vec<Assertion<T>>) -> Vec<Assertion<T>> {
  assertions.sort_by(|left, right| left.owner.cmp(&right.owner));
  assertions
}

impl ReadTx<'_> {
  pub fn entity(&self, id: EntityId) -> StoreResult<Option<Entity>> {
    let assertions = entity_assertions(self.conn(), id.as_i64())?;
    Ok(winning_entity(&assertions)?.cloned())
  }

  pub fn entity_by_key(&self, key: &EntityKey) -> StoreResult<Option<Entity>> {
    Ok(self.entity(EntityId::of(key))?.filter(|entity| entity.key == *key))
  }

  pub fn entity_assertions(&self, id: EntityId) -> StoreResult<Vec<Assertion<Entity>>> {
    Ok(sorted_by_owner(entity_assertions(self.conn(), id.as_i64())?))
  }

  pub fn entities_by_name(&self, name: &str, filter: &EntityFilter) -> StoreResult<Vec<Entity>> {
    self.select_entities(
      "SELECT id, kind FROM entities WHERE name = ?1 AND certainty_tier >= ?2 AND (?3 = 0 OR certainty_class <> 'inferred') ORDER BY id",
      name,
      None,
      filter,
    )
  }

  pub fn entities_by_kind(&self, kind: &EntityKind, filter: &EntityFilter) -> StoreResult<Vec<Entity>> {
    self.select_entities(
      "SELECT id, kind FROM entities WHERE kind = ?1 AND certainty_tier >= ?2 AND (?3 = 0 OR certainty_class <> 'inferred') ORDER BY id",
      kind.as_str(),
      None,
      filter,
    )
  }

  pub fn entities_in_file(&self, repo: &RepoName, path: &RelPath, filter: &EntityFilter) -> StoreResult<Vec<Entity>> {
    self.select_entities(
      "SELECT id, kind FROM entities \
       WHERE ((repo = ?1 AND path = ?4) OR id IN ( \
         SELECT eo.entity_id FROM owners o JOIN entity_owners eo ON eo.owner_id = o.id WHERE o.kind = 'file' AND o.repo = ?1 AND o.path = ?4)) \
       AND certainty_tier >= ?2 AND (?3 = 0 OR certainty_class <> 'inferred') ORDER BY id",
      repo.as_str(),
      Some(path.as_str()),
      filter,
    )
  }

  fn select_entities(&self, sql: &str, first: &str, path: Option<&str>, filter: &EntityFilter) -> StoreResult<Vec<Entity>> {
    let CertaintyBounds { min_tier, exclude_inferred } = filter.bounds();
    let mut values: Vec<&dyn ToSql> = vec![&first, &min_tier, &exclude_inferred];
    if let Some(path) = &path {
      values.push(path);
    }
    let ids: Vec<(i64, String)> = {
      let mut statement = self.conn().prepare_cached(sql)?;
      let rows = statement.query_map(values.as_slice(), |row| Ok((row.get(0)?, row.get(1)?)))?;
      rows.collect::<Result<_, _>>()?
    };
    let mut entities = Vec::with_capacity(ids.len());
    for (id, kind) in ids {
      if !filter.admits_kind(&kind) {
        continue;
      }
      if let Some(entity) = self.entity(EntityId::from_i64(id))? {
        entities.push(entity);
      }
    }
    Ok(entities)
  }

  pub fn edge(&self, id: EdgeId) -> StoreResult<Option<Edge>> {
    merge_edge(&edge_assertions(self.conn(), id.as_i64())?)
  }

  pub fn edge_assertions(&self, id: EdgeId) -> StoreResult<Vec<Assertion<Edge>>> {
    Ok(sorted_by_owner(edge_assertions(self.conn(), id.as_i64())?))
  }

  pub fn edges_from(&self, id: EntityId, filter: &EdgeFilter) -> StoreResult<Vec<Edge>> {
    self.select_edges(
      "SELECT id, kind FROM edges WHERE src = ?1 AND certainty_tier >= ?2 AND (?3 = 0 OR certainty_class <> 'inferred') ORDER BY id",
      id,
      filter,
    )
  }

  pub fn edges_to(&self, id: EntityId, filter: &EdgeFilter) -> StoreResult<Vec<Edge>> {
    self.select_edges(
      "SELECT id, kind FROM edges WHERE dst = ?1 AND certainty_tier >= ?2 AND (?3 = 0 OR certainty_class <> 'inferred') ORDER BY id",
      id,
      filter,
    )
  }

  fn select_edges(&self, sql: &str, id: EntityId, filter: &EdgeFilter) -> StoreResult<Vec<Edge>> {
    let CertaintyBounds { min_tier, exclude_inferred } = filter.bounds();
    let ids: Vec<(i64, String)> = {
      let mut statement = self.conn().prepare_cached(sql)?;
      let rows = statement.query_map(params![id.as_i64(), min_tier, exclude_inferred], |row| Ok((row.get(0)?, row.get(1)?)))?;
      rows.collect::<Result<_, _>>()?
    };
    let mut edges = Vec::with_capacity(ids.len());
    for (id, kind) in ids {
      if !filter.admits_kind(&kind) {
        continue;
      }
      if let Some(edge) = self.edge(EdgeId::from_i64(id))? {
        edges.push(edge);
      }
    }
    Ok(edges)
  }

  pub fn owned_facts(&self, owner: &Owner) -> StoreResult<OwnedFacts> {
    let conn = self.conn();
    let Some(owner_id) = find_owner_id(conn, owner)? else {
      return Ok(OwnedFacts::default());
    };
    let entities = owner_column(conn, "SELECT data FROM entity_owners WHERE owner_id = ?1 ORDER BY entity_id", owner_id, "entity")?;
    let edges = owner_column(conn, "SELECT data FROM edge_owners WHERE owner_id = ?1 ORDER BY edge_id", owner_id, "edge")?;
    let unresolved = owner_column(
      conn,
      "SELECT data FROM unresolved_refs WHERE owner_id = ?1 ORDER BY ordinal",
      owner_id,
      "unresolved reference",
    )?;
    Ok(OwnedFacts { entities, edges, unresolved })
  }

  pub fn owners_referencing(&self, targets: &[EntityId]) -> StoreResult<Vec<Owner>> {
    let mut owners = BTreeSet::new();
    let mut statement = self
      .conn()
      .prepare_cached("SELECT DISTINCT o.key FROM edges e JOIN edge_owners eo ON eo.edge_id = e.id JOIN owners o ON o.id = eo.owner_id WHERE e.dst = ?1")?;
    for target in targets {
      let keys = statement.query_map([target.as_i64()], |row| row.get::<_, String>(0))?;
      for key in keys {
        owners.insert(parse_owner(&key?)?);
      }
    }
    Ok(owners.into_iter().collect())
  }

  pub fn unresolved_by_names(&self, names: &[&str]) -> StoreResult<Vec<OwnedUnresolved>> {
    let unique: BTreeSet<&str> = names.iter().copied().collect();
    let mut found = Vec::new();
    let mut statement = self
      .conn()
      .prepare_cached("SELECT o.key, u.data FROM unresolved_refs u JOIN owners o ON o.id = u.owner_id WHERE u.name = ?1 ORDER BY o.key, u.ordinal")?;
    for name in unique {
      let rows = statement.query_map([name], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
      for row in rows {
        let (owner, data) = row?;
        found.push(OwnedUnresolved {
          owner: parse_owner(&owner)?,
          reference: from_json("unresolved reference", &data)?,
        });
      }
    }
    Ok(found)
  }
}

fn owner_column<T: serde::de::DeserializeOwned>(conn: &Connection, sql: &str, owner_id: i64, what: &'static str) -> StoreResult<Vec<T>> {
  let mut statement = conn.prepare_cached(sql)?;
  let rows = statement.query_map([owner_id], |row| row.get::<_, String>(0))?;
  rows.map(|data| from_json(what, &data?)).collect()
}
