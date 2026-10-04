use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::TempDir;

const ACME_REPOS: [&str; 4] = ["acme-api", "acme-sdk", "acme-web", "acme-billing"];

struct Indexed {
  home: TempDir,
  _work: TempDir,
  root: PathBuf,
}

impl Indexed {
  fn new() -> Self {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(work.path()).unwrap().join("acme");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/acme");
    for repo in ACME_REPOS {
      copy_dir(&fixtures.join(repo), &root.join(repo));
    }
    let indexed = Self { home, _work: work, root };
    let mut args = vec!["project".to_owned(), "create".to_owned(), "acme".to_owned()];
    for repo in ACME_REPOS {
      args.push("--repo".to_owned());
      args.push(indexed.root.join(repo).display().to_string());
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    assert_ok(&indexed.run(&indexed.root, &args));
    assert_ok(&indexed.run(&indexed.root, &["index", "--project", "acme"]));
    indexed
  }

  fn run(&self, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_code-atlas"))
      .args(args)
      .current_dir(cwd)
      .env("CODE_ATLAS_HOME", self.home.path())
      .env_remove("RUST_LOG")
      .stdin(Stdio::null())
      .output()
      .unwrap()
  }

  fn query(&self, cwd: &Path, args: &[&str]) -> Value {
    let mut full = vec!["query"];
    full.extend_from_slice(args);
    full.extend_from_slice(&["--json", "--project", "acme"]);
    let output = self.run(cwd, &full);
    assert_ok(&output);
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("stdout is not JSON ({error}): {}", String::from_utf8_lossy(&output.stdout)))
  }
}

fn copy_dir(from: &Path, to: &Path) {
  fs::create_dir_all(to).unwrap();
  for entry in fs::read_dir(from).unwrap() {
    let entry = entry.unwrap();
    let target = to.join(entry.file_name());
    if entry.file_type().unwrap().is_dir() {
      copy_dir(&entry.path(), &target);
    } else {
      fs::copy(entry.path(), &target).unwrap();
    }
  }
}

fn assert_ok(output: &Output) {
  assert_eq!(
    output.status.code(),
    Some(0),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr)
  );
}

fn short(value: &Value) -> &str {
  value["short"].as_str().unwrap_or_else(|| panic!("no short name in {value}"))
}

#[test]
fn should_resolve_file_targets_relative_to_the_working_directory_in_every_target_query() {
  let indexed = Indexed::new();
  let users = indexed.root.join("acme-api/src/users");

  let symbol = indexed.query(&users, &["symbol", "users.service.ts:18"]);
  assert_eq!(symbol["answer"]["outcome"], "found", "{symbol}");
  assert_eq!(short(&symbol["answer"]["entity"]), "UsersService.findOne");
  assert_eq!(symbol["answer"]["entity"]["location"]["repo"], "acme-api");

  let read = indexed.query(&users, &["read", "./users.service.ts:29"]);
  assert_eq!(read["answer"]["outcome"], "found", "{read}");
  assert_eq!(short(&read["answer"]["entity"]), "UsersService.update");

  for query in ["callers", "callees", "dependents", "dependencies"] {
    let traversed = indexed.query(&users, &[query, "users.service.ts:18"]);
    assert_eq!(traversed["answer"]["outcome"], "found", "{query}: {traversed}");
  }

  let impact = indexed.query(&users, &["impact", "users.service.ts:18", "../common/money.ts"]);
  assert_eq!(impact["answer"]["outcome"], "found", "{impact}");
  let targets: Vec<&str> = impact["answer"]["targets"].as_array().unwrap().iter().map(short).collect();
  assert_eq!(targets.len(), 2, "{targets:?}");
  assert!(targets.contains(&"UsersService.findOne"), "{targets:?}");

  for query in ["tests-for", "explain", "similar"] {
    let answered = indexed.query(&users, &[query, "users.service.ts:18"]);
    assert_eq!(answered["answer"]["outcome"], "found", "{query}: {answered}");
  }
}

#[test]
fn should_resolve_paths_relative_to_a_directory_above_the_repositories_and_absolute_paths() {
  let indexed = Indexed::new();

  let from_root = indexed.query(&indexed.root, &["symbol", "acme-api/src/users/users.service.ts:18"]);
  assert_eq!(from_root["answer"]["outcome"], "found", "{from_root}");
  assert_eq!(short(&from_root["answer"]["entity"]), "UsersService.findOne");

  let absolute = indexed.root.join("acme-sdk/src/money.ts").display().to_string();
  let sdk_web = indexed.root.join("acme-web/src");
  let file = indexed.query(&sdk_web, &["symbol", &absolute]);
  assert_eq!(file["answer"]["outcome"], "found", "{file}");
  assert_eq!(file["answer"]["entity"]["location"]["repo"], "acme-sdk");
  assert_eq!(file["answer"]["entity"]["location"]["path"], "src/money.ts");
}

#[test]
fn should_resolve_a_whole_file_target_relative_to_the_working_directory() {
  let indexed = Indexed::new();
  let sdk_src = indexed.root.join("acme-sdk/src");

  let file = indexed.query(&sdk_src, &["symbol", "money.ts"]);
  assert_eq!(file["answer"]["outcome"], "found", "{file}");
  assert_eq!(file["answer"]["entity"]["location"]["repo"], "acme-sdk");
  assert_eq!(file["answer"]["entity"]["location"]["path"], "src/money.ts");
}

#[test]
fn should_keep_repo_relative_paths_names_and_endpoints_working_from_anywhere() {
  let indexed = Indexed::new();
  let web = indexed.root.join("acme-web");

  let repo_relative = indexed.query(&web, &["symbol", "src/users/users.service.ts:18"]);
  assert_eq!(short(&repo_relative["answer"]["entity"]), "UsersService.findOne", "{repo_relative}");

  let qualified = indexed.query(&web, &["symbol", "acme-api:src/users/users.service.ts:18"]);
  assert_eq!(short(&qualified["answer"]["entity"]), "UsersService.findOne", "{qualified}");

  let named = indexed.query(&web, &["symbol", "UsersService.findOne"]);
  assert_eq!(short(&named["answer"]["entity"]), "UsersService.findOne", "{named}");

  let endpoint = indexed.query(&web, &["symbol", "GET /users/{id}"]);
  assert_eq!(endpoint["answer"]["entity"]["key"], "endpoint:acme-api:GET /users/{id}", "{endpoint}");

  let missing = indexed.run(&web, &["query", "symbol", "src/nowhere.ts:3", "--project", "acme"]);
  assert_eq!(missing.status.code(), Some(4), "{}", String::from_utf8_lossy(&missing.stdout));
}
