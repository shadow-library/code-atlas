mod e2e_support;

use std::fs;
use std::path::PathBuf;

use e2e_support::{Sandbox, Server, binary, repos_of, shorts, text_of};
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

fn acme_and_globex(sandbox: &Sandbox) -> PathBuf {
  let acme = sandbox.acme();
  let globex: Vec<PathBuf> = ["globex-api", "globex-worker"]
    .iter()
    .map(|repo| sandbox.copy_repo("globex", repo))
    .chain([acme.join("acme-sdk")])
    .collect();
  sandbox.create_project("globex", &globex);
  for project in ["acme", "globex"] {
    sandbox.cli_ok(&sandbox.work(), &["index", "--project", project]);
  }
  acme
}

async fn assert_only_acme(server: &Server) {
  let calls: [(&'static str, Value); 8] = [
    ("atlas_search", json!({ "query": "formatMoney" })),
    ("atlas_search", json!({ "query": "UsersService findOne" })),
    ("atlas_symbol", json!({ "target": "OrdersService.create" })),
    ("atlas_api", json!({ "endpoint": "GET /users/{id}" })),
    ("atlas_impact", json!({ "targets": ["formatMoney"] })),
    ("atlas_callers", json!({ "target": "to_cents" })),
    ("atlas_overview", json!({})),
    ("atlas_context", json!({ "task": "format money for an order and look up a user by id", "budget": 6000 })),
  ];
  for (tool, arguments) in calls {
    let document = server.json(tool, arguments.clone()).await;
    let repos = repos_of(&document["answer"]);
    assert!(!repos.is_empty(), "{tool} {arguments} returned no entities: {document:#}");
    assert!(repos.iter().all(|repo| repo.starts_with("acme-")), "{tool} {arguments} leaked {repos:?}");
  }
  let symbol = server.json("atlas_symbol", json!({ "target": "OrdersService.create" })).await;
  assert_eq!(
    symbol["answer"]["outcome"], "found",
    "globex has an OrdersService.create too; it must not make the target ambiguous: {symbol:#}"
  );
  let endpoint = server.json("atlas_api", json!({ "endpoint": "GET /users/{id}" })).await;
  assert_eq!(endpoint["answer"]["endpoints"]["total"], 1, "{endpoint:#}");
}

#[tokio::test(flavor = "multi_thread")]
async fn should_never_return_globex_data_from_the_acme_server() {
  let sandbox = Sandbox::new();
  let acme = acme_and_globex(&sandbox);
  let api_repo = acme.join("acme-api");

  let pinned = sandbox.serve(&sandbox.work(), &["mcp", "--project", "acme"]).await;
  assert_only_acme(&pinned).await;
  assert!(pinned.stop().await.success());

  let inferred = sandbox.serve(&api_repo, &["mcp"]).await;
  assert_only_acme(&inferred).await;
  assert!(inferred.stop().await.success());

  let globex = sandbox.serve(&sandbox.work(), &["mcp", "--project", "globex"]).await;
  let document = globex.json("atlas_search", json!({ "query": "formatMoney" })).await;
  let repos = repos_of(&document["answer"]);
  assert!(repos.contains(&"globex-api".to_owned()), "{document:#}");
  assert!(repos.iter().all(|repo| repo != "acme-api" && repo != "acme-web" && repo != "acme-billing"), "{repos:?}");
  assert!(globex.stop().await.success());
}

#[tokio::test(flavor = "multi_thread")]
async fn should_keep_answering_while_a_cli_index_runs_and_see_cli_edits() {
  let sandbox = Sandbox::new();
  let acme = sandbox.acme();
  let api_repo = acme.join("acme-api");
  sandbox.cli_ok(&api_repo, &["index"]);
  let server = sandbox.serve(&api_repo, &["mcp"]).await;

  let indexer = {
    let (bin, home, cwd) = (binary().to_path_buf(), sandbox.home().to_path_buf(), api_repo.clone());
    tokio::task::spawn_blocking(move || {
      (0..4)
        .map(|_| {
          std::process::Command::new(&bin)
            .args(["index", "--full"])
            .current_dir(&cwd)
            .env("CODE_ATLAS_HOME", &home)
            .env_remove("RUST_LOG")
            .output()
            .unwrap()
        })
        .collect::<Vec<_>>()
    })
  };
  let mut answers = Vec::new();
  for round in 0..24 {
    let (tool, arguments) = match round % 3 {
      0 => ("atlas_callers", json!({ "target": "formatMoney", "format": "json" })),
      1 => ("atlas_search", json!({ "query": "UsersService", "format": "json" })),
      _ => ("atlas_api", json!({ "endpoint": "GET /orders/{id}", "format": "json" })),
    };
    let result = server.call(tool, arguments).await;
    let text = text_of(&result);
    assert_ne!(result.is_error, Some(true), "{tool} failed while indexing: {text}");
    let document: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
      document["freshness"]["state"], "fresh",
      "{tool} answered degraded while another process indexed: {document:#}"
    );
    answers.push((tool, document["answer"].clone()));
  }
  for output in indexer.await.unwrap() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{}\n{stderr}", String::from_utf8_lossy(&output.stdout));
    assert!(!stderr.to_lowercase().contains("lock"), "{stderr}");
  }
  for (tool, answer) in answers.iter().filter(|(tool, _)| *tool == "atlas_callers") {
    let mut callers = shorts(&answer["groups"]);
    callers.sort();
    assert_eq!(callers, ["OrdersService.findOne", "ReportsService.exportCsv"], "{tool}: {answer:#}");
  }

  let money = api_repo.join("src/common/money.ts");
  let mut source = fs::read_to_string(&money).unwrap();
  source.push_str("\nexport function formatCentsCompact(cents: number): string {\n  return `${cents}c`;\n}\n");
  fs::write(&money, source).unwrap();
  let added = server.json("atlas_symbol", json!({ "target": "formatCentsCompact" })).await;
  assert_eq!(added["answer"]["outcome"], "found", "the server did not see an edit made by another process: {added:#}");
  assert!(added["freshness"]["files_updated"].as_u64().unwrap() >= 1, "{added:#}");
  assert!(server.stop().await.success());
}

