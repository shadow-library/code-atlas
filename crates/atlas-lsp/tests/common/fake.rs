use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use atlas_lsp::{Launch, RustAnalyzerSettings, ServerCommand, ServerUnit, TypeScriptSettings, UnitFolder, UnitSettings, canonical_path};
use atlas_test_lsp::fake_lsp_path;
use serde_json::{Value, json};

pub const WAIT: Duration = Duration::from_secs(20);

pub struct Workspace {
  pub dir: tempfile::TempDir,
  pub root: PathBuf,
}

impl Workspace {
  pub fn new() -> Self {
    let dir = tempfile::tempdir().unwrap();
    let root = canonical_path(&dir.path().join("ws")).unwrap();
    fs::create_dir_all(&root).unwrap();
    Self { dir, root }
  }

  pub fn path(&self, name: &str) -> PathBuf {
    canonical_path(&self.dir.path().join(name)).unwrap()
  }

  pub fn write(&self, relative: &str, text: &str) -> PathBuf {
    let path = self.root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
  }

  pub fn script(&self, name: &str, steps: &[Value]) -> ServerCommand {
    let lines: Vec<String> = steps.iter().map(Value::to_string).collect();
    let script = self.path(&format!("{name}.jsonl"));
    fs::write(&script, lines.join("\n")).unwrap();
    ServerCommand::new(fake_lsp_path()).arg(script)
  }

  pub fn counted(&self, name: &str, steps: &[Value]) -> ServerCommand {
    let script = self.script(name, steps).args.remove(0);
    let count = self.path(&format!("{name}.spawns"));
    let line = format!(
      "echo spawn >> '{}'; exec '{}' '{}'",
      count.display(),
      fake_lsp_path().display(),
      Path::new(&script).display()
    );
    ServerCommand::new("sh").arg("-c").arg(line)
  }

  pub fn spawns(&self, name: &str) -> usize {
    fs::read_to_string(self.path(&format!("{name}.spawns"))).map(|text| text.lines().count()).unwrap_or(0)
  }

  pub fn rust_unit(&self, project: &str, command: ServerCommand) -> ServerUnit {
    let settings = RustAnalyzerSettings::new(vec![self.root.join("Cargo.toml")], self.path("target"));
    let mut unit = ServerUnit::new(
      project,
      vec![UnitFolder::new("ws", &self.root)],
      UnitSettings::RustAnalyzer(settings),
      command,
      Launch::Installed,
    );
    unit.timeouts.ready = WAIT;
    unit
  }

  pub fn ts_unit(&self, project: &str, command: ServerCommand) -> ServerUnit {
    let mut unit = ServerUnit::new(
      project,
      vec![UnitFolder::new("ws", &self.root)],
      UnitSettings::TypeScript(TypeScriptSettings::default()),
      command,
      Launch::Installed,
    );
    unit.timeouts.ready = WAIT;
    unit
  }

  pub fn record(&self, name: &str) -> Value {
    json!({"record": self.path(&format!("{name}.log"))})
  }

  pub fn recorded(&self, name: &str) -> Vec<Value> {
    fs::read_to_string(self.path(&format!("{name}.log")))
      .unwrap_or_default()
      .lines()
      .map(|line| serde_json::from_str(line).unwrap())
      .collect()
  }
}

pub fn initialize() -> Vec<Value> {
  vec![
    json!({"recv": {"method": "initialize"}}),
    json!({"reply": {"method": "initialize", "result": {"capabilities": {"positionEncoding": "utf-8", "textDocumentSync": 1}, "serverInfo": {"name": "fake", "version": "0.1"}}}}),
    json!({"recv": {"method": "initialized"}}),
  ]
}

pub fn rust_ready() -> Vec<Value> {
  let mut steps = initialize();
  steps.push(json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": true}}}));
  steps
}

pub fn ts_ready() -> Vec<Value> {
  let mut steps = initialize();
  steps.push(json!({"notify": {"method": "$/typescriptVersion", "params": {"version": "5.9.3", "source": "bundled"}}}));
  steps
}

pub fn with(mut steps: Vec<Value>, more: impl IntoIterator<Item = Value>) -> Vec<Value> {
  steps.extend(more);
  steps
}

pub fn alive(pid: u32) -> bool {
  Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().unwrap().success()
}

pub fn wait_dead(pid: u32) -> bool {
  wait_until(|| !alive(pid))
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
