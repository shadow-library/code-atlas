use atlas_core::{Certainty, ContentHash, EntityId, Evidence, Freshness, RelPath, RepoName, RunId};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::codec::{from_json, to_json};
use crate::error::{StoreError, StoreResult};
use crate::llm::LlmCallId;
use crate::tx::{ReadTx, WriteTx};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InferredFactId(pub i64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInferredFact {
  pub subjects: Vec<EntityId>,
  pub relation: String,
  pub payload: serde_json::Value,
  pub certainty: Certainty,
  pub evidence: Vec<Evidence>,
  pub created_run: Option<RunId>,
  pub llm_call: Option<LlmCallId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredInferredFact {
  pub id: InferredFactId,
  pub subjects: Vec<EntityId>,
  pub relation: String,
  pub payload: serde_json::Value,
  pub certainty: Certainty,
  pub evidence: Vec<Evidence>,
  pub freshness: Freshness,
  pub created_run: Option<RunId>,
  pub llm_call: Option<LlmCallId>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FreshnessPolicy {
  #[default]
  CurrentOnly,
  IncludeStale,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InferredQuery {
  pub subject: Option<EntityId>,
  pub relation: Option<String>,
  pub freshness: FreshnessPolicy,
}

fn freshness_state(freshness: &Freshness) -> &'static str {
  match freshness {
    Freshness::Current => "current",
    Freshness::Stale { .. } => "stale",
    Freshness::NeedsRevalidation { .. } => "needs_revalidation",
  }
}

struct RawInferred {
  id: i64,
  relation: String,
  payload: String,
  certainty: String,
  evidence: String,
  freshness: String,
  created_run: Option<i64>,
  llm_call_id: Option<i64>,
}

impl RawInferred {
  fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
    Ok(Self {
      id: row.get(0)?,
      relation: row.get(1)?,
      payload: row.get(2)?,
      certainty: row.get(3)?,
      evidence: row.get(4)?,
      freshness: row.get(5)?,
      created_run: row.get(6)?,
      llm_call_id: row.get(7)?,
    })
  }

  fn into_fact(self, conn: &Connection) -> StoreResult<StoredInferredFact> {
    let mut statement = conn.prepare_cached("SELECT entity_id FROM inferred_subjects WHERE fact_id = ?1 ORDER BY position")?;
    let subjects = statement
      .query_map([self.id], |row| row.get::<_, i64>(0))?
      .map(|id| Ok(EntityId::from_i64(id?)))
      .collect::<StoreResult<Vec<_>>>()?;
    Ok(StoredInferredFact {
      id: InferredFactId(self.id),
      subjects,
      relation: self.relation,
      payload: from_json("inferred payload", &self.payload)?,
      certainty: from_json("inferred certainty", &self.certainty)?,
      evidence: from_json("inferred evidence", &self.evidence)?,
      freshness: from_json("inferred freshness", &self.freshness)?,
      created_run: self.created_run.map(RunId),
      llm_call: self.llm_call_id.map(LlmCallId),
    })
  }
}

macro_rules! inferred_columns {
  () => {
    "f.id, f.relation, f.payload, f.certainty, f.evidence, f.freshness, f.created_run, f.llm_call_id"
  };
}

enum EvidenceChange<'a> {
  Rewritten(&'a ContentHash),
  Removed,
}

impl WriteTx<'_> {
  pub fn insert_inferred_fact(&mut self, fact: &NewInferredFact) -> StoreResult<InferredFactId> {
    let Certainty::Inferred { confidence, .. } = &fact.certainty else {
      return Err(StoreError::NotInferred(fact.certainty.tag()));
    };
    let conn = self.conn();
    let freshness = Freshness::Current;
    conn
      .prepare_cached(
        "INSERT INTO inferred_facts (relation, payload, certainty, confidence, evidence, freshness, freshness_state, created_run, llm_call_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
      )?
      .execute(params![
        fact.relation,
        to_json(&fact.payload)?,
        to_json(&fact.certainty)?,
        f64::from(confidence.value()),
        to_json(&fact.evidence)?,
        to_json(&freshness)?,
        freshness_state(&freshness),
        fact.created_run.map(|run| run.0),
        fact.llm_call.map(|call| call.0),
      ])?;
    let id = conn.last_insert_rowid();
    let mut subject = conn.prepare_cached("INSERT INTO inferred_subjects (fact_id, position, entity_id) VALUES (?1, ?2, ?3)")?;
    for (position, entity) in fact.subjects.iter().enumerate() {
      subject.execute(params![id, position as i64, entity.as_i64()])?;
    }
    let mut evidence = conn.prepare_cached("INSERT OR IGNORE INTO inferred_evidence (fact_id, repo, path, content_hash) VALUES (?1, ?2, ?3, ?4)")?;
    for item in &fact.evidence {
      evidence.execute(params![id, item.repo.as_str(), item.path.as_str(), item.content_hash.to_string()])?;
    }
    Ok(InferredFactId(id))
  }

  pub fn delete_inferred_fact(&mut self, id: InferredFactId) -> StoreResult<bool> {
    let deleted = self.conn().prepare_cached("DELETE FROM inferred_facts WHERE id = ?1")?.execute([id.0])?;
    Ok(deleted == 1)
  }

  pub fn mark_stale_where_evidence_changed(&mut self, repo: &RepoName, path: &RelPath, new_hash: &ContentHash) -> StoreResult<usize> {
    self.mark_stale(repo, path, EvidenceChange::Rewritten(new_hash))
  }

  pub fn mark_stale_where_evidence_removed(&mut self, repo: &RepoName, path: &RelPath) -> StoreResult<usize> {
    self.mark_stale(repo, path, EvidenceChange::Removed)
  }

  pub fn mark_needs_revalidation(&mut self, id: InferredFactId, reason: &str) -> StoreResult<bool> {
    let freshness = Freshness::NeedsRevalidation { reason: reason.to_owned() };
    let updated = self
      .conn()
      .prepare_cached("UPDATE inferred_facts SET freshness = ?2, freshness_state = ?3 WHERE id = ?1 AND freshness_state = 'current'")?
      .execute(params![id.0, to_json(&freshness)?, freshness_state(&freshness)])?;
    Ok(updated == 1)
  }

  fn mark_stale(&mut self, repo: &RepoName, path: &RelPath, change: EvidenceChange<'_>) -> StoreResult<usize> {
    let conn = self.conn();
    let (current_hash, reason) = match change {
      EvidenceChange::Rewritten(hash) => (Some(hash.to_string()), format!("{repo}:{path} changed")),
      EvidenceChange::Removed => (None, format!("{repo}:{path} no longer exists")),
    };
    let ids: Vec<i64> = {
      let mut statement = conn.prepare_cached(
        "SELECT DISTINCT f.id FROM inferred_evidence ev JOIN inferred_facts f ON f.id = ev.fact_id \
         WHERE ev.repo = ?1 AND ev.path = ?2 AND (?3 IS NULL OR ev.content_hash <> ?3) AND f.freshness_state <> 'stale' ORDER BY f.id",
      )?;
      let rows = statement.query_map(params![repo.as_str(), path.as_str(), current_hash], |row| row.get(0))?;
      rows.collect::<Result<_, _>>()?
    };
    let freshness = Freshness::Stale { reason };
    let encoded = to_json(&freshness)?;
    let mut update = conn.prepare_cached("UPDATE inferred_facts SET freshness = ?2, freshness_state = ?3 WHERE id = ?1")?;
    for id in &ids {
      update.execute(params![id, encoded, freshness_state(&freshness)])?;
    }
    if !ids.is_empty() {
      tracing::debug!(repo = %repo, path = %path, marked = ids.len(), "marked inferred facts stale");
    }
    Ok(ids.len())
  }
}

impl ReadTx<'_> {
  pub fn inferred_fact(&self, id: InferredFactId) -> StoreResult<Option<StoredInferredFact>> {
    let conn = self.conn();
    let raw = conn
      .prepare_cached(concat!("SELECT ", inferred_columns!(), " FROM inferred_facts f WHERE f.id = ?1"))?
      .query_row([id.0], RawInferred::from_row)
      .optional()?;
    raw.map(|raw| raw.into_fact(conn)).transpose()
  }

  pub fn inferred_facts(&self, query: &InferredQuery) -> StoreResult<Vec<StoredInferredFact>> {
    let conn = self.conn();
    let include_stale = query.freshness == FreshnessPolicy::IncludeStale;
    let raws: Vec<RawInferred> = {
      let mut statement = conn.prepare_cached(concat!(
        "SELECT ",
        inferred_columns!(),
        " FROM inferred_facts f \
         WHERE (?1 IS NULL OR f.id IN (SELECT fact_id FROM inferred_subjects WHERE entity_id = ?1)) \
         AND (?2 IS NULL OR f.relation = ?2) AND (?3 = 1 OR f.freshness_state = 'current') ORDER BY f.id"
      ))?;
      let rows = statement.query_map(
        params![query.subject.map(EntityId::as_i64), query.relation.as_deref(), include_stale],
        RawInferred::from_row,
      )?;
      rows.collect::<Result<_, _>>()?
    };
    raws.into_iter().map(|raw| raw.into_fact(conn)).collect()
  }
}
