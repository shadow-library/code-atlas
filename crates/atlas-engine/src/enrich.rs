mod eligibility;
mod links;
mod pairs;
mod signature;
mod sources;
mod summaries;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use atlas_core::{Availability, ContentHash, EntityId, RelPath, RepoName, RunId};
use atlas_semantic::{
  BudgetUse, RELATION_CALLS_ENDPOINT, RELATION_CAPABILITY_SUMMARY, RELATION_EQUIVALENT_TO, RELATION_SIMILAR_TO, RevalidationReport, SemanticBudget, SemanticConfig, SemanticEngine,
  SemanticReport, SemanticRunStatus, SemanticTask, SkipReason, TaskCost, TaskKind, TaskOutcome, TaskProviders, providers_from_settings,
};
use atlas_store::{FreshnessPolicy, InferredFactId, InferredQuery, RunOutcome, RunReport, StoredInferredFact};

pub use links::{
  Agreement, CallSite, EndpointChoice, LinkCandidate, LinkContext, LinkFactState, LinkPlan, client_call_flagged_unmatched, client_call_linked, client_call_site_linked,
  link_fact_state, superseded_fragment_keys,
};
pub use pairs::{NameSignal, PairCandidate, PairSignal};
pub use signature::SignatureMatch;
pub use summaries::SummaryCandidate;

use crate::engine::Engine;
use crate::error::EngineResult;
use sources::Sources;

