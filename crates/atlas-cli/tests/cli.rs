use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use atlas_test_lsp::hermetic::{hermetic_catl, sandbox_path};
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
    self.catl().args(args).current_dir(cwd).envs(envs.iter().copied()).stdin(Stdio::null()).output().unwrap()
  }

  fn catl(&self) -> Command {
    hermetic_catl(env!("CARGO_BIN_EXE_catl"), self.home.path())
  }

  fn write_config(&self, text: &str) {
    let config = self.home.path().join("config");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("config.toml"), text).unwrap();
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
    .strip_prefix(concat!("catl ", env!("CARGO_PKG_VERSION"), " (commit "))
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
    stdout(&first).contains(".mcp.json#mcpServers.code-atlas: created") && stdout(&first).contains("AGENTS.md: created"),
    "{}",
    stdout(&first)
  );

  let config: Value = serde_json::from_str(&fs::read_to_string(api.join(".mcp.json")).unwrap()).unwrap();
  let server = &config["mcpServers"]["code-atlas"];
  assert_eq!(server["command"], catl_path());
  assert_eq!(server["args"], serde_json::json!(["mcp", "--project", "acme", "--watch"]));
  assert!(stderr(&first).contains("machine-specific"), "{}", stderr(&first));
  let guidance = fs::read_to_string(api.join("AGENTS.md")).unwrap();
  for tool in [
    "atlas_context",
    "atlas_impact",
    "atlas_search",
    "atlas_similar",
    "atlas_explain",
    "atlas_edit",
    "[heuristic]",
    "[inferred]",
    "[compiler]",
    "DEGRADED",
  ] {
    assert!(guidance.contains(tool), "missing {tool}");
  }
  assert!(guidance.contains("(project `acme`)"), "{guidance}");
  assert!(!guidance.contains("compiler_verified"), "{guidance}");
  assert!(!api.join(".claude").exists());

  let mcp_before = fs::read(api.join(".mcp.json")).unwrap();
  let agents_before = fs::read(api.join("AGENTS.md")).unwrap();
  let second = sandbox.run(&api, &["connect", "claude"]);
  assert_ok(&second);
  assert!(
    stdout(&second).contains(".mcp.json#mcpServers.code-atlas: unchanged") && stdout(&second).contains("AGENTS.md: unchanged"),
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
  assert!(stdout(&output).contains("refused: cannot be read ("), "{}", stdout(&output));
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
  let mut child = sandbox
    .catl()
    .args(["watch"])
    .current_dir(&api)
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

const CLAUDE_STUB: &str = "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$FAKE_CLAUDE_LOG\"\ncase \"$2\" in\n  remove) exit 1;;\n  add-json) printf '{\"mcpServers\":{\"%s\":%s}}' \"$5\" \"$6\" > \"$CLAUDE_CONFIG_DIR/.claude.json\";;\nesac\nexit 0\n";

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

struct FakeClaude {
  path: String,
  log: PathBuf,
  config_dir: PathBuf,
}

impl FakeClaude {
  fn new(sandbox: &Sandbox) -> Self {
    let bin = fake_claude(&sandbox.work());
    let config_dir = sandbox.work().join("claude-config");
    fs::create_dir_all(&config_dir).unwrap();
    Self {
      path: format!("{}:{}", bin.display(), sandbox_path(sandbox.home.path())),
      log: sandbox.work().join("claude.log"),
      config_dir,
    }
  }

  fn connect(&self, sandbox: &Sandbox, cwd: &Path, extra: &[&str]) -> Output {
    let mut args = vec!["connect", "claude", "--scope", "user"];
    args.extend_from_slice(extra);
    let envs = [
      ("PATH", self.path.as_str()),
      ("FAKE_CLAUDE_LOG", self.log.to_str().unwrap()),
      ("CLAUDE_CONFIG_DIR", self.config_dir.to_str().unwrap()),
    ];
    sandbox.run_env(cwd, &args, &envs)
  }

  fn calls(&self) -> Vec<String> {
    fs::read_to_string(&self.log).map(|text| text.lines().map(str::to_owned).collect()).unwrap_or_default()
  }

  fn added_entries(&self) -> Vec<Value> {
    self
      .calls()
      .iter()
      .filter_map(|call| call.strip_prefix("mcp add-json --scope user code-atlas "))
      .map(|entry| serde_json::from_str(entry).unwrap())
      .collect()
  }

  fn write_user_entry(&self, entry: &Value) {
    let config = serde_json::json!({ "numStartups": 3, "mcpServers": { "other": { "command": "x" }, "code-atlas": entry } });
    fs::write(self.config_dir.join(".claude.json"), config.to_string()).unwrap();
  }
}

fn catl_path() -> String {
  fs::canonicalize(env!("CARGO_BIN_EXE_catl")).unwrap().display().to_string()
}

#[test]
fn should_register_the_user_scope_without_a_project_and_with_watch() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let web = root.join("acme-web");
  let claude = FakeClaude::new(&sandbox);
  assert_ok(&claude.connect(&sandbox, &web, &[]));
  let again = claude.connect(&sandbox, &web, &[]);
  assert_ok(&again);
  assert!(stdout(&again).contains(".claude.json#mcpServers.code-atlas: unchanged"), "{}", stdout(&again));
  assert!(!web.join(".mcp.json").exists());
  assert!(web.join("AGENTS.md").exists());
  let calls = claude.calls();
  assert_eq!(calls.iter().filter(|call| call.as_str() == "mcp remove --scope user code-atlas").count(), 1);
  let added = claude.added_entries();
  assert_eq!(added.len(), 1);
  assert_eq!(added[0]["command"], catl_path());
  assert_eq!(added[0]["args"], serde_json::json!(["mcp", "--watch"]));
  assert_eq!(added[0]["env"]["CODE_ATLAS_HOME"], sandbox.home.path().to_str().unwrap());
  assert!(!claude.config_dir.join("settings.json").exists());
}

#[test]
fn should_replace_a_user_scope_entry_that_pins_a_project_only_after_confirmation() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let web = root.join("acme-web");
  let claude = FakeClaude::new(&sandbox);
  claude.write_user_entry(&serde_json::json!({ "type": "stdio", "command": "/old/bin/catl", "args": ["mcp", "--project", "globex"] }));

  let refused = claude.connect(&sandbox, &web, &[]);
  let accepted = claude.connect(&sandbox, &web, &["--yes"]);

  assert_eq!(code(&refused), 2, "{}", stderr(&refused));
  assert!(
    stderr(&refused).contains("replace an entry written by an older catl") && stderr(&refused).contains("--yes"),
    "{}",
    stderr(&refused)
  );
  let shown = stdout(&refused);
  assert!(shown.contains(".claude.json#mcpServers.code-atlas: updated"), "{shown}");
  assert!(shown.contains("\n-   \"command\": \"/old/bin/catl\",\n"), "{shown}");
  assert!(shown.contains("\n-     \"globex\"\n"), "{shown}");
  assert!(shown.contains("\n+     \"--watch\"\n"), "{shown}");
  assert_ok(&accepted);
  let added = claude.added_entries();
  assert_eq!(added.len(), 1);
  assert_eq!(added[0]["args"], serde_json::json!(["mcp", "--watch"]));
}

#[test]
fn should_ask_before_replacing_a_user_scope_entry_in_an_unreadable_claude_config() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let web = root.join("acme-web");
  let claude = FakeClaude::new(&sandbox);
  fs::write(claude.config_dir.join(".claude.json"), "{ not json").unwrap();

  let refused = claude.connect(&sandbox, &web, &[]);
  let accepted = claude.connect(&sandbox, &web, &["--yes"]);

  assert_eq!(code(&refused), 2, "{}", stderr(&refused));
  assert!(stdout(&refused).contains(".claude.json#mcpServers.code-atlas: updated"), "{}", stdout(&refused));
  assert!(
    stderr(&refused).contains("replace an entry that cannot be read (") && stderr(&refused).contains("--yes"),
    "{}",
    stderr(&refused)
  );
  assert_ok(&accepted);
  assert_eq!(claude.added_entries().len(), 1);
}

#[test]
fn should_leave_an_identical_user_scope_entry_unchanged() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let web = root.join("acme-web");
  let claude = FakeClaude::new(&sandbox);
  claude.write_user_entry(&serde_json::json!({
    "type": "stdio",
    "command": catl_path(),
    "args": ["mcp", "--watch"],
    "env": { "CODE_ATLAS_HOME": sandbox.home.path().to_str().unwrap() },
  }));

  let output = claude.connect(&sandbox, &web, &[]);

  assert_ok(&output);
  assert!(stdout(&output).contains(".claude.json#mcpServers.code-atlas: unchanged"), "{}", stdout(&output));
  assert!(claude.calls().is_empty(), "{:?}", claude.calls());
}

