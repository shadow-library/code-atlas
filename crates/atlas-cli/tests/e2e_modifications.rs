mod e2e_support;

use std::fs;
use std::path::{Path, PathBuf};

use e2e_support::expected::Atlas;
use e2e_support::{Run, Sandbox, assert_same_answers, edit, entity_refs, git, shorts};
use serde_json::Value;

struct Acme {
  sandbox: Sandbox,
  root: PathBuf,
}

impl Acme {
  fn indexed() -> Self {
    let sandbox = Sandbox::new();
    let root = sandbox.acme();
    sandbox.run(&root.join("acme-api"), &["index"]).ok();
    Self { sandbox, root }
  }

  fn api_repo(&self) -> PathBuf {
    self.root.join("acme-api")
  }

  fn file(&self, path: &str) -> PathBuf {
    self.api_repo().join(path)
  }

  fn run(&self, args: &[&str]) -> Run {
    self.sandbox.run(&self.api_repo(), args)
  }

  fn answer(&self, args: &[&str]) -> Value {
    let mut args = args.to_vec();
    args.push("--json");
    self.run(&args).ok().answer()
  }

  fn first_query_after_edit(&self, args: &[&str]) -> Value {
    let mut args = args.to_vec();
    args.push("--json");
    let document = self.run(&args).ok().json();
    assert_eq!(document["freshness"]["state"], "fresh", "{document:#}");
    assert!(
      document["freshness"]["files_updated"].as_u64().unwrap() > 0,
      "the barrier did not pick up the edit: {document:#}"
    );
    document["answer"].clone()
  }

  fn assert_matches_full_index(&self, queries: &[&[&str]], context: &str) {
    assert_same_answers(&self.sandbox, "acme", queries, context);
  }

  fn unresolved(&self, path: &str) -> Vec<(String, String, String)> {
    let atlas = Atlas {
      sandbox: &self.sandbox,
      cwd: self.api_repo(),
      project: "acme".to_owned(),
    };
    let dump = atlas.dump("acme-api", path).unwrap();
    let names: Vec<(String, String)> = dump["entities"]
      .as_array()
      .unwrap()
      .iter()
      .map(|entity| (entity["id"].as_str().unwrap().to_owned(), entity["name"].as_str().unwrap().to_owned()))
      .collect();
    dump["unresolved"]
      .as_array()
      .unwrap()
      .iter()
      .map(|reference| {
        let from = reference["from"].as_str().unwrap();
        let owner = names.iter().find(|(id, _)| id == from).map(|(_, name)| name.clone()).unwrap_or_default();
        let path: Vec<&str> = reference["path"].as_array().unwrap().iter().map(|segment| segment.as_str().unwrap()).collect();
        (owner, path.join("."), reference["reason"]["reason"].as_str().unwrap().to_owned())
      })
      .collect()
  }
}

fn sorted(mut values: Vec<String>) -> Vec<String> {
  values.sort();
  values
}

fn endpoint<'a>(api: &'a Value, method: &str, route: &str) -> Option<&'a Value> {
  api["endpoints"]["items"]
    .as_array()
    .unwrap()
    .iter()
    .find(|item| item["method"] == method && item["route"] == route)
}

fn schemes(item: &Value) -> Vec<String> {
  sorted(
    item["auth"]
      .as_array()
      .unwrap()
      .iter()
      .map(|entry| entry["scheme"]["entity"]["short"].as_str().unwrap().to_owned())
      .collect(),
  )
}

fn supported_endpoints(api: &Value, scheme: &str) -> u64 {
  let matrix = api["auth_matrix"].as_array().unwrap().iter().find(|matrix| matrix["repo"] == "acme-api").unwrap();
  let entry = matrix["schemes"].as_array().unwrap().iter().find(|entry| entry["scheme"]["short"] == scheme).unwrap();
  entry["supported"]["endpoints"].as_u64().unwrap()
}

