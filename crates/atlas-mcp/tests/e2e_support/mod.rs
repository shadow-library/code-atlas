#![allow(dead_code)]

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;
use tempfile::TempDir;
use tokio::process::Child;

pub const ACME_REPOS: [&str; 4] = ["acme-api", "acme-sdk", "acme-web", "acme-billing"];
pub const CALL_TIMEOUT: Duration = Duration::from_secs(120);

pub fn fixtures() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

// The binary lives in atlas-cli, so Cargo gives this crate no CARGO_BIN_EXE_catl; build it into the same target dir and profile.
pub fn binary() -> &'static Path {
  static BINARY: OnceLock<PathBuf> = OnceLock::new();
  BINARY.get_or_init(|| {
    if let Some(path) = std::env::var_os("CODE_ATLAS_BIN") {
      return PathBuf::from(path);
    }
    let test_exe = std::env::current_exe().unwrap();
    let profile_dir = test_exe.parent().and_then(Path::parent).unwrap().to_path_buf();
    let target_dir = profile_dir.parent().unwrap();
    let profile = match profile_dir.file_name().and_then(OsStr::to_str) {
      Some("debug") | None => "dev".to_owned(),
      Some(other) => other.to_owned(),
    };
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
      .args(["build", "--quiet", "-p", "atlas-cli", "--bin", "catl", "--profile", &profile, "--target-dir"])
      .arg(target_dir)
      .current_dir(env!("CARGO_MANIFEST_DIR"))
      .status()
      .unwrap();
    assert!(status.success(), "building catl failed");
    fs::canonicalize(profile_dir.join("catl")).unwrap()
  })
}

fn copy_dir(from: &Path, to: &Path) {
  fs::create_dir_all(to).unwrap();
  for entry in fs::read_dir(from).unwrap() {
    let entry = entry.unwrap();
    let target = to.join(entry.file_name());
    match entry.file_type().unwrap().is_dir() {
      true => copy_dir(&entry.path(), &target),
      false => {
        fs::copy(entry.path(), &target).unwrap();
      }
    }
  }
}

pub struct CliRun {
  pub code: i32,
  pub stdout: String,
  pub stderr: String,
}

pub struct Sandbox {
  home: TempDir,
  work: TempDir,
}

impl Default for Sandbox {
  fn default() -> Self {
    Self::new()
  }
}

impl Sandbox {
  pub fn new() -> Self {
    Self {
      home: tempfile::tempdir().unwrap(),
      work: tempfile::tempdir().unwrap(),
    }
  }

  pub fn home(&self) -> &Path {
    self.home.path()
  }

  pub fn work(&self) -> PathBuf {
    fs::canonicalize(self.work.path()).unwrap()
  }

  pub fn cli(&self, cwd: &Path, args: &[&str]) -> CliRun {
    let output = without_atlas_settings(Command::new(binary()))
      .args(args)
      .current_dir(cwd)
      .env("CODE_ATLAS_HOME", self.home())
      .env_remove("RUST_LOG")
      .stdin(Stdio::null())
      .output()
      .unwrap();
    CliRun {
      code: output.status.code().unwrap_or(-1),
      stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
      stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
  }

  pub fn cli_ok(&self, cwd: &Path, args: &[&str]) -> CliRun {
    let run = self.cli(cwd, args);
    assert_eq!(run.code, 0, "catl {args:?} failed\n{}\n{}", run.stdout, run.stderr);
    run
  }

  pub fn copy_repo(&self, group: &str, repo: &str) -> PathBuf {
    let target = self.work().join(group).join(repo);
    copy_dir(&fixtures().join(group).join(repo), &target);
    target
  }

  pub fn create_project(&self, name: &str, repos: &[PathBuf]) {
    let specs: Vec<String> = repos.iter().map(|repo| repo.display().to_string()).collect();
    let mut args = vec!["project", "create", name];
    for spec in &specs {
      args.extend(["--repo", spec.as_str()]);
    }
    self.cli_ok(&self.work(), &args);
  }

  pub fn acme(&self) -> PathBuf {
    let repos: Vec<PathBuf> = ACME_REPOS.iter().map(|repo| self.copy_repo("acme", repo)).collect();
    self.create_project("acme", &repos);
    self.work().join("acme")
  }

  pub async fn serve(&self, cwd: &Path, args: &[&str]) -> Server {
    Server::spawn(binary(), args, cwd, self.home()).await
  }
}

fn without_atlas_settings(mut command: Command) -> Command {
  for (name, _) in std::env::vars_os() {
    if name.to_string_lossy().starts_with("CODE_ATLAS_") {
      command.env_remove(name);
    }
  }
  command
}

pub struct Server {
  pub client: RunningService<RoleClient, ()>,
  child: Child,
}

impl Server {
  pub async fn spawn(command: &Path, args: &[&str], cwd: &Path, home: &Path) -> Self {
    let mut child = tokio::process::Command::from(without_atlas_settings(Command::new(command)))
      .args(args)
      .current_dir(cwd)
      .env("CODE_ATLAS_HOME", home)
      .env_remove("RUST_LOG")
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .kill_on_drop(true)
      .spawn()
      .unwrap();
    let transport = (child.stdout.take().unwrap(), child.stdin.take().unwrap());
    let client = tokio::time::timeout(CALL_TIMEOUT, ().serve(transport)).await.expect("MCP initialize timed out").unwrap();
    Self { client, child }
  }

