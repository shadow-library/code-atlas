mod common;

use std::fs;
use std::sync::Arc;

use atlas_core::EntityKind;
use atlas_engine::{EngineOptions, EnrichOptions};
use atlas_query::{ApiAnswer, ApiRequest, Caveat, EndpointView, ExplainAnswer, ExplainRequest, ImpactRequest, Targeted, UnusedRequest};
use atlas_semantic::{FakeProvider, FakeStep, SemanticBudget, TaskProviders};
use common::{ACME_REPOS, Fixture, fixture_with};

const USERS_INDEX: usize = 1;
const INVOICES_INDEX: usize = 2;
const HEALTH_CONTROLLER: &str =
  "import { Controller, Get } from '@nestjs/common';\n\n@Controller()\nexport class HealthController {\n  @Get('health')\n  check(): string {\n    return 'ok';\n  }\n}\n";
const STATS_CONTROLLER: &str =
  "import { Controller, Get } from '@nestjs/common';\n\n@Controller('aa')\nexport class StatsController {\n  @Get('stats')\n  stats(): string {\n    return 'ok';\n  }\n}\n";
const PING_CONTROLLER: &str = "import { Controller, Get, Param } from '@nestjs/common';\n\n@Controller('ping')\nexport class PingController {\n  @Get(':id')\n  ping(@Param('id') id: string): string {\n    return id;\n  }\n}\n";

fn linked_acme() -> Fixture {
  linked_acme_to(USERS_INDEX, "/users/{id}")
}

fn linked_acme_to(index: usize, route: &str) -> Fixture {
  let provider = FakeProvider::new()
    .when_prompt_contains(
      "client call targets",
      FakeStep::respond(format!(
        r#"{{"endpoint_index":{index},"confidence":0.8,"rationale":"health probes go to the users service"}}"#
      )),
    )
    .otherwise(FakeStep::respond(
      r#"{"verdict":"unrelated","confidence":0.9,"rationale":"different purpose","behavioural_differences":[],"consolidation_risk":"none"}"#,
    ));
  let options = EngineOptions {
    llm_providers: Some(Arc::new(TaskProviders::shared(Arc::new(provider)))),
    ..EngineOptions::default()
  };
  let fixture = fixture_with("acme", &ACME_REPOS, options);
  let engine = fixture.query.engine();
  engine.ensure_fresh().unwrap();
  let plan = engine.enrich_plan(&EnrichOptions::default()).unwrap();
  assert_eq!(plan.links.candidates[0].endpoints[index].route, route);
  engine.enrich(&EnrichOptions::default()).unwrap();
  fixture
}

fn users_endpoint(answer: &ApiAnswer) -> &EndpointView {
  answer.endpoints.items.iter().find(|view| view.method == "GET" && view.route == "/users/{id}").unwrap()
}

fn add_api_file(fixture: &Fixture, name: &str, source: &str) {
  fs::write(fixture.repo_root("acme-api").join("src").join(name), source).unwrap();
}

fn edit_health_call(fixture: &Fixture) {
  let path = fixture.repo_root("acme-web").join("src/api/health.ts");
  let edited = fs::read_to_string(&path)
    .unwrap()
    .replace("return response.ok;", "return response.ok && response.status === 200;");
  fs::write(path, edited).unwrap();
}

#[test]
fn should_show_an_inferred_link_only_as_a_tagged_inferred_consumer() {
  let fixture = linked_acme();

  let response = fixture.query.api(&ApiRequest::default()).unwrap();

  let users = users_endpoint(&response.answer);
  assert!(users.consumers.iter().all(|consumer| consumer.consumer.entity.short != "checkHealth"));
  assert_eq!(users.inferred_consumers.len(), 1);
  let inferred = &users.inferred_consumers[0];
  assert_eq!(inferred.consumer.entity.short, "checkHealth");
  assert_eq!(inferred.consumer.relation.certainty.tag, "[inferred 0.80]");
  assert!(!inferred.consumer.relation.certainty.authoritative);
  let unmatched = response.answer.unmatched_calls.items.iter().find(|call| call.caller.entity.short == "checkHealth").unwrap();
  assert_eq!(unmatched.reason, "no endpoint in the project matches this call");
  let link = unmatched.inferred_link.as_ref().unwrap();
  assert_eq!(link.endpoint.short, "GET /users/{id}");
  assert_eq!(link.relation.phrase, "likely calls endpoint");
  let text = fixture.text(&response);
  assert!(
    text.contains("inferred consumer checkHealth function acme-web:src/api/health.ts:3 via GET {}/health"),
    "{text}"
  );
  assert!(
    text.contains("likely calls endpoint GET /users/{id} [inferred 0.80]: health probes go to the users service"),
    "{text}"
  );
}

