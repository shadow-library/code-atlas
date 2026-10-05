use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::client::{ClientHandler, ServerEvent};
use crate::error::{LspError, LspResult};
use crate::pool::lock;
use crate::protocol::{ConfigurationItem, Health, ProgressParams, ProgressToken, PublishDiagnosticsParams, ServerStatusParams, WorkDoneProgress, WorkspaceFolder};
use crate::spec::{Readiness, ServerSpec};
use crate::typescript::TypeScriptVersion;
use crate::uri::path_from_uri;

const MIN_WAIT: Duration = Duration::from_millis(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerState {
  Starting,
  Indexing { message: Option<String> },
  Ready,
  Failed { reason: String },
  Exited { reason: String },
}

impl fmt::Display for ServerState {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Starting => formatter.write_str("starting"),
      Self::Indexing { message: None } => formatter.write_str("indexing"),
      Self::Indexing { message: Some(message) } => write!(formatter, "indexing: {message}"),
      Self::Ready => formatter.write_str("ready"),
      Self::Failed { reason } => write!(formatter, "failed: {reason}"),
      Self::Exited { reason } => write!(formatter, "exited: {reason}"),
    }
  }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProgressCount {
  pub begun: u64,
  pub ended: u64,
  pub active: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settle {
  NotNeeded,
  Quiet,
  Ready,
  Busy { status: String },
  Unconfirmed { paths: Vec<PathBuf> },
}

#[derive(Debug, Clone)]
struct ActiveProgress {
  token: ProgressToken,
  title: String,
  message: Option<String>,
  percentage: Option<u32>,
}

impl fmt::Display for ActiveProgress {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str(&self.title)?;
    if let Some(message) = &self.message {
      write!(formatter, " {message}")?;
    }
    if let Some(percentage) = self.percentage {
      write!(formatter, " {percentage}%")?;
    }
    Ok(())
  }
}

#[derive(Debug, Clone)]
struct Published {
  sequence: u64,
  params: PublishDiagnosticsParams,
}

#[derive(Default)]
struct TrackerState {
  status: Option<ServerStatusParams>,
  progress: Vec<ActiveProgress>,
  progress_totals: HashMap<String, (u64, u64)>,
  announcement: Option<Value>,
  initialized_at: Option<Instant>,
  disconnected: Option<LspError>,
  activity: u64,
  diagnostics: HashMap<PathBuf, Published>,
  diagnostics_sequence: u64,
}

enum Verdict {
  Ready,
  Waiting { state: ServerState, wake: Option<Instant> },
  Unhealthy(String),
  Exited(LspError),
}

pub(crate) struct Tracker {
  server: &'static str,
  readiness: Readiness,
  announcement_grace: Duration,
  spec: ServerSpec,
  folders: Vec<WorkspaceFolder>,
  state: Mutex<TrackerState>,
  changed: Condvar,
}

impl Tracker {
  pub(crate) fn new(spec: &ServerSpec, announcement_grace: Duration) -> LspResult<Self> {
    Ok(Self {
      server: spec.kind.name(),
      readiness: spec.readiness(),
      announcement_grace,
      folders: spec.workspace_folders()?,
      spec: spec.clone(),
      state: Mutex::new(TrackerState::default()),
      changed: Condvar::new(),
    })
  }

  pub(crate) fn mark_initialized(&self) {
    lock(&self.state).initialized_at = Some(Instant::now());
    self.changed.notify_all();
  }

  pub(crate) fn state(&self) -> ServerState {
    match self.evaluate(&lock(&self.state), Instant::now()) {
      Verdict::Ready => ServerState::Ready,
      Verdict::Waiting { state, .. } => state,
      Verdict::Unhealthy(reason) => ServerState::Failed { reason },
      Verdict::Exited(error) => ServerState::Exited { reason: error.to_string() },
    }
  }

  pub(crate) fn disconnected(&self) -> Option<LspError> {
    lock(&self.state).disconnected.clone()
  }

  pub(crate) fn activity(&self) -> u64 {
    lock(&self.state).activity
  }

  pub(crate) fn typescript_version(&self) -> Option<TypeScriptVersion> {
    let announcement = lock(&self.state).announcement.clone()?;
    serde_json::from_value(announcement).ok()
  }

