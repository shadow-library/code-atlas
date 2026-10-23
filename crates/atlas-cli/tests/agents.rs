use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use atlas_test_lsp::hermetic::hermetic_catl;
use serde_json::{Value, json};
use tempfile::TempDir;

const CLAUDE_STUB: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_CLAUDE_LOG"
config="$CLAUDE_CONFIG_DIR/.claude.json"
[ -n "$FAKE_CLAUDE_NOOP" ] && exit 0
if [ "$2" = add-json ] && [ -n "$FAKE_CLAUDE_FAIL_ADD" ]; then
  case "$6" in *"$FAKE_CLAUDE_FAIL_ADD"*) echo "add-json failed" >&2; exit 1;; esac
fi
case "$2:$4" in
  add-json:user) printf '{"mcpServers":{"%s":%s}}' "$5" "$6" > "$config";;
  add-json:local) printf '{"projects":{"%s":{"mcpServers":{"%s":%s}}}}' "$(pwd -P)" "$5" "$6" > "$config";;
  remove:*) printf '{}' > "$config";;
esac
exit 0
"#;

fn read_json(path: &Path) -> Value {
  serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

struct Sandbox {
  root: TempDir,
}

impl Sandbox {
  fn new() -> Self {
    let sandbox = Self {
      root: tempfile::tempdir().unwrap(),
    };
    for directory in ["atlas/fake-bin", "atlas/claude", "atlas/codex", "work"] {
      fs::create_dir_all(sandbox.path(directory)).unwrap();
    }
    let stub = sandbox.path("atlas/fake-bin/claude");
    fs::write(&stub, CLAUDE_STUB).unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    sandbox
  }

  fn path(&self, relative: &str) -> PathBuf {
    fs::canonicalize(self.root.path()).unwrap().join(relative)
  }

  fn command(&self, cwd: &Path, args: &[&str]) -> Command {
    let mut command = hermetic_catl(env!("CARGO_BIN_EXE_catl"), &self.path("atlas"));
    command.args(args).current_dir(cwd).env("FAKE_CLAUDE_LOG", self.path("claude.log"));
    command
  }

  fn run(&self, cwd: &Path, args: &[&str]) -> Output {
    self.run_with(cwd, args, &[])
  }

  fn run_with(&self, cwd: &Path, args: &[&str], envs: &[(&str, &Path)]) -> Output {
    self.command(cwd, args).envs(envs.iter().copied()).stdin(Stdio::null()).output().unwrap()
  }

  fn hook(&self, cwd: &Path, hook: &str, input: &str) -> Output {
    let mut child = self
      .command(cwd, &["hook", hook])
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .spawn()
      .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
  }

  fn config(&self, text: &str) {
    fs::create_dir_all(self.path("atlas/config")).unwrap();
    fs::write(self.path("atlas/config/config.toml"), text).unwrap();
  }

  fn repo(&self, name: &str) -> PathBuf {
    let repo = self.path("work").join(name);
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("src/lib.rs"), "pub fn answer() -> u8 {\n  42\n}\n").unwrap();
    assert_ok(&self.run(&repo, &["project", "create", name, "--repo", repo.to_str().unwrap()]));
    repo
  }

  fn git_repo(&self, name: &str) -> PathBuf {
    let repo = self.repo(name);
    for args in [
      &["init", "--quiet", "--initial-branch=trunk"][..],
      &["add", "."],
      &["commit", "--quiet", "--message", "seed"],
    ] {
      let status = Command::new("git")
        .args(args)
        .current_dir(&repo)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap();
      assert!(status.success());
    }
    repo
  }

  fn claude_calls(&self) -> Vec<String> {
    fs::read_to_string(self.path("claude.log"))
      .map(|text| text.lines().map(str::to_owned).collect())
      .unwrap_or_default()
  }

  fn json(&self, relative: &str) -> Value {
    read_json(&self.path(relative))
  }

  fn wiring(&self) -> Vec<Value> {
    self.json("atlas/data/wiring.json")["entries"].as_array().unwrap().clone()
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

fn git_status(repo: &Path) -> String {
  let output = Command::new("git")
    .args(["status", "--porcelain", "--untracked-files=all"])
    .current_dir(repo)
    .output()
    .unwrap();
  String::from_utf8_lossy(&output.stdout).into_owned()
}

const CODEX_CONFIG: &str = "# my codex settings\nmodel = \"o4\"\n\n[mcp_servers.other]\ncommand = \"x\" # keep me\n";
const FOREIGN_HOOK: &str = r#"{
  "model": "opus",
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash", "hooks": [{ "type": "command", "command": "lint-bash" }] }
    ]
  }
}
"#;