  pub async fn call(&self, tool: &'static str, arguments: Value) -> CallToolResult {
    let Value::Object(arguments) = arguments else { panic!("arguments must be an object") };
    let request = CallToolRequestParams::new(tool).with_arguments(arguments);
    tokio::time::timeout(CALL_TIMEOUT, self.client.call_tool(request))
      .await
      .unwrap_or_else(|_| panic!("{tool} timed out"))
      .unwrap()
  }

  pub async fn text(&self, tool: &'static str, arguments: Value) -> String {
    let result = self.call(tool, arguments).await;
    let text = text_of(&result);
    assert_ne!(result.is_error, Some(true), "{tool} failed: {text}");
    text
  }

  pub async fn json(&self, tool: &'static str, mut arguments: Value) -> Value {
    arguments["format"] = Value::from("json");
    let text = self.text(tool, arguments).await;
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{tool} did not return JSON ({error}): {text}"))
  }

  pub async fn stop(mut self) -> ExitStatus {
    self.client.cancel().await.unwrap();
    tokio::time::timeout(CALL_TIMEOUT, self.child.wait())
      .await
      .expect("the MCP server did not exit after stdin closed")
      .unwrap()
  }
}

pub fn text_of(result: &CallToolResult) -> String {
  result
    .content
    .iter()
    .filter_map(|block| block.as_text())
    .map(|text| text.text.as_str())
    .collect::<Vec<_>>()
    .join("\n")
}

pub fn repos_of(value: &Value) -> Vec<String> {
  let mut repos = Vec::new();
  collect_repos(value, &mut repos);
  repos.sort();
  repos.dedup();
  repos
}

fn collect_repos(value: &Value, repos: &mut Vec<String>) {
  match value {
    Value::Object(object) => {
      if object.contains_key("key")
        && let Some(repo) = object.get("location").and_then(|location| location.get("repo")).and_then(Value::as_str)
      {
        repos.push(repo.to_owned());
      }
      object.values().for_each(|nested| collect_repos(nested, repos));
    }
    Value::Array(items) => items.iter().for_each(|nested| collect_repos(nested, repos)),
    _ => {}
  }
}

pub fn shorts(value: &Value) -> Vec<String> {
  let mut found = Vec::new();
  collect_shorts(value, &mut found);
  found
}

fn collect_shorts(value: &Value, found: &mut Vec<String>) {
  match value {
    Value::Object(object) => {
      if object.contains_key("key")
        && let Some(short) = object.get("short").and_then(Value::as_str)
      {
        found.push(short.to_owned());
      }
      object.values().for_each(|nested| collect_shorts(nested, found));
    }
    Value::Array(items) => items.iter().for_each(|nested| collect_shorts(nested, found)),
    _ => {}
  }
}