  pub(crate) fn wait_ready(&self, timeout: Duration) -> LspResult<()> {
    let deadline = Instant::now() + timeout;
    let mut state = lock(&self.state);
    loop {
      let now = Instant::now();
      match self.evaluate(&state, now) {
        Verdict::Ready => return Ok(()),
        Verdict::Exited(error) => return Err(error),
        Verdict::Unhealthy(message) => {
          return Err(LspError::Unhealthy {
            server: self.server.to_owned(),
            message,
          });
        }
        Verdict::Waiting { state: waiting, wake } => {
          if now >= deadline {
            return Err(LspError::NotReady {
              server: self.server.to_owned(),
              waited: timeout,
              status: waiting.to_string(),
            });
          }
          let until = wake.map_or(deadline, |wake| wake.min(deadline));
          state = self
            .changed
            .wait_timeout(state, until.saturating_duration_since(now).max(MIN_WAIT))
            .unwrap_or_else(PoisonError::into_inner)
            .0;
        }
      }
    }
  }

  pub(crate) fn settle(&self, activity_before: u64, window: Duration, timeout: Duration) -> LspResult<Settle> {
    let active = {
      let state = lock(&self.state);
      let (state, _) = self
        .changed
        .wait_timeout_while(state, window, |state| state.activity <= activity_before && state.disconnected.is_none())
        .unwrap_or_else(PoisonError::into_inner);
      state.activity > activity_before
    };
    match self.wait_ready(timeout) {
      Ok(()) if active => Ok(Settle::Ready),
      Ok(()) => Ok(Settle::Quiet),
      Err(LspError::NotReady { status, .. }) => Ok(Settle::Busy { status }),
      Err(error) => Err(error),
    }
  }

  pub(crate) fn progress(&self, prefix: &str) -> ProgressCount {
    progress_count(&lock(&self.state), prefix)
  }

  pub(crate) fn wait_progress(&self, prefix: &str, timeout: Duration, until: impl Fn(&ProgressCount) -> bool) -> ProgressCount {
    let state = lock(&self.state);
    let (state, _) = self
      .changed
      .wait_timeout_while(state, timeout, |state| !until(&progress_count(state, prefix)) && state.disconnected.is_none())
      .unwrap_or_else(PoisonError::into_inner);
    progress_count(&state, prefix)
  }

  pub(crate) fn diagnostics(&self, path: &Path) -> Option<(u64, PublishDiagnosticsParams)> {
    lock(&self.state).diagnostics.get(path).map(|published| (published.sequence, published.params.clone()))
  }

  pub(crate) fn diagnostics_sequence(&self) -> u64 {
    lock(&self.state).diagnostics_sequence
  }

  pub(crate) fn wait_diagnostics(&self, path: &Path, after: u64, timeout: Duration) -> Option<PublishDiagnosticsParams> {
    let state = lock(&self.state);
    let fresh = |state: &TrackerState| state.diagnostics.get(path).filter(|published| published.sequence > after).is_some();
    let (state, _) = self
      .changed
      .wait_timeout_while(state, timeout, |state| !fresh(state) && state.disconnected.is_none())
      .unwrap_or_else(PoisonError::into_inner);
    state
      .diagnostics
      .get(path)
      .filter(|published| published.sequence > after)
      .map(|published| published.params.clone())
  }

  fn evaluate(&self, state: &TrackerState, now: Instant) -> Verdict {
    if let Some(error) = &state.disconnected {
      return Verdict::Exited(error.clone());
    }
    match self.readiness {
      Readiness::Quiescent { ignored_progress } => quiescence(state, ignored_progress),
      Readiness::Announced { .. } => self.announcement(state, now),
    }
  }

  fn announcement(&self, state: &TrackerState, now: Instant) -> Verdict {
    let Some(initialized_at) = state.initialized_at else {
      return Verdict::Waiting {
        state: ServerState::Starting,
        wake: None,
      };
    };
    let grace_ends = initialized_at + self.announcement_grace;
    if state.announcement.is_none() && now < grace_ends {
      return Verdict::Waiting {
        state: ServerState::Starting,
        wake: Some(grace_ends),
      };
    }
    match summary(state.progress.iter()) {
      Some(message) => Verdict::Waiting {
        state: ServerState::Indexing { message: Some(message) },
        wake: None,
      },
      None => Verdict::Ready,
    }
  }

  fn record_progress(state: &mut TrackerState, params: ProgressParams) {
    let Some(progress) = params.work_done() else {
      return;
    };
    let existing = state.progress.iter().position(|active| active.token == params.token);
    let totals = state.progress_totals.entry(params.token.to_string()).or_default();
    match (&progress, existing) {
      (WorkDoneProgress::Begin(_), _) => totals.0 += 1,
      (WorkDoneProgress::End(_), Some(_)) => totals.1 += 1,
      _ => {}
    }
    match (progress, existing) {
      (WorkDoneProgress::Begin(begin), existing) => {
        let active = ActiveProgress {
          token: params.token,
          title: begin.title,
          message: begin.message,
          percentage: begin.percentage,
        };
        match existing {
          Some(index) => state.progress[index] = active,
          None => state.progress.push(active),
        }
      }
      (WorkDoneProgress::Report(report), Some(index)) => {
        let active = &mut state.progress[index];
        active.message = report.message.or(active.message.take());
        active.percentage = report.percentage.or(active.percentage);
      }
      (WorkDoneProgress::End(_), Some(index)) => {
        state.progress.remove(index);
      }
      (WorkDoneProgress::Report(_) | WorkDoneProgress::End(_), None) => {}
    }
  }
}

