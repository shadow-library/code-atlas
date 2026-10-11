#![cfg(unix)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use atlas_lsp::{
  Discovered, Discovery, Launch, LspError, Probe, ServerCommand, StepOutcome, TsServerChoice, TsServerSource, TypeScriptDiscovery, canonical_path, discover_typescript_with,
  rust_analyzer_server,
};
use tempfile::TempDir;

const HINT: &str = "typescript-language-server not found. Install it (npm i -g typescript-language-server typescript@5), set catl project config <p> --lsp-ts-command <path>, or allow download with --lsp-download on.";
const TS7: &str = "TypeScript 7 has no tsserver; install typescript@5 alongside or use the bundled one";

static EXEC_LOCK: RwLock<()> = RwLock::new(());

enum Slot {
  Shared(#[allow(dead_code)] RwLockReadGuard<'static, ()>),
  Exclusive(#[allow(dead_code)] RwLockWriteGuard<'static, ()>),
}

struct Sandbox {
  dir: TempDir,
  _slot: Slot,
}

impl Sandbox {
  fn new() -> Self {
    Self::with(Slot::Shared(EXEC_LOCK.read().unwrap_or_else(PoisonError::into_inner)))
  }

  fn exclusive() -> Self {
    Self::with(Slot::Exclusive(EXEC_LOCK.write().unwrap_or_else(PoisonError::into_inner)))
  }

  fn with(slot: Slot) -> Self {
    let sandbox = Self {
      dir: tempfile::tempdir().unwrap(),
      _slot: slot,
    };
    fs::create_dir_all(sandbox.bin()).unwrap();
    symlink("/bin/sleep", sandbox.bin().join("sleep")).unwrap();
    sandbox
  }

  fn root(&self) -> PathBuf {
    canonical_path(self.dir.path()).unwrap()
  }

  fn bin(&self) -> PathBuf {
    self.dir.path().join("bin")
  }

  fn probe(&self) -> Probe {
    Probe {
      path: Some(OsString::from(self.bin())),
      version_timeout: Duration::from_secs(60),
      ping_timeout: Duration::from_secs(60),
    }
  }

  fn path_script(&self, name: &str, body: &str) -> PathBuf {
    script(&self.bin().join(name), body)
  }

  fn repo(&self, name: &str) -> PathBuf {
    let root = self.root().join(name);
    fs::create_dir_all(&root).unwrap();
    root
  }
}

fn script(path: &Path, body: &str) -> PathBuf {
  fs::create_dir_all(path.parent().unwrap()).unwrap();
  fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
  fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
  path.to_owned()
}

fn typescript(root: &Path, manifest: Option<&str>, tsserver: bool) -> PathBuf {
  let dir = root.join("node_modules/typescript");
  fs::create_dir_all(dir.join("lib")).unwrap();
  if let Some(manifest) = manifest {
    fs::write(dir.join("package.json"), manifest).unwrap();
  }
  if tsserver {
    fs::write(dir.join("lib/tsserver.js"), "").unwrap();
  }
  dir.join("lib/tsserver.js")
}

fn version_json(version: &str) -> String {
  format!("{{\"name\":\"typescript\",\"version\":\"{version}\"}}")
}

fn discover(sandbox: &Sandbox, command: Option<&[String]>, roots: &[PathBuf], download: bool) -> Discovery {
  discover_typescript_with(
    &TypeScriptDiscovery {
      command,
      repo_roots: roots,
      download,
    },
    &sandbox.probe(),
  )
}

fn found(discovery: Discovery) -> Discovered {
  discovery.result.unwrap_or_else(|error| panic!("{error}"))
}

fn labels(discovery: &Discovery) -> Vec<String> {
  discovery.steps.iter().map(|step| step.label.clone()).collect()
}

fn outcome<'a>(discovery: &'a Discovery, label: &str) -> &'a StepOutcome {
  &discovery
    .steps
    .iter()
    .find(|step| step.label == label)
    .unwrap_or_else(|| panic!("no step {label}: {:?}", labels(discovery)))
    .outcome
}

fn missing_reason(discovery: &Discovery, label: &str) -> String {
  match outcome(discovery, label) {
    StepOutcome::Missing { reason } => reason.clone(),
    other => panic!("{label}: {other:?}"),
  }
}

