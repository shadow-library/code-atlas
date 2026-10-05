use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use atlas_core::{ContentHash, RunId};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::client::{Client, ClientHandler, ClientOptions, Initialized};
use crate::docs::OpenDoc;
use crate::error::{LspError, LspResult};
use crate::position::{LineBreaks, PositionEncoding};
use crate::protocol::{InitializeResult, PublishDiagnosticsParams, ServerInfo};
use crate::ready::{ProgressCount, ServerState, Tracker};
use crate::spec::{ServerKind, ServerSpec, ServerUnit, UnitKey, UnitSettings};
use crate::sync::{Applied, ChangeList, RestartReason, SyncDemand, SyncLimits, SyncPlan, SyncReport, SyncRequest, SyncState};
use crate::typescript::TypeScriptVersion;
use crate::uri::canonical_path;

const WARMING_LIMIT: u32 = 3;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
  mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub trait Clock: Send + Sync + 'static {
  fn now(&self) -> Instant;
}

pub struct SystemClock;

impl Clock for SystemClock {
  fn now(&self) -> Instant {
    Instant::now()
  }
}

pub struct ManualClock {
  start: Instant,
  elapsed: Mutex<Duration>,
}

impl Default for ManualClock {
  fn default() -> Self {
    Self {
      start: Instant::now(),
      elapsed: Mutex::new(Duration::ZERO),
    }
  }
}

impl ManualClock {
  pub fn advance(&self, by: Duration) {
    *lock(&self.elapsed) += by;
  }
}