fn quiescence(state: &TrackerState, ignored_progress: &[&str]) -> Verdict {
  let Some(status) = &state.status else {
    return Verdict::Waiting {
      state: ServerState::Starting,
      wake: None,
    };
  };
  let blocking = state.progress.iter().filter(|active| !ignored(&active.token, ignored_progress));
  let busy = summary(blocking);
  match (status.quiescent, status.health, busy) {
    (true, Health::Error, _) => Verdict::Unhealthy(status.message.clone().unwrap_or_else(|| "health error without a message".to_owned())),
    (true, _, None) => Verdict::Ready,
    (_, _, busy) => Verdict::Waiting {
      state: ServerState::Indexing {
        message: busy.or_else(|| status.message.clone()),
      },
      wake: None,
    },
  }
}

fn progress_count(state: &TrackerState, prefix: &str) -> ProgressCount {
  let (begun, ended) = state
    .progress_totals
    .iter()
    .filter(|(token, _)| token.starts_with(prefix))
    .fold((0, 0), |(begun, ended), (_, totals)| (begun + totals.0, ended + totals.1));
  ProgressCount {
    begun,
    ended,
    active: state.progress.iter().filter(|active| active.token.to_string().starts_with(prefix)).count(),
  }
}

fn ignored(token: &ProgressToken, prefixes: &[&str]) -> bool {
  let token = token.to_string();
  prefixes.iter().any(|prefix| token.starts_with(prefix))
}

fn summary<'a>(progress: impl Iterator<Item = &'a ActiveProgress>) -> Option<String> {
  let parts: Vec<String> = progress.map(ToString::to_string).collect();
  match parts.is_empty() {
    true => None,
    false => Some(parts.join(", ")),
  }
}

impl ClientHandler for Tracker {
  fn configuration(&self, item: &ConfigurationItem) -> Value {
    self.spec.configuration_for(item.section.as_deref())
  }

  fn workspace_folders(&self) -> Option<Vec<WorkspaceFolder>> {
    Some(self.folders.clone())
  }

  fn on_event(&self, event: ServerEvent) {
    let mut state = lock(&self.state);
    match event {
      ServerEvent::Progress(params) => {
        Self::record_progress(&mut state, params);
        state.activity += 1;
      }
      ServerEvent::ProgressCreated(_) => state.activity += 1,
      ServerEvent::ServerStatus(status) => {
        state.status = Some(status);
        state.activity += 1;
      }
      ServerEvent::Diagnostics(params) => match path_from_uri(&params.uri) {
        Ok(path) => {
          state.diagnostics_sequence += 1;
          let sequence = state.diagnostics_sequence;
          state.diagnostics.insert(path, Published { sequence, params });
        }
        Err(error) => tracing::debug!(%error, server = self.server, "ignoring diagnostics for a non-file uri"),
      },
      ServerEvent::Notification { method, params } => match self.readiness {
        Readiness::Announced { notification } if notification == method => {
          state.announcement = Some(params);
          state.activity += 1;
        }
        _ => return,
      },
      ServerEvent::ShowMessage(message) => {
        tracing::debug!(server = self.server, text = message.message, "language server message");
        return;
      }
      ServerEvent::Disconnected(error) => state.disconnected = Some(error),
    }
    drop(state);
    self.changed.notify_all();
  }
}

#[cfg(test)]
mod tests {
  use std::path::Path;

  use serde_json::json;

  use super::*;
  use crate::error::Disconnect;
  use crate::process::{ProcessExit, ServerCommand};
  use crate::rust_analyzer::RustAnalyzerSettings;
  use crate::spec::{Launch, ServerUnit, UnitFolder, UnitSettings};
  use crate::typescript::TypeScriptSettings;

  fn tracker(settings: UnitSettings, grace: Duration) -> Tracker {
    let work = tempfile::tempdir().unwrap();
    let unit = ServerUnit::new("demo", vec![UnitFolder::new("w", work.path())], settings, ServerCommand::new("server"), Launch::Installed);
    let spec = unit.spec().unwrap();
    Tracker::new(&spec, grace).unwrap()
  }

