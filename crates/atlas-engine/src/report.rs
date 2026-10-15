use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use atlas_core::{RelPath, RepoName, RunId};
use atlas_project::SkipReason;
use atlas_store::IndexRun;
use serde::{Deserialize, Serialize};

use crate::auto_scip::AutoScipStatus;

pub const SKIPPED_SAMPLE: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BudgetRetryStatus {
  pub repo: RepoName,
  pub path: RelPath,
  pub attempts: u32,
  pub retry_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FailedFile {
  pub repo: RepoName,
  pub path: Option<RelPath>,
  pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkipCause {
  TooLarge { size: u64, limit: u64 },
  NonUtf8Path,
  Symlink,
  Unreadable { message: String },
}

fn human_bytes(bytes: u64) -> String {
  const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
  let mut value = bytes as f64;
  let mut unit = 0;
  while value >= 1024.0 && unit < UNITS.len() - 1 {
    value /= 1024.0;
    unit += 1;
  }
  match (value.fract() == 0.0, unit) {
    (true, _) | (_, 0) => format!("{value:.0} {}", UNITS[unit]),
    _ => format!("{value:.1} {}", UNITS[unit]),
  }
}

impl fmt::Display for SkipCause {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::TooLarge { size, limit } => write!(f, "{} > max_file_bytes {}", human_bytes(*size), human_bytes(*limit)),
      Self::NonUtf8Path => f.write_str("path is not valid UTF-8"),
      Self::Symlink => f.write_str("symlink"),
      Self::Unreadable { message } => write!(f, "unreadable: {message}"),
    }
  }
}

impl From<&SkipReason> for SkipCause {
  fn from(reason: &SkipReason) -> Self {
    match reason {
      SkipReason::TooLarge { size, limit } => Self::TooLarge { size: *size, limit: *limit },
      SkipReason::NonUtf8Path => Self::NonUtf8Path,
      SkipReason::Symlink => Self::Symlink,
      SkipReason::Unreadable(message) => Self::Unreadable { message: message.clone() },
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SkippedFile {
  pub repo: RepoName,
  pub path: RelPath,
  pub cause: SkipCause,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkippedSummary {
  pub total: u64,
  pub first: Vec<SkippedFile>,
}

impl SkippedSummary {
  pub fn of<'a>(files: impl IntoIterator<Item = &'a SkippedFile>) -> Self {
    let mut summary = Self::default();
    for file in files {
      summary.total += 1;
      if summary.first.len() < SKIPPED_SAMPLE {
        summary.first.push(file.clone());
      }
    }
    summary
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanKind {
  StatScan,
  WatcherClean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScipLag {
  NeverRun,
  Current,
  Behind { files: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshnessHeader {
  pub run: Option<RunId>,
  pub last_run: Option<RunId>,
  pub files_updated: u64,
  pub duration: Duration,
  pub scan: ScanKind,
  pub scip: ScipLag,
  pub stale_inferred_facts: u64,
  pub files_failed_analysis: u64,
  pub skipped: SkippedSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreshnessReport {
  Fresh(FreshnessHeader),
  Degraded {
    header: FreshnessHeader,
    failed_files: Vec<FailedFile>,
    reason: String,
  },
}

impl FreshnessReport {
  pub fn header(&self) -> &FreshnessHeader {
    match self {
      Self::Fresh(header) | Self::Degraded { header, .. } => header,
    }
  }

  pub fn is_fresh(&self) -> bool {
    matches!(self, Self::Fresh(_))
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcomeKind {
  Ok,
  Partial { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
  pub run: RunId,
  pub outcome: RunOutcomeKind,
  pub files_updated: u64,
  pub failed_files: Vec<FailedFile>,
  pub phase_timings_ms: BTreeMap<String, u64>,
  pub counts: BTreeMap<String, u64>,
  pub duration: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InferredCounts {
  pub current: u64,
  pub stale: u64,
  pub needs_revalidation: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScipCoverage {
  pub files_with_scip: u64,
  pub files_total: u64,
  pub last_run_ms: Option<i64>,
  pub lag: Option<ScipLag>,
  pub auto: AutoScipStatus,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LlmSpend {
  pub calls: u64,
  pub input_tokens: u64,
  pub output_tokens: u64,
  pub cost_estimate_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RouteFamilyState {
  Matched { controllers: u64, routes: u64 },
  Undeclared { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteFamilyStatus {
  pub name: String,
  #[serde(flatten)]
  pub state: RouteFamilyState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexStatus {
  pub runs: Vec<IndexRun>,
  pub files_by_language: BTreeMap<String, u64>,
  pub files_by_parse_status: BTreeMap<String, u64>,
  pub failed_files: Vec<FailedFile>,
  pub unresolved_by_reason: BTreeMap<String, u64>,
  pub edge_certainty: BTreeMap<String, BTreeMap<String, u64>>,
  pub scip: ScipCoverage,
  pub inferred: InferredCounts,
  pub llm: LlmSpend,
  pub dangling_detector_edges_dropped: u64,
  pub repos_missing: Vec<FailedFile>,
  pub skipped: SkippedSummary,
  pub budget_retries: Vec<BudgetRetryStatus>,
  pub route_families: Vec<RouteFamilyStatus>,
}
