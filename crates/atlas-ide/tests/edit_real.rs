mod common;

use std::fs;

use atlas_ide::{CrossRepo, DiagnosticsDelta, EditOperation, EditOutcome, EditRequest, EditStatus, LaunchSpec, VerifyReason};
use atlas_lsp::{Launch, ServerCommand, ServerKind};
use atlas_query::{Response, Targeted};
use common::*;

fn rename(target: &str, new_name: &str, cross_repo: CrossRepo) -> EditOperation {
  EditOperation::Rename {
    target: target.to_owned(),
    new_name: new_name.to_owned(),
    cross_repo,
    allow_unverified: false,
  }
}

fn read(fixture: &Fixture, repo: &str, path: &str) -> String {
  fs::read_to_string(fixture.file(repo, path)).unwrap()
}

fn plan_id(response: &Response<EditOutcome>) -> String {
  match &response.answer.status {
    EditStatus::DryRun { plan_id, .. } => plan_id.clone(),
    other => panic!("expected a dry run, got {other:?}"),
  }
}

fn body(response: &Response<EditOutcome>) -> Vec<String> {
  text(response).lines().skip(1).map(str::to_owned).collect()
}

fn assert_fresh(fixture: &Fixture, present: &str, gone: &str) {
  let query = fixture.ide.query();
  query.fresh().unwrap();
  assert!(matches!(query.resolve_target(present).unwrap(), Targeted::Found(_)), "{present} is not in the index");
  assert!(matches!(query.resolve_target(gone).unwrap(), Targeted::NotFound { .. }), "{gone} is still in the index");
}

fn assert_no_new_errors(response: &Response<EditOutcome>) {
  match &response.answer.diagnostics {
    Some(DiagnosticsDelta::Checked { new_errors, files, unsettled, .. }) => {
      assert!(new_errors.is_empty(), "{new_errors:?}");
      assert!(unsettled.is_empty(), "{unsettled:?}");
      assert!(*files > 0);
    }
    other => panic!("expected a diagnostics delta, got {other:?}"),
  }
}

#[test]
fn should_rename_users_service_find_one_like_the_ide_with_typescript_language_server() {
  let test = "should_rename_users_service_find_one_like_the_ide_with_typescript_language_server";
  let Some(fixture) = typescript_fixture(test, "acme", &["acme-api", "acme-sdk", "acme-web"]) else {
    return;
  };

  let dry = fixture
    .ide
    .edit(&EditRequest::dry_run(rename("UsersService.findOne", "getById", CrossRepo::Report)))
    .unwrap();

  let summary: Vec<String> = body(&dry).into_iter().filter(|line| !line.starts_with("    ")).collect();
  assert_eq!(
    summary,
    vec![
      "would change 3 file(s), 5 edit(s):",
      "  acme-api:src/users/users.controller.ts    1",
      "  acme-api:src/users/users.service.spec.ts  2",
      "  acme-api:src/users/users.service.ts       2",
      "not renamed, verify:",
      "  (none)",
    ]
  );

  let applied = fixture.ide.edit(&EditRequest::new(EditOperation::Apply { plan_id: plan_id(&dry) })).unwrap();

  assert_eq!(applied.answer.status, EditStatus::Applied, "{}", text(&applied));
  let service = read(&fixture, "acme-api", "src/users/users.service.ts");
  assert!(
    service.contains("async getById(id: string): Promise<User> {") && service.contains("await this.getById(id);"),
    "{service}"
  );
  assert!(service.contains("this.users.findOneBy({ id })") && !service.contains("findOne("), "{service}");
  assert!(read(&fixture, "acme-api", "src/users/users.controller.ts").contains("this.usersService.getById(id)"));
  assert_eq!(read(&fixture, "acme-api", "src/users/users.service.spec.ts").matches("service.getById(").count(), 2);
  assert_fresh(&fixture, "UsersService.getById", "UsersService.findOne");
  assert_no_new_errors(&applied);
}

