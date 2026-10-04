mod common;

use std::collections::BTreeSet;

use atlas_query::{
  ApiRequest, ContextAnswer, ContextRequest, EntityRef, ImpactAnswer, ImpactRequest, SearchRequest, SimilarRequest, SymbolRequest, Targeted, TestsForRequest, TraverseRequest,
  UnusedClass, UnusedRequest, UsageState,
};
use common::truth::{any_matches, expected, label, matches, section};
use toml::Value;

const LINK_DEPENDENT: [(&str, &str); 2] = [("acme-sdk", "AcmeClient.getOrder"), ("acme-billing", "AcmeApiClient::get_order")];

fn found<T: std::fmt::Debug>(targeted: Targeted<T>) -> T {
  match targeted {
    Targeted::Found(answer) => answer,
    other => panic!("expected a resolved target, got {other:?}"),
  }
}

fn impacted(answer: &ImpactAnswer) -> Vec<&EntityRef> {
  [&answer.direct, &answer.transitive, &answer.endpoints, &answer.consumers, &answer.tests, &answer.config]
    .into_iter()
    .flat_map(|group| group.items.iter().map(|item| &item.entity))
    .collect()
}

fn link_dependent(selector: &Value) -> bool {
  LINK_DEPENDENT
    .iter()
    .any(|(repo, symbol)| selector.get("repo").and_then(Value::as_str) == Some(repo) && selector.get("symbol").and_then(Value::as_str) == Some(symbol))
}

fn symbol_selector(change: &Value) -> String {
  change.get("symbol").and_then(Value::as_str).unwrap().to_owned()
}

#[test]
fn should_find_every_cents_conversion_when_searching_by_capability() {
  let fixture = common::acme();

  let answer = fixture.query.search(&SearchRequest::new("convert currency string to cents")).unwrap().answer;

  let hits: Vec<&EntityRef> = answer.exact_name.iter().chain(&answer.strong).chain(&answer.related).map(|hit| &hit.entity).collect();
  for (repo, path, symbol) in [
    ("acme-sdk", "src/money.ts", "toCents"),
    ("acme-web", "src/utils/currency.ts", "parseMoney"),
    ("acme-billing", "src/money.rs", "to_cents"),
  ] {
    let selector: Value = toml::from_str(&format!("repo = {repo:?}\npath = {path:?}\nsymbol = {symbol:?}")).unwrap();
    assert!(any_matches(hits.iter().copied(), &selector), "search missed {symbol}: {hits:#?}");
  }
}

#[test]
fn should_match_blast_radius_ground_truth_without_cross_repo_http_links() {
  let fixture = common::acme();
  let truth = expected("acme");

  for radius in section(&truth, "blast_radius") {
    let change = &radius["change"];
    let answer = found(fixture.query.impact(&ImpactRequest::new([symbol_selector(change)])).unwrap().answer);
    let entities = impacted(&answer);
    for selector in section(radius, "must_include").iter().filter(|selector| !link_dependent(selector)) {
      assert!(any_matches(entities.iter().copied(), selector), "impact of {} missed {}", label(change), label(selector));
    }
    for selector in section(radius, "must_not_include") {
      assert!(
        !any_matches(entities.iter().copied(), selector),
        "impact of {} wrongly includes {}",
        label(change),
        label(selector)
      );
    }
    let repos: BTreeSet<&str> = entities.iter().map(|entity| entity.repo()).collect();
    assert!(repos.iter().all(|repo| repo.starts_with("acme-")), "foreign repos in impact: {repos:?}");
  }
}

