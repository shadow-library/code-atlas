mod common;

use std::fs;

use atlas_ide::{IdeError, LaunchSpec, ReferencesRequest, TsServer, TsServerOrigin};
use atlas_lsp::ServerKind;
use common::*;
use serde_json::json;

const MONEY: &str = "export function toCents(amount: string): number {\n  return Math.round(Number(amount) * 100);\n}\n";
const LIB: &str = "pub fn answer() -> u32 {\n  42\n}\n\npub fn twice() -> u32 {\n  answer() * 2\n}\n";

fn web() -> Fixture {
  Fixture::new(&[("web", &[("package.json", "{\"name\": \"web\"}\n"), ("tsconfig.json", "{}\n"), ("src/money.ts", MONEY)])])
}

fn core() -> Fixture {
  Fixture::new(&[(
    "core",
    &[("Cargo.toml", "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"), ("src/lib.rs", LIB)],
  )])
}

#[test]
fn should_refuse_and_retire_a_typescript_server_that_loaded_typescript_7() {
  let fixture = web();
  let command = fixture.counted("ts", &ts_ready_with("7.0.2", capabilities(json!({}))));
  fixture.serve(ServerKind::TypeScript, command);

  let first = fixture.ide.outline(&lsp_outline("src/money.ts")).unwrap_err();
  let second = fixture.ide.outline(&lsp_outline("src/money.ts")).unwrap_err();

  let expected = "typescript-language-server is unavailable: TypeScript 7 has no tsserver; install typescript@5 alongside or use the bundled one; retry with source='graph' for tagged graph results";
  assert_eq!(first.to_string(), expected);
  assert_eq!(second.to_string(), expected);
  assert_eq!(fixture.spawns("ts"), 1);
  assert!(fixture.ide.status().units.is_empty(), "{:?}", fixture.ide.status());
}

#[test]
fn should_show_the_reported_typescript_version_and_warn_when_it_contradicts_the_workspace_choice() {
  let fixture = web();
  let command = fixture.script(
    "ts",
    &with(
      ts_ready(capabilities(json!({}))),
      [json!({"canned": {"method": "textDocument/documentSymbol", "result": []}})],
    ),
  );
  fixture.launcher.set(
    ServerKind::TypeScript,
    LaunchSpec {
      tsserver: Some(TsServer {
        path: Some(fixture.file("web", "node_modules/typescript/lib/tsserver.js")),
        version: "5.4.5".to_owned(),
        origin: TsServerOrigin::Workspace { repo_root: fixture.root("web") },
      }),
      ..launch(ServerKind::TypeScript, command)
    },
  );

  let response = fixture.ide.outline(&lsp_outline("src/money.ts")).unwrap();

  assert_eq!(
    text(&response),
    format!(
      concat!(
        "outline web:src/money.ts  (typescript-language-server 5.1.3, TypeScript 5.9.3) [compiler]\n",
        "  (no symbols)\n",
        "warnings:\n",
        "  discovery chose the workspace TypeScript 5.4.5 in {}, but the server loaded TypeScript 5.9.3\n",
      ),
      fixture.root("web").display()
    )
  );
}

#[test]
fn should_caveat_a_project_configuration_change_the_typescript_server_cannot_confirm() {
  let fixture = web();
  let command = fixture.script(
    "ts",
    &with(
      ts_ready(capabilities(json!({}))),
      [json!({"canned": {"method": "textDocument/documentSymbol", "result": []}})],
    ),
  );
  fixture.serve(ServerKind::TypeScript, command);
  fixture.ide.outline(&lsp_outline("src/money.ts")).unwrap();
  fs::write(fixture.file("web", "tsconfig.json"), "{\"compilerOptions\": {\"strict\": true}}\n").unwrap();

  let changed = fixture.ide.outline(&lsp_outline("src/money.ts")).unwrap();
  let still = fixture.ide.outline(&lsp_outline("src/money.ts")).unwrap();

  let caveat = "  the language server may not have picked up changes to web:tsconfig.json (tsserver relies on its own file watchers); results involving them may be stale\n";
  assert!(text(&changed).ends_with(&format!("caveats:\n{caveat}")), "{}", text(&changed));
  assert!(text(&still).ends_with(&format!("caveats:\n{caveat}")), "{}", text(&still));
}