#[test]
fn should_list_the_jest_mocks_of_orders_service_find_one_as_not_renamed() {
  let test = "should_list_the_jest_mocks_of_orders_service_find_one_as_not_renamed";
  let Some(fixture) = typescript_fixture(test, "acme", &["acme-api", "acme-sdk", "acme-web"]) else {
    return;
  };
  let spec = read(&fixture, "acme-api", "src/orders/orders.controller.spec.ts");

  let response = fixture.ide.edit(&EditRequest::new(rename("OrdersService.findOne", "fetchOne", CrossRepo::Report))).unwrap();

  assert_eq!(response.answer.status, EditStatus::Applied, "{}", text(&response));
  let not_renamed: Vec<(VerifyReason, String)> = response.answer.not_renamed.iter().map(|site| (site.reason, site.place())).collect();
  assert_eq!(
    not_renamed,
    vec![
      (VerifyReason::Mock, "acme-api:src/orders/orders.controller.spec.ts:5:27".to_owned()),
      (VerifyReason::Mock, "acme-api:src/orders/orders.controller.spec.ts:11:19".to_owned()),
      (VerifyReason::Mock, "acme-api:src/orders/orders.controller.spec.ts:13:26".to_owned()),
    ]
  );
  assert!(
    text(&response).contains("  mock [heuristic 0.60]  acme-api:src/orders/orders.controller.spec.ts:5:27"),
    "{}",
    text(&response)
  );
  assert_eq!(read(&fixture, "acme-api", "src/orders/orders.controller.spec.ts"), spec);
  let controller = read(&fixture, "acme-api", "src/orders/orders.controller.ts");
  assert!(
    controller.contains("  findOne(@Param('id') id: string) {\n    return this.ordersService.fetchOne(id);"),
    "{controller}"
  );
  assert_fresh(&fixture, "OrdersService.fetchOne", "OrdersService.findOne");
  assert_no_new_errors(&response);
}

#[test]
fn should_keep_acme_web_untouched_when_the_server_keeps_to_cents_as_an_alias() {
  let test = "should_keep_acme_web_untouched_when_the_server_keeps_to_cents_as_an_alias";
  let Some(fixture) = typescript_fixture(test, "acme", &["acme-api", "acme-sdk", "acme-web"]) else {
    return;
  };
  let form = read(&fixture, "acme-web", "src/components/OrderForm.tsx");

  let response = fixture.ide.edit(&EditRequest::new(rename("toCents", "toMinorUnits", CrossRepo::Apply))).unwrap();

  assert_eq!(response.answer.status, EditStatus::Applied, "{}", text(&response));
  assert_eq!(
    read(&fixture, "acme-sdk", "src/index.ts").lines().last(),
    Some("export { toMinorUnits as toCents } from './money';")
  );
  assert_eq!(read(&fixture, "acme-web", "src/components/OrderForm.tsx"), form);
  assert!(response.answer.applied_from_graph.is_empty());
  let sites: Vec<(VerifyReason, String, bool)> = response
    .answer
    .not_renamed
    .iter()
    .map(|site| {
      (
        site.reason,
        site.place(),
        site.note.as_deref().is_some_and(|note| note.contains("via kept alias `toMinorUnits as toCents`")),
      )
    })
    .collect();
  assert_eq!(
    sites,
    vec![
      (VerifyReason::OutsideLanguageServer, "acme-web:src/components/OrderForm.tsx:1:10".to_owned(), true),
      (VerifyReason::OutsideLanguageServer, "acme-web:src/components/OrderForm.tsx:15:44".to_owned(), true),
    ]
  );
  assert!(
    response
      .answer
      .caveats
      .iter()
      .any(|caveat| caveat.starts_with("the language server kept the old name toCents at acme-sdk:src/index.ts:3:10")),
    "{:?}",
    response.answer.caveats
  );
  assert_fresh(&fixture, "toMinorUnits", "toCents");
  assert_no_new_errors(&response);
}