#[test]
fn should_write_the_bare_catl_command_when_catl_on_path_is_this_binary() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  let bin = sandbox.work().join("on-path");
  fs::create_dir_all(&bin).unwrap();
  std::os::unix::fs::symlink(catl_path(), bin.join("catl")).unwrap();
  let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());

  let output = sandbox.run_env(&api, &["connect", "claude"], &[("PATH", path.as_str())]);

  assert_ok(&output);
  assert!(!stderr(&output).contains("machine-specific"), "{}", stderr(&output));
  let config: Value = serde_json::from_str(&fs::read_to_string(api.join(".mcp.json")).unwrap()).unwrap();
  assert_eq!(config["mcpServers"]["code-atlas"]["command"], "catl");
  assert_eq!(config["mcpServers"]["code-atlas"]["args"], serde_json::json!(["mcp", "--project", "acme", "--watch"]));
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
  assert!(!sandbox.home.path().join("data/wiring.json").exists());
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
    sandbox
      .catl()
      .args(args)
      .current_dir(&api)
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
  let mut child = sandbox
    .catl()
    .args(["watch"])
    .current_dir(&api)
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
    assert_eq!(server["command"], catl_path());
    let arguments: Vec<&str> = server["args"].as_array().unwrap().iter().map(|argument| argument.as_str().unwrap()).collect();
    Self::spawn(sandbox, cwd, &arguments)
  }

  fn spawn(sandbox: &Sandbox, cwd: &Path, arguments: &[&str]) -> Self {
    let mut child = sandbox
      .catl()
      .args(arguments)
      .current_dir(cwd)
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
fn should_serve_only_atlas_status_outside_every_project() {
  let sandbox = Sandbox::new();
  let mut session = McpSession::spawn(&sandbox, &sandbox.work(), &["mcp", "--watch"]);
  session.handshake();
  let tools = session.request(2, "tools/list", serde_json::json!({}));
  let status = session.request(3, "tools/call", serde_json::json!({"name": "atlas_status", "arguments": {}}));
  let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|tool| tool["name"].as_str().unwrap()).collect();
  assert_eq!(names, ["atlas_status"]);
  let text = status["result"]["content"][0]["text"].as_str().unwrap();
  assert!(
    text.starts_with(&format!(
      "no project is bound: {} is not inside any registered Code Atlas project",
      sandbox.work().display()
    )),
    "{text}"
  );
  assert!(
    text.contains("`catl init -p <project>`") && text.contains("`catl project create <name> --repo <path>`"),
    "{text}"
  );
  drop(session.stdin.take());
  let (code, lines) = session.wait_for_exit();
  assert_eq!(code, Some(0));
  assert_only_json_rpc(&lines);
}

fn listed_tools(session: &mut McpSession) -> Vec<String> {
  let tools = session.request(2, "tools/list", serde_json::json!({}));
  tools["result"]["tools"]
    .as_array()
    .unwrap()
    .iter()
    .map(|tool| tool["name"].as_str().unwrap().to_owned())
    .collect()
}

#[test]
fn should_serve_the_configured_tool_profile_and_let_the_flag_override_it() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  sandbox.write_config("[mcp]\ntools = \"read-only\"\n");

  let mut configured = McpSession::spawn(&sandbox, &api, &["mcp"]);
  configured.handshake();
  let instructions = configured.seen[0].clone();
  assert_eq!(
    listed_tools(&mut configured),
    ["atlas_context", "atlas_impact", "atlas_search", "atlas_status", "atlas_symbol"]
  );
  assert!(!instructions.contains("atlas_edit") && !instructions.contains("atlas_similar"), "{instructions}");
  let hidden = configured.request(3, "tools/call", serde_json::json!({"name": "atlas_edit", "arguments": {"operation": "apply"}}));
  assert_eq!(hidden["result"]["isError"], true, "{hidden}");
  assert_eq!(hidden["result"]["content"][0]["text"], "atlas_edit is not in profile read-only; enable with [mcp] tools");
  drop(configured.stdin.take());
  assert_eq!(configured.wait_for_exit().0, Some(0));

  let mut flagged = McpSession::spawn(&sandbox, &api, &["mcp", "--tools", "atlas_callers,atlas_status"]);
  flagged.handshake();
  assert_eq!(listed_tools(&mut flagged), ["atlas_callers", "atlas_status"]);
  drop(flagged.stdin.take());
  assert_eq!(flagged.wait_for_exit().0, Some(0));

  let doctor = stdout(&sandbox.run(&api, &["doctor"]));
  assert!(
    doctor.contains("ok    mcp tools: profile read-only: atlas_context, atlas_impact, atlas_search, atlas_status, atlas_symbol\n"),
    "{doctor}"
  );

  let unknown = sandbox.run(&api, &["mcp", "--tools", "atlas_read"]);
  assert_eq!(unknown.status.code(), Some(2));
  assert!(
    String::from_utf8_lossy(&unknown.stderr)
      .contains("flag:--tools: mcp.tools: unknown tool \"atlas_read\"; expected the profile core, read-only or full, or a list of tools from: atlas_api, "),
    "{}",
    String::from_utf8_lossy(&unknown.stderr)
  );
}

#[test]
fn should_refuse_to_serve_mcp_when_an_environment_tool_profile_is_invalid_instead_of_serving_every_tool() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  sandbox.write_config("[mcp]\ntools = \"read-only\"\n");
  let typo = [("CODE_ATLAS_MCP_TOOLS", "readonly")];
  let expected = "env:CODE_ATLAS_MCP_TOOLS: mcp.tools: expected \"core\", \"read-only\", \"full\" or an array of tool names, got \"readonly\"";

  let refused = sandbox.run_env(&api, &["mcp"], &typo);
  let doctor = sandbox.run_env(&api, &["doctor"], &typo);

  assert_eq!(refused.status.code(), Some(2));
  assert!(String::from_utf8_lossy(&refused.stderr).contains(expected), "{}", String::from_utf8_lossy(&refused.stderr));
  assert_ne!(doctor.status.code(), Some(0));
  assert!(
    stdout(&doctor).contains(&format!("FAIL  mcp tools: `catl mcp` refuses to start: {expected}\n")),
    "{}",
    stdout(&doctor)
  );
}

#[test]
fn should_fail_mcp_with_a_usage_exit_for_an_unknown_project_or_an_ambiguous_directory() {
  let sandbox = Sandbox::new();
  let shared = sandbox.work().join("shared");
  fs::create_dir_all(&shared).unwrap();
  let shared_arg = shared.display().to_string();
  assert_ok(&sandbox.run(&shared, &["project", "create", "alpha", "--repo", &shared_arg]));
  assert_ok(&sandbox.run(&shared, &["project", "create", "zeta", "--repo", &shared_arg]));

  let unknown = sandbox.run(&sandbox.work(), &["mcp", "--project", "ghost"]);
  let ambiguous = sandbox.run(&shared, &["mcp"]);

  assert_eq!((code(&unknown), stdout(&unknown)), (2, String::new()), "{}", stderr(&unknown));
  assert!(stderr(&unknown).contains("project \"ghost\" not found"), "{}", stderr(&unknown));
  assert_eq!((code(&ambiguous), stdout(&ambiguous)), (2, String::new()), "{}", stderr(&ambiguous));
  assert!(stderr(&ambiguous).contains("  alpha\n  zeta\n"), "{}", stderr(&ambiguous));
}

const SHOP_LIB: &str = "pub fn shipping_fee(weight: u32) -> u32 {\n  weight * 3\n}\n";

fn git(cwd: &Path, args: &[&str]) {
  let output = Command::new("git")
    .args(args)
    .current_dir(cwd)
    .env("GIT_CONFIG_GLOBAL", "/dev/null")
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .env("GIT_AUTHOR_NAME", "atlas")
    .env("GIT_AUTHOR_EMAIL", "atlas@example.invalid")
    .env("GIT_COMMITTER_NAME", "atlas")
    .env("GIT_COMMITTER_EMAIL", "atlas@example.invalid")
    .output()
    .unwrap();
  assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
}