pub const LINK_SHARE: (u32, u32) = (1, 5);
pub const LEXICAL_SHARE: (u32, u32) = (2, 5);
pub const SUMMARY_SHARE: (u32, u32) = (1, 5);
const CANDIDATE_SCAN_LIMIT: usize = 500;
const CANDIDATES_CHANGED: &str = "the candidate endpoints shown to the model changed";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EnrichOptions {
  pub budget: SemanticBudget,
  pub summaries: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnrichPhase {
  Links,
  StructuralPairs,
  LexicalPairs,
  Summaries,
}

impl EnrichPhase {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Links => "links",
      Self::StructuralPairs => "structural",
      Self::LexicalPairs => "lexical",
      Self::Summaries => "summaries",
    }
  }

  pub fn kind(self) -> TaskKind {
    match self {
      Self::Links => TaskKind::LinkUnresolvedClientCall,
      Self::StructuralPairs | Self::LexicalPairs => TaskKind::ClassifySimilarityPair,
      Self::Summaries => TaskKind::SummarizeCapability,
    }
  }

  fn share(self) -> Option<(u32, u32)> {
    match self {
      Self::Links => Some(LINK_SHARE),
      Self::StructuralPairs => None,
      Self::LexicalPairs => Some(LEXICAL_SHARE),
      Self::Summaries => Some(SUMMARY_SHARE),
    }
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnrichPlan {
  pub links: LinkPlan,
  pub structural: Vec<PairCandidate>,
  pub lexical: Vec<PairCandidate>,
  pub summaries: Option<Vec<SummaryCandidate>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TaskCounts {
  pub calls: u32,
  pub cache_hits: u32,
  pub skipped_unavailable: u64,
  pub skipped_budget: u64,
  pub facts_written: u64,
  pub no_relation: u64,
  pub failed: u64,
}

impl TaskCounts {
  pub fn of(report: &SemanticReport) -> Self {
    let mut counts = Self {
      calls: report.provider_calls,
      cache_hits: report.cache_hits,
      ..Self::default()
    };
    for task in &report.tasks {
      match &task.outcome {
        TaskOutcome::Inferred { .. } => counts.facts_written += 1,
        TaskOutcome::NoRelation => counts.no_relation += 1,
        TaskOutcome::Failed { .. } => counts.failed += 1,
        TaskOutcome::Skipped {
          reason: SkipReason::ProviderUnavailable { .. },
        } => counts.skipped_unavailable += 1,
        TaskOutcome::Skipped {
          reason: SkipReason::BudgetExhausted,
        } => counts.skipped_budget += 1,
        TaskOutcome::Skipped { .. } => {}
      }
    }
    counts
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhaseReport {
  pub phase: EnrichPhase,
  pub allowance: SemanticBudget,
  pub candidates: u64,
  pub without_source: u64,
  pub deferred: u64,
  pub semantic: SemanticReport,
}

impl PhaseReport {
  pub fn counts(&self) -> TaskCounts {
    TaskCounts::of(&self.semantic)
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RevalidationPhase {
  pub allowance: SemanticBudget,
  pub due: u64,
  pub pruned_links: u64,
  pub outdated_links: u64,
  pub summaries_left_stale: u64,
  pub report: RevalidationReport,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnrichReport {
  pub run: RunId,
  pub budget: SemanticBudget,
  pub phases: Vec<PhaseReport>,
  pub revalidation: RevalidationPhase,
}

impl EnrichReport {
  pub fn phase(&self, phase: EnrichPhase) -> Option<&PhaseReport> {
    self.phases.iter().find(|report| report.phase == phase)
  }

  pub fn candidates(&self) -> u64 {
    self.phases.iter().map(|phase| phase.candidates).sum()
  }

  pub fn budget_used(&self) -> BudgetUse {
    self
      .phases
      .iter()
      .map(|phase| phase.semantic.budget_used)
      .fold(self.revalidation.report.rerun.budget_used, BudgetUse::plus)
  }

  fn semantic_reports(&self) -> impl Iterator<Item = &SemanticReport> {
    self.phases.iter().map(|phase| &phase.semantic).chain([&self.revalidation.report.rerun])
  }
}

#[derive(Default)]
struct LinkUpkeep {
  pruned: u64,
  outdated: u64,
}

#[derive(Default)]
struct SkipTally {
  unavailable: u64,
  budget: u64,
}

impl SkipTally {
  fn of<'a>(reports: impl IntoIterator<Item = &'a SemanticReport>) -> Self {
    let mut tally = Self::default();
    for counts in reports.into_iter().map(TaskCounts::of) {
      tally.unavailable += counts.skipped_unavailable;
      tally.budget += counts.skipped_budget;
    }
    tally
  }

  fn outcome<'a>(&self, reports: impl IntoIterator<Item = &'a SemanticReport>) -> RunOutcome {
    let unavailable = reports.into_iter().find_map(|report| match &report.status {
      SemanticRunStatus::ProviderUnavailable { reason } => Some(reason.clone()),
      _ => None,
    });
    match (unavailable, self.unavailable, self.budget) {
      (Some(reason), unavailable, budget) => RunOutcome::Partial {
        reason: format!("llm provider unavailable ({reason}); {unavailable} task(s) skipped, {budget} over budget"),
      },
      (None, 0, 0) => RunOutcome::Ok,
      (None, unavailable, budget) => RunOutcome::Partial {
        reason: format!("{unavailable} task(s) skipped as provider unavailable, {budget} skipped over budget"),
      },
    }
  }
}

struct Phases<'a> {
  engine: &'a Engine,
  providers: &'a TaskProviders,
  run: RunId,
  total: SemanticBudget,
  used: BudgetUse,
  sources: Sources,
}

impl<'a> Phases<'a> {
  fn allowance(&self, share: Option<(u32, u32)>) -> SemanticBudget {
    let remaining = self.total.remaining_after(self.used);
    match share {
      Some((numerator, denominator)) => remaining.capped_by(self.total.share(numerator, denominator)),
      None => remaining,
    }
  }

  fn semantic(&self, allowance: SemanticBudget) -> SemanticEngine<'a> {
    let config = SemanticConfig {
      budget: allowance,
      ..SemanticConfig::default()
    };
    SemanticEngine::with_task_providers(self.providers, self.engine.store(), config)
  }

  fn execute(&mut self, phase: EnrichPhase, count: usize, mut build: impl FnMut(&Engine, &mut Sources, usize) -> Option<SemanticTask>) -> EngineResult<PhaseReport> {
    let allowance = self.allowance(phase.share());
    let available = matches!(self.providers.for_kind(phase.kind()).availability(), Availability::Available);
    let (mut calls_left, mut tokens_left) = (allowance.max_calls_per_run, allowance.max_input_tokens_per_run);
    let (mut tasks, mut without_source, mut deferred) = (Vec::new(), 0u64, 0u64);
    let semantic = self.semantic(allowance);
    for index in 0..count {
      if index >= CANDIDATE_SCAN_LIMIT {
        deferred += 1;
        continue;
      }
      let Some(task) = build(self.engine, &mut self.sources, index) else {
        without_source += 1;
        continue;
      };
      if !available {
        tasks.push(task);
        continue;
      }
      match semantic.cost(&task)? {
        TaskCost::Cached => tasks.push(task),
        TaskCost::Uncached { estimated_input_tokens } if calls_left > 0 && estimated_input_tokens <= tokens_left => {
          calls_left -= 1;
          tokens_left -= estimated_input_tokens;
          tasks.push(task);
        }
        TaskCost::Uncached { .. } => deferred += 1,
      }
    }
    let report = semantic.run_in_priority_order(Some(self.run), tasks)?;
    self.used = self.used.plus(report.budget_used);
    Ok(PhaseReport {
      phase,
      allowance,
      candidates: count as u64,
      without_source,
      deferred,
      semantic: report,
    })
  }
}

fn pair_builder(candidates: &[PairCandidate]) -> impl FnMut(&Engine, &mut Sources, usize) -> Option<SemanticTask> + '_ {
  |engine, sources, index| {
    let candidate = candidates.get(index)?;
    let (left, right) = engine
      .store()
      .read(|tx| -> EngineResult<_> { Ok((tx.entity(candidate.left)?, tx.entity(candidate.right)?)) })
      .ok()?;
    sources.pair_task(engine, &left?, &right?)
  }
}

impl Engine {
  fn task_providers(&self) -> Arc<TaskProviders> {
    match &self.options.llm_providers {
      Some(providers) => Arc::clone(providers),
      None => Arc::new(providers_from_settings(&self.project().settings.llm)),
    }
  }

  pub fn enrich_plan(&self, options: &EnrichOptions) -> EngineResult<EnrichPlan> {
    let index = self.similarity();
    self.store.read(|tx| -> EngineResult<EnrichPlan> {
      let links = links::plan(tx)?;
      let structural = pairs::structural(tx, &index)?;
      let known: BTreeSet<(EntityId, EntityId)> = structural.iter().map(PairCandidate::ids).collect();
      let lexical = pairs::lexical(tx, &index, &known)?;
      let summaries = match options.summaries {
        true => Some(summaries::plan(tx)?),
        false => None,
      };
      Ok(EnrichPlan {
        links,
        structural,
        lexical,
        summaries,
      })
    })
  }

  pub fn enrich(&self, options: &EnrichOptions) -> EngineResult<EnrichReport> {
    let providers = self.task_providers();
    let plan = self.enrich_plan(options)?;
    let run = self.store.write(|tx| -> EngineResult<RunId> { Ok(tx.begin_run("enrich")?) })?;
    let mut phases = Phases {
      engine: self,
      providers: &providers,
      run,
      total: options.budget,
      used: BudgetUse::default(),
      sources: Sources::default(),
    };
    let mut reports = Vec::new();
    let links = &plan.links.candidates;
    reports.push(phases.execute(EnrichPhase::Links, links.len(), |engine, sources, index| sources.link_task(engine, links.get(index)?))?);
    reports.push(phases.execute(EnrichPhase::StructuralPairs, plan.structural.len(), pair_builder(&plan.structural))?);
    reports.push(phases.execute(EnrichPhase::LexicalPairs, plan.lexical.len(), pair_builder(&plan.lexical))?);
    if let Some(candidates) = &plan.summaries {
      reports.push(phases.execute(EnrichPhase::Summaries, candidates.len(), |engine, sources, index| {
        summaries::task(engine, sources, candidates.get(index)?)
      })?);
    }
    let revalidation = self.revalidate_inferred(&mut phases, options)?;
    let report = EnrichReport {
      run,
      budget: options.budget,
      phases: reports,
      revalidation,
    };
    self.finish_enrich_run(&report)?;
    Ok(report)
  }

  fn finish_enrich_run(&self, report: &EnrichReport) -> EngineResult<()> {
    let skips = SkipTally::of(report.semantic_reports());
    let mut counts = BTreeMap::new();
    for phase in &report.phases {
      let tally = phase.counts();
      let name = phase.phase.as_str();
      for (field, value) in [
        ("candidates", phase.candidates),
        ("without_source", phase.without_source),
        ("deferred", phase.deferred),
        ("provider_calls", u64::from(tally.calls)),
        ("cache_hits", u64::from(tally.cache_hits)),
        ("skipped_unavailable", tally.skipped_unavailable),
        ("skipped_budget", tally.skipped_budget),
        ("facts_written", tally.facts_written),
        ("no_relation", tally.no_relation),
        ("failed", tally.failed),
      ] {
        counts.insert(format!("{name}.{field}"), value);
      }
    }
    let calls: u64 = report.semantic_reports().map(|semantic| u64::from(semantic.provider_calls)).sum();
    let hits: u64 = report.semantic_reports().map(|semantic| u64::from(semantic.cache_hits)).sum();
    let revalidation = &report.revalidation;
    counts.extend([
      ("candidates".to_owned(), report.candidates()),
      ("provider_calls".to_owned(), calls),
      ("cache_hits".to_owned(), hits),
      ("revalidation_due".to_owned(), revalidation.due),
      ("link_facts_pruned".to_owned(), revalidation.pruned_links),
      ("link_facts_outdated".to_owned(), revalidation.outdated_links),
      ("tasks_skipped_unavailable".to_owned(), skips.unavailable),
      ("tasks_skipped_budget".to_owned(), skips.budget),
      ("tasks_deferred".to_owned(), report.phases.iter().map(|phase| phase.deferred).sum()),
    ]);
    let ledger = RunReport {
      phase_timings_ms: BTreeMap::new(),
      counts,
      change_set: serde_json::Value::Null,
    };
    let outcome = skips.outcome(report.semantic_reports());
    self.store.write(|tx| -> EngineResult<()> { Ok(tx.finish_run(report.run, outcome, &ledger)?) })?;
    tracing::info!(
      run = report.run.0,
      candidates = report.candidates(),
      calls,
      skipped_unavailable = skips.unavailable,
      skipped_budget = skips.budget,
      "semantic enrichment finished"
    );
    Ok(())
  }

  fn revalidate_inferred(&self, phases: &mut Phases<'_>, options: &EnrichOptions) -> EngineResult<RevalidationPhase> {
    let upkeep = self.settle_link_facts()?;
    let facts = self.store.read(|tx| {
      tx.inferred_facts(&InferredQuery {
        subject: None,
        relation: None,
        freshness: FreshnessPolicy::IncludeStale,
      })
    })?;
    let (due, left_stale): (Vec<StoredInferredFact>, Vec<StoredInferredFact>) = facts
      .into_iter()
      .filter(|fact| !fact.freshness.is_current())
      .partition(|fact| options.summaries || fact.relation != RELATION_CAPABILITY_SUMMARY);
    let current_hashes = self.current_hashes(&due)?;
    let allowance = phases.allowance(None);
    let sources = RefCell::new(std::mem::take(&mut phases.sources));
    let report = phases.semantic(allowance).revalidate(
      Some(phases.run),
      &due,
      |repo, path| current_hashes.get(&(repo.clone(), path.clone())).copied(),
      |fact| self.rebuild_task(fact, &mut sources.borrow_mut()),
    )?;
    phases.sources = sources.into_inner();
    phases.used = phases.used.plus(report.rerun.budget_used);
    Ok(RevalidationPhase {
      allowance,
      due: due.len() as u64,
      pruned_links: upkeep.pruned,
      outdated_links: upkeep.outdated,
      summaries_left_stale: left_stale.len() as u64,
      report,
    })
  }

  fn settle_link_facts(&self) -> EngineResult<LinkUpkeep> {
    let facts = self.store.read(|tx| {
      tx.inferred_facts(&InferredQuery {
        subject: None,
        relation: Some(RELATION_CALLS_ENDPOINT.to_owned()),
        freshness: FreshnessPolicy::IncludeStale,
      })
    })?;
    let states: Vec<(InferredFactId, bool, LinkFactState)> = self.store.read(|tx| -> EngineResult<_> {
      let context = LinkContext::load(tx)?;
      facts
        .iter()
        .map(|fact| Ok((fact.id, fact.freshness.is_current(), links::link_fact_state(tx, &context, fact)?)))
        .collect()
    })?;
    let obsolete: Vec<InferredFactId> = states.iter().filter(|(_, _, state)| state.is_settled()).map(|(id, ..)| *id).collect();
    let outdated: Vec<InferredFactId> = states
      .iter()
      .filter(|(_, current, state)| *current && *state == LinkFactState::CandidatesChanged)
      .map(|(id, ..)| *id)
      .collect();
    if obsolete.is_empty() && outdated.is_empty() {
      return Ok(LinkUpkeep::default());
    }
    self.store.write(|tx| -> EngineResult<()> {
      for id in &obsolete {
        tx.delete_inferred_fact(*id)?;
      }
      for id in &outdated {
        tx.mark_needs_revalidation(*id, CANDIDATES_CHANGED)?;
      }
      Ok(())
    })?;
    tracing::info!(
      pruned = obsolete.len(),
      outdated = outdated.len(),
      "settled inferred client-call links against the current graph"
    );
    Ok(LinkUpkeep {
      pruned: obsolete.len() as u64,
      outdated: outdated.len() as u64,
    })
  }

  fn current_hashes(&self, facts: &[StoredInferredFact]) -> EngineResult<BTreeMap<(RepoName, RelPath), ContentHash>> {
    let files: BTreeSet<(RepoName, RelPath)> = facts
      .iter()
      .flat_map(|fact| fact.evidence.iter().map(|evidence| (evidence.repo.clone(), evidence.path.clone())))
      .collect();
    self.store.read(|tx| -> EngineResult<_> {
      let mut hashes = BTreeMap::new();
      for (repo, path) in files {
        if let Some(record) = tx.file(&repo, &path)? {
          hashes.insert((repo, path), record.content_hash);
        }
      }
      Ok(hashes)
    })
  }

  fn rebuild_task(&self, fact: &StoredInferredFact, sources: &mut Sources) -> Option<SemanticTask> {
    let rebuilt = match fact.relation.as_str() {
      RELATION_EQUIVALENT_TO | RELATION_SIMILAR_TO => self.rebuild_similarity_task(fact, sources),
      RELATION_CALLS_ENDPOINT => self.rebuild_link_task(fact, sources),
      RELATION_CAPABILITY_SUMMARY => self.rebuild_summary_task(fact, sources),
      _ => Ok(None),
    };
    rebuilt.unwrap_or_else(|error| {
      tracing::warn!(fact = fact.id.0, relation = %fact.relation, error = %error, "could not rebuild a semantic task for revalidation");
      None
    })
  }

  fn rebuild_similarity_task(&self, fact: &StoredInferredFact, sources: &mut Sources) -> EngineResult<Option<SemanticTask>> {
    let [left, right] = fact.subjects[..] else { return Ok(None) };
    let (left, right) = self.store.read(|tx| -> EngineResult<_> { Ok((tx.entity(left)?, tx.entity(right)?)) })?;
    let (Some(left), Some(right)) = (left, right) else { return Ok(None) };
    Ok(sources.pair_task(self, &left, &right))
  }

  fn rebuild_link_task(&self, fact: &StoredInferredFact, sources: &mut Sources) -> EngineResult<Option<SemanticTask>> {
    let Some(&call) = fact.subjects.first() else { return Ok(None) };
    let candidate = self.store.read(|tx| -> EngineResult<_> {
      let Some(call) = tx.entity(call)? else { return Ok(None) };
      links::candidate_for(tx, &call, &LinkContext::load(tx)?)
    })?;
    Ok(candidate.and_then(|candidate| sources.link_task(self, &candidate)))
  }

  fn rebuild_summary_task(&self, fact: &StoredInferredFact, sources: &mut Sources) -> EngineResult<Option<SemanticTask>> {
    let Some(&file) = fact.subjects.first() else { return Ok(None) };
    let candidate = self.store.read(|tx| -> EngineResult<_> {
      let Some(file) = tx.entity(file)? else { return Ok(None) };
      summaries::candidate_for(tx, &file)
    })?;
    Ok(candidate.and_then(|candidate| summaries::task(self, sources, &candidate)))
  }
}
