use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

const ACME_REPOS: [&str; 4] = ["acme-api", "acme-sdk", "acme-web", "acme-billing"];

struct Sandbox {
  home: TempDir,
  work: TempDir,
}

impl Sandbox {
  fn new() -> Self {
    Self {
      home: tempfile::tempdir().unwrap(),
      work: tempfile::tempdir().unwrap(),
    }
  }

  fn work(&self) -> PathBuf {
    fs::canonicalize(self.work.path()).unwrap()
  }

  fn run(&self, cwd: &Path, args: &[&str]) -> Output {
    self.run_env(cwd, args, &[])
  }

  fn run_env(&self, cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_code-atlas"))
      .args(args)
      .current_dir(cwd)
      .env("CODE_ATLAS_HOME", self.home.path())
      .env_remove("RUST_LOG")
      .envs(envs.iter().copied())
      .stdin(Stdio::null())
      .output()
      .unwrap()
  }

  fn acme(&self) -> PathBuf {
    let root = self.work().join("acme");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/acme");
    for repo in ACME_REPOS {
      copy_dir(&fixtures.join(repo), &root.join(repo));
    }
    root
  }

  fn create_acme(&self) -> PathBuf {
    let root = self.acme();
    let mut args = vec!["project", "create", "acme"];
    let paths: Vec<String> = ACME_REPOS.iter().map(|repo| root.join(repo).display().to_string()).collect();
    for path in &paths {
      args.push("--repo");
      args.push(path);
    }
    assert_ok(&self.run(&root, &args));
    root
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

fn stdout(output: &Output) -> String {
  String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
  String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &Output) -> i32 {
  output.status.code().unwrap()
}

fn assert_ok(output: &Output) {
  assert_eq!(code(output), 0, "stdout: {}\nstderr: {}", stdout(output), stderr(output));
}

fn json(output: &Output) -> Value {
  serde_json::from_str(&stdout(output)).unwrap_or_else(|error| panic!("stdout is not JSON ({error}): {}", stdout(output)))
}

#[test]
fn should_follow_the_full_lifecycle_over_acme() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");

  let indexed = sandbox.run(&api, &["index"]);
  assert_ok(&indexed);
  assert!(stdout(&indexed).contains("indexed acme: run 1"), "{}", stdout(&indexed));
  assert!(stdout(&indexed).contains("outcome: ok"));

  let found = sandbox.run(&api, &["query", "symbol", "UsersService.findOne"]);
  assert_ok(&found);
  assert!(
    stdout(&found).contains("UsersService.findOne method acme-api:src/users/users.service.ts"),
    "{}",
    stdout(&found)
  );

  let before = sandbox.run(&api, &["query", "symbol", "UsersService.archiveUser"]);
  assert_eq!(code(&before), 4);
  assert!(stdout(&before).contains("not found"), "{}", stdout(&before));

  let service = api.join("src/users/users.service.ts");
  let source = fs::read_to_string(&service).unwrap();
  let edited = source.replacen(
    "  create(input: CreateUserInput)",
    "  archiveUser(id: string): Promise<void> {\n    return this.remove(id);\n  }\n\n  create(input: CreateUserInput)",
    1,
  );
  assert_ne!(source, edited);
  fs::write(&service, edited).unwrap();

  let after = sandbox.run(&api, &["query", "symbol", "UsersService.archiveUser"]);
  assert_ok(&after);
  assert!(stdout(&after).contains("UsersService.archiveUser method"), "{}", stdout(&after));

  assert_ok(&sandbox.run(&api, &["query", "symbol", "create_invoice"]));
  assert_ok(&sandbox.run(&api, &["project", "remove-repo", "acme", "acme-billing"]));
  let gone = sandbox.run(&api, &["query", "symbol", "create_invoice"]);
  assert_eq!(code(&gone), 4);
  assert!(stdout(&gone).contains("not found"), "{}", stdout(&gone));
  let overview = sandbox.run(&api, &["query", "overview", "--json"]);
  assert_ok(&overview);
  let repos: Vec<String> = json(&overview)["answer"]["repos"]
    .as_array()
    .unwrap()
    .iter()
    .map(|repo| repo["repo"].as_str().unwrap().to_owned())
    .collect();
  assert_eq!(repos, ["acme-api", "acme-sdk", "acme-web"]);
}

#[test]
fn should_refuse_to_guess_between_projects_that_share_a_repo() {
  let sandbox = Sandbox::new();
  let repo = sandbox.work().join("shared");
  fs::create_dir_all(&repo).unwrap();
  for name in ["alpha", "beta"] {
    assert_ok(&sandbox.run(&repo, &["project", "create", name, "--repo", repo.to_str().unwrap()]));
  }
  let output = sandbox.run(&repo, &["query", "overview"]);
  assert_eq!(code(&output), 2);
  assert_eq!(stdout(&output), "");
  let message = stderr(&output);
  assert!(message.contains("alpha") && message.contains("beta"), "{message}");
  assert!(message.contains("--project"), "{message}");
  assert!(sandbox.run(&repo, &["query", "overview", "--project", "alpha"]).status.success());
  assert!(sandbox.run(&repo, &["--project", "beta", "query", "overview"]).status.success());
}

#[test]
fn should_exit_2_with_a_hint_outside_every_project() {
  let sandbox = Sandbox::new();
  let output = sandbox.run(&sandbox.work(), &["status"]);
  assert_eq!(code(&output), 2);
  assert!(stderr(&output).contains("not inside any registered project"), "{}", stderr(&output));
  assert!(stderr(&output).contains("hint:"));
  assert_eq!(code(&sandbox.run(&sandbox.work(), &["status", "--project", "missing"])), 2);
}

#[test]
fn should_print_the_build_commit_in_the_version() {
  let sandbox = Sandbox::new();
  let output = sandbox.run(&sandbox.work(), &["--version"]);
  assert_ok(&output);
  let version = stdout(&output);
  let commit = version
    .trim()
    .strip_prefix(concat!("code-atlas ", env!("CARGO_PKG_VERSION"), " (commit "))
    .and_then(|rest| rest.strip_suffix(')'))
    .unwrap();
  assert!(commit == "unknown" || (commit.len() == 40 && commit.chars().all(|c| c.is_ascii_hexdigit())), "{version}");
}

#[test]
fn should_print_parseable_json_on_stdout_and_nothing_on_stderr() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  let output = sandbox.run(&api, &["query", "search", "UsersService", "--json", "--limit", "3"]);
  assert_ok(&output);
  assert_eq!(stderr(&output), "");
  let document = json(&output);
  assert_eq!(document["freshness"]["state"], "fresh");
  assert!(document["answer"]["total_hits"].as_u64().unwrap() > 0);
  let text = sandbox.run(&api, &["search", "UsersService"]);
  assert_ok(&text);
  assert!(stdout(&text).starts_with("fresh run="), "{}", stdout(&text));
  assert_eq!(stderr(&text), "");
}

