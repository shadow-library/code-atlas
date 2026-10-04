use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime};

use atlas_core::{Availability, ContentHash, RelPath};
use serde::{Deserialize, Serialize};

use crate::cache::{CACHE_ENTRIES_PER_ROOT, ScipInputFile, cache_key, cache_path, cache_scope, entry_stem, prune, prune_unverified, sidecar_path, touch, unverified_path};
use crate::process::{ProcessOutcome, run_until, run_with_timeout};

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

  fn index_with_cancel(&self, request: &ScipRequest, _cancel: &AtomicBool) -> ScipRun {
    self.index(request)
  }
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
  pub cancel: &'a AtomicBool,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct InputStamp {
  len: u64,
  modified: Option<SystemTime>,
}

fn stamp(path: &Path) -> Option<InputStamp> {
  let metadata = fs::metadata(path).ok()?;
  Some(InputStamp {
    len: metadata.len(),
    modified: metadata.modified().ok(),
  })
}

fn stamp_inputs(root: &Path, inputs: &[ScipInputFile]) -> Vec<Option<InputStamp>> {
  inputs.iter().map(|input| stamp(&root.join(input.path.as_str()))).collect()
}

fn changed_inputs(root: &Path, inputs: &[ScipInputFile], before: &[Option<InputStamp>], protected: &[PathBuf]) -> Vec<RelPath> {
  inputs
    .iter()
    .zip(before)
    .filter(|(input, before)| {
      let path = root.join(input.path.as_str());
      let restamped = !protected.contains(&path) && stamp(&path) != **before;
      restamped || fs::read(&path).map_or(true, |bytes| ContentHash::of(&bytes) != input.content_hash)
    })
    .map(|(input, _)| input.path.clone())
    .collect()
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
    cancel,
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
  let inputs = merged_inputs(request, extra_inputs);
  let root = absolute(&request.root);
  let scope = cache_scope(indexer, &root);
  let stem = entry_stem(&scope, &cache_key(indexer, &version, &inputs));
  let final_path = cache_path(&cache_dir, &stem);
  let sidecar_path = sidecar_path(&cache_dir, &stem);
  if fs::metadata(&final_path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
    && let Some(recorded) = read_sidecar(&sidecar_path)
  {
    touch(&final_path);
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
  let stamps = stamp_inputs(&root, &inputs);
  let spawned_at = SystemTime::now();
  let snapshots: Vec<Snapshot> = protected.iter().cloned().map(Snapshot::take).collect();
  let outcome = run_until(command, timeout, cancel);
  warnings.borrow_mut().extend(snapshots.iter().filter_map(Snapshot::restore_if_changed));
  match outcome {
    ProcessOutcome::SpawnFailed(reason) => failed(format!("cannot start {indexer}: {reason}"), String::new()),
    ProcessOutcome::Cancelled => failed(format!("{indexer} was cancelled"), String::new()),
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
      let changed = changed_inputs(&root, &inputs, &stamps, &protected);
      if let Some(first) = changed.first() {
        let run_name = work_dir
          .path()
          .file_name()
          .map(|name| name.to_string_lossy().trim_start_matches(".run-").to_owned())
          .unwrap_or_default();
        let unverified = unverified_path(&cache_dir, &scope, &run_name);
        if let Err(error) = fs::rename(&scratch_output, &unverified) {
          return failed(format!("cannot move the index to {}: {error}", unverified.display()), stderr);
        }
        prune_unverified(&cache_dir, &scope, spawned_at);
        tracing::info!(indexer, changed = changed.len(), first = %first, "input files changed while the indexer ran; the index is not cached");
        warnings.borrow_mut().push(format!(
          "{} input file(s) changed while {indexer} ran (first: {first}); the index was not cached",
          changed.len()
        ));
        return run(ScipOutcome::Ok, Some(unverified), CacheStatus::Miss, degradations.clone());
      }
      if let Err(reason) = publish(work_dir.path(), &scratch_output, &final_path, &sidecar_path, &degradations) {
        return failed(reason, stderr);
      }
      touch(&final_path);
      prune(&cache_dir, &scope, CACHE_ENTRIES_PER_ROOT, &stem);
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
    ProcessOutcome::Cancelled => Err("was cancelled".to_owned()),
    ProcessOutcome::SpawnFailed(reason) => Err(reason),
  }
}

pub(crate) fn absolute(path: &Path) -> PathBuf {
  std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn input(root: &Path, path: &str, content: &str) -> ScipInputFile {
    fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
    fs::write(root.join(path), content).unwrap();
    ScipInputFile {
      path: RelPath::new(path).unwrap(),
      content_hash: ContentHash::of(content.as_bytes()),
    }
  }

  fn cache_entries(cache_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(cache_dir)
      .map(|entries| entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect())
      .unwrap_or_default();
    names.retain(|name| !name.starts_with(".run-"));
    names.sort();
    names
  }

  fn run_script(request: &ScipRequest, script: &str) -> ScipRun {
    let cancel = AtomicBool::new(false);
    let build = |_work_dir: &Path, output: &Path| {
      let mut command = Command::new("sh");
      command.arg("-c").arg(format!("{script}; printf index > '{}'", output.display()));
      Ok(command)
    };
    run_cached(CachedInvocation {
      indexer: "fake",
      version: "1".to_owned(),
      request,
      timeout: Duration::from_secs(30),
      degradations: Vec::new(),
      extra_inputs: Vec::new(),
      protected: Vec::new(),
      build: &build,
      classify_failure: |_| None,
      cancel: &cancel,
    })
  }

  #[test]
  fn should_not_cache_an_index_whose_inputs_changed_while_the_indexer_ran() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().join("pkg");
    let request = ScipRequest {
      root: root.clone(),
      cache_dir: scratch.path().join("cache"),
      files: vec![input(&root, "src/a.rs", "fn a() {}\n")],
    };

    let raced = run_script(&request, &format!("printf 'fn b() {{}}' > '{}'", root.join("src/a.rs").display()));

    assert_eq!(raced.outcome, ScipOutcome::Ok);
    assert_eq!(raced.cache, CacheStatus::Miss);
    let path = raced.output_path.clone().unwrap();
    assert!(path.to_string_lossy().contains("-unverified-"), "{path:?}");
    assert_eq!(raced.warnings.len(), 1, "{:?}", raced.warnings);
    fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
    let rerun = run_script(&request, "true");
    assert_eq!(rerun.cache, CacheStatus::Miss);
    assert!(
      rerun
        .output_path
        .unwrap()
        .to_string_lossy()
        .ends_with(&format!("{}.scip", cache_key("fake", "1", &request.files)))
    );
    assert_eq!(run_script(&request, "exit 1").cache, CacheStatus::Hit);
  }

  #[test]
  fn should_not_cache_an_index_when_a_file_was_rewritten_with_the_same_content() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().join("pkg");
    let request = ScipRequest {
      root: root.clone(),
      cache_dir: scratch.path().join("cache"),
      files: vec![input(&root, "src/a.rs", "fn a() {}\n")],
    };
    let file = root.join("src/a.rs");

    let raced = run_script(
      &request,
      &format!("sleep 0.05; printf 'fn b() {{}}\\n' > '{0}'; sleep 0.05; printf 'fn a() {{}}\\n' > '{0}'", file.display()),
    );

    assert_eq!(fs::read_to_string(&file).unwrap(), "fn a() {}\n");
    assert!(raced.output_path.unwrap().to_string_lossy().contains("-unverified-"));
  }

  #[test]
  fn should_report_a_cancelled_run_as_a_failure_and_cache_nothing() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().join("pkg");
    let request = ScipRequest {
      root: root.clone(),
      cache_dir: scratch.path().join("cache"),
      files: vec![input(&root, "src/a.rs", "fn a() {}\n")],
    };
    let cancel = AtomicBool::new(true);
    let build = |_work_dir: &Path, _output: &Path| {
      let mut command = Command::new("sh");
      command.args(["-c", "sleep 30"]);
      Ok(command)
    };

    let run = run_cached(CachedInvocation {
      indexer: "fake",
      version: "1".to_owned(),
      request: &request,
      timeout: Duration::from_secs(60),
      degradations: Vec::new(),
      extra_inputs: Vec::new(),
      protected: Vec::new(),
      build: &build,
      classify_failure: |_| None,
      cancel: &cancel,
    });

    assert_eq!(
      run.outcome,
      ScipOutcome::Failed {
        reason: "fake was cancelled".to_owned(),
        stderr_tail: String::new(),
      }
    );
    assert!(run.duration < Duration::from_secs(10));
    assert_eq!(cache_entries(&request.cache_dir), Vec::<String>::new());
  }

  #[test]
  fn should_keep_only_the_newest_entries_per_indexer_and_root() {
    let scratch = tempfile::tempdir().unwrap();
    let cache_dir = scratch.path().join("cache");
    let root = scratch.path().join("pkg");
    let other_root = scratch.path().join("other");
    let other = ScipRequest {
      root: other_root.clone(),
      cache_dir: cache_dir.clone(),
      files: vec![input(&other_root, "src/o.rs", "fn o() {}\n")],
    };
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(cache_dir.join(format!("{}.scip", "a".repeat(64))), "legacy").unwrap();
    fs::write(cache_dir.join(format!("{}.json", "a".repeat(64))), "[]").unwrap();
    run_script(&other, "true");
    let mut stems = Vec::new();
    for version in ["fn a() {}\n", "fn b() {}\n", "fn c() {}\n"] {
      let request = ScipRequest {
        root: root.clone(),
        cache_dir: cache_dir.clone(),
        files: vec![input(&root, "src/a.rs", version)],
      };
      let run = run_script(&request, "true");
      assert_eq!(run.cache, CacheStatus::Miss);
      stems.push(entry_stem(&cache_scope("fake", &absolute(&root)), &cache_key("fake", "1", &request.files)));
      std::thread::sleep(Duration::from_millis(20));
    }

    let entries = cache_entries(&cache_dir);

    let other_stem = entry_stem(&cache_scope("fake", &absolute(&other_root)), &cache_key("fake", "1", &other.files));
    let mut expected: Vec<String> = [&other_stem, &stems[1], &stems[2]]
      .iter()
      .flat_map(|stem| [format!("{stem}.json"), format!("{stem}.scip")])
      .collect();
    expected.sort();
    assert_eq!(entries, expected);
  }

  #[test]
  fn should_name_unverified_indexes_per_run_and_drop_them_once_a_run_is_cached() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().join("pkg");
    let request = ScipRequest {
      root: root.clone(),
      cache_dir: scratch.path().join("cache"),
      files: vec![input(&root, "src/a.rs", "fn a() {}\n")],
    };
    let file = root.join("src/a.rs");
    let scope = cache_scope("fake", &absolute(&root));
    let mut outputs = Vec::new();
    for _ in 0..2 {
      outputs.push(run_script(&request, &format!("printf 'fn b() {{}}\\n' > '{}'", file.display())).output_path.unwrap());
      fs::write(&file, "fn a() {}\n").unwrap();
    }
    let latest = outputs[1].file_name().unwrap().to_string_lossy().into_owned();
    assert_ne!(outputs[0], outputs[1]);
    assert!(latest.starts_with(&format!("{scope}-unverified-")), "{latest}");
    assert_eq!(cache_entries(&request.cache_dir), vec![latest]);

    run_script(&request, "true");

    let stem = entry_stem(&scope, &cache_key("fake", "1", &request.files));
    assert_eq!(cache_entries(&request.cache_dir), vec![format!("{stem}.json"), format!("{stem}.scip")]);
  }

  #[test]
  fn should_never_prune_the_entry_it_just_published() {
    let scratch = tempfile::tempdir().unwrap();
    let cache_dir = scratch.path().join("cache");
    let root = scratch.path().join("pkg");
    let scope = cache_scope("fake", &absolute(&root));
    fs::create_dir_all(&cache_dir).unwrap();
    let future = SystemTime::now() + Duration::from_secs(60);
    for digit in ["b", "c"] {
      let stem = entry_stem(&scope, &digit.repeat(64));
      for path in [cache_path(&cache_dir, &stem), sidecar_path(&cache_dir, &stem)] {
        fs::write(&path, "[]").unwrap();
        fs::File::options()
          .write(true)
          .open(&path)
          .unwrap()
          .set_modified(future + Duration::from_secs(digit.len() as u64))
          .unwrap();
      }
    }
    let request = ScipRequest {
      root: root.clone(),
      cache_dir: cache_dir.clone(),
      files: vec![input(&root, "src/a.rs", "fn a() {}\n")],
    };

    let run = run_script(&request, "true");

    let published = run.output_path.unwrap();
    assert!(published.is_file(), "{published:?}");
    let stem = entry_stem(&scope, &cache_key("fake", "1", &request.files));
    assert_eq!(published, cache_path(&cache_dir, &stem));
    assert_eq!(cache_entries(&cache_dir).len(), 2 * CACHE_ENTRIES_PER_ROOT);
    assert!(cache_path(&cache_dir, &stem).is_file() && sidecar_path(&cache_dir, &stem).is_file());
  }
}
