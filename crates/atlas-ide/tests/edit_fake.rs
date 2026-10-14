mod common;

use std::fs;
use std::time::Duration;

use atlas_ide::{CrossRepo, DryRunReason, EditOperation, EditOutcome, EditRequest, EditStatus, IdeError, IdeSettings, ReferencesRequest, VerifyReason};
use atlas_lsp::ServerKind;
use atlas_query::{Response, Targeted};
use common::*;
use serde_json::{Value, json};

const MONEY: &str = "export function toCents(amount: string): number {\n  return Math.round(Number(amount) * 100);\n}\n";
const USE: &str = "import { toCents } from './money';\nexport function total(): number {\n  return toCents('1') + toCents('2');\n}\n";
const LIB: &str = "pub mod other;\npub fn answer() -> u32 {\n  42\n}\n";
const OTHER: &str = "pub fn twice() -> u32 {\n  crate::answer() * 2\n}\n";
const CARGO: &str = "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

fn web_files() -> Vec<(&'static str, &'static str)> {
  vec![
    ("package.json", "{\"name\": \"web\"}\n"),
    ("tsconfig.json", "{}\n"),
    ("src/money.ts", MONEY),
    ("src/use.ts", USE),
  ]
}

fn web() -> Fixture {
  Fixture::new(&[("web", &web_files())])
}

fn core(module: &str) -> Fixture {
  let lib = LIB.replace("other", module.split('/').next().unwrap_or("other"));
  let other = format!("src/{module}.rs");
  Fixture::new(&[("core", &[("Cargo.toml", CARGO), ("src/lib.rs", &lib), (&other, OTHER)])])
}

fn edit_capabilities() -> Value {
  capabilities(json!({"renameProvider": {"prepareProvider": true}, "diagnosticProvider": {"interFileDependencies": true, "workspaceDiagnostics": false}}))
}

fn text_edit(start: (u32, u32), end: (u32, u32), new_text: &str) -> Value {
  json!({"range": range(start, end), "newText": new_text})
}

fn rename_to_cents(fixture: &Fixture, new_name: &str) -> Value {
  json!({"changes": {
    fixture.uri("web", "src/money.ts"): [text_edit((0, 16), (0, 23), new_name)],
    fixture.uri("web", "src/use.ts"): [
      text_edit((0, 9), (0, 16), new_name),
      text_edit((2, 9), (2, 16), new_name),
      text_edit((2, 24), (2, 31), new_name),
    ],
  }})
}

fn canned(method: &str, result: Value) -> Value {
  json!({"canned": {"method": method, "result": result}})
}

fn serve_with(fixture: &Fixture, kind: ServerKind, extra: Vec<Value>) {
  let (name, ready) = match kind {
    ServerKind::TypeScript => ("ts", ts_ready(edit_capabilities())),
    ServerKind::RustAnalyzer => ("ra", rust_ready(edit_capabilities())),
  };
  let mut steps = vec![
    fixture.record(name),
    canned("textDocument/diagnostic", json!({"kind": "full", "items": []})),
    canned("textDocument/documentSymbol", json!([])),
    canned("textDocument/references", json!([])),
  ];
  steps.extend(extra);
  let command = fixture.script(name, &with(ready, steps));
  fixture.serve(kind, command);
}

fn serve(fixture: &Fixture, prepare: Value, rename: Value) {
  serve_with(
    fixture,
    ServerKind::TypeScript,
    vec![canned("textDocument/prepareRename", prepare), canned("textDocument/rename", rename)],
  );
}

fn prepare_to_cents() -> Value {
  json!({"range": range((0, 16), (0, 23)), "placeholder": "toCents"})
}

fn rename(target: &str, new_name: &str) -> EditOperation {
  EditOperation::Rename {
    target: target.to_owned(),
    new_name: new_name.to_owned(),
    cross_repo: CrossRepo::Report,
    allow_unverified: false,
  }
}

fn rendered(response: &Response<EditOutcome>) -> String {
  let text = text(response);
  match &response.answer.status {
    EditStatus::DryRun { plan_id, .. } => text.replace(plan_id.as_str(), "<plan>"),
    _ => text,
  }
}

fn plan_id(response: &Response<EditOutcome>) -> String {
  match &response.answer.status {
    EditStatus::DryRun { plan_id, .. } => plan_id.clone(),
    other => panic!("expected a dry run, got {other:?}"),
  }
}

fn refusal(response: &Response<EditOutcome>) -> String {
  match &response.answer.status {
    EditStatus::Refused { reason } => reason.clone(),
    other => panic!("expected a refusal, got {other:?}\n{}", text(response)),
  }
}

fn read(fixture: &Fixture, repo: &str, path: &str) -> String {
  fs::read_to_string(fixture.file(repo, path)).unwrap()
}

fn count(recorded: &[Value], method: &str) -> usize {
  methods(recorded).iter().filter(|seen| *seen == method).count()
}

#[test]
fn should_plan_a_rename_from_plain_changes_and_apply_it_by_plan_id() {
  let fixture = web();
  serve(&fixture, prepare_to_cents(), rename_to_cents(&fixture, "cents"));

  let dry = fixture.ide.edit(&EditRequest::dry_run(rename("toCents", "cents"))).unwrap();

  assert_eq!(
    rendered(&dry),
    concat!(
      "rename toCents -> cents  dry run, plan <plan> (apply within 15 min)  (typescript-language-server 5.1.3, TypeScript 5.9.3)\n",
      "would change 2 file(s), 4 edit(s):\n",
      "  web:src/money.ts  1\n",
      "    @@ -1,3 +1,3 @@\n",
      "    -export function toCents(amount: string): number {\n",
      "    +export function cents(amount: string): number {\n",
      "       return Math.round(Number(amount) * 100);\n",
      "     }\n",
      "  web:src/use.ts    3\n",
      "    @@ -1,4 +1,4 @@\n",
      "    -import { toCents } from './money';\n",
      "    +import { cents } from './money';\n",
      "     export function total(): number {\n",
      "    -  return toCents('1') + toCents('2');\n",
      "    +  return cents('1') + cents('2');\n",
      "     }\n",
      "not renamed, verify:\n",
      "  (none)\n",
    )
  );
  assert_eq!(read(&fixture, "web", "src/money.ts"), MONEY);
  assert_eq!(fixture.ide.planned(&plan_id(&dry)).unwrap().files.len(), 2);

  let applied = fixture.ide.edit(&EditRequest::new(EditOperation::Apply { plan_id: plan_id(&dry) })).unwrap();

  assert_eq!(
    rendered(&applied),
    concat!(
      "rename toCents -> cents  applied  (typescript-language-server 5.1.3, TypeScript 5.9.3)\n",
      "changed 2 file(s), 4 edit(s):\n",
      "  web:src/money.ts  1\n",
      "  web:src/use.ts    3\n",
      "not renamed, verify:\n",
      "  (none)\n",
      "diagnostics after edit: no new errors in the 2 edited file(s) (settled)\n",
    )
  );
  assert_eq!(read(&fixture, "web", "src/money.ts"), MONEY.replace("toCents", "cents"));
  assert_eq!(read(&fixture, "web", "src/use.ts"), USE.replace("toCents", "cents"));
  let query = fixture.ide.query();
  assert!(matches!(query.resolve_target("cents").unwrap(), Targeted::Found(_)));
  assert!(matches!(query.resolve_target("toCents").unwrap(), Targeted::NotFound { .. }));
  let again = fixture.ide.edit(&EditRequest::new(EditOperation::Apply { plan_id: plan_id(&dry) })).unwrap_err();
  assert!(matches!(again, IdeError::UnknownPlan { .. }), "{again}");
}