#[test]
fn should_send_structured_logs_to_stderr_only() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  let output = sandbox.run(&api, &["--log-level", "debug", "--log-format", "json", "query", "overview", "--json"]);
  assert_ok(&output);
  json(&output);
  let logs = stderr(&output);
  assert!(!logs.trim().is_empty());
  for line in logs.lines() {
    serde_json::from_str::<Value>(line).unwrap_or_else(|error| panic!("log line is not JSON ({error}): {line}"));
  }
  assert_eq!(code(&sandbox.run(&api, &["--log-level", "=[", "status"])), 2);
}

#[test]
fn should_exit_3_but_still_print_when_a_repo_disappears() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  assert_ok(&sandbox.run(&api, &["index"]));
  fs::remove_dir_all(root.join("acme-web")).unwrap();
  let output = sandbox.run(&api, &["query", "overview"]);
  assert_eq!(code(&output), 3, "stderr: {}", stderr(&output));
  assert!(stdout(&output).starts_with("DEGRADED"), "{}", stdout(&output));
  assert!(stdout(&output).contains("acme-web"));
  let json_output = sandbox.run(&api, &["query", "overview", "--json"]);
  assert_eq!(code(&json_output), 3);
  assert_eq!(json(&json_output)["freshness"]["state"], "degraded");
}

