use std::collections::BTreeMap;

use atlas_core::{Owner, RunId};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::codec::{from_json, from_sql_int, now_unix_ms, to_json, to_sql_int};
use crate::error::{StoreError, StoreResult};
use crate::facts::{ChangeCounts, FactDelta, UnresolvedCounts, parse_owner};
use crate::tx::{ReadTx, WriteTx};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
  Running,
  Ok,
  Failed { error: String },
  Partial { reason: String },
}

impl RunStatus {
  pub fn as_str(&self) -> &'static str {
    match self {
      Self::Running => "running",
      Self::Ok => "ok",
      Self::Failed { .. } => "failed",
      Self::Partial { .. } => "partial",
    }
  }

  fn detail(&self) -> Option<&str> {
    match self {
      Self::Running | Self::Ok => None,
      Self::Failed { error } => Some(error),
      Self::Partial { reason } => Some(reason),
    }
  }

  fn from_columns(status: &str, detail: Option<String>) -> StoreResult<Self> {
    let detail = detail.unwrap_or_default();
    match status {
      "running" => Ok(Self::Running),
      "ok" => Ok(Self::Ok),
      "failed" => Ok(Self::Failed { error: detail }),
      "partial" => Ok(Self::Partial { reason: detail }),
      other => Err(StoreError::corrupt("run status", other)),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
  Ok,
  Failed { error: String },
  Partial { reason: String },
}

impl From<RunOutcome> for RunStatus {
  fn from(outcome: RunOutcome) -> Self {
    match outcome {
      RunOutcome::Ok => Self::Ok,
      RunOutcome::Failed { error } => Self::Failed { error },
      RunOutcome::Partial { reason } => Self::Partial { reason },
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReport {
  pub phase_timings_ms: BTreeMap<String, u64>,
  pub counts: BTreeMap<String, u64>,
  pub change_set: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRun {
  pub id: RunId,
  pub trigger: String,
  pub started_at_ms: i64,
  pub finished_at_ms: Option<i64>,
  pub status: RunStatus,
  pub report: RunReport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerDelta {
  pub owner: Owner,
  pub entities: ChangeCounts,
  pub edges: ChangeCounts,
  pub unresolved: UnresolvedCounts,
}

macro_rules! run_columns {
  () => {
    "id, trigger, started_at, finished_at, status, phase_timings, counts, change_set, error"
  };
}

struct RawRun {
  id: i64,
  trigger: String,
  started_at: i64,
  finished_at: Option<i64>,
  status: String,
  phase_timings: String,
  counts: String,
  change_set: String,
  error: Option<String>,
}

impl RawRun {
  fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
    Ok(Self {
      id: row.get(0)?,
      trigger: row.get(1)?,
      started_at: row.get(2)?,
      finished_at: row.get(3)?,
      status: row.get(4)?,
      phase_timings: row.get(5)?,
      counts: row.get(6)?,
      change_set: row.get(7)?,
      error: row.get(8)?,
    })
  }

  fn into_run(self) -> StoreResult<IndexRun> {
    Ok(IndexRun {
      id: RunId(self.id),
      trigger: self.trigger,
      started_at_ms: self.started_at,
      finished_at_ms: self.finished_at,
      status: RunStatus::from_columns(&self.status, self.error)?,
      report: RunReport {
        phase_timings_ms: from_json("run phase timings", &self.phase_timings)?,
        counts: from_json("run counts", &self.counts)?,
        change_set: from_json("run change set", &self.change_set)?,
      },
    })
  }
}

fn load_run(conn: &Connection, id: RunId) -> StoreResult<Option<IndexRun>> {
  let raw = conn
    .prepare_cached(concat!("SELECT ", run_columns!(), " FROM index_runs WHERE id = ?1"))?
    .query_row([id.0], RawRun::from_row)
    .optional()?;
  raw.map(RawRun::into_run).transpose()
}

fn require_running(conn: &Connection, id: RunId) -> StoreResult<()> {
  let run = load_run(conn, id)?.ok_or(StoreError::RunNotFound(id))?;
  if run.status != RunStatus::Running {
    return Err(StoreError::RunNotRunning { id, status: run.status.as_str() });
  }
  Ok(())
}

impl WriteTx<'_> {
  pub fn begin_run(&mut self, trigger: &str) -> StoreResult<RunId> {
    if let Some(active) = self.active_run() {
      return Err(StoreError::RunAlreadyActive { active });
    }
    let empty = RunReport::default();
    self
      .conn()
      .prepare_cached("INSERT INTO index_runs (trigger, started_at, status, phase_timings, counts, change_set) VALUES (?1, ?2, 'running', ?3, ?4, ?5)")?
      .execute(params![
        trigger,
        now_unix_ms(),
        to_json(&empty.phase_timings_ms)?,
        to_json(&empty.counts)?,
        to_json(&empty.change_set)?
      ])?;
    let id = RunId(self.conn().last_insert_rowid());
    self.set_active_run(Some(id));
    tracing::info!(run = id.0, trigger, "began index run");
    Ok(id)
  }

  pub fn resume_run(&mut self, id: RunId) -> StoreResult<()> {
    if let Some(active) = self.active_run().filter(|active| *active != id) {
      return Err(StoreError::RunAlreadyActive { active });
    }
    require_running(self.conn(), id)?;
    self.set_active_run(Some(id));
    Ok(())
  }

  pub fn finish_run(&mut self, id: RunId, outcome: RunOutcome, report: &RunReport) -> StoreResult<()> {
    require_running(self.conn(), id)?;
    let status = RunStatus::from(outcome);
    self
      .conn()
      .prepare_cached("UPDATE index_runs SET finished_at = ?2, status = ?3, error = ?4, phase_timings = ?5, counts = ?6, change_set = ?7 WHERE id = ?1")?
      .execute(params![
        id.0,
        now_unix_ms(),
        status.as_str(),
        status.detail(),
        to_json(&report.phase_timings_ms)?,
        to_json(&report.counts)?,
        to_json(&report.change_set)?,
      ])?;
    if self.active_run() == Some(id) {
      self.set_active_run(None);
    }
    tracing::info!(run = id.0, status = status.as_str(), "finished index run");
    Ok(())
  }
}

impl ReadTx<'_> {
  pub fn run(&self, id: RunId) -> StoreResult<Option<IndexRun>> {
    load_run(self.conn(), id)
  }

  pub fn runs(&self, limit: usize) -> StoreResult<Vec<IndexRun>> {
    let limit = to_sql_int("run limit", limit as u64)?;
    let mut statement = self
      .conn()
      .prepare_cached(concat!("SELECT ", run_columns!(), " FROM index_runs ORDER BY id DESC LIMIT ?1"))?;
    let rows = statement.query_map([limit], RawRun::from_row)?;
    rows.map(|raw| raw?.into_run()).collect()
  }

  pub fn run_deltas(&self, id: RunId) -> StoreResult<Vec<OwnerDelta>> {
    let mut statement = self.conn().prepare_cached(
      "SELECT owner, entities_added, entities_removed, entities_changed, edges_added, edges_removed, edges_changed, unresolved_added, unresolved_removed \
       FROM run_fact_deltas WHERE run_id = ?1 ORDER BY owner",
    )?;
    let rows = statement.query_map([id.0], |row| {
      let mut counts = [0i64; 8];
      for (index, count) in counts.iter_mut().enumerate() {
        *count = row.get(index + 1)?;
      }
      Ok((row.get::<_, String>(0)?, counts))
    })?;
    rows
      .map(|row| {
        let (owner, counts) = row?;
        let count = |index: usize| from_sql_int("fact delta count", counts[index]);
        Ok(OwnerDelta {
          owner: parse_owner(&owner)?,
          entities: ChangeCounts {
            added: count(0)?,
            removed: count(1)?,
            changed: count(2)?,
          },
          edges: ChangeCounts {
            added: count(3)?,
            removed: count(4)?,
            changed: count(5)?,
          },
          unresolved: UnresolvedCounts {
            added: count(6)?,
            removed: count(7)?,
          },
        })
      })
      .collect()
  }
}

pub(crate) fn record_delta(conn: &Connection, run: RunId, owner: &Owner, delta: &FactDelta) -> StoreResult<()> {
  let count = |value: u64| to_sql_int("fact delta count", value);
  conn
    .prepare_cached(
      "INSERT INTO run_fact_deltas (run_id, owner, entities_added, entities_removed, entities_changed, edges_added, edges_removed, edges_changed, \
       unresolved_added, unresolved_removed) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
       ON CONFLICT(run_id, owner) DO UPDATE SET entities_added = entities_added + excluded.entities_added, \
       entities_removed = entities_removed + excluded.entities_removed, entities_changed = entities_changed + excluded.entities_changed, \
       edges_added = edges_added + excluded.edges_added, edges_removed = edges_removed + excluded.edges_removed, \
       edges_changed = edges_changed + excluded.edges_changed, unresolved_added = unresolved_added + excluded.unresolved_added, \
       unresolved_removed = unresolved_removed + excluded.unresolved_removed",
    )?
    .execute(params![
      run.0,
      to_json(owner)?,
      count(delta.entities.added)?,
      count(delta.entities.removed)?,
      count(delta.entities.changed)?,
      count(delta.edges.added)?,
      count(delta.edges.removed)?,
      count(delta.edges.changed)?,
      count(delta.unresolved.added)?,
      count(delta.unresolved.removed)?,
    ])?;
  Ok(())
}