#[test]
fn should_tell_the_server_the_new_text_and_the_changed_files_after_a_rename() {
  let fixture = web();
  serve(&fixture, prepare_to_cents(), rename_to_cents(&fixture, "cents"));

  fixture.ide.edit(&EditRequest::new(rename("toCents", "cents"))).unwrap();

  let recorded = fixture.recorded("ts");
  let after_rename: Vec<&Value> = recorded.iter().skip_while(|message| message["method"] != "textDocument/rename").collect();
  let changed: Vec<(String, String)> = after_rename
    .iter()
    .filter(|message| message["method"] == "textDocument/didChange")
    .map(|message| {
      (
        message["params"]["textDocument"]["uri"].as_str().unwrap().to_owned(),
        message["params"]["contentChanges"][0]["text"].as_str().unwrap().to_owned(),
      )
    })
    .collect();
  assert_eq!(
    changed,
    vec![
      (fixture.uri("web", "src/money.ts"), MONEY.replace("toCents", "cents")),
      (fixture.uri("web", "src/use.ts"), USE.replace("toCents", "cents")),
    ]
  );
  let watched = after_rename.iter().find(|message| message["method"] == "workspace/didChangeWatchedFiles").unwrap();
  assert_eq!(
    watched["params"]["changes"],
    json!([{"type": 2, "uri": fixture.uri("web", "src/money.ts")}, {"type": 2, "uri": fixture.uri("web", "src/use.ts")}])
  );
  assert_eq!(count(&recorded, "textDocument/prepareRename"), 1);
}

#[test]
fn should_convert_versioned_document_changes_and_a_file_rename() {
  let fixture = core("other");
  let lib = fixture.uri("core", "src/lib.rs");
  let renamed = fixture.uri("core", "src/renamed.rs");
  let edit = json!({"documentChanges": [
    {"textDocument": {"uri": lib, "version": 1}, "edits": [text_edit((1, 7), (1, 13), "reply")]},
    {"kind": "rename", "oldUri": fixture.uri("core", "src/other.rs"), "newUri": renamed},
    {"textDocument": {"uri": renamed, "version": null}, "edits": [text_edit((1, 9), (1, 15), "reply")]}
  ]});
  serve_with(
    &fixture,
    ServerKind::RustAnalyzer,
    vec![
      canned("textDocument/prepareRename", json!({"range": range((1, 7), (1, 13)), "placeholder": "answer"})),
      canned("textDocument/rename", edit),
    ],
  );

  let response = fixture.ide.edit(&EditRequest::new(rename("answer", "reply"))).unwrap();

  assert_eq!(
    rendered(&response),
    concat!(
      "rename answer -> reply  applied  (rust-analyzer 5.1.3)\n",
      "changed 2 file(s), 2 edit(s):\n",
      "  core:src/lib.rs    1\n",
      "  core:src/other.rs  1\n",
      "  rename core:src/other.rs -> core:src/renamed.rs\n",
      "not renamed, verify:\n",
      "  (none)\n",
      "diagnostics after edit: no new native errors in the 2 edited file(s) (settled; cargo check not run)\n",
    )
  );
  assert_eq!(read(&fixture, "core", "src/lib.rs"), LIB.replace("answer", "reply"));
  assert_eq!(read(&fixture, "core", "src/renamed.rs"), OTHER.replace("answer", "reply"));
  assert!(!fixture.file("core", "src/other.rs").exists());
}

#[test]
fn should_refuse_a_rename_whose_versions_disagree_with_the_open_documents_after_one_resync() {
  let fixture = core("other");
  let edit = json!({"documentChanges": [
    {"textDocument": {"uri": fixture.uri("core", "src/lib.rs"), "version": 7}, "edits": [text_edit((1, 7), (1, 13), "reply")]}
  ]});
  serve_with(
    &fixture,
    ServerKind::RustAnalyzer,
    vec![
      canned("textDocument/prepareRename", json!({"range": range((1, 7), (1, 13)), "placeholder": "answer"})),
      canned("textDocument/rename", edit),
    ],
  );

  let response = fixture.ide.edit(&EditRequest::new(rename("answer", "reply"))).unwrap();

  assert_eq!(
    refusal(&response),
    "the language server's view still disagrees with the files on disk after re-syncing them (the server edits core:src/lib.rs at version [7], but the open document is at version 1); nothing was written"
  );
  assert_eq!(read(&fixture, "core", "src/lib.rs"), LIB);
  assert_eq!(count(&fixture.recorded("ra"), "textDocument/rename"), 2);
}

#[test]
fn should_resync_and_ask_again_once_then_refuse_when_an_edit_does_not_replace_the_old_name() {
  let fixture = web();
  let edit = json!({"changes": {
    fixture.uri("web", "src/money.ts"): [text_edit((0, 16), (0, 23), "cents")],
    fixture.uri("web", "src/use.ts"): [text_edit((1, 0), (1, 6), "cents")],
  }});
  serve(&fixture, prepare_to_cents(), edit);

  let response = fixture.ide.edit(&EditRequest::new(rename("toCents", "cents"))).unwrap();

  assert_eq!(
    refusal(&response),
    "the language server's view still disagrees with the files on disk after re-syncing them (at web:src/use.ts:2:1 the server would replace \"export\", not toCents); nothing was written"
  );
  let recorded = fixture.recorded("ts");
  let sequence: Vec<String> = methods(&recorded)
    .into_iter()
    .filter(|method| method == "textDocument/rename" || method == "textDocument/didOpen")
    .collect();
  assert_eq!(sequence, vec!["textDocument/didOpen", "textDocument/didOpen", "textDocument/rename", "textDocument/rename"]);
  assert_eq!(read(&fixture, "web", "src/money.ts"), MONEY);
  assert_eq!(read(&fixture, "web", "src/use.ts"), USE);
}

#[test]
fn should_refuse_the_whole_rename_when_any_file_is_outside_the_editable_scope() {
  let mut files = web_files();
  files.push(("node_modules/lib/index.d.ts", "export declare function toCents(a: string): number;\n"));
  let fixture = Fixture::new(&[("web", &files)]);
  let outside = fixture.scratch.path().join("elsewhere.ts");
  fs::write(&outside, "toCents('1');\n").unwrap();
  let edit = json!({"changes": {
    fixture.uri("web", "src/money.ts"): [text_edit((0, 16), (0, 23), "cents")],
    fixture.uri("web", "node_modules/lib/index.d.ts"): [text_edit((0, 24), (0, 31), "cents")],
    atlas_lsp::uri_from_path(&outside).unwrap(): [text_edit((0, 0), (0, 7), "cents")],
  }});
  serve(&fixture, prepare_to_cents(), edit);

  let response = fixture.ide.edit(&EditRequest::new(rename("toCents", "cents"))).unwrap();

  let reason = refusal(&response);
  assert!(reason.starts_with("the rename touches files outside the editable scope: "), "{reason}");
  assert!(reason.contains("node_modules/lib/index.d.ts (inside excluded directory node_modules)"), "{reason}");
  assert!(reason.contains("elsewhere.ts (outside every registered repo)"), "{reason}");
  assert!(reason.ends_with("; nothing was written"), "{reason}");
  assert_eq!(read(&fixture, "web", "src/money.ts"), MONEY);
}

