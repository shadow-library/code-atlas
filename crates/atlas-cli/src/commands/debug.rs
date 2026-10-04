use atlas_core::{Owner, RelPath, RepoName, RunId};
use atlas_engine::EngineError;
use atlas_query::{ContextRequest, Freshness};
use atlas_store::{ChangeCounts, RunStatus, UnresolvedCounts};
use serde_json::json;

use crate::cli::{DebugCommand, DebugDumpFileArgs, DebugExplainContextArgs, DebugRunArgs};
use crate::commands::query::finish;
use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::output::{Outcome, emit_json, emit_line};

pub fn run(ctx: &Context, command: DebugCommand) -> CliResult<Outcome> {
  match command {
    DebugCommand::Run(args) => run_detail(ctx, args),
    DebugCommand::ExplainContext(args) => explain_context(ctx, args),
    DebugCommand::DumpFile(args) => dump_file(ctx, args),
  }
}

fn owner_label(owner: &Owner) -> String {
  match owner {
    Owner::File { repo, path } => format!("{repo}:{path}"),
    Owner::Project { pass } => format!("pass {pass}"),
  }
}

fn status_detail(status: &RunStatus) -> Option<&str> {
  match status {
    RunStatus::Running | RunStatus::Ok => None,
    RunStatus::Failed { error } => Some(error),
    RunStatus::Partial { reason } => Some(reason),
  }
}

fn delta(counts: &ChangeCounts) -> String {
  format!("+{} -{} ~{}", counts.added, counts.removed, counts.changed)
}

fn unresolved_delta(counts: &UnresolvedCounts) -> String {
  format!("+{} -{}", counts.added, counts.removed)
}

fn run_detail(ctx: &Context, args: DebugRunArgs) -> CliResult<Outcome> {
  let project = ctx.resolve()?;
  let engine = ctx.open_engine(&project)?;
  let id = RunId(args.id);
  let (run, deltas) = engine.store().read(|tx| -> Result<_, EngineError> {
    let Some(run) = tx.run(id)? else { return Ok((None, Vec::new())) };
    let deltas = tx.run_deltas(id)?;
    Ok((Some(run), deltas))
  })?;
  let run = run.ok_or(CliError::RunNotFound(args.id))?;
  if args.json {
    emit_json(&json!({
      "id": run.id.0,
      "trigger": run.trigger,
      "status": run.status.as_str(),
      "detail": status_detail(&run.status),
      "started_at_ms": run.started_at_ms,
      "finished_at_ms": run.finished_at_ms,
      "phase_timings_ms": run.report.phase_timings_ms,
      "counts": run.report.counts,
      "change_set": run.report.change_set,
      "owners": deltas.iter().map(|owner| json!({
        "owner": owner_label(&owner.owner),
        "entities": owner.entities,
        "edges": owner.edges,
        "unresolved": owner.unresolved,
      })).collect::<Vec<_>>(),
    }))?;
    return Ok(Outcome::Complete);
  }
  let detail = status_detail(&run.status).map(|detail| format!(" ({detail})")).unwrap_or_default();
  emit_line(format!("run {} {} {}{detail}", run.id, run.trigger, run.status.as_str()))?;
  for (phase, millis) in &run.report.phase_timings_ms {
    emit_line(format!("  phase {phase}: {millis}ms"))?;
  }
  for (name, count) in &run.report.counts {
    emit_line(format!("  count {name}: {count}"))?;
  }
  emit_line(format!("change set: {}", run.report.change_set))?;
  emit_line(format!("fact deltas by owner ({})", deltas.len()))?;
  for owner in &deltas {
    emit_line(format!(
      "  {}  entities {}  edges {}  unresolved {}",
      owner_label(&owner.owner),
      delta(&owner.entities),
      delta(&owner.edges),
      unresolved_delta(&owner.unresolved)
    ))?;
  }
  Ok(Outcome::Complete)
}

fn explain_context(ctx: &Context, args: DebugExplainContextArgs) -> CliResult<Outcome> {
  let project = ctx.resolve()?;
  let query = ctx.open_query(&project)?;
  let mut request = ContextRequest::new(args.task);
  request.explain = true;
  finish(query.context(&request)?, args.json)
}

fn dump_file(ctx: &Context, args: DebugDumpFileArgs) -> CliResult<Outcome> {
  let project = ctx.resolve()?;
  let invalid = || CliError::InvalidFileTarget(args.file.clone());
  let (repo, path) = args.file.split_once(':').ok_or_else(invalid)?;
  let repo = RepoName::new(repo)?;
  let path = RelPath::new(path)?;
  let not_indexed = || CliError::FileNotIndexed { what: args.file.clone() };
  if project.repo(repo.as_str()).is_none() {
    return Err(not_indexed());
  }
  let engine = ctx.open_engine(&project)?;
  let report = engine.ensure_fresh()?;
  let owner = Owner::File {
    repo: repo.clone(),
    path: path.clone(),
  };
  let (record, facts) = engine.store().read(|tx| -> Result<_, EngineError> {
    let record = tx.files_in_repo(&repo)?.into_iter().find(|record| record.path == path);
    let facts = tx.owned_facts(&owner)?;
    Ok((record, facts))
  })?;
  let record = record.ok_or_else(not_indexed)?;
  emit_json(&json!({
    "freshness": Freshness::from_report(&report),
    "file": {
      "repo": record.repo,
      "path": record.path,
      "language": record.language.as_ref().map(|language| language.as_str()),
      "content_hash": record.content_hash.to_string(),
      "size": record.size,
      "parse_status": record.parse_status,
      "analyzer": record.analyzer.as_ref().map(|producer| producer.to_string()),
      "indexed_run": record.indexed_run.map(|run| run.0),
    },
    "entities": facts.entities,
    "edges": facts.edges,
    "unresolved": facts.unresolved,
  }))?;
  Ok(Outcome::degraded_if(!report.is_fresh()))
}