#[tokio::test(flavor = "multi_thread")]
async fn should_serve_the_server_that_connect_claude_registers() {
  let sandbox = Sandbox::new();
  let api_repo = sandbox.acme().join("acme-api");

  let connected = sandbox.cli_ok(&api_repo, &["connect", "claude"]);
  assert!(connected.stdout.contains(".mcp.json"), "{}", connected.stdout);

  let config: Value = serde_json::from_str(&fs::read_to_string(api_repo.join(".mcp.json")).unwrap()).unwrap();
  let entry = &config["mcpServers"]["code-atlas"];
  let command = PathBuf::from(entry["command"].as_str().unwrap());
  assert_eq!(command, binary());
  assert_eq!(entry["args"], json!(["mcp", "--project", "acme"]));
  let args: Vec<&str> = entry["args"].as_array().unwrap().iter().map(|arg| arg.as_str().unwrap()).collect();
  let server = Server::spawn(&command, &args, &api_repo, sandbox.home()).await;

  let info = server.client.peer_info().unwrap();
  let instructions = info.instructions.clone().unwrap_or_default();
  assert!(instructions.starts_with("Code Atlas answers questions about ONE indexed project"), "{instructions}");
  assert!(instructions.contains("atlas_context"), "{instructions}");
  let mut tools: Vec<String> = server.client.list_all_tools().await.unwrap().into_iter().map(|tool| tool.name.to_string()).collect();
  tools.sort();
  assert_eq!(tools, TOOLS);
  let pack = server
    .text(
      "atlas_context",
      json!({ "task": "fix the lookup of a single user by id in UsersService.findOne", "budget": 3000 }),
    )
    .await;
  assert!(pack.starts_with("fresh run="), "{pack}");
  assert!(pack.contains("MUST READ"), "{pack}");
  assert!(pack.contains("UsersService.findOne method acme-api:src/users/users.service.ts:18"), "{pack}");
  let status = server.stop().await;
  assert!(status.success(), "the server exited with {status} after the client closed stdin");
}