#[test]
fn should_list_inferred_consumers_separately_in_impact_and_explain() {
  let fixture = linked_acme();

  let impact = fixture.query.impact(&ImpactRequest::new(["GET /users/{id}"])).unwrap();

  let Targeted::Found(answer) = &impact.answer else { panic!("{:?}", impact.answer) };
  assert!(answer.consumers.items.iter().all(|item| item.entity.short != "checkHealth"));
  let item = answer.inferred_consumers.items.iter().find(|item| item.entity.short == "checkHealth").unwrap();
  assert!(item.uncertain);
  assert!(item.why.iter().any(|hop| hop.certainty.tag == "[inferred 0.80]" && hop.kind == "calls_endpoint"));
  assert!(
    fixture
      .text(&impact)
      .contains("inferred endpoint consumers (LLM-proposed links, not verified) (1, 1 uncertain)")
  );

  let explained = fixture.query.explain(&ExplainRequest::new("GET /users/{id}")).unwrap();
  let Targeted::Found(ExplainAnswer::Entity { inferred, .. }) = &explained.answer else {
    panic!("{:?}", explained.answer)
  };
  assert_eq!(inferred.len(), 1);
  assert_eq!((inferred[0].relation.as_str(), inferred[0].certainty.tag.as_str()), ("calls_endpoint", "[inferred 0.80]"));
}

#[test]
fn should_hide_an_inferred_link_once_the_client_file_changes() {
  let fixture = linked_acme();
  edit_health_call(&fixture);

  let response = fixture.query.api(&ApiRequest::default()).unwrap();

  assert_eq!(users_endpoint(&response.answer).inferred_consumers, Vec::new());
  let unmatched = response.answer.unmatched_calls.items.iter().find(|call| call.caller.entity.short == "checkHealth").unwrap();
  assert!(unmatched.inferred_link.is_none());
  assert!(
    response.answer.caveats.contains(&Caveat::InferredHidden { stale: 1, low_confidence: 0 }),
    "{:?}",
    response.answer.caveats
  );
}

#[test]
fn should_hide_an_inferred_link_once_a_new_endpoint_enters_the_candidate_set() {
  let fixture = linked_acme();
  add_api_file(&fixture, "ping.controller.ts", PING_CONTROLLER);

  let response = fixture.query.api(&ApiRequest::default()).unwrap();

  let ping = response.answer.endpoints.items.iter().find(|view| view.route == "/ping/{id}").unwrap();
  assert_eq!(ping.consumers.len(), 0);
  assert_eq!(users_endpoint(&response.answer).inferred_consumers, Vec::new());
  let unmatched = response.answer.unmatched_calls.items.iter().find(|call| call.caller.entity.short == "checkHealth").unwrap();
  assert_eq!(unmatched.inferred_link, None);
  assert!(
    response.answer.caveats.contains(&Caveat::InferredHidden { stale: 1, low_confidence: 0 }),
    "{:?}",
    response.answer.caveats
  );

  let options = EnrichOptions {
    budget: SemanticBudget {
      max_calls_per_run: 0,
      ..SemanticBudget::default()
    },
    summaries: false,
  };
  let report = fixture.query.engine().enrich(&options).unwrap();
  assert_eq!((report.revalidation.outdated_links, report.revalidation.due), (1, 1));
}

#[test]
fn should_drop_an_inferred_link_at_query_time_once_a_deterministic_link_exists() {
  let fixture = linked_acme();
  add_api_file(&fixture, "health.controller.ts", HEALTH_CONTROLLER);

  let response = fixture.query.api(&ApiRequest::default()).unwrap();

  assert_eq!(users_endpoint(&response.answer).inferred_consumers, Vec::new());
  let health = response.answer.endpoints.items.iter().find(|view| view.route == "/health").unwrap();
  assert_eq!(
    health.consumers.iter().map(|consumer| consumer.consumer.entity.short.as_str()).collect::<Vec<_>>(),
    ["checkHealth"]
  );
  assert_eq!(response.answer.unmatched_calls.total, 0);
  assert!(!response.answer.caveats.iter().any(|caveat| matches!(caveat, Caveat::InferredHidden { .. })));
  let facts = fixture.query.engine().store().read(|tx| tx.inferred_facts(&Default::default())).unwrap();
  assert_eq!(facts.len(), 1);
}

#[test]
fn should_keep_an_unconsumed_endpoint_and_cite_its_inferred_consumer() {
  let fixture = linked_acme_to(INVOICES_INDEX, "/invoices/{id}");
  let request = UnusedRequest {
    kinds: vec![EntityKind::ApiEndpoint],
    ..UnusedRequest::default()
  };

  let response = fixture.query.unused(&request).unwrap();

  let candidate = response
    .answer
    .possibly_unused
    .items
    .iter()
    .find(|candidate| candidate.entity.short == "GET /invoices/{id}")
    .unwrap();
  assert_eq!(
    candidate.evidence,
    [
      "no detected consumers in project",
      "external clients outside the project may exist",
      "likely consumer checkHealth via GET {}/health [inferred 0.80]"
    ]
  );
  assert!(fixture.text(&response).contains("likely consumer checkHealth via GET {}/health [inferred 0.80]"));
}

#[test]
fn should_keep_an_inferred_link_when_a_new_endpoint_shares_no_path_segment() {
  let fixture = linked_acme();
  add_api_file(&fixture, "stats.controller.ts", STATS_CONTROLLER);

  let response = fixture.query.api(&ApiRequest::default()).unwrap();

  assert!(response.answer.endpoints.items.iter().any(|view| view.route == "/aa/stats"));
  let users = users_endpoint(&response.answer);
  assert_eq!(users.inferred_consumers.len(), 1);
  assert_eq!(users.inferred_consumers[0].consumer.relation.certainty.tag, "[inferred 0.80]");
  assert!(!response.answer.caveats.iter().any(|caveat| matches!(caveat, Caveat::InferredHidden { .. })));
}
