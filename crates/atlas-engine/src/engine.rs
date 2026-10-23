use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

use atlas_core::{ContentHash, Owner, RelPath, RepoName, RunId};
use atlas_project::{AtlasHome, Project};
use atlas_scip::ScipFacts;
use atlas_similarity::SimilarityIndex;
use atlas_store::{RunOutcome, RunReport, RunStatus, Store, StoreOptions};
use rayon::prelude::*;

use crate::analyze::Toolchain;
use crate::error::{EngineError, EngineResult};
use crate::flight::SingleFlight;
use crate::lock::{Contention, IndexLock};
use crate::options::{BarrierMode, EngineOptions, IndexMode};
use crate::passes::is_reserved_pass_name;
use crate::report::{FailedFile, FreshnessHeader, FreshnessReport, RunOutcomeKind, RunSummary, ScanKind, ScipLag, SkippedSummary};
use crate::run::{RunEnv, execute, key_label};
use crate::scan::{RepoScan, ScanOutcome, scan_repo};
use crate::scip::now_ms;
use crate::similarity::{SIMILARITY_BLOB, SIMILARITY_BLOB_VERSION, encode_persisted};
use crate::state::{
  DIRTY_BLOB, EngineMeta, EngineState, RepoHealth, SCIP_BLOB, SCIP_BLOB_VERSION, ScipMeta, all_skipped, load_state, project_fingerprint, read_project_fingerprint,
  write_project_fingerprint, write_skipped,
};
use crate::watch::WatchSignals;

const INTERRUPTED_RUN_SCAN: usize = 100;
const INDEX_TRIGGERS: [&str; 3] = ["barrier", "index", WRITE_TRIGGER];
const WRITE_TRIGGER: &str = "edit";
const FORCE_REHASH_MTIME_NS: i64 = i64::MIN;
pub(crate) const PROJECT_CHANGED: &str = "project configuration changed; reopen the engine";
const BARRIER_SKIPPED: &str = "the freshness barrier was skipped (benchmark only); the answer may be stale";
pub const REFRESH_DEFERRED: &str = "refresh left to another caller, the index may be stale";

