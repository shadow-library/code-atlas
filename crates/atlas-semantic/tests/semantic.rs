use std::time::Duration;

use atlas_core::{Certainty, ContentHash, EdgeKind, EntityId, Freshness, LlmError, RelPath, RepoName, RunId, Span, TokenUsage};
use atlas_semantic::{
  BudgetUse, CapabilityInput, ClientCall, ClientCallLink, EndpointCandidate, FakeProvider, FakeStep, SemanticBudget, SemanticConfig, SemanticEngine, SemanticReport,
  SemanticRunStatus, SemanticTask, SimilarityPair, SkipReason, SourceRef, SymbolDigest, SymbolSnippet, TaskCost, TaskKind, TaskOutcome, included_in_default_results, inferred_edge,
  is_low_confidence,
};
use atlas_store::{CacheStatus, FreshnessPolicy, InferredQuery, LlmCallOutcome, Store, StoredInferredFact};

fn source(path: &str, seed: &str) -> SourceRef {
  SourceRef {
    repo: RepoName::new("acme").unwrap(),
    path: RelPath::new(path).unwrap(),
    span: Span::from_zero_based_rows(0, 40, 0, 0, 4, 1),
    file_hash: ContentHash::of(seed.as_bytes()),
  }
}

fn symbol(id: u64, name: &str, seed: &str) -> SymbolSnippet {
  SymbolSnippet {
    entity: EntityId::from_raw(id),
    key: format!("ts:acme:{name}"),
    signature: Some(format!("function {name}(amount: string): number")),
    snippet: format!("function {name}(amount: string) {{\n  return Number(amount) * 100;\n}}"),
    source: source(&format!("src/{name}.ts"), seed),
    callers: Some(2),
    test_names: vec![],
  }
}

fn edited(id: u64, name: &str, seed: &str) -> SymbolSnippet {
  let mut edited = symbol(id, name, seed);
  edited.snippet.push_str("\n// edited");
  edited
}

fn pair_of(a: SymbolSnippet, b: SymbolSnippet) -> SemanticTask {
  SemanticTask::ClassifySimilarityPair(SimilarityPair::new(a, b).unwrap())
}

fn pair(a: u64, b: u64) -> SemanticTask {
  pair_of(symbol(a, &format!("f{a}"), &format!("v1-{a}")), symbol(b, &format!("f{b}"), &format!("v1-{b}")))
}

fn summary_task(id: u64) -> SemanticTask {
  let digest = SymbolDigest {
    signature: "export function toCents(amount: string): number".into(),
    doc: Some("Parses decimal strings".into()),
  };
  SemanticTask::SummarizeCapability(
    CapabilityInput::new(
      EntityId::from_raw(id),
      "src/money.ts",
      vec![source("src/money.ts", "money")],
      vec![digest],
      vec!["node:util".into()],
    )
    .unwrap(),
  )
}

fn link_task(call_site: u64) -> SemanticTask {
  let call = ClientCall {
    call_site: EntityId::from_raw(call_site),
    method: Some("GET".into()),
    url_pattern: "`/users/${id}`".into(),
    snippet: "fetch(`/users/${id}`)".into(),
    source: source("src/client.ts", "client"),
  };
  let candidate = |entity: u64, route: &str| EndpointCandidate {
    entity: EntityId::from_raw(entity),
    method: "GET".into(),
    route: route.into(),
    handler_signature: Some("getUser(id: string)".into()),
    source: source("src/users.controller.ts", "controller"),
  };
  SemanticTask::LinkUnresolvedClientCall(ClientCallLink::new(call, vec![candidate(900, "/orders/:id"), candidate(901, "/users/:id")]).unwrap())
}

fn similarity_json(verdict: &str, confidence: f64, differences: &[&str]) -> String {
  serde_json::json!({
    "verdict": verdict,
    "confidence": confidence,
    "rationale": "both scale decimal strings to cents",
    "behavioural_differences": differences,
    "consolidation_risk": "callers may rely on rounding",
  })
  .to_string()
}

fn config() -> SemanticConfig {
  let mut config = SemanticConfig::default();
  config.budget.max_concurrency = 1;
  config.call_timeout = Duration::from_secs(5);
  config
}

fn start_run(store: &Store) -> RunId {
  store.write(|tx| tx.begin_run("semantic-test")).unwrap()
}

fn all_facts(store: &Store) -> Vec<StoredInferredFact> {
  store
    .read(|tx| {
      tx.inferred_facts(&InferredQuery {
        subject: None,
        relation: None,
        freshness: FreshnessPolicy::IncludeStale,
      })
    })
    .unwrap()
}

fn ledger(store: &Store, run: RunId) -> Vec<atlas_store::LlmCallRecord> {
  store.read(|tx| tx.llm_calls(run)).unwrap()
}