#[test]
fn should_flag_uncertain_impact_items_instead_of_dropping_them() {
  let fixture = common::acme();

  let answer = found(fixture.query.impact(&ImpactRequest::new(["toCents"])).unwrap().answer);

  let order_form = answer
    .direct
    .items
    .iter()
    .find(|item| item.entity.short == "OrderForm")
    .expect("OrderForm must be impacted");
  assert!(order_form.uncertain);
  assert_eq!(order_form.why[0].certainty.tag, "[heuristic 0.85]");
  assert_eq!(order_form.why[0].phrase, "possibly calls");
  let collapsed = found(
    fixture
      .query
      .impact(&ImpactRequest {
        limit: 0,
        ..ImpactRequest::new(["toCents"])
      })
      .unwrap()
      .answer,
  );
  assert!(collapsed.direct.items.iter().all(|item| item.uncertain));
  assert_eq!(collapsed.direct.items.len(), answer.direct.uncertain);
  assert_eq!(collapsed.direct.total, answer.direct.total);
  assert_eq!(collapsed.direct.collapsed, answer.direct.total - answer.direct.uncertain);
}

#[test]
fn should_reach_sdk_and_web_consumers_of_users_service_find_one_through_calls_endpoint() {
  let fixture = common::acme();

  let answer = found(fixture.query.impact(&ImpactRequest::new(["UsersService.findOne"])).unwrap().answer);

  let get_user = answer
    .consumers
    .items
    .iter()
    .find(|item| item.entity.short == "AcmeClient.getUser")
    .expect("AcmeClient.getUser consumer");
  assert!(get_user.uncertain);
  assert!(get_user.why.iter().any(|hop| hop.kind == "calls_endpoint" && hop.certainty.class == "heuristic"));
  let consumers: BTreeSet<&str> = answer.consumers.items.iter().map(|item| item.entity.short.as_str()).collect();
  assert!(consumers.contains("useUser"), "{consumers:?}");
  assert!(answer.repos.iter().any(|repo| repo.repo == "acme-web"));
}

#[test]
fn should_match_link_dependent_blast_radius_ground_truth() {
  let fixture = common::acme();
  let truth = expected("acme");

  for radius in section(&truth, "blast_radius") {
    let change = &radius["change"];
    let answer = found(fixture.query.impact(&ImpactRequest::new([symbol_selector(change)])).unwrap().answer);
    let entities = impacted(&answer);
    for selector in section(radius, "must_include").iter().filter(|selector| link_dependent(selector)) {
      assert!(any_matches(entities.iter().copied(), selector), "impact of {} missed {}", label(change), label(selector));
    }
  }
}

#[test]
fn should_describe_endpoints_with_handlers_guards_and_permissions() {
  let fixture = common::acme();

  let answer = fixture.query.api(&ApiRequest::default()).unwrap().answer;

  assert_eq!(answer.endpoints.total, 10);
  let users = answer.endpoints.items.iter().find(|view| view.method == "GET" && view.route == "/users/{id}").unwrap();
  assert_eq!(users.handler.as_ref().unwrap().entity.short, "UsersController.findOne");
  let guards: Vec<&str> = users.guards.iter().map(|guard| guard.guard.entity.short.as_str()).collect();
  assert_eq!(guards, ["JwtAuthGuard", "PermissionsGuard"]);
  assert_eq!(users.guards[0].schemes[0].entity.short, "bearer");
  assert_eq!(users.permissions[0].entity.short, "users:read");
  let health = answer.unmatched_calls.items.iter().find(|call| call.call.short == "GET {}/health").unwrap();
  assert_eq!(health.caller.entity.short, "checkHealth");
  assert_eq!(health.site.as_ref().map(ToString::to_string).as_deref(), Some("acme-web:src/api/health.ts:4"));
  let guard_scheme = &users.guards[0].schemes[0].relation;
  assert_eq!(guard_scheme.phrase, "possibly implements auth");
  let matrix = answer.auth_matrix.iter().find(|matrix| matrix.repo == "acme-api").unwrap();
  let api_key = matrix.schemes.iter().find(|scheme| scheme.scheme.short == "api_key").unwrap();
  let support = api_key.supported.as_ref().expect("support derived from guards when supports_auth is missing");
  assert!(support.endpoints >= 1);
}

