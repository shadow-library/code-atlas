use atlas_engine::{FailedFile, FreshnessHeader, FreshnessReport, ScanKind, ScipLag};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailedFileView {
  pub repo: String,
  pub path: Option<String>,
  pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FreshnessState {
  Fresh,
  Degraded { reason: String, failed_files: Vec<FailedFileView> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ScipState {
  NeverRun,
  Current,
  Behind { files: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Freshness {
  #[serde(flatten)]
  pub state: FreshnessState,
  pub run: Option<i64>,
  pub files_updated: u64,
  pub duration_ms: u64,
  pub watcher_clean: bool,
  pub scip: ScipState,
  pub stale_inferred_facts: u64,
  pub files_failed_analysis: u64,
  pub notes: Vec<String>,
}

impl Freshness {
  pub fn from_report(report: &FreshnessReport) -> Self {
    let header = report.header();
    let state = match report {
      FreshnessReport::Fresh(_) => FreshnessState::Fresh,
      FreshnessReport::Degraded { failed_files, reason, .. } => FreshnessState::Degraded {
        reason: reason.clone(),
        failed_files: failed_files.iter().map(failed_view).collect(),
      },
    };
    Self::with_state(header, state)
  }

  fn with_state(header: &FreshnessHeader, state: FreshnessState) -> Self {
    Self {
      state,
      run: header.run.or(header.last_run).map(|run| run.0),
      files_updated: header.files_updated,
      duration_ms: u64::try_from(header.duration.as_millis()).unwrap_or(u64::MAX),
      watcher_clean: header.scan == ScanKind::WatcherClean,
      scip: match header.scip {
        ScipLag::NeverRun => ScipState::NeverRun,
        ScipLag::Current => ScipState::Current,
        ScipLag::Behind { files } => ScipState::Behind { files },
      },
      stale_inferred_facts: header.stale_inferred_facts,
      files_failed_analysis: header.files_failed_analysis,
      notes: Vec::new(),
    }
  }

  pub fn is_degraded(&self) -> bool {
    matches!(self.state, FreshnessState::Degraded { .. })
  }

  pub(crate) fn absorb_resync(&mut self, later: Freshness) {
    self.notes.push(format!(
      "a source file changed while answering; the index was re-synced to run {} and snippets were re-read",
      later.run.map_or_else(|| "none".to_owned(), |run| run.to_string())
    ));
    if let FreshnessState::Degraded { .. } = later.state {
      self.state = later.state;
    }
  }
}

fn failed_view(file: &FailedFile) -> FailedFileView {
  FailedFileView {
    repo: file.repo.to_string(),
    path: file.path.as_ref().map(ToString::to_string),
    reason: file.reason.clone(),
  }
}
