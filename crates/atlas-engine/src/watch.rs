use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use atlas_project::Project;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::auto_scip::AutoScip;
use crate::engine::Engine;
use crate::error::{EngineError, EngineResult};

const DEFAULT_IGNORED_SEGMENTS: [&str; 3] = [".git", "target", "node_modules"];
const WORKTREE_DIRS: [&[&str]; 2] = [&[".worktrees"], &[".claude", "worktrees"]];
const STOP_POLL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchState {
  Off,
  Healthy { dirty: bool },
  Unhealthy { reason: String },
  Dead { reason: String },
}

#[derive(Debug)]
struct Signals {
  state: WatchState,
  events: u64,
  last_scan: Option<Instant>,
}

impl Default for Signals {
  fn default() -> Self {
    Self {
      state: WatchState::Off,
      events: 0,
      last_scan: None,
    }
  }
}

#[derive(Debug, Default)]
pub(crate) struct WatchSignals {
  signals: Mutex<Signals>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ScanToken {
  events: u64,
}

impl WatchSignals {
  fn lock(&self) -> MutexGuard<'_, Signals> {
    self.signals.lock().unwrap_or_else(PoisonError::into_inner)
  }

  pub fn state(&self) -> WatchState {
    self.lock().state.clone()
  }

  pub fn events(&self) -> u64 {
    self.lock().events
  }

  pub fn is_clean(&self, full_scan_interval: Duration) -> bool {
    let signals = self.lock();
    matches!(signals.state, WatchState::Healthy { dirty: false }) && signals.last_scan.is_some_and(|scanned| scanned.elapsed() < full_scan_interval)
  }

  pub fn begin_scan(&self) -> ScanToken {
    ScanToken { events: self.lock().events }
  }

  pub fn finish_scan(&self, token: ScanToken) {
    let mut signals = self.lock();
    if matches!(signals.state, WatchState::Off | WatchState::Dead { .. }) {
      return;
    }
    signals.last_scan = Some(Instant::now());
    if signals.events == token.events {
      signals.state = WatchState::Healthy { dirty: false };
    }
  }

  pub fn abort_scan(&self, _token: ScanToken) {
    let mut signals = self.lock();
    if let WatchState::Healthy { dirty } = &mut signals.state {
      *dirty = true;
    }
  }

  fn activate(&self) {
    let mut signals = self.lock();
    signals.state = WatchState::Healthy { dirty: true };
    signals.events += 1;
  }

  fn deactivate(&self) {
    let mut signals = self.lock();
    signals.state = WatchState::Off;
    signals.events += 1;
  }

  pub(crate) fn mark_event(&self) {
    let mut signals = self.lock();
    signals.events += 1;
    if let WatchState::Healthy { dirty } = &mut signals.state {
      *dirty = true;
    }
  }

  fn mark_unhealthy(&self, reason: String) {
    let mut signals = self.lock();
    signals.events += 1;
    if matches!(signals.state, WatchState::Healthy { .. } | WatchState::Unhealthy { .. }) {
      tracing::warn!(reason = %reason, "file watcher may have missed events; the next barrier does a full scan");
      signals.state = WatchState::Unhealthy { reason };
    }
  }

  fn mark_dead(&self, reason: String) {
    let mut signals = self.lock();
    signals.events += 1;
    if signals.state != WatchState::Off {
      tracing::error!(reason = %reason, "file watcher stopped; every barrier now does a full scan");
      signals.state = WatchState::Dead { reason };
    }
  }
}

pub struct WatchHandle {
  signals: Arc<WatchSignals>,
  stop: Arc<AtomicBool>,
  worker: Option<JoinHandle<()>>,
  watcher: Option<RecommendedWatcher>,
}

impl WatchHandle {
  pub fn state(&self) -> WatchState {
    self.signals.state()
  }

  #[doc(hidden)]
  pub fn events_seen(&self) -> u64 {
    self.signals.events()
  }

  #[doc(hidden)]
  pub fn inject_error(&self, reason: &str) {
    self.signals.mark_unhealthy(reason.to_owned());
  }
}

impl Drop for WatchHandle {
  fn drop(&mut self) {
    self.stop.store(true, Ordering::SeqCst);
    self.watcher.take();
    if let Some(worker) = self.worker.take()
      && worker.join().is_err()
    {
      tracing::error!("file watcher worker panicked");
    }
    self.signals.deactivate();
  }
}

struct WorkerGuard {
  signals: Arc<WatchSignals>,
  stop: Arc<AtomicBool>,
}

impl Drop for WorkerGuard {
  fn drop(&mut self) {
    if !self.stop.load(Ordering::SeqCst) {
      self.signals.mark_dead("the watcher worker exited".to_owned());
    }
  }
}

fn ignored_segments(project: &Project) -> Vec<&'static str> {
  DEFAULT_IGNORED_SEGMENTS
    .into_iter()
    .filter(|segment| {
      !project
        .repos
        .iter()
        .flat_map(|repo| &repo.include)
        .filter(|pattern| !pattern.starts_with('!'))
        .any(|pattern| pattern.split('/').any(|part| part == *segment))
    })
    .collect()
}