fn run_tasks(provider: &FakeProvider, store: &Store, config: SemanticConfig, tasks: Vec<SemanticTask>) -> (RunId, SemanticReport) {
  let run = start_run(store);
  let report = SemanticEngine::new(provider, store, config).run(Some(run), tasks).unwrap();
  (run, report)
}

fn outcomes(report: &SemanticReport) -> Vec<&TaskOutcome> {
  report.tasks.iter().map(|task| &task.outcome).collect()
}

#[test]
fn should_serve_repeated_tasks_from_the_cache_and_log_the_hit() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("similar_not_equivalent", 0.8, &["rounding differs"])));
  let (_, first) = run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  let (run, second) = run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  assert_eq!((first.provider_calls, first.cache_hits), (1, 0));
  assert_eq!((second.provider_calls, second.cache_hits), (0, 1));
  assert_eq!(provider.call_count(), 1);
  let calls = ledger(&store, run);
  assert_eq!(calls.len(), 1);
  assert_eq!(calls[0].call.cache, CacheStatus::Hit);
  assert_eq!(calls[0].call.usage, TokenUsage::default());
  assert_eq!(calls[0].call.outcome, LlmCallOutcome::Ok);
  assert_eq!(all_facts(&store).len(), 1);
}

#[test]
fn should_share_cache_entries_between_both_orderings_of_a_pair() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("similar_not_equivalent", 0.8, &["x"])));
  let (a, b) = (symbol(1, "alpha", "a"), symbol(2, "beta", "b"));
  run_tasks(&provider, &store, config(), vec![pair_of(a.clone(), b.clone())]);
  let (_, report) = run_tasks(&provider, &store, config(), vec![pair_of(b, a)]);
  assert_eq!(report.cache_hits, 1);
  assert_eq!(provider.call_count(), 1);
}

#[test]
fn should_miss_the_cache_when_the_snippet_changes() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("unrelated", 0.9, &[])));
  run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  let mut changed = symbol(2, "f2", "v2-2");
  changed.snippet.push_str("\n// edited");
  run_tasks(&provider, &store, config(), vec![pair_of(symbol(1, "f1", "v1-1"), changed)]);
  assert_eq!(provider.call_count(), 2);
}

#[test]
fn should_record_every_call_in_the_ledger_with_usage_cost_and_outcome() {
  let store = Store::open_in_memory().unwrap();
  let usage = TokenUsage {
    input: 1_000_000,
    output: 100_000,
    cache_read: 0,
    cache_write: 0,
  };
  let provider = FakeProvider::new()
    .with_identity("fake", "claude-sonnet-4-5")
    .otherwise(FakeStep::respond(similarity_json("equivalent", 0.9, &[])).with_usage(usage));
  let (run, report) = run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  let calls = ledger(&store, run);
  assert_eq!(calls.len(), 1);
  let call = &calls[0].call;
  assert_eq!(
    (call.task_kind.as_str(), call.provider.as_str(), call.model.as_str()),
    ("classify_similarity_pair", "fake", "claude-sonnet-4-5")
  );
  assert_eq!((call.usage, call.cache, call.outcome.clone()), (usage, CacheStatus::Miss, LlmCallOutcome::Ok));
  assert_eq!(call.cost_estimate_usd, Some(4.5));
  assert_eq!(report.usage, usage);
  assert_eq!(report.cost_estimate_usd, Some(4.5));
}

#[test]
fn should_store_an_inferred_edge_fact_with_provenance_and_evidence_hashes() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("equivalent", 0.9, &[])));
  let task = pair(1, 2);
  let sources: Vec<SourceRef> = task.sources().into_iter().cloned().collect();
  let (run, report) = run_tasks(&provider, &store, config(), vec![task]);
  let TaskOutcome::Inferred {
    fact: id,
    relation,
    low_confidence,
    ..
  } = report.tasks[0].outcome.clone()
  else {
    panic!("expected a fact, got {:?}", report.tasks[0].outcome)
  };
  assert_eq!((relation, low_confidence), ("equivalent_to", false));
  let stored = store.read(|tx| tx.inferred_fact(id)).unwrap().unwrap();
  assert_eq!(stored.subjects, vec![EntityId::from_raw(1), EntityId::from_raw(2)]);
  assert_eq!(stored.freshness, Freshness::Current);
  assert_eq!(stored.created_run, Some(run));
  assert!(stored.llm_call.is_some());
  let Certainty::Inferred {
    provider,
    model,
    rationale,
    confidence,
  } = &stored.certainty
  else {
    panic!("fact must be inferred")
  };
  assert_eq!(
    (provider.as_str(), model.as_str(), rationale.as_str()),
    ("fake", "fake-model", "both scale decimal strings to cents")
  );
  assert!((confidence.value() - 0.9).abs() < 1e-6);
  let hashes: Vec<ContentHash> = stored.evidence.iter().map(|item| item.content_hash).collect();
  assert_eq!(hashes, sources.iter().map(|source| source.file_hash).collect::<Vec<_>>());
  assert!(
    stored
      .evidence
      .iter()
      .all(|item| item.note.as_deref().is_some_and(|note| note.starts_with("snippet blake3:")))
  );
  assert_eq!(stored.payload["verdict"], "equivalent");
  assert_eq!(stored.payload["consolidation_risk"], "callers may rely on rounding");
  let edge = inferred_edge(&stored).unwrap();
  assert_eq!(
    (edge.fact.kind.clone(), edge.fact.from, edge.fact.to),
    (EdgeKind::EquivalentTo, EntityId::from_raw(1), EntityId::from_raw(2))
  );
  assert_eq!(edge.tag(), "[inferred 0.90]");
}