impl Clock for ManualClock {
  fn now(&self) -> Instant {
    self.start + *lock(&self.elapsed)
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolMode {
  Warm { reap_every: Duration },
  OneShot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
  pub max_failures: usize,
  pub window: Duration,
  pub backoff: Duration,
}

impl Default for RestartPolicy {
  fn default() -> Self {
    Self {
      max_failures: 3,
      window: Duration::from_secs(600),
      backoff: Duration::from_secs(600),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolOptions {
  pub mode: PoolMode,
  pub max_servers: usize,
  pub idle_timeout: Duration,
  pub busy_timeout: Duration,
  pub restart: RestartPolicy,
  pub sync: SyncLimits,
  pub client: ClientOptions,
}

impl PoolOptions {
  pub fn warm() -> Self {
    Self {
      mode: PoolMode::Warm {
        reap_every: Duration::from_secs(30),
      },
      max_servers: 2,
      idle_timeout: Duration::from_secs(600),
      busy_timeout: Duration::from_secs(30),
      restart: RestartPolicy::default(),
      sync: SyncLimits::default(),
      client: ClientOptions::default(),
    }
  }

  pub fn one_shot() -> Self {
    Self {
      mode: PoolMode::OneShot,
      ..Self::warm()
    }
  }
}

pub(crate) struct Server {
  pub(crate) unit: ServerUnit,
  pub(crate) spec: ServerSpec,
  pub(crate) client: Arc<Client>,
  pub(crate) tracker: Arc<Tracker>,
  pub(crate) sync: Mutex<SyncState>,
  pub(crate) open_documents: AtomicUsize,
  initialized: Initialized,
  lost: Mutex<Option<String>>,
  answered: AtomicBool,
  leases: AtomicUsize,
  clock: Arc<dyn Clock>,
  spawned_at: Instant,
  started_at: Instant,
  ready_after: OnceLock<Duration>,
  last_used: Mutex<Instant>,
  cause: Option<RestartReason>,
  lockfiles: Vec<(PathBuf, Option<ContentHash>)>,
}

impl Server {
  fn spawn(unit: &ServerUnit, generation: Option<RunId>, cause: Option<RestartReason>, clock: Arc<dyn Clock>, client: &ClientOptions, flight: &Flight) -> LspResult<Self> {
    let spawned_at = Instant::now();
    let spec = unit.spec()?;
    let lockfiles = match &unit.settings {
      UnitSettings::RustAnalyzer(settings) => settings
        .lockfiles()
        .into_iter()
        .map(|path| {
          let hash = lockfile_hash(&path);
          (path, hash)
        })
        .collect(),
      UnitSettings::TypeScript(_) => Vec::new(),
    };
    let tracker = Arc::new(Tracker::new(&spec, unit.timeouts.announcement)?);
    let options = ClientOptions {
      request_timeout: unit.timeouts.request,
      initialize_timeout: unit.timeouts.initialize,
      ..client.clone()
    };
    let handler: Arc<dyn ClientHandler> = tracker.clone();
    let params = spec.initialize_params()?;
    let client = Arc::new(Client::spawn(&unit.command, handler, options)?);
    if !flight.register(&client) {
      client.shutdown();
      return Err(LspError::PoolShutDown);
    }
    let initialized = match client.initialize(&params) {
      Ok(initialized) => initialized,
      Err(error) => {
        client.shutdown();
        return Err(error);
      }
    };
    tracker.mark_initialized();
    let now = clock.now();
    Ok(Self {
      unit: unit.clone(),
      spec,
      client,
      tracker,
      sync: Mutex::new(SyncState {
        generation,
        ..SyncState::default()
      }),
      open_documents: AtomicUsize::new(0),
      initialized,
      lost: Mutex::new(None),
      answered: AtomicBool::new(false),
      leases: AtomicUsize::new(0),
      clock,
      spawned_at,
      started_at: now,
      ready_after: OnceLock::new(),
      last_used: Mutex::new(now),
      cause,
      lockfiles,
    })
  }

  fn wait_ready(&self, timeout: Duration) -> LspResult<()> {
    self.tracker.wait_ready(timeout)?;
    let ready_after = *self.ready_after.get_or_init(|| self.spawned_at.elapsed());
    tracing::debug!(
      server = self.spec.kind.name(),
      project = self.unit.project,
      pid = self.client.pid(),
      startup_ms = ready_after.as_millis() as u64,
      "language server ready"
    );
    Ok(())
  }

  pub(crate) fn notify(&self, method: &str, params: impl Serialize) -> LspResult<()> {
    let sent = self.client.notify(method, params);
    if let Err(error @ LspError::Timeout { .. }) = &sent {
      let reason = format!("{method} could not be queued: {error}");
      tracing::warn!(server = self.spec.kind.name(), project = self.unit.project, reason, "language server document sync lost");
      lock(&self.lost).get_or_insert(reason);
    }
    sent
  }

  fn request<R: DeserializeOwned>(&self, method: &str, params: impl Serialize, timeout: Duration) -> LspResult<R> {
    let timeout = match self.answered.load(Ordering::Acquire) {
      true => timeout,
      false => timeout.max(self.unit.timeouts.first_request),
    };
    let result = self.client.request_with_timeout(method, params, timeout);
    if let Ok(_) | Err(LspError::Response { .. } | LspError::Decode { .. }) = &result {
      self.answered.store(true, Ordering::Release);
    }
    result
  }

  fn replacement(&self, unit: &ServerUnit) -> Option<RestartReason> {
    if self.tracker.disconnected().is_some() {
      return Some(RestartReason::Crashed);
    }
    if let Some(reason) = lock(&self.lost).clone() {
      return Some(RestartReason::SyncLost(reason));
    }
    match self.unit.same_configuration(unit) {
      true => None,
      false => Some(RestartReason::ConfigurationChanged),
    }
  }

  fn in_use(&self) -> bool {
    self.leases.load(Ordering::SeqCst) > 0
  }

  fn touch(&self) {
    *lock(&self.last_used) = self.clock.now();
  }

  fn last_used(&self) -> Instant {
    *lock(&self.last_used)
  }

  fn shutdown(&self) {
    let outcome = self.client.shutdown();
    tracing::debug!(server = self.spec.kind.name(), project = self.unit.project, ?outcome, "language server shut down");
  }

  fn summary(&self, now: Instant) -> ServerSummary {
    ServerSummary {
      pid: self.client.pid(),
      state: self.tracker.state(),
      uptime: now.saturating_duration_since(self.started_at),
      idle: now.saturating_duration_since(self.last_used()),
      in_use: self.in_use(),
      server: self.initialized.result.server_info.clone(),
      typescript: self.tracker.typescript_version(),
      open_documents: self.open_documents.load(Ordering::Relaxed),
    }
  }
}

fn lockfile_hash(path: &Path) -> Option<ContentHash> {
  fs::read(path).ok().map(|bytes| ContentHash::of(&bytes))
}

pub struct Lease {
  server: Arc<Server>,
  pool: Weak<PoolInner>,
}

impl Drop for Lease {
  fn drop(&mut self) {
    self.server.touch();
    self.server.leases.fetch_sub(1, Ordering::SeqCst);
    if let Some(pool) = self.pool.upgrade() {
      let _state = lock(&pool.state);
      pool.changed.notify_all();
    }
  }
}

impl Lease {
  fn new(server: Arc<Server>, pool: &Arc<PoolInner>) -> Self {
    server.leases.fetch_add(1, Ordering::SeqCst);
    server.touch();
    Self {
      server,
      pool: Arc::downgrade(pool),
    }
  }

  pub fn key(&self) -> UnitKey {
    self.server.unit.key()
  }

  pub fn kind(&self) -> ServerKind {
    self.server.spec.kind
  }

  pub fn pid(&self) -> Option<u32> {
    self.server.client.pid()
  }

  pub fn encoding(&self) -> PositionEncoding {
    self.server.initialized.encoding
  }

  pub fn line_breaks(&self) -> LineBreaks {
    self.server.spec.line_breaks()
  }

  pub fn initialize_result(&self) -> &InitializeResult {
    &self.server.initialized.result
  }

  pub fn server_info(&self) -> Option<&ServerInfo> {
    self.server.initialized.result.server_info.as_ref()
  }

  pub fn typescript_version(&self) -> Option<TypeScriptVersion> {
    self.server.tracker.typescript_version()
  }

  pub fn state(&self) -> ServerState {
    self.server.tracker.state()
  }

  pub fn start_cause(&self) -> Option<&RestartReason> {
    self.server.cause.as_ref()
  }

  pub fn startup(&self) -> Duration {
    self.server.ready_after.get().copied().unwrap_or_default()
  }

  pub fn synced_generation(&self) -> Option<RunId> {
    lock(&self.server.sync).generation
  }

  pub fn sync_lost(&self) -> Option<String> {
    lock(&self.server.lost).clone()
  }

  pub fn open_document(&self, path: &Path) -> Option<OpenDoc> {
    let path = canonical_path(path).ok()?;
    lock(&self.server.sync).docs.get(&path).cloned()
  }

  pub fn touch_document(&self, path: &Path) -> LspResult<Option<i32>> {
    self.server.touch_document(path)
  }

  pub fn close_documents(&self, paths: &[PathBuf]) -> LspResult<Vec<PathBuf>> {
    self.server.close_documents(paths)
  }

  pub fn request<R: DeserializeOwned>(&self, method: &str, params: impl Serialize) -> LspResult<R> {
    self.server.request(method, params, self.server.unit.timeouts.request)
  }

  pub fn request_with_timeout<R: DeserializeOwned>(&self, method: &str, params: impl Serialize, timeout: Duration) -> LspResult<R> {
    self.server.request(method, params, timeout)
  }

  pub fn notify(&self, method: &str, params: impl Serialize) -> LspResult<()> {
    self.server.notify(method, params)
  }

  pub fn wait_ready(&self, timeout: Duration) -> LspResult<()> {
    self.server.tracker.wait_ready(timeout)
  }

  pub fn diagnostics(&self, path: &Path) -> Option<(u64, PublishDiagnosticsParams)> {
    self.server.tracker.diagnostics(&canonical_path(path).ok()?)
  }

  pub fn progress(&self, token_prefix: &str) -> ProgressCount {
    self.server.tracker.progress(token_prefix)
  }

  pub fn wait_progress(&self, token_prefix: &str, timeout: Duration, until: impl Fn(&ProgressCount) -> bool) -> ProgressCount {
    self.server.tracker.wait_progress(token_prefix, timeout, until)
  }

  pub fn diagnostics_sequence(&self) -> u64 {
    self.server.tracker.diagnostics_sequence()
  }

  pub fn wait_diagnostics(&self, path: &Path, after: u64, timeout: Duration) -> Option<PublishDiagnosticsParams> {
    self.server.tracker.wait_diagnostics(&canonical_path(path).ok()?, after, timeout)
  }

  pub fn lockfile_changes(&self) -> Vec<PathBuf> {
    self
      .server
      .lockfiles
      .iter()
      .filter(|(path, hash)| lockfile_hash(path) != *hash)
      .map(|(path, _)| path.clone())
      .collect()
  }
}

pub struct Synced {
  pub lease: Lease,
  pub report: SyncReport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSummary {
  pub pid: Option<u32>,
  pub state: ServerState,
  pub uptime: Duration,
  pub idle: Duration,
  pub in_use: bool,
  pub server: Option<ServerInfo>,
  pub typescript: Option<TypeScriptVersion>,
  pub open_documents: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitPhase {
  Starting,
  Warming(ServerSummary),
  Running(ServerSummary),
  Backoff { failures: usize, retry_in: Duration, last_failure: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitStatus {
  pub key: UnitKey,
  pub phase: UnitPhase,
}

/// Sums the whole process group, which holds tsserver's node children and rust-analyzer's proc-macro server.
pub fn resident_kib(pid: u32) -> Option<u64> {
  let output = Command::new("ps").args(["-A", "-o", "pgid=,rss="]).stderr(Stdio::null()).output().ok()?;
  let group = pid.to_string();
  let sizes: Vec<u64> = String::from_utf8_lossy(&output.stdout)
    .lines()
    .filter_map(|line| {
      let mut fields = line.split_whitespace();
      match (fields.next(), fields.next()) {
        (Some(pgid), Some(rss)) if pgid == group => rss.parse().ok(),
        _ => None,
      }
    })
    .collect();
  match sizes.is_empty() {
    true => None,
    false => Some(sizes.iter().sum()),
  }
}

enum FlightClient {
  Pending,
  Running(Arc<Client>),
  Cancelled,
  Done,
}

struct Flight {
  result: Mutex<Option<LspResult<Arc<Server>>>>,
  done: Condvar,
  client: Mutex<FlightClient>,
}

impl Flight {
  fn new() -> Self {
    Self {
      result: Mutex::new(None),
      done: Condvar::new(),
      client: Mutex::new(FlightClient::Pending),
    }
  }

  fn register(&self, client: &Arc<Client>) -> bool {
    let mut current = lock(&self.client);
    match *current {
      FlightClient::Cancelled => false,
      FlightClient::Pending | FlightClient::Running(_) | FlightClient::Done => {
        *current = FlightClient::Running(Arc::clone(client));
        true
      }
    }
  }

  fn cancel(&self) -> Option<Arc<Client>> {
    match std::mem::replace(&mut *lock(&self.client), FlightClient::Cancelled) {
      FlightClient::Running(client) => Some(client),
      FlightClient::Pending | FlightClient::Cancelled | FlightClient::Done => None,
    }
  }

  fn finish(&self, result: LspResult<Arc<Server>>) {
    *lock(&self.client) = FlightClient::Done;
    *lock(&self.result) = Some(result);
    self.done.notify_all();
  }

  fn wait(&self) -> LspResult<Arc<Server>> {
    let result = lock(&self.result);
    let result = self.done.wait_while(result, |result| result.is_none()).unwrap_or_else(PoisonError::into_inner);
    result.clone().unwrap_or(Err(LspError::PoolShutDown))
  }
}

#[derive(Clone)]
struct Backoff {
  until: Instant,
  failures: usize,
  last_failure: String,
}

enum Slot {
  Starting(Arc<Flight>),
  Warming(Arc<Server>),
  Live(Arc<Server>),
  Backoff(Backoff),
}

enum Current {
  Starting(Arc<Flight>),
  Warming(Arc<Server>),
  Live(Arc<Server>),
  Backoff(Backoff),
  Vacant,
}

enum Snapshot {
  Starting,
  Warming(Arc<Server>),
  Live(Arc<Server>),
  Backoff(Backoff),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PoolPhase {
  Open,
  Closed,
}

struct PoolState {
  phase: PoolPhase,
  slots: HashMap<UnitKey, Slot>,
  failures: HashMap<UnitKey, VecDeque<Instant>>,
  causes: HashMap<UnitKey, RestartReason>,
  retiring: usize,
}

enum Step {
  Use(Lease),
  WaitReady(Lease),
  Wait(Arc<Flight>),
  Retire(Arc<Server>),
  Fail { server: Arc<Server>, error: LspError },
  Start { flight: Arc<Flight>, cause: Option<RestartReason> },
}

enum Launched {
  Ready(Arc<Server>),
  Warming(Arc<Server>, LspError),
  Failed(LspError),
}

struct PoolInner {
  options: PoolOptions,
  clock: Arc<dyn Clock>,
  state: Mutex<PoolState>,
  changed: Condvar,
}

fn crash_loop(key: &UnitKey, backoff: &Backoff, window: Duration, now: Instant) -> LspError {
  LspError::CrashLoop {
    server: key.kind.name().to_owned(),
    failures: backoff.failures,
    window,
    retry_in: backoff.until.saturating_duration_since(now),
    last_failure: backoff.last_failure.clone(),
  }
}

fn counts_as_failure(error: &LspError) -> bool {
  matches!(
    error,
    LspError::ServerExited { .. } | LspError::Protocol { .. } | LspError::Timeout { .. } | LspError::NotReady { .. } | LspError::Unhealthy { .. } | LspError::Response { .. }
  )
}

impl PoolState {
  fn record_failure(&mut self, key: &UnitKey, now: Instant, policy: &RestartPolicy, failure: String) -> Option<Backoff> {
    if self.phase == PoolPhase::Closed {
      return None;
    }
    let failures = self.failures.entry(key.clone()).or_default();
    failures.push_back(now);
    while failures.front().is_some_and(|at| now.saturating_duration_since(*at) > policy.window) {
      failures.pop_front();
    }
    if failures.len() < policy.max_failures {
      return None;
    }
    let backoff = Backoff {
      until: now + policy.backoff,
      failures: failures.len(),
      last_failure: failure,
    };
    self.failures.remove(key);
    self.slots.insert(key.clone(), Slot::Backoff(backoff.clone()));
    Some(backoff)
  }

  fn retire(&mut self, key: &UnitKey, server: Arc<Server>, reason: RestartReason, now: Instant, policy: &RestartPolicy) -> Step {
    self.slots.remove(key);
    self.retiring += 1;
    self.causes.insert(key.clone(), reason.clone());
    if !reason.is_failure() {
      return Step::Retire(server);
    }
    let failure = server
      .tracker
      .disconnected()
      .map_or_else(|| format!("{reason} ({})", server.tracker.state()), |error| error.to_string());
    match self.record_failure(key, now, policy, failure) {
      Some(backoff) => {
        let error = crash_loop(key, &backoff, policy.window, now);
        Step::Fail { server, error }
      }
      None => Step::Retire(server),
    }
  }

  fn occupied(&self, key: &UnitKey) -> usize {
    let servers = self
      .slots
      .iter()
      .filter(|(other, slot)| *other != key && matches!(slot, Slot::Starting(_) | Slot::Warming(_) | Slot::Live(_)))
      .count();
    servers + self.retiring
  }

  fn evict_one(&mut self, key: &UnitKey) -> Option<Arc<Server>> {
    let victim = self
      .slots
      .iter()
      .filter_map(|(other, slot)| match slot {
        Slot::Live(server) if other != key && !server.in_use() => Some((other.clone(), (false, server.last_used()))),
        Slot::Warming(server) if other != key && !server.in_use() => Some((other.clone(), (true, server.last_used()))),
        _ => None,
      })
      .min_by_key(|(_, order)| *order)
      .map(|(victim, _)| victim)?;
    let (Some(Slot::Live(server)) | Some(Slot::Warming(server))) = self.slots.remove(&victim) else {
      return None;
    };
    tracing::debug!(server = victim.kind.name(), project = victim.project, "evicting the least recently used language server");
    self.retiring += 1;
    Some(server)
  }

  fn remove_flight(&mut self, key: &UnitKey, flight: &Arc<Flight>) {
    if let Some(Slot::Starting(current)) = self.slots.get(key)
      && Arc::ptr_eq(current, flight)
    {
      self.slots.remove(key);
    }
  }
}

struct StartGuard<'a> {
  inner: &'a PoolInner,
  key: &'a UnitKey,
  flight: &'a Arc<Flight>,
  finished: bool,
}

impl Drop for StartGuard<'_> {
  fn drop(&mut self) {
    if self.finished {
      return;
    }
    {
      let mut state = lock(&self.inner.state);
      state.remove_flight(self.key, self.flight);
      self.inner.changed.notify_all();
    }
    self.flight.finish(Err(LspError::Spawn {
      program: self.key.kind.name().to_owned(),
      reason: "starting the server panicked".to_owned(),
    }));
  }
}

impl PoolInner {
  fn plan(self: &Arc<Self>, key: &UnitKey, unit: &ServerUnit) -> LspResult<Step> {
    let policy = &self.options.restart;
    let warming_limit = unit.timeouts.ready.saturating_mul(WARMING_LIMIT);
    let mut deadline = None;
    let mut state = lock(&self.state);
    loop {
      if state.phase == PoolPhase::Closed {
        return Err(LspError::PoolShutDown);
      }
      let now = self.clock.now();
      let current = match state.slots.get(key) {
        Some(Slot::Starting(flight)) => Current::Starting(Arc::clone(flight)),
        Some(Slot::Warming(server)) => Current::Warming(Arc::clone(server)),
        Some(Slot::Live(server)) => Current::Live(Arc::clone(server)),
        Some(Slot::Backoff(backoff)) => Current::Backoff(backoff.clone()),
        None => Current::Vacant,
      };
      match current {
        Current::Starting(flight) => return Ok(Step::Wait(flight)),
        Current::Live(server) => match server.replacement(unit) {
          None => return Ok(Step::Use(Lease::new(server, self))),
          Some(reason) => return Ok(state.retire(key, server, reason, now, policy)),
        },
        Current::Warming(server) if now.saturating_duration_since(server.started_at) > warming_limit => {
          let reason = RestartReason::NeverReady {
            waited: now.saturating_duration_since(server.started_at),
          };
          return Ok(state.retire(key, server, reason, now, policy));
        }
        Current::Warming(server) => match server.replacement(unit) {
          None => return Ok(Step::WaitReady(Lease::new(server, self))),
          Some(reason) => return Ok(state.retire(key, server, reason, now, policy)),
        },
        Current::Backoff(backoff) if backoff.until > now => return Err(crash_loop(key, &backoff, policy.window, now)),
        Current::Backoff(_) | Current::Vacant => {}
      }
      state.slots.remove(key);
      if state.occupied(key) < self.options.max_servers.max(1) {
        let flight = Arc::new(Flight::new());
        state.slots.insert(key.clone(), Slot::Starting(Arc::clone(&flight)));
        let cause = state.causes.remove(key);
        return Ok(Step::Start { flight, cause });
      }
      if let Some(victim) = state.evict_one(key) {
        return Ok(Step::Retire(victim));
      }
      let waiting = Instant::now();
      let deadline = *deadline.get_or_insert(waiting + self.options.busy_timeout);
      if waiting >= deadline {
        return Err(LspError::PoolBusy {
          max_servers: self.options.max_servers,
          waited: self.options.busy_timeout,
        });
      }
      state = self.changed.wait_timeout(state, deadline - waiting).unwrap_or_else(PoisonError::into_inner).0;
    }
  }

  fn launch(&self, unit: &ServerUnit, generation: Option<RunId>, cause: Option<RestartReason>, flight: &Flight) -> Launched {
    let server = match Server::spawn(unit, generation, cause, Arc::clone(&self.clock), &self.options.client, flight) {
      Ok(server) => Arc::new(server),
      Err(error) => return Launched::Failed(error),
    };
    match server.wait_ready(unit.timeouts.ready) {
      Ok(()) => Launched::Ready(server),
      Err(error @ LspError::NotReady { .. }) if matches!(self.options.mode, PoolMode::Warm { .. }) => Launched::Warming(server, error),
      Err(error) => {
        server.shutdown();
        Launched::Failed(error)
      }
    }
  }

  fn start(self: &Arc<Self>, key: &UnitKey, unit: &ServerUnit, generation: Option<RunId>, cause: Option<RestartReason>, flight: &Arc<Flight>) -> LspResult<Lease> {
    let mut guard = StartGuard {
      inner: self,
      key,
      flight,
      finished: false,
    };
    let launched = self.launch(unit, generation, cause, flight);
    let mut state = lock(&self.state);
    let (outcome, orphan) = match (state.phase, launched) {
      (PoolPhase::Closed, Launched::Ready(server) | Launched::Warming(server, _)) => (Err(LspError::PoolShutDown), Some(server)),
      (PoolPhase::Closed, Launched::Failed(_)) => (Err(LspError::PoolShutDown), None),
      (PoolPhase::Open, Launched::Ready(server)) => {
        state.slots.insert(key.clone(), Slot::Live(Arc::clone(&server)));
        (Ok(server), None)
      }
      (PoolPhase::Open, Launched::Warming(server, error)) => {
        tracing::warn!(server = key.kind.name(), project = key.project, %error, "language server still loading; later requests keep waiting for it");
        state.slots.insert(key.clone(), Slot::Warming(server));
        (Err(error), None)
      }
      (PoolPhase::Open, Launched::Failed(error)) => {
        state.remove_flight(key, flight);
        if counts_as_failure(&error) {
          state.record_failure(key, self.clock.now(), &self.options.restart, error.to_string());
        }
        (Err(error), None)
      }
    };
    let lease = outcome.as_ref().ok().map(|server| Lease::new(Arc::clone(server), self));
    self.changed.notify_all();
    drop(state);
    if let Some(orphan) = orphan {
      orphan.shutdown();
    }
    flight.finish(outcome.clone());
    guard.finished = true;
    match (outcome, lease) {
      (Ok(_), Some(lease)) => Ok(lease),
      (Err(error), _) => Err(error),
      (Ok(_), None) => Err(LspError::PoolShutDown),
    }
  }

  fn promote(&self, key: &UnitKey, server: &Arc<Server>) {
    let mut state = lock(&self.state);
    if let Some(Slot::Warming(current)) = state.slots.get(key)
      && Arc::ptr_eq(current, server)
    {
      state.slots.insert(key.clone(), Slot::Live(Arc::clone(server)));
    }
  }

  fn retired(&self, count: usize) {
    let mut state = lock(&self.state);
    state.retiring = state.retiring.saturating_sub(count);
    self.changed.notify_all();
  }

  fn take_idle(&self) -> Vec<Arc<Server>> {
    let mut state = lock(&self.state);
    let now = self.clock.now();
    let idle: Vec<UnitKey> = state
      .slots
      .iter()
      .filter_map(|(key, slot)| match slot {
        Slot::Live(server) | Slot::Warming(server) if !server.in_use() && now.saturating_duration_since(server.last_used()) >= self.options.idle_timeout => Some(key.clone()),
        _ => None,
      })
      .collect();
    let servers: Vec<Arc<Server>> = idle
      .iter()
      .filter_map(|key| match state.slots.remove(key) {
        Some(Slot::Live(server) | Slot::Warming(server)) => Some(server),
        _ => None,
      })
      .collect();
    state.retiring += servers.len();
    servers
  }

  fn reap(&self) -> usize {
    let idle = self.take_idle();
    let count = idle.len();
    in_parallel(&idle, |server| server.shutdown());
    self.retired(count);
    count
  }

  fn close(&self) -> (Vec<Arc<Server>>, Vec<Arc<Client>>) {
    let mut state = lock(&self.state);
    state.phase = PoolPhase::Closed;
    let mut servers = Vec::new();
    let mut starting = Vec::new();
    for (_, slot) in state.slots.drain() {
      match slot {
        Slot::Live(server) | Slot::Warming(server) => servers.push(server),
        Slot::Starting(flight) => starting.extend(flight.cancel()),
        Slot::Backoff(_) => {}
      }
    }
    self.changed.notify_all();
    (servers, starting)
  }
}

fn in_parallel<T: Sync>(items: &[T], work: impl Fn(&T) + Sync) {
  thread::scope(|scope| {
    for item in items {
      let work = &work;
      scope.spawn(move || work(item));
    }
  });
}

struct Reaper {
  stop: Option<Sender<()>>,
  thread: Option<JoinHandle<()>>,
}

impl Reaper {
  fn spawn(inner: Weak<PoolInner>, every: Duration) -> Option<Self> {
    let (stop, stopped) = mpsc::channel::<()>();
    let thread = thread::Builder::new().name("lsp-reaper".to_owned()).spawn(move || {
      while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(every) {
        let Some(inner) = inner.upgrade() else {
          return;
        };
        inner.reap();
      }
    });
    match thread {
      Ok(thread) => Some(Self {
        stop: Some(stop),
        thread: Some(thread),
      }),
      Err(error) => {
        tracing::warn!(%error, "cannot start the language server reaper; idle servers stay up until the pool is dropped");
        None
      }
    }
  }
}

impl Drop for Reaper {
  fn drop(&mut self) {
    self.stop.take();
    if let Some(thread) = self.thread.take()
      && thread.join().is_err()
    {
      tracing::warn!("the language server reaper panicked");
    }
  }
}

pub struct LspPool {
  inner: Arc<PoolInner>,
  reaper: Option<Reaper>,
}

impl LspPool {
  pub fn new(options: PoolOptions) -> Self {
    Self::with_clock(options, Arc::new(SystemClock))
  }

  pub fn with_clock(options: PoolOptions, clock: Arc<dyn Clock>) -> Self {
    let mode = options.mode;
    let inner = Arc::new(PoolInner {
      options,
      clock,
      state: Mutex::new(PoolState {
        phase: PoolPhase::Open,
        slots: HashMap::new(),
        failures: HashMap::new(),
        causes: HashMap::new(),
        retiring: 0,
      }),
      changed: Condvar::new(),
    });
    let reaper = match mode {
      PoolMode::Warm { reap_every } => Reaper::spawn(Arc::downgrade(&inner), reap_every),
      PoolMode::OneShot => None,
    };
    Self { inner, reaper }
  }

  pub fn options(&self) -> &PoolOptions {
    &self.inner.options
  }

  pub fn get(&self, unit: &ServerUnit, generation: Option<RunId>) -> LspResult<Lease> {
    let key = unit.key();
    loop {
      match self.inner.plan(&key, unit)? {
        Step::Use(lease) => return Ok(lease),
        Step::WaitReady(lease) => {
          lease.server.wait_ready(unit.timeouts.ready)?;
          self.inner.promote(&key, &lease.server);
          return Ok(lease);
        }
        Step::Wait(flight) => {
          flight.wait()?;
        }
        Step::Retire(server) => {
          server.shutdown();
          self.inner.retired(1);
        }
        Step::Fail { server, error } => {
          server.shutdown();
          self.inner.retired(1);
          return Err(error);
        }
        Step::Start { flight, cause } => return self.inner.start(&key, unit, generation, cause, &flight),
      }
    }
  }

  pub fn sync(
    &self,
    unit: &ServerUnit,
    generation: Option<RunId>,
    demand: SyncDemand,
    mut changes_since: impl FnMut(Option<RunId>) -> ChangeList,
    open: &[PathBuf],
  ) -> LspResult<Synced> {
    let limits = self.inner.options.sync;
    let mut lease = self.get(unit, generation)?;
    let mut restarted: Option<RestartReason> = None;
    loop {
      let since = lease.synced_generation();
      let changes = match since >= generation {
        true => ChangeList::Known(Vec::new()),
        false => changes_since(since),
      };
      let applied = match lease.server.plan(&changes, &limits)? {
        SyncPlan::Restart(reason) => Applied::Restart(reason),
        SyncPlan::Apply(changes) => lease.server.apply(&SyncRequest {
          since,
          generation,
          changes: &changes,
          open,
          demand,
          limits: &limits,
        })?,
      };
      let reason = match applied {
        Applied::Synced(mut report) => {
          report.restarted = restarted;
          return Ok(Synced { lease, report });
        }
        Applied::Stale => continue,
        Applied::Restart(reason) => reason,
      };
      if let Some(first) = &restarted {
        return Err(LspError::SyncIncomplete {
          server: unit.kind().name().to_owned(),
          reason: format!("still out of sync after a restart ({first}): {reason}"),
        });
      }
      tracing::debug!(server = unit.kind().name(), project = unit.project, %reason, "restarting the language server to resynchronise");
      lease = self.restart(unit, lease, generation, reason.clone())?;
      restarted = Some(reason);
    }
  }

  pub fn restart(&self, unit: &ServerUnit, stale: Lease, generation: Option<RunId>, reason: RestartReason) -> LspResult<Lease> {
    self.retire(unit, stale, reason);
    self.get(unit, generation)
  }

  pub fn retire(&self, unit: &ServerUnit, stale: Lease, reason: RestartReason) {
    let server = Arc::clone(&stale.server);
    drop(stale);
    let key = unit.key();
    let removed = {
      let mut state = lock(&self.inner.state);
      match state.slots.get(&key) {
        Some(Slot::Live(current)) if Arc::ptr_eq(current, &server) => {
          state.slots.remove(&key);
          state.retiring += 1;
          state.causes.insert(key.clone(), reason);
          true
        }
        _ => false,
      }
    };
    if removed {
      server.shutdown();
      self.inner.retired(1);
    }
  }

  pub fn reap_idle(&self) -> usize {
    self.inner.reap()
  }

  pub fn status(&self) -> Vec<UnitStatus> {
    let (now, snapshot) = {
      let state = lock(&self.inner.state);
      let now = self.inner.clock.now();
      let snapshot: Vec<(UnitKey, Snapshot)> = state
        .slots
        .iter()
        .map(|(key, slot)| {
          let snapshot = match slot {
            Slot::Starting(_) => Snapshot::Starting,
            Slot::Warming(server) => Snapshot::Warming(Arc::clone(server)),
            Slot::Live(server) => Snapshot::Live(Arc::clone(server)),
            Slot::Backoff(backoff) => Snapshot::Backoff(backoff.clone()),
          };
          (key.clone(), snapshot)
        })
        .collect();
      (now, snapshot)
    };
    let mut status: Vec<UnitStatus> = snapshot
      .into_iter()
      .map(|(key, snapshot)| UnitStatus {
        key,
        phase: match snapshot {
          Snapshot::Starting => UnitPhase::Starting,
          Snapshot::Warming(server) => UnitPhase::Warming(server.summary(now)),
          Snapshot::Live(server) => UnitPhase::Running(server.summary(now)),
          Snapshot::Backoff(backoff) => UnitPhase::Backoff {
            failures: backoff.failures,
            retry_in: backoff.until.saturating_duration_since(now),
            last_failure: backoff.last_failure,
          },
        },
      })
      .collect();
    status.sort_by(|left, right| left.key.cmp(&right.key));
    status
  }

  pub fn shutdown(&self) {
    let (servers, starting) = self.inner.close();
    in_parallel(&servers, |server| server.shutdown());
    in_parallel(&starting, |client| {
      client.shutdown();
    });
  }
}

impl Drop for LspPool {
  fn drop(&mut self) {
    self.reaper.take();
    self.shutdown();
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::process::ServerCommand;
  use crate::spec::{Launch, UnitFolder, UnitSettings};
  use crate::typescript::TypeScriptSettings;

  #[test]
  fn should_measure_the_busy_timeout_from_when_the_wait_for_a_slot_starts() {
    let work = tempfile::tempdir().unwrap();
    let unit = ServerUnit::new(
      "demo",
      vec![UnitFolder::new("w", work.path())],
      UnitSettings::TypeScript(TypeScriptSettings::default()),
      ServerCommand::new("server"),
      Launch::Installed,
    );
    let pool = LspPool::new(PoolOptions {
      max_servers: 1,
      busy_timeout: Duration::from_millis(300),
      ..PoolOptions::one_shot()
    });
    let key = unit.key();
    lock(&pool.inner.state).retiring = 1;
    thread::sleep(Duration::from_millis(400));
    let inner = Arc::clone(&pool.inner);
    let release = thread::spawn(move || {
      thread::sleep(Duration::from_millis(150));
      inner.retired(1);
    });

    let freed = pool.inner.plan(&key, &unit);
    release.join().unwrap();
    assert!(matches!(freed, Ok(Step::Start { .. })));
    lock(&pool.inner.state).slots.clear();
    lock(&pool.inner.state).retiring = 1;
    let started = Instant::now();
    let busy = pool.inner.plan(&key, &unit);

    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(matches!(busy, Err(LspError::PoolBusy { max_servers: 1, .. })));
    lock(&pool.inner.state).retiring = 0;
  }
}