fn skipped_reason(discovery: &Discovery, label: &str) -> String {
  match outcome(discovery, label) {
    StepOutcome::Skipped { reason } => reason.clone(),
    other => panic!("{label}: {other:?}"),
  }
}

fn argv(parts: &[&Path]) -> Vec<String> {
  parts.iter().map(|part| part.display().to_string()).collect()
}

fn alive(pid: u32) -> bool {
  Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().unwrap().success()
}

fn installed_command(sandbox: &Sandbox, program: &Path) -> ServerCommand {
  ServerCommand::new(program).arg("--stdio").env("PATH", sandbox.bin())
}

#[test]
fn should_prefer_the_configured_command_and_mark_every_later_step_skipped() {
  let sandbox = Sandbox::new();
  sandbox.path_script("typescript-language-server", "echo 4.0.0");
  let configured = script(&sandbox.root().join("custom/tls"), "echo 5.1.3");
  let setting = argv(&[&configured]);

  let discovery = discover(&sandbox, Some(&setting), &[], true);

  assert_eq!(
    labels(&discovery),
    vec![
      "lsp.typescript_command",
      "typescript-language-server on PATH",
      "node_modules/.bin of the TypeScript repos",
      "npx cache (npx --no-install typescript-language-server)",
      "download (typescript-language-server@5, typescript@5)",
      "tsserver bundled with the server",
    ]
  );
  assert_eq!(
    outcome(&discovery, "lsp.typescript_command"),
    &StepOutcome::Found {
      detail: format!("{} (version 5.1.3)", configured.display())
    }
  );
  assert_eq!(skipped_reason(&discovery, "typescript-language-server on PATH"), "an earlier candidate was found");
  assert_eq!(
    skipped_reason(&discovery, "download (typescript-language-server@5, typescript@5)"),
    "an earlier candidate was found"
  );
  let server = found(discovery);
  assert_eq!(server.command, installed_command(&sandbox, &configured));
  assert_eq!(server.launch, Launch::Installed);
  assert_eq!(server.server_version, "5.1.3");
}

#[test]
fn should_keep_extra_arguments_of_the_configured_command_in_a_path_with_spaces() {
  let sandbox = Sandbox::new();
  let configured = script(
    &sandbox.root().join("my tools/ts server"),
    "for arg in \"$@\"; do if [ \"$arg\" = --version ]; then echo 5.0.1; fi; done; exit 0",
  );
  let mut setting = argv(&[&configured]);
  setting.extend(["--log-level".to_owned(), "4".to_owned(), "--stdio".to_owned()]);

  let server = found(discover(&sandbox, Some(&setting), &[], false));

  assert_eq!(server.server_version, "5.0.1");
  assert_eq!(server.command.program, configured);
  assert_eq!(server.command.args, ["--log-level", "4", "--stdio"].map(OsString::from));
}

fn assert_unusable_setting(discovery: &Discovery, expected_error: &str, expected_reason: &str) {
  assert_eq!(discovery.result.as_ref().unwrap_err().to_string(), expected_error);
  assert_eq!(missing_reason(discovery, "lsp.typescript_command"), expected_reason);
  let unusable = StepOutcome::Skipped {
    reason: "lsp.typescript_command is set but unusable".to_owned(),
  };
  assert_eq!(
    discovery.steps[1..].iter().map(|step| (step.label.as_str(), &step.outcome)).collect::<Vec<_>>(),
    vec![
      ("typescript-language-server on PATH", &unusable),
      ("node_modules/.bin of the TypeScript repos", &unusable),
      ("npx cache (npx --no-install typescript-language-server)", &unusable),
      ("download (typescript-language-server@5, typescript@5)", &unusable),
    ]
  );
}

fn sandbox_with_working_fallbacks() -> Sandbox {
  let sandbox = Sandbox::new();
  sandbox.path_script("typescript-language-server", "echo 4.3.4");
  sandbox.path_script("npx", "echo 5.2.0");
  sandbox.path_script("npm", "exit 0");
  sandbox
}

