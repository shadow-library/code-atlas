use std::error::Error as StdError;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use atlas_core::{ProjectName, RepoName};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::binding::{Binding, OverlayOf, WorktreeOf};
use crate::config::WorktreeSettings;
use crate::error::ProjectError;
use crate::home::AtlasHome;
use crate::registry::{Project, Registry, slugify};
use crate::worktree::{CheckoutKind, GitCheckout, GitLocation};

const STATE_VERSION: u32 = 1;
const LOCK_SUFFIX: &str = ".lock";
const SEED_SUFFIX: &str = ".seeding";
const SEED_SIDE_FILES: [&str; 5] = ["-wal", "-shm", "-journal", ".lock", ".index.lock"];
const MAX_PROJECT_NAME_LEN: usize = 128;
const MAX_LEAF_LEN: usize = 40;
const DIGEST_LEN: usize = 8;
const FALLBACK_LEAF: &str = "worktree";
const MINUTE_MS: i64 = 60 * 1000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;
const TOUCH_INTERVAL_MS: i64 = HOUR_MS;
const GC_INTERVAL_MS: i64 = HOUR_MS;
const STATE_LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BACKOFF: Duration = Duration::from_millis(50);

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OverlayError {
  #[error(transparent)]
  Project(#[from] ProjectError),
  #[error("invalid worktree overlay state {}: {message}", .path.display())]
  State { path: PathBuf, message: String },
  #[error("timed out after {waited:?} waiting for the overlay lock {}", .path.display())]
  LockTimeout { path: PathBuf, waited: Duration },
  #[error("could not seed the worktree overlay {name}: {source}")]
  Seed {
    name: String,
    #[source]
    source: Box<dyn StdError + Send + Sync>,
  },
  #[error("seeding the worktree overlay {name} left {} behind, so it was not installed", .path.display())]
  SeedLeftover { name: String, path: PathBuf },
  #[error("the worktree overlay {0} was removed while it was being prepared")]
  Vanished(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayPlan {
  pub base: ProjectName,
  pub label: String,
  pub worktree: PathBuf,
  pub common_dir: PathBuf,
  pub repos: Vec<RepoName>,
  pub project: Project,
  pub warnings: Vec<String>,
}

impl OverlayPlan {
  pub fn binding(&self) -> Binding {
    Binding::Overlay(OverlayOf {
      project: self.base.clone(),
      overlay: self.project.name.clone(),
      label: self.label.clone(),
      worktree: self.worktree.clone(),
      repos: self.repos.clone(),
    })
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayDerivation {
  Eligible(Box<OverlayPlan>),
  Ineligible { reason: String },
}

impl Registry {
  pub fn derive_overlay(&self, worktree: &WorktreeOf) -> Result<OverlayDerivation, ProjectError> {
    let ineligible = |reason: String| Ok(OverlayDerivation::Ineligible { reason });
    let base = self.project(worktree.project.as_str())?;
    let checkout = match GitCheckout::locate(&worktree.worktree) {
      GitLocation::Checkout(checkout) if checkout.kind == CheckoutKind::LinkedWorktree && checkout.root == worktree.worktree => checkout,
      _ => return ineligible(format!("{} is not a linked git worktree", worktree.worktree.display())),
    };
    let mut paths = Vec::new();
    for repo in &base.repos {
      let Some(indexed) = GitCheckout::find(&repo.path).filter(|indexed| checkout.is_other_checkout_of(indexed) && indexed.root == worktree.indexed_checkout) else {
        continue;
      };
      let Ok(relative) = repo.path.strip_prefix(&indexed.root) else { continue };
      let mapped = checkout.root.join(relative);
      match fs::canonicalize(&mapped) {
        Ok(found) if found == mapped && found.is_dir() => paths.push((repo.name.clone(), mapped)),
        Ok(found) => {
          return ineligible(format!(
            "repo {}'s directory {} in the worktree resolves to {}",
            repo.name,
            mapped.display(),
            found.display()
          ));
        }
        Err(_) => return ineligible(format!("repo {} has no directory {} in the worktree", repo.name, mapped.display())),
      }
    }
    if paths.is_empty() {
      return ineligible(format!(
        "no repo of project {} is registered inside the checkout {}, so the worktree cannot stand in for one",
        base.name,
        worktree.indexed_checkout.display()
      ));
    }
    let name = overlay_name(&base.name, &checkout.root)?;
    let (project, warnings) = self.project_on_paths(base.name.as_str(), name, &paths)?;
    Ok(OverlayDerivation::Eligible(Box::new(OverlayPlan {
      base: base.name.clone(),
      label: format!("{}@{}", base.name, leaf_name(&checkout.root)),
      worktree: checkout.root,
      common_dir: checkout.common_dir,
      repos: paths.into_iter().map(|(name, _)| name).collect(),
      project,
      warnings,
    })))
  }
}

fn leaf_name(worktree: &Path) -> String {
  worktree
    .file_name()
    .map(|name| name.to_string_lossy().into_owned())
    .unwrap_or_else(|| FALLBACK_LEAF.to_owned())
}

fn overlay_name(base: &ProjectName, worktree: &Path) -> Result<ProjectName, ProjectError> {
  let leaf = slugify(&leaf_name(worktree)).unwrap_or_else(|| FALLBACK_LEAF.to_owned());
  let leaf: String = leaf.chars().take(MAX_LEAF_LEN).collect();
  let mut hasher = blake3::Hasher::new();
  hasher.update(base.as_str().as_bytes());
  hasher.update(&[0]);
  hasher.update(worktree.as_os_str().as_encoded_bytes());
  let digest = hasher.finalize().to_hex();
  let suffix = format!(".{leaf}-{}", &digest[..DIGEST_LEN]);
  let prefix: String = base.as_str().chars().take(MAX_PROJECT_NAME_LEN - suffix.len()).collect();
  Ok(ProjectName::new(format!("{prefix}{suffix}"))?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedSource {
  Base,
  Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedSummary {
  pub source: SeedSource,
  pub bytes: u64,
  pub millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayRecord {
  pub name: ProjectName,
  pub label: String,
  pub base: ProjectName,
  pub worktree: PathBuf,
  pub common_dir: PathBuf,
  pub created_ms: i64,
  pub last_used_ms: i64,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub seeded_ms: Option<i64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub seed: Option<SeedSummary>,
}

impl OverlayRecord {
  fn of(plan: &OverlayPlan, now_ms: i64) -> Self {
    Self {
      name: plan.project.name.clone(),
      label: plan.label.clone(),
      base: plan.base.clone(),
      worktree: plan.worktree.clone(),
      common_dir: plan.common_dir.clone(),
      created_ms: now_ms,
      last_used_ms: now_ms,
      seeded_ms: None,
      seed: None,
    }
  }

  pub fn worktree_present(&self) -> bool {
    matches!(
      GitCheckout::locate(&self.worktree),
      GitLocation::Checkout(checkout) if checkout.kind == CheckoutKind::LinkedWorktree && checkout.root == self.worktree && checkout.common_dir == self.common_dir
    )
  }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StateFile {
  version: u32,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  last_gc_ms: Option<i64>,
  #[serde(default)]
  overlays: Vec<OverlayRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayState {
  Ready,
  NotSeeded,
  WorktreeGone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayListing {
  pub record: OverlayRecord,
  pub state: OverlayState,
  pub db_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcReason {
  WorktreeGone,
  BaseGone,
  Unused { days: u64 },
}

impl GcReason {
  pub fn describe(self) -> String {
    match self {
      Self::WorktreeGone => "its git worktree no longer exists".to_owned(),
      Self::BaseGone => "its project is no longer registered".to_owned(),
      Self::Unused { days } => format!("unused for more than {days} days"),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collected {
  pub record: OverlayRecord,
  pub reason: GcReason,
}

#[derive(Debug)]
pub struct OverlayLease {
  pub home: AtlasHome,
  pub record: OverlayRecord,
  pub seeded: Option<SeedSummary>,
  _lock: File,
}

#[derive(Clone, Copy)]
enum LockKind {
  Shared,
  Exclusive,
}

#[derive(Debug, Clone)]
pub struct Overlays {
  home: AtlasHome,
}

impl Overlays {
  pub fn new(home: &AtlasHome) -> Self {
    Self { home: home.clone() }
  }

  pub fn engine_home(&self) -> AtlasHome {
    self.home.overlay_home()
  }

  pub fn db_path(&self, name: &ProjectName) -> PathBuf {
    self.engine_home().db_path(name)
  }

  pub fn list(&self) -> Result<Vec<OverlayListing>, OverlayError> {
    let state = self.read_state()?;
    Ok(
      state
        .overlays
        .into_iter()
        .map(|record| {
          let db_path = self.db_path(&record.name);
          let state = match (record.worktree_present(), db_path.exists()) {
            (false, _) => OverlayState::WorktreeGone,
            (true, false) => OverlayState::NotSeeded,
            (true, true) => OverlayState::Ready,
          };
          OverlayListing { record, state, db_path }
        })
        .collect(),
    )
  }

  pub fn acquire<E>(&self, plan: &OverlayPlan, timeout: Duration, seed: impl FnOnce(&Path) -> Result<SeedSummary, E>) -> Result<OverlayLease, OverlayError>
  where
    E: Into<Box<dyn StdError + Send + Sync>>,
  {
    let name = &plan.project.name;
    let lock_path = self.lock_path(name);
    let lock = open_lock(&lock_path)?;
    let db_path = self.db_path(name);
    let started = Instant::now();
    let mut seed = Some(seed);
    let mut seeded = None;
    loop {
      lock_within(&lock, &lock_path, LockKind::Shared, started, timeout)?;
      if db_path.exists() {
        let record = self.touch(plan, seeded.as_ref())?;
        return Ok(OverlayLease {
          home: self.engine_home(),
          record,
          seeded,
          _lock: lock,
        });
      }
      unlock(&lock, &lock_path)?;
      lock_within(&lock, &lock_path, LockKind::Exclusive, started, timeout)?;
      let outcome = match (db_path.exists(), seed.take()) {
        (true, _) => Ok(None),
        (false, Some(seed)) => self.seed(plan, &db_path, seed).map(Some),
        (false, None) => Err(OverlayError::Vanished(name.to_string())),
      };
      unlock(&lock, &lock_path)?;
      if let Some(summary) = outcome? {
        seeded = Some(summary);
      }
    }
  }

  pub fn collect_garbage_if_due(&self, settings: &WorktreeSettings, registered: impl Fn(&ProjectName) -> bool, now_ms: i64) -> Result<Vec<Collected>, OverlayError> {
    let state = self.read_state()?;
    if state.overlays.is_empty() || state.last_gc_ms.is_some_and(|last| now_ms.saturating_sub(last) < GC_INTERVAL_MS) {
      return Ok(Vec::new());
    }
    self.collect_garbage(settings, registered, now_ms)
  }

  pub fn collect_garbage(&self, settings: &WorktreeSettings, registered: impl Fn(&ProjectName) -> bool, now_ms: i64) -> Result<Vec<Collected>, OverlayError> {
    let limit_ms = i64::try_from(settings.unused_limit().as_millis()).unwrap_or(i64::MAX);
    let stale = |record: &OverlayRecord| -> Option<GcReason> {
      if !registered(&record.base) {
        return Some(GcReason::BaseGone);
      }
      if !record.worktree_present() {
        return Some(GcReason::WorktreeGone);
      }
      (now_ms.saturating_sub(record.last_used_ms) > limit_ms).then_some(GcReason::Unused { days: settings.gc_after_days })
    };
    let mut collected = Vec::new();
    for record in self.read_state()?.overlays {
      if stale(&record).is_none() {
        continue;
      }
      let lock_path = self.lock_path(&record.name);
      let lock = open_lock(&lock_path)?;
      match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
          tracing::debug!(overlay = %record.name, "skipping a stale worktree overlay that a process still uses");
          continue;
        }
        Err(TryLockError::Error(source)) => return Err(ProjectError::io("lock", &lock_path, source).into()),
      }
      let current = self.read_state()?.overlays.into_iter().find(|current| current.name == record.name);
      let Some((current, reason)) = current.and_then(|current| stale(&current).map(|reason| (current, reason))) else {
        continue;
      };
      self.remove_data(&current.name)?;
      self.update_state(|state| state.overlays.retain(|kept| kept.name != current.name))?;
      tracing::info!(overlay = %current.name, worktree = %current.worktree.display(), reason = %reason.describe(), "removed a worktree overlay");
      collected.push(Collected { record: current, reason });
    }
    self.update_state(|state| state.last_gc_ms = Some(now_ms))?;
    Ok(collected)
  }

  fn lock_path(&self, name: &ProjectName) -> PathBuf {
    self.home.overlays_dir().join(format!("{name}{LOCK_SUFFIX}"))
  }

  fn seed<E>(&self, plan: &OverlayPlan, db_path: &Path, seed: impl FnOnce(&Path) -> Result<SeedSummary, E>) -> Result<SeedSummary, OverlayError>
  where
    E: Into<Box<dyn StdError + Send + Sync>>,
  {
    let name = plan.project.name.to_string();
    let directory = db_path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(directory).map_err(|source| ProjectError::io("create directory", directory, source))?;
    self.update_state(|state| upsert(state, plan, now_ms(), None))?;
    let staging = with_suffix(db_path, SEED_SUFFIX);
    remove_staging(&staging)?;
    let summary = match seed(&staging) {
      Ok(summary) => summary,
      Err(source) => {
        remove_staging(&staging)?;
        return Err(OverlayError::Seed { name, source: source.into() });
      }
    };
    if let Err(error) = install(&staging, db_path, &name) {
      remove_staging(&staging)?;
      return Err(error);
    }
    remove_staging(&staging)?;
    self.update_state(|state| upsert(state, plan, now_ms(), Some(&summary)))?;
    Ok(summary)
  }

  fn touch(&self, plan: &OverlayPlan, seeded: Option<&SeedSummary>) -> Result<OverlayRecord, OverlayError> {
    let now = now_ms();
    let current = self.read_state()?.overlays.into_iter().find(|record| record.name == plan.project.name);
    match current {
      Some(record) if seeded.is_none() && now.saturating_sub(record.last_used_ms) < TOUCH_INTERVAL_MS && same_place(&record, plan) => Ok(record),
      _ => self.update_state(|state| upsert(state, plan, now, seeded)),
    }
  }

  fn remove_data(&self, name: &ProjectName) -> Result<(), OverlayError> {
    let root = self.home.overlays_dir();
    let directory = root.join(name.as_str());
    if directory.parent() != Some(root.as_path()) {
      return Err(OverlayError::State {
        path: self.home.worktrees_path(),
        message: format!("refusing to remove {}, which is not an overlay directory", directory.display()),
      });
    }
    match fs::remove_dir_all(&directory) {
      Ok(()) => Ok(()),
      Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
      Err(source) => Err(ProjectError::io("remove", &directory, source).into()),
    }
  }

  fn read_state(&self) -> Result<StateFile, OverlayError> {
    let path = self.home.worktrees_path();
    let text = match fs::read_to_string(&path) {
      Ok(text) => text,
      Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(StateFile::default()),
      Err(source) => return Err(ProjectError::io("read", &path, source).into()),
    };
    let state: StateFile = serde_json::from_str(&text).map_err(|error| OverlayError::State {
      path: path.clone(),
      message: error.to_string(),
    })?;
    if state.version != STATE_VERSION {
      return Err(OverlayError::State {
        path,
        message: format!("unsupported version {}; this catl reads version {STATE_VERSION}", state.version),
      });
    }
    Ok(state)
  }

  fn update_state<T>(&self, apply: impl FnOnce(&mut StateFile) -> T) -> Result<T, OverlayError> {
    let path = self.home.worktrees_path();
    let directory = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(directory).map_err(|source| ProjectError::io("create directory", directory, source))?;
    let lock_path = with_suffix(&path, LOCK_SUFFIX);
    let lock = open_lock(&lock_path)?;
    lock_within(&lock, &lock_path, LockKind::Exclusive, Instant::now(), STATE_LOCK_TIMEOUT)?;
    let mut state = self.read_state()?;
    state.version = STATE_VERSION;
    let outcome = apply(&mut state);
    let text = serde_json::to_string_pretty(&state).map_err(|error| OverlayError::State {
      path: path.clone(),
      message: error.to_string(),
    })?;
    write_atomically(&path, &text)?;
    Ok(outcome)
  }
}

fn same_place(record: &OverlayRecord, plan: &OverlayPlan) -> bool {
  record.worktree == plan.worktree && record.common_dir == plan.common_dir && record.base == plan.base && record.label == plan.label
}

fn upsert(state: &mut StateFile, plan: &OverlayPlan, now_ms: i64, seeded: Option<&SeedSummary>) -> OverlayRecord {
  let index = match state.overlays.iter().position(|record| record.name == plan.project.name) {
    Some(index) => index,
    None => {
      state.overlays.push(OverlayRecord::of(plan, now_ms));
      state.overlays.len() - 1
    }
  };
  let record = &mut state.overlays[index];
  record.label = plan.label.clone();
  record.base = plan.base.clone();
  record.worktree = plan.worktree.clone();
  record.common_dir = plan.common_dir.clone();
  record.last_used_ms = now_ms;
  if let Some(summary) = seeded {
    record.seeded_ms = Some(now_ms);
    record.seed = Some(summary.clone());
  }
  record.clone()
}

fn install(staging: &Path, db_path: &Path, name: &str) -> Result<(), OverlayError> {
  for suffix in ["-wal", "-journal"] {
    let side = with_suffix(staging, suffix);
    match fs::metadata(&side) {
      Ok(metadata) if metadata.len() > 0 => {
        return Err(OverlayError::SeedLeftover {
          name: name.to_owned(),
          path: side,
        });
      }
      Ok(_) | Err(_) => {}
    }
  }
  let file = File::open(staging).map_err(|source| ProjectError::io("open", staging, source))?;
  file.sync_all().map_err(|source| ProjectError::io("sync", staging, source))?;
  fs::rename(staging, db_path).map_err(|source| ProjectError::io("rename into place", db_path, source))?;
  sync_directory(db_path.parent().unwrap_or(Path::new(".")));
  Ok(())
}

fn remove_staging(staging: &Path) -> Result<(), OverlayError> {
  let paths = std::iter::once(staging.to_path_buf()).chain(SEED_SIDE_FILES.iter().map(|suffix| with_suffix(staging, suffix)));
  for path in paths {
    match fs::remove_file(&path) {
      Ok(()) => {}
      Err(source) if source.kind() == io::ErrorKind::NotFound => {}
      Err(source) => return Err(ProjectError::io("remove", &path, source).into()),
    }
  }
  Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
  let mut name = path.as_os_str().to_owned();
  name.push(suffix);
  PathBuf::from(name)
}

fn open_lock(path: &Path) -> Result<File, OverlayError> {
  let directory = path.parent().unwrap_or(Path::new("."));
  fs::create_dir_all(directory).map_err(|source| ProjectError::io("create directory", directory, source))?;
  OpenOptions::new()
    .create(true)
    .truncate(false)
    .read(true)
    .write(true)
    .open(path)
    .map_err(|source| ProjectError::io("open lock", path, source).into())
}

fn lock_within(file: &File, path: &Path, kind: LockKind, started: Instant, timeout: Duration) -> Result<(), OverlayError> {
  let mut backoff = Duration::from_millis(1);
  loop {
    let attempt = match kind {
      LockKind::Shared => file.try_lock_shared(),
      LockKind::Exclusive => file.try_lock(),
    };
    match attempt {
      Ok(()) => return Ok(()),
      Err(TryLockError::WouldBlock) => {}
      Err(TryLockError::Error(source)) => return Err(ProjectError::io("lock", path, source).into()),
    }
    let waited = started.elapsed();
    if waited >= timeout {
      return Err(OverlayError::LockTimeout { path: path.to_path_buf(), waited });
    }
    thread::sleep(backoff.min(timeout - waited));
    backoff = (backoff * 2).min(MAX_BACKOFF);
  }
}

fn unlock(file: &File, path: &Path) -> Result<(), OverlayError> {
  file.unlock().map_err(|source| ProjectError::io("unlock", path, source).into())
}

fn write_atomically(path: &Path, text: &str) -> Result<(), OverlayError> {
  let directory = path.parent().unwrap_or(Path::new("."));
  let mut temp = tempfile::NamedTempFile::new_in(directory).map_err(|source| ProjectError::io("create temp file", directory, source))?;
  temp.write_all(text.as_bytes()).map_err(|source| ProjectError::io("write", temp.path(), source))?;
  temp.as_file().sync_all().map_err(|source| ProjectError::io("sync", temp.path(), source))?;
  temp.persist(path).map_err(|error| ProjectError::io("rename into place", path, error.error))?;
  sync_directory(directory);
  Ok(())
}

fn sync_directory(directory: &Path) {
  if let Ok(handle) = File::open(directory) {
    let _ = handle.sync_all();
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OverlayReport {
  pub line: String,
  pub json: serde_json::Value,
}

impl OverlayOf {
  pub fn report(&self, home: &AtlasHome) -> OverlayReport {
    let record = match Overlays::new(home).read_state() {
      Ok(state) => state.overlays.into_iter().find(|record| record.name == self.overlay),
      Err(error) => {
        tracing::warn!(error = %error, "cannot read the worktree overlay state");
        None
      }
    };
    let seeded = match record.as_ref().and_then(|record| Some((record.seeded_ms?, record.seed.as_ref()?))) {
      Some((at, seed)) if seed.source == SeedSource::Base => format!(
        "seeded from project {} {} ago; SCIP facts were copied then, and files changed in the worktree lag until `catl scip` runs here",
        self.project,
        elapsed_text(at)
      ),
      Some((at, _)) => format!("seeded empty {} ago because project {} had no index", elapsed_text(at), self.project),
      None => "not seeded yet".to_owned(),
    };
    OverlayReport {
      line: format!("{}; {seeded}", self.describe()),
      json: serde_json::json!({
        "label": self.label,
        "name": self.overlay.as_str(),
        "project": self.project.as_str(),
        "worktree": self.worktree.display().to_string(),
        "repos": self.repos.iter().map(RepoName::as_str).collect::<Vec<_>>(),
        "seeded_ms": record.as_ref().and_then(|record| record.seeded_ms),
        "seed": record.as_ref().and_then(|record| record.seed.as_ref()),
        "last_used_ms": record.as_ref().map(|record| record.last_used_ms),
      }),
    }
  }
}

pub fn elapsed_text(since_ms: i64) -> String {
  let elapsed = now_ms().saturating_sub(since_ms).max(0);
  match elapsed {
    elapsed if elapsed < HOUR_MS => format!("{}m", elapsed / MINUTE_MS),
    elapsed if elapsed < DAY_MS => format!("{}h", elapsed / HOUR_MS),
    elapsed => format!("{}d", elapsed / DAY_MS),
  }
}

pub fn now_ms() -> i64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;
  use std::sync::atomic::{AtomicUsize, Ordering};

  use tempfile::TempDir;

  use super::*;
  use crate::config::ConfigEnv;
  use crate::marker::MARKER_FILE;
  use crate::registry::RepoSpec;
  use crate::worktree::git_fixture::{git, repository};

  const TIMEOUT: Duration = Duration::from_secs(5);
  const DAY_MS: i64 = 86_400_000;

  struct Sandbox {
    _dir: TempDir,
    root: PathBuf,
    home: AtlasHome,
  }

  impl Sandbox {
    fn new() -> Self {
      let dir = tempfile::tempdir().unwrap();
      let root = fs::canonicalize(dir.path()).unwrap();
      let home = AtlasHome::rooted_at(root.join("atlas"));
      Self { _dir: dir, root, home }
    }

    fn registry(&self) -> Registry {
      Registry::load_with(&self.home, ConfigEnv::default()).unwrap()
    }

    fn project(&self, name: &str, repos: &[(&str, &Path)]) -> Registry {
      let mut registry = self.registry();
      let specs = repos.iter().map(|(repo, path)| RepoSpec::new(*path).named(*repo)).collect();
      registry.create_project(name, specs).unwrap();
      registry
    }

    fn worktree(&self, main: &Path, relative: &str) -> PathBuf {
      let branch = format!("wt-{}", relative.replace(['/', '.'], "-").trim_matches('-'));
      git(main, &["worktree", "add", "--quiet", "-b", &branch, relative]);
      fs::canonicalize(main.join(relative)).unwrap()
    }
  }

  fn derive(registry: &Registry, cwd: &Path) -> OverlayDerivation {
    let Binding::Worktree(worktree) = registry.resolve(cwd, None).unwrap().binding else {
      panic!("{} is not bound as a worktree", cwd.display())
    };
    registry.derive_overlay(&worktree).unwrap()
  }

  fn plan(registry: &Registry, cwd: &Path) -> OverlayPlan {
    match derive(registry, cwd) {
      OverlayDerivation::Eligible(plan) => *plan,
      OverlayDerivation::Ineligible { reason } => panic!("no overlay: {reason}"),
    }
  }

  fn write_seed(staging: &Path) -> Result<SeedSummary, io::Error> {
    fs::write(staging, b"seeded")?;
    Ok(SeedSummary {
      source: SeedSource::Base,
      bytes: 6,
      millis: 1,
    })
  }

  fn never_seed(_: &Path) -> Result<SeedSummary, io::Error> {
    panic!("the overlay was seeded twice")
  }

  #[test]
  fn should_move_only_the_worktrees_repo_into_the_overlay() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "shop");
    let other = sandbox.root.join("billing");
    fs::create_dir_all(&other).unwrap();
    let registry = sandbox.project("acme", &[("shop", &main), ("billing", &other)]);
    let worktree = sandbox.worktree(&main, ".worktrees/feature");

    let plan = plan(&registry, &worktree.join("src"));

    assert_eq!(plan.base.as_str(), "acme");
    assert_eq!(plan.label, "acme@feature");
    assert!(plan.project.name.as_str().starts_with("acme.feature-"), "{}", plan.project.name);
    assert_eq!(plan.project.name.as_str().len(), "acme.feature-".len() + DIGEST_LEN);
    assert_eq!(plan.worktree, worktree);
    assert_eq!(plan.common_dir, main.join(".git"));
    assert_eq!(plan.repos, [RepoName::new("shop").unwrap()]);
    let paths: Vec<(&str, &Path)> = plan.project.repos.iter().map(|repo| (repo.name.as_str(), repo.path.as_path())).collect();
    assert_eq!(paths, [("shop", worktree.as_path()), ("billing", other.as_path())]);
    assert_eq!(plan.project.settings, registry.project("acme").unwrap().settings);
  }

  #[test]
  fn should_map_every_repo_registered_inside_the_worktrees_checkout() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "mono");
    for package in ["packages/api", "packages/web"] {
      fs::create_dir_all(main.join(package)).unwrap();
      fs::write(main.join(package).join("index.ts"), "export const x = 1;\n").unwrap();
    }
    git(&main, &["add", "."]);
    git(&main, &["commit", "--quiet", "--message", "packages"]);
    let registry = sandbox.project("acme", &[("api", &main.join("packages/api")), ("web", &main.join("packages/web"))]);
    let worktree = sandbox.worktree(&main, "../mono-feature");

    let plan = plan(&registry, &worktree.join("packages/web"));

    let paths: Vec<PathBuf> = plan.project.repos.iter().map(|repo| repo.path.clone()).collect();
    assert_eq!(paths, [worktree.join("packages/api"), worktree.join("packages/web")]);
    assert_eq!(plan.repos.len(), 2);
  }

  #[test]
  fn should_not_derive_an_overlay_when_a_registered_directory_is_missing_from_the_worktree() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "mono");
    fs::create_dir_all(main.join("untracked")).unwrap();
    let registry = sandbox.project("acme", &[("root", &main), ("untracked", &main.join("untracked"))]);
    let worktree = sandbox.worktree(&main, ".worktrees/feature");

    let derived = derive(&registry, &worktree);

    assert_eq!(
      derived,
      OverlayDerivation::Ineligible {
        reason: format!("repo untracked has no directory {} in the worktree", worktree.join("untracked").display())
      }
    );
  }

  #[test]
  fn should_not_derive_an_overlay_for_a_worktree_inside_a_registered_parent_folder() {
    let sandbox = Sandbox::new();
    let workspace = sandbox.root.join("workspace");
    let app = repository(&workspace, "app");
    let registry = sandbox.project("acme", &[("workspace", &workspace)]);
    let worktree = sandbox.worktree(&app, ".worktrees/feature");

    let derived = derive(&registry, &worktree);

    assert!(
      matches!(&derived, OverlayDerivation::Ineligible { reason } if reason.starts_with("no repo of project acme is registered inside the checkout")),
      "{derived:?}"
    );
  }

  #[test]
  fn should_name_worktrees_with_the_same_directory_name_apart_and_the_same_worktree_alike() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "shop");
    let registry = sandbox.project("acme", &[("shop", &main)]);
    let first = sandbox.worktree(&main, "../a/feature");
    let second = sandbox.worktree(&main, "../b/feature");
    let alias = sandbox.root.join("alias");
    std::os::unix::fs::symlink(&first, &alias).unwrap();

    let (first_plan, second_plan, alias_plan) = (plan(&registry, &first), plan(&registry, &second), plan(&registry, &alias.join("src")));

    assert_eq!((first_plan.label.as_str(), second_plan.label.as_str()), ("acme@feature", "acme@feature"));
    assert_ne!(first_plan.project.name, second_plan.project.name);
    assert_eq!(alias_plan.project.name, first_plan.project.name);
    assert_eq!(alias_plan.worktree, first);
  }

  #[test]
  fn should_read_the_worktrees_own_repo_file() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "shop");
    let registry = sandbox.project("acme", &[("shop", &main)]);
    let worktree = sandbox.worktree(&main, ".worktrees/feature");
    fs::write(worktree.join(MARKER_FILE), "[index]\nexclude = [\"generated/**\"]\n").unwrap();

    let plan = plan(&registry, &worktree);

    assert_eq!(plan.project.repos[0].exclude, ["generated/**"]);
    assert!(registry.project("acme").unwrap().repos[0].exclude.is_empty());
  }

  #[test]
  fn should_seed_an_overlay_once_and_record_it() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "shop");
    let registry = sandbox.project("acme", &[("shop", &main)]);
    let plan = plan(&registry, &sandbox.worktree(&main, ".worktrees/feature"));
    let overlays = Overlays::new(&sandbox.home);

    let first = overlays.acquire(&plan, TIMEOUT, write_seed).unwrap();
    let second = overlays.acquire(&plan, TIMEOUT, never_seed).unwrap();

    let db = overlays.db_path(&plan.project.name);
    assert_eq!(db, sandbox.home.data_dir().join("_worktrees").join(plan.project.name.as_str()).join("atlas.db"));
    assert_eq!(fs::read(&db).unwrap(), b"seeded");
    assert_eq!(first.seeded.as_ref().map(|seed| seed.bytes), Some(6));
    assert_eq!(second.seeded, None);
    assert_eq!(second.home.db_path(&plan.project.name), db);
    let listed = overlays.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
      (listed[0].state, listed[0].record.label.as_str(), listed[0].record.worktree.as_path()),
      (OverlayState::Ready, "acme@feature", plan.worktree.as_path())
    );
    assert!(listed[0].record.seeded_ms.is_some());
    assert!(!with_suffix(&db, SEED_SUFFIX).exists());
  }

  #[test]
  fn should_leave_no_overlay_behind_when_seeding_fails_and_seed_it_on_the_next_use() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "shop");
    let registry = sandbox.project("acme", &[("shop", &main)]);
    let plan = plan(&registry, &sandbox.worktree(&main, ".worktrees/feature"));
    let overlays = Overlays::new(&sandbox.home);
    let db = overlays.db_path(&plan.project.name);

    let failed = overlays.acquire(&plan, TIMEOUT, |staging: &Path| {
      fs::write(staging, b"half")?;
      Err::<SeedSummary, io::Error>(io::Error::other("crashed while copying"))
    });

    assert!(
      matches!(&failed, Err(OverlayError::Seed { source, .. }) if source.to_string() == "crashed while copying"),
      "{failed:?}"
    );
    assert!(!db.exists());
    assert!(!with_suffix(&db, SEED_SUFFIX).exists());
    assert_eq!(overlays.list().unwrap()[0].state, OverlayState::NotSeeded);
    let retried = overlays.acquire(&plan, TIMEOUT, write_seed).unwrap();
    assert!(retried.seeded.is_some());
    assert_eq!(fs::read(&db).unwrap(), b"seeded");
  }

  #[test]
  fn should_replace_the_staging_files_of_a_seed_that_crashed() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "shop");
    let registry = sandbox.project("acme", &[("shop", &main)]);
    let plan = plan(&registry, &sandbox.worktree(&main, ".worktrees/feature"));
    let overlays = Overlays::new(&sandbox.home);
    let db = overlays.db_path(&plan.project.name);
    let staging = with_suffix(&db, SEED_SUFFIX);
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    fs::write(&staging, b"garbage from a crash").unwrap();
    fs::write(with_suffix(&staging, "-wal"), b"stale log").unwrap();

    overlays.acquire(&plan, TIMEOUT, write_seed).unwrap();

    assert_eq!(fs::read(&db).unwrap(), b"seeded");
    assert!(!staging.exists() && !with_suffix(&staging, "-wal").exists());
  }

  #[test]
  fn should_not_install_a_seed_that_left_a_write_ahead_log_behind() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "shop");
    let registry = sandbox.project("acme", &[("shop", &main)]);
    let plan = plan(&registry, &sandbox.worktree(&main, ".worktrees/feature"));
    let overlays = Overlays::new(&sandbox.home);

    let outcome = overlays.acquire(&plan, TIMEOUT, |staging: &Path| {
      write_seed(staging)?;
      fs::write(with_suffix(staging, "-wal"), b"uncheckpointed pages")?;
      write_seed(staging)
    });

    assert!(matches!(outcome, Err(OverlayError::SeedLeftover { .. })), "{outcome:?}");
    assert!(!overlays.db_path(&plan.project.name).exists());
  }

  #[test]
  fn should_seed_once_when_two_processes_prepare_the_same_overlay_together() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "shop");
    let registry = sandbox.project("acme", &[("shop", &main)]);
    let plan = Arc::new(plan(&registry, &sandbox.worktree(&main, ".worktrees/feature")));
    let seeds = Arc::new(AtomicUsize::new(0));

    let handles: Vec<_> = (0..4)
      .map(|_| {
        let (plan, seeds, home) = (Arc::clone(&plan), Arc::clone(&seeds), sandbox.home.clone());
        thread::spawn(move || {
          Overlays::new(&home)
            .acquire(&plan, TIMEOUT, |staging: &Path| {
              seeds.fetch_add(1, Ordering::SeqCst);
              thread::sleep(Duration::from_millis(200));
              write_seed(staging)
            })
            .map(|lease| lease.record.name)
        })
      })
      .collect();
    let names: Vec<ProjectName> = handles.into_iter().map(|handle| handle.join().unwrap().unwrap()).collect();

    assert_eq!(seeds.load(Ordering::SeqCst), 1);
    assert!(names.iter().all(|name| *name == plan.project.name));
  }

  #[test]
  fn should_collect_overlays_of_removed_worktrees_and_never_touch_project_data() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "shop");
    let registry = sandbox.project("acme", &[("shop", &main)]);
    let kept = plan(&registry, &sandbox.worktree(&main, ".worktrees/kept"));
    let removed = plan(&registry, &sandbox.worktree(&main, ".worktrees/removed"));
    let overlays = Overlays::new(&sandbox.home);
    drop(overlays.acquire(&kept, TIMEOUT, write_seed).unwrap());
    drop(overlays.acquire(&removed, TIMEOUT, write_seed).unwrap());
    let base_db = sandbox.home.db_path(&ProjectName::new("acme").unwrap());
    fs::create_dir_all(base_db.parent().unwrap()).unwrap();
    fs::write(&base_db, b"base index").unwrap();
    git(&main, &["worktree", "remove", "--force", ".worktrees/removed"]);

    let collected = overlays.collect_garbage(&WorktreeSettings::default(), |name| name.as_str() == "acme", now_ms()).unwrap();

    assert_eq!(collected.len(), 1);
    assert_eq!(
      (collected[0].record.name.clone(), collected[0].reason),
      (removed.project.name.clone(), GcReason::WorktreeGone)
    );
    assert!(!overlays.db_path(&removed.project.name).parent().unwrap().exists());
    assert!(overlays.db_path(&kept.project.name).exists());
    assert_eq!(fs::read(&base_db).unwrap(), b"base index");
    let names: Vec<ProjectName> = overlays.list().unwrap().into_iter().map(|listing| listing.record.name).collect();
    assert_eq!(names, std::slice::from_ref(&kept.project.name));
  }

  #[test]
  fn should_keep_a_stale_overlay_while_a_process_uses_it() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "shop");
    let registry = sandbox.project("acme", &[("shop", &main)]);
    let plan = plan(&registry, &sandbox.worktree(&main, ".worktrees/feature"));
    let overlays = Overlays::new(&sandbox.home);
    let lease = overlays.acquire(&plan, TIMEOUT, write_seed).unwrap();
    git(&main, &["worktree", "remove", "--force", ".worktrees/feature"]);

    let while_used = overlays.collect_garbage(&WorktreeSettings::default(), |_| true, now_ms()).unwrap();
    drop(lease);
    let after = overlays.collect_garbage(&WorktreeSettings::default(), |_| true, now_ms()).unwrap();

    assert_eq!(while_used, []);
    assert_eq!(after.len(), 1);
    assert!(!overlays.db_path(&plan.project.name).exists());
  }

  #[test]
  fn should_collect_overlays_unused_for_too_long_or_whose_project_is_gone() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "shop");
    let registry = sandbox.project("acme", &[("shop", &main)]);
    let plan = plan(&registry, &sandbox.worktree(&main, ".worktrees/feature"));
    let overlays = Overlays::new(&sandbox.home);
    drop(overlays.acquire(&plan, TIMEOUT, write_seed).unwrap());
    let settings = WorktreeSettings {
      gc_after_days: 2,
      ..WorktreeSettings::default()
    };

    let tomorrow = overlays.collect_garbage(&settings, |_| true, now_ms() + DAY_MS).unwrap();
    let in_three_days = overlays.collect_garbage(&settings, |_| true, now_ms() + 3 * DAY_MS).unwrap();
    drop(overlays.acquire(&plan, TIMEOUT, write_seed).unwrap());
    let unregistered = overlays.collect_garbage(&settings, |_| false, now_ms()).unwrap();

    assert_eq!(tomorrow, []);
    assert_eq!(in_three_days.iter().map(|collected| collected.reason).collect::<Vec<_>>(), [GcReason::Unused { days: 2 }]);
    assert_eq!(unregistered.iter().map(|collected| collected.reason).collect::<Vec<_>>(), [GcReason::BaseGone]);
  }

  #[test]
  fn should_collect_at_most_once_an_hour_when_asked_opportunistically() {
    let sandbox = Sandbox::new();
    let main = repository(&sandbox.root, "shop");
    let registry = sandbox.project("acme", &[("shop", &main)]);
    let plan = plan(&registry, &sandbox.worktree(&main, ".worktrees/feature"));
    let overlays = Overlays::new(&sandbox.home);
    drop(overlays.acquire(&plan, TIMEOUT, write_seed).unwrap());
    let now = now_ms();

    let first = overlays.collect_garbage_if_due(&WorktreeSettings::default(), |_| true, now).unwrap();
    let soon = overlays.collect_garbage_if_due(&WorktreeSettings::default(), |_| false, now + 1000).unwrap();
    let later = overlays.collect_garbage_if_due(&WorktreeSettings::default(), |_| false, now + GC_INTERVAL_MS + 1).unwrap();

    assert_eq!((first.len(), soon.len(), later.len()), (0, 0, 1));
  }

  #[test]
  fn should_refuse_a_state_file_from_a_newer_catl_and_collect_nothing() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.home.data_dir()).unwrap();
    fs::write(sandbox.home.worktrees_path(), "{\"version\": 2, \"overlays\": []}").unwrap();

    let outcome = Overlays::new(&sandbox.home).collect_garbage(&WorktreeSettings::default(), |_| false, now_ms());

    assert!(
      matches!(&outcome, Err(OverlayError::State { message, .. }) if message == "unsupported version 2; this catl reads version 1"),
      "{outcome:?}"
    );
  }
}