#[test]
fn should_find_code_similar_to_a_snippet_from_the_tree_without_a_project_flag() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let billing = root.join("acme-billing");
  assert_ok(&sandbox.run(&billing, &["index", "--project", "acme"]));
  let output = sandbox.run(&billing, &["query", "similar", "--snippet", "src/money.rs:1-15", "--json"]);
  assert_ok(&output);
  let document = json(&output);
  assert_eq!(document["answer"]["outcome"], "found");
  assert_eq!(document["answer"]["target"]["entity"]["short"], "to_cents");
  assert!(
    document["answer"]["members"]
      .as_array()
      .unwrap()
      .iter()
      .any(|member| member["entity"]["short"] == "toCents")
  );
  let malformed = sandbox.run(&billing, &["query", "similar", "--snippet", "src/money.rs", "--json"]);
  assert_eq!(code(&malformed), 2);
}

#[test]
fn should_write_connect_claude_files_idempotently() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  let first = sandbox.run(&api, &["connect", "claude"]);
  assert_ok(&first);
  assert!(
    stdout(&first).contains(".mcp.json: created") && stdout(&first).contains("AGENTS.md: created"),
    "{}",
    stdout(&first)
  );

  let config: Value = serde_json::from_str(&fs::read_to_string(api.join(".mcp.json")).unwrap()).unwrap();
  let server = &config["mcpServers"]["code-atlas"];
  assert_eq!(server["command"], fs::canonicalize(env!("CARGO_BIN_EXE_code-atlas")).unwrap().display().to_string());
  assert_eq!(server["args"], serde_json::json!(["mcp", "--project", "acme"]));
  let guidance = fs::read_to_string(api.join("AGENTS.md")).unwrap();
  for tool in ["`context`", "`impact`", "`search`", "`similar`", "`explain`", "[heuristic]", "[inferred]"] {
    assert!(guidance.contains(tool), "missing {tool}");
  }

  let mcp_before = fs::read(api.join(".mcp.json")).unwrap();
  let agents_before = fs::read(api.join("AGENTS.md")).unwrap();
  let second = sandbox.run(&api, &["connect", "claude"]);
  assert_ok(&second);
  assert!(
    stdout(&second).contains(".mcp.json: unchanged") && stdout(&second).contains("AGENTS.md: unchanged"),
    "{}",
    stdout(&second)
  );
  assert_eq!(fs::read(api.join(".mcp.json")).unwrap(), mcp_before);
  assert_eq!(fs::read(api.join("AGENTS.md")).unwrap(), agents_before);
}

#[test]
fn should_merge_connect_claude_into_existing_files_without_clobbering() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let web = root.join("acme-web");
  let existing = r#"{"mcpServers":{"other":{"command":"other-server"}},"extra":true}"#;
  fs::write(web.join(".mcp.json"), existing).unwrap();
  fs::write(web.join("CLAUDE.md"), "# My rules\n\nnever touch this\n").unwrap();

  let refused = sandbox.run(&web, &["connect", "claude"]);
  assert_eq!(code(&refused), 2);
  assert!(stderr(&refused).contains("--yes"), "{}", stderr(&refused));
  assert_eq!(fs::read_to_string(web.join(".mcp.json")).unwrap(), existing);
  assert_eq!(fs::read_to_string(web.join("CLAUDE.md")).unwrap(), "# My rules\n\nnever touch this\n");
  assert!(!web.join("AGENTS.md").exists());

  assert_ok(&sandbox.run(&web, &["connect", "claude", "--yes"]));
  let config: Value = serde_json::from_str(&fs::read_to_string(web.join(".mcp.json")).unwrap()).unwrap();
  assert_eq!(config["mcpServers"]["other"]["command"], "other-server");
  assert_eq!(config["extra"], true);
  assert_eq!(config["mcpServers"]["code-atlas"]["args"][2], "acme");
  let claude = fs::read_to_string(web.join("CLAUDE.md")).unwrap();
  assert!(claude.starts_with("# My rules\n\nnever touch this\n\n<!-- code-atlas:begin -->"), "{claude}");
  assert!(claude.ends_with("<!-- code-atlas:end -->\n"));
  assert!(!web.join("AGENTS.md").exists());

  let settled = (fs::read(web.join(".mcp.json")).unwrap(), fs::read(web.join("CLAUDE.md")).unwrap());
  let again = sandbox.run(&web, &["connect", "claude"]);
  assert_ok(&again);
  assert_eq!((fs::read(web.join(".mcp.json")).unwrap(), fs::read(web.join("CLAUDE.md")).unwrap()), settled);
}

