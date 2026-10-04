#![allow(dead_code)]

pub mod expected;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

pub const ACME_REPOS: [&str; 4] = ["acme-api", "acme-sdk", "acme-web", "acme-billing"];
pub const GLOBEX_REPOS: [&str; 2] = ["globex-api", "globex-worker"];

pub fn fixtures() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

pub fn copy_dir(from: &Path, to: &Path) {
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

pub struct Run {
  pub args: Vec<String>,
  pub code: i32,
  pub stdout: String,
  pub stderr: String,
}

impl fmt::Display for Run {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      formatter,
      "`code-atlas {}` exited {}\n--- stdout\n{}\n--- stderr\n{}",
      self.args.join(" "),
      self.code,
      self.stdout,
      self.stderr
    )
  }
}

impl Run {
  pub fn ok(self) -> Self {
    self.expect(0)
  }

  pub fn expect(self, code: i32) -> Self {
    assert_eq!(self.code, code, "{self}");
    self
  }

  pub fn json(&self) -> Value {
    serde_json::from_str(&self.stdout).unwrap_or_else(|error| panic!("stdout is not JSON ({error}): {self}"))
  }

  pub fn answer(&self) -> Value {
    self.json()["answer"].clone()
  }

  pub fn assert_no_crash(&self) {
    assert!((0..=4).contains(&self.code), "process crashed or used an undocumented exit code: {self}");
    assert!(!self.stderr.contains("panicked"), "process panicked: {self}");
  }
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

  pub fn db(&self, project: &str) -> PathBuf {
    self.home.path().join("data").join(project).join("atlas.db")
  }

  pub fn command(&self, cwd: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_code-atlas"));
    command
      .args(args)
      .current_dir(cwd)
      .env("CODE_ATLAS_HOME", self.home.path())
      .env_remove("RUST_LOG")
      .stdin(Stdio::null());
    command
  }

  pub fn run(&self, cwd: &Path, args: &[&str]) -> Run {
    self.run_with(cwd, args, &[])
  }

  pub fn run_with(&self, cwd: &Path, args: &[&str], envs: &[(&str, &OsStr)]) -> Run {
    let output = self.command(cwd, args).envs(envs.iter().copied()).output().unwrap();
    Run {
      args: args.iter().map(|arg| (*arg).to_owned()).collect(),
      code: output.status.code().unwrap_or(-1),
      stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
      stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
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
    self.run(&self.work(), &args).ok();
  }

  pub fn acme(&self) -> PathBuf {
    let repos: Vec<PathBuf> = ACME_REPOS.iter().map(|repo| self.copy_repo("acme", repo)).collect();
    self.create_project("acme", &repos);
    self.work().join("acme")
  }

  pub fn globex(&self) -> PathBuf {
    let repos: Vec<PathBuf> = GLOBEX_REPOS.iter().map(|repo| self.copy_repo("globex", repo)).collect();
    self.create_project("globex", &repos);
    self.work().join("globex")
  }

  pub fn repos(&self, project: &str) -> Vec<(String, PathBuf)> {
    let shown = self.run(&self.work(), &["project", "show", project, "--json"]).ok().json();
    shown["repo"]
      .as_array()
      .unwrap()
      .iter()
      .map(|repo| (repo["name"].as_str().unwrap().to_owned(), PathBuf::from(repo["path"].as_str().unwrap())))
      .collect()
  }
}

pub struct Fresh {
  sandbox: Sandbox,
  cwd: PathBuf,
}

impl Fresh {
  pub fn index_of(source: &Sandbox, project: &str) -> Self {
    let sandbox = Sandbox::new();
    let specs: Vec<String> = source.repos(project).iter().map(|(name, path)| format!("{}={name}", path.display())).collect();
    let mut args = vec!["project", "create", project];
    for spec in &specs {
      args.extend(["--repo", spec.as_str()]);
    }
    let cwd = sandbox.work();
    sandbox.run(&cwd, &args).ok();
    let indexed = sandbox.run(&cwd, &["index", "--full", "--project", project]);
    assert!([0, 3].contains(&indexed.code), "{indexed}");
    Self { sandbox, cwd }
  }