#[test]
fn should_sync_the_user_scope_and_codex_then_track_drift_and_unsync_exactly_what_it_wrote() {
  let sandbox = Sandbox::new();
  let work = sandbox.path("work");
  sandbox.config("[agents.claude]\nhooks = [\"auto-approve\", \"remind\"]\n");
  fs::write(sandbox.path("atlas/codex/config.toml"), CODEX_CONFIG).unwrap();
  fs::write(sandbox.path("atlas/claude/settings.json"), FOREIGN_HOOK).unwrap();

  let unconfirmed = sandbox.run(&work, &["agents", "sync"]);
  assert_eq!(code(&unconfirmed), 2, "{}", stderr(&unconfirmed));
  assert!(
    stderr(&unconfirmed).contains("change a file that already exists") && stderr(&unconfirmed).contains("--yes"),
    "{}",
    stderr(&unconfirmed)
  );
  assert_eq!(fs::read_to_string(sandbox.path("atlas/codex/config.toml")).unwrap(), CODEX_CONFIG);
  assert!(sandbox.claude_calls().is_empty());

  assert_ok(&sandbox.run(&work, &["agents", "sync", "--yes"]));
  let registered = sandbox.json("atlas/claude/.claude.json");
  assert_eq!(
    registered["mcpServers"]["code-atlas"],
    json!({"type": "stdio", "command": "catl", "args": ["mcp", "--watch"], "env": {"CODE_ATLAS_HOME": sandbox.path("atlas").to_str().unwrap()}})
  );
  let settings = sandbox.json("atlas/claude/settings.json");
  assert_eq!(settings["model"], "opus");
  let remind = format!("CODE_ATLAS_HOME={} catl hook remind || true", sandbox.path("atlas").display());
  assert_eq!(
    settings["hooks"]["PreToolUse"],
    json!([
      {"matcher": "Bash", "hooks": [{"type": "command", "command": "lint-bash"}]},
      {"matcher": "Grep|Glob|Bash", "hooks": [{"type": "command", "command": remind, "timeout": 10}]}
    ])
  );
  let allowed: Vec<String> = atlas_mcp::guidance::READ_ONLY_TOOLS.iter().map(|tool| format!("mcp__code-atlas__{tool}")).collect();
  assert_eq!(settings["permissions"]["allow"], json!(allowed));
  let codex = fs::read_to_string(sandbox.path("atlas/codex/config.toml")).unwrap();
  assert_eq!(
    codex,
    format!(
      "{CODEX_CONFIG}\n[mcp_servers.code-atlas]\ncommand = \"catl\"\nargs = [\"mcp\", \"--watch\"]\nenv = {{ CODE_ATLAS_HOME = \"{}\" }}\n",
      sandbox.path("atlas").display()
    )
  );
  let kinds: Vec<String> = sandbox
    .wiring()
    .iter()
    .map(|entry| {
      format!(
        "{} {} {}",
        entry["agent"].as_str().unwrap(),
        entry["scope"].as_str().unwrap(),
        entry["target"]["kind"].as_str().unwrap()
      )
    })
    .collect();
  assert_eq!(kinds, ["claude user registration", "claude user permissions", "claude user hooks", "codex user codex"]);
  assert!(sandbox.wiring().iter().all(|entry| entry["hash"].as_str().unwrap().starts_with("blake3:")));

  let calls_before = sandbox.claude_calls().len();
  let again = sandbox.run(&work, &["agents", "sync"]);
  assert_ok(&again);
  assert_eq!(stdout(&again).matches(": unchanged").count(), 4, "{}", stdout(&again));
  assert_eq!(sandbox.claude_calls().len(), calls_before);
  let status = sandbox.run(&work, &["agents", "status"]);
  assert_ok(&status);
  assert_eq!(stdout(&status).matches(": ok\n").count(), 4, "{}", stdout(&status));

  let edited = codex.replace("args = [\"mcp\", \"--watch\"]", "args = [\"mcp\"]");
  fs::write(sandbox.path("atlas/codex/config.toml"), &edited).unwrap();
  let drift = sandbox.run(&work, &["agents", "status", "--json"]);
  assert_eq!(code(&drift), 3);
  let states: Vec<Value> = serde_json::from_str::<Value>(&stdout(&drift)).unwrap()["entries"]
    .as_array()
    .unwrap()
    .iter()
    .map(|entry| entry["state"].clone())
    .collect();
  assert_eq!(states, [json!("ok"), json!("ok"), json!("ok"), json!("changed")]);
  let refused = sandbox.run(&work, &["agents", "sync", "--yes"]);
  assert_eq!(code(&refused), 2, "{}", stderr(&refused));
  assert!(
    stdout(&refused).contains("refused: was changed since the last sync; pass --force to overwrite it"),
    "{}",
    stdout(&refused)
  );
  assert_eq!(fs::read_to_string(sandbox.path("atlas/codex/config.toml")).unwrap(), edited);
  assert_eq!(code(&sandbox.run(&work, &["agents", "unsync", "--yes"])), 2);
  assert_ok(&sandbox.run(&work, &["agents", "sync", "--force", "--yes"]));
  assert_eq!(fs::read_to_string(sandbox.path("atlas/codex/config.toml")).unwrap(), codex);

  assert_ok(&sandbox.run(&work, &["agents", "unsync", "--yes"]));
  assert_eq!(fs::read_to_string(sandbox.path("atlas/codex/config.toml")).unwrap(), CODEX_CONFIG);
  assert_eq!(
    sandbox.json("atlas/claude/settings.json"),
    json!({"model": "opus", "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "lint-bash"}]}]}})
  );
  assert!(sandbox.claude_calls().contains(&"mcp remove --scope user code-atlas".to_owned()));
  assert!(sandbox.wiring().is_empty());
}

#[test]
fn should_wire_local_scope_with_guidance_and_hooks_git_never_sees_and_remove_them_again() {
  let sandbox = Sandbox::new();
  let repo = sandbox.git_repo("solo");
  sandbox.config("[mcp]\ntools = \"core\"\n[agents]\nguidance = \"local\"\n[agents.claude]\nscope = \"local\"\n");
  let core = atlas_mcp::guidance::ToolProfile::from_setting(&atlas_project::ToolsSetting::Core).unwrap();

  let synced = sandbox
    .command(&repo.join("src"), &["agents", "sync", "--agent", "claude"])
    .env("CODE_ATLAS_MCP_TOOLS", "full")
    .stdin(Stdio::null())
    .output()
    .unwrap();
  assert_ok(&synced);

  let registered = sandbox.json("atlas/claude/.claude.json");
  assert_eq!(
    registered["projects"][repo.to_str().unwrap()]["mcpServers"]["code-atlas"]["args"],
    json!(["mcp", "--watch"])
  );
  let guidance = fs::read_to_string(repo.join("CLAUDE.local.md")).unwrap();
  assert_eq!(guidance, format!("{}\n", atlas_mcp::guidance::repo_block("solo", &core)));
  assert!(!guidance.contains("atlas_similar"), "{guidance}");
  let allowed: Vec<String> = core.read_only_tools().iter().map(|tool| format!("mcp__code-atlas__{tool}")).collect();
  assert_eq!(read_json(&repo.join(".claude/settings.local.json"))["permissions"]["allow"], json!(allowed));
  assert_eq!(git_status(&repo), "");
  let exclude = fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
  assert!(exclude.ends_with("/.claude/settings.local.json\n/CLAUDE.local.md\n"), "{exclude}");

  let dry_run = sandbox.run(&repo, &["agents", "unsync", "--dry-run"]);
  assert_ok(&dry_run);
  assert!(stdout(&dry_run).contains("dry run: nothing written"), "{}", stdout(&dry_run));
  assert!(repo.join("CLAUDE.local.md").exists());

  assert_ok(&sandbox.run(&repo, &["agents", "unsync", "--repo", repo.to_str().unwrap()]));
  assert!(!repo.join("CLAUDE.local.md").exists() && !repo.join(".claude/settings.local.json").exists());
  assert!(!fs::read_to_string(repo.join(".git/info/exclude")).unwrap().contains("CLAUDE.local.md"));
  assert!(sandbox.claude_calls().contains(&"mcp remove --scope local code-atlas".to_owned()));
  assert!(sandbox.wiring().is_empty());
}

#[test]
fn should_refuse_local_guidance_that_would_hide_agents_md_unless_forced() {
  let sandbox = Sandbox::new();
  let repo = sandbox.repo("team");
  fs::write(repo.join("AGENTS.md"), "# Team rules\n").unwrap();
  sandbox.config("[agents]\nguidance = \"local\"\n[agents.claude]\nhooks = []\n");

  let refused = sandbox.run(&repo, &["agents", "sync", "--agent", "claude"]);
  assert_eq!(code(&refused), 2, "{}", stderr(&refused));
  assert!(stdout(&refused).contains("would stop Claude Code from reading AGENTS.md"), "{}", stdout(&refused));
  assert!(!repo.join("CLAUDE.local.md").exists());
  assert!(sandbox.claude_calls().is_empty());

  assert_ok(&sandbox.run(&repo, &["agents", "sync", "--agent", "claude", "--force", "--yes"]));
  assert!(repo.join("CLAUDE.local.md").exists());
}

#[test]
fn should_remove_old_local_entries_that_shadow_the_user_scope_only_with_migrate() {
  let sandbox = Sandbox::new();
  let work = sandbox.path("work");
  let legacy = work.join("legacy");
  fs::create_dir_all(&legacy).unwrap();
  let gone = work.join("gone");
  let entry = json!({"command": "/old/bin/catl", "args": ["mcp", "--project", "legacy"]});
  let old = json!({"projects": {
    legacy.to_str().unwrap(): {"mcpServers": {"code-atlas": entry}},
    gone.to_str().unwrap(): {"mcpServers": {"code-atlas": entry}},
  }});
  fs::write(sandbox.path("atlas/claude/.claude.json"), old.to_string()).unwrap();
  sandbox.config("[agents.claude]\nhooks = []\n");

  let noted = sandbox.run(&work, &["agents", "sync", "--agent", "claude", "--dry-run"]);
  assert_ok(&noted);
  let expected = format!(
    "1 local-scope code-atlas entries from an older catl win over the user scope in {}; rerun with --migrate",
    legacy.display()
  );
  assert!(stderr(&noted).contains(&expected), "{}", stderr(&noted));

  let refused = sandbox.run(&work, &["agents", "sync", "--agent", "claude", "--migrate"]);
  assert_eq!(code(&refused), 2);
  assert!(stderr(&refused).contains("remove a local-scope entry from an older catl"), "{}", stderr(&refused));
  assert_ok(&sandbox.run(&work, &["agents", "sync", "--agent", "claude", "--migrate", "--yes"]));
  let removals = sandbox.claude_calls().iter().filter(|call| *call == "mcp remove --scope local code-atlas").count();
  assert_eq!(removals, 1, "{:?}", sandbox.claude_calls());
}

#[test]
fn should_skip_codex_when_it_is_not_installed_unless_asked_for_it() {
  let sandbox = Sandbox::new();
  let work = sandbox.path("work");
  let missing = sandbox.path("no-codex");
  let without_codex = [("CODEX_HOME", missing.as_path())];
  sandbox.config("[agents.claude]\nenabled = false\n");

  let skipped = sandbox.run_with(&work, &["agents", "sync"], &without_codex);
  assert_eq!(code(&skipped), 2);
  assert!(
    stderr(&skipped).contains("skipped Codex: ") && stderr(&skipped).contains("no agent to wire"),
    "{}",
    stderr(&skipped)
  );
  let asked = sandbox.run_with(&work, &["agents", "sync", "--agent", "codex"], &without_codex);
  assert_eq!(code(&asked), 2);
  assert!(stderr(&asked).contains("Codex is not set up here"), "{}", stderr(&asked));
}

fn hook_input(tool: &str, session: &str, cwd: &Path, input: Value) -> String {
  json!({
    "session_id": session,
    "transcript_path": "/tmp/t.jsonl",
    "cwd": cwd,
    "permission_mode": "default",
    "hook_event_name": "PreToolUse",
    "tool_name": tool,
    "tool_input": input,
    "tool_use_id": "toolu_01"
  })
  .to_string()
}

#[test]
fn should_exit_0_silently_from_any_hook_invocation_that_cannot_answer() {
  let sandbox = Sandbox::new();
  let repo = sandbox.repo("solo");
  let grep = hook_input("Grep", "session-x", &repo, json!({"pattern": "x"}));
  fs::write(sandbox.path("atlas/config/projects.toml"), "not = [toml").unwrap();

  let outputs = [
    sandbox.hook(&repo, "auto-approve", &grep),
    sandbox.hook(&repo, "no-such-hook", &grep),
    sandbox.hook(&repo, "remind", "not json"),
    sandbox.hook(&repo, "remind", &grep),
    {
      let mut command = sandbox.command(&repo, &["hook", "remind"]);
      command
        .env_remove("HOME")
        .env_remove("CODE_ATLAS_HOME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
      let mut child = command.spawn().unwrap();
      child.stdin.take().unwrap().write_all(grep.as_bytes()).unwrap();
      child.wait_with_output().unwrap()
    },
  ];

  for output in &outputs {
    assert_eq!((code(output), stdout(output), stderr(output)), (0, String::new(), String::new()));
  }
}

#[test]
fn should_remind_once_per_session_and_only_inside_a_registered_project() {
  let sandbox = Sandbox::new();
  let repo = sandbox.repo("solo");
  let outside = sandbox.path("work");

  let first = sandbox.hook(&repo, "remind", &hook_input("Grep", "session-a", &repo.join("src"), json!({"pattern": "answer"})));
  let second = sandbox.hook(&repo, "remind", &hook_input("Bash", "session-a", &repo, json!({"command": "rg answer"})));
  let other_session = sandbox.hook(&repo, "remind", &hook_input("Glob", "session-b", &repo, json!({"pattern": "**/*.rs"})));
  let unregistered = sandbox.hook(&outside, "remind", &hook_input("Grep", "session-c", &outside, json!({"pattern": "x"})));

  let reminder = json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": atlas_mcp::guidance::REMINDER}});
  assert_eq!(serde_json::from_str::<Value>(&stdout(&first)).unwrap(), reminder);
  assert_eq!(stdout(&second), "");
  assert_eq!(serde_json::from_str::<Value>(&stdout(&other_session)).unwrap(), reminder);
  assert_eq!(stdout(&unregistered), "");
  for output in [&first, &second, &other_session, &unregistered] {
    assert_ok(output);
  }
}

#[test]
fn should_record_only_what_was_applied_when_a_sync_fails_midway_and_converge_on_rerun() {
  let sandbox = Sandbox::new();
  let work = sandbox.path("work");
  fs::write(sandbox.path("atlas/codex/config.toml"), CODEX_CONFIG).unwrap();
  fs::set_permissions(sandbox.path("atlas/codex"), fs::Permissions::from_mode(0o555)).unwrap();

  let failed = sandbox.run(&work, &["agents", "sync", "--yes"]);
  fs::set_permissions(sandbox.path("atlas/codex"), fs::Permissions::from_mode(0o755)).unwrap();

  assert_eq!(code(&failed), 1, "{}", stderr(&failed));
  let recorded: Vec<String> = sandbox.wiring().iter().map(|entry| entry["target"]["kind"].as_str().unwrap().to_owned()).collect();
  assert_eq!(recorded, ["registration", "permissions", "hooks"]);
  assert_eq!(fs::read_to_string(sandbox.path("atlas/codex/config.toml")).unwrap(), CODEX_CONFIG);

  let rerun = sandbox.run(&work, &["agents", "sync", "--yes"]);
  assert_ok(&rerun);
  assert_eq!(stdout(&rerun).matches(": unchanged").count(), 3, "{}", stdout(&rerun));
  assert!(stdout(&rerun).contains("#mcp_servers.code-atlas: created"), "{}", stdout(&rerun));
  assert_eq!(sandbox.wiring().len(), 4);
}

#[test]
fn should_restore_the_previous_registration_when_claude_fails_to_add_the_new_one() {
  let sandbox = Sandbox::new();
  let work = sandbox.path("work");
  sandbox.config("[agents.claude]\nhooks = []\n");
  assert_ok(&sandbox.run(&work, &["agents", "sync", "--agent", "claude"]));
  let before = sandbox.json("atlas/claude/.claude.json");
  sandbox.config("[agents]\nbinary = \"catl-next\"\n[agents.claude]\nhooks = []\n");

  let failed = sandbox.run_with(&work, &["agents", "sync", "--agent", "claude"], &[("FAKE_CLAUDE_FAIL_ADD", Path::new("catl-next"))]);

  assert_eq!(code(&failed), 2, "{}", stderr(&failed));
  assert!(stderr(&failed).contains("add-json failed"), "{}", stderr(&failed));
  assert_eq!(sandbox.json("atlas/claude/.claude.json"), before);
  assert_ok(&sandbox.run(&work, &["agents", "status"]));
  assert_ok(&sandbox.run(&work, &["agents", "sync", "--agent", "claude"]));
  assert_eq!(sandbox.json("atlas/claude/.claude.json")["mcpServers"]["code-atlas"]["command"], "catl-next");
}

#[test]
fn should_forget_a_registration_lost_to_a_failed_update_so_the_next_sync_recreates_it() {
  let sandbox = Sandbox::new();
  let work = sandbox.path("work");
  sandbox.config("[agents.claude]\nhooks = []\n");
  assert_ok(&sandbox.run(&work, &["agents", "sync", "--agent", "claude"]));
  sandbox.config("[agents]\nbinary = \"catl-next\"\n[agents.claude]\nhooks = []\n");

  let failed = sandbox.run_with(&work, &["agents", "sync", "--agent", "claude"], &[("FAKE_CLAUDE_FAIL_ADD", Path::new("catl"))]);

  assert_eq!(code(&failed), 2, "{}", stderr(&failed));
  assert_eq!(sandbox.json("atlas/claude/.claude.json"), json!({}));
  assert!(sandbox.wiring().is_empty(), "{:?}", sandbox.wiring());
  let rerun = sandbox.run(&work, &["agents", "sync", "--agent", "claude"]);
  assert_ok(&rerun);
  assert!(stdout(&rerun).contains("#mcpServers.code-atlas: created"), "{}", stdout(&rerun));
}

#[test]
fn should_fail_loudly_and_record_nothing_when_claude_reports_success_without_registering() {
  let sandbox = Sandbox::new();
  let work = sandbox.path("work");
  sandbox.config("[agents.claude]\nhooks = []\n");

  let output = sandbox.run_with(&work, &["agents", "sync", "--agent", "claude"], &[("FAKE_CLAUDE_NOOP", Path::new("1"))]);

  assert_eq!(code(&output), 1, "{}", stderr(&output));
  assert!(
    stderr(&output).contains("`claude mcp add-json` reported success, but the entry Code Atlas reads back is absent"),
    "{}",
    stderr(&output)
  );
  assert!(!sandbox.path("atlas/data/wiring.json").exists() || sandbox.wiring().is_empty());
}

#[test]
fn should_refuse_to_wire_agents_from_a_cargo_test_without_a_sandboxed_claude_or_codex_home() {
  let sandbox = Sandbox::new();
  let work = sandbox.path("work");
  for (missing, args) in [
    ("CLAUDE_CONFIG_DIR", &["agents", "sync"][..]),
    ("CODEX_HOME", &["agents", "unsync"][..]),
    ("CODE_ATLAS_HOME", &["agents", "sync"][..]),
  ] {
    let output = sandbox
      .command(&work, args)
      .env("CARGO_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR"))
      .env_remove(missing)
      .stdin(Stdio::null())
      .output()
      .unwrap();

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
      stderr(&output).contains(&format!(
        "refusing to change agent configuration from a Cargo build or test (CARGO_MANIFEST_DIR is set) because it would change the real user configuration; unset or outside the temporary directory: {missing}"
      )),
      "{}",
      stderr(&output)
    );
  }
  assert!(sandbox.claude_calls().is_empty());
}

#[test]
fn should_refuse_to_wire_agents_from_a_cargo_test_when_a_home_lies_outside_the_temporary_directory() {
  let sandbox = Sandbox::new();
  let work = sandbox.path("work");
  let output = sandbox
    .command(&work, &["agents", "sync"])
    .env("CARGO_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR"))
    .env("CODEX_HOME", "/nonexistent-codex-home")
    .stdin(Stdio::null())
    .output()
    .unwrap();

  assert_eq!(code(&output), 2, "{}", stderr(&output));
  assert!(stderr(&output).contains("unset or outside the temporary directory: CODEX_HOME"), "{}", stderr(&output));
  assert!(sandbox.claude_calls().is_empty());
  assert!(!Path::new("/nonexistent-codex-home").exists());
}

