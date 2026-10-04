mod e2e_support;

use std::path::PathBuf;

use e2e_support::{Run, Sandbox};

fn indexed_acme() -> (Sandbox, PathBuf) {
  let sandbox = Sandbox::new();
  let root = sandbox.acme();
  sandbox.run(&root.join("acme-api"), &["index"]).ok();
  (sandbox, root)
}

fn without_timing(run: &Run) -> String {
  let mut lines = run.stdout.lines();
  let first = lines.next().unwrap_or_default();
  let mut masked = String::new();
  let mut digits = String::new();
  let mut rest = first.chars().peekable();
  while let Some(c) = rest.next() {
    if c.is_ascii_digit() {
      digits.push(c);
      continue;
    }
    let is_duration = c == 'm' && !digits.is_empty() && rest.peek() == Some(&'s');
    match is_duration {
      true => {
        rest.next();
        masked.push_str("<t>");
      }
      false => {
        masked.push_str(&digits);
        masked.push(c);
      }
    }
    digits.clear();
  }
  masked.push_str(&digits);
  std::iter::once(masked.as_str()).chain(lines).collect::<Vec<_>>().join("\n")
}

fn repo_config(sandbox: &Sandbox, repo: &str) -> serde_json::Value {
  let shown = sandbox.run(&sandbox.work(), &["project", "show", "acme", "--json"]).ok().json();
  shown["repo"]
    .as_array()
    .unwrap()
    .iter()
    .find(|candidate| candidate["name"] == repo)
    .cloned()
    .unwrap_or_else(|| panic!("{repo} is not registered: {shown:#}"))
}

fn repo_names(sandbox: &Sandbox) -> Vec<String> {
  sandbox.repos("acme").into_iter().map(|(name, _)| name).collect()
}

#[test]
fn should_give_byte_identical_output_through_every_top_level_shortcut() {
  let (sandbox, root) = indexed_acme();
  let api = root.join("acme-api");
  let cases: [&[&str]; 10] = [
    &["search", "convert currency string to cents"],
    &["impact", "UsersService.findOne"],
    &["context", "add a DELETE /orders/:id endpoint with api-key auth"],
    &["callers", "UsersService.findOne"],
    &["callees", "OrdersController.findOne"],
    &["symbol", "UsersService.findOne"],
    &["api"],
    &["explain", "UsersController.findOne"],
    &["read", "toCents"],
    &["tests-for", "UsersService"],
  ];

  for case in cases {
    let mut long = vec!["query"];
    long.extend_from_slice(case);
    let shortcut = sandbox.run(&api, case).ok();
    let expected = sandbox.run(&api, &long).ok();

    assert_eq!(without_timing(&shortcut), without_timing(&expected), "{case:?}");
    assert!(shortcut.stdout.lines().count() > 1, "{shortcut}");
  }
}

#[test]
fn should_apply_an_exclude_on_the_next_query_without_an_explicit_index() {
  let (sandbox, root) = indexed_acme();
  let api = root.join("acme-api");
  sandbox.run(&api, &["query", "symbol", "OrdersService"]).ok();

  let changed = sandbox.run(&api, &["project", "set-repo", "acme", "acme-api", "--exclude", "src/orders/**"]).ok();

  assert!(changed.stdout.contains("acme-api: exclude: (none) -> src/orders/**"), "{changed}");
  assert_eq!(repo_config(&sandbox, "acme-api")["exclude"], serde_json::json!(["src/orders/**"]));
  assert_eq!(repo_names(&sandbox), ["acme-api", "acme-sdk", "acme-web", "acme-billing"].map(String::from));
  let excluded = sandbox.run(&api, &["query", "symbol", "OrdersService"]).expect(4);
  assert!(excluded.stdout.contains("not found"), "{excluded}");

  sandbox.run(&api, &["project", "set-repo", "acme", "acme-api", "--clear-filters"]).ok();

  assert_eq!(repo_config(&sandbox, "acme-api").get("exclude"), None);
  let restored = sandbox.run(&api, &["query", "symbol", "OrdersService"]).ok();
  assert!(restored.stdout.contains("OrdersService"), "{restored}");
}