#[test]
fn should_build_the_auth_matrix_from_ground_truth() {
  let fixture = common::acme();
  let truth = expected("acme");

  let answer = fixture.query.api(&ApiRequest::default()).unwrap().answer;

  for auth in section(&truth, "auth") {
    let repo = auth["api_repo"].as_str().unwrap();
    let matrix = answer.auth_matrix.iter().find(|matrix| matrix.repo == repo).unwrap();
    let supported: BTreeSet<&str> = matrix
      .schemes
      .iter()
      .filter(|scheme| scheme.supported.as_ref().is_some_and(|support| support.endpoints > 0))
      .map(|scheme| scheme.scheme.short.as_str())
      .collect();
    let expected_supported: BTreeSet<&str> = section(auth, "supported").iter().filter_map(Value::as_str).collect();
    assert_eq!(supported, expected_supported);
    for (scheme, users) in auth["used"].as_table().unwrap() {
      let usage = matrix.schemes.iter().find(|candidate| candidate.scheme.short == *scheme).unwrap();
      assert_eq!(usage.used, UsageState::Used, "{scheme}");
      let expected_users: BTreeSet<&str> = users.as_array().unwrap().iter().filter_map(Value::as_str).collect();
      let actual: BTreeSet<&str> = usage.used_by_repos.iter().map(String::as_str).collect();
      assert_eq!(actual, expected_users, "{scheme}");
    }
    for scheme in section(auth, "unused").iter().filter_map(Value::as_str) {
      let usage = matrix.schemes.iter().find(|candidate| candidate.scheme.short == scheme).unwrap();
      assert_eq!(usage.used, UsageState::Unused, "{scheme}");
    }
  }
  assert!(!answer.unmatched_calls.items.iter().any(|call| call.call.short == "GET {}/users/me"));
}

#[test]
fn should_report_unused_ground_truth_and_never_report_reachable_code() {
  let fixture = common::acme();
  let truth = expected("acme");

  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 500,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;

  let reported: Vec<(&EntityRef, UnusedClass)> = answer
    .likely_unused
    .items
    .iter()
    .chain(&answer.possibly_unused.items)
    .map(|candidate| (&candidate.entity, candidate.class))
    .collect();
  let selector_of = |entry: &Value| -> Value {
    match (entry.get("sym"), entry.get("endpoint")) {
      (Some(symbol), _) => symbol.clone(),
      (None, Some(endpoint)) => {
        let mut selector = endpoint.as_table().unwrap().clone();
        selector.insert("kind".to_owned(), Value::String("api_endpoint".to_owned()));
        Value::Table(selector)
      }
      _ => panic!("unexpected unused entry {entry}"),
    }
  };
  for entry in section(&truth, "unused") {
    let selector = selector_of(entry);
    assert!(reported.iter().any(|(entity, _)| matches(entity, &selector)), "unused missed {}", label(&selector));
  }
  for entry in section(&truth, "not_unused") {
    let selector = selector_of(entry);
    assert!(!reported.iter().any(|(entity, _)| matches(entity, &selector)), "reported reachable {}", label(&selector));
  }
  for entry in section(&truth, "ambiguous_unused") {
    let selector = selector_of(entry);
    for (entity, class) in reported.iter().filter(|(entity, _)| matches(entity, &selector)) {
      assert_eq!(*class, UnusedClass::PossiblyUnused, "{}", entity.key);
    }
  }
  for name in ["AcmeClient.updateUser", "consume_order_created"] {
    assert!(
      reported.iter().any(|(entity, class)| entity.short == name && *class == UnusedClass::PossiblyUnused),
      "{name}"
    );
  }
}

#[test]
fn should_report_only_the_auth_scheme_no_client_sends() {
  let fixture = common::acme();

  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 500,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;

  let schemes: Vec<&str> = answer
    .possibly_unused
    .items
    .iter()
    .filter(|candidate| candidate.entity.kind == "auth_scheme")
    .map(|candidate| candidate.entity.short.as_str())
    .collect();
  assert_eq!(schemes, ["session_cookie"]);
}