#[test]
fn should_refuse_when_prepare_rename_and_the_index_disagree_or_the_server_refuses() {
  let fixture = web();
  serve(
    &fixture,
    json!({"range": range((1, 2), (1, 8)), "placeholder": "return"}),
    rename_to_cents(&fixture, "cents"),
  );

  let mismatch = fixture.ide.edit(&EditRequest::new(rename("toCents", "cents"))).unwrap();

  assert_eq!(
    refusal(&mismatch),
    "server and index disagree at web:src/money.ts:1:17: the server would rename \"return\", which is elsewhere"
  );

  let refused = web();
  serve_with(
    &refused,
    ServerKind::TypeScript,
    vec![json!({"canned": {"method": "textDocument/prepareRename", "error": {"code": -32600, "message": "You cannot rename this element."}}})],
  );
  let response = refused.ide.edit(&EditRequest::new(rename("toCents", "cents"))).unwrap();
  assert_eq!(
    refusal(&response),
    "typescript-language-server cannot rename at web:src/money.ts:1:17: You cannot rename this element."
  );
  assert_eq!(count(&refused.recorded("ts"), "textDocument/rename"), 0);
}

#[test]
fn should_refuse_edits_that_need_confirmation_and_overlapping_edits() {
  let fixture = web();
  let annotated = json!({
    "changes": {fixture.uri("web", "src/money.ts"): [{"range": range((0, 16), (0, 23)), "newText": "cents", "annotationId": "ask"}]},
    "changeAnnotations": {"ask": {"label": "rename in comments", "needsConfirmation": true}}
  });
  serve(&fixture, prepare_to_cents(), annotated);

  let confirmation = fixture.ide.edit(&EditRequest::new(rename("toCents", "cents"))).unwrap();

  assert_eq!(
    refusal(&confirmation),
    "the server marked this rename as needing confirmation (rename in comments); code-atlas never applies such edits automatically; nothing was written"
  );

  let overlapping = web();
  let edit = json!({"changes": {overlapping.uri("web", "src/money.ts"): [text_edit((0, 16), (0, 23), "cents"), text_edit((0, 16), (0, 23), "centsX")]}});
  serve(&overlapping, prepare_to_cents(), edit);
  let response = overlapping.ide.edit(&EditRequest::new(rename("toCents", "cents"))).unwrap();
  assert_eq!(
    refusal(&response),
    "the server's rename is not a valid edit (web:src/money.ts: edits 16..23 and 16..23 overlap); nothing was written"
  );
  assert_eq!(read(&overlapping, "web", "src/money.ts"), MONEY);
}

#[test]
fn should_refuse_resource_operations_that_create_or_delete_files() {
  let fixture = web();
  let edit = json!({"documentChanges": [
    {"textDocument": {"uri": fixture.uri("web", "src/money.ts"), "version": null}, "edits": [text_edit((0, 16), (0, 23), "cents")]},
    {"kind": "delete", "uri": fixture.uri("web", "src/use.ts")}
  ]});
  serve(&fixture, prepare_to_cents(), edit);

  let response = fixture.ide.edit(&EditRequest::new(rename("toCents", "cents"))).unwrap();

  assert_eq!(
    refusal(&response),
    format!(
      "the server's rename would also delete {}; code-atlas applies renames that edit text and rename files only; nothing was written",
      fixture.file("web", "src/use.ts").display()
    )
  );
  assert!(fixture.file("web", "src/use.ts").exists());
}

#[test]
fn should_refuse_a_rename_plan_once_any_file_changed_after_the_dry_run() {
  let fixture = web();
  serve(&fixture, prepare_to_cents(), rename_to_cents(&fixture, "cents"));
  let dry = fixture.ide.edit(&EditRequest::dry_run(rename("toCents", "cents"))).unwrap();
  fs::write(fixture.file("web", "src/extra.ts"), "export const extra = 1;\n").unwrap();

  let response = fixture.ide.edit(&EditRequest::new(EditOperation::Apply { plan_id: plan_id(&dry) })).unwrap();

  let reason = refusal(&response);
  assert!(reason.starts_with("1 files changed since plan p-"), "{reason}");
  assert!(
    reason.ends_with("a rename or safe delete depends on every file, so run the dry run again; nothing was written"),
    "{reason}"
  );
  assert_eq!(read(&fixture, "web", "src/money.ts"), MONEY);
}

#[test]
fn should_refuse_a_text_plan_whose_file_was_written_externally_and_write_nothing() {
  let fixture = web();
  let replace = EditOperation::ReplaceContent {
    path: "web:src/use.ts".to_owned(),
    pattern: "'1'".to_owned(),
    text: "'3'".to_owned(),
    regex: false,
    all: false,
  };
  let dry = fixture.ide.edit(&EditRequest::dry_run(replace)).unwrap();
  let external = USE.replace("'2'", "'9'");
  fs::write(fixture.file("web", "src/use.ts"), &external).unwrap();

  let response = fixture.ide.edit(&EditRequest::new(EditOperation::Apply { plan_id: plan_id(&dry) })).unwrap();

  assert_eq!(refusal(&response), "changed since planning: web:src/use.ts; nothing was written");
  assert_eq!(read(&fixture, "web", "src/use.ts"), external);
}

#[test]
fn should_roll_back_every_file_when_a_commit_fails_midway() {
  let fixture = core("nested/other");
  let edit = json!({"documentChanges": [
    {"textDocument": {"uri": fixture.uri("core", "src/lib.rs"), "version": 1}, "edits": [text_edit((1, 7), (1, 13), "reply")]},
    {"kind": "rename", "oldUri": fixture.uri("core", "src/nested/other.rs"), "newUri": fixture.uri("core", "src/nested/renamed.rs")}
  ]});
  serve_with(
    &fixture,
    ServerKind::RustAnalyzer,
    vec![
      canned("textDocument/prepareRename", json!({"range": range((1, 7), (1, 13)), "placeholder": "answer"})),
      canned("textDocument/rename", edit),
    ],
  );
  let locked = fixture.file("core", "src/nested");
  let dry = fixture
    .ide
    .edit(&EditRequest::dry_run(EditOperation::Rename {
      target: "answer".to_owned(),
      new_name: "reply".to_owned(),
      cross_repo: CrossRepo::Report,
      allow_unverified: true,
    }))
    .unwrap();
  set_mode(&locked, 0o555);

  let response = fixture.ide.edit(&EditRequest::new(EditOperation::Apply { plan_id: plan_id(&dry) }));
  set_mode(&locked, 0o755);

  let response = response.unwrap();
  assert!(matches!(response.answer.status, EditStatus::RolledBack { .. }), "{}", rendered(&response));
  assert!(
    rendered(&response).starts_with("FAILED: rename answer -> reply was rolled back (create "),
    "{}",
    rendered(&response)
  );
  assert_eq!(read(&fixture, "core", "src/lib.rs"), LIB.replace("other", "nested"));
  assert_eq!(read(&fixture, "core", "src/nested/other.rs"), OTHER);
  assert!(!fixture.file("core", "src/nested/renamed.rs").exists());
}

