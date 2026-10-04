use std::borrow::Cow;
use std::collections::HashMap;

use atlas_core::{
  Attrs, Certainty, ContentHash, Edge, EdgeId, EdgeKind, Entity, EntityId, EntityKey, Evidence, Owner, Producer, ReceiverHint, RefKind, RelPath, RepoName, Span, UnresolvedReason,
  UnresolvedRef, Visibility,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::codec::{from_json, to_json};
use crate::error::{StoreError, StoreResult};

#[derive(Debug, Clone, Copy)]
pub(crate) struct OwnerScope<'a> {
  location: Option<(&'a RepoName, &'a RelPath)>,
  hash: Option<ContentHash>,
}

impl<'a> OwnerScope<'a> {
  pub fn new(owner: &'a Owner, hash: Option<ContentHash>) -> Self {
    let location = match owner {
      Owner::File { repo, path } => Some((repo, path)),
      Owner::Project { .. } => None,
    };
    Self { location, hash }
  }

  fn repo(&self) -> Option<&'a RepoName> {
    self.location.map(|(repo, _)| repo)
  }

  fn path(&self) -> Option<&'a RelPath> {
    self.location.map(|(_, path)| path)
  }

  fn is_at(&self, repo: &RepoName, path: &RelPath) -> bool {
    self.location.is_some_and(|(owner_repo, owner_path)| owner_repo == repo && owner_path == path)
  }

  fn hydrated_hash(&self, stored: Option<ContentHash>) -> StoreResult<ContentHash> {
    stored
      .or(self.hash)
      .ok_or_else(|| StoreError::corrupt("evidence content hash", "elided, but its owner records no content hash"))
  }
}

pub(crate) fn scope_hash<'e>(owner: &Owner, edges: impl IntoIterator<Item = &'e Edge>) -> Option<ContentHash> {
  let Owner::File { repo, path } = owner else { return None };
  let mut counts: HashMap<ContentHash, usize> = HashMap::new();
  for evidence in edges.into_iter().flat_map(|edge| &edge.evidence) {
    if evidence.repo == *repo && evidence.path == *path {
      *counts.entry(evidence.content_hash).or_default() += 1;
    }
  }
  counts
    .into_iter()
    .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
    .map(|(hash, _)| hash)
}

fn elide<'v, T: PartialEq + Clone>(value: &'v T, owner: Option<&T>) -> Option<Cow<'v, T>> {
  match owner {
    Some(owner) if owner == value => None,
    _ => Some(Cow::Borrowed(value)),
  }
}