#[test]
fn should_fail_hard_on_a_relative_empty_missing_or_failing_configured_command() {
  let sandbox = sandbox_with_working_fallbacks();
  let gone = sandbox.root().join("gone/tls");
  let failing = script(&sandbox.root().join("custom/failing"), "echo 'Error: boom' >&2; exit 4");
  let garbage = script(&sandbox.root().join("custom/garbage"), "echo hello");

  let relative = discover(&sandbox, Some(&["bin/tls".to_owned()]), &[], true);
  let empty = discover(&sandbox, Some(&[]), &[], true);
  let missing = discover(&sandbox, Some(&argv(&[&gone])), &[], true);
  let exited = discover(&sandbox, Some(&argv(&[&failing])), &[], true);
  let printed = discover(&sandbox, Some(&argv(&[&garbage])), &[], true);

  assert_unusable_setting(
    &relative,
    "lsp.typescript_command (bin/tls): bin/tls is not an absolute path",
    "bin/tls is not an absolute path",
  );
  assert_unusable_setting(&empty, "lsp.typescript_command (): the command is empty", "the command is empty");
  let spawn_reason = missing_reason(&missing, "lsp.typescript_command");
  assert!(spawn_reason.starts_with(&format!("cannot start {}: ", gone.display())), "{spawn_reason}");
  assert_unusable_setting(&missing, &format!("lsp.typescript_command ({}): {spawn_reason}", gone.display()), &spawn_reason);
  assert_unusable_setting(
    &exited,
    &format!("lsp.typescript_command ({}): --version exited with code 4: Error: boom", failing.display()),
    "--version exited with code 4: Error: boom",
  );
  assert_unusable_setting(
    &printed,
    &format!("lsp.typescript_command ({}): --version printed no version number: \"hello\"", garbage.display()),
    "--version printed no version number: \"hello\"",
  );
}

#[test]
fn should_fail_hard_on_a_hanging_configured_command() {
  let sandbox = Sandbox::exclusive();
  sandbox.path_script("typescript-language-server", "echo 4.3.4");
  let hanging = script(&sandbox.root().join("custom/hang"), "sleep 30");
  let mut probe = sandbox.probe();
  probe.version_timeout = Duration::from_secs(2);

  let discovery = discover_typescript_with(
    &TypeScriptDiscovery {
      command: Some(&argv(&[&hanging])),
      repo_roots: &[],
      download: false,
    },
    &probe,
  );

  assert_unusable_setting(
    &discovery,
    &format!("lsp.typescript_command ({}): --version did not answer within 2s", hanging.display()),
    "--version did not answer within 2s",
  );
}

#[test]
fn should_find_the_server_on_path_and_ignore_relative_path_entries() {
  let sandbox = Sandbox::new();
  let on_path = sandbox.path_script("typescript-language-server", "echo 4.3.4");
  let mut probe = sandbox.probe();
  probe.path = Some(env::join_paths([PathBuf::from(""), PathBuf::from("bin"), sandbox.bin()]).unwrap());

  let discovery = discover_typescript_with(
    &TypeScriptDiscovery {
      command: None,
      repo_roots: &[],
      download: false,
    },
    &probe,
  );

  assert_eq!(
    outcome(&discovery, "lsp.typescript_command"),
    &StepOutcome::Skipped {
      reason: "lsp.typescript_command is not set".to_owned()
    }
  );
  assert_eq!(
    outcome(&discovery, "typescript-language-server on PATH"),
    &StepOutcome::Found {
      detail: format!("{} (version 4.3.4)", on_path.display())
    }
  );
  let server = found(discovery);
  assert_eq!(server.command.program, on_path);
  assert_eq!(server.command.args, [OsString::from("--stdio")]);
}

#[test]
fn should_skip_a_path_entry_that_is_not_executable() {
  let sandbox = Sandbox::new();
  let path = sandbox.path_script("typescript-language-server", "echo 4.3.4");
  fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

  let discovery = discover(&sandbox, None, &[], false);

  assert_eq!(
    missing_reason(&discovery, "typescript-language-server on PATH"),
    "typescript-language-server is not on PATH"
  );
  assert!(matches!(discovery.result, Err(LspError::Unavailable(_))));
}

