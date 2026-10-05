mod common;

use atlas_ide::{DiagnosticsRequest, LineRange, Severity};
use atlas_lsp::ServerKind;
use common::*;
use serde_json::{Value, json};

const USE: &str = "import { toCents } from './money';\nexport function total(): number {\n  const unused = 1;\n  return toCents('1');\n}\n";
const LIB: &str = "pub fn answer() -> u32 {\n  let unused = 1;\n  42\n}\n";

fn diagnostic(start: (u32, u32), end: (u32, u32), severity: u32, code: Value, source: &str, message: &str) -> Value {
  json!({"range": range(start, end), "severity": severity, "code": code, "source": source, "message": message})
}

fn publish(uri: &str, version: i32, diagnostics: Value) -> Value {
  json!({"notify": {"method": "textDocument/publishDiagnostics", "params": {"uri": uri, "version": version, "diagnostics": diagnostics}}})
}

#[test]
fn should_wait_for_pushed_diagnostics_of_the_current_version_and_group_them() {
  let fixture = Fixture::new(&[("web", &[("package.json", "{\"name\": \"web\"}\n"), ("src/use.ts", USE)])]);
  let uri = fixture.uri("web", "src/use.ts");
  let missing = diagnostic(
    (0, 24),
    (0, 33),
    1,
    json!(2307),
    "typescript",
    "Cannot find module './money' or its corresponding type declarations.",
  );
  let unused = diagnostic((2, 8), (2, 14), 4, json!(6133), "typescript", "'unused' is declared but its value is never read.");
  let stale = diagnostic((3, 2), (3, 8), 1, json!(1005), "typescript", "';' expected.");
  let command = fixture.script(
    "ts",
    &with(
      ts_ready(capabilities(json!({}))),
      [
        json!({"recv": {"method": "textDocument/didOpen"}}),
        json!({"recv": {"method": "textDocument/didClose"}}),
        publish(&uri, 1, json!([])),
        json!({"recv": {"method": "textDocument/didOpen", "params": {"textDocument": {"version": 2}}}}),
        publish(&uri, 1, json!([stale])),
        json!({"sleep_ms": 100}),
        publish(&uri, 2, json!([unused.clone(), missing.clone()])),
        json!({"recv": {"method": "textDocument/didClose"}}),
        json!({"recv": {"method": "textDocument/didOpen", "params": {"textDocument": {"version": 3}}}}),
        publish(&uri, 3, json!([unused, missing])),
      ],
    ),
  );
  fixture.serve(ServerKind::TypeScript, command);

  let response = fixture.ide.diagnostics(&DiagnosticsRequest::new("src/use.ts")).unwrap();

  assert_eq!(
    text(&response),
    concat!(
      "diagnostics web:src/use.ts  [settled]  (typescript-language-server 5.1.3, TypeScript 5.9.3) [compiler]\n",
      "error (1)\n",
      "  <file>\n",
      "    1:25 2307 Cannot find module './money' or its corresponding type declarations.\n",
      "hint (1)\n",
      "  total\n",
      "    3:9 6133 'unused' is declared but its value is never read.\n",
      "caveats:\n",
      "  the language server cannot resolve ./money; references through it are invisible to exact operations\n",
    )
  );
  let filtered = fixture
    .ide
    .diagnostics(&DiagnosticsRequest {
      min_severity: Severity::Warning,
      lines: Some(LineRange::parse("1-2").unwrap()),
      ..DiagnosticsRequest::new("web:src/use.ts")
    })
    .unwrap();
  assert!(
    text(&filtered).contains("1 of 2 diagnostics match the filters\nerror (1)\n  <file>\n    1:25 2307"),
    "{}",
    text(&filtered)
  );
}

#[test]
fn should_merge_pulled_native_diagnostics_with_a_finished_flycheck() {
  let fixture = Fixture::new(&[(
    "core",
    &[("Cargo.toml", "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"), ("src/lib.rs", LIB)],
  )]);
  let uri = fixture.uri("core", "src/lib.rs");
  let native = diagnostic((1, 6), (1, 12), 2, json!("unused_variables"), "rust-analyzer", "unused variable: `unused`");
  let checked = diagnostic((2, 2), (2, 4), 1, json!("E0308"), "rustc", "mismatched types");
  let command = fixture.script(
    "ra",
    &with(
      rust_ready(capabilities(
        json!({"diagnosticProvider": {"identifier": "rust-analyzer", "interFileDependencies": true, "workspaceDiagnostics": false}}),
      )),
      [
        json!({"recv": {"method": "textDocument/didOpen"}}),
        json!({"recv": {"method": "textDocument/diagnostic", "params": {"identifier": "rust-analyzer"}}}),
        json!({"reply": {"method": "textDocument/diagnostic", "result": {"kind": "full", "items": [native.clone()]}}}),
        json!({"recv": {"method": "rust-analyzer/runFlycheck", "params": {"textDocument": {"uri": uri}}}}),
        json!({"notify": {"method": "$/progress", "params": {"token": "rust-analyzer/flycheck/0", "value": {"kind": "begin", "title": "cargo check"}}}}),
        publish(&uri, 1, json!([checked])),
        json!({"notify": {"method": "$/progress", "params": {"token": "rust-analyzer/flycheck/0", "value": {"kind": "end"}}}}),
      ],
    ),
  );
  fixture.serve(ServerKind::RustAnalyzer, command);

  let response = fixture.ide.diagnostics(&DiagnosticsRequest::new("src/lib.rs")).unwrap();

  assert_eq!(
    text(&response),
    concat!(
      "diagnostics core:src/lib.rs  [pulled, flycheck: complete]  (rust-analyzer 5.1.3) [compiler]\n",
      "error (1)\n",
      "  answer\n",
      "    3:3 E0308 mismatched types\n",
      "warning (1)\n",
      "  answer\n",
      "    2:7 unused_variables unused variable: `unused`\n",
    )
  );
}