#[test]
fn should_flag_low_confidence_facts_and_exclude_them_from_default_results() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new()
    .when_prompt_contains("f1", FakeStep::respond(similarity_json("similar_not_equivalent", 0.3, &["null handling"])))
    .otherwise(FakeStep::respond(similarity_json("similar_not_equivalent", 0.5, &["null handling"])));
  run_tasks(&provider, &store, config(), vec![pair(1, 2), pair(3, 4)]);
  let facts = all_facts(&store);
  let low = facts.iter().find(|fact| fact.subjects[0] == EntityId::from_raw(1)).unwrap();
  let at_floor = facts.iter().find(|fact| fact.subjects[0] == EntityId::from_raw(3)).unwrap();
  assert_eq!(low.payload["low_confidence"], true);
  assert_eq!((is_low_confidence(low), included_in_default_results(low)), (true, false));
  assert_eq!((is_low_confidence(at_floor), included_in_default_results(at_floor)), (false, true));
}

#[test]
fn should_create_no_fact_for_unrelated_pairs() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("unrelated", 0.95, &[])));
  let (_, report) = run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  assert_eq!(outcomes(&report), [&TaskOutcome::NoRelation]);
  assert!(all_facts(&store).is_empty());
}

#[test]
fn should_skip_remaining_tasks_when_the_call_budget_runs_out() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("unrelated", 0.9, &[])));
  let mut config = config();
  config.budget.max_calls_per_run = 1;
  let (_, report) = run_tasks(&provider, &store, config, vec![pair(1, 2), pair(3, 4), pair(5, 6)]);
  assert_eq!(report.status, SemanticRunStatus::BudgetExhausted);
  assert_eq!(report.tasks[0].outcome, TaskOutcome::NoRelation);
  assert_eq!(
    report.tasks[1].outcome,
    TaskOutcome::Skipped {
      reason: SkipReason::BudgetExhausted
    }
  );
  assert_eq!(
    report.tasks[2].outcome,
    TaskOutcome::Skipped {
      reason: SkipReason::BudgetExhausted
    }
  );
  assert_eq!(provider.call_count(), 1);
}

#[test]
fn should_reserve_the_budget_in_the_given_order_when_asked_to() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("unrelated", 0.9, &[])));
  let mut config = config();
  config.budget.max_calls_per_run = 1;
  let run = start_run(&store);
  let report = SemanticEngine::new(&provider, &store, config)
    .run_in_priority_order(Some(run), vec![pair(5, 6), pair(1, 2)])
    .unwrap();
  let subjects: Vec<Vec<EntityId>> = report.tasks.iter().map(|task| task.subjects.clone()).collect();
  assert_eq!(
    subjects,
    vec![vec![EntityId::from_raw(5), EntityId::from_raw(6)], vec![EntityId::from_raw(1), EntityId::from_raw(2)]]
  );
  assert_eq!(report.tasks[0].outcome, TaskOutcome::NoRelation);
  assert_eq!(
    report.tasks[1].outcome,
    TaskOutcome::Skipped {
      reason: SkipReason::BudgetExhausted
    }
  );
  assert_eq!(report.budget_used.calls, 1);
}

#[test]
fn should_price_a_task_as_cached_only_after_a_valid_answer_was_stored() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("unrelated", 0.9, &[])));
  let engine = SemanticEngine::new(&provider, &store, config());
  let TaskCost::Uncached { estimated_input_tokens } = engine.cost(&pair(1, 2)).unwrap() else {
    panic!("expected a cache miss")
  };
  assert!(estimated_input_tokens > 0);
  run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  assert_eq!(engine.cost(&pair(1, 2)).unwrap(), TaskCost::Cached);
  assert_eq!(provider.call_count(), 1);
}