#[test]
fn should_find_ground_truth_tests_for_each_subject() {
  let fixture = common::acme();
  let truth = expected("acme");

  for test_of in section(&truth, "test_of") {
    let subject = &test_of["subject"];
    let filter = atlas_query::TargetFilter {
      kinds: Vec::new(),
      repo: subject["repo"].as_str().map(str::to_owned),
    };
    let answer = found(
      fixture
        .query
        .tests_for(&TestsForRequest {
          filter,
          ..TestsForRequest::new(symbol_selector(subject))
        })
        .unwrap()
        .answer,
    );
    let tests: Vec<&EntityRef> = answer.tests.items.iter().map(|test| &test.test).collect();
    assert!(
      any_matches(tests.iter().copied(), &test_of["test"]),
      "tests-for {} missed {}: {tests:#?}",
      label(subject),
      label(&test_of["test"])
    );
  }
}

#[test]
fn should_rank_direct_tests_first_with_runnable_hints() {
  let fixture = common::acme();

  let answer = found(fixture.query.tests_for(&TestsForRequest::new("to_cents")).unwrap().answer);

  assert_eq!(answer.tests.items[0].test.short, "converts_whole_and_fractional_amounts");
  assert_eq!(
    answer.tests.items[0].run.as_deref(),
    Some("[acme-billing] cargo test -p acme-billing --lib money::tests::converts_whole_and_fractional_amounts")
  );
  let sdk = found(fixture.query.tests_for(&TestsForRequest::new("toCents")).unwrap().answer);
  assert_eq!(
    sdk.tests.items[0].run.as_deref(),
    Some("[acme-sdk] npx vitest run src/money.test.ts -t \"toCents should convert whole and fractional amounts\"")
  );
}

#[test]
fn should_list_similar_money_helpers_and_suggest_the_sdk_as_shared_home() {
  let fixture = common::acme();

  let answer = found(fixture.query.similar(&SimilarRequest::new("toCents")).unwrap().answer);

  let members: Vec<&str> = answer.members.iter().map(|member| member.entity.short.as_str()).collect();
  assert!(members.contains(&"parseMoney"), "{members:?}");
  assert!(members.contains(&"to_cents"), "{members:?}");
  let home = answer.shared_home.iter().find(|home| home.language == "ts").unwrap();
  assert_eq!(home.package.as_deref(), Some("@acme/sdk"));
  assert!(answer.members.iter().all(|member| !member.verdict.contains("duplicate")));
  assert!(answer.coupling_risks.iter().any(|risk| risk.contains("span languages")));
}

fn listed(answer: &ContextAnswer) -> Vec<&EntityRef> {
  [&answer.must_read, &answer.skim, &answer.reuse, &answer.impact, &answer.tests, &answer.uncertain]
    .into_iter()
    .flat_map(|items| items.iter().map(|item| &item.entity))
    .collect()
}

#[test]
fn should_build_a_context_pack_for_adding_an_authenticated_delete_endpoint() {
  let fixture = common::acme();

  let answer = fixture
    .query
    .context(&ContextRequest::new("add a DELETE /orders/:id endpoint with api-key auth"))
    .unwrap()
    .answer;

  let items = listed(&answer);
  let has = |short: &str| items.iter().any(|entity| entity.short == short);
  for required in [
    "OrdersController",
    "OrdersService",
    "ApiKeyGuard",
    "UsersController.remove",
    "DELETE /users/{id}",
    "ReportsController.export",
  ] {
    assert!(has(required), "context pack misses {required}");
  }
  assert!(
    answer
      .tests
      .iter()
      .any(|test| test.entity.location.as_ref().unwrap().path == "src/orders/orders.controller.spec.ts")
  );
  let sdk = [&answer.impact, &answer.uncertain]
    .into_iter()
    .flatten()
    .find(|item| item.entity.repo() == "acme-sdk" && item.entity.short.starts_with("AcmeClient"))
    .expect("the SDK client must be listed as a possible consumer");
  assert!(sdk.uncertain);
  let relevant = [
    "OrdersController",
    "OrdersService",
    "OrdersController.findOne",
    "OrdersService.findOne",
    "GET /orders/{id}",
    "ApiKeyGuard",
    "JwtOrApiKeyGuard",
    "api_key",
    "UsersController.remove",
    "DELETE /users/{id}",
    "OrdersController.should delegate findOne to the service",
    "AcmeClient.getOrder",
    "ReportsController.export",
    "GET /v1/reports/export",
    "AcmeClient.headers",
  ];
  let relevant: BTreeSet<&str> = relevant.into_iter().collect();
  let shown: BTreeSet<&str> = items.iter().map(|entity| entity.short.as_str()).collect();
  let hits = shown.intersection(&relevant).count();
  let precision = hits as f64 / shown.len() as f64;
  let recall = hits as f64 / relevant.len() as f64;
  println!(
    "context precision {precision:.2} recall {recall:.2} ({hits} relevant of {} listed, {} relevant)",
    shown.len(),
    relevant.len()
  );
  assert_eq!(recall, 1.0, "missing {:?}", relevant.difference(&shown).collect::<Vec<_>>());
  assert!(precision >= 0.3, "precision {precision:.2}: {shown:?}");
  assert!(answer.estimated_tokens <= answer.budget);
}

