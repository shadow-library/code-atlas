mod common;

use std::fs;

use atlas_core::Certainty;
use atlas_mcp::guidance::{self, TOOLS, ToolProfile};
use atlas_project::{Registry, RepoSpec, ToolsSetting};
use common::{ACME_REPOS, Harness, start, start_configured, start_counterpart_corpus, start_docs_corpus, start_limited, start_watching, start_with, text_of};
use serde_json::{Value, json};

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
async fn should_name_certainty_tags_exactly_as_the_query_output_renders_them() {
  let harness = start(&ACME_REPOS).await;
  let instructions = harness.client.peer_info().unwrap().instructions.clone().unwrap();
  let tools = harness.client.list_all_tools().await.unwrap();

  for certainty in [Certainty::CompilerVerified, Certainty::Syntactic, Certainty::Resolved, Certainty::Declared] {
    let tag = certainty.tag();
    assert!(instructions.contains(&tag), "{tag} missing from {instructions}");
  }
  assert!(!instructions.contains("compiler_verified"), "{instructions}");
  for tool in &tools {
    let description = tool.description.clone().unwrap();
    assert!(!description.contains("compiler_verified"), "{}: {description}", tool.name);
  }
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_tell_agents_to_edit_through_atlas_edit_and_reread_changed_files() {
  let harness = start(&ACME_REPOS).await;
  let rule = "To rename or rewrite a symbol use atlas_edit (exact across files), not find-and-replace; after it changes a file, re-read that file before editing it yourself.";

  let instructions = harness.client.peer_info().unwrap().instructions.clone().unwrap();
  let tools = harness.client.list_all_tools().await.unwrap();

  assert!(instructions.contains(rule), "{instructions}");
  assert!(instructions.contains("After editing, call atlas_status with since_run"), "{instructions}");
  assert!(!instructions.contains("own file-reading tool"), "{instructions}");
  for removed in ["atlas_read", "atlas_changes"] {
    assert!(!instructions.contains(removed), "{removed}: {instructions}");
    assert!(tools.iter().all(|tool| tool.name != removed), "{removed}");
    assert!(tools.iter().all(|tool| !tool.description.clone().unwrap().contains(removed)), "{removed}");
  }
  let edit = tools.iter().find(|tool| tool.name == "atlas_edit").and_then(|tool| tool.description.clone()).unwrap();
  assert!(edit.contains("Re-read changed files before editing them with your own tool."), "{edit}");
  assert!(edit.contains(atlas_ide::REPLACE_BODY_NOTE), "{edit}");
  assert!(edit.contains(atlas_ide::REGEX_NOTE), "{edit}");
  assert!(edit.contains(atlas_ide::REPLACE_IN_FILES_NOTE), "{edit}");
  assert!(edit.contains("set allow_unverified only after checking them"), "{edit}");
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_list_as_read_only_exactly_the_tools_the_auto_approve_hook_approves() {
  let harness = start(&ACME_REPOS).await;

  let tools = harness.client.list_all_tools().await.unwrap();

  let mut read_only: Vec<&str> = tools
    .iter()
    .filter(|tool| tool.annotations.as_ref().and_then(|annotations| annotations.read_only_hint) == Some(true))
    .map(|tool| tool.name.as_ref())
    .collect();
  read_only.sort_unstable();
  assert_eq!(read_only, guidance::READ_ONLY_TOOLS);
  assert!(tools.iter().any(|tool| tool.name == guidance::EDIT_TOOL));
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
async fn should_list_the_counterparts_of_a_file_target_in_atlas_impact_and_none_for_a_symbol_without_an_also_change_section() {
  let harness = start_counterpart_corpus().await;
  let root = common::repo_path(harness.scratch.path(), "notes-server");

  let file = harness.text("atlas_impact", json!({ "targets": ["notes-web:src/ops/op-switch.ts"] })).await;
  let json_text = harness
    .text("atlas_impact", json!({ "targets": ["notes-web:src/ops/op-switch.ts"], "format": "json" }))
    .await;
  let symbol = harness.text("atlas_impact", json!({ "targets": ["pinNote"], "format": "json" })).await;

  let section: Vec<&str> = file.lines().skip_while(|line| !line.starts_with("COUNTERPARTS")).take(4).collect();
  assert_eq!(
    section,
    [
      "COUNTERPARTS (1): files in other repos or packages that share contract values with it",
      "  notes-server:src/ops/op-specs.ts [heuristic 0.44] score=31.45",
      &format!("    file: {}", root.join("src/ops/op-specs.ts").display()),
      "    shared values (14), rarest: 'draft_slug', 'revision_mark', 'section_anchor', 'page.archive_draft', 'page.create_draft'",
    ]
  );
  let document: Value = serde_json::from_str(&json_text).unwrap();
  assert_eq!(document["answer"]["counterparts"][0]["path"], "src/ops/op-specs.ts");
  assert_eq!(document["answer"]["counterparts"][0]["certainty"]["tag"], "[heuristic 0.44]");
  assert!(!file.contains("ALSO CHANGE"), "{file}");
  assert_eq!(document["answer"].get("also_change"), None);
  let symbol: Value = serde_json::from_str(&symbol).unwrap();
  assert_eq!(symbol["answer"]["outcome"], "found");
  assert_eq!(symbol["answer"].get("counterparts"), None);
  assert_eq!(symbol["answer"].get("also_change"), None);
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_fuse_doc_sections_into_atlas_search_and_answer_only_docs_for_kind_doc() {
  let harness = start_docs_corpus().await;

  let fused: Value = serde_json::from_str(&harness.text("atlas_search", json!({ "query": "backoff", "format": "json" })).await).unwrap();
  let only: Value = serde_json::from_str(&harness.text("atlas_search", json!({ "query": "backoff", "kind": "doc", "format": "json" })).await).unwrap();
  let coded: Value = serde_json::from_str(&harness.text("atlas_search", json!({ "query": "backoff", "kind": "function", "format": "json" })).await).unwrap();
  let listed: Value = serde_json::from_str(&harness.text("atlas_search", json!({ "query": "backoff", "kind": "function,doc", "format": "json" })).await).unwrap();
  let refused = harness.failure("atlas_search", json!({ "query": "backoff", "kind": "function,Docs" })).await;

  let docs = |answer: &Value| -> Vec<(String, u64)> {
    answer["answer"]["docs"]
      .as_array()
      .map(|docs| docs.iter().map(|doc| (doc["path"].as_str().unwrap().to_owned(), doc["rank"].as_u64().unwrap())).collect())
      .unwrap_or_default()
  };
  assert_eq!(docs(&fused), [("README.md".to_owned(), 2), (".code-atlas/notes/retries.md".to_owned(), 4)]);
  assert_eq!(fused["answer"]["docs"][0]["kind"], "doc");
  assert_eq!(docs(&only), [("README.md".to_owned(), 1), (".code-atlas/notes/retries.md".to_owned(), 2)]);
  assert_eq!(only["answer"]["strong"], json!([]));
  assert_eq!(docs(&coded), []);
  assert_eq!(docs(&listed), docs(&fused));
  assert!(
    refused
      .contains("unknown kind \"Docs\"; kinds are snake_case, e.g. function, class, api_endpoint, or doc for repository docs and notes, alone or in a list such as function,doc"),
    "{refused}"
  );
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_list_notes_and_doc_sections_in_atlas_symbol_and_atlas_impact_only_when_some_mention_the_target() {
  let harness = start_docs_corpus().await;

  let symbol = harness.text("atlas_symbol", json!({ "target": "backoffDelay" })).await;
  let quiet = harness.text("atlas_symbol", json!({ "target": "ShelfOptions" })).await;
  let impact = harness.text("atlas_impact", json!({ "targets": ["shelf-web:src/lib/backoff.ts"] })).await;

  assert!(
    symbol.ends_with(
      "NOTES (1 of 1): notes anchored to it\n  shelf-web:.code-atlas/notes/retries.md > Retry budget [unstamped] on backoffDelay\nDOCS (1 of 1): doc sections that mention it\n  shelf-web:README.md:5 > Shelf web > Retries [heuristic 0.70]\n"
    ),
    "{symbol}"
  );
  assert!(!quiet.contains("NOTES") && !quiet.contains("DOCS"), "{quiet}");
  assert!(
    impact.contains("NOTES (1 of 1): notes anchored to it\n  shelf-web:.code-atlas/notes/retries.md > Retry budget [unstamped] on backoffDelay\n"),
    "{impact}"
  );
  assert!(
    impact.contains("DOCS (1 of 1): doc sections that mention it\n  shelf-web:README.md:5 > Shelf web > Retries [heuristic 0.70]\n"),
    "{impact}"
  );
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_answer_the_path_between_two_targets_with_atlas_explain_to() {
  let harness = start_docs_corpus().await;

  let path = harness.text("atlas_explain", json!({ "target": "loadWithBackoff", "to": "ShelfClient" })).await;
  let provenance = harness.text("atlas_explain", json!({ "target": "loadWithBackoff" })).await;

  assert!(
    path.ends_with(
      "path between loadWithBackoff function shelf-web:src/lib/backoff.ts:7 and ShelfClient class shelf-web:src/lib/shelf-client.ts:10: 2 hops, cost 2\n  loadWithBackoff function shelf-web:src/lib/backoff.ts:7\n  calls createShelfClient function shelf-web:src/lib/shelf-client.ts:6 ([resolved])\n  instantiates ShelfClient class shelf-web:src/lib/shelf-client.ts:10 ([resolved])\n"
    ),
    "{path}"
  );
  assert!(!provenance.contains("path between"), "{provenance}");
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
  assert!(harness.failure("atlas_similar", json!({})).await.contains("exactly one of target, snippet or code"));
  assert!(
    harness
      .failure("atlas_similar", json!({ "target": "a", "snippet": "b" }))
      .await
      .contains("exactly one of target, snippet or code")
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
async fn should_search_a_directory_given_as_the_repo() {
  let harness = start(&ACME_REPOS).await;

  let relative = harness.text("atlas_search", json!({ "query": "remove user", "repo": "src/users" })).await;
  let qualified = harness.text("atlas_search", json!({ "query": "find order", "repo": "acme-api:src/orders/" })).await;
  let unknown = harness.failure("atlas_search", json!({ "query": "orders", "repo": "src/invoices" })).await;

  let header = |text: &str| text.lines().find(|line| line.starts_with("search ")).unwrap_or_default().to_owned();
  assert!(header(&relative).starts_with("search \"remove user\" in acme-api:src/users/: "), "{relative}");
  assert!(header(&qualified).starts_with("search \"find order\" in acme-api:src/orders/: "), "{qualified}");
  for (text, dir) in [(&relative, "acme-api:src/users/"), (&qualified, "acme-api:src/orders/")] {
    let hits: Vec<&str> = text.lines().filter(|line| line.contains(" score=")).collect();
    assert!(!hits.is_empty(), "{text}");
    assert!(hits.iter().all(|line| line.contains(dir)), "{text}");
  }
  assert!(unknown.contains("repo \"src/invoices\" is not in project \"acme\""), "{unknown}");
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_never_answer_about_another_project() {
  let harness = start(&ACME_REPOS).await;

  let search = harness
    .text("atlas_search", json!({ "query": "handle_order_created requireGlobexKey registerOrderRoutes" }))
    .await;
  let symbol = harness.text("atlas_symbol", json!({ "target": "handle_order_created" })).await;
  let file = harness.text("atlas_symbol", json!({ "target": "globex-worker:src/consumer.rs", "mode": "source" })).await;
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
  let symbol = harness.text("atlas_symbol", json!({ "target": "zz_brand_new_marker_fn", "mode": "source" })).await;
  let changes = harness.text("atlas_status", json!({ "since_run": 0 })).await;

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

  let edited = tokio::time::timeout(std::time::Duration::from_secs(120), async {
    let mut touched = std::time::Instant::now();
    loop {
      let text = harness.text("atlas_search", json!({ "query": "zz_watched_marker_fn" })).await;
      if text.contains("acme-billing:src/money.rs") {
        return text;
      }
      if touched.elapsed() >= std::time::Duration::from_secs(2) {
        fs::write(&money, fs::read(&money).unwrap()).unwrap();
        touched = std::time::Instant::now();
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
  assert!(
    text.contains("narrow the query (limit/depth/kind/repo) or use atlas_symbol mode=source on a specific symbol]"),
    "{text}"
  );
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
      .contains("budget must be at least 500 tokens, got 100")
  );
  let clamped = harness
    .text("atlas_context", json!({ "task": "convert currency string to cents", "budget": 1000000 }))
    .await;
  assert!(clamped.contains(" of 4000 tokens)"), "{clamped}");
  let default = harness.text("atlas_context", json!({ "task": "convert currency string to cents" })).await;
  assert!(default.contains(" of 4000 tokens)"), "{default}");
  let smallest = harness.text("atlas_context", json!({ "task": "convert currency string to cents", "budget": 500 })).await;
  assert!(smallest.contains(" of 500 tokens)"), "{smallest}");
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_cap_symbol_and_read_bodies_and_say_how_to_read_the_rest() {
  let harness = start_limited(&ACME_REPOS, 100_000, 3).await;

  let read = harness.text("atlas_symbol", json!({ "target": "toCents", "mode": "source" })).await;
  let symbol = harness.text("atlas_symbol", json!({ "target": "toCents", "body": true })).await;
  let structured = harness.text("atlas_symbol", json!({ "target": "toCents", "mode": "source", "format": "json" })).await;

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

  for mode in ["source", "info"] {
    for target in ["../../etc/passwd", "/etc/passwd", "file:globex-api:src/server.ts", "globex-api:src/server.ts"] {
      let text = harness.text("atlas_symbol", json!({ "target": target, "mode": mode })).await;
      assert!(text.contains("not found"), "{mode} {target}: {text}");
      assert!(!text.contains("root:"), "{mode} {target}: {text}");
      assert!(!text.replace(target, "").contains("globex"), "{mode} {target}: {text}");
    }
  }
  harness.stop().await.unwrap();
}

const FULL_SURFACE_BUDGET: usize = 14_400;
const CORE_SURFACE_BUDGET: usize = 9_000;
const READ_ONLY_SURFACE_BUDGET: usize = 6_500;

async fn start_profile(tools: ToolsSetting) -> Harness {
  start_configured(&ACME_REPOS, |_| {}, |options| options.tools = tools).await
}

async fn surface_bytes(harness: &Harness) -> (usize, usize) {
  let tools = harness.client.list_all_tools().await.unwrap();
  let listed = serde_json::to_string(&json!({ "tools": tools })).unwrap();
  let instructions = harness.client.peer_info().unwrap().instructions.clone().unwrap();
  (instructions.len(), listed.len())
}

#[tokio::test]
async fn should_keep_every_profile_within_its_byte_budget() {
  for (setting, budget) in [
    (ToolsSetting::Full, FULL_SURFACE_BUDGET),
    (ToolsSetting::Core, CORE_SURFACE_BUDGET),
    (ToolsSetting::ReadOnly, READ_ONLY_SURFACE_BUDGET),
  ] {
    let harness = start_profile(setting.clone()).await;
    let (instructions, listed) = surface_bytes(&harness).await;
    assert!(
      instructions + listed <= budget,
      "{setting:?}: instructions {instructions} + tools/list {listed} = {} bytes, over {budget}",
      instructions + listed
    );
    harness.stop().await.unwrap();
  }
}

#[tokio::test]
async fn should_serve_only_the_tools_of_the_profile_with_instructions_rendered_for_it() {
  let harness = start_profile(ToolsSetting::Core).await;
  let core = ToolProfile::from_setting(&ToolsSetting::Core).unwrap();

  let tools = harness.client.list_all_tools().await.unwrap();
  let instructions = harness.client.peer_info().unwrap().instructions.clone().unwrap();
  let hidden = harness.call("atlas_similar", json!({"target": "toCents"})).await;
  let served = harness.call("atlas_search", json!({"query": "toCents"})).await;

  let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
  assert_eq!(names, guidance::CORE_TOOLS);
  let read_only: Vec<&str> = tools
    .iter()
    .filter(|tool| tool.annotations.as_ref().and_then(|annotations| annotations.read_only_hint) == Some(true))
    .map(|tool| tool.name.as_ref())
    .collect();
  assert_eq!(read_only, core.read_only_tools());
  assert_eq!(instructions, guidance::instructions(&core));
  assert_eq!(hidden.is_error, Some(true));
  assert_eq!(text_of(&hidden), "atlas_similar is not in profile core; enable with [mcp] tools");
  assert_ne!(served.is_error, Some(true), "{}", text_of(&served));
  assert!(!harness.server.telemetry().tools.contains_key("atlas_similar"));
  harness.stop().await.unwrap();
}

#[test]
fn should_refuse_to_open_with_an_unknown_tool_in_the_list() {
  let scratch = tempfile::tempdir().unwrap();
  let home = atlas_project::AtlasHome::rooted_at(scratch.path().join("home"));
  let options = atlas_mcp::ServeOptions::new(home, scratch.path()).with_tools(ToolsSetting::Only(vec!["atlas_changes".to_owned()]));

  let error = atlas_mcp::Server::open(&options).err().unwrap();

  assert!(
    error
      .to_string()
      .starts_with("unknown tool \"atlas_changes\"; expected the profile core, read-only or full, or a list of tools from: atlas_api, "),
    "{error}"
  );
}

#[tokio::test]
async fn should_keep_tool_schemas_free_of_schema_noise() {
  let harness = start(&ACME_REPOS).await;

  let tools = harness.client.list_all_tools().await.unwrap();
  for tool in &tools {
    let schema = serde_json::to_string(&tool.input_schema).unwrap();
    for forbidden in ["$schema", "$ref", "$defs", "null"] {
      assert!(!schema.contains(forbidden), "{} schema contains {forbidden}: {schema}", tool.name);
    }
  }
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_rebuild_the_detectors_when_a_route_family_changes_while_running() {
  let harness = start(&["acme-api"]).await;
  let api = common::repo_path(harness.scratch.path(), "acme-api");
  fs::write(
    api.join("src/declared.controller.ts"),
    "import { Api, Read } from '@acme/http';\n\n@Api('declared')\nexport class DeclaredController {\n  @Read()\n  list() {\n    return [];\n  }\n}\n",
  )
  .unwrap();
  let family = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n";
  fs::create_dir_all(harness.home.detectors_dir()).unwrap();
  fs::write(harness.home.detectors_dir().join("acme-http.toml"), family).unwrap();
  let mut registry = Registry::load(&harness.home).unwrap();
  let mut settings = registry.stored_settings("acme").unwrap().clone();
  settings.detect.route_families = vec!["acme-http".to_owned()];
  registry.set_settings("acme", settings).unwrap();

  let enabled = harness.text("atlas_status", json!({})).await;
  let first = harness.text("atlas_api", json!({ "endpoint": "GET /declared" })).await;
  fs::write(harness.home.detectors_dir().join("acme-http.toml"), family.replace("\"GET\"", "\"PUT\"")).unwrap();
  let changed = harness.text("atlas_api", json!({ "endpoint": "PUT /declared" })).await;

  assert!(
    enabled.contains("route family acme-http: 1 controller, 1 route") && !enabled.contains("DEGRADED"),
    "{enabled}"
  );
  assert!(first.contains("GET /declared") && !first.contains("DEGRADED"), "{first}");
  assert!(
    changed.contains("PUT /declared") && !changed.contains("GET /declared") && !changed.contains("DEGRADED"),
    "{changed}"
  );
  harness.stop().await.unwrap();
}