fn set_mode(path: &std::path::Path, mode: u32) {
  use std::os::unix::fs::PermissionsExt;
  fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn should_report_an_expired_or_unknown_plan_id() {
  let fixture = Fixture::with_plan_ttl(&[("web", &web_files())], Duration::from_millis(1));
  let replace = EditOperation::ReplaceContent {
    path: "src/use.ts".to_owned(),
    pattern: "'1'".to_owned(),
    text: "'3'".to_owned(),
    regex: false,
    all: false,
  };
  let dry = fixture.ide.edit(&EditRequest::dry_run(replace)).unwrap();
  std::thread::sleep(Duration::from_millis(20));

  let expired = fixture.ide.edit(&EditRequest::new(EditOperation::Apply { plan_id: plan_id(&dry) })).unwrap_err();
  let unknown = fixture
    .ide
    .edit(&EditRequest::new(EditOperation::Apply {
      plan_id: "p-000000000000".to_owned(),
    }))
    .unwrap_err();

  assert!(matches!(expired, IdeError::PlanExpired { .. }), "{expired}");
  assert!(expired.to_string().ends_with("run the dry run again"), "{expired}");
  assert_eq!(
    unknown.to_string(),
    "unknown plan p-000000000000: it was never made by this process, was already applied, or was evicted (the last 32 plans are kept); run the dry run again"
  );
  assert_eq!(read(&fixture, "web", "src/use.ts"), USE);
}

fn sdk_and_app() -> Fixture {
  Fixture::new(&[
    (
      "lib",
      &[
        ("package.json", "{\"name\": \"@demo/lib\", \"main\": \"src/index.ts\", \"types\": \"src/index.ts\"}\n"),
        ("tsconfig.json", "{}\n"),
        ("src/index.ts", "export function toCents(amount: string): number {\n  return Number(amount) * 100;\n}\n"),
      ],
    ),
    (
      "app",
      &[
        ("package.json", "{\"name\": \"app\", \"dependencies\": {\"@demo/lib\": \"*\"}}\n"),
        ("tsconfig.json", "{}\n"),
        ("src/form.ts", "import { toCents } from '@demo/lib';\nexport const total = toCents('1');\n"),
        (
          "src/mixed.ts",
          "import { toCents } from '@demo/lib';\n// toCents is also named here\nexport const other = toCents('2');\n",
        ),
      ],
    ),
  ])
}

fn serve_sdk(fixture: &Fixture) {
  let edit = json!({"changes": {fixture.uri("lib", "src/index.ts"): [text_edit((0, 16), (0, 23), "cents")]}});
  serve(fixture, prepare_to_cents(), edit);
}

#[test]
fn should_list_cross_repo_sites_as_not_renamed_by_default() {
  let fixture = sdk_and_app();
  serve_sdk(&fixture);

  let response = fixture.ide.edit(&EditRequest::dry_run(rename("toCents", "cents"))).unwrap();

  let not_renamed: Vec<String> = response.answer.not_renamed.iter().map(ToString::to_string).collect();
  assert_eq!(
    not_renamed,
    vec![
      "outside the language server [resolved]  app:src/form.ts:1:10  src/form.ts  import { toCents } from '@demo/lib';",
      "outside the language server [resolved]  app:src/form.ts:2:22  total  export const total = toCents('1');",
      "outside the language server [resolved]  app:src/mixed.ts:1:10  src/mixed.ts  import { toCents } from '@demo/lib';",
      "outside the language server [resolved]  app:src/mixed.ts:3:22  other  export const other = toCents('2');",
    ]
  );
  assert!(response.answer.not_renamed.iter().all(|site| site.reason == VerifyReason::OutsideLanguageServer));
}

#[test]
fn should_apply_only_authoritative_cross_repo_sites_whose_file_has_no_unexplained_occurrence() {
  let fixture = sdk_and_app();
  serve_sdk(&fixture);

  let response = fixture
    .ide
    .edit(&EditRequest::new(EditOperation::Rename {
      target: "toCents".to_owned(),
      new_name: "cents".to_owned(),
      cross_repo: CrossRepo::Apply,
      allow_unverified: false,
    }))
    .unwrap();

  let text = rendered(&response);
  assert!(text.starts_with("rename toCents -> cents  applied"), "{text}");
  assert_eq!(
    read(&fixture, "app", "src/form.ts"),
    "import { cents } from '@demo/lib';\nexport const total = cents('1');\n"
  );
  assert_eq!(
    read(&fixture, "app", "src/mixed.ts"),
    "import { toCents } from '@demo/lib';\n// toCents is also named here\nexport const other = toCents('2');\n"
  );
  let applied: Vec<String> = response.answer.applied_from_graph.iter().map(|site| site.place()).collect();
  assert_eq!(applied, vec!["app:src/form.ts:1:10", "app:src/form.ts:2:22"]);
  let notes: Vec<Option<String>> = response.answer.not_renamed.iter().map(|site| site.note.clone()).collect();
  assert_eq!(
    notes,
    vec![Some("not applied: the file has 1 other occurrence(s) of toCents that the graph does not explain; rename it by hand".to_owned()); 2]
  );
  assert!(
    text.contains("applied from the graph:\n  outside the language server [resolved]  app:src/form.ts:1:10"),
    "{text}"
  );
}

#[test]
fn should_refuse_safe_delete_while_references_remain_and_delete_nothing() {
  let fixture = web();
  serve_with(
    &fixture,
    ServerKind::TypeScript,
    vec![
      canned("textDocument/references", json!([location(&fixture.uri("web", "src/use.ts"), (2, 9), (2, 16))])),
      canned("textDocument/documentSymbol", money_symbols()),
    ],
  );

  let response = fixture.ide.edit(&EditRequest::new(EditOperation::SafeDelete { target: "toCents".to_owned() })).unwrap();

  assert_eq!(
    rendered(&response),
    concat!(
      "REFUSED: safe_delete toCents: 3 reference(s) to toCents would break; nothing was deleted  (typescript-language-server 5.1.3, TypeScript 5.9.3)\n",
      "blocked by 3 reference(s):\n",
      "  reference [compiler]  web:src/use.ts:3:10  total  return toCents('1') + toCents('2');\n",
      "  outside the language server [resolved]  web:src/use.ts:1:10  src/use.ts  import { toCents } from './money';\n",
      "  outside the language server [resolved]  web:src/use.ts:3:25  total  return toCents('1') + toCents('2');\n",
    )
  );
  assert_eq!(read(&fixture, "web", "src/money.ts"), MONEY);
}

const BANNER: &str = "export const banner = `\n// generated`;\n\n/** Doc. */\nexport function unused(): number {\n  return 1;\n}\n";

#[test]
fn should_delete_an_unreferenced_symbol_without_swallowing_the_previous_item() {
  let fixture = Fixture::new(&[("web", &[("package.json", "{\"name\": \"web\"}\n"), ("src/banner.ts", BANNER)])]);
  let symbols = json!([
    {"name": "banner", "kind": 14, "range": range((0, 13), (1, 13)), "selectionRange": range((0, 13), (0, 19))},
    {"name": "unused", "kind": 12, "range": range((4, 0), (6, 1)), "selectionRange": range((4, 16), (4, 22))}
  ]);
  serve_with(&fixture, ServerKind::TypeScript, vec![canned("textDocument/documentSymbol", symbols)]);

  let response = fixture.ide.edit(&EditRequest::new(EditOperation::SafeDelete { target: "unused".to_owned() })).unwrap();

  assert!(matches!(response.answer.status, EditStatus::Applied), "{}", rendered(&response));
  assert_eq!(read(&fixture, "web", "src/banner.ts"), "export const banner = `\n// generated`;\n");
}

const CONTROLLER: &str = "import { Controller, Get } from '@nestjs/common';\n\n@Controller('users')\nexport class UsersController {\n  /** Finds one. */\n  @Get(':id')\n  find() {\n    return 1;\n  }\n}\n";

#[test]
fn should_keep_decorators_unless_the_new_body_restates_them() {
  let fixture = Fixture::new(&[("web", &[("package.json", "{\"name\": \"web\"}\n"), ("src/users.controller.ts", CONTROLLER)])]);
  let body = |text: &str| {
    EditRequest::new(EditOperation::ReplaceBody {
      target: "UsersController.find".to_owned(),
      text: text.to_owned(),
    })
  };

  let kept = fixture.ide.edit(&body("find() {\n  return 2;\n}")).unwrap();

  assert!(matches!(kept.answer.status, EditStatus::Applied), "{}", rendered(&kept));
  assert_eq!(read(&fixture, "web", "src/users.controller.ts"), CONTROLLER.replace("return 1", "return 2"));
  assert!(
    rendered(&kept).contains("no language server is running for this file: the declaration span comes from the index alone"),
    "{}",
    rendered(&kept)
  );

  let restated = fixture.ide.edit(&body("@Post()\nfind() {\n  return 3;\n}")).unwrap();

  assert!(matches!(restated.answer.status, EditStatus::Applied), "{}", rendered(&restated));
  assert_eq!(
    read(&fixture, "web", "src/users.controller.ts"),
    CONTROLLER.replace("@Get(':id')", "@Post()").replace("return 1", "return 3")
  );
}

#[test]
fn should_refuse_a_symbol_edit_when_a_warm_server_disagrees_about_the_declaration() {
  let fixture = web();
  let symbols = json!([{"name": "toCents", "kind": 12, "range": range((0, 0), (1, 10)), "selectionRange": range((0, 16), (0, 23))}]);
  serve_with(&fixture, ServerKind::TypeScript, vec![canned("textDocument/documentSymbol", symbols)]);
  fixture.ide.references(&ReferencesRequest::new("toCents")).unwrap();

  let response = fixture
    .ide
    .edit(&EditRequest::new(EditOperation::ReplaceBody {
      target: "toCents".to_owned(),
      text: "export function toCents(amount: string): number {\n  return 0;\n}".to_owned(),
    }))
    .unwrap();

  assert_eq!(
    refusal(&response),
    "server and index disagree about the extent of toCents at web:src/money.ts:1: the index spans bytes 0..94, typescript-language-server spans 0..60; nothing was written"
  );
  assert_eq!(read(&fixture, "web", "src/money.ts"), MONEY);
}

fn money_symbols() -> Value {
  json!([{"name": "toCents", "kind": 12, "range": range((0, 0), (2, 1)), "selectionRange": range((0, 16), (0, 23))}])
}

#[test]
fn should_insert_after_a_symbol_and_check_diagnostics_on_a_warm_server() {
  let fixture = web();
  serve_with(&fixture, ServerKind::TypeScript, vec![canned("textDocument/documentSymbol", money_symbols())]);
  fixture.ide.references(&ReferencesRequest::new("toCents")).unwrap();

  let response = fixture
    .ide
    .edit(&EditRequest::new(EditOperation::InsertAfter {
      target: "toCents".to_owned(),
      text: "export function toEuros(cents: number): number {\n  return cents / 100;\n}".to_owned(),
    }))
    .unwrap();

  assert_eq!(
    rendered(&response),
    concat!(
      "insert_after toCents  applied  (typescript-language-server 5.1.3, TypeScript 5.9.3)\n",
      "changed 1 file(s), 1 edit(s):\n",
      "  web:src/money.ts  1\n",
      "diagnostics after edit: no new errors in the 1 edited file(s) (settled)\n",
    )
  );
  assert_eq!(
    read(&fixture, "web", "src/money.ts"),
    format!("{MONEY}\nexport function toEuros(cents: number): number {{\n  return cents / 100;\n}}\n")
  );
}

#[test]
fn should_dry_run_replace_in_files_without_a_selection_and_apply_with_an_expected_count() {
  let fixture = web();
  let operation = |expected_count: Option<usize>| EditOperation::ReplaceInFiles {
    pattern: "toCents\\('(\\d)'\\)".to_owned(),
    text: "toCents('${1}0')".to_owned(),
    regex: true,
    include: Vec::new(),
    exclude: Vec::new(),
    occurrences: None,
    expected_count,
  };

  let implicit = fixture.ide.edit(&EditRequest::new(operation(None))).unwrap();

  assert!(
    matches!(
      implicit.answer.status,
      EditStatus::DryRun {
        reason: DryRunReason::NeedsSelection,
        ..
      }
    ),
    "{}",
    rendered(&implicit)
  );
  assert!(rendered(&implicit).contains("2 matches; this call was a dry run because it named neither occurrences nor expected_count"));
  assert_eq!(read(&fixture, "web", "src/use.ts"), USE);

  let applied = fixture.ide.edit(&EditRequest::new(operation(Some(2)))).unwrap();

  assert!(matches!(applied.answer.status, EditStatus::Applied), "{}", rendered(&applied));
  assert_eq!(read(&fixture, "web", "src/use.ts"), USE.replace("'1'", "'10'").replace("'2'", "'20'"));
  assert!(
    rendered(&applied).contains("diagnostics after edit: not checked (no language server is running for the edited files"),
    "{}",
    rendered(&applied)
  );
}

#[test]
fn should_refuse_an_ambiguous_target_and_list_the_candidates() {
  let fixture = Fixture::new(&[(
    "web",
    &[
      ("package.json", "{\"name\": \"web\"}\n"),
      ("src/a.ts", "export function helper() {}\n"),
      ("src/b.ts", "export function helper() {}\n"),
    ],
  )]);

  let response = fixture.ide.edit(&EditRequest::new(rename("helper", "assist"))).unwrap();

  assert_eq!(
    refusal(&response),
    "\"helper\" matches 2 symbols; give a full key or [repo:]path:line:col; nothing was written"
  );
  assert_eq!(response.answer.candidates.len(), 2);
}

#[test]
fn should_fail_rename_and_safe_delete_without_a_server_and_never_fall_back_to_the_graph() {
  let fixture = web();

  let rename = fixture.ide.edit(&EditRequest::new(rename("toCents", "cents"))).unwrap_err();
  let delete = fixture.ide.edit(&EditRequest::new(EditOperation::SafeDelete { target: "toCents".to_owned() })).unwrap_err();
  fixture.ide.reload_settings(IdeSettings { enabled: false, ..settings() });
  let disabled = fixture.ide.edit(&EditRequest::new(EditOperation::SafeDelete { target: "toCents".to_owned() })).unwrap_err();

  let hint = "typescript-language-server is unavailable: typescript-language-server not found. Install it (npm i -g typescript-language-server typescript@5)";
  assert_eq!(rename.to_string(), hint);
  assert_eq!(delete.to_string(), hint);
  assert!(matches!(disabled, IdeError::Disabled { .. }), "{disabled}");
  assert_eq!(read(&fixture, "web", "src/money.ts"), MONEY);
}

#[test]
fn should_apply_no_graph_site_when_the_server_keeps_the_old_name_as_an_alias() {
  let fixture = sdk_and_app();
  fs::write(fixture.file("lib", "src/reexport.ts"), "export { toCents } from './index';\n").unwrap();
  fs::write(
    fixture.file("lib", "src/direct.ts"),
    "import { toCents } from './index';\nexport const direct = toCents('3');\n",
  )
  .unwrap();
  fs::write(
    fixture.file("lib", "src/aliased.ts"),
    "import { toCents } from './reexport';\nexport const aliased = toCents('4');\n",
  )
  .unwrap();
  let edit = json!({"changes": {
    fixture.uri("lib", "src/index.ts"): [text_edit((0, 16), (0, 23), "cents")],
    fixture.uri("lib", "src/reexport.ts"): [text_edit((0, 9), (0, 16), "cents as toCents")],
  }});
  serve(&fixture, prepare_to_cents(), edit);

  let response = fixture
    .ide
    .edit(&EditRequest::new(EditOperation::Rename {
      target: "toCents".to_owned(),
      new_name: "cents".to_owned(),
      cross_repo: CrossRepo::Apply,
      allow_unverified: false,
    }))
    .unwrap();

  assert_eq!(response.answer.status, EditStatus::Applied, "{}", rendered(&response));
  assert_eq!(read(&fixture, "lib", "src/reexport.ts"), "export { cents as toCents } from './index';\n");
  assert_eq!(
    read(&fixture, "app", "src/form.ts"),
    "import { toCents } from '@demo/lib';\nexport const total = toCents('1');\n"
  );
  assert_eq!(
    read(&fixture, "lib", "src/direct.ts"),
    "import { cents } from './index';\nexport const direct = cents('3');\n"
  );
  let applied: Vec<String> = response.answer.applied_from_graph.iter().map(|site| site.place()).collect();
  assert_eq!(applied, vec!["lib:src/direct.ts:1:10", "lib:src/direct.ts:2:23"]);
  assert_eq!(response.answer.not_renamed.len(), 6);
  assert!(response.answer.unverified.is_empty());
  assert_eq!(
    read(&fixture, "lib", "src/aliased.ts"),
    "import { toCents } from './reexport';\nexport const aliased = toCents('4');\n"
  );
  assert!(
    response
      .answer
      .not_renamed
      .iter()
      .all(|site| site.note.as_deref() == Some("via kept alias `cents as toCents`; it still compiles through the alias and was not changed"))
  );
  assert_eq!(
    response.answer.caveats,
    vec![
      "the language server kept the old name toCents at lib:src/reexport.ts:1:10 (\"cents as toCents\"); code that reaches it through those places keeps working under the old name"
    ]
  );
}

fn rename_with(target: &str, new_name: &str, allow_unverified: bool) -> EditOperation {
  EditOperation::Rename {
    target: target.to_owned(),
    new_name: new_name.to_owned(),
    cross_repo: CrossRepo::Report,
    allow_unverified,
  }
}

#[test]
fn should_refuse_to_apply_a_rename_that_leaves_same_repo_graph_sites_unless_allowed() {
  let fixture = web();
  let declaration_only = json!({"changes": {
    fixture.uri("web", "src/money.ts"): [text_edit((0, 16), (0, 23), "cents")],
    fixture.uri("web", "src/use.ts"): [text_edit((0, 9), (0, 16), "cents")],
  }});
  serve(&fixture, prepare_to_cents(), declaration_only);

  let dry = fixture.ide.edit(&EditRequest::dry_run(rename_with("toCents", "cents", false))).unwrap();
  let refused = fixture.ide.edit(&EditRequest::new(EditOperation::Apply { plan_id: plan_id(&dry) })).unwrap();

  let shown = rendered(&dry);
  assert!(
    shown.contains(concat!(
      "UNVERIFIED: 2 site(s) the index resolves to this symbol in the same repo were not renamed by the language server; ",
      "applying this plan refuses unless allow_unverified is set:\n",
      "  outside the language server [resolved]  web:src/use.ts:3:10"
    )),
    "{shown}"
  );
  assert!(refusal(&refused).starts_with("2 site(s) the index resolves to this symbol in the same repo were not renamed"));
  assert_eq!(read(&fixture, "web", "src/use.ts"), USE);

  let allowed = fixture.ide.edit(&EditRequest::new(rename_with("toCents", "cents", true))).unwrap();

  assert_eq!(allowed.answer.status, EditStatus::Applied, "{}", rendered(&allowed));
  assert_eq!(allowed.answer.unverified.len(), 2);
  assert_eq!(
    read(&fixture, "web", "src/use.ts"),
    "import { cents } from './money';\nexport function total(): number {\n  return toCents('1') + toCents('2');\n}\n"
  );
}

#[test]
fn should_refuse_a_rename_onto_a_name_that_already_exists_in_the_same_scope_or_is_bound_in_an_edited_file() {
  let fixture = Fixture::new(&[(
    "web",
    &[
      ("package.json", "{\"name\": \"web\"}\n"),
      (
        "src/money.ts",
        "export function toCents(amount: string): number {\n  return Number(amount) * 100;\n}\nexport function cents(): number {\n  return 1;\n}\n",
      ),
      (
        "src/use.ts",
        "import { toCents } from './money';\nconst euros = 2;\nexport const total = toCents('1') + euros;\n",
      ),
    ],
  )]);
  let edit = json!({"changes": {
    fixture.uri("web", "src/money.ts"): [text_edit((0, 16), (0, 23), "euros")],
    fixture.uri("web", "src/use.ts"): [text_edit((0, 9), (0, 16), "euros"), text_edit((2, 21), (2, 28), "euros")],
  }});
  serve(&fixture, prepare_to_cents(), edit);

  let existing = fixture.ide.edit(&EditRequest::new(rename("toCents", "cents"))).unwrap();
  let shadowing = fixture.ide.edit(&EditRequest::new(rename("toCents", "euros"))).unwrap();

  assert_eq!(
    refusal(&existing),
    "cents already exists in the same scope (function cents at web:src/money.ts:4); the rename would clash with it; nothing was written"
  );
  assert_eq!(
    refusal(&shadowing),
    "euros is already bound in web:src/use.ts:2 (const euros = 2;), in a scope the rename edits; the rename would clash with or shadow it; nothing was written"
  );
  assert!(read(&fixture, "web", "src/money.ts").contains("export function toCents("));
}

const PAD: &str =
  "export function pad(value: string): string;\nexport function pad(value: number): string;\nexport function pad(value: unknown): string {\n  return String(value);\n}\n";

#[test]
fn should_list_sites_of_every_overload_and_refuse_symbol_edits_on_one_signature() {
  let fixture = Fixture::new(&[(
    "web",
    &[
      ("package.json", "{\"name\": \"web\"}\n"),
      ("src/pad.ts", PAD),
      ("src/use.ts", "import { pad } from './pad';\nexport const padded = pad(1);\n"),
    ],
  )]);
  let edit = json!({"changes": {fixture.uri("web", "src/pad.ts"): [
    text_edit((0, 16), (0, 19), "fill"), text_edit((1, 16), (1, 19), "fill"), text_edit((2, 16), (2, 19), "fill")
  ]}});
  serve(&fixture, json!({"range": range((0, 16), (0, 19)), "placeholder": "pad"}), edit);

  let response = fixture.ide.edit(&EditRequest::dry_run(rename_with("pad", "fill", false))).unwrap();
  let body = fixture
    .ide
    .edit(&EditRequest::new(EditOperation::ReplaceBody {
      target: "pad".to_owned(),
      text: "export function pad(value: string): string;".to_owned(),
    }))
    .unwrap();

  let places: Vec<String> = response.answer.not_renamed.iter().map(|site| site.place()).collect();
  assert_eq!(places, vec!["web:src/use.ts:1:10", "web:src/use.ts:2:23"]);
  assert_eq!(response.answer.unverified.len(), 2);
  assert_eq!(
    refusal(&body),
    "pad is one of 3 overload signatures (lines 1, 2, 3); a symbol edit would change only one of them, so edit the set with replace_content; nothing was written"
  );
  assert_eq!(read(&fixture, "web", "src/pad.ts"), PAD);
}

#[test]
fn should_answer_reads_but_refuse_edits_when_rust_analyzer_reports_an_unhealthy_workspace() {
  let fixture = core("other");
  let mut steps = initialize("rust-analyzer", edit_capabilities());
  steps.push(json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "error", "quiescent": true, "message": "Failed to load workspaces: fixtures/broken/Cargo.toml"}}}));
  steps.extend([
    canned("textDocument/documentSymbol", json!([])),
    canned("textDocument/references", json!([])),
    canned("textDocument/prepareRename", json!({"range": range((1, 7), (1, 13)), "placeholder": "answer"})),
  ]);
  fixture.serve(ServerKind::RustAnalyzer, fixture.script("ra", &steps));

  let outline = fixture.ide.outline(&lsp_outline("core:src/lib.rs")).unwrap();
  let references = fixture.ide.references(&ReferencesRequest::new("answer")).unwrap();
  let rename = fixture.ide.edit(&EditRequest::new(rename("answer", "reply"))).unwrap();

  let partial = concat!(
    "\nPARTIAL: rust-analyzer reports an unhealthy workspace and code-atlas cannot tell whether this file's project loaded ",
    "(Failed to load workspaces: fixtures/broken/Cargo.toml); the answer may be incomplete\n"
  );
  assert!(text(&outline).contains(partial), "{}", text(&outline));
  assert!(text(&references).contains(partial), "{}", text(&references));
  assert!(text(&references).contains("  (no references)\n"), "{}", text(&references));
  assert!(
    refusal(&rename)
      .contains("rust-analyzer reports an unhealthy workspace (Failed to load workspaces: fixtures/broken/Cargo.toml); fix the failing projects or exclude them with lsp.exclude"),
    "{}",
    rendered(&rename)
  );
  assert_eq!(read(&fixture, "core", "src/lib.rs"), LIB);
}