#[test]
fn should_refuse_to_overwrite_an_invalid_mcp_config() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let sdk = root.join("acme-sdk");
  fs::write(sdk.join(".mcp.json"), "{ not json").unwrap();
  let output = sandbox.run(&sdk, &["connect", "claude", "--yes"]);
  assert_eq!(code(&output), 2);
  assert_eq!(fs::read_to_string(sdk.join(".mcp.json")).unwrap(), "{ not json");
  assert!(!sdk.join("AGENTS.md").exists());
}

#[test]
fn should_report_environment_and_database_health_in_doctor() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  assert_ok(&sandbox.run(&api, &["index"]));
  let output = sandbox.run(&api, &["doctor"]);
  assert!([0, 3].contains(&code(&output)), "{}", stdout(&output));
  let report = stdout(&output);
  for expected in [
    "registry: 1 projects",
    "belongs to project acme",
    "acme database: integrity_check passed",
    "acme index lock: free",
    "rust-analyzer",
    "npx",
    "claude",
    "0 failures",
  ] {
    assert!(report.contains(expected), "missing {expected:?} in {report}");
  }

  fs::write(sandbox.home.path().join("data/acme/atlas.db"), vec![0xAB; 4096]).unwrap();
  let broken = sandbox.run(&api, &["doctor"]);
  assert_eq!(code(&broken), 1);
  assert!(stdout(&broken).contains("FAIL  acme database"), "{}", stdout(&broken));
}

#[test]
fn should_manage_projects_and_purge_data_only_when_confirmed() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  assert_ok(&sandbox.run(&api, &["index"]));
  let data = sandbox.home.path().join("data/acme");
  assert!(data.exists());

  let listed = sandbox.run(&api, &["project", "list"]);
  assert_eq!(stdout(&listed), "acme  acme-api, acme-sdk, acme-web, acme-billing\n");
  let shown = json(&sandbox.run(&api, &["project", "show", "acme", "--json"]));
  assert_eq!(shown["repo"].as_array().unwrap().len(), 4);

  assert_ok(&sandbox.run(&api, &["project", "rename-repo", "acme", "acme-web", "storefront"]));
  assert!(stdout(&sandbox.run(&api, &["project", "show", "acme"])).contains("storefront"));
  let duplicate = sandbox.run(&api, &["project", "add-repo", "acme", root.join("acme-sdk").to_str().unwrap()]);
  assert_eq!(code(&duplicate), 2);

  let unconfirmed = sandbox.run(&api, &["project", "delete", "acme", "--purge-data"]);
  assert_eq!(code(&unconfirmed), 2);
  assert!(data.exists());
  assert!(stdout(&sandbox.run(&api, &["project", "list"])).contains("acme"));

  assert_ok(&sandbox.run(&api, &["project", "delete", "acme", "--purge-data", "--yes"]));
  assert!(!data.exists());
  assert_eq!(stdout(&sandbox.run(&api, &["project", "list"])), "no projects registered\n");
}

#[test]
fn should_pin_a_repo_to_a_project_with_init() {
  let sandbox = Sandbox::new();
  let repo = sandbox.work().join("Solo Repo");
  fs::create_dir_all(repo.join("src")).unwrap();
  fs::write(repo.join("src/lib.rs"), "pub fn answer() -> u32 {\n  42\n}\n").unwrap();
  fs::write(repo.join("Cargo.toml"), "[package]\nname = \"solo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n").unwrap();

  let first = sandbox.run(&repo, &["init"]);
  assert_ok(&first);
  assert!(stdout(&first).contains("project solo-repo: project created"), "{}", stdout(&first));
  assert_eq!(fs::read_to_string(repo.join(".code-atlas.toml")).unwrap(), "project = \"solo-repo\"\n");
  assert!(stdout(&sandbox.run(&repo, &["init"])).contains(".code-atlas.toml: unchanged"));
  assert_eq!(code(&sandbox.run(&repo, &["init", "--project", "other"])), 2);

  assert_ok(&sandbox.run(&repo.join("src"), &["index"]));
  let found = sandbox.run(&repo.join("src"), &["symbol", "answer"]);
  assert_ok(&found);
  assert!(stdout(&found).contains("answer function solo-repo:src/lib.rs"), "{}", stdout(&found));
}

