mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use atlas_core::{Certainty, EntityId, Freshness, LlmProvider};
use atlas_engine::{Engine, EngineOptions, EnrichOptions, EnrichPhase, EnrichPlan, EnrichReport, PhaseReport};
use atlas_semantic::{FakeProvider, FakeStep, ProviderConfig, SemanticBudget, SkipReason, TaskKind, TaskOutcome, TaskProviders};
use atlas_store::{EdgeFilter, FreshnessPolicy, InferredQuery, RunStatus, StoredInferredFact};
use common::{acme_copy, copy_dir, fixtures_root, fresh, open_with, project, repo_config};

const LINK_PROMPT: &str = "client call targets";
const SUMMARY_PROMPT: &str = "Summarise this module";

fn unrelated() -> FakeStep {
  FakeStep::respond(r#"{"verdict":"unrelated","confidence":0.9,"rationale":"different purpose","behavioural_differences":[],"consolidation_risk":"none"}"#)
}

fn link_reply(index: Option<usize>) -> FakeStep {
  let index = index.map_or_else(|| "null".to_owned(), |index| index.to_string());
  FakeStep::respond(format!(r#"{{"endpoint_index":{index},"confidence":0.8,"rationale":"literal segment matches"}}"#))
}

fn summary_reply() -> FakeStep {
  FakeStep::respond(r#"{"summary":"Handles things.","capabilities":["handle things"],"confidence":0.9}"#)
}

fn fake(link: Option<usize>) -> Arc<FakeProvider> {
  Arc::new(
    FakeProvider::new()
      .when_prompt_contains(LINK_PROMPT, link_reply(link))
      .when_prompt_contains(SUMMARY_PROMPT, summary_reply())
      .otherwise(unrelated()),
  )
}

fn with_provider(provider: Arc<dyn LlmProvider>) -> EngineOptions {
  EngineOptions {
    llm_providers: Some(Arc::new(TaskProviders::shared(provider))),
    ..EngineOptions::default()
  }
}

fn offline() -> EngineOptions {
  EngineOptions {
    llm_providers: Some(Arc::new(TaskProviders::from_config(&ProviderConfig::None { reason: "offline".to_owned() }))),
    ..EngineOptions::default()
  }
}

fn budget(calls: u32) -> EnrichOptions {
  EnrichOptions {
    budget: SemanticBudget {
      max_calls_per_run: calls,
      ..SemanticBudget::default()
    },
    summaries: false,
  }
}

fn acme_engine(scratch: &Path, options: EngineOptions) -> (atlas_project::Project, Engine) {
  let project = acme_copy(&scratch.join("tree"));
  let engine = open_with(&project, &scratch.join("home"), options);
  fresh(&engine);
  (project, engine)
}

fn adversarial_engine(scratch: &Path) -> Engine {
  let root = scratch.join("tree/similar-not-equivalent");
  copy_dir(&fixtures_root().join("adversarial/similar-not-equivalent"), &root);
  let project = project("similar-not-equivalent", vec![repo_config("similar-not-equivalent", &root)]);
  let engine = open_with(&project, &scratch.join("home"), EngineOptions::default());
  fresh(&engine);
  engine
}

fn name_of(engine: &Engine, id: EntityId) -> String {
  engine.store().read(|tx| tx.entity(id)).unwrap().unwrap().name
}

fn lexical_names(engine: &Engine, plan: &EnrichPlan) -> Vec<(String, String)> {
  plan
    .lexical
    .iter()
    .map(|candidate| {
      let mut names = [name_of(engine, candidate.left), name_of(engine, candidate.right)];
      names.sort();
      (names[0].clone(), names[1].clone())
    })
    .collect()
}

fn pair(left: &str, right: &str) -> (String, String) {
  (left.to_owned(), right.to_owned())
}

fn phase(report: &EnrichReport, phase: EnrichPhase) -> &PhaseReport {
  report.phase(phase).unwrap_or_else(|| panic!("{phase:?} missing: {report:#?}"))
}

fn link_facts(engine: &Engine) -> Vec<StoredInferredFact> {
  engine
    .store()
    .read(|tx| {
      tx.inferred_facts(&InferredQuery {
        subject: None,
        relation: Some("calls_endpoint".to_owned()),
        freshness: FreshnessPolicy::IncludeStale,
      })
    })
    .unwrap()
}

fn users_index(plan: &EnrichPlan) -> usize {
  plan.links.candidates[0].endpoints.iter().position(|choice| choice.route == "/users/{id}").unwrap()
}

#[test]
fn should_generate_the_behaviour_equivalent_fixture_pairs_as_lexical_candidates() {
  let scratch = tempfile::tempdir().unwrap();
  let (_, acme) = acme_engine(scratch.path(), offline());
  let plan = acme.enrich_plan(&EnrichOptions::default()).unwrap();
  assert_eq!(
    lexical_names(&acme, &plan),
    vec![
      pair("toCents", "to_cents"),
      pair("parseMoney", "to_cents"),
      pair("roundHalfEven", "roundHalfUp"),
      pair("parseMoney", "toCents"),
    ]
  );
  let structural: BTreeSet<(EntityId, EntityId)> = plan.structural.iter().map(|candidate| candidate.ids()).collect();
  assert!(plan.lexical.iter().all(|candidate| !structural.contains(&candidate.ids())));

  let other = tempfile::tempdir().unwrap();
  let adversarial = adversarial_engine(other.path());
  let plan = adversarial.enrich_plan(&EnrichOptions::default()).unwrap();
  assert_eq!(
    lexical_names(&adversarial, &plan),
    vec![
      pair("normalize", "normalize"),
      pair("roundHalfEven", "roundHalfUp"),
      pair("sumLoop", "sumReduce"),
      pair("isEvenBit", "isEvenMod"),
    ]
  );
}

#[test]
fn should_plan_identical_candidates_for_identical_trees() {
  let first = tempfile::tempdir().unwrap();
  let second = tempfile::tempdir().unwrap();
  let (_, left) = acme_engine(first.path(), offline());
  let (_, right) = acme_engine(second.path(), offline());
  let options = EnrichOptions {
    summaries: true,
    ..EnrichOptions::default()
  };
  let (left, right) = (left.enrich_plan(&options).unwrap(), right.enrich_plan(&options).unwrap());
  let keys = |plan: &EnrichPlan| {
    let pairs: Vec<(String, String, f64)> = plan
      .structural
      .iter()
      .chain(&plan.lexical)
      .map(|candidate| (candidate.left_key.clone(), candidate.right_key.clone(), candidate.score))
      .collect();
    let links: Vec<(String, Vec<String>)> = plan
      .links
      .candidates
      .iter()
      .map(|link| (link.call.key.to_string(), link.endpoints.iter().map(|choice| choice.endpoint.key.to_string()).collect()))
      .collect();
    let summaries: Vec<String> = plan.summaries.iter().flatten().map(|summary| summary.file.key.to_string()).collect();
    (pairs, links, summaries)
  };
  assert_eq!(keys(&left), keys(&right));
  assert_eq!((keys(&left).0.len(), keys(&left).1.len(), keys(&left).2.len()), (7, 1, 27));
}

#[test]
fn should_spend_at_most_the_budget_and_give_structural_pairs_priority() {
  let scratch = tempfile::tempdir().unwrap();
  let provider = fake(None);
  let (_, engine) = acme_engine(scratch.path(), with_provider(provider.clone()));

  let report = engine.enrich(&budget(3)).unwrap();

  assert_eq!(provider.call_count(), 3);
  assert_eq!(report.budget_used().calls, 3);
  let spent = |report: &EnrichReport, kind: EnrichPhase| {
    let phase = phase(report, kind);
    (phase.allowance.max_calls_per_run, phase.counts().calls, phase.deferred)
  };
  assert_eq!(spent(&report, EnrichPhase::Links), (0, 0, 1));
  assert_eq!(spent(&report, EnrichPhase::StructuralPairs), (3, 3, 0));
  assert_eq!(spent(&report, EnrichPhase::LexicalPairs), (0, 0, 4));
  assert!(report.phase(EnrichPhase::Summaries).is_none());
  let run = engine.status().unwrap().runs.into_iter().find(|run| run.id == report.run).unwrap();
  assert_eq!(run.status, RunStatus::Ok);
  assert_eq!(run.report.counts["tasks_deferred"], 5);

  let other = tempfile::tempdir().unwrap();
  let roomier = fake(None);
  let (_, engine) = acme_engine(other.path(), with_provider(roomier.clone()));
  let report = engine.enrich(&budget(6)).unwrap();
  assert_eq!(roomier.call_count(), 6);
  assert_eq!(spent(&report, EnrichPhase::Links), (1, 1, 0));
  assert_eq!(spent(&report, EnrichPhase::StructuralPairs), (5, 3, 0));
  assert_eq!(spent(&report, EnrichPhase::LexicalPairs), (2, 2, 2));
}

#[test]
fn should_serve_cached_answers_without_spending_budget_on_the_next_run() {
  let scratch = tempfile::tempdir().unwrap();
  let provider = fake(None);
  let (_, engine) = acme_engine(scratch.path(), with_provider(provider.clone()));
  let first = engine.enrich(&budget(50)).unwrap();
  let calls = provider.call_count();
  assert_eq!(calls, 8, "{first:#?}");

  let second = engine.enrich(&budget(0)).unwrap();

  assert_eq!(provider.call_count(), calls);
  let hits: u32 = second.phases.iter().map(|phase| phase.counts().cache_hits).sum();
  let deferred: u64 = second.phases.iter().map(|phase| phase.deferred).sum();
  assert_eq!((hits, deferred), (8, 0));
}

#[test]
fn should_skip_every_task_kind_without_touching_freshness_when_the_llm_is_unavailable() {
  let scratch = tempfile::tempdir().unwrap();
  let (_, engine) = acme_engine(scratch.path(), offline());

  let report = engine.enrich(&EnrichOptions::default()).unwrap();

  let skipped = |kind: EnrichPhase| phase(&report, kind).counts().skipped_unavailable;
  assert_eq!(
    (skipped(EnrichPhase::Links), skipped(EnrichPhase::StructuralPairs), skipped(EnrichPhase::LexicalPairs)),
    (1, 3, 4)
  );
  assert!(report.phases.iter().all(|phase| phase.counts().calls == 0));
  let after = fresh(&engine);
  assert!(after.is_fresh());
  assert_eq!(after.header().run, None);
  let status = engine.status().unwrap();
  assert_eq!(status.inferred, atlas_engine::InferredCounts::default());
  let run = status.runs.iter().find(|run| run.id == report.run).unwrap();
  assert!(matches!(&run.status, RunStatus::Partial { reason } if reason.contains("unavailable")), "{:?}", run.status);
  assert_eq!(run.report.counts["tasks_skipped_unavailable"], 8);
  assert_eq!(run.report.counts["links.skipped_unavailable"], 1);
  assert_eq!(run.report.counts["tasks_skipped_budget"], 0);
}

#[test]
fn should_only_enrich_small_body_pairs_that_a_name_corroborates() {
  let scratch = tempfile::tempdir().unwrap();
  let repo = scratch.path().join("small");
  fs::create_dir_all(repo.join("src")).unwrap();
  fs::write(repo.join("package.json"), r#"{ "name": "small", "version": "1.0.0" }"#).unwrap();
  let body = "(items: number[]): number {\n  let total = 0;\n  for (const item of items) { if (item > 0) total += item * 2; }\n  return total;\n}\n";
  let source: String = ["sumItems", "sumValues", "quxAlpha", "plaidBeta"]
    .iter()
    .map(|name| format!("export function {name}{body}\n"))
    .collect();
  fs::write(repo.join("src/a.ts"), source).unwrap();
  let project = project("small", vec![repo_config("small", &repo)]);
  let engine = open_with(&project, &scratch.path().join("home"), offline());
  fresh(&engine);
  assert_eq!(engine.similarity().structural_pairs(0.6).len(), 6);

  let plan = engine.enrich_plan(&EnrichOptions::default()).unwrap();

  let names: Vec<(String, String)> = plan
    .structural
    .iter()
    .map(|candidate| (name_of(&engine, candidate.left), name_of(&engine, candidate.right)))
    .collect();
  assert_eq!(names.len(), 1);
  assert!(names[0] == pair("sumItems", "sumValues") || names[0] == pair("sumValues", "sumItems"), "{names:?}");
  assert_eq!(plan.lexical, Vec::new());
}

#[test]
fn should_keep_summaries_off_by_default_and_within_their_share_when_enabled() {
  let scratch = tempfile::tempdir().unwrap();
  let provider = fake(None);
  let (_, engine) = acme_engine(scratch.path(), with_provider(provider.clone()));
  engine.enrich(&budget(50)).unwrap();
  assert!(!provider.requests().iter().any(|request| request.prompt.contains(SUMMARY_PROMPT)));

  let options = EnrichOptions { summaries: true, ..budget(10) };
  let report = engine.enrich(&options).unwrap();

  let summaries = phase(&report, EnrichPhase::Summaries);
  assert_eq!(summaries.allowance.max_calls_per_run, 2);
  assert_eq!((summaries.counts().calls, summaries.counts().facts_written), (2, 2));
  assert_eq!((summaries.candidates, summaries.deferred), (27, 25));
  assert_eq!(provider.requests().iter().filter(|request| request.prompt.contains(SUMMARY_PROMPT)).count(), 2);
}

#[test]
fn should_link_the_unmatched_health_call_as_an_inferred_fact_and_revalidate_it() {
  let scratch = tempfile::tempdir().unwrap();
  let plan = acme_engine(scratch.path(), offline()).1.enrich_plan(&EnrichOptions::default()).unwrap();
  assert_eq!(plan.links.candidates.len(), 1);
  let candidate = &plan.links.candidates[0];
  assert_eq!(candidate.target, "{}/health");
  assert_eq!(
    candidate.endpoints.iter().map(|choice| format!("{} {}", choice.method, choice.route)).collect::<Vec<_>>(),
    ["GET /orders/{id}", "GET /users/{id}", "GET /invoices/{id}"]
  );

  let other = tempfile::tempdir().unwrap();
  let provider = fake(Some(users_index(&plan)));
  let (project, engine) = acme_engine(other.path(), with_provider(provider.clone()));
  let report = engine.enrich(&budget(50)).unwrap();
  assert_eq!(phase(&report, EnrichPhase::Links).counts().facts_written, 1);
  let facts = link_facts(&engine);
  assert_eq!(facts.len(), 1);
  let fact = &facts[0];
  assert!(matches!(&fact.certainty, Certainty::Inferred { .. }));
  assert_eq!(name_of(&engine, fact.subjects[1]), "GET /users/{id}");
  let files: BTreeSet<String> = fact.evidence.iter().map(|evidence| format!("{}:{}", evidence.repo, evidence.path)).collect();
  assert!(
    files.contains("acme-web:src/api/health.ts") && files.contains("acme-api:src/users/users.controller.ts"),
    "{files:?}"
  );

  let health = project.repos.iter().find(|repo| repo.name.as_str() == "acme-web").unwrap().path.join("src/api/health.ts");
  let edited = fs::read_to_string(&health)
    .unwrap()
    .replace("return response.ok;", "return response.ok && response.status === 200;");
  fs::write(&health, edited).unwrap();
  fresh(&engine);
  assert!(matches!(link_facts(&engine)[0].freshness, Freshness::Stale { .. }));

  let deferred = engine.enrich(&budget(0)).unwrap();
  let rerun = &deferred.revalidation.report.rerun.tasks;
  assert_eq!(rerun.len(), 1);
  assert_eq!((rerun[0].kind, rerun[0].replaces), (TaskKind::LinkUnresolvedClientCall, Some(fact.id)));
  assert_eq!(
    rerun[0].outcome,
    TaskOutcome::Skipped {
      reason: SkipReason::BudgetExhausted
    }
  );
  assert_eq!(phase(&deferred, EnrichPhase::Links).deferred, 1);

  engine.enrich(&budget(50)).unwrap();
  let refreshed = link_facts(&engine);
  assert_eq!(refreshed.len(), 1);
  assert!(refreshed[0].freshness.is_current());
}

#[test]
fn should_prune_an_inferred_link_once_a_deterministic_link_exists() {
  let scratch = tempfile::tempdir().unwrap();
  let provider = fake(Some(0));
  let (project, engine) = acme_engine(scratch.path(), with_provider(provider));
  engine.enrich(&budget(50)).unwrap();
  assert_eq!(link_facts(&engine).len(), 1);
  let api = project
    .repos
    .iter()
    .find(|repo| repo.name.as_str() == "acme-api")
    .unwrap()
    .path
    .join("src/health.controller.ts");
  fs::write(
    &api,
    "import { Controller, Get } from '@nestjs/common';\n\n@Controller()\nexport class HealthController {\n  @Get('health')\n  check(): string {\n    return 'ok';\n  }\n}\n",
  )
  .unwrap();
  fresh(&engine);
  assert_eq!(engine.enrich_plan(&EnrichOptions::default()).unwrap().links.candidates, Vec::new());

  let report = engine.enrich(&budget(50)).unwrap();

  assert_eq!(report.revalidation.pruned_links, 1);
  assert_eq!(link_facts(&engine), Vec::new());
}

#[test]
fn should_treat_both_calls_behind_one_merged_link_edge_as_linked() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let web = project.repos.iter().find(|repo| repo.name.as_str() == "acme-web").unwrap().path.clone();
  fs::write(
    web.join("src/api/twice.ts"),
    "import { API_URL } from '../config';\n\nexport async function loadUserTwice(id: string): Promise<boolean> {\n  const plain = await fetch(`${API_URL}/users/${id}`);\n  const verbose = await fetch(`${API_URL}/users/${id}?verbose=1`);\n  return plain.ok && verbose.ok;\n}\n",
  )
  .unwrap();
  let engine = open_with(&project, &scratch.path().join("home"), offline());
  fresh(&engine);

  let (named, linked) = engine
    .store()
    .read(|tx| -> atlas_engine::EngineResult<_> {
      let calls = tx.entities_by_kind(&atlas_core::EntityKind::ApiClientCall, &atlas_store::EntityFilter::default())?;
      let twice: Vec<_> = calls
        .into_iter()
        .filter(|call| call.path.as_ref().is_some_and(|path| path.as_str() == "src/api/twice.ts"))
        .collect();
      let caller = tx.edges_to(
        twice[0].id,
        &EdgeFilter {
          kinds: vec![atlas_core::EdgeKind::MakesRequest],
          ..EdgeFilter::default()
        },
      )?[0]
        .from;
      let links = tx.edges_from(
        caller,
        &EdgeFilter {
          kinds: vec![atlas_core::EdgeKind::CallsEndpoint],
          ..EdgeFilter::default()
        },
      )?;
      let named: Vec<bool> = twice
        .iter()
        .map(|call| {
          links
            .iter()
            .any(|link| link.attrs.get(atlas_link::ATTR_CLIENT_CALL).and_then(serde_json::Value::as_str) == Some(call.key.to_string().as_str()))
        })
        .collect();
      let linked: Vec<bool> = twice.iter().map(|call| atlas_engine::client_call_linked(tx, call)).collect::<Result<_, _>>()?;
      Ok((named, linked))
    })
    .unwrap();

  assert_eq!(
    named.iter().filter(|named| **named).count(),
    1,
    "the fixture must merge both calls into one edge naming one of them"
  );
  assert_eq!(linked, vec![true, true]);
  let plan = engine.enrich_plan(&EnrichOptions::default()).unwrap();
  assert_eq!(
    plan.links.candidates.iter().map(|candidate| candidate.target.clone()).collect::<Vec<_>>(),
    vec!["{}/health".to_owned()]
  );
}