#[test]
fn should_apply_authoritative_to_cents_sites_in_acme_web_from_the_graph() {
  let test = "should_apply_authoritative_to_cents_sites_in_acme_web_from_the_graph";
  let Some(fixture) = typescript_fixture(test, "acme", &["acme-api", "acme-sdk", "acme-web"]) else {
    return;
  };
  let manifest = read(&fixture, "acme-sdk", "package.json")
    .replace("dist/index.js", "src/index.ts")
    .replace("dist/index.d.ts", "src/index.ts");
  fs::write(fixture.file("acme-sdk", "package.json"), manifest).unwrap();
  let index = read(&fixture, "acme-sdk", "src/index.ts").replace("export { toCents } from './money';", "export * from './money';");
  fs::write(fixture.file("acme-sdk", "src/index.ts"), index).unwrap();

  let reported = fixture.ide.edit(&EditRequest::dry_run(rename("toCents", "toMinorUnits", CrossRepo::Report))).unwrap();
  let response = fixture.ide.edit(&EditRequest::new(rename("toCents", "toMinorUnits", CrossRepo::Apply))).unwrap();

  let listed: Vec<(VerifyReason, String, bool)> = reported.answer.not_renamed.iter().map(|site| (site.reason, site.place(), site.authoritative)).collect();
  assert_eq!(
    listed,
    vec![
      (VerifyReason::OutsideLanguageServer, "acme-web:src/components/OrderForm.tsx:1:10".to_owned(), true),
      (VerifyReason::OutsideLanguageServer, "acme-web:src/components/OrderForm.tsx:15:44".to_owned(), true),
    ]
  );
  assert_eq!(response.answer.status, EditStatus::Applied, "{}", text(&response));
  let applied: Vec<String> = response.answer.applied_from_graph.iter().map(|site| site.place()).collect();
  assert_eq!(applied, vec!["acme-web:src/components/OrderForm.tsx:1:10", "acme-web:src/components/OrderForm.tsx:15:44"]);
  assert!(response.answer.not_renamed.is_empty(), "{}", text(&response));
  let form = read(&fixture, "acme-web", "src/components/OrderForm.tsx");
  assert!(form.starts_with("import { toMinorUnits } from '@acme/sdk';\n"), "{form}");
  assert!(form.contains("subtotalCents: toMinorUnits(amount)") && !form.contains("toCents"), "{form}");
  assert!(text(&response).contains("applied from the graph:\n  outside the language server [resolved]  acme-web:src/components/OrderForm.tsx:1:10"));
  assert_fresh(&fixture, "toMinorUnits", "toCents");
  let query = fixture.ide.query();
  let Targeted::Found(renamed) = query.resolve_target("toMinorUnits").unwrap() else {
    panic!("toMinorUnits is not in the index")
  };
  let web_sites: Vec<String> = query
    .graph_references(&renamed)
    .unwrap()
    .references
    .iter()
    .filter_map(|row| row.location.as_ref().filter(|location| location.repo == "acme-web").map(ToString::to_string))
    .collect();
  assert_eq!(web_sites, vec!["acme-web:src/components/OrderForm.tsx:1:10", "acme-web:src/components/OrderForm.tsx:15:44"]);
  assert_no_new_errors(&response);
}

fn rust_fixture(test: &str, fixture: Fixture) -> Option<Fixture> {
  let binary = match rust_analyzer() {
    Ok(binary) => binary,
    Err(reason) => {
      eprintln!("SKIPPED {test}: rust-analyzer unavailable ({reason})");
      return None;
    }
  };
  fixture.ide.reload_settings(atlas_ide::IdeSettings {
    ready_timeout_secs: 300,
    ..settings()
  });
  fixture.launcher.set(
    ServerKind::RustAnalyzer,
    LaunchSpec {
      command: ServerCommand::new(binary),
      launch: Launch::Installed,
      server_version: "located".to_owned(),
      tsserver: None,
    },
  );
  Some(fixture)
}

const WORKSPACE: &str = "[workspace]\nmembers = [\"money\", \"app\"]\nresolver = \"2\"\n";
const MONEY_CRATE: &str = "[package]\nname = \"money\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const APP_CRATE: &str = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nmoney = { path = \"../money\" }\n";
const MONEY_LIB: &str = "/// Converts an amount to cents.\npub fn to_cents(amount: f64) -> i64 {\n  (amount * 100.0).round() as i64\n}\n";
const APP_MAIN: &str = "mod report;\n\nuse money::to_cents;\n\nfn main() {\n  println!(\"{} {}\", to_cents(1.5), report::total());\n}\n";
const APP_REPORT: &str = "pub fn total() -> i64 {\n  money::to_cents(2.0) + money::to_cents(0.5)\n}\n";

