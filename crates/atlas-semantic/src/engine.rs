use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use atlas_core::{Availability, Certainty, Confidence, ContentHash, EntityId, Evidence, LlmError, LlmProvider, LlmRequest, Producer, RunId, TokenUsage};
use atlas_store::{
  CacheStatus, FreshnessPolicy, InferredFactId, InferredQuery, LlmCallId, LlmCallOutcome, NewInferredFact, NewLlmCacheEntry, NewLlmCall, ReadTx, Store, StoreResult,
};
use serde_json::Value;

use crate::budget::{BudgetState, BudgetUse, SemanticBudget};
use crate::error::SemanticResult;
use crate::facts::LOW_CONFIDENCE_ATTR;
use crate::pricing::PriceTable;
use crate::provider::TaskProviders;
use crate::task::{FactDraft, Interpretation, RenderedPrompt, SemanticTask, TaskKind};
use crate::validate::{parse_json_object, validate};

const CHARS_PER_TOKEN: usize = 4;
const REPAIR_ECHO_CHARS: usize = 2_000;

#[derive(Debug, Clone)]
pub struct SemanticConfig {
  pub budget: SemanticBudget,
  pub low_confidence_floor: f32,
  pub prices: PriceTable,
  pub call_timeout: Duration,
  pub max_output_tokens: u32,
}