fn shop_with_worktree(sandbox: &Sandbox) -> (PathBuf, PathBuf) {
  let main = sandbox.work().join("shop");
  fs::create_dir_all(main.join("src")).unwrap();
  fs::write(main.join("Cargo.toml"), "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2024\"\n").unwrap();
  fs::write(main.join("src/lib.rs"), SHOP_LIB).unwrap();
  git(&main, &["init", "--quiet", "--initial-branch=trunk"]);
  git(&main, &["add", "."]);
  git(&main, &["commit", "--quiet", "--message", "seed"]);
  git(&main, &["worktree", "add", "--quiet", "-b", "feature", ".claude/worktrees/feature"]);
  assert_ok(&sandbox.run(&main, &["project", "create", "shop", "--repo", main.to_str().unwrap()]));
  (main.clone(), main.join(".claude/worktrees/feature"))
}

#[test]
fn should_refuse_an_edit_from_a_worktree_in_main_mode_with_exit_5_and_write_nothing() {
  let sandbox = Sandbox::new();
  let (main, worktree) = shop_with_worktree(&sandbox);
  sandbox.write_config("[worktrees]\nmode = \"main\"\n");
  let edit = ["edit", "replace", "src/lib.rs", "--pattern", "weight * 3", "--text", "weight * 4"];

  let refused = sandbox.run(&worktree.join("src"), &edit);
  let dry_run = sandbox.run(&worktree, &[&edit[..], &["--dry-run", "--json"]].concat());

  assert_eq!(code(&refused), 5, "{}", stderr(&refused));
  assert_eq!(stdout(&refused), "");
  assert_eq!(
    stderr(&refused),
    format!(
      "error: refusing to edit from the git worktree at {}: project shop indexes the main checkout at {} (repo shop), so the edit would write there, not into this worktree; nothing was written. Edit the worktree's files directly, or run the edit from {}\n",
      worktree.display(),
      main.display(),
      main.display()
    )
  );
  assert_eq!((code(&dry_run), stdout(&dry_run)), (5, String::new()), "{}", stderr(&dry_run));
  assert_eq!(fs::read_to_string(main.join("src/lib.rs")).unwrap(), SHOP_LIB);
  assert_eq!(fs::read_to_string(worktree.join("src/lib.rs")).unwrap(), SHOP_LIB);

  let applied = sandbox.run(&main, &edit);
  assert_ok(&applied);
  assert!(!stderr(&applied).contains("worktree"), "{}", stderr(&applied));
  assert_eq!(fs::read_to_string(main.join("src/lib.rs")).unwrap(), SHOP_LIB.replace("weight * 3", "weight * 4"));
  assert_eq!(fs::read_to_string(worktree.join("src/lib.rs")).unwrap(), SHOP_LIB);
}