#[test]
fn should_drop_earlier_cargo_check_results_when_flycheck_does_not_run() {
  let fixture = Fixture::new(&[(
    "core",
    &[("Cargo.toml", "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"), ("src/lib.rs", LIB)],
  )]);
  let uri = fixture.uri("core", "src/lib.rs");
  let native = diagnostic((1, 6), (1, 12), 2, json!("unused_variables"), "rust-analyzer", "unused variable: `unused`");
  let old_check = diagnostic((2, 2), (2, 4), 1, json!("E0308"), "rustc", "mismatched types");
  let command = fixture.script(
    "ra",
    &with(
      rust_ready(capabilities(json!({}))),
      [
        json!({"recv": {"method": "textDocument/didOpen"}}),
        json!({"recv": {"method": "textDocument/didChange"}}),
        publish(&uri, 2, json!([native, old_check])),
      ],
    ),
  );
  fixture.serve(ServerKind::RustAnalyzer, command);

  let response = fixture.ide.diagnostics(&DiagnosticsRequest::new("src/lib.rs")).unwrap();

  assert_eq!(
    text(&response),
    concat!(
      "diagnostics core:src/lib.rs  [settled, flycheck: not_run]  (rust-analyzer 5.1.3) [compiler]\n",
      "PARTIAL: cargo check (flycheck) did not run; only rust-analyzer's native diagnostics are shown; the answer may be incomplete\n",
      "warning (1)\n",
      "  answer\n",
      "    2:7 unused_variables unused variable: `unused`\n",
      "caveats:\n",
      "  1 results of an earlier cargo check were omitted\n",
    )
  );
}

#[test]
fn should_answer_partial_and_never_clean_when_diagnostics_do_not_settle() {
  let fixture = Fixture::new(&[("web", &[("package.json", "{\"name\": \"web\"}\n"), ("src/use.ts", USE), ("src/busy.ts", USE)])]);
  let busy_uri = fixture.uri("web", "src/busy.ts");
  let unused = diagnostic((2, 8), (2, 14), 4, json!(6133), "typescript", "'unused' is declared but its value is never read.");
  let mut steps = ts_ready(capabilities(json!({})));
  for _ in 0..2 {
    steps.push(json!({"recv": {"method": "textDocument/didOpen"}}));
    steps.push(json!({"recv": {"method": "textDocument/didClose"}}));
    steps.push(json!({"recv": {"method": "textDocument/didOpen"}}));
  }
  for _ in 0..60 {
    steps.push(publish(&busy_uri, 4, json!([unused.clone()])));
    steps.push(json!({"sleep_ms": 200}));
  }
  fixture.serve(ServerKind::TypeScript, fixture.script("ts", &steps));

  let silent = fixture.ide.diagnostics(&DiagnosticsRequest::new("src/use.ts")).unwrap();
  let changing = fixture.ide.diagnostics(&DiagnosticsRequest::new("src/busy.ts")).unwrap();

  let silent = text(&silent);
  assert!(silent.starts_with("diagnostics web:src/use.ts  [timed out after "), "{silent}");
  assert!(
    silent.ends_with(concat!(
      "PARTIAL: nothing known: the server published no diagnostics within 10 s; the answer may be incomplete\n",
      "  nothing known: the server published no diagnostics within 10 s\n",
    )),
    "{silent}"
  );
  let changing = text(&changing);
  assert!(
    changing.contains("\nPARTIAL: diagnostics were still changing when the 10 s wait ended; the answer may be incomplete\nhint (1)\n"),
    "{changing}"
  );
}

#[test]
fn should_wait_for_the_semantic_publish_after_reopening_a_typescript_file() {
  let fixture = Fixture::new(&[("web", &[("package.json", "{\"name\": \"web\"}\n"), ("src/use.ts", USE)])]);
  let uri = fixture.uri("web", "src/use.ts");
  let semantic = diagnostic((3, 9), (3, 16), 1, json!(2304), "typescript", "Cannot find name 'toCents'.");
  let command = fixture.script(
    "ts",
    &with(
      ts_ready(capabilities(json!({}))),
      [
        json!({"recv": {"method": "textDocument/didOpen"}}),
        json!({"recv": {"method": "textDocument/didClose"}}),
        json!({"notify": {"method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": []}}}),
        json!({"recv": {"method": "textDocument/didOpen"}}),
        json!({"notify": {"method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": []}}}),
        json!({"sleep_ms": 700}),
        json!({"notify": {"method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": [semantic]}}}),
      ],
    ),
  );
  fixture.serve(ServerKind::TypeScript, command);

  let response = fixture.ide.diagnostics(&DiagnosticsRequest::new("src/use.ts")).unwrap();

  assert_eq!(
    text(&response),
    concat!(
      "diagnostics web:src/use.ts  [settled]  (typescript-language-server 5.1.3, TypeScript 5.9.3) [compiler]\n",
      "error (1)\n",
      "  total\n",
      "    4:10 2304 Cannot find name 'toCents'.\n",
    )
  );
}