fn shop() -> Fixture {
  Fixture::new(&[(
    "shop",
    &[
      ("Cargo.toml", WORKSPACE),
      ("money/Cargo.toml", MONEY_CRATE),
      ("money/src/lib.rs", MONEY_LIB),
      ("app/Cargo.toml", APP_CRATE),
      ("app/src/main.rs", APP_MAIN),
      ("app/src/report.rs", APP_REPORT),
    ],
  )])
}

#[test]
fn should_rename_a_function_across_the_crates_of_a_cargo_workspace_with_rust_analyzer() {
  let test = "should_rename_a_function_across_the_crates_of_a_cargo_workspace_with_rust_analyzer";
  let Some(fixture) = rust_fixture(test, shop()) else { return };

  let response = fixture.ide.edit(&EditRequest::new(rename("to_cents", "to_minor_units", CrossRepo::Report))).unwrap();

  assert_eq!(
    body(&response),
    vec![
      "changed 3 file(s), 5 edit(s):",
      "  shop:app/src/main.rs    2",
      "  shop:app/src/report.rs  2",
      "  shop:money/src/lib.rs   1",
      "not renamed, verify:",
      "  (none)",
      "diagnostics after edit: no new native errors in the 3 edited file(s) (settled; cargo check not run)",
    ]
  );
  assert!(
    text(&response).starts_with("rename to_cents -> to_minor_units  applied  (rust-analyzer "),
    "{}",
    text(&response)
  );
  assert_eq!(read(&fixture, "shop", "money/src/lib.rs"), MONEY_LIB.replace("to_cents", "to_minor_units"));
  assert_eq!(read(&fixture, "shop", "app/src/main.rs"), APP_MAIN.replace("to_cents", "to_minor_units"));
  assert_eq!(read(&fixture, "shop", "app/src/report.rs"), APP_REPORT.replace("to_cents", "to_minor_units"));
  assert_fresh(&fixture, "to_minor_units", "to_cents");
  assert_no_new_errors(&response);
}

#[test]
fn should_rename_store_get_and_every_impl_with_rust_analyzer() {
  let test = "should_rename_store_get_and_every_impl_with_rust_analyzer";
  let Some(fixture) = rust_fixture(test, Fixture::copied("adversarial", &["trait-dispatch-rust"])) else {
    return;
  };
  let before = read(&fixture, "trait-dispatch-rust", "src/lib.rs");

  let response = fixture.ide.edit(&EditRequest::new(rename("Store::get", "fetch", CrossRepo::Report))).unwrap();

  assert_eq!(
    body(&response)[..3],
    ["changed 1 file(s), 7 edit(s):", "  trait-dispatch-rust:src/lib.rs  7", "not renamed, verify:"]
  );
  let after = read(&fixture, "trait-dispatch-rust", "src/lib.rs");
  assert_eq!(after.matches("fn fetch(&self, key: &str) -> Option<String>").count(), 3);
  assert_eq!(after.matches("store.fetch(key)").count(), 4);
  assert_eq!(after.matches("self.entries.get(key)").count(), before.matches("self.entries.get(key)").count());
  assert_fresh(&fixture, "Store::fetch", "Store::get");
  assert_no_new_errors(&response);
}

fn typescript_command(test: &str) -> Option<ServerCommand> {
  match typescript_language_server() {
    Ok(command) => Some(command),
    Err(reason) => {
      eprintln!("SKIPPED {test}: typescript-language-server unavailable ({reason})");
      None
    }
  }
}

fn serve_typescript(fixture: &Fixture, command: ServerCommand) {
  fixture.launcher.set(
    ServerKind::TypeScript,
    LaunchSpec {
      tsserver: None,
      ..launch(ServerKind::TypeScript, command)
    },
  );
}

fn typescript_tree(test: &str, repos: &[(&str, &[(&str, &str)])]) -> Option<Fixture> {
  let command = typescript_command(test)?;
  let fixture = Fixture::new(repos);
  serve_typescript(&fixture, command);
  Some(fixture)
}

const CLEAN_MONEY: &str = "export function toCents(amount: number): number {\n  return Math.round(amount * 100);\n}\n";
const CLEAN_USE: &str = "import { toCents } from './money';\n\nexport function total(): number {\n  return toCents(1.5) + toCents(2);\n}\n";