const CSS: &str = "export const buildStyleString = (a: string): [string, string] => {\n  let styleString = ''\n  styleString += a\n  return [a, styleString]\n}\n// mentioned: styleString\nexport const quoted = 'mentioned'\nexport const keyframes = (a: string): string => {\n  const [first, styleString] = buildStyleString(a)\n  return first + styleString\n}\n";

fn css(new_name: &str) -> (Fixture, Value) {
  let fixture = Fixture::new(&[("web", &[("package.json", "{\"name\": \"web\"}\n"), ("src/css.ts", CSS)])]);
  let edit = json!({"changes": {fixture.uri("web", "src/css.ts"): [text_edit((0, 13), (0, 29), new_name), text_edit((8, 31), (8, 47), new_name)]}});
  serve(&fixture, json!({"range": range((0, 13), (0, 29)), "placeholder": "buildStyleString"}), edit.clone());
  (fixture, edit)
}

#[test]
fn should_refuse_a_binding_in_an_edited_scope_and_ignore_comments_strings_and_other_scopes() {
  let (clash, _) = css("styleString");
  let (mentioned, _) = css("mentioned");

  let refused = clash.ide.edit(&EditRequest::new(rename("buildStyleString", "styleString"))).unwrap();
  let applied = mentioned.ide.edit(&EditRequest::new(rename("buildStyleString", "mentioned"))).unwrap();

  assert_eq!(
    refusal(&refused),
    "styleString is already bound in web:src/css.ts:9 (const [first, styleString] = buildStyleString(a)), in a scope the rename edits; the rename would clash with or shadow it; nothing was written"
  );
  assert_eq!(applied.answer.status, EditStatus::Applied, "{}", rendered(&applied));
  assert_eq!(read(&mentioned, "web", "src/css.ts"), CSS.replace("buildStyleString", "mentioned"));
}

