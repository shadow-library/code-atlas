mod common;

use std::time::{Duration, Instant};

use atlas_engine::WatchHandle;
use atlas_ide::{CrossRepo, EditOperation, EditRequest, EditStatus};
use atlas_lsp::ServerKind;
use atlas_query::{Targeted, TraverseRequest};
use common::*;
use serde_json::{Value, json};

const MONEY: &str = "export function toCents(amount: string): number {\n  return Math.round(Number(amount) * 100);\n}\n";
const USE: &str = "import { toCents } from './money';\nexport function total(): number {\n  return toCents('1') + toCents('2');\n}\n";
const MONEY_KEY: &str = "ts:web:web:src/money::NAME#function";
const TOTAL_KEY: &str = "ts:web:web:src/use::total#function";
const EVENT_QUIET: Duration = Duration::from_millis(500);

fn canned(method: &str, result: Value) -> Value {
  json!({"canned": {"method": method, "result": result}})
}

fn text_edit(start: (u32, u32), end: (u32, u32), new_text: &str) -> Value {
  json!({"range": range(start, end), "newText": new_text})
}

fn serve_rename(fixture: &Fixture) {
  let rename = json!({"changes": {
    fixture.uri("web", "src/money.ts"): [text_edit((0, 16), (0, 23), "cents")],
    fixture.uri("web", "src/use.ts"): [text_edit((0, 9), (0, 16), "cents"), text_edit((2, 9), (2, 16), "cents"), text_edit((2, 24), (2, 31), "cents")],
  }});
  let ready = ts_ready(capabilities(
    json!({"renameProvider": {"prepareProvider": true}, "diagnosticProvider": {"interFileDependencies": true, "workspaceDiagnostics": false}}),
  ));
  let steps = with(
    ready,
    [
      canned("textDocument/diagnostic", json!({"kind": "full", "items": []})),
      canned("textDocument/documentSymbol", json!([])),
      canned("textDocument/references", json!([])),
      canned("textDocument/prepareRename", json!({"range": range((0, 16), (0, 23)), "placeholder": "toCents"})),
      canned("textDocument/rename", rename),
    ],
  );
  fixture.serve(ServerKind::TypeScript, fixture.script("ts", &steps));
}

fn wait_for_quiet_events(watch: &WatchHandle) {
  let mut last = (watch.events_seen(), Instant::now());
  let quiet = wait_until(|| {
    let seen = watch.events_seen();
    if seen != last.0 {
      last = (seen, Instant::now());
    }
    last.1.elapsed() >= EVENT_QUIET
  });
  assert!(quiet, "the watcher kept delivering events");
}

fn callers(fixture: &Fixture, target: &str) -> (u64, String, Vec<String>) {
  let response = fixture.ide.query().callers(&TraverseRequest::new(target)).unwrap();
  let Targeted::Found(answer) = &response.answer else {
    panic!("{target}: {:?}", response.answer);
  };
  let callers = answer.groups.iter().flat_map(|group| group.items.iter().map(|item| item.entity.key.clone())).collect();
  (response.freshness.files_updated, answer.target.key.clone(), callers)
}

#[test]
fn should_reflect_a_rename_in_the_next_query() {
  let fixture = Fixture::new(&[(
    "web",
    &[
      ("package.json", "{\"name\": \"web\"}\n"),
      ("tsconfig.json", "{}\n"),
      ("src/money.ts", MONEY),
      ("src/use.ts", USE),
    ],
  )]);
  serve_rename(&fixture);
  let engine = fixture.ide.query().engine();
  engine.ensure_fresh().unwrap();
  let watch = engine.watch().unwrap();
  wait_for_quiet_events(&watch);
  engine.ensure_fresh().unwrap();
  assert!(fixture.ide.query().fresh().unwrap().watcher_clean, "the watcher must be clean before the rename");
  assert_eq!(callers(&fixture, "toCents"), (0, MONEY_KEY.replace("NAME", "toCents"), vec![TOTAL_KEY.to_owned()]));

  let renamed = fixture.ide.edit(&EditRequest::new(EditOperation::Rename {
    target: "toCents".to_owned(),
    new_name: "cents".to_owned(),
    cross_repo: CrossRepo::Report,
    allow_unverified: false,
  }));
  let after = callers(&fixture, "cents");
  let old = fixture.ide.query().resolve_target("toCents").unwrap();

  assert_eq!(renamed.unwrap().answer.status, EditStatus::Applied);
  assert_eq!(after, (0, MONEY_KEY.replace("NAME", "cents"), vec![TOTAL_KEY.to_owned()]));
  assert!(matches!(old, Targeted::NotFound { .. }), "{old:?}");
  let runs = engine.store().read(|tx| tx.runs(1)).unwrap();
  assert_eq!(runs.iter().map(|run| run.trigger.as_str()).collect::<Vec<_>>(), ["edit"]);
  drop(watch);
}
