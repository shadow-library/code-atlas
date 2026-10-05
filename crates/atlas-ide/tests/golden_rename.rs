mod common;

use std::collections::BTreeMap;
use std::fs;

use atlas_ide::{CrossRepo, EditOperation, EditOutcome, EditRequest, EditStatus, IdePlan, IdeSettings, LaunchSpec, VerifyReason};
use atlas_lsp::{Launch, ServerCommand, ServerKind};
use atlas_query::{Response, Targeted};
use common::*;

const ACME: [&str; 3] = ["acme-api", "acme-sdk", "acme-web"];

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

fn line_column(text: &str, offset: usize) -> (usize, usize) {
  let before = &text[..offset];
  let line_start = before.rfind('\n').map_or(0, |at| at + 1);
  (before.matches('\n').count() + 1, offset - line_start + 1)
}

fn offset_of(text: &str, line: usize, column: usize) -> usize {
  let line_start: usize = text.split_inclusive('\n').take(line - 1).map(str::len).sum();
  line_start + column - 1
}

struct ExpectedEdit {
  repo: String,
  path: String,
  line: usize,
  column: usize,
  old: String,
  new: String,
}

fn parse(site: &str) -> ExpectedEdit {
  let (place, change) = site.split_once(' ').unwrap();
  let (old, new) = change.split_once(" -> ").unwrap();
  let mut parts = place.rsplitn(3, ':');
  let column = parts.next().unwrap().parse().unwrap();
  let line = parts.next().unwrap().parse().unwrap();
  let (repo, path) = parts.next().unwrap().split_once(':').unwrap();
  ExpectedEdit {
    repo: repo.to_owned(),
    path: path.to_owned(),
    line,
    column,
    old: old.to_owned(),
    new: new.to_owned(),
  }
}

fn dry_plan(fixture: &Fixture, response: &Response<EditOutcome>) -> IdePlan {
  match &response.answer.status {
    EditStatus::DryRun { plan_id, .. } => fixture.ide.planned(plan_id).unwrap(),
    other => panic!("expected a dry run, got {other:?}\n{}", text(response)),
  }
}

fn planned_sites(fixture: &Fixture, plan: &IdePlan) -> Vec<String> {
  let mut sites = Vec::new();
  for file in &plan.edits.files {
    let (repo, path) = (file.file.repo.as_str(), file.file.path.as_str());
    let content = read(fixture, repo, path);
    for edit in &file.edits {
      let (line, column) = line_column(&content, edit.start);
      sites.push((
        repo.to_owned(),
        path.to_owned(),
        line,
        column,
        content[edit.start..edit.end].to_owned(),
        edit.new_text.clone(),
      ));
    }
  }
  sites.sort();
  sites
    .into_iter()
    .map(|(repo, path, line, column, old, new)| format!("{repo}:{path}:{line}:{column} {old} -> {new}"))
    .collect()
}

fn places(sites: &[atlas_ide::VerifySite]) -> Vec<(VerifyReason, String)> {
  sites.iter().map(|site| (site.reason, site.place())).collect()
}

struct Snapshot(BTreeMap<(String, String), String>);

impl Snapshot {
  fn take(fixture: &Fixture, files: &[(&str, &str)]) -> Self {
    Self(
      files
        .iter()
        .map(|(repo, path)| (((*repo).to_owned(), (*path).to_owned()), read(fixture, repo, path)))
        .collect(),
    )
  }

  fn assert_renamed_exactly(&self, fixture: &Fixture, expected: &[&str]) {
    let edits: Vec<ExpectedEdit> = expected.iter().map(|site| parse(site)).collect();
    for ((repo, path), before) in &self.0 {
      let mut sites: Vec<&ExpectedEdit> = edits.iter().filter(|edit| edit.repo == *repo && edit.path == *path).collect();
      sites.sort_by_key(|edit| std::cmp::Reverse((edit.line, edit.column)));
      let mut after = before.clone();
      for edit in sites {
        let start = offset_of(&after, edit.line, edit.column);
        assert_eq!(
          &after[start..start + edit.old.len()],
          edit.old,
          "{repo}:{path}:{}:{} does not read {}",
          edit.line,
          edit.column,
          edit.old
        );
        after.replace_range(start..start + edit.old.len(), &edit.new);
      }
      assert_eq!(read(fixture, repo, path), after, "{repo}:{path}");
    }
  }
}