const COUNTER: &str = "export class Counter {\n  count(): number {\n    return 1\n  }\n  report(): number {\n    const total = this.count()\n    return total\n  }\n}\n";

#[test]
fn should_not_check_locals_when_renaming_a_member() {
  let fixture = Fixture::new(&[("web", &[("package.json", "{\"name\": \"web\"}\n"), ("src/counter.ts", COUNTER)])]);
  let edit = json!({"changes": {fixture.uri("web", "src/counter.ts"): [text_edit((1, 2), (1, 7), "total"), text_edit((5, 23), (5, 28), "total")]}});
  serve(&fixture, json!({"range": range((1, 2), (1, 7)), "placeholder": "count"}), edit);

  let response = fixture.ide.edit(&EditRequest::new(rename("Counter.count", "total"))).unwrap();

  assert_eq!(response.answer.status, EditStatus::Applied, "{}", rendered(&response));
  assert_eq!(read(&fixture, "web", "src/counter.ts"), COUNTER.replace("count()", "total()"));
}

const STYLE: &str = "export const composeStyle = (a: string, b: string): string => {\n  return a + b\n}\n\nexport const decodeBuffer = (buffer: ArrayBuffer): string => {\n  return new TextDecoder().decode(buffer)\n}\n\nexport const UNUSED_LIMIT = 10;\n\nexport const first = 1, second = 2\n";

