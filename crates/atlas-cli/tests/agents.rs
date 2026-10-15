use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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
  assert_eq!(recorded, ["registration", "permissions"]);
  assert_eq!(fs::read_to_string(sandbox.path("atlas/codex/config.toml")).unwrap(), CODEX_CONFIG);

  let rerun = sandbox.run(&work, &["agents", "sync", "--yes"]);
  assert_ok(&rerun);
  assert_eq!(stdout(&rerun).matches(": unchanged").count(), 2, "{}", stdout(&rerun));
  assert!(stdout(&rerun).contains("#mcp_servers.code-atlas: created"), "{}", stdout(&rerun));
  assert_eq!(sandbox.wiring().len(), 3);
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