#[test]
fn should_tag_an_answer_partial_while_rust_analyzer_is_busy() {
  let fixture = core();
  let command = fixture.script(
    "ra",
    &with(
      rust_ready(capabilities(json!({}))),
      [
        json!({"recv": {"method": "textDocument/didOpen"}}),
        json!({"recv": {"method": "textDocument/documentSymbol"}}),
        json!({"reply": {"method": "textDocument/documentSymbol", "result": []}}),
        json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": false, "message": "Building 3/10"}}}),
        json!({"canned": {"method": "textDocument/references", "result": []}}),
      ],
    ),
  );
  fixture.serve(ServerKind::RustAnalyzer, command);
  fixture.ide.outline(&lsp_outline("src/lib.rs")).unwrap();
  assert!(wait_until(|| fixture
    .ide
    .status()
    .units
    .iter()
    .any(|unit| unit.state.as_deref() == Some("indexing: Building 3/10"))));

  let response = fixture.ide.references(&ReferencesRequest::new("answer")).unwrap();

  assert_eq!(
    text(&response),
    concat!(
      "references answer function core:src/lib.rs:1:8  (rust-analyzer 5.1.3) [compiler]\n",
      "PARTIAL: server busy: indexing: Building 3/10; the answer may be incomplete\n",
      "  (no references)\n",
      "outside the language server's view:\n",
      "  not in the server's project [resolved]  core:src/lib.rs:6:3  twice  answer() * 2\n",
    )
  );
}

#[test]
fn should_warn_on_every_answer_after_rust_analyzer_rewrote_the_lock_file() {
  let fixture = core();
  let lib_uri = fixture.uri("core", "src/lib.rs");
  let command = fixture.script(
    "ra",
    &with(
      rust_ready(capabilities(json!({}))),
      [json!({"canned": {"method": "textDocument/references", "result": [location(&lib_uri, (5, 2), (5, 8))]}})],
    ),
  );
  fixture.serve(ServerKind::RustAnalyzer, command);
  let before = fixture.ide.references(&ReferencesRequest::new("answer")).unwrap();
  fs::write(fixture.file("core", "Cargo.lock"), "version = 4\n").unwrap();

  let after = fixture.ide.references(&ReferencesRequest::new("answer")).unwrap();

  assert!(!text(&before).contains("warnings:"), "{}", text(&before));
  assert!(text(&after).contains("  twice  src/lib.rs:6:3  answer() * 2\n"), "{}", text(&after));
  assert!(
    text(&after).ends_with("warnings:\n  rust-analyzer rewrote core:Cargo.lock since it started (cargo updated the lock file); code-atlas does not restore it\n"),
    "{}",
    text(&after)
  );
  assert_eq!(
    fixture.ide.status().units[0].warnings,
    vec!["core:Cargo.lock was rewritten by rust-analyzer since it started".to_owned()]
  );
}

#[test]
fn should_refuse_a_rust_file_outside_every_cargo_package_and_a_path_outside_the_project() {
  let fixture = Fixture::new(&[
    (
      "core",
      &[("Cargo.toml", "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"), ("src/lib.rs", LIB)],
    ),
    ("loose", &[("src/x.rs", "pub fn x() {}\n")]),
  ]);
  fixture.serve(ServerKind::RustAnalyzer, fixture.script("ra", &rust_ready(capabilities(json!({})))));

  let loose = fixture.ide.outline(&lsp_outline("loose:src/x.rs")).unwrap_err();
  let outside = fixture.ide.outline(&lsp_outline("/nowhere/x.rs")).unwrap_err();

  assert_eq!(
    loose.to_string(),
    "loose:src/x.rs is not covered by rust-analyzer: no Cargo workspace or package found by the index contains it"
  );
  assert!(matches!(outside, IdeError::OutsideProject { .. }), "{outside}");
}
