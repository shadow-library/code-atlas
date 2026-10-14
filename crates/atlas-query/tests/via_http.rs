mod common;

use std::fs;
use std::sync::Arc;

use atlas_core::{Certainty, Confidence, EdgeKind, EntityId};
use atlas_engine::{EngineOptions, EnrichOptions};
use atlas_query::{Caveat, Response, Targeted, TraversalAnswer, TraversalItem, TraversalKind, TraverseRequest};
use atlas_semantic::{FakeProvider, FakeStep, TaskProviders};
use atlas_store::NewInferredFact;
use common::{ACME_REPOS, Fixture, fixture_with};

const ECHO_CONTROLLER: &str = "import { Controller, Get, Param } from '@nestjs/common';\n\nimport { relayEcho } from './echo.relay';\n\n@Controller('echo')\nexport class EchoController {\n  @Get(':id')\n  echo(@Param('id') id: string) {\n    return relayEcho(id);\n  }\n}\n";
const ECHO_BOTH: &str = "import { EchoController } from './echo.controller';\n\nexport async function both(controller: EchoController, id: string) {\n  controller.echo(id);\n  const response = await fetch(`${process.env.SELF_URL}/echo/${id}`);\n  return response.json();\n}\n";
const PING_CONTROLLER: &str = "import { Controller, Get, Param } from '@nestjs/common';\n\n@Controller('ping')\nexport class PingController {\n  @Get('pong')\n  pong() {\n    return 'pong';\n  }\n\n  @Get(':id')\n  echo(@Param('id') id: string) {\n    return id;\n  }\n}\n";
const HEALTH_CLIENT: &str = "export async function checkHealth(): Promise<boolean> {\n  const response = await fetch(`${process.env.API_URL}/health`);\n  return response.ok;\n}\n";
const ECHO_RELAY: &str = "export async function relayEcho(id: string) {\n  const response = await fetch(`${process.env.SELF_URL}/echo/${id}`);\n  return response.json();\n}\n";

fn http(depth: u32, target: &str) -> TraverseRequest {
  TraverseRequest {
    depth,
    via_http: true,
    ..TraverseRequest::new(target)
  }
}

fn answer(response: &Response<Targeted<TraversalAnswer>>) -> &TraversalAnswer {
  match &response.answer {
    Targeted::Found(answer) => answer,
    other => panic!("{other:?}"),
  }
}

fn items(answer: &TraversalAnswer) -> Vec<&TraversalItem> {
  let mut items: Vec<&TraversalItem> = answer.groups.iter().flat_map(|group| &group.items).collect();
  items.sort_by_key(|item| (item.depth, item.entity.short.clone()));
  items
}

fn reached(answer: &TraversalAnswer) -> Vec<(u32, &str)> {
  items(answer).into_iter().map(|item| (item.depth, item.entity.short.as_str())).collect()
}

fn item<'a>(answer: &'a TraversalAnswer, short: &str) -> &'a TraversalItem {
  items(answer).into_iter().find(|item| item.entity.short == short).unwrap()
}

fn via_http_notes(response: &Response<Targeted<TraversalAnswer>>) -> Vec<String> {
  answer(response)
    .caveats
    .iter()
    .filter_map(|caveat| match caveat {
      Caveat::Note { message } if message.starts_with("via_http") => Some(message.clone()),
      _ => None,
    })
    .collect()
}