  fn rust_tracker() -> Tracker {
    tracker(
      UnitSettings::RustAnalyzer(RustAnalyzerSettings::new(vec![], std::env::temp_dir().join("atlas-lsp-ready-target"))),
      Duration::ZERO,
    )
  }

  fn progress(token: &str, value: Value) -> ServerEvent {
    ServerEvent::Progress(serde_json::from_value(json!({"token": token, "value": value})).unwrap())
  }

  fn status(quiescent: bool, health: &str, message: Option<&str>) -> ServerEvent {
    ServerEvent::ServerStatus(serde_json::from_value(json!({"quiescent": quiescent, "health": health, "message": message})).unwrap())
  }

  #[test]
  fn should_count_begun_and_ended_progress_by_token_prefix() {
    let tracker = rust_tracker();
    tracker.on_event(progress("rust-analyzer/flycheck/0", json!({"kind": "begin", "title": "cargo check"})));
    tracker.on_event(progress("rust-analyzer/flycheck/1", json!({"kind": "begin", "title": "cargo check"})));
    tracker.on_event(progress("rustAnalyzer/Indexing", json!({"kind": "begin", "title": "Indexing"})));
    tracker.on_event(progress("rust-analyzer/flycheck/0", json!({"kind": "end"})));
    tracker.on_event(progress("rust-analyzer/flycheck/9", json!({"kind": "end"})));

    assert_eq!(tracker.progress("rust-analyzer/flycheck"), ProgressCount { begun: 2, ended: 1, active: 1 });
    assert_eq!(
      tracker.wait_progress("rust-analyzer/flycheck", Duration::from_millis(10), |count| count.ended == 2),
      ProgressCount { begun: 2, ended: 1, active: 1 }
    );
    tracker.on_event(progress("rust-analyzer/flycheck/1", json!({"kind": "end"})));
    assert_eq!(
      tracker.wait_progress("rust-analyzer/flycheck", Duration::from_secs(5), |count| count.ended == 2),
      ProgressCount { begun: 2, ended: 2, active: 0 }
    );
    assert_eq!(tracker.progress("rustAnalyzer"), ProgressCount { begun: 1, ended: 0, active: 1 });
  }

  #[test]
  fn should_wait_for_quiescence_and_the_end_of_blocking_progress() {
    let tracker = rust_tracker();
    assert_eq!(tracker.state(), ServerState::Starting);

    tracker.on_event(progress(
      "rustAnalyzer/Fetching",
      json!({"kind": "begin", "title": "Fetching", "message": "cargo metadata"}),
    ));
    tracker.on_event(status(false, "ok", None));
    assert_eq!(
      tracker.state(),
      ServerState::Indexing {
        message: Some("Fetching cargo metadata".to_owned())
      }
    );

    tracker.on_event(progress("rustAnalyzer/Indexing", json!({"kind": "begin", "title": "Indexing", "percentage": 0})));
    tracker.on_event(progress("rustAnalyzer/Indexing", json!({"kind": "report", "message": "3/10", "percentage": 30})));
    tracker.on_event(progress("rustAnalyzer/Fetching", json!({"kind": "end"})));
    tracker.on_event(status(true, "ok", None));
    assert_eq!(
      tracker.state(),
      ServerState::Indexing {
        message: Some("Indexing 3/10 30%".to_owned())
      }
    );

    tracker.on_event(progress("rust-analyzer/flycheck/0", json!({"kind": "begin", "title": "cargo check"})));
    tracker.on_event(progress("rustAnalyzer/cachePriming", json!({"kind": "begin", "title": "Indexing"})));
    tracker.on_event(progress("rustAnalyzer/Indexing", json!({"kind": "end"})));
    assert_eq!(tracker.state(), ServerState::Ready);
    tracker.wait_ready(Duration::ZERO).unwrap();
    assert_eq!(tracker.activity(), 9);
  }

  #[test]
  fn should_fail_fast_on_a_quiescent_server_with_an_error_health() {
    let tracker = rust_tracker();
    tracker.on_event(status(false, "error", Some("cargo metadata failed")));
    assert_eq!(
      tracker.state(),
      ServerState::Indexing {
        message: Some("cargo metadata failed".to_owned())
      }
    );

    tracker.on_event(status(true, "error", Some("cargo metadata failed")));

    assert_eq!(
      tracker.wait_ready(Duration::from_secs(60)).unwrap_err().to_string(),
      "rust-analyzer reports an unhealthy workspace: cargo metadata failed; fix the workspace (rust-analyzer needs `cargo metadata` to succeed) and check it with `code-atlas lsp check`"
    );
  }

