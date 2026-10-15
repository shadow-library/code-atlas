use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::accuracy::AtlasAccuracy;
use super::atlas_config::AtlasConfigRecord;
use super::claude::ConfigIsolation;
use super::error::{AgentError, AgentResult};
use super::judge::JudgeOutcome;
use super::plan::{Condition, PlannedRun, PreambleVariant};
use super::probe::ProbeReport;
use super::process::ProcessExit;
use super::reuse::{DuplicationFinding, LexicalCandidate, NewFunction, ReuseReference, reclassify_legacy};
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
const DENSE_JSON_WIDTH: usize = 160;
const DENSE_JSON_INDENT: usize = 2;

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
  #[serde(default)]
  pub preamble_variant: Option<PreambleVariant>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub atlas_config_blake3: Option<String>,
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
    #[serde(default)]
    lexical_candidates: Vec<LexicalCandidate>,
    failed_queries: Vec<String>,
  },
}

impl DuplicationCheck {
  pub fn introduced(&self) -> Option<bool> {
    match self {
      Self::Checked { findings, .. } if findings.iter().any(|finding| finding.evidence.is_conclusive()) => Some(true),
      Self::Checked { findings, failed_queries, .. } if findings.is_empty() && failed_queries.is_empty() => Some(false),
      _ => None,
    }
  }

  pub fn lexical_candidate_count(&self) -> Option<usize> {
    match self {
      Self::Checked {
        lexical_candidates,
        failed_queries,
        ..
      } if failed_queries.is_empty() => Some(lexical_candidates.len()),
      _ => None,
    }
  }

  fn reclassified(self) -> Self {
    let Self::Checked {
      new_functions,
      findings,
      mut lexical_candidates,
      failed_queries,
    } = self
    else {
      return self;
    };
    let legacy = reclassify_legacy(findings);
    lexical_candidates.extend(legacy.lexical_candidates);
    Self::Checked {
      new_functions,
      findings: legacy.findings,
      lexical_candidates,
      failed_queries,
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

impl Score {
  pub fn rescored(mut self) -> Self {
    self.duplication = self.duplication.reclassified();
    self.duplication_introduced = self.duplication.introduced();
    self
  }
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
  #[serde(default)]
  pub atlas_bin_blake3: Option<String>,
  pub atlas_bin: String,
  pub preamble_version: u32,
  #[serde(default)]
  pub preamble_variant: PreambleVariant,
  pub repo_override: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub atlas_config: Option<AtlasConfigRecord>,
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
  #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
  pub instruction_sections_dropped: BTreeMap<String, Vec<String>>,
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

pub fn write_json_dense(path: &Path, value: &impl Serialize) -> AgentResult<()> {
  let value = serde_json::to_value(value).map_err(|error| AgentError::Parse {
    path: path.to_owned(),
    reason: error.to_string(),
  })?;
  let mut text = String::new();
  render_dense(&value, 0, &mut text);
  fs::write(path, text + "\n").map_err(|error| AgentError::io(path, error))
}

fn inline_json(value: &Value) -> String {
  match value {
    Value::Array(items) => format!("[{}]", items.iter().map(inline_json).collect::<Vec<_>>().join(", ")),
    Value::Object(fields) => {
      let fields: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{}: {}", Value::String(key.clone()), inline_json(value)))
        .collect();
      format!("{{{}}}", fields.join(", "))
    }
    scalar => scalar.to_string(),
  }
}

fn render_dense(value: &Value, indent: usize, out: &mut String) {
  let flat = inline_json(value);
  let fits = indent + flat.len() <= DENSE_JSON_WIDTH;
  let (members, open, close): (Vec<(Option<&String>, &Value)>, char, char) = match value {
    Value::Array(items) if !fits => (items.iter().map(|item| (None, item)).collect(), '[', ']'),
    Value::Object(fields) if !fits => (fields.iter().map(|(key, value)| (Some(key), value)).collect(), '{', '}'),
    _ => {
      out.push_str(&flat);
      return;
    }
  };
  out.push(open);
  for (index, (key, member)) in members.iter().enumerate() {
    out.push_str(if index == 0 { "\n" } else { ",\n" });
    out.push_str(&" ".repeat(indent + DENSE_JSON_INDENT));
    if let Some(key) = key {
      out.push_str(&format!("{}: ", Value::String((*key).clone())));
    }
    render_dense(member, indent + DENSE_JSON_INDENT, out);
  }
  out.push('\n');
  out.push_str(&" ".repeat(indent));
  out.push(close);
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::agent::LexicalCandidate;
  use crate::agent::TargetRole;

  fn checked(failed_queries: Vec<String>) -> DuplicationCheck {
    DuplicationCheck::Checked {
      new_functions: Vec::new(),
      findings: Vec::new(),
      lexical_candidates: vec![LexicalCandidate {
        new_function: "negotiate".to_owned(),
        location: "src/a.ts:1-9".to_owned(),
        matched_symbol: "ts:tree:hono:src/utils/accept::parseAccept#function".to_owned(),
        target: "src/utils/accept::parseAccept".to_owned(),
        role: TargetRole::ReuseTarget,
        score: 5.6,
      }],
      failed_queries,
    }
  }

  #[test]
  fn should_count_lexical_candidates_only_when_every_query_ran() {
    assert_eq!(checked(Vec::new()).lexical_candidate_count(), Some(1));
    assert_eq!(checked(vec!["src/a.ts:1: timed out".to_owned()]).lexical_candidate_count(), None);
    assert_eq!(DuplicationCheck::Unavailable { reason: "no binary".to_owned() }.lexical_candidate_count(), None);
  }

  #[test]
  fn should_keep_small_json_values_on_one_line_and_break_large_ones() {
    let long: Vec<String> = (0..12).map(|index| format!("crates/atlas-engine/tests/file-{index}.rs")).collect();
    let value = serde_json::json!({"read": {"turn": 2, "call": 3, "via": "say \"hi\""}, "files": long, "none": null, "empty": []});
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dense.json");
    write_json_dense(&path, &value).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 19);
    assert_eq!(lines[0], "{");
    assert_eq!(lines[1], "  \"empty\": [],");
    assert_eq!(lines[2], "  \"files\": [");
    assert_eq!(lines[3], "    \"crates/atlas-engine/tests/file-0.rs\",");
    assert_eq!(lines[14], "    \"crates/atlas-engine/tests/file-11.rs\"");
    assert_eq!(lines[15], "  ],");
    assert_eq!(lines[17], "  \"read\": {\"call\": 3, \"turn\": 2, \"via\": \"say \\\"hi\\\"\"}");
    assert_eq!(read_json::<Value>(&path).unwrap(), value);
  }

  #[test]
  fn should_report_unknown_duplication_when_a_query_failed_without_findings() {
    assert_eq!(checked(Vec::new()).introduced(), Some(false));
    assert_eq!(checked(vec!["src/a.ts:1: timed out".to_owned()]).introduced(), None);
  }
}
