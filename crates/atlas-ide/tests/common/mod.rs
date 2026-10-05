#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use atlas_core::{ProjectName, RepoName};
use atlas_engine::{Engine, EngineOptions};
use atlas_ide::{IdeError, IdeMode, IdeOptions, IdeResult, IdeService, IdeSettings, LaunchRequest, LaunchSpec, Launcher, TsServer, TsServerOrigin};
use atlas_lsp::{Launch, ServerCommand, ServerKind, SyncLimits, canonical_path, uri_from_path};
use atlas_project::{AtlasHome, Project, ProjectSettings, RepoConfig};
use atlas_query::{RenderText, Response, render_text};
use serde_json::{Value, json};
use tempfile::TempDir;

pub const FAKE: &str = env!("CARGO_BIN_EXE_atlas-ide-fake-lsp");
pub const WAIT: Duration = Duration::from_secs(20);

#[derive(Default)]
pub struct ScriptedLauncher {
  specs: Mutex<HashMap<ServerKind, LaunchSpec>>,
}

impl ScriptedLauncher {
  pub fn set(&self, kind: ServerKind, spec: LaunchSpec) {
    self.specs.lock().unwrap().insert(kind, spec);
  }
}

impl Launcher for ScriptedLauncher {
  fn launch(&self, request: &LaunchRequest<'_>) -> IdeResult<LaunchSpec> {
    self.specs.lock().unwrap().get(&request.kind).cloned().ok_or_else(|| IdeError::Unavailable {
      server: request.kind.name().to_owned(),
      reason: "typescript-language-server not found. Install it (npm i -g typescript-language-server typescript@5)".to_owned(),
      alternative: None,
    })
  }
}

pub struct Fixture {
  pub scratch: TempDir,
  pub launcher: Arc<ScriptedLauncher>,
  pub ide: IdeService,
}

pub fn settings() -> IdeSettings {
  IdeSettings {
    ready_timeout_secs: 20,
    check_timeout_secs: 20,
    ..IdeSettings::default()
  }
}

pub fn limits() -> SyncLimits {
  SyncLimits {
    settle: Duration::from_millis(50),
    quiescence: Duration::from_millis(400),
    ..SyncLimits::default()
  }
}

impl Fixture {
  pub fn new(repos: &[(&str, &[(&str, &str)])]) -> Self {
    let scratch = tempfile::tempdir().unwrap();
    let configs = repos
      .iter()
      .map(|(name, files)| {
        let root = scratch.path().join("tree").join(name);
        for (path, content) in *files {
          let target = root.join(path);
          fs::create_dir_all(target.parent().unwrap()).unwrap();
          fs::write(target, content).unwrap();
        }
        repo_config(name, root)
      })
      .collect();
    Self::open(scratch, configs, settings())
  }

  pub fn copied(group: &str, repos: &[&str]) -> Self {
    let scratch = tempfile::tempdir().unwrap();
    let configs = repos
      .iter()
      .map(|name| {
        let root = scratch.path().join("tree").join(name);
        copy_dir(&fixtures_root().join(group).join(name), &root);
        repo_config(name, root)
      })
      .collect();
    Self::open(scratch, configs, settings())
  }

  pub fn open(scratch: TempDir, repos: Vec<RepoConfig>, settings: IdeSettings) -> Self {
    let project = Project {
      name: ProjectName::new("demo").unwrap(),
      settings: ProjectSettings::default(),
      repos,
    };
    let engine = Engine::open(&project, &AtlasHome::rooted_at(scratch.path().join("home")), EngineOptions::default()).unwrap();
    let launcher = Arc::new(ScriptedLauncher::default());
    let options = IdeOptions {
      sync: limits(),
      ..IdeOptions::new(IdeMode::OneShot, settings, scratch.path().join("cache"))
    };
    let ide = IdeService::new(Arc::new(engine), launcher.clone(), options);
    Self { scratch, launcher, ide }
  }

  pub fn root(&self, repo: &str) -> PathBuf {
    canonical_path(&self.scratch.path().join("tree").join(repo)).unwrap()
  }

  pub fn file(&self, repo: &str, path: &str) -> PathBuf {
    self.root(repo).join(path)
  }

  pub fn uri(&self, repo: &str, path: &str) -> String {
    uri_from_path(&self.file(repo, path)).unwrap()
  }

  pub fn script(&self, name: &str, steps: &[Value]) -> ServerCommand {
    let lines: Vec<String> = steps.iter().map(Value::to_string).collect();
    let script = self.scratch.path().join(format!("{name}.jsonl"));
    fs::write(&script, lines.join("\n")).unwrap();
    ServerCommand::new(FAKE).arg(script)
  }

  pub fn counted(&self, name: &str, steps: &[Value]) -> ServerCommand {
    let script = self.script(name, steps).args.remove(0);
    let count = self.scratch.path().join(format!("{name}.spawns"));
    let line = format!("echo spawn >> '{}'; exec '{FAKE}' '{}'", count.display(), Path::new(&script).display());
    ServerCommand::new("sh").arg("-c").arg(line)
  }