#[test]
fn should_see_an_edited_body_without_reindexing() {
  let acme = Acme::indexed();
  assert_eq!(
    sorted(shorts(&acme.answer(&["callers", "formatMoney"])["groups"])),
    ["OrdersService.findOne", "ReportsService.exportCsv"]
  );

  edit(
    &acme.file("src/orders/orders.service.ts"),
    "return { ...order, display: formatMoney(order.totalCents) };",
    "return order;",
  );

  let callers = acme.first_query_after_edit(&["callers", "formatMoney"]);
  assert_eq!(shorts(&callers["groups"]), ["ReportsService.exportCsv"]);
  let body = acme.run(&["query", "symbol", "OrdersService.findOne", "--body"]).ok();
  assert!(body.stdout.contains("return order;") && !body.stdout.contains("display:"), "{body}");
  acme.assert_matches_full_index(
    &[&["callers", "formatMoney"], &["impact", "formatMoney"], &["query", "symbol", "OrdersService.findOne"]],
    "after a body edit",
  );
}

#[test]
fn should_follow_a_rename_of_a_function_and_its_callers() {
  let acme = Acme::indexed();

  for path in ["src/common/money.ts", "src/orders/orders.service.ts", "src/reports/reports.service.ts"] {
    edit(&acme.file(path), "formatMoney", "formatCents");
  }

  let callers = acme.first_query_after_edit(&["callers", "formatCents"]);
  assert_eq!(sorted(shorts(&callers["groups"])), ["OrdersService.findOne", "ReportsService.exportCsv"]);
  assert_eq!(callers["groups"][0]["items"][0]["relation"]["certainty"]["class"], "resolved");
  let old = acme.run(&["query", "symbol", "formatMoney"]).expect(4);
  assert!(old.stdout.contains("not found"), "{old}");
  let changes = acme.run(&["query", "changes"]).ok();
  assert!(
    changes
      .stdout
      .contains("renamed ts:acme-api:acme-api:src/common/money::formatMoney#function -> formatCents function acme-api:src/common/money.ts:1"),
    "{changes}"
  );
  let explained = acme.run(&["explain", "formatCents"]).ok();
  assert!(
    explained
      .stdout
      .contains("renamed from ts:acme-api:acme-api:src/common/money::formatMoney#function [heuristic"),
    "{explained}"
  );
  acme.assert_matches_full_index(&[&["callers", "formatCents"], &["impact", "formatCents"], &["search", "formatMoney"]], "after a rename");
}

#[test]
fn should_follow_a_moved_file() {
  let acme = Acme::indexed();

  fs::create_dir_all(acme.file("src/shared")).unwrap();
  fs::rename(acme.file("src/common/money.ts"), acme.file("src/shared/money.ts")).unwrap();
  for path in ["src/orders/orders.service.ts", "src/reports/reports.service.ts"] {
    edit(&acme.file(path), "'../common/money'", "'../shared/money'");
  }

  let symbol = acme.first_query_after_edit(&["query", "symbol", "formatMoney"]);
  assert_eq!(symbol["entity"]["location"]["path"], "src/shared/money.ts", "{symbol:#}");
  let callers = acme.answer(&["callers", "formatMoney"]);
  assert_eq!(sorted(shorts(&callers["groups"])), ["OrdersService.findOne", "ReportsService.exportCsv"]);
  acme.run(&["query", "symbol", "acme-api:src/common/money.ts"]).expect(4);
  acme.assert_matches_full_index(&[&["callers", "formatMoney"], &["impact", "formatMoney"], &["query", "overview"]], "after a move");
}