fn apply(fixture: &Fixture, plan: &IdePlan) -> Response<EditOutcome> {
  let applied = fixture.ide.edit(&EditRequest::new(EditOperation::Apply { plan_id: plan.id.clone() })).unwrap();
  assert_eq!(applied.answer.status, EditStatus::Applied, "{}", text(&applied));
  applied
}

fn assert_fresh(fixture: &Fixture, present: &str, gone: &str) {
  let query = fixture.ide.query();
  query.fresh().unwrap();
  assert!(matches!(query.resolve_target(present).unwrap(), Targeted::Found(_)), "{present} is not in the index");
  assert!(matches!(query.resolve_target(gone).unwrap(), Targeted::NotFound { .. }), "{gone} is still in the index");
}

const FORMAT_MONEY_SITES: [&str; 6] = [
  "acme-api:src/common/money.ts:1:17 formatMoney -> formatCents",
  "acme-api:src/orders/orders.service.ts:7:10 formatMoney -> formatCents",
  "acme-api:src/orders/orders.service.ts:26:33 formatMoney -> formatCents",
  "acme-api:src/reports/reports.service.ts:6:10 formatMoney -> formatCents",
  "acme-api:src/reports/reports.service.ts:15:71 formatMoney -> formatCents",
  "acme-api:src/reports/reports.service.ts:17:51 formatMoney -> formatCents",
];

#[test]
fn should_rename_format_money_across_acme_api() {
  let test = "should_rename_format_money_across_acme_api";
  let Some(fixture) = typescript_fixture(test, "acme", &ACME) else { return };
  let before = Snapshot::take(
    &fixture,
    &[
      ("acme-api", "src/common/money.ts"),
      ("acme-api", "src/orders/orders.service.ts"),
      ("acme-api", "src/reports/reports.service.ts"),
    ],
  );

  let dry = fixture.ide.edit(&EditRequest::dry_run(rename("formatMoney", "formatCents", CrossRepo::Report))).unwrap();
  let plan = dry_plan(&fixture, &dry);

  assert_eq!(planned_sites(&fixture, &plan), FORMAT_MONEY_SITES);
  assert_eq!(places(&dry.answer.not_renamed), []);
  assert_eq!(places(&dry.answer.unverified), []);
  apply(&fixture, &plan);
  before.assert_renamed_exactly(&fixture, &FORMAT_MONEY_SITES);
  assert_fresh(&fixture, "formatCents", "formatMoney");
}

#[test]
fn should_rename_users_service_find_one_including_its_spec_calls() {
  let test = "should_rename_users_service_find_one_including_its_spec_calls";
  let Some(fixture) = typescript_fixture(test, "acme", &ACME) else { return };
  let before = Snapshot::take(
    &fixture,
    &[
      ("acme-api", "src/users/users.service.ts"),
      ("acme-api", "src/users/users.controller.ts"),
      ("acme-api", "src/users/users.service.spec.ts"),
    ],
  );
  let expected = [
    "acme-api:src/users/users.controller.ts:16:30 findOne -> getById",
    "acme-api:src/users/users.service.spec.ts:15:28 findOne -> getById",
    "acme-api:src/users/users.service.spec.ts:20:28 findOne -> getById",
    "acme-api:src/users/users.service.ts:18:9 findOne -> getById",
    "acme-api:src/users/users.service.ts:29:29 findOne -> getById",
  ];

  let dry = fixture
    .ide
    .edit(&EditRequest::dry_run(rename("UsersService.findOne", "getById", CrossRepo::Report)))
    .unwrap();
  let plan = dry_plan(&fixture, &dry);

  assert_eq!(planned_sites(&fixture, &plan), expected);
  assert_eq!(places(&dry.answer.not_renamed), []);
  assert_eq!(places(&dry.answer.unverified), []);
  apply(&fixture, &plan);
  before.assert_renamed_exactly(&fixture, &expected);
  assert_fresh(&fixture, "UsersService.getById", "UsersService.findOne");
}

