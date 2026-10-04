use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

use atlas_core::{ContentHash, Owner, RelPath, RepoName, RunId};
use atlas_project::{AtlasHome, Project};
use atlas_scip::ScipFacts;
use atlas_similarity::SimilarityIndex;
use atlas_store::{RunOutcome, RunReport, RunStatus, Store, StoreOptions};

use crate::analyze::Toolchain;
use crate::error::{EngineError, EngineResult};
use crate::lock::{Contention, IndexLock};
use crate::options::{EngineOptions, IndexMode};
use crate::passes::is_reserved_pass_name;
use crate::report::{FailedFile, FreshnessHeader, FreshnessReport, RunOutcomeKind, RunSummary, ScanKind, ScipLag, SkippedSummary};
use crate::run::{RunEnv, execute, key_label};
use crate::scan::{RepoScan, ScanOutcome, scan_repo};
use crate::similarity::{SIMILARITY_BLOB, SIMILARITY_BLOB_VERSION, encode_persisted};
use crate::state::{
  DIRTY_BLOB, EngineMeta, EngineState, RepoHealth, ScipMeta, all_skipped, load_state, project_fingerprint, read_project_fingerprint, write_project_fingerprint, write_skipped,
};
use crate::watch::WatchSignals;

const INTERRUPTED_RUN_SCAN: usize = 100;
const INDEX_TRIGGERS: [&str; 2] = ["barrier", "index"];
pub(crate) const PROJECT_CHANGED: &str = "project configuration changed; reopen the engine";

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
}

pub struct Engine {
  project: RwLock<ProjectHandle>,
  pub(crate) home: AtlasHome,
  pub(crate) store: Store,
  pub(crate) options: EngineOptions,
  pub(crate) toolchain: Toolchain,
  pub(crate) inner: Mutex<Inner>,
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
  scip: ScipMeta,
}

enum Plan {
  Clean(Vec<RepoScan>),
  Run(Vec<RepoScan>),
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
    let toolchain = Toolchain::new(&options.extra_detectors);
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
      }),
      home: home.clone(),
      store,
      toolchain,
      inner: Mutex::new(Inner {
        state,
        lock,
        health: StateHealth::Synced,
      }),
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
    *self.project.write().unwrap_or_else(PoisonError::into_inner) = ProjectHandle {
      project: Arc::new(project.clone()),
      fingerprint,
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

  pub fn ensure_fresh(&self) -> EngineResult<FreshnessReport> {
    let started = Instant::now();
    let mut inner = lock_inner(&self.inner);
    let outcome = catch_panic(|| self.barrier(&mut inner, started));
    match outcome {
      Ok(report) => Ok(report),
      Err(EngineError::ProjectConfigChanged) => Ok(FreshnessReport::Degraded {
        header: self.header(&inner.state, None, 0, started.elapsed(), ScanKind::StatScan),
        failed_files: Vec::new(),
        reason: PROJECT_CHANGED.to_owned(),
      }),
      Err(error) => {
        tracing::error!(error = %error, "freshness barrier could not bring the index up to date");
        inner.health = StateHealth::NeedsReload { reason: error.to_string() };
        Ok(FreshnessReport::Degraded {
          header: self.header(&inner.state, None, 0, started.elapsed(), ScanKind::StatScan),
          failed_files: Vec::new(),
          reason: format!("index update failed: {error}"),
        })
      }
    }
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
    if !in_progress && self.watch.is_clean(self.options.full_scan_interval) && inner.state.pending.is_empty() {
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
      (false, Plan::Run(scans)) => Some(scans),
      (true, _) => None,
    };
    let timeout = self.options.store.lock_timeout;
    self.options.fault_injection.pause_before_index_lock();
    let Inner { state, lock, health } = inner;
    let outcome = lock.hold(timeout, |contention| -> EngineResult<Option<RunSummary>> {
      let reloaded = self.sync_holding_lock(state, health)?;
      let scans = match (contention, scans, reloaded) {
        (Contention::Uncontended, Some(scans), false) => scans,
        _ => match self.plan(state) {
          Plan::Clean(scans) => {
            self.settle_clean(state, &scans)?;
            return Ok(None);
          }
          Plan::Run(scans) => scans,
        },
      };
      self.run(state, scans, IndexMode::Incremental, "barrier").map(Some)
    });
    let summary = match outcome {
      Ok(Ok(summary)) => summary,
      Ok(Err(error)) | Err(error) => {
        self.watch.abort_scan(token);
        return Err(error);
      }
    };
    self.watch.finish_scan(token);
    let Some(summary) = summary else {
      return Ok(self.settled(&inner.state, None, 0, started, ScanKind::StatScan, Vec::new()));
    };
    let mut failures = summary.failed_files.clone();
    failures.extend(missing_repos(&inner.state));
    failures.sort();
    failures.dedup();
    let run = Some(summary.run);
    Ok(match summary.outcome {
      RunOutcomeKind::Ok if failures.is_empty() => FreshnessReport::Fresh(self.header(&inner.state, run, summary.files_updated, started.elapsed(), ScanKind::StatScan)),
      RunOutcomeKind::Ok => self.degraded(
        &inner.state,
        run,
        summary.files_updated,
        started,
        ScanKind::StatScan,
        failures,
        "some files or repositories could not be indexed".to_owned(),
      ),
      RunOutcomeKind::Partial { reason } => self.degraded(&inner.state, run, summary.files_updated, started, ScanKind::StatScan, failures, reason),
    })
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
    state.scip = view.scip.clone();
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
    state.scip = view.scip.clone();
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
        scip: ScipMeta::read(tx)?,
      })
    })?;
    if view.project.as_ref().is_some_and(|persisted| *persisted != self.fingerprint()) {
      tracing::warn!("another engine adopted a different project configuration; this engine must be reopened");
      return Err(EngineError::ProjectConfigChanged);
    }
    Ok(view)
  }

  fn reload(&self, state: &mut EngineState, health: &mut StateHealth, recovery: Recovery) -> EngineResult<()> {
    match &*health {
      StateHealth::NeedsReload { reason } => tracing::info!(reason = %reason, "reloading engine state"),
      StateHealth::Synced => tracing::info!(ours = ?state.generation, recovery = ?recovery, "the index changed outside this engine; reloading state"),
    }
    let project = self.project();
    let loaded = load_state(&self.store, &project, &self.toolchain, &self.options)?;
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
    state.repos.values().map(|repo| scan_repo(repo, max_file_bytes)).collect()
  }

  fn plan(&self, state: &EngineState) -> Plan {
    let scans = self.scan_all(state);
    let needs_run = !state.pending.is_empty() || scans.iter().any(|scan| state.repos.get(&scan.repo).is_some_and(|repo| scan.needs_run(repo)));
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
    let env = RunEnv {
      store: &self.store,
      project: &project,
      toolchain: &self.toolchain,
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
