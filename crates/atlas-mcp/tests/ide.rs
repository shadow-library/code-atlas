mod common;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use atlas_ide::{IdeError, IdeResult, LaunchRequest, LaunchSpec, Launcher, TsServer, TsServerOrigin};
use atlas_lsp::{Launch, ServerCommand, ServerKind, canonical_path, uri_from_path};
use common::{ACME_REPOS, Harness, repo_path, start_configured};
use serde_json::{Value, json};
use tempfile::TempDir;

const INSTALL_HINT: &str = "typescript-language-server not found. Install it (npm i -g typescript-language-server typescript@5)";

#[derive(Default)]
struct ScriptedLauncher {
  specs: Mutex<HashMap<ServerKind, LaunchSpec>>,
}

impl Launcher for ScriptedLauncher {
  fn launch(&self, request: &LaunchRequest<'_>) -> IdeResult<LaunchSpec> {
    self.specs.lock().unwrap().get(&request.kind).cloned().ok_or_else(|| IdeError::Unavailable {
      server: request.kind.name().to_owned(),
      reason: INSTALL_HINT.to_owned(),
      alternative: None,
    })
  }
}

struct Ide {
  harness: Harness,
  launcher: Arc<ScriptedLauncher>,
  scripts: TempDir,
}

async fn start_ide() -> Ide {
  let launcher = Arc::new(ScriptedLauncher::default());
  let handle = Arc::clone(&launcher);
  let harness = start_configured(&ACME_REPOS, |_| {}, move |options| *options = options.clone().with_launcher(handle)).await;
  Ide {
    harness,
    launcher,
    scripts: tempfile::tempdir().unwrap(),
  }
}

impl Ide {
  fn file(&self, repo: &str, path: &str) -> PathBuf {
    canonical_path(&repo_path(self.harness.scratch.path(), repo)).unwrap().join(path)
  }

  fn uri(&self, repo: &str, path: &str) -> String {
    uri_from_path(&self.file(repo, path)).unwrap()
  }

  fn record(&self, name: &str) -> Value {
    json!({"record": self.scripts.path().join(format!("{name}.log"))})
  }