const ORDERS_RS: &str = "pub struct Order {\n  pub cents: i64,\n}\n\npub fn parse_order(text: &str) -> Order {\n  Order { cents: total_cents(text) }\n}\n\nfn total_cents(text: &str) -> i64 {\n  text.len() as i64 * 100\n}\n";
const SEED_PROMPT: &str = "why does parse_order round the quokka total down?";

fn prompt_event(prompt: &str, cwd: &Path) -> String {
  json!({
    "session_id": "session-seed",
    "transcript_path": "/tmp/t.jsonl",
    "cwd": cwd,
    "permission_mode": "default",
    "hook_event_name": "UserPromptSubmit",
    "prompt": prompt
  })
  .to_string()
}

impl Sandbox {
  fn orders_repo(&self, name: &str) -> PathBuf {
    let repo = self.repo(name);
    fs::write(repo.join("src/lib.rs"), ORDERS_RS).unwrap();
    repo
  }

  fn seed(&self, cwd: &Path, args: &[&str], input: &str) -> (Output, Duration) {
    let mut arguments = args.to_vec();
    arguments.extend(["hook", "seed"]);
    let started = Instant::now();
    let mut child = self
      .command(cwd, &arguments)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .spawn()
      .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    (output, started.elapsed())
  }
}

#[test]
fn should_seed_a_prompt_from_a_fresh_index_with_the_ranked_code_and_never_log_the_prompt() {
  let sandbox = Sandbox::new();
  let repo = sandbox.orders_repo("shop");
  assert_ok(&sandbox.run(&repo, &["index"]));

  let (output, _) = sandbox.seed(&repo.join("src"), &["--log-level", "trace"], &prompt_event(SEED_PROMPT, &repo.join("src")));

  assert_ok(&output);
  let answer: Value = serde_json::from_str(&stdout(&output)).unwrap();
  let context = answer["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_owned();
  assert_eq!(answer, json!({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit", "additionalContext": context}}));
  let footer = atlas_mcp::guidance::seed_footer(&atlas_mcp::guidance::ToolProfile::full()).unwrap();
  assert_eq!(
    context,
    format!(
      "{}\nRead first:\n- parse_order function shop:src/lib.rs:5-7  why: named in task as parse_order\n- Order struct shop:src/lib.rs:1-3  why: instantiated by parse_order [resolved]\n- total_cents function shop:src/lib.rs:9-11  why: called by parse_order [resolved]\n{footer}",
      atlas_mcp::guidance::SEED_INTRO
    )
  );
  let log = stderr(&output);
  assert!(log.contains("seeded the prompt") && log.contains(&format!("prompt_chars={}", SEED_PROMPT.len())), "{log}");
  assert!(!log.contains("quokka") && !log.contains("round the"), "the prompt leaked into the log:\n{log}");
}

#[test]
fn should_seed_nothing_and_exit_0_for_any_event_it_cannot_answer_from_a_fresh_index() {
  let sandbox = Sandbox::new();
  let indexed = sandbox.orders_repo("shop");
  assert_ok(&sandbox.run(&indexed, &["index"]));
  let unindexed = sandbox.orders_repo("fresh");
  let outside = sandbox.path("work");
  let event = prompt_event(SEED_PROMPT, &indexed);
  let without = |field: &str| {
    let mut value: Value = serde_json::from_str(&event).unwrap();
    value.as_object_mut().unwrap().remove(field);
    value.to_string()
  };

  let not_a_prompt = "no seed: not a UserPromptSubmit event with a prompt and an absolute cwd";
  let no_code = "no seed: the prompt is too short or has no code-like content";
  let silent = [
    ("{\"hook_event_name\": \"UserPromptSubmit\"".to_owned(), &indexed, "the hook input is not JSON"),
    (event.replace("UserPromptSubmit", "PreToolUse"), &indexed, not_a_prompt),
    (without("prompt"), &indexed, not_a_prompt),
    (without("cwd"), &indexed, not_a_prompt),
    (prompt_event(SEED_PROMPT, Path::new("shop/src")), &indexed, not_a_prompt),
    (prompt_event("fix it quokka", &indexed), &indexed, no_code),
    (prompt_event("looks good quokka, thanks! now run the tests", &indexed), &indexed, no_code),
    (prompt_event(SEED_PROMPT, &outside), &outside, "reason=no project: "),
    (prompt_event(SEED_PROMPT, &unindexed), &unindexed, "reason=the project has never been indexed"),
    (
      prompt_event("rename the quokka WombatRegistry::flush_all helper", &indexed),
      &indexed,
      "reason=the pack is empty",
    ),
  ];
  for (input, cwd, reason) in silent {
    let (quiet, _) = sandbox.seed(cwd, &[], &input);
    assert_eq!((code(&quiet), stdout(&quiet), stderr(&quiet)), (0, String::new(), String::new()), "{reason}");
    let (logged, _) = sandbox.seed(cwd, &["--log-level", "debug"], &input);
    assert_eq!((code(&logged), stdout(&logged)), (0, String::new()), "{reason}");
    assert!(stderr(&logged).contains(reason) && !stderr(&logged).contains("quokka"), "{reason}:\n{}", stderr(&logged));
  }
  let unindexed_db = sandbox.path("atlas/data/fresh");
  assert!(!unindexed_db.exists(), "a hook must never create or index a database: {}", unindexed_db.display());
}

fn session_event(prompt: &str, cwd: &Path, session: Option<&str>) -> String {
  let mut event: Value = serde_json::from_str(&prompt_event(prompt, cwd)).unwrap();
  match session {
    Some(session) => event["session_id"] = json!(session),
    None => drop(event.as_object_mut().unwrap().remove("session_id")),
  }
  event.to_string()
}

#[test]
fn should_seed_plain_english_only_for_the_first_prompt_of_a_session_and_code_prompts_on_any_turn() {
  let sandbox = Sandbox::new();
  let repo = sandbox.orders_repo("shop");
  assert_ok(&sandbox.run(&repo, &["index"]));
  let plain = "the order total cents are rounded wrong";
  let seeded = |output: &Output| stdout(output).contains("total_cents function shop:src/lib.rs:9-11");

  let (first, _) = sandbox.seed(&repo, &[], &session_event(plain, &repo, Some("session-1")));
  let (again, _) = sandbox.seed(&repo, &["--log-level", "debug"], &session_event(plain, &repo, Some("session-1")));
  let (code_later, _) = sandbox.seed(&repo, &[], &session_event("why does total_cents round down?", &repo, Some("session-1")));
  let (other_session, _) = sandbox.seed(&repo, &[], &session_event(plain, &repo, Some("session-2")));
  let (no_session, _) = sandbox.seed(&repo, &[], &session_event(plain, &repo, None));
  let (unrelated, _) = sandbox.seed(
    &repo,
    &["--log-level", "info"],
    &session_event("try a different approach for the order report", &repo, Some("session-3")),
  );

  assert!(seeded(&first), "{}", stdout(&first));
  assert_eq!(stdout(&again), "");
  assert!(
    stderr(&again).contains("no seed: a plain-English prompt after the first of its session"),
    "{}",
    stderr(&again)
  );
  assert!(seeded(&code_later), "{}", stdout(&code_later));
  assert!(seeded(&other_session), "{}", stdout(&other_session));
  assert_eq!(stdout(&no_session), "");
  assert_eq!(stdout(&unrelated), "");
  assert!(
    stderr(&unrelated).contains("reason=no item is named in the prompt and none of the top 3 matches 2 of its words"),
    "{}",
    stderr(&unrelated)
  );
  assert_eq!(fs::read_dir(sandbox.path("atlas/data/hooks/seeded")).unwrap().count(), 2);
}

#[test]
fn should_keep_the_first_plain_prompt_of_a_session_for_the_first_seed_that_is_emitted() {
  let sandbox = Sandbox::new();
  let repo = sandbox.orders_repo("shop");
  assert_ok(&sandbox.run(&repo, &["index"]));
  let plain = "the order total cents are rounded wrong";
  let markers = sandbox.path("atlas/data/hooks/seeded");
  let marker_count = || fs::read_dir(&markers).map(Iterator::count).unwrap_or(0);
  fs::write(repo.join("src/lib.rs"), format!("{ORDERS_RS}\npub fn refund_order() {{}}\n")).unwrap();

  let (stale, _) = sandbox.seed(&repo, &["--log-level", "info"], &session_event(plain, &repo, Some("session-stale")));

  assert_eq!(stdout(&stale), "");
  assert!(stderr(&stale).contains("reason=the index is not fresh"), "{}", stderr(&stale));
  assert_eq!(marker_count(), 0);
  assert_ok(&sandbox.run(&repo, &["status"]));
  let (second, _) = sandbox.seed(&repo, &[], &session_event(plain, &repo, Some("session-stale")));
  let (third, _) = sandbox.seed(&repo, &[], &session_event(plain, &repo, Some("session-stale")));
  assert!(stdout(&second).contains("total_cents function shop:src/lib.rs:9-11"), "{}", stdout(&second));
  assert_eq!(stdout(&third), "");
  assert_eq!(marker_count(), 1);
}

const CACHE_RS: &str = "pub const REFRESH_DEFERRED: u32 = 7;\n\npub struct Cache {\n  pub pending: u32,\n}\n\nimpl Cache {\n  pub fn flush_pending(&mut self) -> u32 {\n    let flushed = self.pending;\n    self.pending = REFRESH_DEFERRED - 7;\n    flushed\n  }\n}\n";

#[test]
fn should_seed_a_code_prompt_whose_token_is_the_short_name_of_a_top_item_even_when_the_pack_only_has_search_hits() {
  let sandbox = Sandbox::new();
  let repo = sandbox.repo("shop");
  fs::write(repo.join("src/lib.rs"), CACHE_RS).unwrap();
  assert_ok(&sandbox.run(&repo, &["index"]));

  for (prompt, expected) in [
    ("what does `flush_pending` return when nothing changed?", "flush_pending"),
    ("where is `REFRESH_DEFERRED` checked?", "REFRESH_DEFERRED"),
  ] {
    let (output, _) = sandbox.seed(&repo, &["--log-level", "info"], &session_event(prompt, &repo, None));
    assert!(stdout(&output).contains(expected), "{prompt}: {}\n{}", stdout(&output), stderr(&output));
  }
}

fn index_runs(sandbox: &Sandbox) -> i64 {
  let db = rusqlite::Connection::open(sandbox.path("atlas/data/shop/atlas.db")).unwrap();
  db.query_row("SELECT count(*) FROM index_runs", [], |row| row.get(0)).unwrap()
}

#[test]
fn should_never_refresh_the_index_itself_and_seed_again_once_another_caller_has() {
  let sandbox = Sandbox::new();
  let repo = sandbox.orders_repo("shop");
  assert_ok(&sandbox.run(&repo, &["index"]));
  let runs = index_runs(&sandbox);
  fs::write(repo.join("src/lib.rs"), format!("{ORDERS_RS}\npub fn refund_order() {{}}\n")).unwrap();
  let prompt = prompt_event("why does refund_order skip parse_order?", &repo);

  let (stale, elapsed) = sandbox.seed(&repo, &["--log-level", "info"], &prompt);

  assert_eq!((code(&stale), stdout(&stale)), (0, String::new()));
  assert!(
    stderr(&stale).contains("reason=the index is not fresh: refresh left to another caller, the index may be stale: 1 file changed since the last index run"),
    "{}",
    stderr(&stale)
  );
  assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
  assert_eq!(index_runs(&sandbox), runs);
  assert_ok(&sandbox.run(&repo, &["status"]));
  let (fresh, _) = sandbox.seed(&repo, &[], &prompt);
  assert!(stdout(&fresh).contains("refund_order function shop:src/lib.rs:13"), "{}", stdout(&fresh));
}

#[test]
fn should_seed_nothing_within_the_time_limit_while_another_index_run_holds_the_lock() {
  let sandbox = Sandbox::new();
  let repo = sandbox.orders_repo("shop");
  assert_ok(&sandbox.run(&repo, &["index"]));
  let db_path = sandbox.path("atlas/data/shop/atlas.db");
  let db = rusqlite::Connection::open(&db_path).unwrap();
  db.execute(
    "INSERT INTO derived_blobs (name, version, bytes, updated_run) VALUES ('engine:dirty', 1, x'0900000000000000', NULL)",
    [],
  )
  .unwrap();
  let mut lock = fd_lock::RwLock::new(
    fs::OpenOptions::new()
      .read(true)
      .write(true)
      .open(sandbox.path("atlas/data/shop/atlas.db.index.lock"))
      .unwrap(),
  );
  let guard = lock.write().unwrap();

  let (output, elapsed) = sandbox.seed(&repo, &["--log-level", "info"], &prompt_event(SEED_PROMPT, &repo));
  drop(guard);

  assert_eq!((code(&output), stdout(&output)), (0, String::new()));
  assert!(stderr(&output).contains("waiting for the index lock"), "{}", stderr(&output));
  assert!(elapsed >= Duration::from_millis(2_500) && elapsed < Duration::from_secs(10), "{elapsed:?}");
  db.execute("DELETE FROM derived_blobs WHERE name = 'engine:dirty'", []).unwrap();
  let (seeded, _) = sandbox.seed(&repo, &[], &prompt_event(SEED_PROMPT, &repo));
  assert!(stdout(&seeded).contains("parse_order function"), "{}", stdout(&seeded));
}

fn hook_kinds(sandbox: &Sandbox) -> Vec<String> {
  sandbox
    .wiring()
    .iter()
    .map(|entry| format!("{} {}", entry["target"]["kind"].as_str().unwrap(), entry["target"]["event"].as_str().unwrap_or("-")))
    .collect()
}

#[test]
fn should_add_the_seed_group_to_an_earlier_sync_keep_foreign_prompt_hooks_and_remove_only_its_own() {
  let sandbox = Sandbox::new();
  let work = sandbox.path("work");
  let foreign_prompt = json!({"hooks": [{"type": "command", "command": "log-prompt"}]});
  let mut settings: Value = serde_json::from_str(FOREIGN_HOOK).unwrap();
  settings["hooks"]["UserPromptSubmit"] = json!([foreign_prompt.clone()]);
  fs::write(sandbox.path("atlas/claude/settings.json"), settings.to_string()).unwrap();
  sandbox.config("[agents.claude]\nhooks = [\"auto-approve\", \"remind\"]\n");
  assert_ok(&sandbox.run(&work, &["agents", "sync", "--agent", "claude", "--yes"]));
  assert_eq!(hook_kinds(&sandbox), ["registration -", "permissions -", "hooks -"]);

  sandbox.config("[agents.claude]\nhooks = [\"auto-approve\", \"remind\", \"seed\"]\n");
  let unconfirmed = sandbox.run(&work, &["agents", "sync", "--agent", "claude"]);
  assert_eq!(code(&unconfirmed), 2, "{}", stderr(&unconfirmed));
  let migrated = sandbox.run(&work, &["agents", "sync", "--agent", "claude", "--yes"]);
  assert_ok(&migrated);
  assert_eq!(stdout(&migrated).matches(": unchanged").count(), 3, "{}", stdout(&migrated));
  assert!(stdout(&migrated).contains("settings.json#hooks.UserPromptSubmit: created"), "{}", stdout(&migrated));
  assert!(!stderr(&migrated).contains("seed hook is on by default"), "{}", stderr(&migrated));
  assert_eq!(sandbox.json("atlas/data/wiring.json")["version"], 2);
  assert_eq!(hook_kinds(&sandbox), ["registration -", "permissions -", "hooks -", "hooks UserPromptSubmit"]);
  let home = sandbox.path("atlas");
  let seed = format!("CODE_ATLAS_HOME={} catl hook seed || true", home.display());
  let written = sandbox.json("atlas/claude/settings.json");
  assert_eq!(
    written["hooks"]["UserPromptSubmit"],
    json!([foreign_prompt.clone(), {"hooks": [{"type": "command", "command": seed, "timeout": 10}]}])
  );
  assert_eq!(
    written["hooks"]["PreToolUse"][0],
    json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "lint-bash"}]})
  );
  let printed = sandbox.run(&work, &["agents", "hooks", "--json"]);
  assert_ok(&printed);
  assert_eq!(
    serde_json::from_str::<Value>(&stdout(&printed)).unwrap(),
    json!({"PreToolUse": [written["hooks"]["PreToolUse"][1].clone()], "UserPromptSubmit": [written["hooks"]["UserPromptSubmit"][1].clone()]})
  );
  let status = sandbox.run(&work, &["agents", "status"]);
  assert_ok(&status);
  assert!(stdout(&status).contains("settings.json#hooks.UserPromptSubmit: ok\n"), "{}", stdout(&status));

  let mut edited = sandbox.json("atlas/claude/settings.json");
  edited["hooks"]["UserPromptSubmit"][1]["hooks"][0]["timeout"] = json!(30);
  fs::write(sandbox.path("atlas/claude/settings.json"), edited.to_string()).unwrap();
  let drift = sandbox.run(&work, &["agents", "status"]);
  assert_eq!(code(&drift), 3);
  assert!(stdout(&drift).contains("settings.json#hooks.UserPromptSubmit: changed"), "{}", stdout(&drift));
  fs::write(sandbox.path("atlas/claude/settings.json"), written.to_string()).unwrap();

  sandbox.config("[agents.claude]\nhooks = [\"auto-approve\", \"remind\"]\n");
  let turned_off = sandbox.run(&work, &["agents", "sync", "--agent", "claude", "--yes"]);
  assert_ok(&turned_off);
  assert!(stdout(&turned_off).contains("settings.json#hooks.UserPromptSubmit: removed"), "{}", stdout(&turned_off));
  assert_eq!(sandbox.json("atlas/claude/settings.json")["hooks"]["UserPromptSubmit"], json!([foreign_prompt.clone()]));
  assert_eq!(hook_kinds(&sandbox), ["registration -", "permissions -", "hooks -"]);

  assert_ok(&sandbox.run(&work, &["agents", "unsync", "--yes"]));
  assert_eq!(sandbox.json("atlas/claude/settings.json"), settings);
}