#[test]
fn should_settle_the_diagnostics_delta_of_clean_typescript_files() {
  let test = "should_settle_the_diagnostics_delta_of_clean_typescript_files";
  let files: &[(&str, &str)] = &[
    ("package.json", "{\"name\": \"clean\"}\n"),
    ("tsconfig.json", "{\"compilerOptions\": {\"strict\": true, \"noEmit\": true}, \"include\": [\"src\"]}\n"),
    ("src/money.ts", CLEAN_MONEY),
    ("src/use.ts", CLEAN_USE),
  ];
  let Some(command) = typescript_command(test) else { return };
  let fixture = Fixture::with_options(&[("clean", files)], |options| atlas_ide::IdeOptions {
    diagnostics_timeout: std::time::Duration::from_secs(60),
    ..options
  });
  serve_typescript(&fixture, command);

  let response = fixture.ide.edit(&EditRequest::new(rename("toCents", "toMinorUnits", CrossRepo::Report))).unwrap();
  let diagnostics = fixture.ide.diagnostics(&atlas_ide::DiagnosticsRequest::new("clean:src/use.ts")).unwrap();
  let again = fixture.ide.diagnostics(&atlas_ide::DiagnosticsRequest::new("clean:src/use.ts")).unwrap();

  assert_eq!(response.answer.status, EditStatus::Applied, "{}", text(&response));
  assert_no_new_errors(&response);
  assert!(
    text(&response).contains("diagnostics after edit: no new errors in the 2 edited file(s) (settled)"),
    "{}",
    text(&response)
  );
  for answer in [diagnostics, again] {
    assert!(text(&answer).starts_with("diagnostics clean:src/use.ts  [settled]"), "{}", text(&answer));
    assert!(text(&answer).contains("(no diagnostics)"), "{}", text(&answer));
  }
}

const SOLUTION: &str = "{\"files\": [], \"references\": [{\"path\": \"./tsconfig.build.json\"}, {\"path\": \"./tsconfig.spec.json\"}]}\n";
const BUILD: &str =
  "{\"compilerOptions\": {\"composite\": true, \"strict\": true, \"outDir\": \"dist/build\"}, \"include\": [\"src/**/*.ts\"], \"exclude\": [\"src/**/*.test.ts\"]}\n";
const SPEC: &str = "{\"compilerOptions\": {\"composite\": true, \"strict\": true, \"outDir\": \"dist/spec\"}, \"include\": [\"src/**/*.ts\"]}\n";
const HANDLER: &str = "export function handleMiddleware(count: number): number {\n  return count + 1;\n}\n";
const INDEX: &str = "export { handleMiddleware } from './handler';\n";
const HANDLER_TEST: &str = "import { handleMiddleware } from './handler';\n\nexport const first = handleMiddleware(1);\nexport const second = handleMiddleware(2);\n";

fn solution_files() -> Vec<(&'static str, &'static str)> {
  vec![
    ("package.json", "{\"name\": \"solution\"}\n"),
    ("tsconfig.json", SOLUTION),
    ("tsconfig.build.json", BUILD),
    ("tsconfig.spec.json", SPEC),
    ("src/handler.ts", HANDLER),
    ("src/index.ts", INDEX),
    ("src/handler.test.ts", HANDLER_TEST),
  ]
}

fn planned_files(response: &Response<EditOutcome>) -> Vec<(String, usize)> {
  response.answer.files.iter().map(|file| (file.label(), file.edits)).collect()
}

#[test]
fn should_plan_the_same_rename_in_a_solution_style_project_whatever_was_read_before() {
  let test = "should_plan_the_same_rename_in_a_solution_style_project_whatever_was_read_before";
  let Some(fresh) = typescript_tree(test, &[("solution", &solution_files())]) else {
    return;
  };
  let Some(warmed) = typescript_tree(test, &[("solution", &solution_files())]) else {
    return;
  };
  warmed.ide.outline(&atlas_ide::OutlineRequest::new("solution:src/handler.test.ts")).unwrap();
  let request = EditRequest::dry_run(rename("handleMiddleware", "handlePagesMiddleware", CrossRepo::Report));

  let first = fresh.ide.edit(&request).unwrap();
  let second = warmed.ide.edit(&request).unwrap();

  let expected = vec![
    ("solution:src/handler.test.ts".to_owned(), 3),
    ("solution:src/handler.ts".to_owned(), 1),
    ("solution:src/index.ts".to_owned(), 1),
  ];
  assert_eq!(planned_files(&first), expected, "{}", text(&first));
  assert_eq!(planned_files(&second), expected, "{}", text(&second));
  assert!(first.answer.unverified.is_empty() && first.answer.not_renamed.is_empty(), "{}", text(&first));
}