  pub fn run(&self, args: &[&str]) -> Run {
    self.sandbox.run(&self.cwd, args)
  }
}

pub fn assert_same_answers(sandbox: &Sandbox, project: &str, queries: &[&[&str]], context: &str) {
  let fresh = Fresh::index_of(sandbox, project);
  let cwd = sandbox.work();
  for query in queries {
    let mut args: Vec<&str> = query.to_vec();
    args.extend(["--json", "--project", project]);
    let incremental = sandbox.run(&cwd, &args);
    let full = fresh.run(&args);
    assert_eq!(incremental.code, full.code, "{context}: exit codes differ\nincremental: {incremental}\nfull: {full}");
    let (left, right) = (incremental.answer(), full.answer());
    if let Some(difference) = first_difference(&left, &right, "answer") {
      panic!(
        "{context}: `{}` differs from a fresh full index at {difference}\nincremental: {incremental}\nfull: {full}",
        query.join(" ")
      );
    }
  }
}

pub fn first_difference(left: &Value, right: &Value, path: &str) -> Option<String> {
  match (left, right) {
    (Value::Object(left), Value::Object(right)) => {
      let mut keys: Vec<&String> = left.keys().chain(right.keys()).collect();
      keys.sort();
      keys.dedup();
      keys.into_iter().find_map(|key| match (left.get(key), right.get(key)) {
        (Some(a), Some(b)) => first_difference(a, b, &format!("{path}.{key}")),
        (a, b) => Some(format!("{path}.{key}: {a:?} vs {b:?}")),
      })
    }
    (Value::Array(left), Value::Array(right)) if left.len() == right.len() => left
      .iter()
      .zip(right)
      .enumerate()
      .find_map(|(index, (a, b))| first_difference(a, b, &format!("{path}[{index}]"))),
    _ if left == right => None,
    _ => Some(format!("{path}: {} vs {}", truncate(left), truncate(right))),
  }
}

fn truncate(value: &Value) -> String {
  let text = value.to_string();
  match text.char_indices().nth(400) {
    Some((cut, _)) => format!("{}...", &text[..cut]),
    None => text,
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
  pub id: String,
  pub short: String,
  pub kind: String,
  pub repo: Option<String>,
  pub path: Option<String>,
}

impl Found {
  pub fn of(value: &Value) -> Option<Self> {
    let object = value.as_object()?;
    let text = |field: &str| object.get(field).and_then(Value::as_str).map(str::to_owned);
    let location = object.get("location").filter(|location| location.is_object());
    let at = |field: &str| location.and_then(|location| location[field].as_str()).map(str::to_owned);
    Some(Self {
      id: text("id")?,
      short: text("short")?,
      kind: text("kind")?,
      repo: at("repo"),
      path: at("path"),
    })
  }
}

pub fn entity_refs(value: &Value) -> Vec<Found> {
  let mut found = Vec::new();
  collect_refs(value, &mut found);
  found
}

fn collect_refs(value: &Value, found: &mut Vec<Found>) {
  match value {
    Value::Object(object) => {
      if object.contains_key("key")
        && let Some(entity) = Found::of(value)
      {
        found.push(entity);
      }
      object.values().for_each(|nested| collect_refs(nested, found));
    }
    Value::Array(items) => items.iter().for_each(|nested| collect_refs(nested, found)),
    _ => {}
  }
}

pub fn shorts(value: &Value) -> Vec<String> {
  entity_refs(value).into_iter().map(|entity| entity.short).collect()
}

pub fn executable(path: &Path, body: &str) {
  fs::write(path, body).unwrap();
  fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

pub struct FakeClaude {
  pub bin: PathBuf,
  pub calls: PathBuf,
}

impl FakeClaude {
  pub fn replying(dir: &Path, reply: &Value) -> Self {
    Self::routing(dir, reply, &[("client call targets", link_reply())])
  }

  pub fn routing(dir: &Path, fallback: &Value, routes: &[(&str, Value)]) -> Self {
    let bin = dir.join("fake-bin");
    fs::create_dir_all(&bin).unwrap();
    let calls = dir.join("claude-calls.log");
    let cases: String = routes
      .iter()
      .map(|(needle, reply)| format!("  *'{needle}'*) cat <<'REPLY'\n{reply}\nREPLY\n  ;;\n"))
      .collect();
    let body = format!(
      "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'fake-claude 1.0.0'; exit 0; fi\nprompt=$(cat)\necho call >> '{}'\ncase \"$prompt\" in\n{cases}  *) cat <<'REPLY'\n{}\nREPLY\n  ;;\nesac\n",
      calls.display(),
      fallback
    );
    executable(&bin.join("claude"), &body);
    Self { bin, calls }
  }

  pub fn path(&self) -> OsString {
    let mut path = self.bin.clone().into_os_string();
    path.push(":/usr/bin:/bin");
    path
  }

  pub fn calls(&self) -> usize {
    fs::read_to_string(&self.calls).map(|log| log.lines().count()).unwrap_or(0)
  }
}

pub fn link_reply() -> Value {
  serde_json::json!({
    "type": "result",
    "is_error": false,
    "result": "",
    "structured_output": { "endpoint_index": null, "confidence": 0.9, "rationale": "scripted by the e2e fake provider: no candidate fits" },
    "usage": { "input_tokens": 120, "output_tokens": 30 }
  })
}

pub fn similarity_reply(verdict: &str, confidence: f64, differences: &[&str]) -> Value {
  serde_json::json!({
    "type": "result",
    "is_error": false,
    "result": "",
    "structured_output": {
      "verdict": verdict,
      "confidence": confidence,
      "rationale": "scripted by the e2e fake provider",
      "behavioural_differences": differences,
      "consolidation_risk": "callers relying on the differing behaviour would break"
    },
    "usage": { "input_tokens": 120, "output_tokens": 30 }
  })
}

pub fn git(dir: &Path, args: &[&str]) {
  let output = Command::new("git")
    .args([
      "-c",
      "user.name=e2e",
      "-c",
      "user.email=e2e@example.invalid",
      "-c",
      "commit.gpgsign=false",
      "-c",
      "init.defaultBranch=main",
    ])
    .args(args)
    .current_dir(dir)
    .env("GIT_CONFIG_GLOBAL", "/dev/null")
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .output()
    .unwrap();
  assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
}

pub fn edit(path: &Path, from: &str, to: &str) {
  let source = fs::read_to_string(path).unwrap();
  assert!(source.contains(from), "{} does not contain {from:?}", path.display());
  fs::write(path, source.replace(from, to)).unwrap();
}