#[test]
fn should_keep_the_impact_placement_of_a_heuristic_consumer_that_is_also_a_search_hit() {
  let fixture = common::acme();
  let mut request = ContextRequest::new("add a DELETE /orders/:id endpoint with api-key auth");
  request.explain = true;

  let answer = fixture.query.context(&request).unwrap().answer;

  let seeds = &answer.explain.as_ref().unwrap().seeds;
  assert!(
    seeds.iter().any(|seed| seed.contains("AcmeApiClient::get_order") && seed.contains("search hit")),
    "{seeds:#?}"
  );
  let get_order = answer
    .impact
    .iter()
    .find(|item| item.entity.short == "AcmeApiClient::get_order")
    .unwrap_or_else(|| panic!("get_order lost its IMPACT placement: {:#?}", answer.impact));
  assert!(get_order.uncertain);
  assert_eq!(get_order.why, "possibly calls endpoint GET /orders/{id} [heuristic 0.77]");
  let elsewhere: Vec<&str> = [&answer.must_read, &answer.skim, &answer.uncertain]
    .into_iter()
    .flatten()
    .map(|item| item.entity.short.as_str())
    .filter(|short| *short == "AcmeApiClient::get_order")
    .collect();
  assert_eq!(elsewhere, Vec::<&str>::new());
}

#[test]
fn should_list_every_candidate_for_an_ambiguous_name_and_never_pick_one() {
  let fixture = common::acme();

  let answer = fixture.query.symbol(&SymbolRequest::new("findOne")).unwrap().answer;

  let Targeted::Ambiguous { candidates, .. } = answer else {
    panic!("expected an ambiguous response")
  };
  let names: Vec<&str> = candidates.iter().map(|candidate| candidate.short.as_str()).collect();
  assert_eq!(
    names,
    ["OrdersController.findOne", "OrdersService.findOne", "UsersController.findOne", "UsersService.findOne"]
  );
}

#[test]
fn should_prefer_definitions_over_same_named_test_blocks() {
  let fixture = common::acme();

  let answer = found(fixture.query.symbol(&SymbolRequest::new("UsersService.findOne")).unwrap().answer);

  assert_eq!(answer.entity.kind, "method");
  assert_eq!(answer.counts.callers.total, 4);
}

#[test]
fn should_report_truncation_and_omitted_items() {
  let fixture = common::acme();

  let limited = fixture
    .query
    .callers(&TraverseRequest {
      limit: 1,
      depth: 2,
      ..TraverseRequest::new("UsersService.findOne")
    })
    .unwrap();
  let text = fixture.text(&limited);
  assert!(text.contains("4 more omitted (raise limit)"), "{text}");
  let capped = fixture
    .query
    .impact(&ImpactRequest {
      max_nodes: 2,
      ..ImpactRequest::new(["InvoiceStore::insert"])
    })
    .unwrap();
  let text = fixture.text(&capped);
  assert!(text.contains("traversal truncated at 2 nodes; results are incomplete"), "{text}");
}