#[test]
fn should_refuse_edits_from_a_worktree_of_a_repo_inside_a_registered_parent_folder() {
  let sandbox = Sandbox::new();
  let workspace = sandbox.work().join("workspace");
  let app = workspace.join("app");
  fs::create_dir_all(app.join("src")).unwrap();
  fs::write(app.join("Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n").unwrap();
  fs::write(app.join("src/lib.rs"), SHOP_LIB).unwrap();
  git(&app, &["init", "--quiet", "--initial-branch=trunk"]);
  git(&app, &["add", "."]);
  git(&app, &["commit", "--quiet", "--message", "seed"]);
  git(&app, &["worktree", "add", "--quiet", "-b", "feature", ".worktrees/feature"]);
  let worktree = app.join(".worktrees/feature");
  assert_ok(&sandbox.run(&workspace, &["project", "create", "ws", "--repo", workspace.to_str().unwrap()]));
  let refusal = format!(
    "warning: no worktree overlay: no repo of project ws is registered inside the checkout {}, so the worktree cannot stand in for one; answering from the indexed checkout, and edits are refused here\nerror: refusing to edit from the git worktree at {}: project ws indexes the main checkout at {} (repo workspace), so the edit would write there, not into this worktree; nothing was written. Edit the worktree's files directly, or run the edit from {}\n",
    app.display(),
    worktree.display(),
    app.display(),
    app.display()
  );

  let by_path = sandbox.run(&worktree, &["edit", "replace", "app/src/lib.rs", "--pattern", "weight * 3", "--text", "weight * 4"]);
  let by_symbol = sandbox.run(
    &worktree.join("src"),
    &[
      "edit",
      "replace-body",
      "shipping_fee",
      "--text",
      "pub fn shipping_fee(weight: u32) -> u32 {\n  weight * 4\n}",
    ],
  );
  let query = sandbox.run(&worktree, &["query", "symbol", "shipping_fee"]);

  assert_eq!((code(&by_path), stderr(&by_path)), (5, refusal.clone()));
  assert_eq!((code(&by_symbol), stderr(&by_symbol)), (5, refusal));
  assert_eq!(fs::read_to_string(app.join("src/lib.rs")).unwrap(), SHOP_LIB);
  assert_eq!(fs::read_to_string(worktree.join("src/lib.rs")).unwrap(), SHOP_LIB);
  assert_ok(&query);
  assert!(
    stderr(&query).contains(&format!("\nworktree caveat: answers describe the main checkout at {}", app.display())),
    "{}",
    stderr(&query)
  );
}

#[test]
fn should_refuse_edits_from_a_worktree_whose_git_file_no_longer_resolves() {
  let sandbox = Sandbox::new();
  let (main, worktree) = shop_with_worktree(&sandbox);
  fs::remove_dir_all(main.join(".git/worktrees/feature")).unwrap();

  let refused = sandbox.run(&worktree, &["edit", "replace", "src/lib.rs", "--pattern", "weight * 3", "--text", "weight * 4"]);

  assert_eq!(code(&refused), 5, "{}", stderr(&refused));
  assert_eq!(
    stderr(&refused),
    format!(
      "error: refusing to edit from {}: its .git entry does not resolve to a git checkout, so the worktree could not be resolved and the edit could write into the checkout registered at {} (repo shop of project shop) instead; nothing was written. Repair it with `git worktree repair`, or run the edit from {}\n",
      worktree.display(),
      main.display(),
      main.display()
    )
  );
  assert_eq!(fs::read_to_string(main.join("src/lib.rs")).unwrap(), SHOP_LIB);
}

#[test]
fn should_refuse_edits_in_main_mode_from_a_worktree_whose_commondir_file_is_missing() {
  let sandbox = Sandbox::new();
  let (main, worktree) = shop_with_worktree(&sandbox);
  fs::remove_file(main.join(".git/worktrees/feature/commondir")).unwrap();

  sandbox.write_config("[worktrees]\nmode = \"main\"\n");
  let refused = sandbox.run(&worktree, &["edit", "replace", "src/lib.rs", "--pattern", "weight * 3", "--text", "weight * 4"]);

  assert_eq!(code(&refused), 5, "{}", stderr(&refused));
  assert!(
    stderr(&refused).starts_with(&format!("error: refusing to edit from the git worktree at {}:", worktree.display())),
    "{}",
    stderr(&refused)
  );
  assert_eq!(fs::read_to_string(main.join("src/lib.rs")).unwrap(), SHOP_LIB);
}

#[test]
fn should_connect_claude_from_a_worktree_into_the_worktree_and_leave_the_main_checkout_alone() {
  let sandbox = Sandbox::new();
  let (main, worktree) = shop_with_worktree(&sandbox);

  let output = sandbox.run(&worktree.join("src"), &["connect", "claude"]);

  assert_ok(&output);
  assert!(stderr(&output).contains("worktree caveat:"), "{}", stderr(&output));
  let config: Value = serde_json::from_str(&fs::read_to_string(worktree.join(".mcp.json")).unwrap()).unwrap();
  assert_eq!(config["mcpServers"]["code-atlas"]["args"], serde_json::json!(["mcp", "--project", "shop", "--watch"]));
  assert!(worktree.join("AGENTS.md").exists());
  assert!(!main.join(".mcp.json").exists() && !main.join("AGENTS.md").exists());

  fs::write(worktree.join("CLAUDE.md"), "# rules\n").unwrap();
  sandbox.write_config("[agents]\nguidance = \"local\"\n[agents.claude]\nhooks = [\"auto-approve\", \"remind\"]\n");
  let synced = sandbox.run(&worktree.join("src"), &["agents", "sync", "--agent", "claude", "--scope", "project", "--migrate", "--yes"]);

  assert_ok(&synced);
  let config: Value = serde_json::from_str(&fs::read_to_string(worktree.join(".mcp.json")).unwrap()).unwrap();
  assert_eq!(
    config["mcpServers"]["code-atlas"],
    serde_json::json!({"type": "stdio", "command": "catl", "args": ["mcp", "--watch"]})
  );
  assert!(worktree.join("CLAUDE.local.md").exists() && worktree.join(".claude/settings.local.json").exists());
  assert!(!main.join("CLAUDE.local.md").exists() && !main.join(".claude/settings.local.json").exists());
  assert!(!worktree.join("AGENTS.md").exists(), "the tracked block connect wrote is no longer wanted");
  let exclude = fs::read_to_string(main.join(".git/info/exclude")).unwrap();
  assert!(exclude.ends_with("/.claude/settings.local.json\n/CLAUDE.local.md\n"), "{exclude}");
}

#[test]
fn should_print_the_worktree_caveat_with_answers_given_from_a_worktree_in_main_mode() {
  let sandbox = Sandbox::new();
  let (main, worktree) = shop_with_worktree(&sandbox);
  sandbox.write_config("[worktrees]\nmode = \"main\"\n");
  let caveat = format!(
    "worktree caveat: answers describe the main checkout at {}, not the git worktree at {} (repo shop of project shop); edits are refused here\n",
    main.display(),
    worktree.display()
  );

  let in_worktree = sandbox.run(&worktree, &["query", "symbol", "shipping_fee"]);
  let as_json = sandbox.run(&worktree, &["query", "symbol", "shipping_fee", "--json"]);
  let in_main = sandbox.run(&main, &["query", "symbol", "shipping_fee"]);

  assert_ok(&in_worktree);
  assert!(stdout(&in_worktree).contains("shipping_fee function shop:src/lib.rs"), "{}", stdout(&in_worktree));
  assert_eq!(stderr(&in_worktree), caveat);
  assert_ok(&as_json);
  assert!(json(&as_json)["answer"].to_string().contains("\"shipping_fee\""), "{}", stdout(&as_json));
  assert_eq!(stderr(&as_json), caveat);
  assert_ok(&in_main);
  assert_eq!(stderr(&in_main), "");
  let body = |output: &Output| stdout(output).lines().skip(1).map(str::to_owned).collect::<Vec<_>>();
  assert_eq!(body(&in_main), body(&in_worktree));
}

#[test]
fn should_answer_from_and_edit_the_worktree_through_its_overlay() {
  let sandbox = Sandbox::new();
  let (main, worktree) = shop_with_worktree(&sandbox);
  assert_ok(&sandbox.run(&main, &["index"]));
  let edited = SHOP_LIB.replace("weight * 3", "weight * 4");

  let applied = sandbox.run(&worktree.join("src"), &["edit", "replace", "src/lib.rs", "--pattern", "weight * 3", "--text", "weight * 4"]);
  let in_worktree = sandbox.run(&worktree, &["query", "read", "shipping_fee"]);
  let in_main = sandbox.run(&main, &["query", "read", "shipping_fee"]);
  let status = sandbox.run(&worktree, &["status"]);
  let status_json = sandbox.run(&worktree, &["status", "--json"]);
  let main_status = sandbox.run(&main, &["status"]);
  let listed = sandbox.run(&main, &["project", "list"]);

  assert_ok(&applied);
  assert_eq!(stderr(&applied), "");
  assert_eq!(fs::read_to_string(worktree.join("src/lib.rs")).unwrap(), edited);
  assert_eq!(fs::read_to_string(main.join("src/lib.rs")).unwrap(), SHOP_LIB);
  assert_ok(&in_worktree);
  assert_eq!(stderr(&in_worktree), "");
  assert!(stdout(&in_worktree).contains("weight * 4"), "{}", stdout(&in_worktree));
  assert_ok(&in_main);
  assert!(stdout(&in_main).contains("weight * 3"), "{}", stdout(&in_main));
  assert_ok(&status);
  assert!(
    stdout(&status).contains(&format!(
      "worktree overlay shop@feature of project shop: repo shop indexed from the git worktree at {}; seeded from project shop 0m ago; SCIP facts were copied then",
      worktree.display()
    )),
    "{}",
    stdout(&status)
  );
  assert_eq!(json(&status_json)["overlay"]["label"], "shop@feature");
  assert_eq!(json(&status_json)["overlay"]["seed"]["source"], "base");
  assert!(
    stdout(&main_status).contains(&format!("worktree overlay shop@feature  {}  (ready, last used 0m ago)", worktree.display())),
    "{}",
    stdout(&main_status)
  );
  assert_eq!(
    stdout(&listed),
    format!("shop  shop\n  overlay shop@feature  {}  (ready, last used 0m ago)\n", worktree.display())
  );
}

fn add_billing(sandbox: &Sandbox, main: &Path) -> PathBuf {
  let billing = sandbox.work().join("billing");
  fs::create_dir_all(billing.join("src")).unwrap();
  fs::write(billing.join("Cargo.toml"), "[package]\nname = \"billing\"\nversion = \"0.1.0\"\nedition = \"2024\"\n").unwrap();
  fs::write(billing.join("src/lib.rs"), "pub fn invoice() -> u32 {\n  1\n}\n").unwrap();
  assert_ok(&sandbox.run(main, &["project", "add-repo", "shop", billing.to_str().unwrap(), "--name", "billing"]));
  billing
}

#[test]
fn should_mark_edited_files_outside_the_worktree_and_name_the_base_project_in_hints() {
  let sandbox = Sandbox::new();
  let (main, worktree) = shop_with_worktree(&sandbox);
  let billing = add_billing(&sandbox, &main);
  assert_ok(&sandbox.run(&main, &["index"]));

  let edit = sandbox.run(&worktree, &["edit", "replace", "billing:src/lib.rs", "--pattern", "  1", "--text", "  2"]);
  let inside = sandbox.run(
    &worktree,
    &["edit", "replace", "shop:src/lib.rs", "--pattern", "weight * 3", "--text", "weight * 4", "--json"],
  );
  let status = sandbox.run(&worktree, &["status"]);

  assert_ok(&edit);
  assert!(
    stdout(&edit).contains(&format!("  billing:src/lib.rs  1  (outside the worktree: {})\n", billing.display())),
    "{}",
    stdout(&edit)
  );
  assert_ok(&inside);
  assert_eq!(json(&inside)["answer"]["files"][0]["outside_worktree"], Value::Null);
  assert_ok(&status);
  assert!(
    stdout(&status).contains("scip auto: off (enable with `catl project config shop --scip on`)"),
    "{}",
    stdout(&status)
  );
  assert!(
    stdout(&status).contains("worktree overlay shop@feature of project shop: repo shop indexed from the git worktree"),
    "{}",
    stdout(&status)
  );
}

#[test]
fn should_edit_the_worktree_through_an_overlay_of_a_project_that_was_never_indexed() {
  let sandbox = Sandbox::new();
  let (main, worktree) = shop_with_worktree(&sandbox);
  let base_data = sandbox.home.path().join("data/shop");

  let edit = sandbox.run(&worktree, &["edit", "replace", "src/lib.rs", "--pattern", "weight * 3", "--text", "weight * 4"]);

  assert_ok(&edit);
  assert_eq!(fs::read_to_string(worktree.join("src/lib.rs")).unwrap(), SHOP_LIB.replace("weight * 3", "weight * 4"));
  assert_eq!(fs::read_to_string(main.join("src/lib.rs")).unwrap(), SHOP_LIB);
  let mut created: Vec<String> = fs::read_dir(&base_data)
    .unwrap()
    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
    .collect();
  created.sort();
  assert_eq!(created, ["atlas.db.edit.lock"]);
}

#[test]
fn should_make_overlay_edits_wait_for_the_project_edit_lock() {
  let sandbox = Sandbox::new();
  let (main, worktree) = shop_with_worktree(&sandbox);
  assert_ok(&sandbox.run(&main, &["index"]));
  assert_ok(&sandbox.run(&worktree, &["status"]));
  let lock = fs::File::options()
    .create(true)
    .truncate(false)
    .write(true)
    .open(sandbox.home.path().join("data/shop/atlas.db.edit.lock"))
    .unwrap();
  lock.lock().unwrap();

  let edited = std::thread::scope(|scope| {
    let edit = scope.spawn(|| sandbox.run(&worktree, &["edit", "replace", "src/lib.rs", "--pattern", "weight * 3", "--text", "weight * 4"]));
    std::thread::sleep(std::time::Duration::from_secs(3));
    let while_locked = fs::read_to_string(worktree.join("src/lib.rs")).unwrap();
    lock.unlock().unwrap();
    (while_locked, edit.join().unwrap())
  });

  assert_eq!(edited.0, SHOP_LIB);
  assert_ok(&edited.1);
  assert_eq!(fs::read_to_string(worktree.join("src/lib.rs")).unwrap(), SHOP_LIB.replace("weight * 3", "weight * 4"));
}

#[test]
fn should_never_lose_a_successful_edit_when_the_project_and_an_overlay_edit_a_shared_repo_together() {
  const FILES: usize = 12;
  const ROUNDS: usize = 12;
  let sandbox = Sandbox::new();
  let (main, worktree) = shop_with_worktree(&sandbox);
  let billing = sandbox.work().join("billing");
  fs::create_dir_all(billing.join("src/race")).unwrap();
  fs::write(billing.join("Cargo.toml"), "[package]\nname = \"billing\"\nversion = \"0.1.0\"\nedition = \"2024\"\n").unwrap();
  let modules: String = (0..FILES).map(|index| format!("pub mod f{index};\n")).collect();
  fs::write(billing.join("src/lib.rs"), "pub mod race;\n").unwrap();
  fs::write(billing.join("src/race.rs"), modules).unwrap();
  for index in 0..FILES {
    fs::write(billing.join(format!("src/race/f{index}.rs")), format!("// o=0 b=0\npub fn f{index}() {{}}\n")).unwrap();
  }
  assert_ok(&sandbox.run(&main, &["project", "add-repo", "shop", billing.to_str().unwrap(), "--name", "billing"]));
  assert_ok(&sandbox.run(&main, &["index"]));
  assert_ok(&sandbox.run(&worktree, &["status"]));
  let expected = FILES.to_string();
  let writer = |tag: &str, cwd: &Path| -> Vec<i32> {
    (1..=ROUNDS)
      .map(|round| {
        let pattern = format!("{tag}=([0-9_]+)");
        let text = format!("{tag}=${{1}}_{round}");
        let args = [
          "edit",
          "replace-in-files",
          "--regex",
          "--pattern",
          &pattern,
          "--text",
          &text,
          "--include",
          "billing:src/race/**",
          "--expected-count",
          &expected,
        ];
        code(&sandbox.run(cwd, &args))
      })
      .collect()
  };

  let (overlay_codes, base_codes) = std::thread::scope(|scope| {
    let overlay = scope.spawn(|| writer("o", &worktree));
    let base = scope.spawn(|| writer("b", &main));
    (overlay.join().unwrap(), base.join().unwrap())
  });

  let codes: Vec<i32> = overlay_codes.iter().chain(&base_codes).copied().collect();
  assert!(codes.iter().all(|code| [0, 5, 6].contains(code)), "{overlay_codes:?} {base_codes:?}");
  assert!(codes.contains(&0), "{overlay_codes:?} {base_codes:?}");
  for index in 0..FILES {
    let line = fs::read_to_string(billing.join(format!("src/race/f{index}.rs")))
      .unwrap()
      .lines()
      .next()
      .unwrap()
      .to_owned();
    for (tag, codes) in [("o", &overlay_codes), ("b", &base_codes)] {
      let rounds: Vec<String> = line
        .split(' ')
        .find_map(|part| part.strip_prefix(&format!("{tag}=")))
        .unwrap()
        .split('_')
        .skip(1)
        .map(str::to_owned)
        .collect();
      let lost: Vec<usize> = (1..=ROUNDS).filter(|round| codes[round - 1] == 0 && !rounds.contains(&round.to_string())).collect();
      assert_eq!(lost, Vec::<usize>::new(), "f{index}.rs lost successful {tag} rounds: {line} {codes:?}");
    }
  }
}

#[test]
fn should_refuse_to_serve_a_worktree_when_worktrees_are_off() {
  let sandbox = Sandbox::new();
  let (main, worktree) = shop_with_worktree(&sandbox);
  sandbox.write_config("[worktrees]\nmode = \"off\"\n");

  let query = sandbox.run(&worktree, &["query", "symbol", "shipping_fee"]);
  let edit = sandbox.run(&worktree, &["edit", "replace", "src/lib.rs", "--pattern", "weight * 3", "--text", "weight * 4"]);

  assert_eq!(code(&query), 2, "{}", stderr(&query));
  assert!(
    stderr(&query).starts_with(&format!(
      "error: {} is in the git worktree at {} of project \"shop\", and config.toml sets [worktrees] mode = \"off\", so Code Atlas does not serve it",
      worktree.display(),
      worktree.display()
    )),
    "{}",
    stderr(&query)
  );
  assert_eq!(code(&edit), 2, "{}", stderr(&edit));
  assert_eq!(fs::read_to_string(main.join("src/lib.rs")).unwrap(), SHOP_LIB);
  assert_eq!(fs::read_to_string(worktree.join("src/lib.rs")).unwrap(), SHOP_LIB);
}

fn run_with_path(sandbox: &Sandbox, cwd: &Path, args: &[&str], path: &Path) -> Output {
  sandbox.run_env(cwd, args, &[("PATH", path.to_str().unwrap())])
}

fn script(path: &Path, body: &str) {
  use std::os::unix::fs::PermissionsExt;
  fs::write(path, body).unwrap();
  fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn should_document_the_edit_exit_codes_and_the_one_shot_server_cost_in_help() {
  let sandbox = Sandbox::new();
  let output = sandbox.run(sandbox.work().as_path(), &["--help"]);
  assert_ok(&output);
  let help = stdout(&output);
  for expected in [
    "5  edit refused, nothing written",
    "6  edit failed and was rolled back, nothing changed",
    "7  edit partially applied",
    "never exits 0",
    "start a language server per invocation and stop it on exit",
    "2-5 s on TypeScript fixtures, 20-65 s cold on Rust",
    "130  interrupted (a write in progress finishes first; if that edit is refused, rolled back or partially applied, its 5, 6 or 7 wins)",
    "every refused edit exits 5, never 4",
    "An applied or dry-run edit whose index freshness is degraded exits 3.",
    "edit replace-body|insert-before|insert-after start one only when one is already warm",
  ] {
    assert!(help.contains(expected), "missing {expected:?} in {help}");
  }
  for command in ["find", "outline", "refs", "decl", "impls", "hierarchy", "diagnostics", "edit", "lsp"] {
    assert!(help.contains(&format!("  {command} ")), "missing the {command} command in {help}");
  }
  let edit = stdout(&sandbox.run(sandbox.work().as_path(), &["edit", "--help"]));
  for subcommand in [
    "rename",
    "replace-body",
    "insert-before",
    "insert-after",
    "safe-delete",
    "replace",
    "replace-in-files",
    "apply",
  ] {
    assert!(edit.contains(&format!("  {subcommand} ")), "missing edit {subcommand} in {edit}");
  }
}

#[test]
fn should_default_lsp_settings_to_enabled_with_downloads_and_round_trip_every_flag() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  let defaults = json(&sandbox.run(&api, &["project", "config", "acme", "--json"]));
  assert_eq!(defaults.get("lsp"), None);
  let shown = stdout(&sandbox.run(&api, &["project", "config", "acme"]));
  for line in [
    "lsp.enabled = true",
    "lsp.download = true",
    "lsp.typescript_command = (discover)",
    "lsp.unit = project",
    "lsp.max_servers = 2",
    "lsp.idle_secs = 600",
  ] {
    assert!(shown.contains(line), "missing {line:?} in {shown}");
  }

  let changed = sandbox.run(
    &api,
    &[
      "project",
      "config",
      "acme",
      "--lsp",
      "off",
      "--lsp-download",
      "off",
      "--lsp-ts-command",
      "/usr/bin/node",
      "--lsp-ts-command",
      "/opt/ts/cli.mjs",
      "--lsp-idle-secs",
      "30",
      "--lsp-max-servers",
      "3",
      "--lsp-unit",
      "repo",
      "--lsp-ready-timeout-secs",
      "90",
      "--lsp-check-timeout-secs",
      "45",
      "--lsp-exclude",
      "fixtures/**",
      "--lsp-exclude",
      "vendor/",
    ],
  );
  assert_ok(&changed);
  let text = stdout(&changed);
  for line in [
    "lsp.enabled: true -> false",
    "lsp.download: true -> false",
    "lsp.typescript_command: (discover) -> /usr/bin/node /opt/ts/cli.mjs",
    "lsp.unit: project -> repo",
    "lsp.max_servers: 2 -> 3",
    "lsp.idle_secs: 600 -> 30",
    "lsp.ready_timeout_secs: 300 -> 90",
    "lsp.check_timeout_secs: 120 -> 45",
    "lsp.exclude: (none) -> fixtures/**, vendor/",
  ] {
    assert!(text.contains(line), "missing {line:?} in {text}");
  }
  let persisted = json(&sandbox.run(&api, &["project", "config", "acme", "--json"]));
  assert_eq!(
    persisted["lsp"],
    serde_json::json!({
      "enabled": false,
      "download": false,
      "typescript_command": ["/usr/bin/node", "/opt/ts/cli.mjs"],
      "unit": "repo",
      "max_servers": 3,
      "idle_secs": 30,
      "ready_timeout_secs": 90,
      "check_timeout_secs": 45,
      "exclude": ["fixtures/**", "vendor/"]
    })
  );

  assert_ok(&sandbox.run(&api, &["project", "config", "acme", "--lsp-exclude-clear", "--lsp-ts-command-clear", "--lsp", "on"]));
  let cleared = json(&sandbox.run(&api, &["project", "config", "acme", "--json"]));
  assert_eq!(cleared["lsp"]["exclude"], Value::Null);
  assert_eq!(cleared["lsp"]["typescript_command"], Value::Null);
  assert_eq!(cleared["lsp"]["enabled"], true);
  assert_ok(&sandbox.run(&api, &["project", "config", "acme", "--reset"]));
  assert_eq!(json(&sandbox.run(&api, &["project", "config", "acme", "--json"])).get("lsp"), None);
}

#[test]
fn should_reject_invalid_lsp_settings_and_arguments_with_exit_2() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let api = root.join("acme-api");
  for args in [
    vec!["project", "config", "acme", "--lsp-max-servers", "0"],
    vec!["project", "config", "acme", "--lsp-idle-secs", "0"],
    vec!["project", "config", "acme", "--lsp-ready-timeout-secs", "0"],
    vec!["project", "config", "acme", "--lsp-check-timeout-secs", "0"],
    vec!["project", "config", "acme", "--lsp-ts-command", "node"],
    vec!["project", "config", "acme", "--lsp-exclude", "[z-a]"],
    vec!["project", "config", "acme", "--lsp-exclude", "["],
    vec!["project", "config", "acme", "--lsp-exclude", "/abs/**"],
    vec!["project", "config", "acme", "--lsp-exclude", "src/{a,b"],
    vec!["project", "config", "acme", "--lsp-unit", "galaxy"],
    vec!["project", "config", "acme", "--lsp-exclude", "a", "--lsp-exclude-clear"],
    vec!["edit", "rename", "toCents", "cents", "--plan-out", "plan.json"],
    vec!["edit", "replace-body", "toCents"],
    vec!["edit", "replace-body", "toCents", "--text", "x", "--text-file", "-"],
    vec!["edit", "replace", "src/a.ts", "--pattern", "x"],
    vec!["edit", "rename", "toCents", "cents", "--cross-repo", "everywhere"],
    vec!["edit", "apply"],
    vec!["decl"],
    vec!["decl", "--path", "src/a.ts"],
    vec!["decl", "src/a.ts:1:1", "--path", "src/a.ts", "--regex", "x"],
    vec!["outline", "src/a.ts", "--source", "magic"],
    vec!["diagnostics", "src/a.ts", "--min-severity", "loud"],
    vec!["query", "similar", "--code", "-"],
    vec!["query", "similar", "--language", "rust"],
    vec!["query", "similar", "toCents", "--code", "-", "--language", "rust"],
    vec!["query", "similar", "--code", "-", "--language", "python"],
    vec!["query", "dependents", "toCents", "--via-http"],
  ] {
    let output = sandbox.run(&api, &args);
    assert_eq!(code(&output), 2, "{args:?}: {}", stderr(&output));
  }
  let output = sandbox.run(&api, &["project", "config", "acme", "--lsp-ts-command", "node"]);
  assert!(stderr(&output).contains("lsp.typescript_command must start with an absolute path"), "{}", stderr(&output));
  let output = sandbox.run(&api, &["project", "config", "acme", "--lsp-max-servers", "0"]);
  assert!(stderr(&output).contains("lsp.max_servers must be at least 1"), "{}", stderr(&output));
  assert_eq!(json(&sandbox.run(&api, &["project", "config", "acme", "--json"])).get("lsp"), None);
}

