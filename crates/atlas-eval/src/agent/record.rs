use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::accuracy::AtlasAccuracy;
use super::claude::ConfigIsolation;
use super::error::{AgentError, AgentResult};
use super::judge::JudgeOutcome;
use super::plan::{Condition, PlannedRun};
use super::probe::ProbeReport;
use super::process::ProcessExit;
use super::reuse::{DuplicationFinding, NewFunction, ReuseReference};
use super::spec::GroundTruth;
use super::validation::{Regressions, SuiteResult, TestCounts};
use super::workspace::TreeSource;

pub const RUN_FILE: &str = "run.json";
pub const METRICS_FILE: &str = "metrics.json";
pub const SCORE_FILE: &str = "score.json";
pub const BATCH_FILE: &str = "batch.json";
pub const BASE_FILE: &str = "base.json";
pub const ATLAS_REVIEW_FILE: &str = "atlas-review.json";
pub const TRANSCRIPT_FILE: &str = "transcript.jsonl";
pub const OFFSETS_FILE: &str = "transcript.offsets.json";
pub const DIFF_FILE: &str = "diff.patch";
pub const BASE_DIR: &str = "_base";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RunStatus {
  Completed { session: SessionOutcome },
  SetupFailed { stage: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SessionOutcome {
  Finished { subtype: String },
  MaxTurns,
  ReportedError { subtype: String },
  NoResultEvent { exit: ProcessExit },
  TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
  Reported,
  EstimatedFromUsage,
  Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupStageKind {
  Infrastructure,
  Product,
}

pub fn setup_stage_kind(stage: &str) -> SetupStageKind {
  if stage.starts_with("atlas-") {
    SetupStageKind::Product
  } else {
    SetupStageKind::Infrastructure
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunManifest {
  pub run_id: String,
  pub task_id: String,
  pub condition: Condition,
  pub run_index: u32,
  pub order: usize,
  pub spec_hash: String,
  pub source: TreeSource,
  pub base_sha: Option<String>,
  pub tree: Option<String>,
  pub project_instructions: Vec<String>,
  pub model_requested: String,
  pub models_reported: Vec<String>,
  pub claude_code_version: Option<String>,
  pub preamble_version: Option<u32>,
  pub args: Vec<String>,
  pub env_names: Vec<String>,
  pub atlas_project: Option<String>,
  pub atlas_index_ms: Option<u64>,
  pub wall_ms: Option<u64>,
  pub session_cost_usd: Option<f64>,
  pub cost_source: CostSource,
  pub cost_usd: f64,
  pub status: RunStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
  pub covered: Vec<String>,
  pub missing: Vec<String>,
  pub ratio: Option<f64>,
}

impl Coverage {
  pub fn of(expected: &[String], seen: &BTreeSet<&String>) -> Self {
    let (covered, missing): (Vec<String>, Vec<String>) = expected.iter().cloned().partition(|path| seen.contains(path));
    let ratio = (!expected.is_empty()).then(|| covered.len() as f64 / expected.len() as f64);
    Self { covered, missing, ratio }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DuplicationCheck {
  NotApplicable {
    reason: String,
  },
  Unavailable {
    reason: String,
  },
  Checked {
    new_functions: Vec<NewFunction>,
    findings: Vec<DuplicationFinding>,
    failed_queries: Vec<String>,
  },
}

impl DuplicationCheck {
  pub fn introduced(&self) -> Option<bool> {
    match self {
      Self::Checked { findings, .. } if !findings.is_empty() => Some(true),
      Self::Checked { failed_queries, .. } if failed_queries.is_empty() => Some(false),
      _ => None,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HiddenTests {
  pub passed: bool,
  pub commands_passed: usize,
  pub commands_total: usize,
  pub counts: Option<TestCounts>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Score {
  pub hidden_tests: Option<HiddenTests>,
  pub validation: SuiteResult,
  pub regression: SuiteResult,
  pub regressions: Regressions,
  pub regression_count: Option<usize>,
  pub must_change_coverage: Coverage,
  pub must_consider_coverage: Coverage,
  pub reuse_references: Vec<ReuseReference>,
  pub duplication: DuplicationCheck,
  pub duplication_introduced: Option<bool>,
  pub new_functions_named_like_targets: Vec<String>,
  pub judge: JudgeOutcome,
  pub atlas_accuracy: Option<AtlasAccuracy>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaseCheck {
  pub task_id: String,
  pub validation: SuiteResult,
  pub regression: SuiteResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
  pub date: String,
  pub seed: u64,
  pub claude_version: Option<String>,
  pub model: String,
  pub judge_model: Option<String>,
  pub config_isolation: ConfigIsolation,
  pub code_atlas_commit: Option<String>,
  pub code_atlas_version: Option<String>,
  pub atlas_bin: String,
  pub preamble_version: u32,
  pub repo_override: Option<String>,
  pub probe: Option<ProbeReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchManifest {
  pub environment: Environment,
  pub tasks: Vec<String>,
  pub truths: Vec<(String, GroundTruth)>,
  pub planned: Vec<PlannedRun>,
  pub skipped_for_budget: Vec<String>,
  pub spent_usd: f64,
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> AgentResult<T> {
  let text = fs::read_to_string(path).map_err(|error| AgentError::io(path, error))?;
  serde_json::from_str(&text).map_err(|error| AgentError::Parse {
    path: path.to_owned(),
    reason: error.to_string(),
  })
}

pub fn write_json(path: &Path, value: &impl Serialize) -> AgentResult<()> {
  let text = serde_json::to_string_pretty(value).map_err(|error| AgentError::Parse {
    path: path.to_owned(),
    reason: error.to_string(),
  })?;
  fs::write(path, text + "\n").map_err(|error| AgentError::io(path, error))
}