#[test]
fn should_forget_a_deleted_file_and_leave_its_callers_unresolved() {
  let acme = Acme::indexed();
  assert_eq!(shorts(&acme.answer(&["query", "callees", "BillingService.applyTax"])["groups"]), ["roundHalfEven"]);

  fs::remove_file(acme.file("src/billing/rounding.ts")).unwrap();

  let gone = acme.run(&["query", "symbol", "roundHalfEven"]).expect(4);
  assert!(gone.stdout.starts_with("fresh run=2 (+1 files"), "{gone}");
  acme.run(&["query", "symbol", "roundHalfUp"]).expect(4);
  assert_eq!(acme.answer(&["query", "callees", "BillingService.applyTax"])["total"], 0);
  let billing = acme.unresolved("src/billing/billing.service.ts");
  assert!(
    billing.contains(&("applyTax".to_owned(), "roundHalfEven".to_owned(), "not_found".to_owned())),
    "{billing:?}"
  );
  let reports = acme.unresolved("src/reports/reports.service.ts");
  assert!(reports.contains(&("exportCsv".to_owned(), "roundHalfUp".to_owned(), "not_found".to_owned())), "{reports:?}");
  acme.assert_matches_full_index(
    &[
      &["query", "callees", "BillingService.applyTax"],
      &["impact", "BillingService.applyTax"],
      &["query", "overview"],
    ],
    "after a delete",
  );
}

#[test]
fn should_list_a_new_endpoint_in_api() {
  let acme = Acme::indexed();

  edit(
    &acme.file("src/users/users.controller.ts"),
    "  @Delete(':id')",
    "  @Get(':id/orders')\n  @RequirePermission('users:read')\n  listOrders(@Param('id') id: string) {\n    return this.usersService.findOne(id);\n  }\n\n  @Delete(':id')",
  );

  let api = acme.first_query_after_edit(&["api"]);
  let added = endpoint(&api, "GET", "/users/{id}/orders").unwrap_or_else(|| panic!("new endpoint missing: {api:#}"));
  assert_eq!(added["certainty"]["class"], "syntactic");
  assert_eq!(added["handler"]["entity"]["short"], "UsersController.listOrders");
  assert_eq!(schemes(added), ["bearer"]);
  assert_eq!(shorts(&added["permissions"]), ["users:read"]);
  assert_eq!(api["endpoints"]["total"], 11);
  acme.assert_matches_full_index(&[&["api"], &["impact", "UsersService.findOne"]], "after adding an endpoint");
}

#[test]
fn should_update_the_auth_matrix_when_a_guard_changes() {
  let acme = Acme::indexed();
  let before = acme.answer(&["api"]);
  assert_eq!(schemes(endpoint(&before, "GET", "/v1/reports/export").unwrap()), ["api_key"]);
  assert_eq!((supported_endpoints(&before, "api_key"), supported_endpoints(&before, "bearer")), (2, 6));

  let controller = acme.file("src/reports/reports.controller.ts");
  edit(
    &controller,
    "import { ApiKeyGuard } from '../auth/api-key.guard';",
    "import { JwtAuthGuard } from '../auth/jwt-auth.guard';",
  );
  edit(&controller, "@UseGuards(ApiKeyGuard)", "@UseGuards(JwtAuthGuard)");

  let after = acme.first_query_after_edit(&["api"]);
  let export = endpoint(&after, "GET", "/v1/reports/export").unwrap();
  assert_eq!(schemes(export), ["bearer"]);
  assert_eq!(shorts(&export["guards"]).first().map(String::as_str), Some("JwtAuthGuard"));
  assert_eq!((supported_endpoints(&after, "api_key"), supported_endpoints(&after, "bearer")), (1, 7));
  acme.assert_matches_full_index(&[&["api"]], "after a guard change");
}