#[test]
fn should_exit_1_with_the_install_hint_when_the_typescript_server_is_unavailable() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let sdk = root.join("acme-sdk");
  assert_ok(&sandbox.run(&sdk, &["index"]));
  assert_ok(&sandbox.run(&sdk, &["project", "config", "acme", "--lsp-download", "off"]));
  let empty = sandbox.work().join("empty-bin");
  fs::create_dir_all(&empty).unwrap();

  let unavailable = run_with_path(&sandbox, &sdk, &["outline", "src/money.ts", "--source", "lsp"], &empty);

  assert_eq!(code(&unavailable), 1, "{}", stderr(&unavailable));
  let message = stderr(&unavailable);
  assert!(
    message.contains("typescript-language-server not found. Install it (npm i -g typescript-language-server typescript@5)"),
    "{message}"
  );
  assert!(message.contains("--lsp-ts-command"), "{message}");
  assert!(
    message.contains("hint: pass --source graph for tagged graph results that need no language server"),
    "{message}"
  );
  assert_eq!(stdout(&unavailable), "");

  let graph = run_with_path(&sandbox, &sdk, &["outline", "src/money.ts"], &empty);
  assert_ok(&graph);
  assert!(stdout(&graph).contains("source=graph (from the index, syntactic;"), "{}", stdout(&graph));
  assert!(stdout(&graph).contains("toCents"), "{}", stdout(&graph));

  let edit = run_with_path(&sandbox, &sdk, &["edit", "rename", "toCents", "cents"], &empty);
  assert_eq!(code(&edit), 1, "{}", stderr(&edit));
  assert!(stderr(&edit).contains("typescript-language-server not found"), "{}", stderr(&edit));
  assert_eq!(
    fs::read_to_string(sdk.join("src/money.ts")).unwrap(),
    fs::read_to_string(fixtures_dir().join("acme-sdk/src/money.ts")).unwrap()
  );

  assert_ok(&sandbox.run(&sdk, &["project", "config", "acme", "--lsp", "off"]));
  let disabled = sandbox.run(&sdk, &["refs", "toCents"]);
  assert_eq!(code(&disabled), 1);
  assert!(
    stderr(&disabled).contains("language servers are disabled for project acme (lsp.enabled = false); enable them with `catl project config acme --lsp on`"),
    "{}",
    stderr(&disabled)
  );
}