#[test]
fn should_report_orders_service_find_one_jest_mocks_as_not_renamed() {
  let test = "should_report_orders_service_find_one_jest_mocks_as_not_renamed";
  let Some(fixture) = typescript_fixture(test, "acme", &ACME) else { return };
  let before = Snapshot::take(
    &fixture,
    &[
      ("acme-api", "src/orders/orders.service.ts"),
      ("acme-api", "src/orders/orders.controller.ts"),
      ("acme-api", "src/orders/orders.controller.spec.ts"),
    ],
  );
  let expected = [
    "acme-api:src/orders/orders.controller.ts:16:31 findOne -> fetchOne",
    "acme-api:src/orders/orders.service.ts:23:9 findOne -> fetchOne",
  ];

  let dry = fixture
    .ide
    .edit(&EditRequest::dry_run(rename("OrdersService.findOne", "fetchOne", CrossRepo::Report)))
    .unwrap();
  let plan = dry_plan(&fixture, &dry);

  assert_eq!(planned_sites(&fixture, &plan), expected);
  assert_eq!(
    places(&dry.answer.not_renamed),
    [
      (VerifyReason::Mock, "acme-api:src/orders/orders.controller.spec.ts:5:27".to_owned()),
      (VerifyReason::Mock, "acme-api:src/orders/orders.controller.spec.ts:11:19".to_owned()),
      (VerifyReason::Mock, "acme-api:src/orders/orders.controller.spec.ts:13:26".to_owned()),
    ]
  );
  let rendered = text(&dry);
  for (line, column) in [(5, 27), (11, 19), (13, 26)] {
    let listed = format!("  mock [heuristic 0.60]  acme-api:src/orders/orders.controller.spec.ts:{line}:{column}");
    assert!(rendered.contains(&listed), "{rendered}");
  }
  assert!(
    !rendered.contains("orders.controller.spec.ts:12:"),
    "controller.findOne('o1') calls the controller's own method:\n{rendered}"
  );
  assert_eq!(places(&dry.answer.unverified), []);
  apply(&fixture, &plan);
  before.assert_renamed_exactly(&fixture, &expected);
  assert_fresh(&fixture, "OrdersService.fetchOne", "OrdersService.findOne");
}

#[test]
fn should_rename_a_ts_interface_method_and_its_implementations() {
  let test = "should_rename_a_ts_interface_method_and_its_implementations";
  let Some(fixture) = typescript_fixture(test, "adversarial", &["ts-overrides"]) else {
    return;
  };
  let files = [
    "src/payment-provider.ts",
    "src/base-provider.ts",
    "src/paypal-provider.ts",
    "src/stripe-provider.ts",
    "src/checkout.ts",
    "src/checkout.spec.ts",
  ];
  let before = Snapshot::take(&fixture, &files.map(|path| ("ts-overrides", path)));
  let expected = [
    "ts-overrides:src/base-provider.ts:7:12 charge -> capture",
    "ts-overrides:src/checkout.spec.ts:9:21 charge -> capture",
    "ts-overrides:src/checkout.ts:4:33 charge -> capture",
    "ts-overrides:src/payment-provider.ts:8:3 charge -> capture",
    "ts-overrides:src/paypal-provider.ts:7:9 charge -> capture",
    "ts-overrides:src/stripe-provider.ts:7:9 charge -> capture",
  ];

  let dry = fixture
    .ide
    .edit(&EditRequest::dry_run(rename("PaymentProvider.charge", "capture", CrossRepo::Report)))
    .unwrap();
  let plan = dry_plan(&fixture, &dry);

  assert_eq!(planned_sites(&fixture, &plan), expected);
  assert_eq!(places(&dry.answer.not_renamed), [(VerifyReason::Mock, "ts-overrides:src/checkout.spec.ts:6:24".to_owned())]);
  assert_eq!(places(&dry.answer.unverified), []);
  apply(&fixture, &plan);
  before.assert_renamed_exactly(&fixture, &expected);
  assert_fresh(&fixture, "PaymentProvider.capture", "PaymentProvider.charge");
}

