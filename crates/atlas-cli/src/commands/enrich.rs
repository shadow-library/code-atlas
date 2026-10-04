use atlas_semantic::{SemanticBudget, SemanticReport, SemanticRunStatus};

use crate::cli::EnrichArgs;
use crate::context::Context;
use crate::error::CliResult;
use crate::output::{Outcome, emit_line};

pub fn run(ctx: &Context, args: EnrichArgs) -> CliResult<Outcome> {
  let project = ctx.resolve()?;
  let engine = ctx.open_engine(&project)?;
  let freshness = engine.ensure_fresh()?;
  let mut budget = SemanticBudget::default();
  if let Some(calls) = args.budget_calls {
    budget.max_calls_per_run = calls;
  }
  if let Some(tokens) = args.budget_tokens {
    budget.max_input_tokens_per_run = tokens;
  }
  let report = engine.enrich(budget)?;
  emit_line(format!(
    "enrich run {}: {} candidates ({} without readable source)",
    report.run, report.candidates, report.candidates_without_source
  ))?;
  emit_line(format!("similarity: {}", describe(&report.similarity)))?;
  emit_line(format!(
    "revalidation: {} unchanged, {} skipped; {}",
    report.revalidation.unchanged.len(),
    report.revalidation.skipped.len(),
    describe(&report.revalidation.rerun)
  ))?;
  let complete = report.similarity.status == SemanticRunStatus::Completed && report.revalidation.rerun.status == SemanticRunStatus::Completed;
  Ok(Outcome::degraded_if(!complete || !freshness.is_fresh()))
}

fn describe(report: &SemanticReport) -> String {
  let status = match &report.status {
    SemanticRunStatus::Completed => "completed".to_owned(),
    SemanticRunStatus::BudgetExhausted => "budget exhausted".to_owned(),
    SemanticRunStatus::ProviderUnavailable { reason } => format!("provider unavailable ({reason})"),
  };
  let cost = report.cost_estimate_usd.map_or_else(String::new, |cost| format!(", ~${cost:.4}"));
  format!(
    "{status}; {} provider calls, {} cache hits, {} input / {} output tokens{cost}",
    report.provider_calls, report.cache_hits, report.usage.input, report.usage.output
  )
}
