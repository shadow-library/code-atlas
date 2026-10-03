use atlas_core::{ContentHash, RunId, TokenUsage};
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::codec::{from_json, from_sql_int, now_unix_ms, to_json, to_sql_int};
use crate::error::{StoreError, StoreResult};
use crate::tx::{ReadTx, WriteTx};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LlmCallId(pub i64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLlmCacheEntry {
  pub key: ContentHash,
  pub provider: String,
  pub model: String,
  pub response: String,
  pub usage: TokenUsage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmCacheEntry {
  pub key: ContentHash,
  pub provider: String,
  pub model: String,
  pub response: String,
  pub usage: TokenUsage,
  pub created_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
  Hit,
  Miss,
}

impl CacheStatus {
  fn as_str(self) -> &'static str {
    match self {
      Self::Hit => "hit",
      Self::Miss => "miss",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmCallOutcome {
  Ok,
  Failed { error: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewLlmCall {
  pub run: Option<RunId>,
  pub task_kind: String,
  pub provider: String,
  pub model: String,
  pub usage: TokenUsage,
  pub cost_estimate_usd: Option<f64>,
  pub cache: CacheStatus,
  pub duration_ms: u64,
  pub outcome: LlmCallOutcome,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmCallRecord {
  pub id: LlmCallId,
  pub call: NewLlmCall,
  pub created_at_ms: i64,
}

struct RawCall {
  id: i64,
  run_id: Option<i64>,
  task_kind: String,
  provider: String,
  model: String,
  usage: String,
  cost_estimate: Option<f64>,
  cache: String,
  duration_ms: i64,
  outcome: String,
  error: Option<String>,
  created_at: i64,
}

impl RawCall {
  fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
    Ok(Self {
      id: row.get(0)?,
      run_id: row.get(1)?,
      task_kind: row.get(2)?,
      provider: row.get(3)?,
      model: row.get(4)?,
      usage: row.get(5)?,
      cost_estimate: row.get(6)?,
      cache: row.get(7)?,
      duration_ms: row.get(8)?,
      outcome: row.get(9)?,
      error: row.get(10)?,
      created_at: row.get(11)?,
    })
  }

  fn into_record(self) -> StoreResult<LlmCallRecord> {
    let cache = match self.cache.as_str() {
      "hit" => CacheStatus::Hit,
      "miss" => CacheStatus::Miss,
      other => return Err(StoreError::corrupt("llm call cache status", other)),
    };
    let outcome = match (self.outcome.as_str(), self.error) {
      ("ok", None) => LlmCallOutcome::Ok,
      ("failed", Some(error)) => LlmCallOutcome::Failed { error },
      (outcome, error) => return Err(StoreError::corrupt("llm call outcome", format!("{outcome} with error {error:?}"))),
    };
    Ok(LlmCallRecord {
      id: LlmCallId(self.id),
      call: NewLlmCall {
        run: self.run_id.map(RunId),
        task_kind: self.task_kind,
        provider: self.provider,
        model: self.model,
        usage: from_json("llm call usage", &self.usage)?,
        cost_estimate_usd: self.cost_estimate,
        cache,
        duration_ms: from_sql_int("llm call duration", self.duration_ms)?,
        outcome,
      },
      created_at_ms: self.created_at,
    })
  }
}

impl WriteTx<'_> {
  pub fn put_llm_cache(&mut self, entry: &NewLlmCacheEntry) -> StoreResult<()> {
    self
      .conn()
      .prepare_cached(
        "INSERT INTO llm_cache (key, provider, model, response, usage, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(key) DO UPDATE SET provider = excluded.provider, model = excluded.model, response = excluded.response, \
         usage = excluded.usage, created_at = excluded.created_at",
      )?
      .execute(params![
        entry.key.to_string(),
        entry.provider,
        entry.model,
        entry.response,
        to_json(&entry.usage)?,
        now_unix_ms()
      ])?;
    Ok(())
  }

  pub fn record_llm_call(&mut self, call: &NewLlmCall) -> StoreResult<LlmCallId> {
    let (outcome, error) = match &call.outcome {
      LlmCallOutcome::Ok => ("ok", None),
      LlmCallOutcome::Failed { error } => ("failed", Some(error.as_str())),
    };
    self
      .conn()
      .prepare_cached(
        "INSERT INTO llm_calls (run_id, task_kind, provider, model, usage, cost_estimate, cache, duration_ms, outcome, error, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
      )?
      .execute(params![
        call.run.map(|run| run.0),
        call.task_kind,
        call.provider,
        call.model,
        to_json(&call.usage)?,
        call.cost_estimate_usd,
        call.cache.as_str(),
        to_sql_int("llm call duration", call.duration_ms)?,
        outcome,
        error,
        now_unix_ms(),
      ])?;
    Ok(LlmCallId(self.conn().last_insert_rowid()))
  }
}

impl ReadTx<'_> {
  pub fn llm_cache(&self, key: &ContentHash) -> StoreResult<Option<LlmCacheEntry>> {
    let row = self
      .conn()
      .prepare_cached("SELECT provider, model, response, usage, created_at FROM llm_cache WHERE key = ?1")?
      .query_row([key.to_string()], |row| {
        Ok((
          row.get::<_, String>(0)?,
          row.get::<_, String>(1)?,
          row.get::<_, String>(2)?,
          row.get::<_, String>(3)?,
          row.get::<_, i64>(4)?,
        ))
      })
      .optional()?;
    row
      .map(|(provider, model, response, usage, created_at_ms)| {
        Ok(LlmCacheEntry {
          key: *key,
          provider,
          model,
          response,
          usage: from_json("llm cache usage", &usage)?,
          created_at_ms,
        })
      })
      .transpose()
  }

  pub fn llm_calls(&self, run: RunId) -> StoreResult<Vec<LlmCallRecord>> {
    let mut statement = self.conn().prepare_cached(
      "SELECT id, run_id, task_kind, provider, model, usage, cost_estimate, cache, duration_ms, outcome, error, created_at \
       FROM llm_calls WHERE run_id = ?1 ORDER BY id",
    )?;
    let rows = statement.query_map([run.0], RawCall::from_row)?;
    rows.map(|raw| raw?.into_record()).collect()
  }
}
