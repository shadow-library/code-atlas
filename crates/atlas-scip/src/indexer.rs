use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use atlas_core::{Availability, RelPath};
use serde::{Deserialize, Serialize};

use crate::cache::{ScipInputFile, cache_key, cache_path, sidecar_path};
use crate::process::{ProcessOutcome, run_with_timeout};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipRequest {
  pub root: PathBuf,
  pub cache_dir: PathBuf,
  pub files: Vec<ScipInputFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScipOutcome {
  Ok,
  Failed { reason: String, stderr_tail: String },
  TimedOut,
  Unavailable { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
  Hit,
  Miss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Degradation {
  MissingNodeModules,
  InferredTsconfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipRun {
  pub indexer: String,
  pub version: Option<String>,
  pub duration: Duration,
  pub output_path: Option<PathBuf>,
  pub outcome: ScipOutcome,
  pub cache: CacheStatus,
  pub degradations: Vec<Degradation>,
  pub warnings: Vec<String>,
}

impl ScipRun {
  pub(crate) fn unavailable(indexer: &str, reason: String) -> Self {
    Self {
      indexer: indexer.to_owned(),
      version: None,
      duration: Duration::ZERO,
      output_path: None,
      outcome: ScipOutcome::Unavailable { reason },
      cache: CacheStatus::Miss,
      degradations: Vec::new(),
      warnings: Vec::new(),
    }
  }
}

pub trait ScipIndexer: Send + Sync {
  fn id(&self) -> &'static str;

  fn availability(&self) -> Availability;

  fn index(&self, request: &ScipRequest) -> ScipRun;
}

pub(crate) type BuildCommand<'a> = &'a dyn Fn(&Path, &Path) -> Result<Command, String>;

pub(crate) struct CachedInvocation<'a> {
  pub indexer: &'static str,
  pub version: String,
  pub request: &'a ScipRequest,
  pub timeout: Duration,
  pub degradations: Vec<Degradation>,
  pub extra_inputs: Vec<ScipInputFile>,
  pub protected: Vec<PathBuf>,
  pub build: BuildCommand<'a>,
  pub classify_failure: fn(&str) -> Option<String>,
}

struct Snapshot {
  path: PathBuf,
  original: Option<Vec<u8>>,
}

impl Snapshot {
  fn take(path: PathBuf) -> Self {
    let original = fs::read(&path).ok();
    Self { path, original }
  }

  fn restore_if_changed(&self) -> Option<String> {
    let current = fs::read(&self.path).ok();
    if current == self.original {
      return None;
    }
    let restored = match &self.original {
      Some(bytes) => fs::write(&self.path, bytes),
      None => fs::remove_file(&self.path),
    };
    Some(match restored {
      Ok(()) => format!("{} was modified by the indexer and has been restored", self.path.display()),
      Err(error) => format!("{} was modified by the indexer and could not be restored: {error}", self.path.display()),
    })
  }
}

fn merged_inputs(request: &ScipRequest, extra: Vec<ScipInputFile>) -> Vec<ScipInputFile> {
  let known: HashSet<&RelPath> = request.files.iter().map(|file| &file.path).collect();
  let additional: Vec<ScipInputFile> = extra.into_iter().filter(|file| !known.contains(&file.path)).collect();
  request.files.iter().cloned().chain(additional).collect()
}

fn read_sidecar(path: &Path) -> Option<Vec<Degradation>> {
  serde_json::from_slice(&fs::read(path).ok()?).ok()
}

pub(crate) fn run_cached(invocation: CachedInvocation<'_>) -> ScipRun {
  let started = Instant::now();
  let CachedInvocation {
    indexer,
    version,
    request,
    timeout,
    degradations,
    extra_inputs,
    protected,
    build,
    classify_failure,
  } = invocation;
  let warnings = std::cell::RefCell::new(Vec::<String>::new());
  let run = |outcome: ScipOutcome, output_path: Option<PathBuf>, cache: CacheStatus, degradations: Vec<Degradation>| ScipRun {
    indexer: indexer.to_owned(),
    version: Some(version.clone()),
    duration: started.elapsed(),
    output_path,
    outcome,
    cache,
    degradations,
    warnings: warnings.borrow().clone(),
  };
  let failed = |reason: String, stderr_tail: String| run(ScipOutcome::Failed { reason, stderr_tail }, None, CacheStatus::Miss, degradations.clone());

  let cache_dir = absolute(&request.cache_dir);
  let key = cache_key(indexer, &version, &merged_inputs(request, extra_inputs));
  let final_path = cache_path(&cache_dir, &key);
  let sidecar_path = sidecar_path(&cache_dir, &key);
  if fs::metadata(&final_path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
    && let Some(recorded) = read_sidecar(&sidecar_path)
  {
    return run(ScipOutcome::Ok, Some(final_path), CacheStatus::Hit, recorded);
  }
  if let Err(error) = fs::create_dir_all(&cache_dir) {
    return failed(format!("cannot create cache dir {}: {error}", cache_dir.display()), String::new());
  }
  let work_dir = match tempfile::Builder::new().prefix(".run-").tempdir_in(&cache_dir) {
    Ok(dir) => dir,
    Err(error) => return failed(format!("cannot create a working directory: {error}"), String::new()),
  };
  let scratch_output = work_dir.path().join("index.scip");
  let mut command = match build(work_dir.path(), &scratch_output) {
    Ok(command) => command,
    Err(reason) => return failed(reason, String::new()),
  };
  command.current_dir(work_dir.path());
  let snapshots: Vec<Snapshot> = protected.into_iter().map(Snapshot::take).collect();
  let outcome = run_with_timeout(command, timeout);
  warnings.borrow_mut().extend(snapshots.iter().filter_map(Snapshot::restore_if_changed));
  match outcome {
    ProcessOutcome::SpawnFailed(reason) => failed(format!("cannot start {indexer}: {reason}"), String::new()),
    ProcessOutcome::TimedOut { stderr } => {
      tracing::warn!(indexer, timeout_secs = timeout.as_secs(), stderr_bytes = stderr.len(), "scip indexer timed out");
      run(ScipOutcome::TimedOut, None, CacheStatus::Miss, degradations.clone())
    }
    ProcessOutcome::Exited { success: false, code, stderr, .. } => match classify_failure(&stderr) {
      Some(reason) => run(ScipOutcome::Unavailable { reason }, None, CacheStatus::Miss, degradations.clone()),
      None => failed(
        format!("{indexer} exited with {}", code.map_or_else(|| "a signal".to_owned(), |code| format!("status {code}"))),
        stderr,
      ),
    },
    ProcessOutcome::Exited { success: true, stderr, .. } => {
      if !fs::metadata(&scratch_output).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0) {
        return failed(format!("{indexer} exited successfully but wrote no index"), stderr);
      }
      if let Err(reason) = publish(work_dir.path(), &scratch_output, &final_path, &sidecar_path, &degradations) {
        return failed(reason, stderr);
      }
      run(ScipOutcome::Ok, Some(final_path), CacheStatus::Miss, degradations.clone())
    }
  }
}

fn publish(work_dir: &Path, scratch_output: &Path, final_path: &Path, sidecar_path: &Path, degradations: &[Degradation]) -> Result<(), String> {
  let scratch_sidecar = work_dir.join("index.json");
  let encoded = serde_json::to_vec(degradations).map_err(|error| format!("cannot encode the cache sidecar: {error}"))?;
  fs::write(&scratch_sidecar, encoded).map_err(|error| format!("cannot write the cache sidecar: {error}"))?;
  fs::rename(&scratch_sidecar, sidecar_path).map_err(|error| format!("cannot move the sidecar into the cache at {}: {error}", sidecar_path.display()))?;
  fs::rename(scratch_output, final_path).map_err(|error| format!("cannot move the index into the cache at {}: {error}", final_path.display()))
}

pub(crate) fn probe_stdout(command: Command) -> Result<String, String> {
  match run_with_timeout(command, PROBE_TIMEOUT) {
    ProcessOutcome::Exited { success: true, stdout, .. } if !stdout.trim().is_empty() => Ok(stdout.trim().to_owned()),
    ProcessOutcome::Exited { success: true, .. } => Err("printed nothing".to_owned()),
    ProcessOutcome::Exited { code, stderr, .. } => Err(format!("exited with {code:?}: {}", stderr.trim())),
    ProcessOutcome::TimedOut { .. } => Err(format!("did not answer within {}s", PROBE_TIMEOUT.as_secs())),
    ProcessOutcome::SpawnFailed(reason) => Err(reason),
  }
}

pub(crate) fn absolute(path: &Path) -> PathBuf {
  std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}
