mod common;

use std::fs;

use atlas_ide::OutlineRequest;
use atlas_lsp::ServerKind;
use atlas_store::{RunOutcome, RunReport, StoreError};
use common::*;
use serde_json::{Value, json};

const MONEY: &str = "export function toCents(amount: string): number {\n  return Math.round(Number(amount) * 100);\n}\n";
const RESTARTED: &str = "the language server was restarted: the changes since the server's generation are unknown";

fn web() -> Fixture {
  Fixture::new(&[(
    "web",
    &[
      ("package.json", "{\"name\": \"web\"}\n"),
      ("src/money.ts", MONEY),
      ("src/use.ts", "export const a = 1;\n"),
      ("src/extra.ts", "export const b = 2;\n"),
    ],
  )])
}

fn serve(fixture: &Fixture) {
  let steps = with(
    ts_ready(capabilities(json!({}))),
    [fixture.record("ts"), json!({"canned": {"method": "textDocument/documentSymbol", "result": []}})],
  );
  fixture.serve(ServerKind::TypeScript, fixture.counted("ts", &steps));
}

fn outline(fixture: &Fixture) -> String {
  text(&fixture.ide.outline(&OutlineRequest::new("src/money.ts")).unwrap())
}

fn watched(recorded: &[Value]) -> Vec<Value> {
  recorded
    .iter()
    .filter(|message| message["method"] == "workspace/didChangeWatchedFiles")
    .flat_map(|message| message["params"]["changes"].as_array().cloned().unwrap_or_default())
    .collect()
}

#[test]
fn should_send_a_file_edited_between_calls_as_an_overlay_with_its_new_text() {
  let fixture = web();
  serve(&fixture);
  outline(&fixture);
  fs::write(fixture.file("web", "src/use.ts"), "export const a = 42;\n").unwrap();

  outline(&fixture);

  let recorded = fixture.recorded("ts");
  let uri = fixture.uri("web", "src/use.ts");
  let opened: Vec<&Value> = recorded
    .iter()
    .filter(|message| message["method"] == "textDocument/didOpen" && message["params"]["textDocument"]["uri"] == uri)
    .collect();
  assert_eq!(opened.len(), 1, "{recorded:?}");
  assert_eq!(opened[0]["params"]["textDocument"]["text"], json!("export const a = 42;\n"));
  assert_eq!(watched(&recorded), vec![json!({"uri": uri, "type": 2})]);
  assert_eq!(fixture.spawns("ts"), 1);
}

#[test]
fn should_announce_deleted_and_moved_files() {
  let fixture = web();
  serve(&fixture);
  outline(&fixture);
  fs::remove_file(fixture.file("web", "src/use.ts")).unwrap();
  fs::rename(fixture.file("web", "src/extra.ts"), fixture.file("web", "src/moved.ts")).unwrap();

  outline(&fixture);

  let mut changes = watched(&fixture.recorded("ts"));
  changes.sort_by_key(|change| change["uri"].as_str().unwrap_or_default().to_owned());
  let uri = |path: &str| json!(fixture.uri("web", path));
  assert_eq!(
    changes,
    vec![
      json!({"uri": uri("src/extra.ts"), "type": 3}),
      json!({"uri": uri("src/moved.ts"), "type": 2}),
      json!({"uri": uri("src/use.ts"), "type": 3}),
    ]
  );
  assert_eq!(fixture.spawns("ts"), 1);
}

fn inject_run(fixture: &Fixture, outcome: Option<RunOutcome>) {
  fixture
    .ide
    .query()
    .engine()
    .store()
    .write(|tx| -> Result<(), StoreError> {
      let run = tx.begin_run("barrier")?;
      if let Some(outcome) = outcome {
        tx.finish_run(run, outcome, &RunReport::default())?;
      }
      Ok(())
    })
    .unwrap();
}

#[test]
fn should_restart_the_server_when_an_index_run_in_range_failed_or_never_finished() {
  for outcome in [Some(RunOutcome::Failed { error: "disk full".to_owned() }), None] {
    let fixture = web();
    serve(&fixture);
    outline(&fixture);
    inject_run(&fixture, outcome.clone());
    fs::write(fixture.file("web", "src/use.ts"), "export const a = 42;\n").unwrap();

    let answer = outline(&fixture);

    assert_eq!(fixture.spawns("ts"), 2, "{outcome:?}");
    assert!(answer.ends_with(&format!("warnings:\n  {RESTARTED}\n")), "{outcome:?}: {answer}");
  }
}

#[test]
fn should_restart_the_server_when_a_change_set_was_capped() {
  let fixture = web();
  serve(&fixture);
  outline(&fixture);
  fs::create_dir_all(fixture.file("web", "src/many")).unwrap();
  for index in 0..1001 {
    fs::write(fixture.file("web", &format!("src/many/f{index}.ts")), format!("export const v{index} = {index};\n")).unwrap();
  }

  let answer = outline(&fixture);

  assert_eq!(fixture.spawns("ts"), 2);
  assert!(answer.ends_with(&format!("warnings:\n  {RESTARTED}\n")), "{answer}");
}