fn fixtures_dir() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/acme")
}

#[test]
fn should_print_every_discovery_step_and_the_rust_analyzer_locator_in_doctor_without_the_network() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let sdk = root.join("acme-sdk");
  assert_ok(&sandbox.run(&sdk, &["index"]));
  assert_ok(&sandbox.run(&sdk, &["project", "config", "acme", "--lsp-download", "on"]));
  let bin = sandbox.work().join("fake-bin");
  fs::create_dir_all(&bin).unwrap();
  script(&bin.join("typescript-language-server"), "#!/bin/sh\necho 5.1.3\n");
  script(&bin.join("rust-analyzer"), "#!/bin/sh\necho 'rust-analyzer 1.2.3 (fake)'\n");
  let web = root.join("acme-web");
  fs::create_dir_all(web.join("node_modules/typescript")).unwrap();
  fs::write(web.join("node_modules/typescript/package.json"), "{\"version\": \"7.0.1\"}\n").unwrap();

  let output = run_with_path(&sandbox, &sdk, &["doctor"], &bin);

  assert_eq!(code(&output), 3, "{}", stdout(&output));
  let report = stdout(&output);
  for expected in [
    "ok    rust-analyzer: ",
    "fake-bin/rust-analyzer (rust-analyzer 1.2.3 (fake))",
    "ok    typescript-language-server: ",
    "lsp.typescript_command: skipped (lsp.typescript_command is not set)",
    "typescript-language-server on PATH: found (",
    "(version 5.1.3)",
    "acme-sdk/node_modules/.bin/typescript-language-server: skipped (an earlier candidate was found)",
    "npx cache (npx --no-install typescript-language-server): skipped (an earlier candidate was found)",
    "download (typescript-language-server@5, typescript@5): skipped by doctor (no network); lsp.download = on",
    "warn  acme/acme-web typescript: node_modules holds TypeScript 7.0.1, which has no tsserver",
  ] {
    assert!(report.contains(expected), "missing {expected:?} in {report}");
  }
}