  fn serve(&self, capabilities: Value, canned: &[(&str, Value)]) {
    let mut steps = vec![
      json!({"recv": {"method": "initialize"}}),
      json!({"reply": {"method": "initialize", "result": {"capabilities": capabilities, "serverInfo": {"name": "typescript-language-server", "version": "5.1.3"}}}}),
      json!({"recv": {"method": "initialized"}}),
      json!({"notify": {"method": "$/typescriptVersion", "params": {"version": "5.9.3", "source": "bundled"}}}),
      self.record("ts"),
    ];
    steps.extend(canned.iter().map(|(method, result)| json!({"canned": {"method": method, "result": result}})));
    let script = self.scripts.path().join("ts.jsonl");
    fs::write(&script, steps.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
    self.launcher.specs.lock().unwrap().insert(
      ServerKind::TypeScript,
      LaunchSpec {
        command: ServerCommand::new(atlas_test_lsp::fake_lsp_path()).arg(script),
        launch: Launch::Installed,
        server_version: "0.0-discovered".to_owned(),
        tsserver: Some(TsServer {
          path: None,
          version: "5.9.3".to_owned(),
          origin: TsServerOrigin::Bundled,
        }),
      },
    );
  }
}

fn capabilities() -> Value {
  json!({
    "textDocumentSync": 1,
    "referencesProvider": true,
    "definitionProvider": true,
    "implementationProvider": true,
    "documentSymbolProvider": true,
    "renameProvider": {"prepareProvider": true},
    "diagnosticProvider": {"interFileDependencies": true, "workspaceDiagnostics": false}
  })
}

fn range(start: (u32, u32), end: (u32, u32)) -> Value {
  json!({"start": {"line": start.0, "character": start.1}, "end": {"line": end.0, "character": end.1}})
}

fn recorded_methods(scripts: &Path, name: &str) -> Vec<String> {
  fs::read_to_string(scripts.join(format!("{name}.log")))
    .unwrap_or_default()
    .lines()
    .filter_map(|line| serde_json::from_str::<Value>(line).unwrap()["method"].as_str().map(str::to_owned))
    .collect()
}

fn position(text: &str, needle: &str, nth: usize) -> (u32, u32) {
  let offset = text.match_indices(needle).nth(nth).unwrap().0;
  let line = text[..offset].matches('\n').count() as u32;
  let line_start = text[..offset].rfind('\n').map_or(0, |at| at + 1);
  (line, text[line_start..offset].encode_utf16().count() as u32)
}

fn location(uri: &str, text: &str, needle: &str, nth: usize) -> Value {
  let (line, column) = position(text, needle, nth);
  json!({"uri": uri, "range": range((line, column), (line, column + needle.len() as u32))})
}

fn answer(text: &str) -> String {
  text.lines().skip(1).map(|line| format!("{line}\n")).collect()
}

fn without_plan(text: &str) -> String {
  let id = text.split("plan ").nth(1).and_then(|rest| rest.split(' ').next()).unwrap_or_default();
  text.replace(id, "<plan>")
}

fn plan_id(text: &str) -> String {
  text.split("plan ").nth(1).unwrap().split(' ').next().unwrap().to_owned()
}

const SERVER: &str = "(typescript-language-server 5.1.3, TypeScript 5.9.3)";

struct Sdk {
  money: String,
  money_uri: String,
  test: String,
  test_uri: String,
}

impl Ide {
  fn sdk(&self) -> Sdk {
    Sdk {
      money: fs::read_to_string(self.file("acme-sdk", "src/money.ts")).unwrap(),
      money_uri: self.uri("acme-sdk", "src/money.ts"),
      test: fs::read_to_string(self.file("acme-sdk", "src/money.test.ts")).unwrap(),
      test_uri: self.uri("acme-sdk", "src/money.test.ts"),
    }
  }
}

#[tokio::test]
async fn should_name_what_each_symbol_mode_takes_when_the_arguments_do_not_fit() {
  let ide = start_ide().await;
  let cases = [
    (json!({}), "mode info takes target (optional: body); target is missing"),
    (json!({"target": "a", "regex": "(a)"}), "mode info takes target (optional: body); regex is not allowed"),
    (
      json!({"mode": "find", "target": "a", "lines": "1-2"}),
      "mode find takes target (optional: path, depth, body, substring, kinds, exclude_kinds, max_matches); lines is not allowed",
    ),
    (
      json!({"mode": "find", "target": "a", "kinds": ["nope"]}),
      "invalid query: nope is not a symbol kind; find searches code symbols",
    ),
    (json!({"mode": "outline"}), "mode outline needs path or target"),
    (json!({"mode": "outline", "path": "a.ts", "target": "b.ts"}), "mode outline takes path or target, not both"),
    (
      json!({"mode": "outline", "path": "a.ts", "lines": "1-2"}),
      "mode outline takes path or target (optional: depth, source); lines is not allowed",
    ),
    (
      json!({"mode": "source", "target": "a", "source": "graph"}),
      "mode source takes target (optional: context_lines); source is not allowed",
    ),
    (json!({"mode": "source", "target": "a", "context_lines": 101}), "context_lines must be at most 100, got 101"),
    (json!({"mode": "references"}), "mode references takes target (optional: source); target is missing"),
    (
      json!({"mode": "declaration", "target": "a", "path": "b"}),
      "mode declaration takes target, or path with regex; path is not allowed",
    ),
    (json!({"mode": "declaration", "regex": "(a)"}), "mode declaration takes path, regex; path is missing"),
    (
      json!({"mode": "implementations", "target": "a", "depth": 1}),
      "mode implementations takes target; depth is not allowed",
    ),
    (
      json!({"mode": "hierarchy", "target": "a", "body": true}),
      "mode hierarchy takes target (optional: depth); body is not allowed",
    ),
    (
      json!({"mode": "diagnostics", "path": "a.ts", "lines": "5-1"}),
      "lines must be START-END with 1-based START <= END, got \"5-1\"",
    ),
    (
      json!({"mode": "diagnostics", "path": "a.ts", "body": true}),
      "mode diagnostics takes path or target (optional: lines, min_severity); body is not allowed",
    ),
  ];

  for (arguments, expected) in cases {
    assert_eq!(ide.harness.failure("atlas_symbol", arguments.clone()).await, expected, "{arguments}");
  }
  ide.harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_name_what_each_edit_operation_takes_when_the_arguments_do_not_fit() {
  let ide = start_ide().await;
  let cases = [
    (
      json!({"operation": "rename", "target": "a", "new_name": "b", "pattern": "x"}),
      "operation rename takes target, new_name (optional: cross_repo, allow_unverified, dry_run); pattern is not allowed",
    ),
    (
      json!({"operation": "rename", "target": "a"}),
      "operation rename takes target, new_name (optional: cross_repo, allow_unverified, dry_run); new_name is missing",
    ),
    (
      json!({"operation": "replace_body", "target": "a"}),
      "operation replace_body takes target, text (optional: dry_run); text is missing",
    ),
    (
      json!({"operation": "insert_after", "target": "a", "text": "x", "path": "p"}),
      "operation insert_after takes target, text (optional: dry_run); path is not allowed",
    ),
    (
      json!({"operation": "safe_delete", "target": "a", "text": "x"}),
      "operation safe_delete takes target (optional: dry_run); text is not allowed",
    ),
    (
      json!({"operation": "replace_content", "path": "p", "pattern": "x", "text": "y", "include": ["a"]}),
      "operation replace_content takes path, pattern, text (optional: regex, all, dry_run); include is not allowed",
    ),
    (
      json!({"operation": "replace_in_files", "pattern": "x", "text": "y", "all": true}),
      "operation replace_in_files takes pattern, text (optional: regex, include, exclude, occurrences, expected_count, dry_run); all is not allowed",
    ),
    (
      json!({"operation": "apply", "plan": "p-1", "dry_run": true}),
      "operation apply takes plan; dry_run is not allowed",
    ),
    (json!({"operation": "apply"}), "operation apply takes plan; plan is missing"),
  ];

  for (arguments, expected) in cases {
    assert_eq!(ide.harness.failure("atlas_edit", arguments.clone()).await, expected, "{arguments}");
  }
  assert_eq!(ide.harness.server.telemetry().tools["atlas_edit"].errors, 9);
  ide.harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_serve_the_graph_modes_without_a_language_server() {
  let ide = start_ide().await;

  let info = ide.harness.text("atlas_symbol", json!({"target": "toCents"})).await;
  let find = ide.harness.text("atlas_symbol", json!({"target": "toCents", "mode": "find"})).await;
  let outline = ide.harness.text("atlas_symbol", json!({"path": "acme-sdk:src/money.ts", "mode": "outline"})).await;
  let source = ide.harness.text("atlas_symbol", json!({"target": "toCents", "mode": "source", "context_lines": 1})).await;
  let hierarchy = ide.harness.text("atlas_symbol", json!({"target": "UsersService", "mode": "hierarchy"})).await;

  assert!(info.lines().nth(1).unwrap().starts_with("toCents function"), "{info}");
  assert_eq!(
    answer(&find),
    concat!(
      "find \"toCents\": 2 matches\n",
      "  toCents function acme-sdk:src/money.ts:1  function toCents(amount: string): number\n",
      "  toCents test_suite acme-sdk:src/money.test.ts:5  describe('toCents')\n",
    )
  );
  assert_eq!(
    answer(&outline),
    "outline acme-sdk:src/money.ts source=graph (from the index, syntactic; for the exact language-server outline use source=lsp)\n  toCents function :1  function toCents(amount: string): number\n"
  );
  assert!(source.contains("export function toCents(amount: string): number {"), "{source}");
  assert_eq!(
    answer(&hierarchy),
    "hierarchy UsersService class acme-api:src/users/users.service.ts:14\nsupertypes:\n  (none)\nsubtypes:\n  (none)\n"
  );
  ide.harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_answer_exact_references_with_the_compiler_tag_and_server_versions() {
  let ide = start_ide().await;
  let sdk = ide.sdk();
  ide.serve(
    capabilities(),
    &[(
      "textDocument/references",
      json!([location(&sdk.test_uri, &sdk.test, "toCents", 1), location(&sdk.money_uri, &sdk.money, "toCents", 0)]),
    )],
  );

  let text = ide
    .harness
    .text("atlas_symbol", json!({"target": "acme-sdk:src/money.ts:1:17", "mode": "references"}))
    .await;

  let lines: Vec<&str> = text.lines().collect();
  assert_eq!(lines[1], format!("references at acme-sdk:src/money.ts:1:17  {SERVER} [compiler]"));
  assert_eq!(lines[2], "acme-sdk (2)");
  assert_eq!(lines[3], "  toCents (test)  src/money.test.ts:5:11  describe('toCents', () => {");
  assert_eq!(lines[4], "  toCents  src/money.ts:1:17  export function toCents(amount: string): number {");
  assert_eq!(lines[5], "outside the language server's view:");
  assert_eq!(
    lines[6],
    "  not in the server's project [resolved]  acme-sdk:src/index.ts:3:10  src/index.ts  export { toCents } from './money';"
  );
  assert!(
    lines.contains(&"  other repo [resolved]  acme-web:src/components/OrderForm.tsx:1:10  src/components/OrderForm.tsx  import { toCents } from '@acme/sdk';"),
    "{text}"
  );
  ide.harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_answer_declaration_implementations_outline_and_diagnostics_from_the_server() {
  let ide = start_ide().await;
  let sdk = ide.sdk();
  ide.serve(
    capabilities(),
    &[
      ("textDocument/definition", json!([location(&sdk.money_uri, &sdk.money, "toCents", 0)])),
      ("textDocument/declaration", json!([])),
      ("textDocument/implementation", json!([location(&sdk.test_uri, &sdk.test, "toCents", 1)])),
      (
        "textDocument/documentSymbol",
        json!([{"name": "toCents", "kind": 12, "range": range((0, 0), (6, 1)), "selectionRange": range((0, 16), (0, 23)), "detail": "(amount: string): number"}]),
      ),
      (
        "textDocument/diagnostic",
        json!({"kind": "full", "items": [{"range": range((1, 8), (1, 15)), "severity": 1, "code": 2322, "source": "ts", "message": "Type 'string' is not assignable to type 'number'."}]}),
      ),
    ],
  );

  let by_position = ide
    .harness
    .text("atlas_symbol", json!({"target": "acme-sdk:src/money.test.ts:7:12", "mode": "declaration"}))
    .await;
  let by_regex = ide
    .harness
    .text("atlas_symbol", json!({"path": "acme-sdk:src/money.ts", "mode": "declaration", "regex": "(toCents)\\("}))
    .await;
  let implementations = ide.harness.text("atlas_symbol", json!({"target": "toCents", "mode": "implementations"})).await;
  let outline = ide
    .harness
    .text("atlas_symbol", json!({"path": "acme-sdk:src/money.ts", "mode": "outline", "source": "lsp"}))
    .await;
  let diagnostics = ide
    .harness
    .text("atlas_symbol", json!({"target": "acme-sdk:src/money.ts", "mode": "diagnostics", "min_severity": "error"}))
    .await;

  assert_eq!(
    answer(&by_position),
    format!(
      "declaration at acme-sdk:src/money.test.ts:7:12  {SERVER} [compiler]\n  definition  toCents  acme-sdk:src/money.ts:1:17  export function toCents(amount: string): number {{\n"
    )
  );
  assert_eq!(
    answer(&by_regex),
    format!(
      "declaration at acme-sdk:src/money.ts:1:17  {SERVER} [compiler]\n  definition  toCents  acme-sdk:src/money.ts:1:17  export function toCents(amount: string): number {{\n"
    )
  );
  assert_eq!(
    answer(&implementations),
    format!(
      "implementations of toCents function acme-sdk:src/money.ts:1:17  {SERVER} [compiler]\n  acme-sdk  toCents (test)  src/money.test.ts:5:11  describe('toCents', () => {{\n"
    )
  );
  assert_eq!(
    answer(&outline),
    format!("outline acme-sdk:src/money.ts  {SERVER} [compiler]\n  toCents function 1:17  (amount: string): number\n")
  );
  assert_eq!(
    answer(&diagnostics),
    format!("diagnostics acme-sdk:src/money.ts  [pulled]  {SERVER} [compiler]\nerror (1)\n  toCents\n    2:9 2322 Type 'string' is not assignable to type 'number'.\n")
  );
  ide.harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_return_the_install_hint_and_the_graph_retry_hint_when_no_server_is_available() {
  let ide = start_ide().await;
  let hint = format!("typescript-language-server is unavailable: {INSTALL_HINT}");
  let retry = "; retry with source='graph' for tagged graph results";

  assert_eq!(
    ide.harness.failure("atlas_symbol", json!({"target": "toCents", "mode": "references"})).await,
    format!("{hint}{retry}")
  );
  assert_eq!(
    ide
      .harness
      .failure("atlas_symbol", json!({"path": "acme-sdk:src/money.ts", "mode": "outline", "source": "lsp"}))
      .await,
    format!("{hint}{retry}")
  );
  assert_eq!(
    ide
      .harness
      .failure("atlas_symbol", json!({"target": "acme-sdk:src/money.ts:1:17", "mode": "declaration"}))
      .await,
    hint
  );
  assert_eq!(ide.harness.failure("atlas_symbol", json!({"target": "toCents", "mode": "implementations"})).await, hint);
  assert_eq!(
    ide.harness.failure("atlas_symbol", json!({"path": "acme-sdk:src/money.ts", "mode": "diagnostics"})).await,
    hint
  );
  assert_eq!(
    ide
      .harness
      .failure("atlas_edit", json!({"operation": "rename", "target": "toCents", "new_name": "cents"}))
      .await,
    hint
  );
  assert!(
    ide
      .harness
      .text("atlas_symbol", json!({"target": "toCents", "mode": "references", "source": "graph"}))
      .await
      .contains("graph references are not exhaustive")
  );
  ide.harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_dry_run_a_rename_refuse_unverified_sites_and_apply_the_plan_with_allow_unverified() {
  let ide = start_ide().await;
  let sdk = ide.sdk();
  ide.serve(
    capabilities(),
    &[
      ("textDocument/prepareRename", json!({"range": range((0, 16), (0, 23)), "placeholder": "toCents"})),
      (
        "textDocument/rename",
        json!({"changes": {sdk.money_uri.clone(): [{"range": range((0, 16), (0, 23)), "newText": "cents"}]}}),
      ),
      ("textDocument/diagnostic", json!({"kind": "full", "items": []})),
      ("textDocument/documentSymbol", json!([])),
      ("textDocument/references", json!([])),
    ],
  );
  let rename = json!({"operation": "rename", "target": "acme-sdk:src/money.ts:1:17", "new_name": "cents", "dry_run": true});

  let dry = ide.harness.text("atlas_edit", rename.clone()).await;
  let refused = ide.harness.failure("atlas_edit", json!({"operation": "apply", "plan": plan_id(&dry)})).await;
  let allowed = ide
    .harness
    .text(
      "atlas_edit",
      json!({"operation": "rename", "target": "acme-sdk:src/money.ts:1:17", "new_name": "cents", "dry_run": true, "allow_unverified": true}),
    )
    .await;

  assert_eq!(fs::read_to_string(ide.file("acme-sdk", "src/money.ts")).unwrap(), sdk.money);
  assert_eq!(
    without_plan(&dry).lines().nth(1).unwrap(),
    format!("rename toCents -> cents  dry run, plan <plan> (apply within 15 min)  {SERVER}")
  );
  assert!(
    dry.contains(
      "UNVERIFIED: 6 site(s) the index resolves to this symbol in the same repo were not renamed by the language server; applying this plan refuses unless allow_unverified is set:"
    ),
    "{dry}"
  );
  assert!(
    dry.contains("  acme-sdk:src/money.ts  1\n    @@ -1,4 +1,4 @@\n    -export function toCents(amount: string): number {\n    +export function cents(amount: string): number {\n"),
    "{dry}"
  );
  assert!(dry.contains("not renamed, verify:\n"), "{dry}");
  assert!(refused.lines().nth(1).unwrap().starts_with("REFUSED: rename toCents -> cents: "), "{refused}");
  assert!(allowed.contains("allow_unverified is set, so applying this plan leaves them as they are:"), "{allowed}");

  let applied = ide.harness.text("atlas_edit", json!({"operation": "apply", "plan": plan_id(&allowed)})).await;

  assert_eq!(applied.lines().nth(1).unwrap(), format!("rename toCents -> cents  applied  {SERVER}"));
  assert_eq!(
    fs::read_to_string(ide.file("acme-sdk", "src/money.ts")).unwrap(),
    sdk.money.replace("function toCents", "function cents")
  );
  let again = ide.harness.failure("atlas_edit", json!({"operation": "apply", "plan": plan_id(&allowed)})).await;
  assert!(again.starts_with(&format!("unknown plan {}: ", plan_id(&allowed))), "{again}");
  ide.harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_apply_text_edits_without_a_server_and_report_the_refreshed_index() {
  let ide = start_ide().await;
  let before = fs::read_to_string(ide.file("acme-sdk", "src/money.ts")).unwrap();

  let dry = ide
    .harness
    .text(
      "atlas_edit",
      json!({"operation": "replace_in_files", "pattern": "const trimmed", "text": "const cleaned", "include": ["acme-sdk:src/money.ts"]}),
    )
    .await;
  let applied = ide
    .harness
    .text(
      "atlas_edit",
      json!({"operation": "replace_content", "path": "acme-sdk:src/money.ts", "pattern": "const trimmed", "text": "const cleaned"}),
    )
    .await;
  let status = ide.harness.text("atlas_status", json!({"since_run": 0})).await;

  assert!(dry.lines().nth(1).unwrap().contains("dry run (no occurrences or expected_count given), plan "), "{dry}");
  assert_eq!(applied.lines().nth(1).unwrap(), "replace_content acme-sdk:src/money.ts  applied");
  assert_eq!(
    fs::read_to_string(ide.file("acme-sdk", "src/money.ts")).unwrap(),
    before.replacen("const trimmed", "const cleaned", 1)
  );
  assert!(status.contains("analyzed: acme-sdk:src/money.ts"), "{status}");
  ide.harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_compare_draft_code_with_atlas_similar() {
  let ide = start_ide().await;
  let draft = "export function toCents(amount: string): number {\n  const trimmed = amount.trim();\n  const negative = trimmed.startsWith('-');\n  return negative ? -1 : 1;\n}\n";

  let text = ide.harness.text("atlas_similar", json!({"code": draft, "language": "ts"})).await;

  assert_eq!(text.lines().nth(1).unwrap(), "similar to draft fn toCents (typescript, 5 lines)");
  assert!(text.contains("  toCents function acme-sdk:src/money.ts:1 [ts]\n"), "{text}");
  assert_eq!(
    ide.harness.failure("atlas_similar", json!({"code": draft})).await,
    "code needs language: rust, ts, tsx or js"
  );
  assert_eq!(
    ide.harness.failure("atlas_similar", json!({"target": "toCents", "language": "ts"})).await,
    "language only goes with code"
  );
  assert_eq!(
    ide.harness.failure("atlas_similar", json!({"code": draft, "language": "go"})).await,
    "language must be rust, ts, tsx or js, got \"go\""
  );
  assert_eq!(
    ide.harness.failure("atlas_similar", json!({"target": "toCents", "code": draft, "language": "ts"})).await,
    "provide exactly one of target, snippet or code"
  );
  ide.harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_follow_http_hops_when_via_http_is_set() {
  let ide = start_ide().await;

  let text = ide.harness.text("atlas_callers", json!({"target": "toCents", "via_http": true})).await;
  let plain = ide.harness.text("atlas_callers", json!({"target": "toCents"})).await;

  assert!(text.lines().nth(1).unwrap().ends_with("depth=1 via_http"), "{text}");
  assert!(text.contains("  via_http found no HTTP hop: no reached symbol exposes an endpoint\n"), "{text}");
  assert!(!plain.contains("via_http"), "{plain}");
  ide.harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_report_language_servers_and_index_changes_through_atlas_status() {
  let ide = start_ide().await;

  let plain = ide.harness.text("atlas_status", json!({})).await;
  let since = ide.harness.text("atlas_status", json!({"since_run": 0})).await;
  let structured: Value = serde_json::from_str(&ide.harness.text("atlas_status", json!({"since_run": 0, "format": "json"})).await).unwrap();

  assert!(plain.contains("language servers: enabled, warm\n  (none running)\n"), "{plain}");
  assert!(!plain.contains("changes since run"), "{plain}");
  assert!(since.contains("changes since run 0 (index at run 1):\nrun 1 barrier ok\n  analyzed: "), "{since}");
  assert_eq!(structured["language_servers"], json!({"enabled": true, "mode": "warm", "units": []}));
  assert_eq!(structured["changes"]["runs"][0]["run"]["id"], 1);
  ide.harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_shut_the_language_servers_down_when_the_client_disconnects() {
  let ide = start_ide().await;
  let sdk = ide.sdk();
  ide.serve(
    capabilities(),
    &[
      ("textDocument/definition", json!([location(&sdk.money_uri, &sdk.money, "toCents", 0)])),
      ("textDocument/declaration", json!([])),
    ],
  );
  ide
    .harness
    .text("atlas_symbol", json!({"target": "acme-sdk:src/money.ts:1:17", "mode": "declaration"}))
    .await;
  let Ide { harness, scripts, .. } = ide;

  harness.stop().await.unwrap();

  let methods = recorded_methods(scripts.path(), "ts");
  assert_eq!(&methods[methods.len() - 2..], ["shutdown", "exit"], "{methods:?}");
}

#[tokio::test]
async fn should_shut_the_language_servers_down_when_the_shutdown_signal_arrives() {
  let ide = start_ide().await;
  let sdk = ide.sdk();
  ide.serve(
    capabilities(),
    &[
      ("textDocument/definition", json!([location(&sdk.money_uri, &sdk.money, "toCents", 0)])),
      ("textDocument/declaration", json!([])),
    ],
  );
  ide
    .harness
    .text("atlas_symbol", json!({"target": "acme-sdk:src/money.ts:1:17", "mode": "declaration"}))
    .await;
  let Ide { harness, scripts, .. } = ide;

  harness.shutdown.send(()).unwrap();
  harness.task.await.unwrap().unwrap();

  let methods = recorded_methods(scripts.path(), "ts");
  assert_eq!(&methods[methods.len() - 2..], ["shutdown", "exit"], "{methods:?}");
  let store = atlas_store::Store::open(harness.home.db_path(&"acme".parse().unwrap())).unwrap();
  assert!(store.read(|tx| tx.blob("similarity")).unwrap().is_some());
}

#[tokio::test]
async fn should_reject_a_negative_since_run() {
  let ide = start_ide().await;

  assert_eq!(ide.harness.failure("atlas_status", json!({"since_run": -1})).await, "since_run must not be negative");
  ide.harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_name_the_replacement_when_a_removed_tool_is_called() {
  let ide = start_ide().await;

  assert_eq!(
    ide.harness.failure("atlas_read", json!({"target": "toCents"})).await,
    "atlas_read was removed; use atlas_symbol mode=source"
  );
  assert_eq!(
    ide.harness.failure("atlas_changes", json!({})).await,
    "atlas_changes was removed; use atlas_status since_run"
  );
  ide.harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_mark_atlas_edit_destructive_and_not_read_only() {
  let ide = start_ide().await;

  let tools = ide.harness.client.list_all_tools().await.unwrap();

  let annotations = tools.iter().find(|tool| tool.name == "atlas_edit").unwrap().annotations.clone().unwrap();
  assert_eq!(
    (annotations.destructive_hint, annotations.read_only_hint, annotations.idempotent_hint),
    (Some(true), Some(false), Some(false))
  );
  ide.harness.stop().await.unwrap();
}