#[test]
fn should_search_repo_node_modules_in_registry_order() {
  let sandbox = Sandbox::new();
  let first = sandbox.repo("first repo");
  let second = sandbox.repo("second");
  let third = sandbox.repo("third");
  let in_second = script(&second.join("node_modules/.bin/typescript-language-server"), "echo 4.9.0");
  script(&third.join("node_modules/.bin/typescript-language-server"), "echo 3.0.0");
  let roots = [first.clone(), second.clone(), third.clone()];

  let discovery = discover(&sandbox, None, &roots, false);

  let label = |root: &Path| root.join("node_modules/.bin/typescript-language-server").display().to_string();
  assert_eq!(missing_reason(&discovery, &label(&first)), "no executable there");
  assert_eq!(
    outcome(&discovery, &label(&second)),
    &StepOutcome::Found {
      detail: format!("{} (version 4.9.0)", in_second.display())
    }
  );
  assert_eq!(skipped_reason(&discovery, &label(&third)), "an earlier candidate was found");
  assert_eq!(found(discovery).command, installed_command(&sandbox, &in_second));
}

#[test]
fn should_deduplicate_repo_roots() {
  let sandbox = Sandbox::new();
  let repo = sandbox.repo("only");

  let discovery = discover(&sandbox, None, &[repo.clone(), repo.clone()], false);

  assert_eq!(labels(&discovery).iter().filter(|label| label.starts_with(&repo.display().to_string())).count(), 1);
}

#[test]
fn should_time_out_a_hanging_version_probe_without_leaving_an_orphan() {
  let sandbox = Sandbox::exclusive();
  let pid_file = sandbox.root().join("hung.pid");
  let hanging = vec!["/bin/sh".to_owned(), "-c".to_owned(), format!("/bin/sleep 30 &\necho $! > '{}'\nwait", pid_file.display())];
  let mut probe = sandbox.probe();
  probe.version_timeout = Duration::from_secs(3);
  let started = Instant::now();

  let discovery = discover_typescript_with(
    &TypeScriptDiscovery {
      command: Some(&hanging),
      repo_roots: &[],
      download: false,
    },
    &probe,
  );

  assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
  assert_eq!(missing_reason(&discovery, "lsp.typescript_command"), "--version did not answer within 3s");
  let grandchild: u32 = fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
  std::thread::sleep(Duration::from_millis(200));
  assert!(!alive(grandchild), "orphan {grandchild} survived");
}

#[test]
fn should_report_failing_empty_and_garbage_version_output_and_end_unavailable() {
  for (body, expected) in [
    ("echo 'Error: Cannot find module' >&2; exit 1", "--version exited with code 1: Error: Cannot find module"),
    ("echo 'oops'", "--version printed no version number: \"oops\""),
    ("exit 0", "--version printed nothing"),
    ("echo 5.0.0; exit 2", "--version exited with code 2"),
    ("kill -9 $$", "--version was killed by a signal"),
  ] {
    let sandbox = Sandbox::new();
    sandbox.path_script("typescript-language-server", body);

    let discovery = discover(&sandbox, None, &[], false);

    assert_eq!(missing_reason(&discovery, "typescript-language-server on PATH"), expected, "{body}");
    assert_eq!(discovery.result.unwrap_err().to_string(), HINT, "{body}");
  }
}

#[test]
fn should_end_with_the_exact_hint_and_record_every_candidate_when_nothing_is_found() {
  let sandbox = Sandbox::new();
  let repo = sandbox.repo("web");

  let discovery = discover(&sandbox, None, std::slice::from_ref(&repo), false);

  assert_eq!(
    discovery.steps,
    vec![
      step(
        "lsp.typescript_command",
        StepOutcome::Skipped {
          reason: "lsp.typescript_command is not set".to_owned()
        }
      ),
      step(
        "typescript-language-server on PATH",
        StepOutcome::Missing {
          reason: "typescript-language-server is not on PATH".to_owned()
        }
      ),
      step(
        &repo.join("node_modules/.bin/typescript-language-server").display().to_string(),
        StepOutcome::Missing {
          reason: "no executable there".to_owned()
        }
      ),
      step(
        "npx cache (npx --no-install typescript-language-server)",
        StepOutcome::Missing {
          reason: "npx is not on PATH".to_owned()
        }
      ),
      step(
        "download (typescript-language-server@5, typescript@5)",
        StepOutcome::Skipped {
          reason: "download is off (lsp.download = false)".to_owned()
        }
      ),
    ]
  );
  assert!(matches!(&discovery.result, Err(LspError::Unavailable(message)) if message == HINT));
}

