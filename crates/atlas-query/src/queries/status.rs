use std::collections::BTreeMap;

use atlas_core::Owner;
use atlas_engine::{IndexStatus, PASS_LINEAGE};
use atlas_store::{IndexRun, RunStatus};
use serde::Serialize;

use crate::QueryEngine;
use crate::engine::{Snippet, SnippetMode};
use crate::error::QueryResult;
use crate::freshness::{FailedFileView, ScipState};
use crate::model::{EntityRef, Listing, Response, Targeted};
use crate::policy::Policy;
use crate::queries::common::resolve_one;
use crate::target::TargetFilter;

const DEFAULT_RUNS: usize = 5;
const CHANGE_ENTRIES_SHOWN: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunView {
  pub id: i64,
  pub trigger: String,
  pub status: String,
  pub detail: Option<String>,
  pub started_at_ms: i64,
  pub finished_at_ms: Option<i64>,
  pub counts: BTreeMap<String, u64>,
}

impl RunView {
  fn of(run: &IndexRun) -> Self {
    let detail = match &run.status {
      RunStatus::Failed { error } => Some(error.clone()),
      RunStatus::Partial { reason } => Some(reason.clone()),
      RunStatus::Running | RunStatus::Ok => None,
    };
    Self {
      id: run.id.0,
      trigger: run.trigger.clone(),
      status: run.status.as_str().to_owned(),
      detail,
      started_at_ms: run.started_at_ms,
      finished_at_ms: run.finished_at_ms,
      counts: run.report.counts.clone(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatusAnswer {
  pub project: String,
  pub repos: Vec<String>,
  pub runs: Vec<RunView>,
  pub files_by_language: BTreeMap<String, u64>,
  pub files_by_parse_status: BTreeMap<String, u64>,
  pub failed_files: Vec<FailedFileView>,
  pub repos_missing: Vec<FailedFileView>,
  pub unresolved_by_reason: BTreeMap<String, u64>,
  pub edge_certainty: BTreeMap<String, BTreeMap<String, u64>>,
  pub scip_files: u64,
  pub scip_total: u64,
  pub scip: Option<ScipState>,
  pub inferred_current: u64,
  pub inferred_stale: u64,
  pub inferred_needs_revalidation: u64,
  pub llm_calls: u64,
  pub llm_input_tokens: u64,
  pub llm_output_tokens: u64,
  pub llm_cost_estimate_usd: f64,
  pub dangling_detector_edges_dropped: u64,
}

fn failed(file: &atlas_engine::FailedFile) -> FailedFileView {
  FailedFileView {
    repo: file.repo.to_string(),
    path: file.path.as_ref().map(ToString::to_string),
    reason: file.reason.clone(),
  }
}

impl QueryEngine {
  pub fn status(&self) -> QueryResult<Response<StatusAnswer>> {
    let freshness = self.barrier()?;
    let status: IndexStatus = self.engine().status()?;
    let project = self.engine().project();
    let answer = StatusAnswer {
      project: project.name.to_string(),
      repos: project.repos.iter().map(|repo| repo.name.to_string()).collect(),
      runs: status.runs.iter().map(RunView::of).collect(),
      files_by_language: status.files_by_language,
      files_by_parse_status: status.files_by_parse_status,
      failed_files: status.failed_files.iter().map(failed).collect(),
      repos_missing: status.repos_missing.iter().map(failed).collect(),
      unresolved_by_reason: status.unresolved_by_reason,
      edge_certainty: status.edge_certainty,
      scip_files: status.scip.files_with_scip,
      scip_total: status.scip.files_total,
      scip: status.scip.lag.map(|lag| match lag {
        atlas_engine::ScipLag::NeverRun => ScipState::NeverRun,
        atlas_engine::ScipLag::Current => ScipState::Current,
        atlas_engine::ScipLag::Behind { files } => ScipState::Behind { files },
      }),
      inferred_current: status.inferred.current,
      inferred_stale: status.inferred.stale,
      inferred_needs_revalidation: status.inferred.needs_revalidation,
      llm_calls: status.llm.calls,
      llm_input_tokens: status.llm.input_tokens,
      llm_output_tokens: status.llm.output_tokens,
      llm_cost_estimate_usd: status.llm.cost_estimate_usd,
      dangling_detector_edges_dropped: status.dangling_detector_edges_dropped,
    };
    Ok(Response { freshness, answer })
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadRequest {
  pub target: String,
  pub filter: TargetFilter,
  pub body: bool,
  pub context: u32,
}

impl ReadRequest {
  pub fn new(target: impl Into<String>) -> Self {
    Self {
      target: target.into(),
      filter: TargetFilter::default(),
      body: true,
      context: 0,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReadAnswer {
  pub entity: EntityRef,
  pub source: Snippet,
}

impl QueryEngine {
  pub fn read_symbol(&self, request: &ReadRequest) -> QueryResult<Response<Targeted<ReadAnswer>>> {
    let mut freshness = self.barrier()?;
    let resolved = self.read(Policy::with_floor(0.0), |graph| resolve_one::<ReadAnswer>(self, graph, &request.target, &request.filter))?;
    let entity = match resolved {
      Ok(entity) => entity,
      Err(outcome) => return Ok(Response { freshness, answer: outcome }),
    };
    let mode = match request.body {
      true => SnippetMode::Body { max_lines: None },
      false => SnippetMode::Signature,
    };
    let source = self.sources(&mut freshness).snippet(entity.id, mode, request.context)?;
    let answer = Targeted::Found(ReadAnswer {
      entity: EntityRef::of(&entity),
      source,
    });
    Ok(Response { freshness, answer })
  }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChangesRequest {
  pub since_run: Option<i64>,
  pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OwnerChange {
  pub owner: String,
  pub entities: String,
  pub edges: String,
  pub unresolved: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rename {
  pub new: Option<EntityRef>,
  pub old_key: String,
  pub certainty: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunChange {
  pub run: RunView,
  pub analyzed: Vec<String>,
  pub deleted: Vec<String>,
  pub moved: Vec<String>,
  pub omitted_entries: u64,
  pub owners: Listing<OwnerChange>,
  pub renames: Vec<Rename>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChangesAnswer {
  pub runs: Vec<RunChange>,
}

fn capped_entries(value: &serde_json::Value, name: &str) -> (Vec<String>, u64) {
  let Some(section) = value.get(name) else { return (Vec::new(), 0) };
  let entries: Vec<String> = section
    .get("entries")
    .and_then(serde_json::Value::as_array)
    .map(|entries| entries.iter().filter_map(|entry| entry.as_str().map(str::to_owned)).collect())
    .unwrap_or_default();
  let total = section.get("total").and_then(serde_json::Value::as_u64).unwrap_or(entries.len() as u64);
  (entries, total)
}

fn delta(counts: &atlas_store::ChangeCounts) -> String {
  format!("+{} -{} ~{}", counts.added, counts.removed, counts.changed)
}

fn owner_label(owner: &Owner) -> String {
  match owner {
    Owner::File { repo, path } => format!("{repo}:{path}"),
    Owner::Project { pass } => format!("pass {pass}"),
  }
}

impl QueryEngine {
  pub fn changes(&self, request: &ChangesRequest) -> QueryResult<Response<ChangesAnswer>> {
    let freshness = self.barrier()?;
    let limit = if request.limit == 0 { DEFAULT_RUNS } else { request.limit };
    let answer = self.read(Policy::with_floor(0.0), |graph| -> QueryResult<ChangesAnswer> {
      let tx = graph.tx();
      let runs: Vec<IndexRun> = tx
        .runs(limit.max(1) * 4)?
        .into_iter()
        .filter(|run| request.since_run.is_none_or(|since| run.id.0 > since))
        .take(match request.since_run {
          Some(_) => limit,
          None => 1,
        })
        .collect();
      let lineage = tx.owned_facts(&Owner::Project { pass: PASS_LINEAGE.to_owned() })?;
      let mut changes = Vec::new();
      for run in runs {
        let change_set = &run.report.change_set;
        let (analyzed, analyzed_total) = capped_entries(change_set, "analyzed");
        let (deleted, deleted_total) = capped_entries(change_set, "deleted");
        let (moved, moved_total) = capped_entries(change_set, "moved");
        let shown = |mut entries: Vec<String>| {
          entries.truncate(CHANGE_ENTRIES_SHOWN);
          entries
        };
        let omitted_entries = (analyzed_total + deleted_total + moved_total)
          .saturating_sub((analyzed.len().min(CHANGE_ENTRIES_SHOWN) + deleted.len().min(CHANGE_ENTRIES_SHOWN) + moved.len().min(CHANGE_ENTRIES_SHOWN)) as u64);
        let owners: Vec<OwnerChange> = tx
          .run_deltas(run.id)?
          .iter()
          .map(|owner| OwnerChange {
            owner: owner_label(&owner.owner),
            entities: delta(&owner.entities),
            edges: delta(&owner.edges),
            unresolved: format!("+{} -{}", owner.unresolved.added, owner.unresolved.removed),
          })
          .collect();
        let mut renames = Vec::new();
        for edge in lineage
          .edges
          .iter()
          .filter(|edge| edge.attrs.get("run").and_then(serde_json::Value::as_i64) == Some(run.id.0))
        {
          renames.push(Rename {
            new: graph.entity(edge.from)?.as_ref().map(EntityRef::of),
            old_key: edge
              .attrs
              .get("old_key")
              .and_then(serde_json::Value::as_str)
              .map_or_else(|| edge.to.to_string(), str::to_owned),
            certainty: edge.certainty.tag(),
          });
        }
        renames.sort_by(|left, right| left.old_key.cmp(&right.old_key));
        changes.push(RunChange {
          run: RunView::of(&run),
          analyzed: shown(analyzed),
          deleted: shown(deleted),
          moved: shown(moved),
          omitted_entries,
          owners: Listing::truncated(owners, CHANGE_ENTRIES_SHOWN),
          renames,
        });
      }
      Ok(ChangesAnswer { runs: changes })
    })?;
    Ok(Response { freshness, answer })
  }
}