fn hydrate<T: Clone>(stored: Option<Cow<'_, T>>, owner: Option<&T>, what: &'static str) -> StoreResult<T> {
  match (stored, owner) {
    (Some(value), _) => Ok(value.into_owned()),
    (None, Some(owner)) => Ok(owner.clone()),
    (None, None) => Err(StoreError::corrupt(what, "elided, but its owner is not a file")),
  }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct StoredSpan(u32, u32, u32, u32, u32, u32);

impl From<Span> for StoredSpan {
  fn from(span: Span) -> Self {
    let Span {
      start_byte,
      end_byte,
      start_line,
      start_col,
      end_line,
      end_col,
    } = span;
    Self(start_byte, end_byte, start_line, start_col, end_line, end_col)
  }
}

impl From<StoredSpan> for Span {
  fn from(StoredSpan(start_byte, end_byte, start_line, start_col, end_line, end_col): StoredSpan) -> Self {
    Self {
      start_byte,
      end_byte,
      start_line,
      start_col,
      end_line,
      end_col,
    }
  }
}

#[derive(Serialize, Deserialize)]
struct StoredEvidence<'a> {
  #[serde(rename = "s")]
  span: StoredSpan,
  #[serde(rename = "r", default, skip_serializing_if = "Option::is_none")]
  repo: Option<Cow<'a, RepoName>>,
  #[serde(rename = "p", default, skip_serializing_if = "Option::is_none")]
  path: Option<Cow<'a, RelPath>>,
  #[serde(rename = "h", default, skip_serializing_if = "Option::is_none")]
  content_hash: Option<ContentHash>,
  #[serde(rename = "b")]
  producer: (Cow<'a, str>, u32),
  #[serde(rename = "n", default, skip_serializing_if = "Option::is_none")]
  note: Option<Cow<'a, str>>,
}

impl<'a> StoredEvidence<'a> {
  fn of(evidence: &'a Evidence, scope: &OwnerScope<'_>) -> Self {
    let Evidence {
      repo,
      path,
      span,
      content_hash,
      producer,
      note,
    } = evidence;
    let Producer { name: producer, version } = producer;
    let elided_hash = scope.is_at(repo, path) && scope.hash == Some(*content_hash);
    Self {
      span: (*span).into(),
      repo: elide(repo, scope.repo()),
      path: elide(path, scope.path()),
      content_hash: (!elided_hash).then_some(*content_hash),
      producer: (Cow::Borrowed(producer.as_str()), *version),
      note: note.as_deref().map(Cow::Borrowed),
    }
  }

  fn hydrate(self, scope: &OwnerScope<'_>) -> StoreResult<Evidence> {
    let (name, version) = self.producer;
    Ok(Evidence {
      repo: hydrate(self.repo, scope.repo(), "evidence repo")?,
      path: hydrate(self.path, scope.path(), "evidence path")?,
      span: self.span.into(),
      content_hash: scope.hydrated_hash(self.content_hash)?,
      producer: Producer::new(name, version),
      note: self.note.map(Cow::into_owned),
    })
  }
}

#[derive(Serialize, Deserialize)]
struct StoredEdge<'a> {
  #[serde(rename = "k")]
  kind: Cow<'a, EdgeKind>,
  #[serde(rename = "f")]
  from: EntityId,
  #[serde(rename = "t")]
  to: EntityId,
  #[serde(rename = "c")]
  certainty: Cow<'a, Certainty>,
  #[serde(rename = "e", default, skip_serializing_if = "Vec::is_empty")]
  evidence: Vec<StoredEvidence<'a>>,
  #[serde(rename = "a", default, skip_serializing_if = "Attrs::is_empty")]
  attrs: Cow<'a, Attrs>,
}

pub(crate) fn encode_edge(edge: &Edge, scope: &OwnerScope<'_>) -> StoreResult<String> {
  let Edge {
    id: _,
    kind,
    from,
    to,
    certainty,
    evidence,
    attrs,
  } = edge;
  to_json(&StoredEdge {
    kind: Cow::Borrowed(kind),
    from: *from,
    to: *to,
    certainty: Cow::Borrowed(certainty),
    evidence: evidence.iter().map(|item| StoredEvidence::of(item, scope)).collect(),
    attrs: Cow::Borrowed(attrs),
  })
}

pub(crate) fn decode_edge(id: i64, data: &str, scope: &OwnerScope<'_>) -> StoreResult<Edge> {
  let stored: StoredEdge<'_> = from_json("edge", data)?;
  Ok(Edge {
    id: EdgeId::from_i64(id),
    kind: stored.kind.into_owned(),
    from: stored.from,
    to: stored.to,
    certainty: stored.certainty.into_owned(),
    evidence: stored.evidence.into_iter().map(|item| item.hydrate(scope)).collect::<StoreResult<_>>()?,
    attrs: stored.attrs.into_owned(),
  })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StoredPath {
  Owner,
  Other(RelPath),
}

impl Serialize for StoredPath {
  fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    match self {
      Self::Owner => serializer.serialize_str(""),
      Self::Other(path) => serializer.serialize_str(path.as_str()),
    }
  }
}

impl<'de> Deserialize<'de> for StoredPath {
  fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    let text = String::deserialize(deserializer)?;
    match text.is_empty() {
      true => Ok(Self::Owner),
      false => RelPath::new(&text).map(Self::Other).map_err(serde::de::Error::custom),
    }
  }
}