  pub fn spawns(&self, name: &str) -> usize {
    fs::read_to_string(self.scratch.path().join(format!("{name}.spawns")))
      .map(|text| text.lines().count())
      .unwrap_or(0)
  }

  pub fn serve(&self, kind: ServerKind, command: ServerCommand) {
    self.launcher.set(kind, launch(kind, command));
  }

  pub fn record(&self, name: &str) -> Value {
    json!({"record": self.scratch.path().join(format!("{name}.log"))})
  }

  pub fn recorded(&self, name: &str) -> Vec<Value> {
    fs::read_to_string(self.scratch.path().join(format!("{name}.log")))
      .unwrap_or_default()
      .lines()
      .map(|line| serde_json::from_str(line).unwrap())
      .collect()
  }
}

pub fn launch(kind: ServerKind, command: ServerCommand) -> LaunchSpec {
  LaunchSpec {
    command,
    launch: Launch::Installed,
    server_version: "0.0-discovered".to_owned(),
    tsserver: match kind {
      ServerKind::TypeScript => Some(TsServer {
        path: None,
        version: "5.9.3".to_owned(),
        origin: TsServerOrigin::Bundled,
      }),
      ServerKind::RustAnalyzer => None,
    },
  }
}

fn repo_config(name: &str, root: PathBuf) -> RepoConfig {
  RepoConfig {
    name: RepoName::new(name).unwrap(),
    path: root,
    include: Vec::new(),
    exclude: Vec::new(),
    languages: None,
  }
}

pub fn fixtures_root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

pub fn copy_dir(from: &Path, to: &Path) {
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

pub fn capabilities(extra: Value) -> Value {
  let mut capabilities = json!({
    "textDocumentSync": 1,
    "referencesProvider": true,
    "definitionProvider": true,
    "implementationProvider": true,
    "documentSymbolProvider": true
  });
  if let (Some(base), Some(extra)) = (capabilities.as_object_mut(), extra.as_object()) {
    base.extend(extra.clone());
  }
  capabilities
}

pub fn initialize(name: &str, capabilities: Value) -> Vec<Value> {
  vec![
    json!({"recv": {"method": "initialize"}}),
    json!({"reply": {"method": "initialize", "result": {"capabilities": capabilities, "serverInfo": {"name": name, "version": "5.1.3"}}}}),
    json!({"recv": {"method": "initialized"}}),
  ]
}

pub fn ts_ready(capabilities: Value) -> Vec<Value> {
  ts_ready_with("5.9.3", capabilities)
}

pub fn ts_ready_with(version: &str, capabilities: Value) -> Vec<Value> {
  let mut steps = initialize("typescript-language-server", capabilities);
  steps.push(json!({"notify": {"method": "$/typescriptVersion", "params": {"version": version, "source": "bundled"}}}));
  steps
}

pub fn rust_ready(capabilities: Value) -> Vec<Value> {
  let mut steps = initialize("rust-analyzer", capabilities);
  steps.push(json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": true}}}));
  steps
}

pub fn with(mut steps: Vec<Value>, more: impl IntoIterator<Item = Value>) -> Vec<Value> {
  steps.extend(more);
  steps
}

pub fn range(start: (u32, u32), end: (u32, u32)) -> Value {
  json!({"start": {"line": start.0, "character": start.1}, "end": {"line": end.0, "character": end.1}})
}

pub fn location(uri: &str, start: (u32, u32), end: (u32, u32)) -> Value {
  json!({"uri": uri, "range": range(start, end)})
}

pub fn utf16_position(text: &str, needle: &str, nth: usize) -> (u32, u32) {
  let offset = text.match_indices(needle).nth(nth).unwrap().0;
  let before = &text[..offset];
  let line_start = before.rfind(['\n', '\r']).map_or(0, |at| at + 1);
  let mut line = 0;
  let mut chars = before.chars().peekable();
  while let Some(character) = chars.next() {
    match character {
      '\n' => line += 1,
      '\r' => {
        chars.next_if_eq(&'\n');
        line += 1;
      }
      _ => {}
    }
  }
  (line, before[line_start..].encode_utf16().count() as u32)
}

pub fn text<T: RenderText>(response: &Response<T>) -> String {
  let rendered = render_text(response);
  rendered.lines().skip(1).map(|line| format!("{line}\n")).collect()
}

pub fn wait_until(mut done: impl FnMut() -> bool) -> bool {
  let deadline = Instant::now() + WAIT;
  while !done() {
    if Instant::now() > deadline {
      return false;
    }
    thread::sleep(Duration::from_millis(20));
  }
  true
}

pub fn methods(recorded: &[Value]) -> Vec<String> {
  recorded.iter().filter_map(|message| message["method"].as_str().map(str::to_owned)).collect()
}