#[test]
fn should_split_and_subtract_budgets_without_underflow() {
  let total = SemanticBudget {
    max_calls_per_run: 12,
    max_input_tokens_per_run: 1_000,
    max_concurrency: 2,
  };
  assert_eq!(
    total.share(1, 5),
    SemanticBudget {
      max_calls_per_run: 2,
      max_input_tokens_per_run: 200,
      max_concurrency: 2
    }
  );
  let used = BudgetUse { calls: 10, input_tokens: 400 }.plus(BudgetUse { calls: 5, input_tokens: 100 });
  assert_eq!(
    total.remaining_after(used),
    SemanticBudget {
      max_calls_per_run: 0,
      max_input_tokens_per_run: 500,
      max_concurrency: 2
    }
  );
  assert_eq!(total.capped_by(total.share(2, 5)).max_calls_per_run, 4);
  assert_eq!(SemanticBudget { max_calls_per_run: 4, ..total }.share(1, 5).max_calls_per_run, 0);
}

#[test]
fn should_skip_tasks_that_would_exceed_the_input_token_budget() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("unrelated", 0.9, &[])));
  let mut config = config();
  config.budget.max_input_tokens_per_run = 10;
  let (_, report) = run_tasks(&provider, &store, config, vec![pair(1, 2)]);
  assert_eq!(report.status, SemanticRunStatus::BudgetExhausted);
  assert_eq!(provider.call_count(), 0);
}

#[test]
fn should_still_serve_cache_hits_after_the_budget_is_exhausted() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("unrelated", 0.9, &[])));
  run_tasks(&provider, &store, config(), vec![pair(5, 6)]);
  let mut config = config();
  config.budget.max_calls_per_run = 1;
  let (_, report) = run_tasks(&provider, &store, config, vec![pair(1, 2), pair(3, 4), pair(5, 6)]);
  assert_eq!(report.cache_hits, 1);
  assert_eq!(report.provider_calls, 1);
  assert_eq!(report.tasks[2].outcome, TaskOutcome::NoRelation);
  assert_eq!(
    report.tasks[1].outcome,
    TaskOutcome::Skipped {
      reason: SkipReason::BudgetExhausted
    }
  );
}

#[test]
fn should_repair_invalid_json_once_and_store_the_repaired_answer() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new()
    .then(FakeStep::respond("I think they match!"))
    .then(FakeStep::respond(similarity_json("equivalent", 0.9, &[])));
  let (run, report) = run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  assert!(matches!(report.tasks[0].outcome, TaskOutcome::Inferred { .. }));
  assert_eq!(provider.call_count(), 2);
  let repair = &provider.requests()[1];
  assert!(repair.prompt.contains("Your previous response was rejected: response is not valid JSON"));
  assert!(repair.prompt.contains("I think they match!"));
  let calls = ledger(&store, run);
  assert!(matches!(&calls[0].call.outcome, LlmCallOutcome::Failed { error } if error.starts_with("invalid response:")));
  assert_eq!(calls[1].call.outcome, LlmCallOutcome::Ok);
  let (_, again) = run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  assert_eq!(again.cache_hits, 1);
}

#[test]
fn should_fail_with_invalid_response_and_create_no_fact_after_a_failed_repair() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new()
    .then(FakeStep::respond("not json"))
    .then(FakeStep::respond(r#"{"verdict":"equivalent"}"#));
  let (run, report) = run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  assert_eq!(
    report.tasks[0].outcome,
    TaskOutcome::Failed {
      error: LlmError::InvalidResponse(r#"$ is missing required property "confidence""#.to_owned())
    }
  );
  assert_eq!(provider.call_count(), 2);
  assert!(all_facts(&store).is_empty());
  let calls = ledger(&store, run);
  assert_eq!(calls.len(), 2);
  assert!(calls.iter().all(|call| matches!(call.call.outcome, LlmCallOutcome::Failed { .. })));
}

#[test]
fn should_count_the_repair_attempt_against_the_budget() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond("not json"));
  let mut config = config();
  config.budget.max_calls_per_run = 1;
  let (_, report) = run_tasks(&provider, &store, config, vec![pair(1, 2)]);
  assert_eq!(provider.call_count(), 1);
  let TaskOutcome::Failed {
    error: LlmError::InvalidResponse(reason),
  } = &report.tasks[0].outcome
  else {
    panic!("unexpected {:?}", report.tasks[0].outcome)
  };
  assert!(reason.ends_with("(repair skipped: budget exhausted)"));
}

#[test]
fn should_reject_an_equivalent_verdict_that_lists_behavioural_differences() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("equivalent", 0.9, &["rounds half up"])));
  let (_, report) = run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  assert!(matches!(&report.tasks[0].outcome, TaskOutcome::Failed { error: LlmError::InvalidResponse(reason) } if reason.contains("contradicts")));
  assert!(all_facts(&store).is_empty());
}

#[test]
fn should_reject_out_of_range_confidence() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("equivalent", 1.5, &[])));
  let (_, report) = run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  assert_eq!(
    report.tasks[0].outcome,
    TaskOutcome::Failed {
      error: LlmError::InvalidResponse("$.confidence must be at most 1, got 1.5".to_owned())
    }
  );
}

