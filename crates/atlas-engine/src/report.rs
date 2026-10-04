use std::collections::BTreeMap;
use std::time::Duration;

use atlas_core::{RelPath, RepoName, RunId};
use atlas_store::IndexRun;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FailedFile {
  pub repo: RepoName,
  pub path: Option<RelPath>,
  pub reason: String,
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
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LlmSpend {
  pub calls: u64,
  pub input_tokens: u64,
  pub output_tokens: u64,
  pub cost_estimate_usd: f64,
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
}