impl Default for SemanticConfig {
  fn default() -> Self {
    Self {
      budget: SemanticBudget::default(),
      low_confidence_floor: 0.5,
      prices: PriceTable::default(),
      call_timeout: Duration::from_secs(120),
      max_output_tokens: 4_096,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
  BudgetExhausted,
  ProviderUnavailable { reason: String },
  EvidenceChanged,
  SourcesUnavailable,
  RebuiltTaskMismatch,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskOutcome {
  Inferred {
    fact: InferredFactId,
    relation: &'static str,
    confidence: Confidence,
    low_confidence: bool,
  },
  NoRelation,
  Skipped {
    reason: SkipReason,
  },
  Failed {
    error: LlmError,
  },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskReport {
  pub kind: TaskKind,
  pub subjects: Vec<EntityId>,
  pub replaces: Option<InferredFactId>,
  pub outcome: TaskOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticRunStatus {
  Completed,
  BudgetExhausted,
  ProviderUnavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticReport {
  pub status: SemanticRunStatus,
  pub tasks: Vec<TaskReport>,
  pub provider_calls: u32,
  pub cache_hits: u32,
  pub usage: TokenUsage,
  pub cost_estimate_usd: Option<f64>,
  pub budget_used: BudgetUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCost {
  Cached,
  Uncached { estimated_input_tokens: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobOrder {
  ByKindAndSubjects,
  AsGiven,
}

pub(crate) struct Job {
  pub task: SemanticTask,
  pub replaces: Option<InferredFactId>,
}

struct Prepared {
  rendered: RenderedPrompt,
  key: ContentHash,
  estimate: u64,
}

struct CallRecord {
  model: String,
  usage: TokenUsage,
  duration: Duration,
  cache: CacheStatus,
  outcome: LlmCallOutcome,
  cache_entry: Option<NewLlmCacheEntry>,
}

enum JobResult {
  Answered(Interpretation),
  Failed(LlmError),
  Skipped(SkipReason),
}

struct Finished {
  index: usize,
  calls: Vec<CallRecord>,
  result: JobResult,
}

enum Rejected {
  Provider(LlmError),
  Invalid { reason: String, text: String },
}

struct CachedAnswer {
  interpretation: Interpretation,
  model: String,
}

pub struct SemanticEngine<'a> {
  similarity: &'a dyn LlmProvider,
  summary: &'a dyn LlmProvider,
  link: &'a dyn LlmProvider,
  store: &'a Store,
  config: SemanticConfig,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
  mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn input_tokens(usage: &TokenUsage) -> u64 {
  usage.input + usage.cache_read + usage.cache_write
}

fn interpret_text(task: &SemanticTask, schema: &Value, text: &str) -> Result<Interpretation, String> {
  let value = parse_json_object(text)?;
  validate(schema, &value)?;
  task.interpret(&value)
}

fn cache_key(rendered: &RenderedPrompt, provider: &str, model: &str) -> ContentHash {
  let mut hasher = blake3::Hasher::new();
  let template_version = rendered.template_version.to_string();
  for part in [
    rendered.kind.as_str(),
    template_version.as_str(),
    provider,
    model,
    rendered.system.as_str(),
    rendered.prompt.as_str(),
  ] {
    hasher.update(&(part.len() as u64).to_le_bytes());
    hasher.update(part.as_bytes());
  }
  ContentHash::from_bytes(*hasher.finalize().as_bytes())
}

fn estimate_tokens(rendered: &RenderedPrompt) -> u64 {
  ((rendered.system.len() + rendered.prompt.len()).div_ceil(CHARS_PER_TOKEN) as u64).max(1)
}

fn superseded_by(tx: &ReadTx<'_>, job: &Job) -> StoreResult<Vec<InferredFactId>> {
  let mut ids = Vec::new();
  for subject in job.task.subjects() {
    let existing = tx.inferred_facts(&InferredQuery {
      subject: Some(subject),
      relation: None,
      freshness: FreshnessPolicy::IncludeStale,
    })?;
    ids.extend(existing.iter().filter(|fact| job.task.supersedes(fact)).map(|fact| fact.id));
  }
  ids.sort();
  ids.dedup();
  Ok(ids)
}

impl<'a> SemanticEngine<'a> {
  pub fn new(provider: &'a dyn LlmProvider, store: &'a Store, config: SemanticConfig) -> Self {
    Self {
      similarity: provider,
      summary: provider,
      link: provider,
      store,
      config,
    }
  }

  pub fn with_task_providers(providers: &'a TaskProviders, store: &'a Store, config: SemanticConfig) -> Self {
    Self {
      similarity: providers.for_kind(TaskKind::ClassifySimilarityPair),
      summary: providers.for_kind(TaskKind::SummarizeCapability),
      link: providers.for_kind(TaskKind::LinkUnresolvedClientCall),
      store,
      config,
    }
  }

  fn provider_for(&self, kind: TaskKind) -> &'a dyn LlmProvider {
    match kind {
      TaskKind::ClassifySimilarityPair => self.similarity,
      TaskKind::SummarizeCapability => self.summary,
      TaskKind::LinkUnresolvedClientCall => self.link,
    }
  }

  pub fn run(&self, run: Option<RunId>, tasks: Vec<SemanticTask>) -> SemanticResult<SemanticReport> {
    self.execute(run, Self::jobs(tasks), JobOrder::ByKindAndSubjects)
  }

  pub fn run_in_priority_order(&self, run: Option<RunId>, tasks: Vec<SemanticTask>) -> SemanticResult<SemanticReport> {
    self.execute(run, Self::jobs(tasks), JobOrder::AsGiven)
  }

  pub fn cost(&self, task: &SemanticTask) -> SemanticResult<TaskCost> {
    let prepared = self.prepare(task);
    Ok(match self.cached(task, &prepared)? {
      Some(_) => TaskCost::Cached,
      None => TaskCost::Uncached {
        estimated_input_tokens: prepared.estimate,
      },
    })
  }

  fn jobs(tasks: Vec<SemanticTask>) -> Vec<Job> {
    tasks.into_iter().map(|task| Job { task, replaces: None }).collect()
  }

  fn prepare(&self, task: &SemanticTask) -> Prepared {
    let rendered = task.render();
    let provider = self.provider_for(rendered.kind);
    Prepared {
      key: cache_key(&rendered, provider.id(), provider.model()),
      estimate: estimate_tokens(&rendered),
      rendered,
    }
  }

  pub(crate) fn execute(&self, run: Option<RunId>, mut jobs: Vec<Job>, order: JobOrder) -> SemanticResult<SemanticReport> {
    if order == JobOrder::ByKindAndSubjects {
      jobs.sort_by_cached_key(|job| (job.task.kind(), job.task.subjects()));
    }
    let mut outcomes: Vec<Option<TaskOutcome>> = jobs.iter().map(|_| None).collect();
    let mut tally = Tally::default();
    let mut unavailable: BTreeMap<TaskKind, String> = BTreeMap::new();
    for job in &jobs {
      let kind = job.task.kind();
      if let (false, Availability::Unavailable { reason }) = (unavailable.contains_key(&kind), self.provider_for(kind).availability()) {
        tracing::info!(task = kind.as_str(), reason = %reason, "LLM provider unavailable; skipping semantic tasks");
        unavailable.insert(kind, reason);
      }
    }

    let prepared: Vec<Prepared> = jobs.iter().map(|job| self.prepare(&job.task)).collect();
    let budget = Mutex::new(BudgetState::new(self.config.budget));
    let mut hits = Vec::new();
    let mut pending = Vec::new();
    for (index, job) in jobs.iter().enumerate() {
      let kind = job.task.kind();
      if let Some(reason) = unavailable.get(&kind) {
        outcomes[index] = Some(TaskOutcome::Skipped {
          reason: SkipReason::ProviderUnavailable { reason: reason.clone() },
        });
        continue;
      }
      let started = Instant::now();
      if let Some(answer) = self.cached(&job.task, &prepared[index])? {
        let record = CallRecord {
          model: answer.model,
          usage: TokenUsage::default(),
          duration: started.elapsed(),
          cache: CacheStatus::Hit,
          outcome: LlmCallOutcome::Ok,
          cache_entry: None,
        };
        hits.push(Finished {
          index,
          calls: vec![record],
          result: JobResult::Answered(answer.interpretation),
        });
      } else if lock(&budget).try_reserve(prepared[index].estimate) {
        pending.push(index);
      } else {
        outcomes[index] = Some(TaskOutcome::Skipped {
          reason: SkipReason::BudgetExhausted,
        });
      }
    }
    self.persist(run, &jobs, hits, &mut outcomes, &mut tally)?;

    for wave in pending.chunks(self.config.budget.max_concurrency.max(1)) {
      let (runnable, blocked): (Vec<usize>, Vec<usize>) = wave.iter().partition(|&&index| !unavailable.contains_key(&jobs[index].task.kind()));
      for index in blocked {
        let reason = unavailable[&jobs[index].task.kind()].clone();
        outcomes[index] = Some(TaskOutcome::Skipped {
          reason: SkipReason::ProviderUnavailable { reason },
        });
      }
      let (jobs_ref, prepared_ref, budget_ref) = (&jobs, &prepared, &budget);
      let finished: Vec<Finished> = thread::scope(|scope| {
        let handles: Vec<_> = runnable
          .iter()
          .map(|&index| scope.spawn(move || self.run_one(index, &jobs_ref[index].task, &prepared_ref[index], budget_ref)))
          .collect();
        handles
          .into_iter()
          .zip(&runnable)
          .map(|(handle, &index)| {
            handle.join().unwrap_or_else(|_| Finished {
              index,
              calls: Vec::new(),
              result: JobResult::Failed(LlmError::Provider("semantic worker panicked".to_owned())),
            })
          })
          .collect()
      });
      for done in &finished {
        if let JobResult::Failed(LlmError::Unavailable(reason)) = &done.result {
          unavailable.entry(jobs[done.index].task.kind()).or_insert_with(|| reason.clone());
        }
      }
      self.persist(run, &jobs, finished, &mut outcomes, &mut tally)?;
    }
    let used = lock(&budget).used();
    Ok(self.report(&jobs, outcomes, tally, used))
  }

  fn cached(&self, task: &SemanticTask, prepared: &Prepared) -> SemanticResult<Option<CachedAnswer>> {
    let Some(entry) = self.store.read(|tx| tx.llm_cache(&prepared.key))? else {
      return Ok(None);
    };
    match interpret_text(task, &prepared.rendered.schema, &entry.response) {
      Ok(interpretation) => Ok(Some(CachedAnswer {
        interpretation,
        model: entry.model,
      })),
      Err(reason) => {
        tracing::warn!(task = task.kind().as_str(), reason = %reason, "ignoring cached response that no longer validates");
        Ok(None)
      }
    }
  }

  fn run_one(&self, index: usize, task: &SemanticTask, prepared: &Prepared, budget: &Mutex<BudgetState>) -> Finished {
    if !lock(budget).has_headroom() {
      return Finished {
        index,
        calls: Vec::new(),
        result: JobResult::Skipped(SkipReason::BudgetExhausted),
      };
    }
    let provider = self.provider_for(task.kind());
    let mut calls = Vec::new();
    let request = self.request(&prepared.rendered, prepared.rendered.prompt.clone());
    let (reason, text) = match self.attempt(provider, task, prepared, &request, prepared.estimate, budget, &mut calls) {
      Ok(interpretation) => {
        return Finished {
          index,
          calls,
          result: JobResult::Answered(interpretation),
        };
      }
      Err(Rejected::Provider(error)) => {
        return Finished {
          index,
          calls,
          result: JobResult::Failed(error),
        };
      }
      Err(Rejected::Invalid { reason, text }) => (reason, text),
    };
    let echoed: String = text.chars().take(REPAIR_ECHO_CHARS).collect();
    let repair_prompt = format!(
      "{}\n\nYour previous response was rejected: {reason}\nPrevious response:\n{echoed}\n\nAnswer again with only a JSON object that fixes this.",
      prepared.rendered.prompt
    );
    let repair_request = self.request(&prepared.rendered, repair_prompt);
    let repair_estimate = ((repair_request.system.len() + repair_request.prompt.len()).div_ceil(CHARS_PER_TOKEN) as u64).max(1);
    if !lock(budget).has_headroom() || !lock(budget).try_reserve(repair_estimate) {
      let error = LlmError::InvalidResponse(format!("{reason} (repair skipped: budget exhausted)"));
      return Finished {
        index,
        calls,
        result: JobResult::Failed(error),
      };
    }
    let result = match self.attempt(provider, task, prepared, &repair_request, repair_estimate, budget, &mut calls) {
      Ok(interpretation) => JobResult::Answered(interpretation),
      Err(Rejected::Provider(error)) => JobResult::Failed(error),
      Err(Rejected::Invalid { reason, .. }) => JobResult::Failed(LlmError::InvalidResponse(reason)),
    };
    Finished { index, calls, result }
  }

  fn request(&self, rendered: &RenderedPrompt, prompt: String) -> LlmRequest {
    LlmRequest {
      system: rendered.system.clone(),
      prompt,
      max_output_tokens: self.config.max_output_tokens,
      json_schema: Some(rendered.schema.clone()),
      timeout: self.config.call_timeout,
    }
  }

  #[allow(clippy::too_many_arguments)]
  fn attempt(
    &self,
    provider: &dyn LlmProvider,
    task: &SemanticTask,
    prepared: &Prepared,
    request: &LlmRequest,
    estimate: u64,
    budget: &Mutex<BudgetState>,
    calls: &mut Vec<CallRecord>,
  ) -> Result<Interpretation, Rejected> {
    let started = Instant::now();
    let reply = provider.complete(request);
    let duration = started.elapsed();
    let response = match reply {
      Ok(response) => response,
      Err(error) => {
        lock(budget).settle(estimate, estimate);
        calls.push(CallRecord {
          model: provider.model().to_owned(),
          usage: TokenUsage::default(),
          duration,
          cache: CacheStatus::Miss,
          outcome: LlmCallOutcome::Failed { error: error.to_string() },
          cache_entry: None,
        });
        return Err(Rejected::Provider(error));
      }
    };
    let actual = input_tokens(&response.usage);
    lock(budget).settle(estimate, if actual == 0 { estimate } else { actual });
    let record = |outcome, cache_entry| CallRecord {
      model: response.model.clone(),
      usage: response.usage,
      duration,
      cache: CacheStatus::Miss,
      outcome,
      cache_entry,
    };
    match interpret_text(task, &prepared.rendered.schema, &response.text) {
      Ok(interpretation) => {
        let entry = NewLlmCacheEntry {
          key: prepared.key,
          provider: provider.id().to_owned(),
          model: response.model.clone(),
          response: response.text.clone(),
          usage: response.usage,
        };
        calls.push(record(LlmCallOutcome::Ok, Some(entry)));
        Ok(interpretation)
      }
      Err(reason) => {
        calls.push(record(
          LlmCallOutcome::Failed {
            error: format!("invalid response: {reason}"),
          },
          None,
        ));
        Err(Rejected::Invalid { reason, text: response.text })
      }
    }
  }

  fn persist(&self, run: Option<RunId>, jobs: &[Job], finished: Vec<Finished>, outcomes: &mut [Option<TaskOutcome>], tally: &mut Tally) -> SemanticResult<()> {
    if finished.is_empty() {
      return Ok(());
    }
    let results = self.store.write(|tx| -> SemanticResult<Vec<TaskOutcome>> {
      let mut results = Vec::new();
      for done in &finished {
        let job = &jobs[done.index];
        let provider = self.provider_for(job.task.kind());
        let mut last_call: Option<LlmCallId> = None;
        for call in &done.calls {
          if let Some(entry) = &call.cache_entry {
            tx.put_llm_cache(entry)?;
          }
          let cost = self
            .config
            .prices
            .estimate_usd(&call.model, &call.usage)
            .or(matches!(call.cache, CacheStatus::Hit).then_some(0.0));
          last_call = Some(tx.record_llm_call(&NewLlmCall {
            run,
            task_kind: job.task.kind().as_str().to_owned(),
            provider: provider.id().to_owned(),
            model: call.model.clone(),
            usage: call.usage,
            cost_estimate_usd: cost,
            cache: call.cache,
            duration_ms: u64::try_from(call.duration.as_millis()).unwrap_or(u64::MAX),
            outcome: call.outcome.clone(),
          })?);
        }
        let outcome = match &done.result {
          JobResult::Skipped(reason) => TaskOutcome::Skipped { reason: reason.clone() },
          JobResult::Failed(error) => TaskOutcome::Failed { error: error.clone() },
          JobResult::Answered(interpretation) => {
            let superseded = superseded_by(tx, job)?;
            for id in superseded.into_iter().chain(job.replaces) {
              tx.delete_inferred_fact(id)?;
            }
            match interpretation {
              Interpretation::NoRelation => TaskOutcome::NoRelation,
              Interpretation::Fact(draft) => {
                let model = done.calls.last().map_or_else(|| provider.model().to_owned(), |call| call.model.clone());
                let low_confidence = draft.confidence.value() < self.config.low_confidence_floor;
                let new_fact = self.new_fact(provider, job.task.kind(), draft, model, low_confidence, run, last_call);
                TaskOutcome::Inferred {
                  fact: tx.insert_inferred_fact(&new_fact)?,
                  relation: draft.relation,
                  confidence: draft.confidence,
                  low_confidence,
                }
              }
            }
          }
        };
        results.push(outcome);
      }
      Ok(results)
    })?;
    for (done, outcome) in finished.iter().zip(results) {
      for call in &done.calls {
        tally.record(call, self.config.prices.estimate_usd(&call.model, &call.usage));
      }
      tracing::info!(task = jobs[done.index].task.kind().as_str(), outcome = ?outcome, "semantic task finished");
      outcomes[done.index] = Some(outcome);
    }
    Ok(())
  }

  #[allow(clippy::too_many_arguments)]
  fn new_fact(
    &self,
    provider: &dyn LlmProvider,
    kind: TaskKind,
    draft: &FactDraft,
    model: String,
    low_confidence: bool,
    run: Option<RunId>,
    llm_call: Option<LlmCallId>,
  ) -> NewInferredFact {
    let mut payload = draft.payload.clone();
    payload.insert(LOW_CONFIDENCE_ATTR.to_owned(), Value::Bool(low_confidence));
    let template_version = payload
      .get("template_version")
      .and_then(Value::as_u64)
      .and_then(|version| u32::try_from(version).ok())
      .unwrap_or(0);
    let producer = Producer::new(format!("semantic.{}", kind.as_str()), template_version);
    let evidence = draft
      .evidence
      .iter()
      .map(|item| Evidence {
        repo: item.source.repo.clone(),
        path: item.source.path.clone(),
        span: item.source.span,
        content_hash: item.source.file_hash,
        producer: producer.clone(),
        note: Some(item.note.clone()),
      })
      .collect();
    NewInferredFact {
      subjects: draft.subjects.clone(),
      relation: draft.relation.to_owned(),
      payload: Value::Object(payload),
      certainty: Certainty::Inferred {
        confidence: draft.confidence,
        provider: provider.id().to_owned(),
        model,
        rationale: draft.rationale.clone(),
      },
      evidence,
      created_run: run,
      llm_call,
    }
  }

  fn report(&self, jobs: &[Job], outcomes: Vec<Option<TaskOutcome>>, tally: Tally, budget_used: BudgetUse) -> SemanticReport {
    let tasks: Vec<TaskReport> = jobs
      .iter()
      .zip(outcomes)
      .map(|(job, outcome)| TaskReport {
        kind: job.task.kind(),
        subjects: job.task.subjects(),
        replaces: job.replaces,
        outcome: outcome.unwrap_or_else(|| TaskOutcome::Failed {
          error: LlmError::Provider("task was never executed".to_owned()),
        }),
      })
      .collect();
    let skipped = |wanted: fn(&SkipReason) -> bool| tasks.iter().any(|task| matches!(&task.outcome, TaskOutcome::Skipped { reason } if wanted(reason)));
    let unavailable = tasks.iter().find_map(|task| match &task.outcome {
      TaskOutcome::Skipped {
        reason: SkipReason::ProviderUnavailable { reason },
      } => Some(reason.clone()),
      _ => None,
    });
    let status = match unavailable {
      Some(reason) => SemanticRunStatus::ProviderUnavailable { reason },
      None if skipped(|reason| matches!(reason, SkipReason::BudgetExhausted)) => SemanticRunStatus::BudgetExhausted,
      None => SemanticRunStatus::Completed,
    };
    SemanticReport {
      status,
      tasks,
      provider_calls: tally.provider_calls,
      cache_hits: tally.cache_hits,
      usage: tally.usage,
      cost_estimate_usd: tally.cost,
      budget_used,
    }
  }

  pub(crate) fn store(&self) -> &Store {
    self.store
  }
}

#[derive(Default)]
struct Tally {
  provider_calls: u32,
  cache_hits: u32,
  usage: TokenUsage,
  cost: Option<f64>,
}

impl Tally {
  fn record(&mut self, call: &CallRecord, cost: Option<f64>) {
    match call.cache {
      CacheStatus::Hit => self.cache_hits += 1,
      CacheStatus::Miss => self.provider_calls += 1,
    }
    self.usage.input += call.usage.input;
    self.usage.output += call.usage.output;
    self.usage.cache_read += call.usage.cache_read;
    self.usage.cache_write += call.usage.cache_write;
    if let Some(cost) = cost {
      self.cost = Some(self.cost.unwrap_or(0.0) + cost);
    }
  }
}