#[test]
fn should_record_a_project_without_typescript_repos() {
  let sandbox = Sandbox::new();

  let discovery = discover(&sandbox, None, &[], false);

  assert_eq!(
    skipped_reason(&discovery, "node_modules/.bin of the TypeScript repos"),
    "the project has no TypeScript repos"
  );
}

fn step(label: &str, outcome: StepOutcome) -> atlas_lsp::DiscoveryStep {
  atlas_lsp::DiscoveryStep { label: label.to_owned(), outcome }
}

#[test]
fn should_use_the_npx_cache_without_the_network() {
  let sandbox = Sandbox::new();
  let npx = sandbox.path_script(
    "npx",
    "if [ \"$1 $2 $3\" = '--no-install typescript-language-server --version' ]; then echo 5.2.0; else exit 9; fi",
  );
  sandbox.path_script("npm", "exit 1");

  let discovery = discover(&sandbox, None, &[], true);

  assert_eq!(
    skipped_reason(&discovery, "download (typescript-language-server@5, typescript@5)"),
    "an earlier candidate was found"
  );
  let server = found(discovery);
  assert_eq!(server.launch, Launch::Installed);
  assert_eq!(server.server_version, "5.2.0");
  assert_eq!(
    server.command,
    ServerCommand::new(npx)
      .arg("--no-install")
      .arg("typescript-language-server")
      .arg("--stdio")
      .env("PATH", sandbox.bin())
      .cwd(env::temp_dir())
  );
}

#[test]
fn should_download_through_npx_only_after_npm_ping_answers() {
  let sandbox = Sandbox::new();
  let npx = sandbox.path_script("npx", "exit 1");
  sandbox.path_script("npm", "[ \"$1\" = ping ] && [ \"$2\" = --fetch-timeout ] && [ \"$3\" = 5000 ] && exit 0; exit 3");

  let discovery = discover(&sandbox, None, &[], true);

  assert!(matches!(
    outcome(&discovery, "download (typescript-language-server@5, typescript@5)"),
    StepOutcome::Found { .. }
  ));
  let server = found(discovery);
  assert_eq!(server.launch, Launch::Download);
  assert_eq!(server.server_version, "typescript-language-server@5 (fetched on first start)");
  let expected = ServerCommand::new(npx)
    .arg("--yes")
    .arg("-p")
    .arg("typescript-language-server@5")
    .arg("-p")
    .arg("typescript@5")
    .arg("typescript-language-server")
    .arg("--stdio")
    .env("PATH", sandbox.bin())
    .cwd(env::temp_dir());
  assert_eq!(server.command, expected);
  assert_eq!(
    server.tsserver,
    Some(TsServerChoice {
      path: None,
      version: "typescript@5 (pinned by the download command)".to_owned(),
      source: TsServerSource::Bundled
    })
  );
}

#[test]
fn should_say_download_is_impossible_when_npm_ping_fails_hangs_or_is_missing() {
  let label = "download (typescript-language-server@5, typescript@5)";
  let sandbox = Sandbox::exclusive();
  sandbox.path_script("npx", "exit 1");
  let failing = sandbox.path_script("npm", "echo 'npm error code ENOTFOUND' >&2; exit 1");

  let failed = discover(&sandbox, None, &[], true);

  assert_eq!(
    missing_reason(&failed, label),
    "download is on but the npm registry is unreachable (npm ping exited with code 1: npm error code ENOTFOUND)"
  );
  assert_eq!(failed.result.unwrap_err().to_string(), HINT);

  script(&failing, "sleep 30");
  let mut probe = sandbox.probe();
  probe.ping_timeout = Duration::from_secs(2);
  let hung = discover_typescript_with(
    &TypeScriptDiscovery {
      command: None,
      repo_roots: &[],
      download: true,
    },
    &probe,
  );
  assert_eq!(missing_reason(&hung, label), "download is on but the npm registry did not answer npm ping within 2s");
  assert!(matches!(hung.result, Err(LspError::Unavailable(_))));

  fs::remove_file(&failing).unwrap();
  let no_npm = discover(&sandbox, None, &[], true);
  assert_eq!(missing_reason(&no_npm, label), "download is on but npm is not on PATH");

  fs::remove_file(sandbox.bin().join("npx")).unwrap();
  let no_npx = discover(&sandbox, None, &[], true);
  assert_eq!(missing_reason(&no_npx, label), "download is on but npx is not on PATH");
}

