use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use atlas_core::Availability;

use crate::cache::collect_inputs;
use crate::indexer::{CachedInvocation, DEFAULT_TIMEOUT, ScipIndexer, ScipRequest, ScipRun, absolute, probe_stdout, run_cached};

pub const RUST_ANALYZER_ID: &str = "rust-analyzer";

#[derive(Debug, Clone)]
pub struct RustAnalyzerScip {
  binary: Option<PathBuf>,
  timeout: Duration,
}

impl Default for RustAnalyzerScip {
  fn default() -> Self {
    Self::new()
  }
}

struct Resolved {
  binary: PathBuf,
  version: String,
}

impl RustAnalyzerScip {
  pub fn new() -> Self {
    Self {
      binary: None,
      timeout: DEFAULT_TIMEOUT,
    }
  }

  pub fn with_binary(mut self, binary: PathBuf) -> Self {
    self.binary = Some(binary);
    self
  }

  pub fn with_timeout(mut self, timeout: Duration) -> Self {
    self.timeout = timeout;
    self
  }

  fn candidates(&self) -> Vec<PathBuf> {
    if let Some(binary) = &self.binary {
      return vec![binary.clone()];
    }
    [find_on_path(), rustup_which()].into_iter().flatten().collect()
  }

  fn resolve(&self) -> Result<Resolved, String> {
    let candidates = self.candidates();
    if candidates.is_empty() {
      return Err("rust-analyzer is not on PATH and `rustup which rust-analyzer` found nothing".to_owned());
    }
    let mut failures = Vec::new();
    for binary in candidates {
      let mut command = Command::new(&binary);
      command.arg("--version");
      match probe_stdout(command) {
        Ok(version) => return Ok(Resolved { binary, version }),
        Err(reason) => failures.push(format!("{}: {reason}", binary.display())),
      }
    }
    Err(format!("rust-analyzer --version failed ({})", failures.join("; ")))
  }
}

fn find_on_path() -> Option<PathBuf> {
  let name = format!("rust-analyzer{}", env::consts::EXE_SUFFIX);
  env::split_paths(&env::var_os("PATH")?)
    .map(|directory| directory.join(&name))
    .find(|candidate| candidate.is_file())
}

fn rustup_which() -> Option<PathBuf> {
  let mut command = Command::new("rustup");
  command.args(["which", "rust-analyzer"]);
  probe_stdout(command).ok().map(PathBuf::from)
}

impl ScipIndexer for RustAnalyzerScip {
  fn id(&self) -> &'static str {
    RUST_ANALYZER_ID
  }

  fn availability(&self) -> Availability {
    match self.resolve() {
      Ok(_) => Availability::Available,
      Err(reason) => Availability::Unavailable { reason },
    }
  }

  fn index(&self, request: &ScipRequest) -> ScipRun {
    let resolved = match self.resolve() {
      Ok(resolved) => resolved,
      Err(reason) => return ScipRun::unavailable(RUST_ANALYZER_ID, reason),
    };
    let root = absolute(&request.root);
    let target_dir = absolute(&request.cache_dir)
      .join("rust-target")
      .join(&blake3::hash(root.to_string_lossy().as_bytes()).to_hex()[..16]);
    let build = |_work_dir: &std::path::Path, output: &std::path::Path| {
      let mut command = Command::new(&resolved.binary);
      command.arg("scip").arg(&root).arg("--output").arg(output).env("CARGO_TARGET_DIR", &target_dir);
      Ok(command)
    };
    run_cached(CachedInvocation {
      indexer: RUST_ANALYZER_ID,
      version: resolved.version.clone(),
      request,
      timeout: self.timeout,
      degradations: Vec::new(),
      extra_inputs: collect_inputs(&root, is_cargo_input),
      protected: vec![root.join("Cargo.lock")],
      build: &build,
      classify_failure: |_| None,
    })
  }
}

fn is_cargo_input(path: &str) -> bool {
  let name = path.rsplit('/').next().unwrap_or(path);
  matches!(name, "Cargo.toml" | "Cargo.lock" | "rust-toolchain" | "rust-toolchain.toml") || path == ".cargo/config.toml" || path == ".cargo/config"
}
