mod common;

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use atlas_core::RunId;
use atlas_ide::{CrossRepo, EditOperation, EditRequest, EditStatus};
use atlas_lsp::ServerKind;
use common::*;
use serde_json::{Value, json};

const MONEY: &str = "export function toCents(amount: string): number {\n  return Math.round(Number(amount) * 100);\n}\n";
const USE: &str = "import { toCents } from './money';\nexport function total(): number {\n  return toCents('1') + toCents('2');\n}\n";
const LIB: &str = "pub fn answer() -> u32 {\n  42\n}\n\npub fn twice() -> u32 {\n  answer() * 2\n}\n";
const CARGO: &str = "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

fn edit_capabilities() -> Value {
  capabilities(json!({"renameProvider": {"prepareProvider": true}, "diagnosticProvider": {"interFileDependencies": true, "workspaceDiagnostics": false}}))
}

fn text_edit(start: (u32, u32), end: (u32, u32), new_text: &str) -> Value {
  json!({"range": range(start, end), "newText": new_text})
}

fn canned(method: &str, result: Value) -> Value {
  json!({"canned": {"method": method, "result": result}})
}

fn fixture() -> Fixture {
  Fixture::new(&[
    (
      "web",
      &[
        ("package.json", "{\"name\": \"web\"}\n"),
        ("tsconfig.json", "{}\n"),
        ("src/money.ts", MONEY),
        ("src/use.ts", USE),
      ],
    ),
    ("core", &[("Cargo.toml", CARGO), ("src/lib.rs", LIB)]),
  ])
}

fn rename_to_cents(fixture: &Fixture) -> Value {
  json!({"changes": {
    fixture.uri("web", "src/money.ts"): [text_edit((0, 16), (0, 23), "cents")],
    fixture.uri("web", "src/use.ts"): [text_edit((0, 9), (0, 16), "cents"), text_edit((2, 9), (2, 16), "cents"), text_edit((2, 24), (2, 31), "cents")],
  }})
}

fn tail() -> Vec<Value> {
  vec![
    canned("textDocument/diagnostic", json!({"kind": "full", "items": []})),
    canned("textDocument/documentSymbol", json!([])),
  ]
}

fn serve_slow_typescript(fixture: &Fixture, delay_ms: u64) {
  let mut steps = with(
    ts_ready(edit_capabilities()),
    [
      fixture.record("ts"),
      json!({"recv_all": ["textDocument/didOpen", "textDocument/didOpen"]}),
      json!({"recv": {"method": "textDocument/prepareRename"}}),
      json!({"reply": {"method": "textDocument/prepareRename", "result": {"range": range((0, 16), (0, 23)), "placeholder": "toCents"}}}),
      json!({"recv": {"method": "textDocument/rename"}}),
      json!({"sleep_ms": delay_ms}),
      json!({"reply": {"method": "textDocument/rename", "result": rename_to_cents(fixture)}}),
    ],
  );
  steps.extend(tail());
  let command = fixture.script("ts", &steps);
  fixture.serve(ServerKind::TypeScript, command);
}

fn serve_rust(fixture: &Fixture) {
  let edit = json!({"changes": {fixture.uri("core", "src/lib.rs"): [text_edit((0, 7), (0, 13), "reply"), text_edit((5, 2), (5, 8), "reply")]}});
  let mut steps = with(
    rust_ready(edit_capabilities()),
    [
      canned("textDocument/prepareRename", json!({"range": range((0, 7), (0, 13)), "placeholder": "answer"})),
      canned("textDocument/rename", edit),
    ],
  );
  steps.extend(tail());
  let command = fixture.script("ra", &steps);
  fixture.serve(ServerKind::RustAnalyzer, command);
}

fn rename(target: &str, new_name: &str) -> EditRequest {
  EditRequest::new(EditOperation::Rename {
    target: target.to_owned(),
    new_name: new_name.to_owned(),
    cross_repo: CrossRepo::Report,
    allow_unverified: false,
  })
}

#[test]
fn should_serialise_two_renames_from_two_threads() {
  let fixture = Arc::new(fixture());
  serve_slow_typescript(&fixture, 800);
  serve_rust(&fixture);

  let first = {
    let fixture = Arc::clone(&fixture);
    thread::spawn(move || {
      let response = fixture.ide.edit(&rename("toCents", "cents")).unwrap();
      (response.answer.status, Instant::now())
    })
  };
  assert!(wait_until(|| methods(&fixture.recorded("ts")).iter().any(|method| method == "textDocument/rename")));
  let second = fixture.ide.edit(&rename("answer", "reply")).unwrap();
  let second_done = Instant::now();
  let (first_status, first_done) = first.join().unwrap();

  assert_eq!(first_status, EditStatus::Applied);
  assert_eq!(second.answer.status, EditStatus::Applied);
  assert!(second_done > first_done, "the second rename finished while the first was still being planned");
  assert_eq!(fs::read_to_string(fixture.file("web", "src/use.ts")).unwrap(), USE.replace("toCents", "cents"));
  assert_eq!(fs::read_to_string(fixture.file("core", "src/lib.rs")).unwrap(), LIB.replace("answer", "reply"));
}

fn analyzed(fixture: &Fixture, run: i64) -> Option<(String, Vec<String>)> {
  let record = fixture.ide.query().engine().store().read(|tx| tx.run(RunId(run))).unwrap()?;
  let entries = record.report.change_set["analyzed"]["entries"]
    .as_array()
    .map(|entries| entries.iter().filter_map(|entry| entry.as_str().map(str::to_owned)).collect())
    .unwrap_or_default();
  Some((record.trigger, entries))
}

#[test]
fn should_never_let_a_barrier_from_another_engine_index_a_half_applied_rename() {
  let fixture = fixture();
  serve_slow_typescript(&fixture, 0);
  fixture.ide.query().fresh().unwrap();
  let other = Arc::new(fixture.second_engine());
  let stop = Arc::new(AtomicBool::new(false));
  let barriers = {
    let (other, stop) = (Arc::clone(&other), Arc::clone(&stop));
    thread::spawn(move || {
      let mut count = 0;
      while !stop.load(Ordering::SeqCst) {
        other.ensure_fresh().unwrap();
        count += 1;
      }
      count
    })
  };

  let response = fixture.ide.edit(&rename("toCents", "cents")).unwrap();
  thread::sleep(Duration::from_millis(50));
  stop.store(true, Ordering::SeqCst);
  let count = barriers.join().unwrap();

  assert_eq!(response.answer.status, EditStatus::Applied);
  assert!(count > 0);
  let last = response.freshness.run.unwrap() + 5;
  let edited = ["web:src/money.ts".to_owned(), "web:src/use.ts".to_owned()];
  let mut edit_runs = 0;
  for run in 1..=last {
    let Some((trigger, entries)) = analyzed(&fixture, run) else { continue };
    let touched = edited.iter().filter(|file| entries.contains(file)).count();
    assert!(touched != 1, "run {run} ({trigger}) indexed half of the rename: {entries:?}");
    if trigger == "edit" && touched == 2 {
      edit_runs += 1;
    }
  }
  assert_eq!(edit_runs, 1);
}
