use atlas_engine::{EnrichOptions, EnrichPhase, EnrichPlan, EnrichReport, LEXICAL_SHARE, LINK_SHARE, NameSignal, PairCandidate, PairSignal, PhaseReport, SUMMARY_SHARE};
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
  let options = EnrichOptions {
    budget,
    summaries: args.summaries,
  };
  if args.dry_run {
    print_plan(&engine.enrich_plan(&options)?, &options)?;
    return Ok(Outcome::degraded_if(!freshness.is_fresh()));
  }
  let report = engine.enrich(&options)?;
  print_report(&report, &options)?;
  let complete = report.phases.iter().all(|phase| phase.semantic.status == SemanticRunStatus::Completed) && report.revalidation.report.rerun.status == SemanticRunStatus::Completed;
  Ok(Outcome::degraded_if(!complete || !freshness.is_fresh()))
}

fn split(budget: &SemanticBudget) -> String {
  let share = |(numerator, denominator): (u32, u32)| format!("{numerator}/{denominator}");
  format!(
    "budget {} calls / {} input tokens: links up to {} (rounded down), structural pairs next, lexical pairs up to {}, summaries up to {} when enabled, revalidation gets the rest",
    budget.max_calls_per_run,
    budget.max_input_tokens_per_run,
    share(LINK_SHARE),
    share(LEXICAL_SHARE),
    share(SUMMARY_SHARE)
  )
}

fn label(phase: EnrichPhase) -> &'static str {
  match phase {
    EnrichPhase::Links => "links",
    EnrichPhase::StructuralPairs => "similarity (structural)",
    EnrichPhase::LexicalPairs => "similarity (lexical)",
    EnrichPhase::Summaries => "summaries",
  }
}

fn print_report(report: &EnrichReport, options: &EnrichOptions) -> CliResult<()> {
  emit_line(format!("enrich run {}: {} candidates; {}", report.run, report.candidates(), split(&report.budget)))?;
  for phase in &report.phases {
    emit_line(phase_line(phase))?;
  }
  if !options.summaries {
    emit_line("summaries: off (enable with --summaries)")?;
  }
  let revalidation = &report.revalidation;
  let stale_summaries = match revalidation.summaries_left_stale {
    0 => String::new(),
    count => format!(", {count} stale summaries left for --summaries"),
  };
  emit_line(format!(
    "revalidation: {} due, {} unchanged, {} skipped, {} obsolete links pruned, {} links with changed candidates{stale_summaries}; {}",
    revalidation.due,
    revalidation.report.unchanged.len(),
    revalidation.report.skipped.len(),
    revalidation.pruned_links,
    revalidation.outdated_links,
    describe(&revalidation.report.rerun)
  ))
}

fn phase_line(phase: &PhaseReport) -> String {
  let counts = phase.counts();
  format!(
    "{}: {}; {} candidates ({} without readable source, {} deferred over budget); skipped {} unavailable, {} over budget; {} facts written, {} no relation, {} failed",
    label(phase.phase),
    describe(&phase.semantic),
    phase.candidates,
    phase.without_source,
    phase.deferred,
    counts.skipped_unavailable,
    counts.skipped_budget,
    counts.facts_written,
    counts.no_relation,
    counts.failed
  )
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

fn pair_line(candidate: &PairCandidate) -> String {
  let signal = match &candidate.signal {
    PairSignal::Structural { jaccard, small } => format!("jaccard {jaccard:.3}{}", if *small { ", small bodies" } else { "" }),
    PairSignal::Lexical { bm25, name, signature } => {
      let name = match name {
        NameSignal::SharedWords { words, overlap } => format!("shared name words {} (overlap {overlap:.2})", words.join(", ")),
        NameSignal::RelatedConcepts(terms) => format!("related name concepts {}", terms.join(", ")),
      };
      format!("bm25 {bm25:.3}, {name}, signature {}", signature.as_str())
    }
  };
  format!("{:.3} {} <> {} ({signal})", candidate.score, candidate.left_key, candidate.right_key)
}

fn print_plan(plan: &EnrichPlan, options: &EnrichOptions) -> CliResult<()> {
  emit_line(format!("dry run: no LLM calls and no writes; {}", split(&options.budget)))?;
  emit_line(format!(
    "links ({} unmatched calls, {} without a same-method endpoint)",
    plan.links.candidates.len(),
    plan.links.without_endpoints
  ))?;
  for candidate in &plan.links.candidates {
    let caller = candidate.site.as_ref().map(|site| format!(" from {}", site.caller.name)).unwrap_or_default();
    let endpoints: Vec<String> = candidate
      .endpoints
      .iter()
      .map(|choice| format!("{} {} {:.3}", choice.method, choice.route, choice.score))
      .collect();
    emit_line(format!("  {} {}{caller}: {}", candidate.method.as_str(), candidate.target, endpoints.join(", ")))?;
  }
  for (phase, candidates) in [(EnrichPhase::StructuralPairs, &plan.structural), (EnrichPhase::LexicalPairs, &plan.lexical)] {
    emit_line(format!("{} ({})", label(phase), candidates.len()))?;
    for candidate in candidates {
      emit_line(format!("  {}", pair_line(candidate)))?;
    }
  }
  match &plan.summaries {
    None => emit_line("summaries: off (enable with --summaries)"),
    Some(candidates) => {
      emit_line(format!("summaries ({})", candidates.len()))?;
      for candidate in candidates {
        emit_line(format!(
          "  {} ({} symbols, {} imports)",
          candidate.file.key,
          candidate.symbols.len(),
          candidate.imports.len()
        ))?;
      }
      Ok(())
    }
  }
}