fn linked_acme() -> Fixture {
  let provider = FakeProvider::new()
    .when_prompt_contains(
      "client call targets",
      FakeStep::respond(r#"{"endpoint_index":1,"confidence":0.8,"rationale":"health probes go to the users service"}"#),
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
  assert_eq!(engine.enrich_plan(&EnrichOptions::default()).unwrap().links.candidates[0].endpoints[1].route, "/users/{id}");
  engine.enrich(&EnrichOptions::default()).unwrap();
  fixture
}

#[test]
fn should_count_the_http_hop_as_one_hop() {
  let fixture = common::acme();

  let one = fixture.query.callers(&http(1, "UsersController.findOne")).unwrap();
  let two = fixture.query.callers(&http(2, "UsersService.findOne")).unwrap();

  assert_eq!(reached(answer(&one)), [(1, "AcmeClient.getUser"), (1, "useMe")]);
  assert_eq!(
    reached(answer(&two)),
    [
      (1, "UsersController.findOne"),
      (1, "UsersService.findOne.should return the user when it exists"),
      (1, "UsersService.findOne.should throw NotFoundException when the user is missing"),
      (1, "UsersService.update"),
      (2, "AcmeClient.getUser"),
      (2, "UsersController.update"),
      (2, "useMe"),
    ]
  );
}

#[test]
fn should_render_the_http_hop_with_the_link_tag_and_mark_the_item_uncertain() {
  let fixture = common::acme();

  let response = fixture.query.callers(&http(1, "UsersController.findOne")).unwrap();

  let get_user = item(answer(&response), "AcmeClient.getUser");
  assert!(get_user.uncertain);
  assert_eq!(
    (get_user.relation.kind.as_str(), get_user.relation.phrase.as_str(), get_user.relation.entity.as_str()),
    ("via_http", "via HTTP GET /users/{id} to", "UsersController.findOne")
  );
  assert_eq!(
    (get_user.relation.certainty.tag.as_str(), get_user.relation.certainty.authoritative),
    ("[heuristic 0.77]", false)
  );
  assert_eq!(
    fixture.text(&response).split_once('\n').unwrap().1,
    "callers of UsersController.findOne method acme-api:src/users/users.controller.ts:13 depth=1 via_http\n\
     acme-sdk (1)\n  \
     AcmeClient.getUser method acme-sdk:src/client.ts:19  via HTTP GET /users/{id} to UsersController.findOne [heuristic 0.77]\n\
     acme-web (1)\n  \
     useMe function acme-web:src/hooks/useMe.ts:7  via HTTP GET /users/{id} to UsersController.findOne [heuristic 0.47]\n"
  );
}

#[test]
fn should_bridge_from_an_endpoint_target_to_its_clients() {
  let fixture = common::acme();

  let response = fixture.query.callers(&http(1, "GET /users/{id}")).unwrap();

  let answer = answer(&response);
  assert_eq!(reached(answer), [(1, "AcmeClient.getUser"), (1, "useMe")]);
  let relation = &item(answer, "AcmeClient.getUser").relation;
  assert_eq!((relation.phrase.as_str(), relation.entity.as_str()), ("via HTTP", "GET /users/{id}"));
}

#[test]
fn should_hide_http_links_below_the_confidence_floor_and_count_them() {
  let fixture = common::acme();
  let request = TraverseRequest {
    min_confidence: Some(0.5),
    ..http(1, "UsersController.findOne")
  };

  let response = fixture.query.callers(&request).unwrap();

  let answer = answer(&response);
  assert_eq!(reached(answer), [(1, "AcmeClient.getUser")]);
  assert_eq!(
    answer.caveats,
    [Caveat::BelowConfidenceFloor {
      hidden: 1,
      fallback_tests: 0,
      floor: 0.5
    }]
  );
}

#[test]
fn should_terminate_on_a_cycle_across_http() {
  let fixture = common::acme();
  fs::write(fixture.repo_root("acme-api").join("src/echo.controller.ts"), ECHO_CONTROLLER).unwrap();
  fs::write(fixture.repo_root("acme-api").join("src/echo.relay.ts"), ECHO_RELAY).unwrap();

  let callers = fixture.query.callers(&http(6, "EchoController.echo")).unwrap();
  let callees = fixture.query.callees(&http(6, "EchoController.echo")).unwrap();

  let callers = answer(&callers);
  assert_eq!(reached(callers), [(1, "relayEcho")]);
  assert_eq!(item(callers, "relayEcho").relation.phrase, "via HTTP GET /echo/{id} to");
  let callees = answer(&callees);
  assert_eq!(reached(callees), [(1, "relayEcho")]);
  assert_eq!(item(callees, "relayEcho").relation.phrase, "called by");
}

#[test]
fn should_cross_an_inferred_link_with_its_inferred_tag_in_both_directions() {
  let fixture = linked_acme();

  let callers = fixture.query.callers(&http(1, "UsersController.findOne")).unwrap();
  let callees = fixture.query.callees(&http(1, "checkHealth")).unwrap();

  let check_health = item(answer(&callers), "checkHealth");
  assert!(check_health.uncertain);
  assert_eq!(check_health.relation.certainty.tag, "[inferred 0.80]");
  assert!(check_health.relation.edge.starts_with("inferred:"), "{}", check_health.relation.edge);
  assert!(
    fixture
      .text(&callers)
      .contains("checkHealth function acme-web:src/api/health.ts:3  via HTTP GET /users/{id} to UsersController.findOne [inferred 0.80]"),
    "{}",
    fixture.text(&callers)
  );
  let handler = item(answer(&callees), "UsersController.findOne");
  assert!(handler.uncertain);
  assert_eq!(
    (handler.relation.phrase.as_str(), handler.relation.entity.as_str(), handler.relation.certainty.tag.as_str()),
    ("via HTTP GET /users/{id} from", "checkHealth", "[inferred 0.80]")
  );
}

#[test]
fn should_hide_a_stale_inferred_link_with_a_caveat() {
  let fixture = linked_acme();
  let path = fixture.repo_root("acme-web").join("src/api/health.ts");
  let edited = fs::read_to_string(&path)
    .unwrap()
    .replace("return response.ok;", "return response.ok && response.status === 200;");
  fs::write(path, edited).unwrap();

  let response = fixture.query.callers(&http(1, "UsersController.findOne")).unwrap();

  let answer = answer(&response);
  assert_eq!(reached(answer), [(1, "AcmeClient.getUser"), (1, "useMe")]);
  assert_eq!(answer.caveats, [Caveat::InferredHidden { stale: 1, low_confidence: 0 }]);
}

#[test]
fn should_explain_why_via_http_found_no_hop() {
  let fixture = common::acme();

  let no_endpoint = fixture.query.callers(&http(2, "toCents")).unwrap();
  let no_client = fixture.query.callers(&http(1, "UsersController.remove")).unwrap();
  let no_request = fixture.query.callees(&http(2, "toCents")).unwrap();
  let unmatched = fixture.query.callees(&http(1, "checkHealth")).unwrap();

  assert_eq!(via_http_notes(&no_endpoint), ["via_http found no HTTP hop: no reached symbol exposes an endpoint"]);
  assert_eq!(
    via_http_notes(&no_client),
    ["via_http found no HTTP hop: no client in this project is linked to DELETE /users/{id} (external clients may exist)"]
  );
  assert_eq!(via_http_notes(&no_request), ["via_http found no HTTP hop: no reached symbol makes an HTTP request"]);
  assert_eq!(
    via_http_notes(&unmatched),
    ["via_http found no HTTP hop: 1 client call made by reached symbols matches no endpoint handled in this project"]
  );
}

fn solo_controller() -> Fixture {
  common::single_repo(
    "solo",
    &[
      ("package.json", r#"{ "name": "solo", "dependencies": { "@nestjs/common": "^10.0.0" } }"#),
      (
        "src/echo.controller.ts",
        &ECHO_CONTROLLER.replace("import { relayEcho } from './echo.relay';\n\n", "").replace("relayEcho(id)", "id"),
      ),
    ],
  )
}

#[test]
fn should_report_missing_links_when_the_index_has_none() {
  let fixture = solo_controller();

  let response = fixture.query.callers(&http(1, "EchoController.echo")).unwrap();

  assert_eq!(
    answer(&response).caveats,
    [Caveat::MissingFacts {
      detail: "via_http found no HTTP hop: no client->endpoint links (calls_endpoint), static or current inferred, exist in this index".to_owned()
    }]
  );
}

#[test]
fn should_not_count_an_inferred_link_whose_subjects_vanished() {
  let fixture = solo_controller();
  let fact = NewInferredFact {
    subjects: vec![EntityId::from_raw(41), EntityId::from_raw(42)],
    relation: "calls_endpoint".to_owned(),
    payload: serde_json::json!({ "rationale": "planted by the test" }),
    certainty: Certainty::Inferred {
      confidence: Confidence::new(0.7).unwrap(),
      provider: "fake".to_owned(),
      model: "fake-model".to_owned(),
      rationale: "planted by the test".to_owned(),
    },
    evidence: Vec::new(),
    created_run: None,
    llm_call: None,
  };
  fixture.query.engine().ensure_fresh().unwrap();
  fixture.query.engine().store().write(|tx| tx.insert_inferred_fact(&fact)).unwrap();

  let response = fixture.query.callers(&http(1, "EchoController.echo")).unwrap();

  assert_eq!(
    answer(&response).caveats,
    [Caveat::MissingFacts {
      detail: "via_http found no HTTP hop: no client->endpoint links (calls_endpoint), static or current inferred, exist in this index".to_owned()
    }]
  );
}

#[test]
fn should_point_past_the_depth_limit_instead_of_claiming_no_hop_exists() {
  let fixture = common::acme();

  let callers = fixture.query.callers(&http(1, "UsersService.findOne")).unwrap();
  let callees = fixture.query.callees(&http(2, "UserCard")).unwrap();

  assert_eq!(
    via_http_notes(&callers),
    ["via_http: HTTP hop beyond depth 1 (UsersController.findOne exposes GET /users/{id}); raise depth to follow it"]
  );
  assert_eq!(
    via_http_notes(&callees),
    ["via_http: HTTP hop beyond depth 2 (AcmeClient.getUser requests GET /users/{id}); raise depth to follow it"]
  );
}

#[test]
fn should_say_the_walk_was_truncated_instead_of_claiming_no_hop_exists() {
  let fixture = common::acme();
  let request = TraverseRequest {
    max_nodes: 1,
    ..http(4, "toCents")
  };

  let response = fixture.query.callers(&request).unwrap();

  assert!(answer(&response).caveats.contains(&Caveat::Truncated { max_nodes: 1 }));
  assert_eq!(
    via_http_notes(&response),
    ["via_http found no HTTP hop among the reached symbols, but the traversal was truncated; raise max_nodes to look further"]
  );
}

#[test]
fn should_prefer_a_direct_call_over_an_http_hop_to_the_same_caller() {
  let fixture = common::acme();
  fs::write(fixture.repo_root("acme-api").join("src/echo.controller.ts"), ECHO_CONTROLLER).unwrap();
  fs::write(fixture.repo_root("acme-api").join("src/echo.relay.ts"), ECHO_RELAY).unwrap();
  fs::write(fixture.repo_root("acme-api").join("src/echo.both.ts"), ECHO_BOTH).unwrap();

  let response = fixture.query.callers(&http(1, "EchoController.echo")).unwrap();

  let callers = answer(&response);
  assert_eq!(reached(callers), [(1, "both"), (1, "relayEcho")]);
  let both = item(callers, "both");
  assert_eq!((both.relation.kind.as_str(), both.relation.certainty.tag.as_str()), ("calls", "[resolved]"));
  assert!(!both.uncertain);
  assert_eq!(item(callers, "relayEcho").relation.kind, "via_http");
  let http_only = TraverseRequest {
    kinds: vec![EdgeKind::Tests],
    ..http(1, "EchoController.echo")
  };
  let http_only = fixture.query.callers(&http_only).unwrap();
  assert_eq!(item(answer(&http_only), "both").relation.kind, "via_http");
}

#[test]
fn should_cross_http_for_dependents_and_dependencies() {
  let fixture = common::acme();

  let dependents = fixture.query.dependents(&http(2, "UsersService.findOne")).unwrap();
  let dependencies = fixture.query.dependencies(&http(3, "UserCard")).unwrap();
  let endpoint = fixture.query.dependents(&http(1, "GET /users/{id}")).unwrap();

  let get_user = item(answer(&dependents), "AcmeClient.getUser");
  assert_eq!((get_user.depth, get_user.relation.kind.as_str()), (2, "via_http"));
  let handler = item(answer(&dependencies), "UsersController.findOne");
  assert_eq!((handler.depth, handler.relation.kind.as_str()), (3, "via_http"));
  let direct = item(answer(&endpoint), "AcmeClient.getUser");
  assert_eq!((direct.depth, direct.relation.kind.as_str()), (1, "calls_endpoint"));
}

#[test]
fn should_not_count_a_hidden_link_twice_when_the_walk_already_steps_over_it() {
  let fixture = common::acme();
  let floored = |via_http: bool| TraverseRequest {
    min_confidence: Some(0.5),
    via_http,
    ..TraverseRequest::new("GET /users/{id}")
  };

  let plain = fixture.query.dependents(&floored(false)).unwrap();
  let bridged = fixture.query.dependents(&floored(true)).unwrap();

  let floor = |response: &Response<Targeted<TraversalAnswer>>| -> Vec<Caveat> {
    answer(response)
      .caveats
      .iter()
      .filter(|caveat| matches!(caveat, Caveat::BelowConfidenceFloor { .. }))
      .cloned()
      .collect()
  };
  assert_eq!(
    floor(&plain),
    [Caveat::BelowConfidenceFloor {
      hidden: 1,
      fallback_tests: 0,
      floor: 0.5
    }]
  );
  assert_eq!(floor(&bridged), floor(&plain));
}

#[test]
fn should_leave_the_json_answer_unchanged_without_via_http() {
  let fixture = common::acme();

  let response = fixture.query.callers(&TraverseRequest::new("UsersService.findOne")).unwrap();

  let json = serde_json::to_value(&response).unwrap();
  assert_eq!(json["answer"]["found"].get("via_http"), None, "{json}");
  assert!(items(answer(&response)).iter().all(|item| item.relation.kind != "via_http"));
}

#[test]
fn should_keep_the_dispatch_macro_dynamic_and_unresolved_caveats() {
  let cases = [
    (common::acme(), TraversalKind::Callers, "InvoiceStore::insert"),
    (common::acme(), TraversalKind::Callers, "PgInvoiceStore::insert"),
    (common::fixture("adversarial", &["macro-heavy-rust"]), TraversalKind::Callees, "Point::manhattan"),
    (common::fixture("adversarial", &["dynamic-ts"]), TraversalKind::Callers, "stop"),
  ];

  for (fixture, kind, target) in cases {
    let plain = fixture.query.traverse(kind, &TraverseRequest::new(target)).unwrap();
    let bridged = fixture.query.traverse(kind, &http(1, target)).unwrap();

    let plain = &answer(&plain).caveats;
    let kept: Vec<&Caveat> = answer(&bridged)
      .caveats
      .iter()
      .filter(|caveat| !matches!(caveat, Caveat::Note { message } if message.starts_with("via_http")))
      .collect();
    assert!(!plain.is_empty(), "{target}");
    assert_eq!(kept, plain.iter().collect::<Vec<_>>(), "{target}");
  }
}

fn floored(depth: u32, target: &str) -> TraverseRequest {
  TraverseRequest {
    min_confidence: Some(0.8),
    ..http(depth, target)
  }
}

#[test]
fn should_count_current_inferred_links_that_miss_the_traversed_symbols() {
  let provider = FakeProvider::new()
    .when_prompt_contains(
      "client call targets",
      FakeStep::respond(r#"{"endpoint_index":0,"confidence":0.8,"rationale":"health probes hit the ping service"}"#),
    )
    .otherwise(FakeStep::respond(
      r#"{"verdict":"unrelated","confidence":0.9,"rationale":"different purpose","behavioural_differences":[],"consolidation_risk":"none"}"#,
    ));
  let options = EngineOptions {
    llm_providers: Some(Arc::new(TaskProviders::shared(Arc::new(provider)))),
    ..EngineOptions::default()
  };
  let fixture = common::single_repo_with(
    "solo",
    &[
      ("package.json", r#"{ "name": "solo", "dependencies": { "@nestjs/common": "^10.0.0" } }"#),
      ("src/ping.controller.ts", PING_CONTROLLER),
      ("src/health.ts", HEALTH_CLIENT),
    ],
    options,
  );
  let engine = fixture.query.engine();
  engine.ensure_fresh().unwrap();
  assert_eq!(engine.enrich_plan(&EnrichOptions::default()).unwrap().links.candidates[0].endpoints[0].route, "/ping/{id}");
  engine.enrich(&EnrichOptions::default()).unwrap();

  let missed = fixture.query.callers(&http(1, "PingController.pong")).unwrap();
  let crossed = fixture.query.callers(&http(1, "PingController.echo")).unwrap();

  assert_eq!(
    answer(&missed).caveats,
    [Caveat::MissingFacts {
      detail:
        "via_http found no HTTP hop: no static client->endpoint links (calls_endpoint) exist in this index, and none of its current inferred links (1) reach the traversed symbols"
          .to_owned()
    }]
  );
  let check_health = item(answer(&crossed), "checkHealth");
  assert_eq!(check_health.relation.certainty.tag, "[inferred 0.80]");
}

#[test]
fn should_say_http_links_exist_below_the_floor_instead_of_claiming_none_exist() {
  let fixture = common::acme();

  let endpoint = fixture.query.dependents(&floored(1, "GET /users/{id}")).unwrap();
  let client = fixture.query.dependencies(&floored(2, "AcmeClient.getUser")).unwrap();
  let frontier = fixture.query.callers(&floored(1, "UsersService.findOne")).unwrap();

  let below = "via_http found no HTTP hop above the confidence floor: 2 links on HTTP paths are below 0.80 (lower min_confidence to follow them)";
  assert_eq!(via_http_notes(&endpoint), [below]);
  assert_eq!(via_http_notes(&client), [below]);
  assert_eq!(via_http_notes(&frontier), [below]);
  let plain = TraverseRequest {
    via_http: false,
    ..floored(1, "GET /users/{id}")
  };
  let plain = fixture.query.dependents(&plain).unwrap();
  assert_eq!(
    answer(&plain).caveats,
    [Caveat::BelowConfidenceFloor {
      hidden: 2,
      fallback_tests: 0,
      floor: 0.8
    }]
  );
  assert_eq!(
    answer(&endpoint).caveats[0],
    Caveat::BelowConfidenceFloor {
      hidden: 2,
      fallback_tests: 0,
      floor: 0.8
    }
  );
}

#[test]
fn should_not_point_past_the_depth_limit_at_an_endpoint_without_clients() {
  let fixture = common::acme();

  let response = fixture.query.callers(&http(1, "UsersService.remove")).unwrap();

  assert_eq!(
    via_http_notes(&response),
    ["via_http found no HTTP hop: no client in this project is linked to DELETE /users/{id} (external clients may exist)"]
  );
}