#[derive(Serialize, Deserialize)]
struct StoredEntity<'a> {
  #[serde(rename = "k")]
  key: Cow<'a, EntityKey>,
  #[serde(rename = "n", default, skip_serializing_if = "Option::is_none")]
  name: Option<Cow<'a, str>>,
  #[serde(rename = "p", default, skip_serializing_if = "Option::is_none")]
  path: Option<StoredPath>,
  #[serde(rename = "s", default, skip_serializing_if = "Option::is_none")]
  span: Option<StoredSpan>,
  #[serde(rename = "g", default, skip_serializing_if = "Option::is_none")]
  signature: Option<Cow<'a, str>>,
  #[serde(rename = "d", default, skip_serializing_if = "Option::is_none")]
  doc: Option<Cow<'a, str>>,
  #[serde(rename = "v", default, skip_serializing_if = "Option::is_none")]
  visibility: Option<Cow<'a, Visibility>>,
  #[serde(rename = "c")]
  certainty: Cow<'a, Certainty>,
  #[serde(rename = "a", default, skip_serializing_if = "Attrs::is_empty")]
  attrs: Cow<'a, Attrs>,
}

pub(crate) fn encode_entity(entity: &Entity, scope: &OwnerScope<'_>) -> StoreResult<String> {
  if !entity.is_consistent() {
    return Err(StoreError::corrupt("entity", format!("{} does not match its key {}", entity.id, entity.key)));
  }
  let Entity {
    id: _,
    key,
    kind: _,
    name,
    repo: _,
    path,
    span,
    signature,
    doc,
    visibility,
    certainty,
    attrs,
  } = entity;
  to_json(&StoredEntity {
    key: Cow::Borrowed(key),
    name: (*name != key.display_name()).then_some(Cow::Borrowed(name.as_str())),
    path: path.as_ref().map(|path| match scope.path() == Some(path) {
      true => StoredPath::Owner,
      false => StoredPath::Other(path.clone()),
    }),
    span: span.map(StoredSpan::from),
    signature: signature.as_deref().map(Cow::Borrowed),
    doc: doc.as_deref().map(Cow::Borrowed),
    visibility: visibility.as_ref().map(Cow::Borrowed),
    certainty: Cow::Borrowed(certainty),
    attrs: Cow::Borrowed(attrs),
  })
}

pub(crate) fn decode_entity(id: i64, data: &str, scope: &OwnerScope<'_>) -> StoreResult<Entity> {
  let stored: StoredEntity<'_> = from_json("entity", data)?;
  let key = stored.key.into_owned();
  let path = match stored.path {
    None => None,
    Some(StoredPath::Other(path)) => Some(path),
    Some(StoredPath::Owner) => Some(hydrate(None, scope.path(), "entity path")?),
  };
  Ok(Entity {
    id: EntityId::from_i64(id),
    kind: key.kind(),
    name: stored.name.map_or_else(|| key.display_name(), Cow::into_owned),
    repo: key.repo().cloned(),
    path,
    span: stored.span.map(Span::from),
    signature: stored.signature.map(Cow::into_owned),
    doc: stored.doc.map(Cow::into_owned),
    visibility: stored.visibility.map(Cow::into_owned),
    certainty: stored.certainty.into_owned(),
    attrs: stored.attrs.into_owned(),
    key,
  })
}

#[derive(Serialize, Deserialize)]
struct StoredUnresolved<'a> {
  #[serde(rename = "p")]
  path: Cow<'a, [String]>,
  #[serde(rename = "k")]
  kind: RefKind,
  #[serde(rename = "v", default, skip_serializing_if = "Option::is_none")]
  receiver: Option<Cow<'a, ReceiverHint>>,
  #[serde(rename = "x")]
  reason: Cow<'a, UnresolvedReason>,
  #[serde(rename = "r", default, skip_serializing_if = "Option::is_none")]
  repo: Option<Cow<'a, RepoName>>,
  #[serde(rename = "f", default, skip_serializing_if = "Option::is_none")]
  file: Option<Cow<'a, RelPath>>,
  #[serde(rename = "s")]
  span: StoredSpan,
}