const OPTIONS: &str = "export interface DetectorOptions {\n  order: string[]\n}\n\nexport const DEFAULTS: DetectorOptions = { order: ['header'] }\n";
const OPTIONS_TEST: &str = "import type { DetectorOptions } from './language'\n\nexport const custom: DetectorOptions = { order: ['cookie'] }\n";

fn options_files() -> Vec<(&'static str, &'static str)> {
  vec![
    ("package.json", "{\"name\": \"solution\"}\n"),
    ("tsconfig.json", SOLUTION),
    ("tsconfig.build.json", BUILD),
    ("tsconfig.spec.json", SPEC),
    ("src/language.ts", OPTIONS),
    ("src/language.test.ts", OPTIONS_TEST),
  ]
}

#[test]
fn should_rename_an_object_literal_property_in_spec_files_without_earlier_reads() {
  let test = "should_rename_an_object_literal_property_in_spec_files_without_earlier_reads";
  let Some(fresh) = typescript_tree(test, &[("solution", &options_files())]) else {
    return;
  };
  let Some(warmed) = typescript_tree(test, &[("solution", &options_files())]) else {
    return;
  };
  warmed.ide.outline(&atlas_ide::OutlineRequest::new("solution:src/language.test.ts")).unwrap();
  let request = EditRequest::dry_run(rename("DetectorOptions.order", "priority", CrossRepo::Report));

  let first = fresh.ide.edit(&request).unwrap();
  let second = warmed.ide.edit(&request).unwrap();

  let expected = vec![("solution:src/language.test.ts".to_owned(), 1), ("solution:src/language.ts".to_owned(), 2)];
  assert_eq!(planned_files(&first), expected, "{}", text(&first));
  assert_eq!(planned_files(&second), expected, "{}", text(&second));
}

const OPTS: &str = "export interface Opts {\n  order: string[]\n}\n\nexport function make(opts: Opts): string[] {\n  return opts.order\n}\n";
const T1: &str = "import { make } from '../app/src/opts'\n\nexport const made = make({ order: ['a'] })\n";

fn sibling_project_files() -> Vec<(&'static str, &'static str)> {
  vec![
    ("package.json", "{\"name\": \"sibling\"}\n"),
    ("app/tsconfig.json", "{\"compilerOptions\": {\"strict\": true, \"noEmit\": true}, \"include\": [\"src\"]}\n"),
    ("app/src/opts.ts", OPTS),
    (
      "tests/tsconfig.json",
      "{\n  \"compilerOptions\": {\"strict\": true, \"noEmit\": true},\n  \"include\": [\"**/*.ts\", \"../app/src/**/*.ts\"], // also the app\n}\n",
    ),
    ("tests/t1.ts", T1),
  ]
}

#[test]
fn should_load_a_sibling_project_that_includes_the_anchor_through_a_parent_glob() {
  let test = "should_load_a_sibling_project_that_includes_the_anchor_through_a_parent_glob";
  let Some(fresh) = typescript_tree(test, &[("sibling", &sibling_project_files())]) else {
    return;
  };
  let Some(warmed) = typescript_tree(test, &[("sibling", &sibling_project_files())]) else {
    return;
  };
  warmed.ide.outline(&atlas_ide::OutlineRequest::new("sibling:tests/t1.ts")).unwrap();
  let request = EditRequest::dry_run(rename("Opts.order", "orderBy", CrossRepo::Report));

  let first = fresh.ide.edit(&request).unwrap();
  let second = warmed.ide.edit(&request).unwrap();

  let expected = vec![("sibling:app/src/opts.ts".to_owned(), 2), ("sibling:tests/t1.ts".to_owned(), 1)];
  assert_eq!(planned_files(&first), expected, "{}", text(&first));
  assert_eq!(planned_files(&second), expected, "{}", text(&second));
}
