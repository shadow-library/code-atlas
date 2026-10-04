use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use atlas_core::{RelPath, RepoName, RunId};
use atlas_project::Ecosystem;
use atlas_scip::ScipIndexer;
use atlas_store::ReadTx;
use serde::{Deserialize, Serialize};

use crate::engine::{Engine, lock_inner};
use crate::error::{EngineError, EngineResult};
use crate::lock::ScipLock;
use crate::run::key_label;
use crate::scip::{HeldPackages, ScipIndexers, ScipPackageOutcome, ScipRunOutput, ScipTrigger, now_ms, package_jobs};

const AUTO_SCIP_BLOB: &str = "engine:scip.auto";
const AUTO_SCIP_BLOB_VERSION: u32 = 1;
const CHECK_INTERVAL: Duration = Duration::from_secs(1);
const BUSY_RETRY: Duration = Duration::from_secs(5);
const DEFAULT_RETRY: Duration = Duration::from_secs(60 * 60);

static WARNED_UNAVAILABLE: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

#[derive(Clone)]
pub struct AutoScipOptions {
  pub cargo: Option<Arc<dyn ScipIndexer>>,
  pub npm: Option<Arc<dyn ScipIndexer>>,
  pub unavailable_retry: Duration,
  pub failure_retry: Duration,
}

impl Default for AutoScipOptions {
  fn default() -> Self {
    Self {
      cargo: None,
      npm: None,
      unavailable_retry: DEFAULT_RETRY,
      failure_retry: DEFAULT_RETRY,
    }
  }
}

impl AutoScipOptions {
  pub fn with_indexers(cargo: Arc<dyn ScipIndexer>, npm: Arc<dyn ScipIndexer>) -> Self {
    Self {
      cargo: Some(cargo),
      npm: Some(npm),
      ..Self::default()
    }
  }

  fn indexer(&self, ecosystem: Ecosystem) -> Option<&Arc<dyn ScipIndexer>> {
    match ecosystem {
      Ecosystem::Cargo => self.cargo.as_ref(),
      Ecosystem::Npm => self.npm.as_ref(),
    }
  }

  pub(crate) fn is_configured(&self) -> bool {
    self.cargo.is_some() || self.npm.is_some()
  }
}