#[test]
fn should_say_when_rust_analyzer_and_typescript_server_are_missing_in_doctor() {
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let sdk = root.join("acme-sdk");
  assert_ok(&sandbox.run(&sdk, &["project", "config", "acme", "--lsp-download", "off"]));
  let empty = sandbox.work().join("empty-bin");
  fs::create_dir_all(&empty).unwrap();

  let output = run_with_path(&sandbox, &sdk, &["doctor"], &empty);

  let report = stdout(&output);
  assert_eq!(code(&output), 3, "{report}");
  assert!(report.contains("warn  rust-analyzer: "), "{report}");
  assert!(
    report.contains("exact Rust operations and Rust SCIP indexing are unavailable (`rustup component add rust-analyzer`)"),
    "{report}"
  );
  assert!(
    report.contains("warn  typescript-language-server: typescript-language-server not found. Install it"),
    "{report}"
  );
  assert!(
    report.contains("typescript-language-server on PATH: missing (typescript-language-server is not on PATH)"),
    "{report}"
  );
  assert!(
    report.contains("download (typescript-language-server@5, typescript@5): skipped (download is off (lsp.download = false))"),
    "{report}"
  );
}

#[test]
fn should_find_code_similar_to_a_draft_on_stdin_and_follow_http_hops() {
  use std::io::Write;
  let sandbox = Sandbox::new();
  let root = sandbox.create_acme();
  let sdk = root.join("acme-sdk");
  assert_ok(&sandbox.run(&sdk, &["index"]));
  let draft = fs::read_to_string(sdk.join("src/money.ts")).unwrap();
  let mut command = sandbox.catl();
  command
    .args(["query", "similar", "--code", "-", "--language", "ts", "--json"])
    .current_dir(&sdk)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  let mut child = command.spawn().unwrap();
  child.stdin.take().unwrap().write_all(draft.as_bytes()).unwrap();

  let output = child.wait_with_output().unwrap();

  assert_ok(&output);
  let answer = json(&output);
  assert_eq!(answer["answer"]["language"], "typescript");
  assert_eq!(answer["answer"]["drafts"][0]["draft"]["name"], "toCents");

  let plain = sandbox.run(&sdk, &["callers", "toCents", "--json"]);
  let http = sandbox.run(&sdk, &["callees", "toCents", "--via-http", "--json"]);
  assert!([0, 3].contains(&code(&plain)), "{}", stderr(&plain));
  assert!([0, 3].contains(&code(&http)), "{}", stderr(&http));
  assert_eq!(json(&http)["answer"]["via_http"], true);
  assert_eq!(json(&plain)["answer"].get("via_http"), None);
}