#[test]
fn should_track_a_renamed_env_var() {
  let acme = Acme::indexed();
  acme.run(&["query", "symbol", "DATABASE_URL", "--kind", "env_var", "--repo", "acme-api"]).ok();

  edit(&acme.file("src/common/database.config.ts"), "process.env.DATABASE_URL", "process.env.DB_CONNECTION_URL");

  let found = acme.first_query_after_edit(&["query", "symbol", "DB_CONNECTION_URL", "--kind", "env_var"]);
  assert_eq!(found["entity"]["location"]["repo"], "acme-api", "{found:#}");
  acme.run(&["query", "symbol", "DATABASE_URL", "--kind", "env_var", "--repo", "acme-api"]).expect(4);
  let billing = acme.answer(&["query", "symbol", "DATABASE_URL", "--kind", "env_var", "--repo", "acme-billing"]);
  assert_eq!(billing["outcome"], "found", "the same-named env var of another repo must survive");
  let dependents = acme.answer(&["query", "dependents", "DB_CONNECTION_URL", "--kind", "env_var"]);
  assert_eq!(shorts(&dependents["groups"]), ["databaseConfig"]);
  acme.assert_matches_full_index(
    &[&["query", "dependents", "DB_CONNECTION_URL", "--kind", "env_var"], &["query", "overview"]],
    "after renaming an env var",
  );
}

const BRANCH_QUERIES: [&[&str]; 6] = [
  &["api"],
  &["query", "overview"],
  &["impact", "OrdersService.findOne"],
  &["search", "formatMoney"],
  &["query", "unused"],
  &["query", "callees", "BillingService.applyTax"],
];

fn commit_feature_branch(repo: &Path) {
  git(repo, &["checkout", "-q", "-b", "feature"]);
  for path in ["src/common/money.ts", "src/orders/orders.service.ts", "src/reports/reports.service.ts"] {
    edit(&repo.join(path), "formatMoney", "formatCents");
  }
  fs::remove_file(repo.join("src/billing/rounding.ts")).unwrap();
  edit(&repo.join("src/billing/billing.service.ts"), "import { roundHalfEven } from './rounding';\n", "");
  edit(
    &repo.join("src/billing/billing.service.ts"),
    "roundHalfEven(subtotalCents * TAX_RATE)",
    "Math.round(subtotalCents * TAX_RATE)",
  );
  edit(&repo.join("src/reports/reports.service.ts"), "import { roundHalfUp } from '../billing/rounding';\n", "");
  edit(&repo.join("src/reports/reports.service.ts"), "roundHalfUp(", "Math.round(");
  edit(
    &repo.join("src/reports/reports.controller.ts"),
    "@UseGuards(ApiKeyGuard)",
    "@UseGuards(ApiKeyGuard)\n  @Get('v2/reports/export')",
  );
  fs::write(
    repo.join("src/common/currency.ts"),
    "export function toDollars(cents: number): number {\n  return cents / 100;\n}\n",
  )
  .unwrap();
  git(repo, &["add", "-A"]);
  git(repo, &["commit", "-q", "-m", "feature"]);
}

#[test]
fn should_match_a_fresh_full_index_after_every_branch_switch() {
  let acme = Acme::indexed();
  let repo = acme.api_repo();
  git(&repo, &["init", "-q"]);
  git(&repo, &["add", "-A"]);
  git(&repo, &["commit", "-q", "-m", "base"]);
  commit_feature_branch(&repo);
  acme.assert_matches_full_index(&BRANCH_QUERIES, "on feature after committing");

  for (branch, renamed) in [("main", false), ("feature", true), ("main", false)] {
    git(&repo, &["checkout", "-q", branch]);
    let symbol = acme.run(&["query", "symbol", "formatCents"]);
    assert_eq!(symbol.code, if renamed { 0 } else { 4 }, "{branch}: {symbol}");
    let api = acme.answer(&["api"]);
    let routes: Vec<String> = entity_refs(&api["endpoints"])
      .into_iter()
      .filter(|found| found.kind == "api_endpoint")
      .map(|found| found.short)
      .collect();
    assert_eq!(routes.contains(&"GET /v2/reports/export".to_owned()), renamed, "{branch}: {routes:?}");
    acme.assert_matches_full_index(&BRANCH_QUERIES, &format!("after checking out {branch}"));
  }
}