#[test]
fn should_skip_every_task_without_errors_when_the_provider_is_unavailable() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().unavailable("claude binary missing");
  let (run, report) = run_tasks(&provider, &store, config(), vec![pair(1, 2), summary_task(7), link_task(8)]);
  let reason = SkipReason::ProviderUnavailable {
    reason: "claude binary missing".to_owned(),
  };
  assert_eq!(
    report.status,
    SemanticRunStatus::ProviderUnavailable {
      reason: "claude binary missing".to_owned()
    }
  );
  assert!(report.tasks.iter().all(|task| task.outcome == TaskOutcome::Skipped { reason: reason.clone() }));
  assert_eq!(provider.call_count(), 0);
  assert!(ledger(&store, run).is_empty());
  assert!(all_facts(&store).is_empty());
}

#[test]
fn should_stop_calling_after_the_provider_becomes_unavailable_mid_run() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::fail(LlmError::Unavailable("binary vanished".into())));
  let (_, report) = run_tasks(&provider, &store, config(), vec![pair(1, 2), pair(3, 4), pair(5, 6)]);
  assert_eq!(provider.call_count(), 1);
  assert_eq!(
    report.status,
    SemanticRunStatus::ProviderUnavailable {
      reason: "binary vanished".to_owned()
    }
  );
  assert!(matches!(report.tasks[0].outcome, TaskOutcome::Failed { error: LlmError::Unavailable(_) }));
  assert!(matches!(report.tasks[1].outcome, TaskOutcome::Skipped { .. }));
}

#[test]
fn should_record_a_timeout_as_a_failed_call_without_a_fact() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("equivalent", 0.9, &[])).with_delay(Duration::from_millis(300)));
  let mut config = config();
  config.call_timeout = Duration::from_millis(30);
  let (run, report) = run_tasks(&provider, &store, config, vec![pair(1, 2)]);
  assert_eq!(
    report.tasks[0].outcome,
    TaskOutcome::Failed {
      error: LlmError::Timeout(Duration::from_millis(30))
    }
  );
  assert!(all_facts(&store).is_empty());
  let calls = ledger(&store, run);
  assert_eq!(calls.len(), 1);
  assert!(matches!(&calls[0].call.outcome, LlmCallOutcome::Failed { error } if error.contains("timed out")));
}

#[test]
fn should_not_cache_failed_calls() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new()
    .then(FakeStep::fail(LlmError::Provider("boom".into())))
    .otherwise(FakeStep::respond(similarity_json("unrelated", 0.9, &[])));
  let (_, first) = run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  let (_, second) = run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  assert_eq!(
    first.tasks[0].outcome,
    TaskOutcome::Failed {
      error: LlmError::Provider("boom".into())
    }
  );
  assert_eq!(second.tasks[0].outcome, TaskOutcome::NoRelation);
}

#[test]
fn should_process_tasks_in_a_deterministic_priority_order_regardless_of_input_order() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("unrelated", 0.9, &[])));
  let summary = r#"{"summary":"Converts money.","capabilities":["convert money"],"confidence":0.9}"#;
  let link = r#"{"endpoint_index":null,"confidence":0.9,"rationale":"nothing fits"}"#;
  let provider = provider
    .when_prompt_contains("Summarise this module", FakeStep::respond(summary))
    .when_prompt_contains("client call targets", FakeStep::respond(link));
  let forward = vec![summary_task(7), pair(3, 4), link_task(8), pair(1, 2)];
  let reversed: Vec<SemanticTask> = vec![pair(1, 2), link_task(8), pair(3, 4), summary_task(7)];
  let (_, first) = run_tasks(&provider, &store, config(), forward);
  let other_store = Store::open_in_memory().unwrap();
  let (_, second) = run_tasks(&provider, &other_store, config(), reversed);
  let order = |report: &SemanticReport| report.tasks.iter().map(|task| (task.kind, task.subjects.clone())).collect::<Vec<_>>();
  let expected = vec![
    (TaskKind::LinkUnresolvedClientCall, vec![EntityId::from_raw(8)]),
    (TaskKind::ClassifySimilarityPair, vec![EntityId::from_raw(1), EntityId::from_raw(2)]),
    (TaskKind::ClassifySimilarityPair, vec![EntityId::from_raw(3), EntityId::from_raw(4)]),
    (TaskKind::SummarizeCapability, vec![EntityId::from_raw(7)]),
  ];
  assert_eq!(order(&first), expected);
  assert_eq!(order(&second), expected);
  let requests = provider.requests();
  let markers = ["client call targets", "ts:acme:f1\n", "ts:acme:f3\n", "Summarise this module"];
  for (request, marker) in requests.iter().take(4).zip(markers) {
    assert!(request.prompt.contains(marker), "expected {marker:?} in {:?}", request.prompt);
  }
}