#[test]
fn should_restrict_a_repo_to_the_requested_languages() {
  let (sandbox, root) = indexed_acme();
  let api = root.join("acme-api");
  sandbox.run(&api, &["query", "symbol", "useMe"]).ok();

  sandbox.run(&api, &["project", "set-repo", "acme", "acme-web", "--languages", "rust"]).ok();

  assert_eq!(repo_config(&sandbox, "acme-web")["languages"], serde_json::json!(["rust"]));
  sandbox.run(&api, &["query", "symbol", "useMe"]).expect(4);

  sandbox.run(&api, &["project", "set-repo", "acme", "acme-web", "--languages", "rust,ts"]).ok();

  assert_eq!(
    repo_config(&sandbox, "acme-web")["languages"],
    serde_json::json!(["rust", "typescript", "tsx", "javascript", "jsx"])
  );
  sandbox.run(&api, &["query", "symbol", "useMe"]).ok();
}

#[test]
fn should_record_filters_given_to_add_repo() {
  let sandbox = Sandbox::new();
  let sdk = sandbox.copy_repo("acme", "acme-sdk");
  let web = sandbox.copy_repo("acme", "acme-web");
  sandbox.create_project("acme", &[sdk]);

  let added = sandbox.run(
    &sandbox.work(),
    &[
      "project",
      "add-repo",
      "acme",
      web.to_str().unwrap(),
      "--include",
      "src/**",
      "--exclude",
      "**/*.spec.ts",
      "--exclude",
      "legacy/",
      "--languages",
      "ts",
    ],
  );

  added.ok();
  let config = repo_config(&sandbox, "acme-web");
  assert_eq!(config["include"], serde_json::json!(["src/**"]));
  assert_eq!(config["exclude"], serde_json::json!(["**/*.spec.ts", "legacy/"]));
  assert_eq!(config["languages"], serde_json::json!(["typescript", "tsx", "javascript", "jsx"]));
  let shown = sandbox.run(&sandbox.work(), &["project", "show", "acme"]).ok();
  assert!(shown.stdout.contains("    exclude **/*.spec.ts, legacy/"), "{shown}");
}

#[test]
fn should_reject_a_bad_glob_with_exit_2_and_a_hint_and_leave_the_registry_untouched() {
  let (sandbox, root) = indexed_acme();
  let api = root.join("acme-api");
  let before = repo_names(&sandbox);
  let extra = sandbox.copy_repo("globex", "globex-api");

  let set = sandbox.run(&api, &["project", "set-repo", "acme", "acme-sdk", "--exclude", "{a"]).expect(2);
  let added = sandbox.run(&api, &["project", "add-repo", "acme", extra.to_str().unwrap(), "--exclude", "{a"]).expect(2);

  assert!(set.stderr.contains("hint: globs use gitignore syntax"), "{set}");
  assert!(added.stderr.contains("hint: globs use gitignore syntax"), "{added}");
  assert_eq!(repo_names(&sandbox), before);
  assert_eq!(repo_config(&sandbox, "acme-sdk").get("exclude"), None);
}

#[test]
fn should_reject_an_unknown_language_and_an_empty_set_repo_with_exit_2() {
  let (sandbox, root) = indexed_acme();
  let api = root.join("acme-api");

  let language = sandbox.run(&api, &["project", "set-repo", "acme", "acme-sdk", "--languages", "cobol"]).expect(2);
  let empty = sandbox.run(&api, &["project", "set-repo", "acme", "acme-sdk"]).expect(2);
  let conflicting = sandbox
    .run(&api, &["project", "set-repo", "acme", "acme-sdk", "--clear-filters", "--exclude", "x/"])
    .expect(2);

  assert!(language.stderr.contains("unknown language \"cobol\""), "{language}");
  assert!(empty.stderr.contains("--include"), "{empty}");
  assert!(conflicting.stderr.contains("--clear-filters"), "{conflicting}");
  assert_eq!(repo_names(&sandbox), ["acme-api", "acme-sdk", "acme-web", "acme-billing"].map(String::from));
}

#[test]
fn should_report_no_changes_when_set_repo_repeats_the_current_filters() {
  let (sandbox, root) = indexed_acme();
  let api = root.join("acme-api");
  sandbox.run(&api, &["project", "set-repo", "acme", "acme-sdk", "--exclude", "dist/"]).ok();

  let again = sandbox.run(&api, &["project", "set-repo", "acme", "acme-sdk", "--exclude", "dist/"]).ok();
  let missing = sandbox.run(&api, &["project", "set-repo", "acme", "ghost", "--exclude", "dist/"]).expect(2);

  assert_eq!(again.stdout, "no changes\n");
  assert!(missing.stderr.contains("hint: list a project's repos"), "{missing}");
}