#[test]
fn should_report_unresolved_references_that_could_hide_callers() {
  let fixture = common::acme();

  let response = fixture.query.callers(&TraverseRequest::new("InvoiceStore::insert")).unwrap();

  let text = fixture.text(&response);
  assert!(text.contains("2 unresolved references named \"insert\" could hide relations (2 ambiguous)"), "{text}");
}

#[test]
fn should_keep_projects_isolated() {
  let acme = common::acme();
  let globex = common::globex();

  let acme_hits = acme.query.search(&SearchRequest::new("formatMoney")).unwrap().answer;
  let globex_impact = found(globex.query.impact(&ImpactRequest::new(["UsersService.findOne"])).unwrap().answer);

  let acme_repos: BTreeSet<&str> = acme_hits
    .exact_name
    .iter()
    .chain(&acme_hits.strong)
    .chain(&acme_hits.related)
    .map(|hit| hit.entity.repo())
    .collect();
  assert!(acme_repos.iter().all(|repo| repo.starts_with("acme-")), "{acme_repos:?}");
  assert!(impacted(&globex_impact).iter().all(|entity| entity.repo().starts_with("globex-")));
  assert!(globex_impact.targets[0].repo().starts_with("globex-"));
}

#[test]
fn should_explain_an_edge_with_owner_assertions_and_hash_checked_excerpts() {
  let fixture = common::acme();
  let callers = found(fixture.query.callers(&TraverseRequest::new("UsersService.findOne")).unwrap().answer);
  let edge = &callers.groups[0]
    .items
    .iter()
    .find(|item| item.entity.short == "UsersController.findOne")
    .unwrap()
    .relation
    .edge;

  let answer = found(fixture.query.explain(&atlas_query::ExplainRequest::new(format!("edge:{edge}"))).unwrap().answer);

  let atlas_query::ExplainAnswer::Edge { kind, assertions, certainty, .. } = answer else {
    panic!("expected an edge explanation")
  };
  assert_eq!(kind, "calls");
  assert_eq!(certainty.tag, "[resolved]");
  assert_eq!(assertions.len(), 1);
  assert_eq!(assertions[0].owner, "file acme-api:src/users/users.controller.ts");
  let evidence = &assertions[0].evidence[0];
  assert_eq!(evidence.location.to_string(), "acme-api:src/users/users.controller.ts:16");
  let Some(atlas_query::Snippet::Text { lines, .. }) = &evidence.excerpt else {
    panic!("expected an excerpt")
  };
  assert_eq!(lines, &["    return this.usersService.findOne(id);".to_owned()]);
}

#[test]
fn should_aggregate_file_targets_over_their_symbols() {
  let fixture = common::acme();

  let answer = found(fixture.query.callers(&TraverseRequest::new("acme-api:src/users/users.service.ts")).unwrap().answer);

  assert_eq!(answer.aggregated_symbols, Some(11));
  let callers: BTreeSet<&str> = answer.groups.iter().flat_map(|group| group.items.iter().map(|item| item.entity.short.as_str())).collect();
  assert!(callers.contains("UsersController.findOne"), "{callers:?}");
  assert!(
    fixture
      .text(&fixture.query.callers(&TraverseRequest::new("acme-api:src/users/users.service.ts")).unwrap())
      .contains("file target: aggregated over 11 symbols")
  );
}

#[test]
fn should_say_when_a_line_target_is_outside_every_symbol() {
  let fixture = common::acme();

  let answer = fixture.query.symbol(&SymbolRequest::new("acme-billing:src/money.rs:16")).unwrap().answer;

  let Targeted::NotFound { hint, .. } = answer else {
    panic!("expected not found, got {answer:?}")
  };
  assert_eq!(
    hint,
    "line 16 is outside any symbol in acme-billing:src/money.rs; the file has 4 symbols (target the file itself to aggregate them)"
  );
}