fn style() -> Fixture {
  Fixture::new(&[("web", &[("package.json", "{\"name\": \"web\"}\n"), ("src/style.ts", STYLE)])])
}

fn body(target: &str, text: &str) -> EditRequest {
  EditRequest::new(EditOperation::ReplaceBody {
    target: target.to_owned(),
    text: text.to_owned(),
  })
}

#[test]
fn should_replace_an_exported_const_as_a_whole_statement() {
  let fixture = style();

  let compose = fixture
    .ide
    .edit(&body("composeStyle", "export const composeStyle = (a: string): string => {\n  return a\n}"))
    .unwrap();
  let decode = fixture
    .ide
    .edit(&body(
      "decodeBuffer",
      "export const decodeBuffer = (buffer: ArrayBuffer): string => {\n  return String(buffer)\n}",
    ))
    .unwrap();
  let bare = fixture.ide.edit(&body("decodeBuffer", "decodeBuffer = (buffer: ArrayBuffer): string => ''")).unwrap();
  let after = fixture
    .ide
    .edit(&EditRequest::new(EditOperation::InsertAfter {
      target: "decodeBuffer".to_owned(),
      text: "export const encodeText = (text: string): Uint8Array => new TextEncoder().encode(text)".to_owned(),
    }))
    .unwrap();

  assert_eq!(compose.answer.status, EditStatus::Applied, "{}", rendered(&compose));
  assert_eq!(decode.answer.status, EditStatus::Applied, "{}", rendered(&decode));
  assert_eq!(after.answer.status, EditStatus::Applied, "{}", rendered(&after));
  assert_eq!(
    refusal(&bare),
    "replace_body replaces the whole `export const decodeBuffer` statement; restate it from `export const`; nothing was written"
  );
  assert_eq!(
    read(&fixture, "web", "src/style.ts"),
    concat!(
      "export const composeStyle = (a: string): string => {\n  return a\n}\n\n",
      "export const decodeBuffer = (buffer: ArrayBuffer): string => {\n  return String(buffer)\n}\n\n",
      "export const encodeText = (text: string): Uint8Array => new TextEncoder().encode(text)\n\n",
      "export const UNUSED_LIMIT = 10;\n\nexport const first = 1, second = 2\n",
    )
  );
}