const TO_CENTS_SDK_SITES: [&str; 6] = [
  "acme-sdk:src/money.test.ts:3:10 toCents -> toMinorUnits",
  "acme-sdk:src/money.test.ts:7:12 toCents -> toMinorUnits",
  "acme-sdk:src/money.test.ts:8:12 toCents -> toMinorUnits",
  "acme-sdk:src/money.test.ts:9:12 toCents -> toMinorUnits",
  "acme-sdk:src/money.test.ts:13:12 toCents -> toMinorUnits",
  "acme-sdk:src/money.ts:1:17 toCents -> toMinorUnits",
];

const TO_CENTS_FILES: [(&str, &str); 4] = [
  ("acme-sdk", "src/money.ts"),
  ("acme-sdk", "src/money.test.ts"),
  ("acme-sdk", "src/index.ts"),
  ("acme-web", "src/components/OrderForm.tsx"),
];

#[test]
fn should_report_to_cents_consumers_in_acme_web_as_not_renamed() {
  let test = "should_report_to_cents_consumers_in_acme_web_as_not_renamed";
  let Some(fixture) = typescript_fixture(test, "acme", &ACME) else { return };
  let before = Snapshot::take(&fixture, &TO_CENTS_FILES);
  let mut expected = vec!["acme-sdk:src/index.ts:3:10 toCents -> toMinorUnits as toCents"];
  expected.extend(TO_CENTS_SDK_SITES);

  let dry = fixture.ide.edit(&EditRequest::dry_run(rename("toCents", "toMinorUnits", CrossRepo::Report))).unwrap();
  let plan = dry_plan(&fixture, &dry);

  assert_eq!(planned_sites(&fixture, &plan), expected);
  assert_eq!(
    places(&dry.answer.not_renamed),
    [
      (VerifyReason::OutsideLanguageServer, "acme-web:src/components/OrderForm.tsx:1:10".to_owned()),
      (VerifyReason::OutsideLanguageServer, "acme-web:src/components/OrderForm.tsx:15:44".to_owned()),
    ]
  );
  assert_eq!(places(&dry.answer.unverified), []);
  apply(&fixture, &plan);
  before.assert_renamed_exactly(&fixture, &expected);
}

#[test]
fn should_apply_authoritative_to_cents_sites_in_acme_web() {
  let test = "should_apply_authoritative_to_cents_sites_in_acme_web";
  let Some(fixture) = typescript_fixture(test, "acme", &ACME) else { return };
  let manifest = read(&fixture, "acme-sdk", "package.json")
    .replace("dist/index.js", "src/index.ts")
    .replace("dist/index.d.ts", "src/index.ts");
  fs::write(fixture.file("acme-sdk", "package.json"), manifest).unwrap();
  let index = read(&fixture, "acme-sdk", "src/index.ts").replace("export { toCents } from './money';", "export * from './money';");
  fs::write(fixture.file("acme-sdk", "src/index.ts"), index).unwrap();
  let before = Snapshot::take(&fixture, &TO_CENTS_FILES);
  let mut expected = TO_CENTS_SDK_SITES.to_vec();
  expected.extend([
    "acme-web:src/components/OrderForm.tsx:1:10 toCents -> toMinorUnits",
    "acme-web:src/components/OrderForm.tsx:15:44 toCents -> toMinorUnits",
  ]);

  let dry = fixture.ide.edit(&EditRequest::dry_run(rename("toCents", "toMinorUnits", CrossRepo::Apply))).unwrap();
  let plan = dry_plan(&fixture, &dry);

  assert_eq!(planned_sites(&fixture, &plan), expected);
  assert_eq!(
    places(&dry.answer.applied_from_graph),
    [
      (VerifyReason::OutsideLanguageServer, "acme-web:src/components/OrderForm.tsx:1:10".to_owned()),
      (VerifyReason::OutsideLanguageServer, "acme-web:src/components/OrderForm.tsx:15:44".to_owned()),
    ]
  );
  assert_eq!(places(&dry.answer.not_renamed), []);
  assert_eq!(places(&dry.answer.unverified), []);
  apply(&fixture, &plan);
  before.assert_renamed_exactly(&fixture, &expected);
  assert_fresh(&fixture, "toMinorUnits", "toCents");
}