pub(crate) fn encode_unresolved(reference: &UnresolvedRef, scope: &OwnerScope<'_>) -> StoreResult<String> {
  let UnresolvedRef {
    from: _,
    path,
    kind,
    receiver,
    reason,
    repo,
    file,
    span,
  } = reference;
  to_json(&StoredUnresolved {
    path: Cow::Borrowed(path),
    kind: *kind,
    receiver: receiver.as_ref().map(Cow::Borrowed),
    reason: Cow::Borrowed(reason),
    repo: elide(repo, scope.repo()),
    file: elide(file, scope.path()),
    span: (*span).into(),
  })
}

pub(crate) fn decode_unresolved(from: i64, data: &str, scope: &OwnerScope<'_>) -> StoreResult<UnresolvedRef> {
  let stored: StoredUnresolved<'_> = from_json("unresolved reference", data)?;
  Ok(UnresolvedRef {
    from: EntityId::from_i64(from),
    path: stored.path.into_owned(),
    kind: stored.kind,
    receiver: stored.receiver.map(Cow::into_owned),
    reason: stored.reason.into_owned(),
    repo: hydrate(stored.repo, scope.repo(), "unresolved repo")?,
    file: hydrate(stored.file, scope.path(), "unresolved file")?,
    span: stored.span.into(),
  })
}

pub(crate) fn parse_hash(stored: Option<String>) -> StoreResult<Option<ContentHash>> {
  stored.map(|text| text.parse().map_err(|_| StoreError::corrupt("owner content hash", text))).transpose()
}

pub(crate) fn migrate_v1_rows(conn: &Connection) -> StoreResult<()> {
  let owners: Vec<(i64, String)> = {
    let mut statement = conn.prepare("SELECT id, key FROM owners ORDER BY id")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<_, _>>()?
  };
  let mut rows = 0usize;
  for (owner_id, key) in &owners {
    let owner: Owner = from_json("owner", key)?;
    let edges: Vec<(i64, Edge)> = legacy_rows(conn, "SELECT edge_id, data FROM edge_owners WHERE owner_id = ?1", *owner_id, "edge")?;
    let hash = scope_hash(&owner, edges.iter().map(|(_, edge)| edge));
    let scope = OwnerScope::new(&owner, hash);
    conn
      .prepare_cached("UPDATE owners SET content_hash = ?2 WHERE id = ?1")?
      .execute(params![owner_id, hash.map(|hash| hash.to_string())])?;
    let mut update = conn.prepare_cached("UPDATE edge_owners SET data = ?3 WHERE edge_id = ?1 AND owner_id = ?2")?;
    for (id, edge) in &edges {
      update.execute(params![id, owner_id, encode_edge(edge, &scope)?])?;
    }
    let entities: Vec<(i64, Entity)> = legacy_rows(conn, "SELECT entity_id, data FROM entity_owners WHERE owner_id = ?1", *owner_id, "entity")?;
    let mut update = conn.prepare_cached("UPDATE entity_owners SET data = ?3 WHERE entity_id = ?1 AND owner_id = ?2")?;
    for (id, entity) in &entities {
      update.execute(params![id, owner_id, encode_entity(entity, &scope)?])?;
    }
    let unresolved: Vec<(i64, UnresolvedRef)> = legacy_rows(conn, "SELECT ordinal, data FROM unresolved_refs WHERE owner_id = ?1", *owner_id, "unresolved reference")?;
    let mut update = conn.prepare_cached("UPDATE unresolved_refs SET data = ?3 WHERE owner_id = ?1 AND ordinal = ?2")?;
    for (ordinal, reference) in &unresolved {
      update.execute(params![owner_id, ordinal, encode_unresolved(reference, &scope)?])?;
    }
    rows += edges.len() + entities.len() + unresolved.len();
  }
  tracing::info!(
    owners = owners.len(),
    rows,
    "re-encoded stored facts in the compact format; the file keeps its size until a fresh index or a manual VACUUM"
  );
  Ok(())
}

fn legacy_rows<T: serde::de::DeserializeOwned>(conn: &Connection, sql: &str, owner_id: i64, what: &'static str) -> StoreResult<Vec<(i64, T)>> {
  let mut statement = conn.prepare_cached(sql)?;
  let rows = statement.query_map([owner_id], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?;
  rows
    .map(|row| {
      let (id, data) = row?;
      Ok((id, from_json(what, &data)?))
    })
    .collect()
}