#[test]
fn should_inspect_runs_and_files_with_debug_commands() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  assert_ok(&sandbox.run(&api, &["index"]));
  let run = json(&sandbox.run(&api, &["debug", "run", "1", "--json"]));
  assert_eq!(run["status"], "ok");
  assert!(
    run["owners"]
      .as_array()
      .unwrap()
      .iter()
      .any(|owner| owner["owner"] == "acme-api:src/users/users.service.ts")
  );
  assert_eq!(code(&sandbox.run(&api, &["debug", "run", "99"])), 4);

  let dump = json(&sandbox.run(&api, &["debug", "dump-file", "acme-api:src/users/users.service.ts"]));
  assert_eq!(dump["file"]["language"], "typescript");
  assert!(
    dump["entities"]
      .as_array()
      .unwrap()
      .iter()
      .any(|entity| entity["key"].as_str().is_some_and(|key| key.contains("findOne")))
  );
  assert_eq!(code(&sandbox.run(&api, &["debug", "dump-file", "acme-api:src/nope.ts"])), 4);

  let context = sandbox.run(&api, &["debug", "explain-context", "add rate limiting to user lookup"]);
  assert_ok(&context);
}

#[test]
fn should_keep_progress_and_diagnostics_off_stdout_during_index() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let output = sandbox.run(&root.join("acme-api"), &["index", "--full"]);
  assert_ok(&output);
  assert_eq!(stderr(&output), "");
  assert!(stdout(&output).starts_with("indexed acme: run 1"));
}

#[test]
fn should_shut_watch_down_cleanly_on_interrupt() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  let mut child = Command::new(env!("CARGO_BIN_EXE_code-atlas"))
    .args(["watch"])
    .current_dir(&api)
    .env("CODE_ATLAS_HOME", sandbox.home.path())
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
  let mut banner = String::new();
  BufReader::new(child.stderr.take().unwrap()).read_line(&mut banner).unwrap();
  assert!(banner.starts_with("watching acme (4 repos)"), "{banner}");
  let signalled = Command::new("kill").args(["-INT", &child.id().to_string()]).status().unwrap();
  assert!(signalled.success());
  let status = child.wait().unwrap();
  assert_eq!(status.code(), Some(0));
  assert!(sandbox.home.path().join("data/acme/atlas.db").exists());
}

const CLAUDE_STUB: &str = "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$FAKE_CLAUDE_LOG\"\ncase \"$2\" in remove) exit 1;; esac\nexit 0\n";

fn fake_claude(dir: &Path) -> PathBuf {
  use std::os::unix::fs::PermissionsExt;
  let bin = dir.join("bin");
  fs::create_dir_all(&bin).unwrap();
  let script = bin.join("claude");
  fs::write(&script, CLAUDE_STUB).unwrap();
  fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
  bin
}

#[test]
fn should_leave_every_untouched_byte_of_an_existing_mcp_config_alone() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let sdk = root.join("acme-sdk");
  let existing = "{\n    \"zeta\": 1.10,\n    \"mcpServers\": {\n        \"other\": { \"command\": \"x\", \"ratio\": 2.50 }\n    },\n    \"alpha\": [3, 1, 2]\n}\n";
  fs::write(sdk.join(".mcp.json"), existing).unwrap();
  assert_ok(&sandbox.run(&sdk, &["connect", "claude", "--yes"]));
  let edited = fs::read_to_string(sdk.join(".mcp.json")).unwrap();
  assert!(
    edited.starts_with("{\n    \"zeta\": 1.10,\n    \"mcpServers\": {\n        \"other\": { \"command\": \"x\", \"ratio\": 2.50 },\n        \"code-atlas\": {"),
    "{edited}"
  );
  assert!(edited.ends_with("\n        }\n    },\n    \"alpha\": [3, 1, 2]\n}\n"), "{edited}");
  let before = fs::read(sdk.join(".mcp.json")).unwrap();
  assert_ok(&sandbox.run(&sdk, &["connect", "claude"]));
  assert_eq!(fs::read(sdk.join(".mcp.json")).unwrap(), before);
}