#[test]
fn should_refuse_a_seed_group_it_did_not_write_and_delete_a_local_settings_file_it_created_once_hollow() {
  let sandbox = Sandbox::new();
  let repo = sandbox.git_repo("solo");
  sandbox.config("[agents.claude]\nscope = \"local\"\nhooks = [\"auto-approve\"]\n");
  assert_ok(&sandbox.run(&repo, &["agents", "sync", "--agent", "claude"]));
  sandbox.config("[agents.claude]\nscope = \"local\"\n");
  let defaulted = sandbox.run(&repo, &["agents", "sync", "--agent", "claude", "--yes"]);
  assert_ok(&defaulted);
  assert!(
    stderr(&defaulted).contains("new: the seed hook is on by default; it adds a short Code Atlas context pack to each prompt you submit in a registered project (UserPromptSubmit). To turn it off, set [agents.claude] hooks = [\"auto-approve\"] in config.toml and rerun `catl agents sync`"),
    "{}",
    stderr(&defaulted)
  );
  let again = sandbox.run(&repo, &["agents", "sync", "--agent", "claude", "--yes"]);
  assert!(!stderr(&again).contains("seed hook is on by default"), "{}", stderr(&again));
  let local = repo.join(".claude/settings.local.json");
  assert_eq!(
    read_json(&local)["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
    json!(format!("CODE_ATLAS_HOME={} catl hook seed || true", sandbox.path("atlas").display()))
  );
  assert_ok(&sandbox.run(&repo, &["agents", "unsync", "--yes"]));
  assert!(!local.exists(), "{}", fs::read_to_string(&local).unwrap_or_default());

  let other = sandbox.git_repo("other");
  let hand_written = json!({"hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "catl hook seed"}]}]}});
  fs::create_dir_all(other.join(".claude")).unwrap();
  fs::write(other.join(".claude/settings.local.json"), hand_written.to_string()).unwrap();
  let refused = sandbox.run(&other, &["agents", "sync", "--agent", "claude", "--yes"]);
  assert_eq!(code(&refused), 2, "{}", stderr(&refused));
  assert!(
    stdout(&refused).contains("#hooks.UserPromptSubmit: refused: holds an entry that catl agents sync did not write; pass --force to replace it"),
    "{}",
    stdout(&refused)
  );
  assert_eq!(read_json(&other.join(".claude/settings.local.json")), hand_written);
}

#[test]
fn should_write_the_counterpart_group_only_when_listed_record_its_event_and_remove_only_its_own() {
  let sandbox = Sandbox::new();
  let work = sandbox.path("work");
  let foreign_post = json!({"matcher": "Edit", "hooks": [{"type": "command", "command": "format-on-save"}]});
  fs::write(
    sandbox.path("atlas/claude/settings.json"),
    json!({"hooks": {"PostToolUse": [foreign_post.clone()]}}).to_string(),
  )
  .unwrap();
  sandbox.config("[agents.claude]\nhooks = [\"auto-approve\"]\n");
  assert_ok(&sandbox.run(&work, &["agents", "sync", "--agent", "claude", "--yes"]));
  assert_eq!(hook_kinds(&sandbox), ["registration -", "permissions -"]);

  sandbox.config("[agents.claude]\nhooks = [\"auto-approve\", \"counterpart\"]\n");
  let synced = sandbox.run(&work, &["agents", "sync", "--agent", "claude", "--yes"]);

  assert_ok(&synced);
  assert!(stdout(&synced).contains("settings.json#hooks.PostToolUse: created"), "{}", stdout(&synced));
  assert_eq!(sandbox.json("atlas/data/wiring.json")["version"], 2);
  assert_eq!(hook_kinds(&sandbox), ["registration -", "permissions -", "hooks PostToolUse"]);
  let command = format!("CODE_ATLAS_HOME={} catl hook counterpart || true", sandbox.path("atlas").display());
  let ours = json!({"matcher": "Read", "hooks": [{"type": "command", "command": command, "timeout": 10}]});
  assert_eq!(
    sandbox.json("atlas/claude/settings.json")["hooks"]["PostToolUse"],
    json!([foreign_post.clone(), ours.clone()])
  );
  let printed = sandbox.run(&work, &["agents", "hooks", "--json"]);
  assert_eq!(serde_json::from_str::<Value>(&stdout(&printed)).unwrap(), json!({"PostToolUse": [ours]}));

  sandbox.config("[agents.claude]\nhooks = [\"auto-approve\"]\n");
  let turned_off = sandbox.run(&work, &["agents", "sync", "--agent", "claude", "--yes"]);
  assert_ok(&turned_off);
  assert!(stdout(&turned_off).contains("settings.json#hooks.PostToolUse: removed"), "{}", stdout(&turned_off));
  assert_eq!(sandbox.json("atlas/claude/settings.json")["hooks"]["PostToolUse"], json!([foreign_post]));
}

#[test]
fn should_write_the_notes_group_next_to_the_counterpart_group_and_remove_only_its_own() {
  let sandbox = Sandbox::new();
  let work = sandbox.path("work");
  sandbox.config("[agents.claude]\nhooks = [\"auto-approve\", \"counterpart\", \"notes\"]\n");

  let synced = sandbox.run(&work, &["agents", "sync", "--agent", "claude", "--yes"]);

  assert_ok(&synced);
  assert_eq!(hook_kinds(&sandbox), ["registration -", "permissions -", "hooks PostToolUse", "hooks PostToolUse:notes"]);
  let home = sandbox.path("atlas").display().to_string();
  let group = |hook: &str| json!({"matcher": "Read", "hooks": [{"type": "command", "command": format!("CODE_ATLAS_HOME={home} catl hook {hook} || true"), "timeout": 10}]});
  assert_eq!(
    sandbox.json("atlas/claude/settings.json")["hooks"]["PostToolUse"],
    json!([group("counterpart"), group("notes")])
  );
  let printed = sandbox.run(&work, &["agents", "hooks", "--json"]);
  assert_eq!(
    serde_json::from_str::<Value>(&stdout(&printed)).unwrap(),
    json!({"PostToolUse": [group("counterpart"), group("notes")]})
  );

  let resynced = sandbox.run(&work, &["agents", "sync", "--agent", "claude", "--yes"]);
  let status = sandbox.run(&work, &["agents", "status"]);
  assert_ok(&resynced);
  assert!(
    !stdout(&resynced).contains("PostToolUse: created") && !stdout(&resynced).contains("PostToolUse: changed"),
    "{}",
    stdout(&resynced)
  );
  assert_ok(&status);
  assert_eq!(stdout(&status).matches("settings.json#hooks.PostToolUse: ok").count(), 2, "{}", stdout(&status));

  sandbox.config("[agents.claude]\nhooks = [\"auto-approve\", \"counterpart\"]\n");
  let turned_off = sandbox.run(&work, &["agents", "sync", "--agent", "claude", "--yes"]);

  assert_ok(&turned_off);
  assert_eq!(sandbox.json("atlas/claude/settings.json")["hooks"]["PostToolUse"], json!([group("counterpart")]));
  assert_eq!(hook_kinds(&sandbox), ["registration -", "permissions -", "hooks PostToolUse"]);
}