#[test]
fn should_target_rust_trait_impl_methods_by_their_natural_name() {
  let fixture = common::acme();

  for spec in ["PgInvoiceStore::insert", "PgInvoiceStore.insert", "<PgInvoiceStore as InvoiceStore>::insert"] {
    let answer = found(fixture.query.symbol(&SymbolRequest::new(spec)).unwrap().answer);
    assert_eq!(answer.entity.short, "PgInvoiceStore::<InvoiceStore>::insert", "{spec}");
  }
  let impact = found(fixture.query.impact(&ImpactRequest::new(["PgInvoiceStore::insert"])).unwrap().answer);
  let text = fixture.text(&fixture.query.impact(&ImpactRequest::new(["PgInvoiceStore::insert"])).unwrap());
  assert!(!impact.unresolved_risk.is_empty());
  assert!(
    text.contains("callers of the implemented or overridden declaration InvoiceStore::insert may dispatch here dynamically"),
    "{text}"
  );
}

#[test]
fn should_cap_context_tests_and_carry_structured_certainty() {
  let fixture = common::acme();

  let answer = fixture
    .query
    .context(&ContextRequest::new("add a DELETE /orders/:id endpoint with api-key auth"))
    .unwrap()
    .answer;

  assert!(answer.tests.len() <= 8);
  assert!(answer.impact.len() <= 15);
  let reached: Vec<&atlas_query::ContextItem> = answer.must_read.iter().chain(&answer.skim).filter(|item| item.via.is_some()).collect();
  assert!(!reached.is_empty());
  assert!(reached.iter().all(|item| item.certainty.as_ref() == item.via.as_ref().map(|via| &via.certainty)));
  assert!(
    answer.patterns.iter().any(|item| item.entity.short == "AuthModule" || item.entity.short == "SessionGuard"),
    "{:#?}",
    answer.patterns
  );
}

#[test]
fn should_tag_each_attr_with_its_own_source_certainty() {
  let fixture = common::acme();
  fixture.query.status().unwrap();
  let key: atlas_core::EntityKey = "endpoint:acme-api:GET /users/{id}".parse().unwrap();
  let mut asserted = atlas_core::Entity::new(key, atlas_core::Certainty::heuristic(atlas_core::Confidence::new(0.4).unwrap(), "probe"));
  asserted.attrs.insert("probe_note".to_owned(), serde_json::json!("guessed"));
  let owner = atlas_core::Owner::Project { pass: "probe".to_owned() };
  let facts = atlas_store::OwnedFacts {
    entities: vec![asserted],
    ..atlas_store::OwnedFacts::default()
  };
  fixture.query.engine().store().write(|tx| tx.replace_owner(&owner, &facts)).unwrap();

  let response = fixture.query.explain(&atlas_query::ExplainRequest::new("GET /users/:id")).unwrap();

  let atlas_query::ExplainAnswer::Entity { certainty, attrs, .. } = found(response.answer.clone()) else {
    panic!("expected an entity")
  };
  assert_eq!(certainty.tag, "[syntactic]");
  let note = attrs.iter().find(|attr| attr.name == "probe_note").unwrap();
  assert_eq!(note.tag(), "[heuristic 0.40, probe]");
  let framework = attrs.iter().find(|attr| attr.name == "framework").unwrap();
  assert_eq!(framework.tag(), "[syntactic, file]");
  assert!(fixture.text(&response).contains("  attr probe_note: guessed [heuristic 0.40, probe]"));
}

#[test]
fn should_reach_the_endpoint_of_a_direct_dependent_at_every_depth() {
  let fixture = common::acme();

  for depth in [1, 3] {
    let answer = found(
      fixture
        .query
        .impact(&ImpactRequest {
          depth,
          ..ImpactRequest::new(["UsersService.findOne"])
        })
        .unwrap()
        .answer,
    );
    let endpoints: Vec<&str> = answer.endpoints.items.iter().map(|item| item.entity.short.as_str()).collect();
    assert!(endpoints.contains(&"GET /users/{id}"), "depth {depth}: {endpoints:?}");
  }
}