#[test]
fn should_omit_the_atlas_home_from_a_project_mcp_config_and_warn() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let sdk = root.join("acme-sdk");
  let output = sandbox.run(&sdk, &["connect", "claude"]);
  assert_ok(&output);
  assert!(stderr(&output).contains("CODE_ATLAS_HOME") && stderr(&output).contains("left out"), "{}", stderr(&output));
  let config: Value = serde_json::from_str(&fs::read_to_string(sdk.join(".mcp.json")).unwrap()).unwrap();
  assert!(config["mcpServers"]["code-atlas"].get("env").is_none());
}

#[test]
fn should_register_the_user_scope_through_the_claude_cli() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let web = root.join("acme-web");
  let bin = fake_claude(&sandbox.work());
  let log = sandbox.work().join("claude.log");
  let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
  let envs = [("PATH", path.as_str()), ("FAKE_CLAUDE_LOG", log.to_str().unwrap())];
  assert_ok(&sandbox.run_env(&web, &["connect", "claude", "--scope", "user"], &envs));
  assert_ok(&sandbox.run_env(&web, &["connect", "claude", "--scope", "user"], &envs));
  assert!(!web.join(".mcp.json").exists());
  assert!(web.join("AGENTS.md").exists());
  let calls: Vec<String> = fs::read_to_string(&log).unwrap().lines().map(str::to_owned).collect();
  assert_eq!(calls.iter().filter(|call| call.as_str() == "mcp remove --scope user code-atlas").count(), 2);
  let add = calls.iter().find(|call| call.starts_with("mcp add-json --scope user code-atlas ")).unwrap();
  let entry: Value = serde_json::from_str(add.trim_start_matches("mcp add-json --scope user code-atlas ")).unwrap();
  assert_eq!(entry["args"], serde_json::json!(["mcp", "--project", "acme"]));
  assert_eq!(entry["env"]["CODE_ATLAS_HOME"], sandbox.home.path().to_str().unwrap());
}

#[test]
fn should_print_the_claude_command_and_exit_2_when_claude_is_missing() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let web = root.join("acme-web");
  let empty = sandbox.work().join("empty-bin");
  fs::create_dir_all(&empty).unwrap();
  let output = sandbox.run_env(&web, &["connect", "claude", "--scope", "user"], &[("PATH", empty.to_str().unwrap())]);
  assert_eq!(code(&output), 2);
  assert!(stderr(&output).contains("claude mcp add-json --scope user code-atlas"), "{}", stderr(&output));
  assert!(!web.join("AGENTS.md").exists());
}

#[test]
fn should_show_set_and_persist_project_settings() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  let shown = json(&sandbox.run(&api, &["project", "config", "acme", "--json"]));
  assert_eq!(shown["llm"]["enabled"], false);
  assert_eq!(shown["scip"]["debounce_secs"], 30);

  let changed = sandbox.run(
    &api,
    &[
      "project",
      "config",
      "acme",
      "--llm",
      "on",
      "--llm-provider",
      "none",
      "--scip-debounce",
      "10",
      "--max-file-bytes",
      "2048",
    ],
  );
  assert_ok(&changed);
  let text = stdout(&changed);
  for line in [
    "llm.enabled: false -> true",
    "llm.provider: (default) -> none",
    "scip.debounce_secs: 30 -> 10",
    "max_file_bytes: 1048576 -> 2048",
  ] {
    assert!(text.contains(line), "missing {line:?} in {text}");
  }
  let persisted = json(&sandbox.run(&api, &["project", "config", "acme", "--json"]));
  assert_eq!(persisted["llm"]["enabled"], true);
  assert_eq!(persisted["max_file_bytes"], 2048);
  assert_eq!(stdout(&sandbox.run(&api, &["project", "config", "acme", "--llm", "on"])), "no changes\n");

  assert_ok(&sandbox.run(&api, &["project", "config", "acme", "--reset", "--scip", "off"]));
  let reset = json(&sandbox.run(&api, &["project", "config", "acme", "--json"]));
  assert_eq!(
    (reset["llm"]["enabled"].as_bool(), reset["scip"]["enabled"].as_bool(), reset["max_file_bytes"].as_u64()),
    (Some(false), Some(false), Some(1_048_576))
  );
}