#[test]
fn should_handle_concurrent_waves_and_keep_outcomes_attached_to_their_tasks() {
  let store = Store::open_in_memory().unwrap();
  let mut provider = FakeProvider::new();
  for index in 0..8u64 {
    let verdict = if index % 2 == 0 { "equivalent" } else { "unrelated" };
    provider = provider.when_prompt_contains(
      format!("ts:acme:f{}\n", index * 2 + 1),
      FakeStep::respond(similarity_json(verdict, 0.9, &[])).with_delay(Duration::from_millis(5)),
    );
  }
  let mut config = config();
  config.budget.max_concurrency = 4;
  let tasks = (0..8u64).map(|index| pair(index * 2 + 1, index * 2 + 2)).collect();
  let (_, report) = run_tasks(&provider, &store, config, tasks);
  assert_eq!(provider.call_count(), 8);
  for (index, task) in report.tasks.iter().enumerate() {
    let inferred = matches!(task.outcome, TaskOutcome::Inferred { .. });
    assert_eq!(inferred, index % 2 == 0, "task {index}: {:?}", task.outcome);
  }
  assert_eq!(all_facts(&store).len(), 4);
}

#[test]
fn should_replace_the_previous_fact_for_the_same_pair() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new()
    .then(FakeStep::respond(similarity_json("similar_not_equivalent", 0.8, &["x"])))
    .then(FakeStep::respond(similarity_json("equivalent", 0.9, &[])));
  run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  let mut changed = symbol(1, "f1", "v1-1");
  changed.snippet.push_str("\n// edit");
  run_tasks(&provider, &store, config(), vec![pair_of(changed, symbol(2, "f2", "v1-2"))]);
  let facts = all_facts(&store);
  assert_eq!(facts.len(), 1);
  assert_eq!(facts[0].relation, "equivalent_to");
}

#[test]
fn should_store_a_capability_summary_as_an_inferred_fact() {
  let store = Store::open_in_memory().unwrap();
  let reply = r#"{"summary":"Converts decimal money strings to integer cents.","capabilities":["parse money","convert to cents"],"confidence":0.85}"#;
  let provider = FakeProvider::new().otherwise(FakeStep::respond(reply));
  let (_, report) = run_tasks(&provider, &store, config(), vec![summary_task(7)]);
  assert!(matches!(
    report.tasks[0].outcome,
    TaskOutcome::Inferred {
      relation: "capability_summary",
      ..
    }
  ));
  let fact = &all_facts(&store)[0];
  assert_eq!(fact.subjects, vec![EntityId::from_raw(7)]);
  assert_eq!(fact.payload["summary"], "Converts decimal money strings to integer cents.");
  assert_eq!(fact.payload["capabilities"], serde_json::json!(["parse money", "convert to cents"]));
  assert!(matches!(&fact.certainty, Certainty::Inferred { rationale, .. } if rationale == "Converts decimal money strings to integer cents."));
  assert!(inferred_edge(fact).is_none());
}

#[test]
fn should_link_a_client_call_to_the_chosen_endpoint_with_both_files_as_evidence() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(r#"{"endpoint_index":1,"confidence":0.9,"rationale":"literal segment users matches"}"#));
  let (_, report) = run_tasks(&provider, &store, config(), vec![link_task(8)]);
  assert!(matches!(report.tasks[0].outcome, TaskOutcome::Inferred { relation: "calls_endpoint", .. }));
  let fact = &all_facts(&store)[0];
  let edge = inferred_edge(fact).unwrap();
  assert_eq!(
    (edge.fact.kind.clone(), edge.fact.from, edge.fact.to),
    (EdgeKind::CallsEndpoint, EntityId::from_raw(8), EntityId::from_raw(901))
  );
  assert_eq!(edge.fact.attrs["route"], "/users/:id");
  assert_eq!(
    edge.fact.attrs["candidates"],
    serde_json::json!([EntityId::from_raw(900).to_string(), EntityId::from_raw(901).to_string()])
  );
  let notes: Vec<&str> = fact.evidence.iter().map(|item| item.note.as_deref().unwrap_or_default()).collect();
  assert_eq!(notes.len(), 3);
  assert!(notes[0].starts_with("snippet blake3:"));
  assert_eq!(notes[1], "endpoint candidate 0 GET /orders/:id");
  assert_eq!(notes[2], "endpoint candidate 1 GET /users/:id (chosen)");
}