fn solo_repo(sandbox: &Sandbox, name: &str) -> PathBuf {
  let repo = sandbox.work().join(name);
  fs::create_dir_all(repo.join("src")).unwrap();
  fs::create_dir_all(repo.join("legacy")).unwrap();
  fs::write(repo.join("src/lib.rs"), "pub fn answer() -> u32 {\n  42\n}\n").unwrap();
  fs::write(repo.join("legacy/old.rs"), "pub fn ancient() -> u32 {\n  7\n}\n").unwrap();
  fs::write(repo.join("Cargo.toml"), format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n")).unwrap();
  repo
}

fn write_user_config(sandbox: &Sandbox, text: &str) {
  let config = sandbox.home.path().join("config");
  fs::create_dir_all(&config).unwrap();
  fs::write(config.join("config.toml"), text).unwrap();
}

#[test]
fn should_show_each_set_key_with_the_layer_it_came_from() {
  let sandbox = Sandbox::new();
  let app = solo_repo(&sandbox, "app");
  assert_ok(&sandbox.run(&app, &["project", "create", "acme", "--repo", app.to_str().unwrap()]));
  assert_ok(&sandbox.run(&app, &["project", "config", "acme", "--scip-debounce", "10"]));
  write_user_config(
    &sandbox,
    "version = 1\n\n[defaults]\nexclude = [\"**/*.gen.ts\"]\n\n[defaults.lsp]\nmax_servers = 3\n\n[mcp]\nmax_body_lines = 200\n\n[worktrees]\nmode = \"main\"\n",
  );
  fs::write(
    app.join(".code-atlas.toml"),
    "project = \"acme\"\n\n[index]\nexclude = [\"legacy/**\"]\n\n[lsp]\ntypescript_command = [\"/bin/sh\"]\n",
  )
  .unwrap();
  let envs = [("CODE_ATLAS_LSP_ENABLED", "false"), ("CODE_ATLAS_MAX_FILE_BYTES", "5")];

  let shown = sandbox.run_env(&app, &["config", "show", "--origin"], &envs);

  assert_ok(&shown);
  assert_eq!(
    stdout(&shown),
    format!(
      "projects.toml                       scip.debounce_secs = 10\n\
       env:CODE_ATLAS_LSP_ENABLED          lsp.enabled = false\n\
       config.toml                         lsp.max_servers = 3\n\
       projects.toml                       repo.app.path = \"{}\"\n\
       config.toml + app/.code-atlas.toml  repo.app.exclude = [\"**/*.gen.ts\", \"legacy/**\"]\n\
       config.toml                         mcp.max_body_lines = 200\n\
       config.toml                         worktrees.mode = \"main\"\n",
      app.display()
    )
  );
  assert_eq!(
    stderr(&shown),
    "warning: env:CODE_ATLAS_MAX_FILE_BYTES: max_file_bytes ignored: it changes indexed facts, so only config.toml, projects.toml or a repository's .code-atlas.toml may set it\n\
     warning: app/.code-atlas.toml: lsp.typescript_command ignored: a repository file may not name an executable\n"
  );
  let plain = stdout(&sandbox.run_env(&app, &["config", "show"], &envs));
  assert!(plain.starts_with("scip.debounce_secs = 10\nlsp.enabled = false\n"), "{plain}");
  let effective = stdout(&sandbox.run_env(&app, &["config", "show", "--effective", "--origin"], &envs));
  assert!(effective.contains("built-in                            max_file_bytes = 1048576\n"), "{effective}");
  assert!(effective.contains("built-in                            mcp.tools = \"full\"\n"), "{effective}");
  let listed = json(&sandbox.run_env(&app, &["config", "show", "--json"], &envs));
  assert_eq!(listed["project"], "acme");
  assert_eq!(listed["entries"][1]["origins"][0], serde_json::json!({ "layer": "env", "var": "CODE_ATLAS_LSP_ENABLED" }));
}

#[test]
fn should_show_only_global_keys_outside_every_project() {
  let sandbox = Sandbox::new();
  let elsewhere = sandbox.work();
  write_user_config(&sandbox, "[defaults]\nexclude = [\"tmp/\"]\n[defaults.scip]\nenabled = true\n");

  let shown = sandbox.run(&elsewhere, &["config", "show", "--origin"]);

  assert_ok(&shown);
  assert_eq!(stdout(&shown), "config.toml  scip.enabled = true\nconfig.toml  defaults.exclude = [\"tmp/\"]\n");
  write_user_config(&sandbox, "[defaults]\nbogus = 1\n");
  let newer = sandbox.run(&elsewhere, &["config", "show"]);
  assert_ok(&newer);
  assert_eq!(
    stderr(&newer),
    "warning: config.toml: unknown key defaults.bogus ignored\nevery key has its built-in value; pass --effective to list them\n"
  );
  write_user_config(&sandbox, "[defaults.lsp]\nmax_servers = 0\n");
  let broken = sandbox.run(&elsewhere, &["config", "show"]);
  assert_eq!(code(&broken), 2, "{}", stderr(&broken));
  assert!(stderr(&broken).contains("config.toml: lsp.max_servers must be at least 1"), "{}", stderr(&broken));
}

#[test]
fn should_index_with_repo_layer_excludes_and_ignore_fact_keys_from_the_environment() {
  let sandbox = Sandbox::new();
  let repo = solo_repo(&sandbox, "solo");
  assert_ok(&sandbox.run(&repo, &["init"]));
  assert_ok(&sandbox.run(&repo, &["index"]));
  assert_ok(&sandbox.run(&repo, &["symbol", "ancient"]));

  let marker = fs::read_to_string(repo.join(".code-atlas.toml")).unwrap();
  fs::write(repo.join(".code-atlas.toml"), format!("{marker}\n[index]\nexclude = [\"legacy/**\"]\n")).unwrap();
  let envs = [("CODE_ATLAS_MAX_FILE_BYTES", "1")];

  let excluded = sandbox.run_env(&repo, &["symbol", "ancient"], &envs);
  let kept = sandbox.run_env(&repo, &["symbol", "answer"], &envs);

  assert_eq!(code(&excluded), 4, "{}\n{}", stdout(&excluded), stderr(&excluded));
  assert!(
    stderr(&excluded).contains("warning: env:CODE_ATLAS_MAX_FILE_BYTES: max_file_bytes ignored"),
    "{}",
    stderr(&excluded)
  );
  assert_ok(&kept);
  assert!(stdout(&kept).contains("answer function solo:src/lib.rs"), "{}", stdout(&kept));
}

#[test]
fn should_pin_the_project_in_init_without_dropping_other_marker_keys() {
  let sandbox = Sandbox::new();
  let repo = solo_repo(&sandbox, "solo");
  fs::write(repo.join(".code-atlas.toml"), "# shared with the team\n[index]\nexclude = [\"legacy/**\"]\n").unwrap();

  let output = sandbox.run(&repo, &["init"]);

  assert_ok(&output);
  assert_eq!(
    fs::read_to_string(repo.join(".code-atlas.toml")).unwrap(),
    "project = \"solo\"\n# shared with the team\n[index]\nexclude = [\"legacy/**\"]\n"
  );
}

#[test]
fn should_keep_an_explicit_project_value_over_a_global_default() {
  let sandbox = Sandbox::new();
  let repo = solo_repo(&sandbox, "solo");
  assert_ok(&sandbox.run(&repo, &["init"]));
  write_user_config(&sandbox, "[defaults.scip]\nenabled = true\n");
  assert_eq!(json(&sandbox.run(&repo, &["project", "config", "solo", "--json"]))["scip"]["enabled"], true);

  let changed = sandbox.run(&repo, &["project", "config", "solo", "--scip", "off"]);
  let overridden = sandbox.run_env(&repo, &["project", "config", "solo", "--scip", "off"], &[("CODE_ATLAS_SCIP_ENABLED", "on")]);

  assert_eq!(stdout(&changed), "scip.enabled: true -> false\n");
  let registry = fs::read_to_string(sandbox.home.path().join("config/projects.toml")).unwrap();
  assert!(registry.contains("pinned = [\"scip.enabled\"]"), "{registry}");
  assert_eq!(json(&sandbox.run(&repo, &["project", "config", "solo", "--json"]))["scip"]["enabled"], false);
  assert_eq!(stdout(&overridden), "no changes\n");
  assert!(
    stderr(&overridden).contains("warning: scip.enabled is saved in projects.toml, but env:CODE_ATLAS_SCIP_ENABLED overrides it"),
    "{}",
    stderr(&overridden)
  );
}

#[test]
fn should_trace_the_project_resolution_and_flag_a_pinned_user_scope_server_in_doctor() {
  let sandbox = Sandbox::new();
  let repo = solo_repo(&sandbox, "solo");
  assert_ok(&sandbox.run(&repo, &["init"]));
  let claude = sandbox.home.path().join("claude");
  fs::create_dir_all(&claude).unwrap();
  let entry = serde_json::json!({ "mcpServers": { "code-atlas": { "command": "catl", "args": ["mcp", "--project", "globex"] } } });
  fs::write(claude.join(".claude.json"), entry.to_string()).unwrap();

  let report = stdout(&sandbox.run(&repo.join("src"), &["doctor"]));

  for expected in [
    "ok    config: ".to_owned(),
    "config.toml absent; built-in defaults apply".to_owned(),
    format!("      working directory {}\n", repo.join("src").display()),
    format!("      {} pins project solo\n", repo.join(".code-atlas.toml").display()),
    "      resolved project solo\n".to_owned(),
    "warn  claude user scope: the user-scope code-atlas server in ".to_owned(),
    "pins --project globex, so every repo without its own .mcp.json talks to globex".to_owned(),
  ] {
    assert!(report.contains(&expected), "missing {expected:?} in {report}");
  }
}

const ACME_FAMILY: &str = "version = 1\npackages = [\"@acme/http\"]\n\n[controller]\ndecorators = [\"Api\"]\n\n[routes.methods]\nRead = \"GET\"\nWrite = \"POST\"\n";
const ACME_CONTROLLER: &str = "import { Api, Read, Write } from '@acme/http';\n\n@Api('orders')\nexport class OrdersController {\n  @Read()\n  list() {\n    return [];\n  }\n\n  @Write()\n  create() {\n    return null;\n  }\n}\n";

#[test]
fn should_index_a_declared_route_family_and_report_each_enabled_family_in_status() {
  let sandbox = Sandbox::new();
  let repo = sandbox.work().join("orders");
  fs::create_dir_all(repo.join("src")).unwrap();
  fs::write(repo.join("package.json"), "{ \"name\": \"orders\", \"version\": \"1.0.0\" }\n").unwrap();
  fs::write(repo.join("src/orders.controller.ts"), ACME_CONTROLLER).unwrap();
  let detectors = sandbox.home.path().join("config/detectors");
  fs::create_dir_all(&detectors).unwrap();
  fs::write(detectors.join("acme-http.toml"), ACME_FAMILY).unwrap();
  assert_ok(&sandbox.run(&repo, &["init"]));
  let marker = fs::read_to_string(repo.join(".code-atlas.toml")).unwrap();
  fs::write(
    repo.join(".code-atlas.toml"),
    format!("{marker}\n[detect]\nroute_families = [\"acme-http\", \"acme-rpc\"]\n"),
  )
  .unwrap();
  let undeclared = sandbox.home.path().join("config/detectors/acme-rpc.toml");

  let status = sandbox.run(&repo, &["status"]);
  let api = sandbox.run(&repo, &["api"]);

  assert_ok(&status);
  assert!(stdout(&status).contains("route family acme-http: 1 controller, 2 routes\n"), "{}", stdout(&status));
  assert!(
    stdout(&status).contains(&format!(
      "warning: route family acme-rpc is enabled but not declared: {} does not exist\n",
      undeclared.display()
    )),
    "{}",
    stdout(&status)
  );
  assert!(stderr(&status).contains("warning: route family acme-rpc ignored: "), "{}", stderr(&status));
  assert_ok(&api);
  assert!(stdout(&api).contains("GET /orders") && stdout(&api).contains("POST /orders"), "{}", stdout(&api));
  let listed = json(&sandbox.run(&repo, &["status", "--json"]));
  assert_eq!(
    listed["answer"]["route_families"][0],
    serde_json::json!({ "name": "acme-http", "state": "matched", "controllers": 1, "routes": 2 })
  );

  fs::write(detectors.join("acme-http.toml"), "packages = 1\n").unwrap();
  let broken = sandbox.run(&repo, &["status"]);

  assert_eq!(code(&broken), 2, "{}", stderr(&broken));
  assert!(
    stderr(&broken).contains("acme-http.toml: packages: expected an array of strings, got 1"),
    "{}",
    stderr(&broken)
  );
}

#[test]
fn should_not_echo_the_source_line_of_a_malformed_config_or_registry_file() {
  const FAKE_KEY: &str = "sk-ant-api03-FAKEKEYFAKEKEYFAKEKEY";
  let sandbox = Sandbox::new();
  let work = sandbox.work();
  let config = sandbox.home.path().join("config");
  fs::create_dir_all(&config).unwrap();
  for (file, text, expected) in [
    ("config.toml", format!("[defaults]\n\nsemantic_api_key_env = {FAKE_KEY}\n"), "config.toml"),
    ("projects.toml", format!("[[project]]\nname = \"acme\"\napi_key_env = {FAKE_KEY}\n"), "projects.toml"),
  ] {
    let path = config.join(file);
    fs::write(&path, text).unwrap();

    let output = sandbox.run(&work, &["status"]);

    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(!combined.contains(FAKE_KEY), "{file}: {combined}");
    assert!(!combined.contains("FAKEKEY"), "{file}: {combined}");
    assert!(combined.contains(expected) && combined.contains("line 3, column"), "{file}: {combined}");
    fs::remove_file(&path).unwrap();
  }
}