#[test]
fn should_reject_invalid_project_settings_with_exit_2() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  for args in [
    ["project", "config", "acme", "--max-file-bytes", "0"],
    ["project", "config", "acme", "--scip-debounce", "0"],
    ["project", "config", "acme", "--llm-provider", "gpt-magic"],
    ["project", "config", "acme", "--llm", "maybe"],
  ] {
    let output = sandbox.run(&api, &args);
    assert_eq!(code(&output), 2, "{args:?}: {}", stderr(&output));
  }
  assert_eq!(code(&sandbox.run(&api, &["project", "config", "ghost"])), 2);
  assert_eq!(json(&sandbox.run(&api, &["project", "config", "acme", "--json"]))["max_file_bytes"], 1_048_576);
}

#[test]
fn should_pass_the_llm_gate_in_enrich_once_enabled() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  assert_ok(&sandbox.run(&api, &["index"]));
  let gated = sandbox.run(&api, &["enrich"]);
  assert!(stdout(&gated).contains("disabled in the project settings"), "{}", stdout(&gated));
  assert_ok(&sandbox.run(&api, &["project", "config", "acme", "--llm", "on", "--llm-provider", "none"]));
  let open = sandbox.run(&api, &["enrich"]);
  assert!(!stdout(&open).contains("disabled in the project settings"), "{}", stdout(&open));
  assert!(stdout(&open).contains("provider is set to none"), "{}", stdout(&open));
  assert_eq!(code(&open), 3);
}

#[test]
fn should_serve_a_search_while_another_process_indexes() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  let spawn = |args: &[&str]| {
    Command::new(env!("CARGO_BIN_EXE_code-atlas"))
      .args(args)
      .current_dir(&api)
      .env("CODE_ATLAS_HOME", sandbox.home.path())
      .stdin(Stdio::null())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .spawn()
      .unwrap()
  };
  let indexer = spawn(&["index", "--full"]);
  let searcher = spawn(&["search", "UsersService", "--json"]);
  let indexed = indexer.wait_with_output().unwrap();
  let searched = searcher.wait_with_output().unwrap();
  assert_ok(&indexed);
  assert_ok(&searched);
  assert!(json(&searched)["answer"]["total_hits"].as_u64().unwrap() > 0);
}

#[test]
fn should_reload_the_project_when_another_process_changes_it_while_watching() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  assert_ok(&sandbox.run(&api, &["index"]));
  let mut child = Command::new(env!("CARGO_BIN_EXE_code-atlas"))
    .args(["watch"])
    .current_dir(&api)
    .env("CODE_ATLAS_HOME", sandbox.home.path())
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
  let (sender, receiver) = std::sync::mpsc::channel();
  let stderr_pipe = child.stderr.take().unwrap();
  std::thread::spawn(move || {
    for line in BufReader::new(stderr_pipe).lines().map_while(Result::ok) {
      if sender.send(line).is_err() {
        return;
      }
    }
  });
  let timeout = std::time::Duration::from_secs(30);
  assert!(receiver.recv_timeout(timeout).unwrap().starts_with("watching acme (4 repos)"));
  assert_ok(&sandbox.run(&api, &["project", "remove-repo", "acme", "acme-web"]));
  assert_ok(&sandbox.run(&api, &["status"]));
  let mut reloaded = None;
  while let Ok(line) = receiver.recv_timeout(timeout) {
    if line.contains("reloaded") {
      reloaded = Some(line);
      break;
    }
  }
  let _ = Command::new("kill").args(["-INT", &child.id().to_string()]).status();
  let status = child.wait().unwrap();
  assert_eq!(reloaded.as_deref(), Some("project configuration changed; reloaded (3 repos)"));
  assert_eq!(status.code(), Some(0));
}

struct McpSession {
  child: Child,
  stdin: Option<ChildStdin>,
  lines: Receiver<String>,
  seen: Vec<String>,
}