#[test]
fn should_create_no_link_for_a_null_endpoint_and_reject_out_of_range_indexes() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new().otherwise(FakeStep::respond(r#"{"endpoint_index":null,"confidence":0.8,"rationale":"none fit"}"#));
  let (_, null_report) = run_tasks(&provider, &store, config(), vec![link_task(8)]);
  assert_eq!(null_report.tasks[0].outcome, TaskOutcome::NoRelation);
  let provider = FakeProvider::new().otherwise(FakeStep::respond(r#"{"endpoint_index":5,"confidence":0.8,"rationale":"guess"}"#));
  let store = Store::open_in_memory().unwrap();
  let (_, bad_report) = run_tasks(&provider, &store, config(), vec![link_task(9)]);
  assert_eq!(
    bad_report.tasks[0].outcome,
    TaskOutcome::Failed {
      error: LlmError::InvalidResponse("endpoint_index 5 is outside the 2 candidates".to_owned())
    }
  );
  assert!(all_facts(&store).is_empty());
}

fn current_hash_from(changed: &'static [(&'static str, &'static str)]) -> impl Fn(&RepoName, &RelPath) -> Option<ContentHash> {
  move |_, path| {
    let seed = changed.iter().find(|(file, _)| *file == path.as_str()).map(|(_, seed)| *seed)?;
    Some(ContentHash::of(seed.as_bytes()))
  }
}

fn stored_pair_fact(store: &Store) -> StoredInferredFact {
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("similar_not_equivalent", 0.8, &["rounding differs"])));
  run_tasks(&provider, store, config(), vec![pair(1, 2)]);
  all_facts(store).remove(0)
}

#[test]
fn should_leave_current_facts_untouched_when_evidence_hashes_match() {
  let store = Store::open_in_memory().unwrap();
  let fact = stored_pair_fact(&store);
  let provider = FakeProvider::new();
  let engine = SemanticEngine::new(&provider, &store, config());
  let report = engine
    .revalidate(
      None,
      std::slice::from_ref(&fact),
      current_hash_from(&[("src/f1.ts", "v1-1"), ("src/f2.ts", "v1-2")]),
      |_| None,
    )
    .unwrap();
  assert_eq!(report.unchanged, vec![fact.id]);
  assert!(report.skipped.is_empty() && report.rerun.tasks.is_empty());
  assert_eq!(all_facts(&store)[0].freshness, Freshness::Current);
  assert_eq!(provider.call_count(), 0);
}

#[test]
fn should_mark_facts_stale_when_evidence_changes_and_keep_them_stale_if_the_sources_moved_again() {
  let store = Store::open_in_memory().unwrap();
  let fact = stored_pair_fact(&store);
  let provider = FakeProvider::new();
  let engine = SemanticEngine::new(&provider, &store, config());
  let hashes = current_hash_from(&[("src/f1.ts", "v2-1"), ("src/f2.ts", "v1-2")]);
  let report = engine.revalidate(None, std::slice::from_ref(&fact), hashes, |_| Some(pair(1, 2))).unwrap();
  assert_eq!(report.skipped, vec![(fact.id, SkipReason::EvidenceChanged)]);
  assert_eq!(provider.call_count(), 0);
  let stored = all_facts(&store);
  assert_eq!(stored.len(), 1);
  assert_eq!(
    stored[0].freshness,
    Freshness::Stale {
      reason: "acme:src/f1.ts changed".to_owned()
    }
  );
  assert!(!included_in_default_results(&stored[0]));
}

#[test]
fn should_mark_facts_stale_when_a_file_disappears_and_keep_them_if_nothing_can_be_rebuilt() {
  let store = Store::open_in_memory().unwrap();
  let fact = stored_pair_fact(&store);
  let provider = FakeProvider::new();
  let engine = SemanticEngine::new(&provider, &store, config());
  let report = engine
    .revalidate(None, std::slice::from_ref(&fact), current_hash_from(&[("src/f2.ts", "v1-2")]), |_| None)
    .unwrap();
  assert_eq!(report.skipped, vec![(fact.id, SkipReason::SourcesUnavailable)]);
  assert_eq!(
    all_facts(&store)[0].freshness,
    Freshness::Stale {
      reason: "acme:src/f1.ts no longer exists".to_owned()
    }
  );
}

#[test]
fn should_replace_a_stale_fact_when_the_rerun_sees_current_evidence() {
  let store = Store::open_in_memory().unwrap();
  let fact = stored_pair_fact(&store);
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("equivalent", 0.9, &[])));
  let engine = SemanticEngine::new(&provider, &store, config());
  let rebuilt = pair_of(edited(1, "f1", "v2-1"), symbol(2, "f2", "v1-2"));
  let report = engine
    .revalidate(
      None,
      std::slice::from_ref(&fact),
      current_hash_from(&[("src/f1.ts", "v2-1"), ("src/f2.ts", "v1-2")]),
      |_| Some(rebuilt.clone()),
    )
    .unwrap();
  assert_eq!(report.rerun.tasks[0].replaces, Some(fact.id));
  let facts = all_facts(&store);
  assert_eq!(facts.len(), 1);
  assert_ne!(facts[0].id, fact.id);
  assert_eq!((facts[0].relation.as_str(), &facts[0].freshness), ("equivalent_to", &Freshness::Current));
  assert!(facts[0].evidence.iter().any(|item| item.content_hash == ContentHash::of(b"v2-1")));
  assert!(provider.requests()[0].prompt.contains("ts:acme:f1"));
}

