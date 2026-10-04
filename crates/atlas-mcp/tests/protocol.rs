mod common;

use std::fs;

use atlas_project::{Registry, RepoSpec};
use common::{ACME_REPOS, start, start_limited, start_watching, start_with, text_of};
use serde_json::{Value, json};

const TOOLS: [&str; 16] = [
  "atlas_api",
  "atlas_callees",
  "atlas_callers",
  "atlas_changes",
  "atlas_context",
  "atlas_explain",
  "atlas_impact",
  "atlas_overview",
  "atlas_patterns",
  "atlas_read",
  "atlas_search",
  "atlas_similar",
  "atlas_status",
  "atlas_symbol",
  "atlas_tests_for",
  "atlas_unused",
];

#[tokio::test]
async fn should_advertise_the_workflow_instructions_after_initialize() {
  let harness = start(&ACME_REPOS).await;

  let info = harness.client.peer_info().unwrap();
  let instructions = info.instructions.clone().unwrap();

  assert!(info.capabilities.tools.is_some());
  assert!(instructions.lines().count() <= 12, "{instructions}");
  assert!(instructions.starts_with("Code Atlas answers questions about ONE indexed project"));
  assert!(instructions.contains("atlas_context"));
  assert!(instructions.contains("[heuristic] and [inferred] relations are leads: verify them in the source"));
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_tell_agents_to_read_files_they_will_edit_with_their_own_tool() {
  let harness = start(&ACME_REPOS).await;
  let guidance = "To edit a file, read it with your own file-reading tool; use atlas_read for code you only need to view.";

  let instructions = harness.client.peer_info().unwrap().instructions.clone().unwrap();
  let tools = harness.client.list_all_tools().await.unwrap();

  assert!(instructions.contains(guidance), "{instructions}");
  for name in ["atlas_read", "atlas_context"] {
    let description = tools.iter().find(|tool| tool.name == name).and_then(|tool| tool.description.clone()).unwrap();
    assert!(description.contains(guidance), "{name}: {description}");
  }
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_list_every_tool_with_a_schema_and_an_agent_facing_description() {
  let harness = start(&ACME_REPOS).await;

  let tools = harness.client.list_all_tools().await.unwrap();

  let mut names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
  names.sort_unstable();
  assert_eq!(names, TOOLS);
  for tool in &tools {
    assert_eq!(tool.input_schema.get("type"), Some(&json!("object")), "{}", tool.name);
    assert!(tool.input_schema["properties"]["format"].is_object(), "{} lacks the format option", tool.name);
    assert!(tool.description.as_deref().is_some_and(|description| description.len() > 40), "{}", tool.name);
  }
  let required = |name: &str| -> Vec<String> {
    let tool = tools.iter().find(|tool| tool.name == name).unwrap();
    let mut required: Vec<String> = tool
      .input_schema
      .get("required")
      .and_then(Value::as_array)
      .into_iter()
      .flatten()
      .filter_map(Value::as_str)
      .map(str::to_owned)
      .collect();
    required.sort_unstable();
    required
  };
  assert_eq!(required("atlas_search"), ["query"]);
  assert_eq!(required("atlas_context"), ["task"]);
  assert_eq!(required("atlas_impact"), ["targets"]);
  assert_eq!(required("atlas_callers"), ["target"]);
  assert!(required("atlas_overview").is_empty());
  for name in ["atlas_context", "atlas_callers", "atlas_callees", "atlas_impact", "atlas_tests_for", "atlas_api"] {
    let description = tools.iter().find(|tool| tool.name == name).unwrap().description.clone().unwrap();
    assert!(description.contains("[heuristic]/[inferred]"), "{name}: {description}");
    assert!(description.contains("verify"), "{name}: {description}");
  }
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_find_a_capability_across_repos_with_atlas_search() {
  let harness = start(&ACME_REPOS).await;

  let text = harness.text("atlas_search", json!({ "query": "convert currency string to cents" })).await;

  for expected in ["toCents", "parseMoney", "to_cents"] {
    assert!(text.contains(expected), "missing {expected}: {text}");
  }
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_report_the_blast_radius_across_repos_with_atlas_impact() {
  let harness = start(&ACME_REPOS).await;

  let text = harness.text("atlas_impact", json!({ "targets": ["UsersService.findOne"] })).await;

  assert!(text.starts_with("fresh run="), "{text}");
  assert!(text.contains("acme-api"), "{text}");
  assert!(text.contains("UsersController.findOne"), "{text}");
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_return_a_context_pack_for_a_task() {
  let harness = start(&ACME_REPOS).await;

  let text = harness.text("atlas_context", json!({ "task": "convert currency string to cents", "budget": 4000 })).await;

  assert!(text.contains("toCents") || text.contains("to_cents"), "{text}");
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_return_structured_output_when_json_is_requested() {
  let harness = start(&ACME_REPOS).await;

  let text = harness.text("atlas_search", json!({ "query": "to_cents", "format": "json" })).await;

  let document: Value = serde_json::from_str(&text).unwrap();
  assert_eq!(document["freshness"]["state"], "fresh");
  assert_eq!(document["answer"]["query"], "to_cents");
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_reject_malformed_arguments_with_a_schema_error_and_not_count_them_as_queries() {
  let harness = start(&ACME_REPOS).await;

  let wrong_type = harness.failure("atlas_search", json!({ "query": 5 })).await;
  let missing = harness.failure("atlas_search", json!({})).await;
  let unknown = harness.failure("atlas_search", json!({ "query": "x", "bogus": 1 })).await;
  let wrong_list = harness.failure("atlas_impact", json!({ "targets": "UsersService.findOne" })).await;

  assert!(wrong_type.contains("failed to deserialize parameters: invalid type: integer `5`"), "{wrong_type}");
  assert!(missing.contains("missing field `query`"), "{missing}");
  assert!(unknown.contains("unknown field `bogus`"), "{unknown}");
  assert!(wrong_list.contains("invalid type: string"), "{wrong_list}");
  assert_eq!(harness.server.telemetry().total_calls, 0);
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_explain_semantic_argument_errors_as_tool_errors() {
  let harness = start(&ACME_REPOS).await;

  assert!(
    harness
      .failure("atlas_callers", json!({ "target": "UsersService.findOne", "depth": 99 }))
      .await
      .contains("depth must be between 1 and 6")
  );
  assert!(harness.failure("atlas_impact", json!({ "targets": [] })).await.contains("targets must be a non-empty list"));
  assert!(
    harness
      .failure("atlas_search", json!({ "query": "x", "kind": "Not A Kind" }))
      .await
      .contains("unknown kind")
  );
  assert!(harness.failure("atlas_similar", json!({})).await.contains("exactly one of target or snippet"));
  assert!(
    harness
      .failure("atlas_similar", json!({ "target": "a", "snippet": "b" }))
      .await
      .contains("exactly one of target or snippet")
  );
  assert!(harness.failure("atlas_search", json!({ "query": "  " })).await.contains("query must not be empty"));
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_suggest_similar_names_when_a_target_is_not_found() {
  let harness = start(&ACME_REPOS).await;

  let text = harness.text("atlas_symbol", json!({ "target": "UsersServce.findOn" })).await;
  let structured = harness.text("atlas_symbol", json!({ "target": "UsersServce.findOn", "format": "json" })).await;

  assert!(text.contains("not found"), "{text}");
  assert!(text.contains("did you mean"), "{text}");
  assert!(text.contains("findOne"), "{text}");
  let document: Value = serde_json::from_str(&structured).unwrap();
  assert_eq!(document["answer"]["outcome"], "not_found");
  assert!(document["answer"]["did_you_mean"].as_array().is_some_and(|items| !items.is_empty()), "{structured}");
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_turn_a_panic_inside_a_query_into_a_tool_error_and_keep_serving() {
  let harness = start_with(&ACME_REPOS, Some(|tool: &str| assert_ne!(tool, "atlas_callers", "injected failure"))).await;

  let failure = harness.failure("atlas_callers", json!({ "target": "UsersService.findOne" })).await;
  let afterwards = harness.text("atlas_search", json!({ "query": "to_cents" })).await;

  assert!(failure.contains("internal error while running atlas_callers"), "{failure}");
  assert!(failure.contains("injected failure"), "{failure}");
  assert!(failure.contains("the server is still running"), "{failure}");
  assert!(afterwards.contains("to_cents"), "{afterwards}");
  let status = harness.text("atlas_status", json!({})).await;
  assert!(status.contains("atlas_callers calls=1 errors=1"), "{status}");
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_never_answer_about_another_project() {
  let harness = start(&ACME_REPOS).await;

  let search = harness
    .text("atlas_search", json!({ "query": "handle_order_created requireGlobexKey registerOrderRoutes" }))
    .await;
  let symbol = harness.text("atlas_symbol", json!({ "target": "handle_order_created" })).await;
  let file = harness.text("atlas_read", json!({ "target": "globex-worker:src/consumer.rs" })).await;
  let impact = harness.text("atlas_impact", json!({ "targets": ["globex-api:src/services/orders.service.ts"] })).await;
  let repo_filter = harness.failure("atlas_search", json!({ "query": "orders", "repo": "globex-api" })).await;
  let overview = harness.text("atlas_overview", json!({})).await;

  let echoed = [
    ("handle_order_created requireGlobexKey registerOrderRoutes", &search),
    ("handle_order_created", &symbol),
    ("globex-worker:src/consumer.rs", &file),
    ("globex-api:src/services/orders.service.ts", &impact),
  ];
  for (asked, text) in echoed {
    assert!(!text.replace(asked, "").to_lowercase().contains("globex"), "{text}");
  }
  assert!(symbol.contains("not found"), "{symbol}");
  assert!(file.contains("not found"), "{file}");
  assert!(impact.contains("not found"), "{impact}");
  assert!(repo_filter.contains("is not in project \"acme\""), "{repo_filter}");
  assert!(!overview.contains("globex"), "{overview}");
  assert_eq!(harness.server.project_name(), "acme");
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_serve_concurrent_calls() {
  let harness = start(&ACME_REPOS).await;

  let (search, impact, symbol, overview, api, status) = tokio::join!(
    harness.call("atlas_search", json!({ "query": "to_cents" })),
    harness.call("atlas_impact", json!({ "targets": ["UsersService.findOne"] })),
    harness.call("atlas_symbol", json!({ "target": "toCents" })),
    harness.call("atlas_overview", json!({})),
    harness.call("atlas_api", json!({})),
    harness.call("atlas_status", json!({})),
  );

  for result in [&search, &impact, &symbol, &overview, &api, &status] {
    assert_ne!(result.is_error, Some(true), "{}", text_of(result));
    assert!(text_of(result).starts_with("fresh run="), "{}", text_of(result));
  }
  assert!(text_of(&search).contains("to_cents"));
  assert!(text_of(&impact).contains("UsersController.findOne"));
  assert_eq!(harness.server.telemetry().total_calls, 6);
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_reflect_an_edit_made_between_calls() {
  let harness = start(&ACME_REPOS).await;
  let before = harness.text("atlas_search", json!({ "query": "zz_brand_new_marker_fn" })).await;
  assert!(!before.contains("zz_brand_new_marker_fn  ") && !before.contains("fn zz_brand_new_marker_fn"), "{before}");
  let money = common::repo_path(harness.scratch.path(), "acme-billing").join("src/money.rs");
  let mut source = fs::read_to_string(&money).unwrap();
  source.push_str("\npub fn zz_brand_new_marker_fn() -> i64 {\n  7\n}\n");
  fs::write(&money, source).unwrap();

  let after = harness.text("atlas_search", json!({ "query": "zz_brand_new_marker_fn" })).await;
  let symbol = harness.text("atlas_read", json!({ "target": "zz_brand_new_marker_fn" })).await;
  let changes = harness.text("atlas_changes", json!({})).await;

  assert!(after.contains("zz_brand_new_marker_fn"), "{after}");
  assert!(symbol.contains("7"), "{symbol}");
  assert!(changes.contains("acme-billing:src/money.rs"), "{changes}");
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_pick_up_a_repo_added_to_the_registry_while_running() {
  let harness = start(&["acme-api", "acme-sdk", "acme-web"]).await;
  let before = harness.text("atlas_search", json!({ "query": "to_cents" })).await;
  assert!(!before.contains("acme-billing"), "{before}");
  let billing = common::materialise(harness.scratch.path(), "acme", "acme-billing");

  Registry::load(&harness.home)
    .unwrap()
    .add_repo("acme", RepoSpec::new(billing).named("acme-billing"))
    .unwrap();
  let after = harness.text("atlas_search", json!({ "query": "to_cents" })).await;
  let status = harness.text("atlas_status", json!({})).await;

  assert!(after.contains("acme-billing"), "{after}");
  assert!(!after.contains("DEGRADED"), "{after}");
  assert!(status.contains("acme-billing"), "{status}");
  assert!(!status.contains("DEGRADED"), "{status}");
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_follow_registry_changes_and_edits_with_the_watcher_enabled() {
  let harness = start_watching(&["acme-api", "acme-sdk", "acme-web"]).await;
  harness.text("atlas_overview", json!({})).await;
  let billing = common::materialise(harness.scratch.path(), "acme", "acme-billing");
  Registry::load(&harness.home)
    .unwrap()
    .add_repo("acme", RepoSpec::new(billing.clone()).named("acme-billing"))
    .unwrap();
  let added = harness.text("atlas_search", json!({ "query": "to_cents" })).await;
  let money = billing.join("src/money.rs");
  let mut source = fs::read_to_string(&money).unwrap();
  source.push_str("\npub fn zz_watched_marker_fn() -> i64 {\n  9\n}\n");
  fs::write(&money, source).unwrap();

  let edited = tokio::time::timeout(std::time::Duration::from_secs(20), async {
    loop {
      let text = harness.text("atlas_search", json!({ "query": "zz_watched_marker_fn" })).await;
      if text.contains("acme-billing:src/money.rs") {
        return text;
      }
      tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
  })
  .await
  .unwrap();

  assert!(added.contains("acme-billing") && !added.contains("DEGRADED"), "{added}");
  assert!(edited.contains("acme-billing:src/money.rs"), "{edited}");
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_report_per_tool_telemetry_through_atlas_status() {
  let harness = start(&ACME_REPOS).await;
  harness.text("atlas_search", json!({ "query": "to_cents" })).await;
  harness.text("atlas_search", json!({ "query": "parseMoney" })).await;
  harness.failure("atlas_impact", json!({ "targets": [] })).await;

  let text = harness.text("atlas_status", json!({})).await;
  let structured: Value = serde_json::from_str(&harness.text("atlas_status", json!({ "format": "json" })).await).unwrap();

  assert!(text.contains("mcp telemetry: 3 calls"), "{text}");
  assert!(text.contains("atlas_search calls=2 errors=0"), "{text}");
  assert!(text.contains("atlas_impact calls=1 errors=1"), "{text}");
  let search = &structured["mcp_telemetry"]["tools"]["atlas_search"];
  assert_eq!(search["calls"], 2);
  assert!(search["bytes_returned"].as_u64().unwrap() > 0);
  assert!(search["max_latency_us"].as_u64().unwrap() >= search["total_latency_us"].as_u64().unwrap() / 2);
  assert_eq!(harness.server.telemetry().total_calls, 5);
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_persist_the_similarity_index_when_the_shutdown_signal_arrives() {
  let harness = start(&ACME_REPOS).await;
  harness.text("atlas_search", json!({ "query": "to_cents" })).await;

  harness.shutdown.send(()).unwrap();
  harness.task.await.unwrap().unwrap();

  let db = harness.home.db_path(&"acme".parse().unwrap());
  let store = atlas_store::Store::open(&db).unwrap();
  let blob = store.read(|tx| tx.blob("similarity")).unwrap();
  assert!(blob.is_some(), "the similarity blob was not persisted on shutdown");
}

#[tokio::test]
async fn should_stop_cleanly_when_the_client_disconnects() {
  let harness = start(&ACME_REPOS).await;
  harness.text("atlas_overview", json!({})).await;

  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_truncate_oversized_text_output_and_record_it_in_telemetry() {
  let full = start(&ACME_REPOS).await.text("atlas_overview", json!({})).await;
  assert!(full.len() > 800, "fixture overview too small to test truncation: {}", full.len());
  let harness = start_limited(&ACME_REPOS, 600, 400).await;

  let text = harness.text("atlas_overview", json!({})).await;

  assert!(text.starts_with("fresh run="), "{text}");
  assert!(text.contains("[output truncated: "), "{text}");
  assert!(text.contains("narrow the query (limit/depth/kind/repo) or use atlas_read on a specific symbol]"), "{text}");
  assert!(text.len() < 600 + 250, "{}", text.len());
  let telemetry = harness.server.telemetry();
  let overview = &telemetry.tools["atlas_overview"];
  assert_eq!(overview.truncated_calls, 1);
  assert!(overview.bytes_truncated > 0);
  assert_eq!(overview.bytes_returned as usize, text.len());
}

#[tokio::test]
async fn should_wrap_truncated_json_output_in_a_valid_document() {
  let harness = start_limited(&ACME_REPOS, 600, 400).await;

  let text = harness.text("atlas_overview", json!({ "format": "json" })).await;

  let document: Value = serde_json::from_str(&text).unwrap();
  assert_eq!(document["truncated"], true);
  assert!(document["bytes"].as_u64().unwrap() > 600, "{document}");
  assert!(document["note"].as_str().unwrap().contains("narrow the query"), "{document}");
  assert!(document["partial_text"].as_str().unwrap().len() <= 600, "{document}");
}

#[tokio::test]
async fn should_bound_the_context_budget() {
  let harness = start(&ACME_REPOS).await;

  assert!(
    harness
      .failure("atlas_context", json!({ "task": "x", "budget": 100 }))
      .await
      .contains("budget must be between 500 and 32000")
  );
  assert!(
    harness
      .failure("atlas_context", json!({ "task": "x", "budget": 1000000 }))
      .await
      .contains("budget must be between 500 and 32000")
  );
  harness.text("atlas_context", json!({ "task": "convert currency string to cents", "budget": 500 })).await;
}

#[tokio::test]
async fn should_cap_symbol_and_read_bodies_and_say_how_to_read_the_rest() {
  let harness = start_limited(&ACME_REPOS, 100_000, 3).await;

  let read = harness.text("atlas_read", json!({ "target": "toCents" })).await;
  let symbol = harness.text("atlas_symbol", json!({ "target": "toCents", "body": true })).await;
  let structured = harness.text("atlas_read", json!({ "target": "toCents", "format": "json" })).await;

  for text in [&read, &symbol] {
    assert!(text.contains("body capped at 3 lines, 4 more not shown"), "{text}");
    assert!(text.contains("const trimmed"), "{text}");
    assert!(!text.contains("return negative"), "{text}");
  }
  let document: Value = serde_json::from_str(&structured).unwrap();
  assert!(document["answer"]["note"].as_str().unwrap().contains("body capped at 3 lines"), "{structured}");
  assert_eq!(document["answer"]["source"]["lines"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn should_refuse_paths_outside_the_project_and_other_projects_keys() {
  let harness = start(&ACME_REPOS).await;

  for tool in ["atlas_read", "atlas_symbol"] {
    for target in ["../../etc/passwd", "/etc/passwd", "file:globex-api:src/server.ts", "globex-api:src/server.ts"] {
      let text = harness.text(tool, json!({ "target": target })).await;
      assert!(text.contains("not found"), "{tool} {target}: {text}");
      assert!(!text.contains("root:"), "{tool} {target}: {text}");
      assert!(!text.replace(target, "").contains("globex"), "{tool} {target}: {text}");
    }
  }
  harness.stop().await.unwrap();
}