  #[test]
  fn should_report_what_the_server_was_doing_when_readiness_times_out() {
    let tracker = rust_tracker();
    tracker.on_event(progress(
      "rustAnalyzer/Building",
      json!({"kind": "begin", "title": "Building CrateGraph", "message": "2/9"}),
    ));
    tracker.on_event(status(false, "ok", None));

    let error = tracker.wait_ready(Duration::from_millis(50)).unwrap_err();

    assert_eq!(
      error.to_string(),
      "rust-analyzer was not ready after 0.05s (indexing: Building CrateGraph 2/9); check it with `code-atlas lsp check`, and on a large workspace raise lsp.ready_timeout_secs in the project config"
    );
  }

  #[test]
  fn should_be_ready_on_the_typescript_version_or_after_the_grace_period() {
    let announced = tracker(UnitSettings::TypeScript(TypeScriptSettings::default()), Duration::from_secs(600));
    let silent = tracker(UnitSettings::TypeScript(TypeScriptSettings::default()), Duration::from_millis(30));
    announced.mark_initialized();
    silent.mark_initialized();
    assert_eq!(announced.state(), ServerState::Starting);

    announced.on_event(ServerEvent::Notification {
      method: "$/typescriptVersion".to_owned(),
      params: json!({"version": "5.9.3", "source": "bundled"}),
    });

    announced.wait_ready(Duration::ZERO).unwrap();
    assert_eq!(
      announced.typescript_version(),
      Some(TypeScriptVersion {
        version: "5.9.3".to_owned(),
        source: Some("bundled".to_owned())
      })
    );
    silent.wait_ready(Duration::from_secs(10)).unwrap();
    assert_eq!(silent.typescript_version(), None);
  }

  #[test]
  fn should_wait_for_typescript_project_loading_progress() {
    let tracker = tracker(UnitSettings::TypeScript(TypeScriptSettings::default()), Duration::ZERO);
    tracker.mark_initialized();
    tracker.on_event(progress("1", json!({"kind": "begin", "title": "Initializing JS/TS language features…"})));

    assert_eq!(
      tracker.state(),
      ServerState::Indexing {
        message: Some("Initializing JS/TS language features…".to_owned())
      }
    );
    tracker.on_event(progress("1", json!({"kind": "end"})));
    assert_eq!(tracker.state(), ServerState::Ready);
  }

  #[test]
  fn should_return_the_disconnect_error_while_waiting() {
    let tracker = rust_tracker();
    tracker.on_event(ServerEvent::Disconnected(LspError::ServerExited {
      cause: Disconnect::ProcessExited(ProcessExit::Code(3)),
      stderr_tail: "boom".to_owned(),
    }));

    let error = tracker.wait_ready(Duration::from_secs(60)).unwrap_err();

    assert_eq!(error.to_string(), "language server exited (the process exited with code 3); stderr tail:\nboom");
    assert_eq!(
      tracker.state(),
      ServerState::Exited {
        reason: "language server exited (the process exited with code 3); stderr tail:\nboom".to_owned()
      }
    );
  }

  #[test]
  fn should_keep_the_latest_diagnostics_per_file() {
    let tracker = rust_tracker();
    let publish = |message: &str| {
      ServerEvent::Diagnostics(
        serde_json::from_value(json!({"uri": "file:///w/src/lib.rs", "diagnostics": [
          {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "message": message}
        ]}))
        .unwrap(),
      )
    };
    tracker.on_event(publish("first"));
    let mark = tracker.diagnostics_sequence();
    tracker.on_event(publish("second"));

    let (sequence, latest) = tracker.diagnostics(Path::new("/w/src/lib.rs")).unwrap();

    assert_eq!(sequence, 2);
    assert_eq!(latest.diagnostics[0].message, "second");
    assert_eq!(
      tracker
        .wait_diagnostics(Path::new("/w/src/lib.rs"), mark, Duration::ZERO)
        .map(|params| params.diagnostics[0].message.clone()),
      Some("second".to_owned())
    );
    assert_eq!(tracker.wait_diagnostics(Path::new("/w/src/lib.rs"), 2, Duration::from_millis(10)), None);
  }

  #[test]
  fn should_answer_rust_analyzer_configuration_and_workspace_folders() {
    let tracker = rust_tracker();

    let section = tracker.configuration(&ConfigurationItem {
      scope_uri: None,
      section: Some("rust-analyzer".to_owned()),
    });

    assert_eq!(section["checkOnSave"], json!(false));
    assert_eq!(tracker.workspace_folders().map(|folders| folders.len()), Some(1));
  }
}