pub(crate) struct Inner {
  pub state: EngineState,
  pub lock: IndexLock,
  pub health: StateHealth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StateHealth {
  Synced,
  NeedsReload { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recovery {
  None,
  Crashed,
}

struct ProjectHandle {
  project: Arc<Project>,
  fingerprint: String,
  toolchain: Arc<Toolchain>,
}

pub struct Engine {
  project: RwLock<ProjectHandle>,
  pub(crate) home: AtlasHome,
  pub(crate) store: Store,
  pub(crate) options: EngineOptions,
  pub(crate) inner: Mutex<Inner>,
  barriers: SingleFlight<FreshnessReport>,
  scip_meta: Mutex<Option<DecodedScipMeta>>,
  pub(crate) similarity: RwLock<Arc<SimilarityIndex>>,
  pub(crate) watch: Arc<WatchSignals>,
}

pub(crate) fn lock_inner(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
  inner.lock().unwrap_or_else(PoisonError::into_inner)
}

struct PersistedView {
  generation: Option<RunId>,
  in_progress: bool,
  project: Option<String>,
  scip: Option<Vec<u8>>,
}

struct DecodedScipMeta {
  bytes: Option<Vec<u8>>,
  meta: ScipMeta,
}

struct Written<R> {
  written: std::thread::Result<R>,
  started: Instant,
  refreshed: EngineResult<FreshnessReport>,
}

enum Refreshed {
  Ran(RunSummary),
  Clean(Vec<FailedFile>),
}

enum Plan {
  Clean(Vec<RepoScan>),
  Run(Vec<RepoScan>),
}

enum Held {
  Ran(RunSummary),
  Clean,
  Deferred(String),
}

impl Engine {
  pub fn open(project: &Project, home: &AtlasHome, options: EngineOptions) -> EngineResult<Self> {
    let mut names = std::collections::BTreeSet::new();
    if let Some(pass) = options.passes.iter().find(|pass| is_reserved_pass_name(pass.name()) || !names.insert(pass.name())) {
      return Err(EngineError::InvalidPass(pass.name().to_owned()));
    }
    let data_dir = home.project_data_dir(&project.name);
    std::fs::create_dir_all(&data_dir).map_err(|source| EngineError::io(&data_dir, source))?;
    let db_path = home.db_path(&project.name);
    let store = Store::open_with(&db_path, options.store.clone())?;
    let toolchain = Arc::new(Toolchain::new(project, &options.extra_detectors, options.parse_budget));
    let fingerprint = project_fingerprint(project);
    adopt_project(&store, &fingerprint)?;
    let loaded = load_state(&store, project, &toolchain, &options)?;
    let mut lock = IndexLock::open(&db_path)?;
    let mut state = loaded.state;
    if loaded.interrupted {
      lock.try_hold(|| recover_interrupted(&store, project, &mut state))?.transpose()?;
    }
    Ok(Self {
      project: RwLock::new(ProjectHandle {
        project: Arc::new(project.clone()),
        fingerprint,
        toolchain,
      }),
      home: home.clone(),
      store,
      inner: Mutex::new(Inner {
        state,
        lock,
        health: StateHealth::Synced,
      }),
      barriers: SingleFlight::default(),
      scip_meta: Mutex::new(None),
      similarity: RwLock::new(Arc::new(loaded.similarity)),
      watch: Arc::new(WatchSignals::default()),
      options,
    })
  }

  pub fn project(&self) -> Arc<Project> {
    Arc::clone(&self.project.read().unwrap_or_else(PoisonError::into_inner).project)
  }

  fn fingerprint(&self) -> String {
    self.project.read().unwrap_or_else(PoisonError::into_inner).fingerprint.clone()
  }

  pub(crate) fn toolchain(&self) -> Arc<Toolchain> {
    Arc::clone(&self.project.read().unwrap_or_else(PoisonError::into_inner).toolchain)
  }

  pub fn reload_project(&self, project: &Project) -> EngineResult<()> {
    let current = self.project();
    if project.name != current.name {
      return Err(EngineError::ProjectMismatch {
        expected: current.name.to_string(),
        found: project.name.to_string(),
      });
    }
    let mut inner = lock_inner(&self.inner);
    let fingerprint = project_fingerprint(project);
    adopt_project(&self.store, &fingerprint)?;
    let toolchain = match project.settings.detect == current.settings.detect {
      true => self.toolchain(),
      false => Arc::new(Toolchain::new(project, &self.options.extra_detectors, self.options.parse_budget)),
    };
    *self.project.write().unwrap_or_else(PoisonError::into_inner) = ProjectHandle {
      project: Arc::new(project.clone()),
      fingerprint,
      toolchain,
    };
    let Inner { state, health, .. } = &mut *inner;
    *health = StateHealth::NeedsReload {
      reason: "the project configuration was reloaded".to_owned(),
    };
    self.reload(state, health, Recovery::None)
  }

  pub fn store(&self) -> &Store {
    &self.store
  }

  pub fn db_path(&self) -> PathBuf {
    self.home.db_path(&self.project().name)
  }

  pub fn store_options(&self) -> &StoreOptions {
    &self.options.store
  }

  pub fn similarity(&self) -> Arc<SimilarityIndex> {
    Arc::clone(&crate::similarity::read_lock(&self.similarity))
  }

  pub fn persist_similarity(&self) -> EngineResult<bool> {
    let mut inner = lock_inner(&self.inner);
    if inner.health != StateHealth::Synced {
      return Ok(false);
    }
    let state = &mut inner.state;
    let Some(generation) = state.generation else { return Ok(false) };
    if state.similarity_persisted == Some(generation) {
      return Ok(false);
    }
    let bytes = encode_persisted(&crate::similarity::read_lock(&self.similarity), generation);
    self.store.write(|tx| tx.put_blob(SIMILARITY_BLOB, SIMILARITY_BLOB_VERSION, &bytes))?;
    state.similarity_persisted = Some(generation);
    tracing::debug!(generation = generation.0, bytes = bytes.len(), "persisted the similarity index");
    Ok(true)
  }

  pub fn persisted_generation(&self) -> EngineResult<Option<RunId>> {
    Self::persisted_generation_in(&self.store)
  }

  pub fn persisted_generation_in(store: &Store) -> EngineResult<Option<RunId>> {
    store.read(|tx| -> EngineResult<Option<RunId>> { Ok(EngineMeta::read(tx)?.and_then(|meta| meta.generation).map(RunId)) })
  }

  pub fn index_run_in_progress(&self) -> EngineResult<bool> {
    self.store.read(|tx| -> EngineResult<bool> { Ok(tx.blob(DIRTY_BLOB)?.is_some()) })
  }

  pub fn ensure_fresh(&self) -> EngineResult<FreshnessReport> {
    let arrived = Instant::now();
    let mut report = self.barriers.board(|| self.fly_barrier());
    match &mut report {
      FreshnessReport::Fresh(header) | FreshnessReport::Degraded { header, .. } => header.duration = arrived.elapsed(),
    }
    Ok(report)
  }

  #[doc(hidden)]
  pub fn barrier_scans(&self) -> u64 {
    self.barriers.launched()
  }

  #[doc(hidden)]
  pub fn skip_barrier(&self) -> FreshnessReport {
    let inner = lock_inner(&self.inner);
    FreshnessReport::Degraded {
      header: self.header(&inner.state, None, 0, Duration::ZERO, ScanKind::StatScan),
      failed_files: Vec::new(),
      reason: BARRIER_SKIPPED.to_owned(),
    }
  }

  fn fly_barrier(&self) -> FreshnessReport {
    let started = Instant::now();
    let mut inner = lock_inner(&self.inner);
    let outcome = catch_panic(|| self.barrier(&mut inner, started));
    outcome.unwrap_or_else(|error| self.failed_refresh(&mut inner, error, started))
  }

  fn failed_refresh(&self, inner: &mut Inner, error: EngineError, started: Instant) -> FreshnessReport {
    let reason = match error {
      EngineError::ProjectConfigChanged => PROJECT_CHANGED.to_owned(),
      error => {
        tracing::error!(error = %error, "freshness barrier could not bring the index up to date");
        inner.health = StateHealth::NeedsReload { reason: error.to_string() };
        format!("index update failed: {error}")
      }
    };
    FreshnessReport::Degraded {
      header: self.header(&inner.state, None, 0, started.elapsed(), ScanKind::StatScan),
      failed_files: Vec::new(),
      reason,
    }
  }

  /// `write` must not call back into this engine: it runs under the engine mutex, which is not reentrant.
  pub fn write_then_refresh<R>(&self, files: &[(RepoName, RelPath)], write: impl FnOnce() -> R) -> EngineResult<(R, FreshnessReport)> {
    let project = self.project();
    if let Some((repo, _)) = files.iter().find(|(repo, _)| !project.repos.iter().any(|config| config.name == *repo)) {
      return Err(EngineError::UnknownRepo(repo.clone()));
    }
    let span = tracing::info_span!("write_then_refresh", files = files.len());
    let _entered = span.enter();
    let timeout = self.options.store.lock_timeout;
    let mut inner = lock_inner(&self.inner);
    let Inner { state, lock, health } = &mut *inner;
    let outcome = lock.hold(timeout, |_| -> EngineResult<Written<R>> {
      catch_panic(|| self.sync_holding_lock(state, health))?;
      let written = std::panic::catch_unwind(std::panic::AssertUnwindSafe(write));
      self.watch.mark_event();
      force_rehash(state, files);
      let started = Instant::now();
      let refreshed = catch_panic(|| self.refresh_holding_lock(state, started));
      Ok(Written { written, started, refreshed })
    });
    let Written { written, started, refreshed } = match outcome.and_then(|held| held) {
      Ok(written) => written,
      Err(error) => {
        if !matches!(error, EngineError::ProjectConfigChanged) {
          inner.health = StateHealth::NeedsReload { reason: error.to_string() };
        }
        return Err(error);
      }
    };
    let report = refreshed.unwrap_or_else(|error| self.failed_refresh(&mut inner, error, started));
    drop(inner);
    match written {
      Ok(value) => Ok((value, report)),
      Err(payload) => std::panic::resume_unwind(payload),
    }
  }

  fn refresh_holding_lock(&self, state: &mut EngineState, started: Instant) -> EngineResult<FreshnessReport> {
    let token = self.watch.begin_scan();
    let refreshed = match self.plan(state) {
      Plan::Clean(scans) => self.settle_clean(state, &scans).map(|()| Refreshed::Clean(scan_failures(&scans))),
      Plan::Run(scans) => self.run(state, scans, IndexMode::Incremental, WRITE_TRIGGER).map(Refreshed::Ran),
    };
    let refreshed = match refreshed {
      Ok(refreshed) => refreshed,
      Err(error) => {
        self.watch.abort_scan(token);
        return Err(error);
      }
    };
    self.watch.finish_scan(token);
    Ok(match refreshed {
      Refreshed::Ran(summary) => self.summarized(state, summary, started),
      Refreshed::Clean(failures) => self.settled(state, None, 0, started, ScanKind::StatScan, failures),
    })
  }

  pub fn index(&self, mode: IndexMode) -> EngineResult<RunSummary> {
    let mut inner = lock_inner(&self.inner);
    let summary = catch_panic(|| self.locked_index(&mut inner, mode));
    if let Some(error) = summary.as_ref().err().filter(|error| !matches!(error, EngineError::ProjectConfigChanged)) {
      inner.health = StateHealth::NeedsReload { reason: error.to_string() };
    }
    summary
  }

  fn locked_index(&self, inner: &mut Inner, mode: IndexMode) -> EngineResult<RunSummary> {
    self.sync(inner)?;
    let timeout = self.options.store.lock_timeout;
    let Inner { state, lock, health } = inner;
    let token = self.watch.begin_scan();
    let summary = lock.hold(timeout, |_| -> EngineResult<RunSummary> {
      self.sync_holding_lock(state, health)?;
      let scans = self.scan_all(state);
      self.run(state, scans, mode, "index")
    })??;
    self.watch.finish_scan(token);
    Ok(summary)
  }

  fn barrier(&self, inner: &mut Inner, started: Instant) -> EngineResult<FreshnessReport> {
    let in_progress = self.sync(inner)?;
    if !in_progress && self.watch.is_clean(self.options.full_scan_interval) && !inner.state.needs_run(now_ms()) {
      return Ok(self.settled(&inner.state, None, 0, started, ScanKind::WatcherClean, Vec::new()));
    }
    let token = self.watch.begin_scan();
    let scans = match (in_progress, self.plan(&inner.state)) {
      (false, Plan::Clean(scans)) => {
        let failures = scan_failures(&scans);
        self.settle_clean(&mut inner.state, &scans)?;
        self.watch.finish_scan(token);
        return Ok(self.settled(&inner.state, None, 0, started, ScanKind::StatScan, failures));
      }
      (false, Plan::Run(scans)) => match self.refresh_refusal(&inner.state, &scans) {
        Some(reason) => {
          self.watch.abort_scan(token);
          return Ok(self.deferred(&inner.state, started, reason));
        }
        None => Some(scans),
      },
      (true, _) => None,
    };
    let timeout = self.options.store.lock_timeout;
    self.options.fault_injection.pause_before_index_lock();
    let Inner { state, lock, health } = inner;
    let outcome = lock.hold(timeout, |contention| -> EngineResult<Held> {
      let reloaded = self.sync_holding_lock(state, health)?;
      let scans = match (contention, scans, reloaded) {
        (Contention::Uncontended, Some(scans), false) => scans,
        _ => match self.plan(state) {
          Plan::Clean(scans) => {
            self.settle_clean(state, &scans)?;
            return Ok(Held::Clean);
          }
          Plan::Run(scans) => match self.refresh_refusal(state, &scans) {
            Some(reason) => return Ok(Held::Deferred(reason)),
            None => scans,
          },
        },
      };
      self.run(state, scans, IndexMode::Incremental, "barrier").map(Held::Ran)
    });
    let held = match outcome {
      Ok(Ok(held)) => held,
      Ok(Err(error)) | Err(error) => {
        self.watch.abort_scan(token);
        return Err(error);
      }
    };
    match held {
      Held::Deferred(reason) => {
        self.watch.abort_scan(token);
        Ok(self.deferred(&inner.state, started, reason))
      }
      Held::Ran(summary) => {
        self.watch.finish_scan(token);
        Ok(self.summarized(&inner.state, summary, started))
      }
      Held::Clean => {
        self.watch.finish_scan(token);
        Ok(self.settled(&inner.state, None, 0, started, ScanKind::StatScan, Vec::new()))
      }
    }
  }

  fn refresh_refusal(&self, state: &EngineState, scans: &[RepoScan]) -> Option<String> {
    if self.options.barrier == BarrierMode::Refresh {
      return None;
    }
    if state.generation.is_none() {
      return Some("the project has never been indexed".to_owned());
    }
    if !state.pending.is_empty() {
      return Some("the index has pending work (re-analysis, a full re-resolve, a purge or a pass re-run)".to_owned());
    }
    if state.has_due_budget_retry(now_ms()) {
      return Some("a file that overran its parse budget is due for a retry".to_owned());
    }
    let mut changed = 0;
    for scan in scans {
      match &scan.outcome {
        ScanOutcome::Scanned(scanned) => {
          let changes = &scanned.changes;
          changed += changes.added.len() + changes.modified.len() + changes.deleted.len();
        }
        ScanOutcome::Missing { .. } if state.repos.get(&scan.repo).is_some_and(|repo| scan.needs_run(repo)) => {
          return Some(format!("repository {} is unavailable", scan.repo));
        }
        ScanOutcome::Missing { .. } => {}
      }
    }
    Some(match changed {
      0 => "the working tree changed since the last index run".to_owned(),
      1 => "1 file changed since the last index run".to_owned(),
      changed => format!("{changed} files changed since the last index run"),
    })
  }

  fn deferred(&self, state: &EngineState, started: Instant, reason: String) -> FreshnessReport {
    tracing::debug!(reason = %reason, "freshness barrier deferred the refresh");
    self.degraded(state, None, 0, started, ScanKind::StatScan, Vec::new(), format!("{REFRESH_DEFERRED}: {reason}"))
  }

  fn summarized(&self, state: &EngineState, summary: RunSummary, started: Instant) -> FreshnessReport {
    let mut failures = summary.failed_files;
    failures.extend(missing_repos(state));
    failures.sort();
    failures.dedup();
    let run = Some(summary.run);
    match summary.outcome {
      RunOutcomeKind::Ok if failures.is_empty() => FreshnessReport::Fresh(self.header(state, run, summary.files_updated, started.elapsed(), ScanKind::StatScan)),
      RunOutcomeKind::Ok => self.degraded(
        state,
        run,
        summary.files_updated,
        started,
        ScanKind::StatScan,
        failures,
        "some files or repositories could not be indexed".to_owned(),
      ),
      RunOutcomeKind::Partial { reason } => self.degraded(state, run, summary.files_updated, started, ScanKind::StatScan, failures, reason),
    }
  }

  fn settled(&self, state: &EngineState, run: Option<RunId>, files: u64, started: Instant, scan: ScanKind, mut failures: Vec<FailedFile>) -> FreshnessReport {
    failures.extend(missing_repos(state));
    failures.sort();
    failures.dedup();
    match failures.is_empty() {
      true => FreshnessReport::Fresh(self.header(state, run, files, started.elapsed(), scan)),
      false => {
        let reason = format!("{} file(s) or repositories could not be indexed", failures.len());
        self.degraded(state, run, files, started, scan, failures, reason)
      }
    }
  }

  #[allow(clippy::too_many_arguments)]
  fn degraded(&self, state: &EngineState, run: Option<RunId>, files: u64, started: Instant, scan: ScanKind, failed_files: Vec<FailedFile>, reason: String) -> FreshnessReport {
    FreshnessReport::Degraded {
      header: self.header(state, run, files, started.elapsed(), scan),
      failed_files,
      reason,
    }
  }

  pub(crate) fn header(&self, state: &EngineState, run: Option<RunId>, files_updated: u64, duration: Duration, scan: ScanKind) -> FreshnessHeader {
    let scip = match (&state.scip.last_run_ms, state.scip.lagging.len()) {
      (None, _) => ScipLag::NeverRun,
      (Some(_), 0) => ScipLag::Current,
      (Some(_), files) => ScipLag::Behind { files: files as u64 },
    };
    FreshnessHeader {
      run,
      last_run: state.generation,
      files_updated,
      duration,
      scan,
      scip,
      stale_inferred_facts: state.stale_inferred,
      files_failed_analysis: state.files.values().filter(|file| file.failure.is_some()).count() as u64,
      skipped: SkippedSummary::of(state.repos.values().flat_map(|repo| &repo.skipped)),
    }
  }

  pub(crate) fn sync(&self, inner: &mut Inner) -> EngineResult<bool> {
    let view = self.persisted_view()?;
    let Inner { state, health, lock } = inner;
    self.adopt_scip_meta(state, view.scip)?;
    if view.in_progress && !state.pending.force_full_resolve {
      let recovered = lock.try_hold(|| self.reload(state, health, Recovery::Crashed))?;
      return match recovered {
        Some(result) => result.map(|()| false),
        None => Ok(true),
      };
    }
    if *health != StateHealth::Synced || view.generation != state.generation {
      self.reload(state, health, Recovery::None)?;
    }
    Ok(false)
  }

  fn sync_holding_lock(&self, state: &mut EngineState, health: &mut StateHealth) -> EngineResult<bool> {
    let view = self.persisted_view()?;
    self.adopt_scip_meta(state, view.scip)?;
    let recovery = match view.in_progress && !state.pending.force_full_resolve {
      true => Recovery::Crashed,
      false => Recovery::None,
    };
    if recovery == Recovery::None && *health == StateHealth::Synced && view.generation == state.generation {
      return Ok(false);
    }
    self.reload(state, health, recovery)?;
    Ok(true)
  }

  fn persisted_view(&self) -> EngineResult<PersistedView> {
    let view = self.store.read(|tx| -> EngineResult<PersistedView> {
      Ok(PersistedView {
        generation: EngineMeta::read(tx)?.and_then(|meta| meta.generation).map(RunId),
        in_progress: tx.blob(DIRTY_BLOB)?.is_some(),
        project: read_project_fingerprint(tx)?,
        scip: tx.blob(SCIP_BLOB)?.filter(|blob| blob.version == SCIP_BLOB_VERSION).map(|blob| blob.bytes),
      })
    })?;
    if view.project.as_ref().is_some_and(|persisted| *persisted != self.fingerprint()) {
      tracing::warn!("another engine adopted a different project configuration; this engine must be reopened");
      return Err(EngineError::ProjectConfigChanged);
    }
    Ok(view)
  }

  fn adopt_scip_meta(&self, state: &mut EngineState, bytes: Option<Vec<u8>>) -> EngineResult<()> {
    let mut decoded = self.scip_meta.lock().unwrap_or_else(PoisonError::into_inner);
    let cached = match decoded.take().filter(|cached| cached.bytes == bytes) {
      Some(cached) => cached,
      None => DecodedScipMeta {
        meta: match &bytes {
          Some(bytes) => serde_json::from_slice(bytes).map_err(|error| EngineError::codec("scip meta", error))?,
          None => ScipMeta::default(),
        },
        bytes,
      },
    };
    if state.scip != cached.meta {
      state.scip = cached.meta.clone();
    }
    *decoded = Some(cached);
    Ok(())
  }

  fn reload(&self, state: &mut EngineState, health: &mut StateHealth, recovery: Recovery) -> EngineResult<()> {
    match &*health {
      StateHealth::NeedsReload { reason } => tracing::info!(reason = %reason, "reloading engine state"),
      StateHealth::Synced => tracing::info!(ours = ?state.generation, recovery = ?recovery, "the index changed outside this engine; reloading state"),
    }
    let project = self.project();
    let loaded = load_state(&self.store, &project, &self.toolchain(), &self.options)?;
    *state = loaded.state;
    if recovery == Recovery::Crashed {
      recover_interrupted(&self.store, &project, state)?;
    }
    *crate::similarity::write_lock(&self.similarity) = Arc::new(loaded.similarity);
    *health = StateHealth::Synced;
    Ok(())
  }

  fn scan_all(&self, state: &EngineState) -> Vec<RepoScan> {
    let max_file_bytes = self.project().settings.max_file_bytes;
    let repos: Vec<_> = state.repos.values().collect();
    repos.into_par_iter().map(|repo| scan_repo(repo, max_file_bytes)).collect()
  }

  fn plan(&self, state: &EngineState) -> Plan {
    let scans = self.scan_all(state);
    let needs_run = state.needs_run(now_ms()) || scans.iter().any(|scan| state.repos.get(&scan.repo).is_some_and(|repo| scan.needs_run(repo)));
    match needs_run {
      true => Plan::Run(scans),
      false => Plan::Clean(scans),
    }
  }

  fn settle_clean(&self, state: &mut EngineState, scans: &[RepoScan]) -> EngineResult<()> {
    let restamps: Vec<(RepoName, RelPath, atlas_project::FileStamp)> = scans
      .iter()
      .flat_map(|scan| {
        let changes = match &scan.outcome {
          ScanOutcome::Scanned(scanned) => Some(&scanned.changes),
          ScanOutcome::Missing { .. } => None,
        };
        changes.into_iter().flat_map(|changes| {
          changes
            .restamped
            .iter()
            .filter_map(|path| Some((scan.repo.clone(), path.clone(), changes.stamps.get(path)?.clone())))
        })
      })
      .collect();
    if !restamps.is_empty() {
      self.store.write(|tx| -> EngineResult<()> {
        for (repo, path, stamp) in &restamps {
          if let Some(mut record) = tx.file(repo, path)? {
            record.size = stamp.size;
            record.mtime_ns = stamp.mtime_ns;
            record.inode = Some(stamp.inode);
            tx.upsert_file(&record)?;
          }
        }
        Ok(())
      })?;
    }
    for (repo, path, stamp) in restamps {
      if let Some(repo) = state.repos.get_mut(&repo) {
        repo.stamps.insert(path, stamp);
      }
    }
    let mut skipped_changed = false;
    for scan in scans {
      let Some(repo) = state.repos.get_mut(&scan.repo) else { continue };
      match &scan.outcome {
        ScanOutcome::Scanned(scanned) => {
          repo.health = RepoHealth::Present;
          repo.scan_started_ns = Some(scan.started_ns);
          if repo.skipped != scanned.skipped {
            repo.skipped = scanned.skipped.clone();
            skipped_changed = true;
          }
        }
        ScanOutcome::Missing { reason } => {
          repo.health = RepoHealth::Missing { reason: reason.clone() };
          skipped_changed |= !std::mem::take(&mut repo.skipped).is_empty();
        }
      }
    }
    if skipped_changed {
      let skipped = all_skipped(&state.repos);
      self.store.write(|tx| write_skipped(tx, &skipped))?;
    }
    Ok(())
  }

  fn run(&self, state: &mut EngineState, scans: Vec<RepoScan>, mode: IndexMode, trigger: &str) -> EngineResult<RunSummary> {
    let project = self.project();
    let toolchain = self.toolchain();
    let env = RunEnv {
      store: &self.store,
      project: &project,
      toolchain: &toolchain,
      options: &self.options,
    };
    execute(&env, state, &self.similarity, scans, mode, trigger)
  }

  pub fn read_source(&self, repo: &RepoName, path: &RelPath) -> EngineResult<(String, ContentHash)> {
    let project = self.project();
    let config = project
      .repos
      .iter()
      .find(|candidate| candidate.name == *repo)
      .ok_or_else(|| EngineError::UnknownRepo(repo.clone()))?;
    let indexed = self.store.read(|tx| tx.file(repo, path))?.ok_or_else(|| EngineError::NotIndexed {
      repo: repo.clone(),
      path: path.clone(),
    })?;
    let absolute = config.path.join(path.as_str());
    let bytes = std::fs::read(&absolute).map_err(|source| EngineError::io(&absolute, source))?;
    let hash = ContentHash::of(&bytes);
    if hash != indexed.content_hash {
      return Err(EngineError::SourceChanged {
        repo: repo.clone(),
        path: path.clone(),
      });
    }
    let text = String::from_utf8(bytes).map_err(|error| EngineError::codec("source text", error))?;
    Ok((text, hash))
  }
}

impl Drop for Engine {
  fn drop(&mut self) {
    if let Err(error) = self.persist_similarity() {
      tracing::warn!(error = %error, "could not persist the similarity index on close");
    }
  }
}

fn catch_panic<T>(f: impl FnOnce() -> EngineResult<T>) -> EngineResult<T> {
  std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|payload| {
    let reason = crate::analyze::panic_message(payload.as_ref());
    tracing::error!(reason = %reason, "index update panicked; engine state will be reloaded");
    Err(EngineError::Panicked(reason))
  })
}

fn adopt_project(store: &Store, fingerprint: &str) -> EngineResult<()> {
  if store.read(read_project_fingerprint)?.as_deref() == Some(fingerprint) {
    return Ok(());
  }
  store.write(|tx| write_project_fingerprint(tx, fingerprint))
}

fn recover_interrupted(store: &Store, project: &Project, state: &mut EngineState) -> EngineResult<()> {
  tracing::warn!("a previous index run did not finish; the next run re-resolves every file");
  state.pending.force_full_resolve = true;
  let interrupted: Vec<RunId> = store
    .read(|tx| tx.runs(INTERRUPTED_RUN_SCAN))?
    .into_iter()
    .filter(|run| run.status == RunStatus::Running && INDEX_TRIGGERS.contains(&run.trigger.as_str()))
    .map(|run| run.id)
    .collect();
  let scip = store.write(|tx| -> EngineResult<ScipMeta> {
    for run in interrupted {
      tx.finish_run(run, RunOutcome::Failed { error: "interrupted".to_owned() }, &RunReport::default())?;
    }
    let mut scip = ScipMeta::read(tx)?;
    if scip.last_run_ms.is_none() {
      return Ok(scip);
    }
    for repo in &project.repos {
      for record in tx.files_in_repo(&repo.name)? {
        let owner = Owner::Project {
          pass: ScipFacts::owner_pass(&record.repo, &record.path),
        };
        let facts = tx.owned_facts(&owner)?;
        let outdated = facts
          .edges
          .iter()
          .flat_map(|edge| &edge.evidence)
          .any(|evidence| evidence.content_hash != record.content_hash);
        if outdated {
          scip.lagging.insert(key_label(&(record.repo.clone(), record.path.clone())));
        }
      }
    }
    scip.write(tx)?;
    Ok(scip)
  })?;
  state.scip = scip;
  Ok(())
}

fn force_rehash(state: &mut EngineState, files: &[(RepoName, RelPath)]) {
  for (repo, path) in files {
    if let Some(stamp) = state.repos.get_mut(repo).and_then(|repo| repo.stamps.get_mut(path)) {
      stamp.mtime_ns = FORCE_REHASH_MTIME_NS;
    }
  }
}

fn scan_failures(scans: &[RepoScan]) -> Vec<FailedFile> {
  let mut failures = Vec::new();
  for scan in scans {
    match &scan.outcome {
      ScanOutcome::Scanned(scanned) => {
        failures.extend(scanned.changes.errors.iter().map(|error| FailedFile {
          repo: scan.repo.clone(),
          path: Some(error.path.clone()),
          reason: format!("unreadable: {}", error.message),
        }));
        failures.extend(scanned.walk_errors.iter().map(|error| FailedFile {
          repo: scan.repo.clone(),
          path: None,
          reason: format!("walk error: {error}"),
        }));
      }
      ScanOutcome::Missing { .. } => {}
    }
  }
  failures
}

fn missing_repos(state: &EngineState) -> Vec<FailedFile> {
  state
    .repos
    .iter()
    .filter_map(|(name, repo)| match &repo.health {
      RepoHealth::Present => None,
      RepoHealth::Missing { reason } => Some(FailedFile {
        repo: name.clone(),
        path: None,
        reason: format!("repository is unavailable: {reason}"),
      }),
    })
    .collect()
}
