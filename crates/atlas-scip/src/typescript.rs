use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use atlas_core::Availability;

use crate::cache::collect_inputs;
use crate::indexer::{CachedInvocation, DEFAULT_TIMEOUT, Degradation, ScipIndexer, ScipRequest, ScipRun, absolute, probe_stdout, run_cached};

pub const SCIP_TYPESCRIPT_ID: &str = "scip-typescript";
const NPM_PACKAGE: &str = "@sourcegraph/scip-typescript";
const NETWORK_FAILURE_MARKERS: [&str; 6] = ["ENOTFOUND", "ETIMEDOUT", "EAI_AGAIN", "ENETUNREACH", "ECONNREFUSED", "ECONNRESET"];

#[derive(Debug, Clone)]
pub struct ScipTypescript {
  timeout: Duration,
}

impl Default for ScipTypescript {
  fn default() -> Self {
    Self::new()
  }
}

impl ScipTypescript {
  pub fn new() -> Self {
    Self { timeout: DEFAULT_TIMEOUT }
  }

  pub fn with_timeout(mut self, timeout: Duration) -> Self {
    self.timeout = timeout;
    self
  }

  fn cached_version() -> Option<String> {
    let mut command = Command::new("npx");
    command.args(["--no-install", NPM_PACKAGE, "--version"]);
    probe_stdout(command).ok()
  }

  fn registry_reachable() -> bool {
    let mut command = Command::new("npm");
    command.args(["ping", "--fetch-timeout", "5000", "--fetch-retries", "0"]);
    probe_stdout(command).is_ok()
  }

  fn check_npx() -> Result<(), String> {
    let mut command = Command::new("npx");
    command.arg("--version");
    probe_stdout(command).map(drop).map_err(|reason| format!("npx is not usable: {reason}"))
  }

  fn resolve_version() -> Result<String, String> {
    Self::check_npx()?;
    if let Some(version) = Self::cached_version() {
      return Ok(version);
    }
    if !Self::registry_reachable() {
      return Err(format!("{NPM_PACKAGE} is not cached and the npm registry is unreachable"));
    }
    let mut command = Command::new("npx");
    command.args(["--yes", NPM_PACKAGE, "--version"]);
    probe_stdout(command).map_err(|reason| format!("cannot fetch {NPM_PACKAGE}: {reason}"))
  }
}

fn classify_failure(stderr: &str) -> Option<String> {
  NETWORK_FAILURE_MARKERS
    .iter()
    .any(|marker| stderr.contains(marker))
    .then(|| format!("network failure while fetching {NPM_PACKAGE}"))
}

fn has_node_modules(root: &Path) -> bool {
  root.ancestors().any(|directory| directory.join("node_modules").is_dir())
}

impl ScipIndexer for ScipTypescript {
  fn id(&self) -> &'static str {
    SCIP_TYPESCRIPT_ID
  }

  fn availability(&self) -> Availability {
    let usable = Self::check_npx().and_then(|()| {
      if Self::cached_version().is_some() || Self::registry_reachable() {
        return Ok(());
      }
      Err(format!("{NPM_PACKAGE} is not cached and the npm registry is unreachable"))
    });
    match usable {
      Ok(()) => Availability::Available,
      Err(reason) => Availability::Unavailable { reason },
    }
  }

  fn index(&self, request: &ScipRequest) -> ScipRun {
    let version = match Self::resolve_version() {
      Ok(version) => version,
      Err(reason) => return ScipRun::unavailable(SCIP_TYPESCRIPT_ID, reason),
    };
    let root = absolute(&request.root);
    let infer_tsconfig = !root.join("tsconfig.json").is_file();
    let mut degradations = Vec::new();
    if infer_tsconfig {
      degradations.push(Degradation::InferredTsconfig);
    }
    if !has_node_modules(&root) {
      degradations.push(Degradation::MissingNodeModules);
    }
    let build = |work_dir: &Path, output: &Path| {
      let cwd = if infer_tsconfig {
        build_overlay(&root, &work_dir.join("overlay"))?
      } else {
        root.clone()
      };
      let mut command = Command::new("npx");
      command
        .args(["--yes", NPM_PACKAGE, "index", "--no-progress-bar", "--cwd"])
        .arg(&cwd)
        .arg("--output")
        .arg(output);
      if infer_tsconfig {
        command.arg("--infer-tsconfig");
      }
      Ok(command)
    };
    run_cached(CachedInvocation {
      indexer: SCIP_TYPESCRIPT_ID,
      version,
      request,
      timeout: self.timeout,
      degradations,
      extra_inputs: collect_inputs(&root, is_typescript_input),
      protected: Vec::new(),
      build: &build,
      classify_failure,
    })
  }
}

fn is_typescript_input(path: &str) -> bool {
  let name = path.rsplit('/').next().unwrap_or(path);
  matches!(
    name,
    "package.json" | "package-lock.json" | "npm-shrinkwrap.json" | "yarn.lock" | "pnpm-lock.yaml" | "bun.lock" | "bun.lockb"
  ) || (name.starts_with("tsconfig") && name.ends_with(".json"))
}

#[cfg(unix)]
fn build_overlay(root: &Path, overlay: &Path) -> Result<PathBuf, String> {
  std::fs::create_dir_all(overlay).map_err(|error| format!("cannot create the overlay directory: {error}"))?;
  let entries = std::fs::read_dir(root).map_err(|error| format!("cannot list {}: {error}", root.display()))?;
  for entry in entries.flatten() {
    let link = overlay.join(entry.file_name());
    std::os::unix::fs::symlink(entry.path(), &link).map_err(|error| format!("cannot link {} into the overlay: {error}", entry.path().display()))?;
  }
  Ok(overlay.to_path_buf())
}

#[cfg(not(unix))]
fn build_overlay(_root: &Path, _overlay: &Path) -> Result<PathBuf, String> {
  Err("indexing a package without tsconfig.json needs symlink support, which is unix-only here".to_owned())
}