struct EventFilter {
  ignored: Vec<&'static str>,
  roots: Vec<PathBuf>,
}

impl EventFilter {
  fn new(project: &Project) -> Self {
    Self {
      ignored: ignored_segments(project),
      roots: project.repos.iter().map(|repo| repo.path.clone()).collect(),
    }
  }

  fn is_relevant(&self, event: &Event) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
      return false;
    }
    event.paths.is_empty() || event.paths.iter().any(|path| !self.is_ignored(path))
  }

  fn is_ignored(&self, path: &Path) -> bool {
    is_ignored(path, &self.ignored) || is_in_worktree_dir(path, &self.roots)
  }
}

fn is_ignored(path: &Path, ignored: &[&str]) -> bool {
  path
    .components()
    .any(|component| matches!(component, Component::Normal(name) if ignored.iter().any(|segment| name == *segment)))
}

fn is_in_worktree_dir(path: &Path, roots: &[PathBuf]) -> bool {
  let Some(relative) = roots
    .iter()
    .filter_map(|root| path.strip_prefix(root).ok())
    .min_by_key(|relative| relative.components().count())
  else {
    return false;
  };
  let names: Vec<&std::ffi::OsStr> = relative
    .components()
    .filter_map(|component| match component {
      Component::Normal(name) => Some(name),
      _ => None,
    })
    .collect();
  WORKTREE_DIRS
    .iter()
    .any(|dir| names.windows(dir.len()).any(|window| window.iter().zip(dir.iter()).all(|(name, segment)| name == segment)))
}

fn on_event(signals: &WatchSignals, sender: &Sender<()>, filter: &EventFilter, event: notify::Result<Event>) {
  match event {
    Ok(event) if event.need_rescan() => signals.mark_unhealthy("the watcher requested a rescan".to_owned()),
    Ok(event) if filter.is_relevant(&event) => signals.mark_event(),
    Ok(_) => return,
    Err(error) => signals.mark_unhealthy(error.to_string()),
  }
  if sender.send(()).is_err() {
    signals.mark_dead("the watcher worker no longer receives events".to_owned());
  }
}

struct Worker {
  engine: Weak<Engine>,
  debounce: Duration,
  persist_idle: Duration,
  auto_scip: Option<AutoScip>,
}

impl Worker {
  fn run(&mut self, receiver: mpsc::Receiver<()>, stop: &AtomicBool) {
    let mut pending: Option<Instant> = None;
    let mut persist_due: Option<Instant> = None;
    let mut quiet_since = Instant::now();
    while !stop.load(Ordering::SeqCst) {
      match receiver.recv_timeout(self.debounce.min(STOP_POLL)) {
        Ok(()) => {
          pending = Some(Instant::now());
          quiet_since = Instant::now();
        }
        Err(RecvTimeoutError::Timeout) => {}
        Err(RecvTimeoutError::Disconnected) => return,
      }
      if pending.is_some_and(|last| last.elapsed() >= self.debounce) {
        pending = None;
        let Some(engine) = self.engine.upgrade() else { return };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| engine.ensure_fresh())) {
          Ok(Ok(report)) => tracing::debug!(fresh = report.is_fresh(), files = report.header().files_updated, "watcher refreshed the index"),
          Ok(Err(error)) => tracing::warn!(error = %error, "watcher refresh failed"),
          Err(_) => tracing::error!("watcher refresh panicked; the next barrier scans"),
        }
        persist_due = Some(Instant::now() + self.persist_idle);
      }
      if pending.is_none() && persist_due.is_some_and(|due| Instant::now() >= due) {
        persist_due = None;
        let Some(engine) = self.engine.upgrade() else { return };
        if let Err(error) = engine.persist_similarity() {
          tracing::warn!(error = %error, "could not persist the similarity index");
        }
      }
      if let Some(auto_scip) = &mut self.auto_scip {
        auto_scip.tick(quiet_since, pending.is_some());
      }
    }
  }
}