#[test]
fn should_not_download_when_download_is_off_even_if_npm_answers() {
  let sandbox = Sandbox::new();
  sandbox.path_script("npx", "exit 1");
  sandbox.path_script("npm", "exit 0");

  let discovery = discover(&sandbox, None, &[], false);

  assert_eq!(
    skipped_reason(&discovery, "download (typescript-language-server@5, typescript@5)"),
    "download is off (lsp.download = false)"
  );
  assert_eq!(discovery.result.unwrap_err().to_string(), HINT);
}

#[test]
fn should_pick_the_first_workspace_typescript_below_7_and_pass_its_tsserver() {
  let sandbox = Sandbox::new();
  sandbox.path_script("typescript-language-server", "echo 4.3.4");
  let first = sandbox.repo("first");
  let second = sandbox.repo("second");
  let third = sandbox.repo("third");
  let seven = typescript(&first, Some(&version_json("7.0.0")), false);
  let five = typescript(&second, Some(&version_json("5.4.2")), true);
  typescript(&third, Some(&version_json("4.9.5")), true);

  let discovery = discover(&sandbox, None, &[first.clone(), second.clone(), third.clone()], false);

  let label = |root: &Path| format!("tsserver in {}", root.display());
  assert_eq!(skipped_reason(&discovery, &label(&first)), "typescript 7.0.0 has no tsserver");
  assert_eq!(
    outcome(&discovery, &label(&second)),
    &StepOutcome::Found {
      detail: format!("typescript 5.4.2 at {}", five.display())
    }
  );
  assert_eq!(skipped_reason(&discovery, &label(&third)), "an earlier repo supplies tsserver");
  assert!(!seven.exists());
  assert_eq!(
    found(discovery).tsserver,
    Some(TsServerChoice {
      path: Some(five),
      version: "5.4.2".to_owned(),
      source: TsServerSource::Workspace { repo_root: second }
    })
  );
}

#[test]
fn should_treat_a_missing_or_unparsable_typescript_manifest_as_an_unknown_version() {
  let sandbox = Sandbox::new();
  sandbox.path_script("typescript-language-server", "echo 4.3.4");
  let no_manifest = sandbox.repo("no-manifest");
  let broken = sandbox.repo("broken");
  let nonsense = sandbox.repo("nonsense");
  let no_version = sandbox.repo("no-version");
  let empty = sandbox.repo("empty");
  typescript(&no_manifest, None, true);
  typescript(&broken, Some("{ not json"), true);
  typescript(&nonsense, Some(&version_json("next")), true);
  typescript(&no_version, Some("{\"name\":\"typescript\"}"), true);
  fs::create_dir_all(empty.join("node_modules")).unwrap();
  let roots = [no_manifest.clone(), broken.clone(), nonsense.clone(), no_version.clone(), empty.clone()];

  let discovery = discover(&sandbox, None, &roots, false);

  let label = |root: &Path| format!("tsserver in {}", root.display());
  let unreadable = "typescript is installed but its package.json has no readable version";
  assert_eq!(skipped_reason(&discovery, &label(&no_manifest)), unreadable);
  assert_eq!(skipped_reason(&discovery, &label(&broken)), unreadable);
  assert_eq!(
    skipped_reason(&discovery, &label(&nonsense)),
    "typescript version \"next\" cannot be read as a version number"
  );
  assert_eq!(skipped_reason(&discovery, &label(&no_version)), unreadable);
  assert_eq!(missing_reason(&discovery, &label(&empty)), "typescript is not installed there");
  assert_eq!(
    found(discovery).tsserver,
    Some(TsServerChoice {
      path: None,
      version: "bundled typescript (version not verified)".to_owned(),
      source: TsServerSource::Bundled
    })
  );
}

#[test]
fn should_skip_a_workspace_typescript_without_tsserver_js() {
  let sandbox = Sandbox::new();
  sandbox.path_script("typescript-language-server", "echo 4.3.4");
  let repo = sandbox.repo("web");
  typescript(&repo, Some(&version_json("5.0.0")), false);

  let discovery = discover(&sandbox, None, std::slice::from_ref(&repo), false);

  assert_eq!(
    missing_reason(&discovery, &format!("tsserver in {}", repo.display())),
    "typescript 5.0.0 has no lib/tsserver.js"
  );
  assert_eq!(found(discovery).tsserver.unwrap().source, TsServerSource::Bundled);
}