#[test]
fn should_drop_the_old_claim_when_the_rerun_finds_the_pair_unrelated() {
  let store = Store::open_in_memory().unwrap();
  let fact = stored_pair_fact(&store);
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("unrelated", 0.9, &[])));
  let engine = SemanticEngine::new(&provider, &store, config());
  let rebuilt = pair_of(edited(1, "f1", "v2-1"), symbol(2, "f2", "v1-2"));
  let report = engine
    .revalidate(
      None,
      std::slice::from_ref(&fact),
      current_hash_from(&[("src/f1.ts", "v2-1"), ("src/f2.ts", "v1-2")]),
      |_| Some(rebuilt.clone()),
    )
    .unwrap();
  assert_eq!(report.rerun.tasks[0].outcome, TaskOutcome::NoRelation);
  assert!(all_facts(&store).is_empty());
}

#[test]
fn should_keep_the_stale_fact_when_the_rerun_fails() {
  let store = Store::open_in_memory().unwrap();
  let fact = stored_pair_fact(&store);
  let provider = FakeProvider::new().otherwise(FakeStep::fail(LlmError::Provider("down".into())));
  let engine = SemanticEngine::new(&provider, &store, config());
  let rebuilt = pair_of(edited(1, "f1", "v2-1"), symbol(2, "f2", "v1-2"));
  engine
    .revalidate(
      None,
      std::slice::from_ref(&fact),
      current_hash_from(&[("src/f1.ts", "v2-1"), ("src/f2.ts", "v1-2")]),
      |_| Some(rebuilt.clone()),
    )
    .unwrap();
  let facts = all_facts(&store);
  assert_eq!(facts.len(), 1);
  assert_eq!(facts[0].id, fact.id);
  assert!(matches!(facts[0].freshness, Freshness::Stale { .. }));
}

#[test]
fn should_rerun_facts_that_need_revalidation_after_a_rename() {
  let store = Store::open_in_memory().unwrap();
  let fact = stored_pair_fact(&store);
  store.write(|tx| tx.mark_needs_revalidation(fact.id, "f1 renamed")).unwrap();
  let flagged = all_facts(&store).remove(0);
  assert_eq!(flagged.freshness, Freshness::NeedsRevalidation { reason: "f1 renamed".to_owned() });
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("similar_not_equivalent", 0.7, &["renamed helper differs"])));
  let engine = SemanticEngine::new(&provider, &store, config());
  let renamed = pair_of(symbol(1, "renamedF1", "v1-1"), symbol(2, "f2", "v1-2"));
  let hashes = current_hash_from(&[("src/renamedF1.ts", "v1-1"), ("src/f1.ts", "v1-1"), ("src/f2.ts", "v1-2")]);
  engine.revalidate(None, std::slice::from_ref(&flagged), hashes, |_| Some(renamed.clone())).unwrap();
  let facts = all_facts(&store);
  assert_eq!(facts.len(), 1);
  assert_eq!(facts[0].freshness, Freshness::Current);
  assert!(provider.requests()[0].prompt.contains("renamedF1"));
  assert!(facts[0].evidence.iter().any(|item| item.path.as_str() == "src/renamedF1.ts"));
}

#[test]
fn should_attribute_cache_hit_facts_to_the_model_that_produced_the_response() {
  let store = Store::open_in_memory().unwrap();
  let provider = FakeProvider::new()
    .with_identity("fake", "claude-sonnet-5-5")
    .otherwise(FakeStep::respond(similarity_json("equivalent", 0.9, &[])).with_model("fake-model-served"));
  run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  let (run, report) = run_tasks(&provider, &store, config(), vec![pair(1, 2)]);
  assert_eq!(report.cache_hits, 1);
  let facts = all_facts(&store);
  let Certainty::Inferred { model, .. } = &facts[0].certainty else {
    panic!("fact must be inferred")
  };
  assert_eq!(model, "fake-model-served");
  assert_eq!(ledger(&store, run)[0].call.model, "fake-model-served");
}

#[test]
fn should_skip_later_tasks_once_actual_input_usage_exceeds_the_budget() {
  let store = Store::open_in_memory().unwrap();
  let heavy = TokenUsage {
    input: 50_000,
    output: 10,
    cache_read: 0,
    cache_write: 0,
  };
  let provider = FakeProvider::new().otherwise(FakeStep::respond(similarity_json("unrelated", 0.9, &[])).with_usage(heavy));
  let mut config = config();
  config.budget.max_input_tokens_per_run = 5_000;
  let (_, report) = run_tasks(&provider, &store, config, vec![pair(1, 2), pair(3, 4), pair(5, 6)]);
  assert_eq!(provider.call_count(), 1);
  assert_eq!(report.status, SemanticRunStatus::BudgetExhausted);
  assert_eq!(report.tasks[0].outcome, TaskOutcome::NoRelation);
  assert_eq!(
    report.tasks[2].outcome,
    TaskOutcome::Skipped {
      reason: SkipReason::BudgetExhausted
    }
  );
}