impl fmt::Debug for AutoScipOptions {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("AutoScipOptions")
      .field("cargo", &self.cargo.as_ref().map(|indexer| indexer.id()))
      .field("npm", &self.npm.as_ref().map(|indexer| indexer.id()))
      .field("unavailable_retry", &self.unavailable_retry)
      .field("failure_retry", &self.failure_retry)
      .finish()
  }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AutoScipSetting {
  #[default]
  Off,
  On {
    debounce_secs: u64,
  },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutoScipOutcome {
  Completed { packages_failed: u64 },
  Cancelled,
  Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoScipRun {
  pub started_ms: i64,
  pub duration_ms: u64,
  pub files_ingested: u64,
  pub edges: u64,
  pub outcome: AutoScipOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScipBackoff {
  pub indexer: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub package: Option<String>,
  pub reason: String,
  pub retry_at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutoScipStatus {
  pub setting: AutoScipSetting,
  pub in_progress: bool,
  pub last_run: Option<AutoScipRun>,
  pub backoff: Vec<ScipBackoff>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AutoScipRecord {
  pub last_run: Option<AutoScipRun>,
  pub backoff: Vec<ScipBackoff>,
}

impl AutoScipRecord {
  pub fn read(tx: &ReadTx<'_>) -> EngineResult<Self> {
    let Some(blob) = tx.blob(AUTO_SCIP_BLOB)?.filter(|blob| blob.version == AUTO_SCIP_BLOB_VERSION) else {
      return Ok(Self::default());
    };
    serde_json::from_slice(&blob.bytes).map_err(|error| EngineError::codec("automatic scip record", error))
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScipBacklog {
  pub generation: Option<RunId>,
  pub files: u64,
}

impl Engine {
  fn known_generation(&self) -> Option<RunId> {
    lock_inner(&self.inner).state.generation
  }

  pub(crate) fn scip_backlog(&self, ecosystems: &[Ecosystem], held: HeldPackages<'_>) -> EngineResult<ScipBacklog> {
    let mut inner = lock_inner(&self.inner);
    self.sync(&mut inner)?;
    let state = &inner.state;
    let mut files = 0u64;
    for job in package_jobs(state) {
      if !ecosystems.contains(&job.ecosystem) || held(&job.repo, &job.manifest, &job.fingerprint()) {
        continue;
      }
      files += job
        .files
        .keys()
        .filter(|path| state.scip.lagging.contains(&key_label(&(job.repo.clone(), (*path).clone()))))
        .count() as u64;
    }
    Ok(ScipBacklog {
      generation: state.generation,
      files,
    })
  }

  fn scip_auto(&self, indexers: &ScipIndexers<'_>, cancel: &AtomicBool, held: HeldPackages<'_>) -> EngineResult<Option<ScipRunOutput>> {
    let mut lock = ScipLock::open(&self.db_path())?;
    lock.try_hold(|| self.scip_locked(indexers, ScipTrigger::Automatic, cancel, held))?.transpose()
  }

  fn write_auto_record(&self, record: &AutoScipRecord) -> EngineResult<()> {
    let bytes = serde_json::to_vec(record).map_err(|error| EngineError::codec("automatic scip record", error))?;
    self.store.write(|tx| tx.put_blob(AUTO_SCIP_BLOB, AUTO_SCIP_BLOB_VERSION, &bytes))?;
    Ok(())
  }

  pub(crate) fn auto_scip_status(&self, tx: &ReadTx<'_>) -> EngineResult<AutoScipStatus> {
    let settings = self.project().settings.scip.clone();
    let mut record = AutoScipRecord::read(tx)?;
    let now = now_ms();
    record.backoff.retain(|backoff| backoff.retry_at_ms > now);
    Ok(AutoScipStatus {
      setting: match settings.enabled {
        true => AutoScipSetting::On {
          debounce_secs: settings.debounce_secs,
        },
        false => AutoScipSetting::Off,
      },
      in_progress: ScipLock::open(&self.db_path())?.is_busy()?,
      last_run: record.last_run,
      backoff: record.backoff,
    })
  }
}

#[derive(Debug, Clone)]
enum Gate {
  Ready,
  BackedOff { until: Instant, backoff: ScipBackoff },
}

#[derive(Debug, Clone)]
struct HeldPackage {
  fingerprint: String,
  until: Instant,
  backoff: ScipBackoff,
}

#[derive(Debug)]
struct Gates {
  cargo: Gate,
  npm: Gate,
  failed: BTreeMap<(RepoName, RelPath), HeldPackage>,
  attempted: Option<Option<RunId>>,
  not_before: Option<Instant>,
}

impl Gates {
  fn gate(&mut self, ecosystem: Ecosystem) -> &mut Gate {
    match ecosystem {
      Ecosystem::Cargo => &mut self.cargo,
      Ecosystem::Npm => &mut self.npm,
    }
  }

  fn backoffs(&self) -> Vec<ScipBackoff> {
    [&self.cargo, &self.npm]
      .into_iter()
      .filter_map(|gate| match gate {
        Gate::Ready => None,
        Gate::BackedOff { backoff, .. } => Some(backoff.clone()),
      })
      .chain(self.failed.values().map(|held| held.backoff.clone()))
      .collect()
  }

  fn is_held(&self, repo: &RepoName, manifest: &RelPath, fingerprint: &str) -> bool {
    self
      .failed
      .get(&(repo.clone(), manifest.clone()))
      .is_some_and(|held| held.fingerprint == fingerprint && Instant::now() < held.until)
  }
}

struct RunPlan {
  engine: Arc<Engine>,
  options: AutoScipOptions,
  ecosystems: Vec<Ecosystem>,
  generation: Option<RunId>,
  gates: Arc<Mutex<Gates>>,
  cancel: Arc<AtomicBool>,
}

pub(crate) struct AutoScip {
  engine: Weak<Engine>,
  options: AutoScipOptions,
  gates: Arc<Mutex<Gates>>,
  cancel: Arc<AtomicBool>,
  running: Option<JoinHandle<()>>,
  next_check: Instant,
}

fn lock_gates(gates: &Mutex<Gates>) -> MutexGuard<'_, Gates> {
  gates.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AutoScip {
  pub fn new(engine: Weak<Engine>, options: AutoScipOptions) -> Self {
    Self {
      engine,
      options,
      gates: Arc::new(Mutex::new(Gates {
        cargo: Gate::Ready,
        npm: Gate::Ready,
        failed: BTreeMap::new(),
        attempted: None,
        not_before: None,
      })),
      cancel: Arc::new(AtomicBool::new(false)),
      running: None,
      next_check: Instant::now(),
    }
  }

  pub fn tick(&mut self, quiet_since: Instant, refresh_pending: bool) {
    if self.running.as_ref().is_some_and(|running| !running.is_finished()) {
      return;
    }
    if let Some(finished) = self.running.take()
      && finished.join().is_err()
    {
      tracing::error!("the automatic SCIP thread panicked");
    }
    let now = Instant::now();
    if refresh_pending || now < self.next_check {
      return;
    }
    self.next_check = now + CHECK_INTERVAL;
    let Some(engine) = self.engine.upgrade() else { return };
    let settings = engine.project().settings.scip.clone();
    if !settings.enabled || quiet_since.elapsed() < Duration::from_secs(settings.debounce_secs) {
      return;
    }
    let ecosystems = self.ready_ecosystems(now);
    if ecosystems.is_empty() || lock_gates(&self.gates).attempted == Some(engine.known_generation()) {
      return;
    }
    let held = |repo: &RepoName, manifest: &RelPath, fingerprint: &str| lock_gates(&self.gates).is_held(repo, manifest, fingerprint);
    let backlog = match engine.scip_backlog(&ecosystems, &held) {
      Ok(backlog) => backlog,
      Err(error) => {
        tracing::debug!(error = %error, "could not measure the SCIP backlog");
        return;
      }
    };
    let mut gates = lock_gates(&self.gates);
    if backlog.files == 0 {
      gates.attempted = Some(backlog.generation);
      return;
    }
    if gates.attempted == Some(backlog.generation) {
      return;
    }
    drop(gates);
    tracing::info!(lagging = backlog.files, generation = ?backlog.generation, "starting an automatic SCIP run");
    let plan = RunPlan {
      engine,
      options: self.options.clone(),
      ecosystems,
      generation: backlog.generation,
      gates: Arc::clone(&self.gates),
      cancel: Arc::clone(&self.cancel),
    };
    match thread::Builder::new().name("atlas-scip".to_owned()).spawn(move || plan.execute()) {
      Ok(handle) => self.running = Some(handle),
      Err(error) => tracing::warn!(error = %error, "could not start the automatic SCIP thread"),
    }
  }

  fn ready_ecosystems(&self, now: Instant) -> Vec<Ecosystem> {
    let mut gates = lock_gates(&self.gates);
    if gates.not_before.is_some_and(|not_before| now < not_before) {
      return Vec::new();
    }
    let expired = gates.failed.len();
    gates.failed.retain(|_, held| now < held.until);
    if gates.failed.len() != expired {
      gates.attempted = None;
    }
    let mut ready = Vec::new();
    for ecosystem in [Ecosystem::Cargo, Ecosystem::Npm] {
      if self.options.indexer(ecosystem).is_none() {
        continue;
      }
      if let Gate::BackedOff { until, .. } = gates.gate(ecosystem)
        && now >= *until
      {
        *gates.gate(ecosystem) = Gate::Ready;
        gates.attempted = None;
      }
      if matches!(gates.gate(ecosystem), Gate::Ready) {
        ready.push(ecosystem);
      }
    }
    ready
  }
}

impl Drop for AutoScip {
  fn drop(&mut self) {
    self.cancel.store(true, Ordering::SeqCst);
    if let Some(running) = self.running.take()
      && running.join().is_err()
    {
      tracing::error!("the automatic SCIP thread panicked");
    }
  }
}

fn warn_unavailable_once(indexer: &'static str, reason: &str, retry: Duration) {
  let first = WARNED_UNAVAILABLE.lock().unwrap_or_else(PoisonError::into_inner).insert(indexer);
  match first {
    true => tracing::warn!(
      indexer,
      reason,
      retry_secs = retry.as_secs(),
      "SCIP indexer unavailable; automatic SCIP runs for it are paused"
    ),
    false => tracing::info!(indexer, reason, retry_secs = retry.as_secs(), "SCIP indexer still unavailable"),
  }
}

impl RunPlan {
  fn indexer(&self, ecosystem: Ecosystem) -> Option<&dyn ScipIndexer> {
    self.ecosystems.contains(&ecosystem).then(|| self.options.indexer(ecosystem).map(Arc::as_ref)).flatten()
  }

  fn execute(self) {
    let span = tracing::info_span!("auto_scip", generation = ?self.generation);
    let _entered = span.enter();
    let started_ms = now_ms();
    let started = Instant::now();
    let indexers = ScipIndexers {
      cargo: self.indexer(Ecosystem::Cargo),
      npm: self.indexer(Ecosystem::Npm),
    };
    let held = |repo: &RepoName, manifest: &RelPath, fingerprint: &str| lock_gates(&self.gates).is_held(repo, manifest, fingerprint);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.engine.scip_auto(&indexers, &self.cancel, &held)));
    let outcome = match result {
      Ok(Ok(None)) => {
        tracing::debug!("another SCIP run holds the lock; retrying later");
        lock_gates(&self.gates).not_before = Some(Instant::now() + BUSY_RETRY);
        return;
      }
      Ok(Ok(Some(output))) => self.absorb(&output),
      Ok(Err(error)) => {
        tracing::warn!(error = %error, "automatic SCIP run failed");
        (AutoScipOutcome::Failed { reason: error.to_string() }, 0, 0)
      }
      Err(payload) => {
        let reason = crate::analyze::panic_message(payload.as_ref());
        tracing::error!(reason = %reason, "automatic SCIP run panicked");
        (
          AutoScipOutcome::Failed {
            reason: format!("panicked: {reason}"),
          },
          0,
          0,
        )
      }
    };
    let (outcome, files_ingested, edges) = outcome;
    let backoff = {
      let mut gates = lock_gates(&self.gates);
      if outcome != AutoScipOutcome::Cancelled {
        gates.attempted = Some(self.generation);
      }
      gates.backoffs()
    };
    let run = AutoScipRun {
      started_ms,
      duration_ms: started.elapsed().as_millis() as u64,
      files_ingested,
      edges,
      outcome,
    };
    tracing::info!(files = run.files_ingested, edges = run.edges, duration_ms = run.duration_ms, outcome = ?run.outcome, "automatic SCIP run finished");
    if let Err(error) = self.engine.write_auto_record(&AutoScipRecord { last_run: Some(run), backoff }) {
      tracing::warn!(error = %error, "could not record the automatic SCIP run");
    }
  }

  fn absorb(&self, output: &ScipRunOutput) -> (AutoScipOutcome, u64, u64) {
    let mut gates = lock_gates(&self.gates);
    let cancelled = self.cancel.load(Ordering::SeqCst);
    let (mut failed, mut files, mut edges) = (0u64, 0u64, 0u64);
    for package in &output.report.packages {
      let ecosystem = package.ecosystem;
      let unit = (package.repo.clone(), package.manifest.clone());
      if matches!(package.outcome, ScipPackageOutcome::Ingested { .. }) {
        gates.failed.remove(&unit);
      }
      match &package.outcome {
        ScipPackageOutcome::Ingested {
          files: ingested, edges: written, ..
        } => {
          files += ingested;
          edges += written;
        }
        ScipPackageOutcome::IndexerUnavailable { reason } => {
          failed += 1;
          let Some(indexer) = self.options.indexer(ecosystem) else { continue };
          if matches!(gates.gate(ecosystem), Gate::BackedOff { .. }) {
            continue;
          }
          let retry = self.options.unavailable_retry;
          warn_unavailable_once(indexer.id(), reason, retry);
          *gates.gate(ecosystem) = Gate::BackedOff {
            until: Instant::now() + retry,
            backoff: ScipBackoff {
              indexer: indexer.id().to_owned(),
              package: None,
              reason: reason.clone(),
              retry_at_ms: now_ms().saturating_add(i64::try_from(retry.as_millis()).unwrap_or(i64::MAX)),
            },
          };
        }
        ScipPackageOutcome::IndexerFailed { reason } => {
          failed += 1;
          let (Some(indexer), Some(fingerprint), false) = (package.indexer.as_ref(), output.fingerprints.get(&unit), cancelled) else {
            continue;
          };
          let retry = self.options.failure_retry;
          tracing::warn!(repo = %package.repo, manifest = %package.manifest, reason = %reason, retry_secs = retry.as_secs(), "SCIP indexer failed; the package is skipped until its inputs change");
          gates.failed.insert(
            unit,
            HeldPackage {
              fingerprint: fingerprint.clone(),
              until: Instant::now() + retry,
              backoff: ScipBackoff {
                indexer: indexer.clone(),
                package: Some(format!("{}:{}", package.repo, package.manifest)),
                reason: reason.clone(),
                retry_at_ms: now_ms().saturating_add(i64::try_from(retry.as_millis()).unwrap_or(i64::MAX)),
              },
            },
          );
        }
        ScipPackageOutcome::NoIndexer => {}
      }
    }
    let outcome = match cancelled {
      true => AutoScipOutcome::Cancelled,
      false => AutoScipOutcome::Completed { packages_failed: failed },
    };
    (outcome, files, edges)
  }
}