#[test]
fn should_refuse_a_symbol_edit_on_one_of_several_declarators() {
  let fixture = style();

  let response = fixture.ide.edit(&body("first", "export const first = 3")).unwrap();

  assert_eq!(
    refusal(&response),
    "first is declared together with other variables in one statement; edit it with replace_content; nothing was written"
  );
  assert_eq!(read(&fixture, "web", "src/style.ts"), STYLE);
}

#[test]
fn should_safe_delete_an_unused_exported_const() {
  let fixture = style();
  let symbols = json!([{"name": "UNUSED_LIMIT", "kind": 14, "range": range((8, 13), (8, 30)), "selectionRange": range((8, 13), (8, 25))}]);
  serve_with(&fixture, ServerKind::TypeScript, vec![canned("textDocument/documentSymbol", symbols)]);

  let response = fixture
    .ide
    .edit(&EditRequest::new(EditOperation::SafeDelete {
      target: "UNUSED_LIMIT".to_owned(),
    }))
    .unwrap();

  assert_eq!(response.answer.status, EditStatus::Applied, "{}", rendered(&response));
  assert_eq!(read(&fixture, "web", "src/style.ts"), STYLE.replace("export const UNUSED_LIMIT = 10;\n\n", ""));
}

const SHAPES: &str = "export const { alpha, beta } = { alpha: 1, beta: 2 }\n\nexport const\n  split = 3\n\nexport class Box {\n  size(): number {\n    return 1\n  }\n}\n";

#[test]
fn should_refuse_symbol_edits_on_destructured_bindings_and_keywords_the_span_lacks() {
  let fixture = Fixture::new(&[("web", &[("package.json", "{\"name\": \"web\"}\n"), ("src/shapes.ts", SHAPES)])]);
  let symbols = json!([{"name": "alpha", "kind": 14, "range": range((0, 13), (0, 52)), "selectionRange": range((0, 15), (0, 20))}]);
  serve_with(&fixture, ServerKind::TypeScript, vec![canned("textDocument/documentSymbol", symbols)]);

  let replaced = fixture.ide.edit(&body("alpha", "export const alpha = 3")).unwrap();
  let inserted = fixture
    .ide
    .edit(&EditRequest::new(EditOperation::InsertAfter {
      target: "beta".to_owned(),
      text: "export const gamma = 4".to_owned(),
    }))
    .unwrap();
  let deleted = fixture.ide.edit(&EditRequest::new(EditOperation::SafeDelete { target: "alpha".to_owned() })).unwrap();
  let method = fixture.ide.edit(&body("Box.size", "export const size = (): number => 2")).unwrap();

  let destructured = "alpha is bound by a destructuring pattern together with other bindings; edit it with replace_content; nothing was written";
  assert_eq!(refusal(&replaced), destructured);
  assert_eq!(
    refusal(&inserted),
    "beta is bound by a destructuring pattern together with other bindings; edit it with replace_content; nothing was written"
  );
  assert_eq!(refusal(&deleted), destructured);
  assert_eq!(
    refusal(&method),
    "the new text starts with `export const`, which the declaration of size does not contain; nothing was written"
  );
  assert_eq!(read(&fixture, "web", "src/shapes.ts"), SHAPES);
}

#[test]
fn should_widen_a_declarator_whose_keywords_sit_on_the_previous_line() {
  let fixture = Fixture::new(&[("web", &[("package.json", "{\"name\": \"web\"}\n"), ("src/shapes.ts", SHAPES)])]);

  let response = fixture.ide.edit(&body("split", "export const split = 30")).unwrap();

  assert_eq!(response.answer.status, EditStatus::Applied, "{}", rendered(&response));
  assert_eq!(
    read(&fixture, "web", "src/shapes.ts"),
    SHAPES.replace("export const\n  split = 3\n", "export const split = 30\n")
  );
}

#[test]
fn should_open_a_file_beside_an_unreadable_tsconfig_and_refuse_when_none_exists() {
  let mut files = web_files();
  files.push(("tests/tsconfig.json", "{ \"include\": [ broken\n"));
  files.push(("tests/t1.ts", "import { toCents } from '../src/money';\nexport const t = toCents('1');\n"));
  files.push(("empty/tsconfig.json", "not json at all\n"));
  let fixture = Fixture::new(&[("web", &files)]);
  serve(&fixture, prepare_to_cents(), rename_to_cents(&fixture, "cents"));

  let dry = fixture.ide.edit(&EditRequest::dry_run(rename("toCents", "cents"))).unwrap();
  let refused = fixture.ide.edit(&EditRequest::new(EditOperation::Apply { plan_id: plan_id(&dry) })).unwrap();

  let opened: Vec<String> = fixture
    .recorded("ts")
    .iter()
    .filter(|message| message["method"] == "textDocument/didOpen")
    .filter_map(|message| message["params"]["textDocument"]["uri"].as_str().map(str::to_owned))
    .collect();
  assert!(opened.contains(&fixture.uri("web", "tests/t1.ts")), "{opened:?}");
  assert!(
    dry.answer.caveats.contains(
      &"tests/tsconfig.json could not be read as a tsconfig, so code-atlas cannot tell what it includes; tests/t1.ts was opened so the language server loads it".to_owned()
    ),
    "{:?}",
    dry.answer.caveats
  );
  assert_eq!(dry.answer.unloaded_projects, vec!["empty/tsconfig.json".to_owned()]);
  assert!(
    rendered(&dry)
      .contains("UNLOADED: the language server could not load empty/tsconfig.json, so sites there were not checked; applying this plan refuses unless allow_unverified is set"),
    "{}",
    rendered(&dry)
  );
  assert!(
    refusal(&refused).starts_with("the language server could not load empty/tsconfig.json"),
    "{}",
    rendered(&refused)
  );
  assert_eq!(read(&fixture, "web", "src/money.ts"), MONEY);
}