impl McpSession {
  fn start(sandbox: &Sandbox, cwd: &Path) -> Self {
    assert_ok(&sandbox.run(cwd, &["connect", "claude", "--yes"]));
    let config: Value = serde_json::from_str(&fs::read_to_string(cwd.join(".mcp.json")).unwrap()).unwrap();
    let server = &config["mcpServers"]["code-atlas"];
    let arguments: Vec<&str> = server["args"].as_array().unwrap().iter().map(|argument| argument.as_str().unwrap()).collect();
    let mut child = Command::new(server["command"].as_str().unwrap())
      .args(arguments)
      .current_dir(cwd)
      .env("CODE_ATLAS_HOME", sandbox.home.path())
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .spawn()
      .unwrap();
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().unwrap();
    let (sender, lines) = mpsc::channel();
    std::thread::spawn(move || {
      for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if sender.send(line).is_err() {
          return;
        }
      }
    });
    Self {
      child,
      stdin,
      lines,
      seen: Vec::new(),
    }
  }

  fn send(&mut self, message: Value) {
    let stdin = self.stdin.as_mut().unwrap();
    writeln!(stdin, "{message}").unwrap();
    stdin.flush().unwrap();
  }

  fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
    self.send(serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    loop {
      let line = self.lines.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|_| panic!("no response to {method}"));
      let message: Value = serde_json::from_str(&line).unwrap_or_else(|error| panic!("stdout line is not JSON ({error}): {line}"));
      assert_eq!(message["jsonrpc"], "2.0", "{line}");
      self.seen.push(line);
      if message["id"] == id {
        return message;
      }
    }
  }

  fn handshake(&mut self) {
    let reply = self.request(
      1,
      "initialize",
      serde_json::json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "0"}}),
    );
    assert!(reply["result"]["serverInfo"].is_object(), "{reply}");
    self.send(serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
  }

  fn wait_for_exit(mut self) -> (Option<i32>, Vec<String>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
      if let Some(status) = self.child.try_wait().unwrap() {
        while let Ok(line) = self.lines.try_recv() {
          self.seen.push(line);
        }
        return (status.code(), self.seen);
      }
      if Instant::now() > deadline {
        let _ = self.child.kill();
        panic!("the MCP server did not exit within 10s");
      }
      std::thread::sleep(Duration::from_millis(50));
    }
  }
}

fn assert_only_json_rpc(lines: &[String]) {
  assert!(!lines.is_empty());
  for line in lines {
    let message: Value = serde_json::from_str(line).unwrap_or_else(|error| panic!("stdout line is not JSON ({error}): {line}"));
    assert_eq!(message["jsonrpc"], "2.0", "{line}");
  }
}

#[test]
fn should_serve_mcp_over_stdio_with_the_command_connect_writes_and_exit_on_stdin_close() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  assert_ok(&sandbox.run(&api, &["index"]));
  let mut session = McpSession::start(&sandbox, &api);
  session.handshake();
  let tools = session.request(2, "tools/list", serde_json::json!({}));
  let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|tool| tool["name"].as_str().unwrap()).collect();
  assert!(names.contains(&"atlas_context") && names.contains(&"atlas_impact"), "{names:?}");
  let search = session.request(3, "tools/call", serde_json::json!({"name": "atlas_search", "arguments": {"query": "toCents"}}));
  assert_ne!(search["result"]["isError"], true, "{search}");
  let text = search["result"]["content"][0]["text"].as_str().unwrap();
  assert!(text.contains("toCents"), "{text}");
  drop(session.stdin.take());
  let (code, lines) = session.wait_for_exit();
  assert_eq!(code, Some(0));
  assert_only_json_rpc(&lines);
}

#[test]
fn should_exit_cleanly_on_sigterm_while_serving_mcp() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  assert_ok(&sandbox.run(&api, &["index"]));
  let mut session = McpSession::start(&sandbox, &api);
  session.handshake();
  session.request(2, "tools/list", serde_json::json!({}));
  let signalled = Command::new("kill").args(["-TERM", &session.child.id().to_string()]).status().unwrap();
  assert!(signalled.success());
  let (code, lines) = session.wait_for_exit();
  assert_eq!(code, Some(0));
  assert_only_json_rpc(&lines);
}

#[test]
fn should_fail_mcp_with_a_usage_exit_outside_every_project() {
  let sandbox = Sandbox::new();
  let output = sandbox.run(&sandbox.work(), &["mcp"]);
  assert_eq!(code(&output), 2, "{}", stderr(&output));
  assert_eq!(stdout(&output), "");
}