#[test]
fn should_refuse_safe_delete_of_format_money_and_list_references() {
  let test = "should_refuse_safe_delete_of_format_money_and_list_references";
  let Some(fixture) = typescript_fixture(test, "acme", &ACME) else { return };
  let before = Snapshot::take(
    &fixture,
    &[
      ("acme-api", "src/common/money.ts"),
      ("acme-api", "src/orders/orders.service.ts"),
      ("acme-api", "src/reports/reports.service.ts"),
    ],
  );

  let response = fixture.ide.edit(&EditRequest::new(EditOperation::SafeDelete { target: "formatMoney".to_owned() })).unwrap();

  assert!(matches!(response.answer.status, EditStatus::Refused { .. }), "{}", text(&response));
  assert_eq!(
    places(&response.answer.blockers),
    [
      (VerifyReason::LanguageServer, "acme-api:src/orders/orders.service.ts:7:10".to_owned()),
      (VerifyReason::LanguageServer, "acme-api:src/orders/orders.service.ts:26:33".to_owned()),
      (VerifyReason::LanguageServer, "acme-api:src/reports/reports.service.ts:6:10".to_owned()),
      (VerifyReason::LanguageServer, "acme-api:src/reports/reports.service.ts:15:71".to_owned()),
      (VerifyReason::LanguageServer, "acme-api:src/reports/reports.service.ts:17:51".to_owned()),
    ]
  );
  assert!(response.answer.files.is_empty(), "{}", text(&response));
  before.assert_renamed_exactly(&fixture, &[]);
  assert_fresh(&fixture, "formatMoney", "formatCents");
}

fn rust_fixture(test: &str, fixture: Fixture) -> Option<Fixture> {
  let binary = match rust_analyzer() {
    Ok(binary) => binary,
    Err(reason) => {
      eprintln!("SKIPPED {test}: rust-analyzer unavailable ({reason})");
      return None;
    }
  };
  fixture.ide.reload_settings(IdeSettings {
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

#[test]
fn should_rename_store_get_and_every_impl_and_call_with_rust_analyzer() {
  let test = "should_rename_store_get_and_every_impl_and_call_with_rust_analyzer";
  let Some(fixture) = rust_fixture(test, Fixture::copied("adversarial", &["trait-dispatch-rust"])) else {
    return;
  };
  let before = Snapshot::take(&fixture, &[("trait-dispatch-rust", "src/lib.rs")]);
  let expected = [
    "trait-dispatch-rust:src/lib.rs:4:6 get -> fetch",
    "trait-dispatch-rust:src/lib.rs:28:6 get -> fetch",
    "trait-dispatch-rust:src/lib.rs:38:6 get -> fetch",
    "trait-dispatch-rust:src/lib.rs:48:9 get -> fetch",
    "trait-dispatch-rust:src/lib.rs:52:9 get -> fetch",
    "trait-dispatch-rust:src/lib.rs:56:9 get -> fetch",
    "trait-dispatch-rust:src/lib.rs:60:9 get -> fetch",
  ];

  let dry = fixture.ide.edit(&EditRequest::dry_run(rename("Store::get", "fetch", CrossRepo::Report))).unwrap();
  let plan = dry_plan(&fixture, &dry);

  assert_eq!(planned_sites(&fixture, &plan), expected);
  assert_eq!(places(&dry.answer.unverified), []);
  apply(&fixture, &plan);
  before.assert_renamed_exactly(&fixture, &expected);
  assert_fresh(&fixture, "Store::fetch", "Store::get");
}