#[test]
fn should_refuse_a_typescript_7_only_workspace_when_the_server_has_no_verifiable_bundled_typescript() {
  let sandbox = Sandbox::new();
  sandbox.path_script("typescript-language-server", "echo 4.3.4");
  let repo = sandbox.repo("web");
  typescript(&repo, Some(&version_json("7.1.0-beta.1")), false);

  let discovery = discover(&sandbox, None, &[repo], false);

  assert!(missing_reason(&discovery, "tsserver bundled with the server").starts_with("no typescript below 7 resolves from "));
  assert_eq!(discovery.result.unwrap_err().to_string(), TS7);
}

#[test]
fn should_fall_back_to_a_verified_bundled_typescript_beside_the_server() {
  let sandbox = Sandbox::new();
  let global = sandbox.root().join("global/lib/node_modules");
  let server = script(&global.join("typescript-language-server/lib/cli.sh"), "echo 4.3.4");
  let bundled_tsserver = typescript(&global.join("typescript-language-server"), Some(&version_json("5.6.3")), true);
  symlink(&server, sandbox.bin().join("typescript-language-server")).unwrap();
  let repo = sandbox.repo("web");
  typescript(&repo, Some(&version_json("7.0.0")), false);

  let discovery = discover(&sandbox, None, &[repo], false);

  let real_tsserver = fs::canonicalize(&bundled_tsserver).unwrap();
  let choice = found(discovery).tsserver.unwrap();
  assert_eq!(
    choice,
    TsServerChoice {
      path: Some(real_tsserver),
      version: "5.6.3".to_owned(),
      source: TsServerSource::Bundled
    }
  );
}

#[test]
fn should_find_a_hoisted_bundled_typescript_and_leave_the_path_unset_without_workspace_installs() {
  let sandbox = Sandbox::new();
  let global = sandbox.root().join("global/lib/node_modules");
  let server = script(&global.join("typescript-language-server/lib/cli.sh"), "echo 4.3.4");
  typescript(&global, Some(&version_json("5.8.0")), true);
  symlink(&server, sandbox.bin().join("typescript-language-server")).unwrap();

  let choice = found(discover(&sandbox, None, &[], false)).tsserver.unwrap();

  assert_eq!(
    choice,
    TsServerChoice {
      path: None,
      version: "5.8.0".to_owned(),
      source: TsServerSource::Bundled
    }
  );
}

#[test]
fn should_not_trust_a_bundled_typescript_7_beside_the_server() {
  let sandbox = Sandbox::new();
  let global = sandbox.root().join("global/lib/node_modules");
  let server = script(&global.join("typescript-language-server/lib/cli.sh"), "echo 4.3.4");
  typescript(&global, Some(&version_json("7.0.0")), true);
  symlink(&server, sandbox.bin().join("typescript-language-server")).unwrap();
  let repo = sandbox.repo("web");
  typescript(&repo, Some(&version_json("7.0.0")), false);

  let discovery = discover(&sandbox, None, &[repo], false);

  assert_eq!(discovery.result.unwrap_err().to_string(), TS7);
}

#[test]
fn should_report_the_same_steps_for_the_same_inputs() {
  let sandbox = Sandbox::new();
  sandbox.path_script("typescript-language-server", "echo 4.3.4");
  let repo = sandbox.repo("web");
  typescript(&repo, Some(&version_json("5.4.2")), true);
  let roots = [repo];

  let first = discover(&sandbox, None, &roots, true);
  let second = discover(&sandbox, None, &roots, true);

  assert_eq!(first.steps, second.steps);
  assert_eq!(found(first), found(second));
}

#[test]
fn should_describe_an_installed_rust_analyzer() {
  let server = rust_analyzer_server(PathBuf::from("/opt/ra/rust-analyzer"), "rust-analyzer 1.85.0".to_owned());

  assert_eq!(
    server,
    Discovered {
      command: ServerCommand::new("/opt/ra/rust-analyzer"),
      launch: Launch::Installed,
      server_version: "rust-analyzer 1.85.0".to_owned(),
      tsserver: None,
    }
  );
}