impl Engine {
  pub fn watch(self: &Arc<Self>) -> EngineResult<WatchHandle> {
    let signals = Arc::clone(&self.watch);
    let project = self.project();
    let filter = EventFilter::new(&project);
    let (sender, receiver) = mpsc::channel();
    let mut watcher = {
      let signals = Arc::clone(&signals);
      notify::recommended_watcher(move |event| on_event(&signals, &sender, &filter, event)).map_err(|error| EngineError::Watch(error.to_string()))?
    };
    let roots: Vec<PathBuf> = project.repos.iter().map(|repo| repo.path.clone()).collect();
    for root in &roots {
      watcher
        .watch(root, RecursiveMode::Recursive)
        .map_err(|error| EngineError::Watch(format!("{}: {error}", root.display())))?;
    }
    signals.activate();
    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
      let guard = WorkerGuard {
        signals: Arc::clone(&signals),
        stop: Arc::clone(&stop),
      };
      let mut worker = Worker {
        engine: Arc::downgrade(self),
        debounce: self.options.watch_debounce,
        persist_idle: self.options.similarity_persist_idle,
        auto_scip: self
          .options
          .auto_scip
          .is_configured()
          .then(|| AutoScip::new(Arc::downgrade(self), self.options.auto_scip.clone())),
      };
      thread::Builder::new()
        .name("atlas-watch".to_owned())
        .spawn(move || {
          let guard = guard;
          worker.run(receiver, &guard.stop);
        })
        .map_err(|error| EngineError::Watch(error.to_string()))?
    };
    Ok(WatchHandle {
      signals,
      stop,
      worker: Some(worker),
      watcher: Some(watcher),
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_keep_default_excluded_segments_that_an_include_pattern_overrides() {
    let repo = atlas_project::RepoConfig {
      name: atlas_core::RepoName::new("demo").unwrap(),
      path: PathBuf::from("/tmp/demo"),
      include: vec!["vendor-js/node_modules/**".to_owned(), "!target/**".to_owned()],
      exclude: Vec::new(),
      languages: None,
    };
    let project = Project {
      name: atlas_core::ProjectName::new("demo").unwrap(),
      settings: atlas_project::ProjectSettings::default(),
      repos: vec![repo],
    };
    let ignored = ignored_segments(&project);
    assert_eq!(ignored, vec![".git", "target"]);
    assert!(is_ignored(Path::new("/tmp/demo/target/debug/x"), &ignored));
    assert!(!is_ignored(Path::new("/tmp/demo/vendor-js/node_modules/a.ts"), &ignored));
  }

  fn demo_project(paths: &[&str]) -> Project {
    let repos = paths
      .iter()
      .enumerate()
      .map(|(index, path)| atlas_project::RepoConfig {
        name: atlas_core::RepoName::new(format!("repo-{index}")).unwrap(),
        path: PathBuf::from(path),
        include: Vec::new(),
        exclude: Vec::new(),
        languages: None,
      })
      .collect();
    Project {
      name: atlas_core::ProjectName::new("demo").unwrap(),
      settings: atlas_project::ProjectSettings::default(),
      repos,
    }
  }

  fn modified(path: &str) -> Event {
    Event::new(EventKind::Modify(notify::event::ModifyKind::Any)).add_path(PathBuf::from(path))
  }

  #[test]
  fn should_ignore_events_under_worktree_directories_of_a_repo() {
    let filter = EventFilter::new(&demo_project(&["/src/demo"]));

    let relevance: Vec<bool> = [
      "/src/demo/.worktrees/feature/src/lib.rs",
      "/src/demo/.worktrees",
      "/src/demo/.claude/worktrees/agent-1/src/main.ts",
      "/src/demo/packages/api/.worktrees/x/a.ts",
      "/src/demo/.claude/settings.json",
      "/src/demo/worktrees/notes.md",
      "/src/demo/src/lib.rs",
    ]
    .iter()
    .map(|path| filter.is_relevant(&modified(path)))
    .collect();

    assert_eq!(relevance, [false, false, false, false, true, true, true]);
  }

  #[test]
  fn should_keep_watching_a_repo_registered_inside_a_worktree_directory() {
    let filter = EventFilter::new(&demo_project(&["/src/demo/.worktrees/feature"]));

    assert!(filter.is_relevant(&modified("/src/demo/.worktrees/feature/src/lib.rs")));
    assert!(!filter.is_relevant(&modified("/src/demo/.worktrees/feature/.claude/worktrees/x/src/lib.rs")));
  }

  #[test]
  fn should_judge_a_path_by_the_innermost_repo_containing_it() {
    let filter = EventFilter::new(&demo_project(&["/src/demo", "/src/demo/.worktrees/feature"]));

    assert!(filter.is_relevant(&modified("/src/demo/.worktrees/feature/src/lib.rs")));
    assert!(!filter.is_relevant(&modified("/src/demo/.worktrees/other/src/lib.rs")));
  }

  #[test]
  fn should_never_report_clean_after_the_worker_died() {
    let signals = WatchSignals::default();
    signals.activate();
    let token = signals.begin_scan();
    signals.finish_scan(token);
    assert!(signals.is_clean(Duration::from_secs(60)));
    signals.mark_dead("gone".to_owned());
    let token = signals.begin_scan();
    signals.finish_scan(token);
    assert!(!signals.is_clean(Duration::from_secs(60)));
    assert_eq!(signals.state(), WatchState::Dead { reason: "gone".to_owned() });
  }

  #[test]
  fn should_not_report_clean_when_an_event_arrives_during_a_scan() {
    let signals = WatchSignals::default();
    signals.activate();
    let token = signals.begin_scan();
    signals.finish_scan(token);
    signals.mark_event();
    let token = signals.begin_scan();
    signals.mark_event();
    signals.finish_scan(token);
    assert!(!signals.is_clean(Duration::from_secs(60)));
    assert_eq!(signals.state(), WatchState::Healthy { dirty: true });
    let token = signals.begin_scan();
    signals.finish_scan(token);
    assert!(signals.is_clean(Duration::from_secs(60)));
  }
}
