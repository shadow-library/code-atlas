use std::fs;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::claude::{ATLAS_TOOL_PREFIX, ClaudeConfig, EMPTY_MCP_CONFIG, host_env, judge_invocation};
use super::process::{ProcessExit, run_captured, tail};
use super::validation::SuiteResult;

pub const JUDGE_PROMPT_VERSION: u32 = 1;
pub const RUBRIC_CRITERIA: [&str; 5] = ["correctness", "completeness", "architectural_consistency", "appropriate_reuse", "blast_radius_handling"];
const MAX_DIFF_BYTES: usize = 200 * 1024;
const MAX_ATLAS_EXCERPT_BYTES: usize = 4 * 1024;
const MAX_ATLAS_EXCERPTS: usize = 12;
const JUDGE_TIMEOUT: Duration = Duration::from_secs(600);
const NEUTRAL_TOOL_NAME: &str = "knowledge tool";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeInput<'a> {
  pub task_prompt: &'a str,
  pub diff: &'a str,
  pub validation: &'a SuiteResult,
  pub regression: &'a SuiteResult,
  pub files_read: &'a [String],
  pub files_changed: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CriterionScore {
  pub score: u8,
  pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum JudgeOutcome {
  Scored {
    scores: std::collections::BTreeMap<String, CriterionScore>,
    model: Option<String>,
    cost_usd: Option<f64>,
    prompt_hash: String,
  },
  Reviewed {
    review: Value,
    model: Option<String>,
    cost_usd: Option<f64>,
    prompt_hash: String,
  },
  Failed {
    reason: String,
  },
  Skipped {
    reason: String,
  },
}

impl JudgeOutcome {
  pub fn score(&self, criterion: &str) -> Option<u8> {
    match self {
      Self::Scored { scores, .. } => scores.get(criterion).map(|score| score.score),
      _ => None,
    }
  }

  pub fn cost_usd(&self) -> Option<f64> {
    match self {
      Self::Scored { cost_usd, .. } | Self::Reviewed { cost_usd, .. } => *cost_usd,
      _ => None,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgeKind {
  BlindedRubric,
  AtlasReview,
}

impl JudgeKind {
  pub fn schema(self) -> Value {
    match self {
      Self::BlindedRubric => rubric_schema(),
      Self::AtlasReview => review_schema(),
    }
  }
}

fn rubric_schema() -> Value {
  let criterion = json!({
    "type": "object",
    "properties": {
      "score": { "type": "integer", "minimum": 1, "maximum": 5 },
      "rationale": { "type": "string" }
    },
    "required": ["score", "rationale"],
    "additionalProperties": false
  });
  let properties: serde_json::Map<String, Value> = RUBRIC_CRITERIA.iter().map(|name| ((*name).to_owned(), criterion.clone())).collect();
  json!({ "type": "object", "properties": properties, "required": RUBRIC_CRITERIA, "additionalProperties": false })
}

fn review_schema() -> Value {
  json!({
    "type": "object",
    "properties": {
      "misleading": { "type": "boolean" },
      "examples": {
        "type": "array",
        "items": {
          "type": "object",
          "properties": { "tool_call": { "type": "string" }, "problem": { "type": "string" }, "effect_on_agent": { "type": "string" } },
          "required": ["tool_call", "problem", "effect_on_agent"],
          "additionalProperties": false
        }
      },
      "summary": { "type": "string" }
    },
    "required": ["misleading", "examples", "summary"],
    "additionalProperties": false
  })
}

pub fn blind(text: &str) -> String {
  let mut blinded = String::with_capacity(text.len());
  let mut rest = text;
  while let Some(index) = rest.find(ATLAS_TOOL_PREFIX) {
    blinded.push_str(&rest[..index]);
    blinded.push_str(NEUTRAL_TOOL_NAME);
    let after = &rest[index + ATLAS_TOOL_PREFIX.len()..];
    let name_length = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')).unwrap_or(after.len());
    rest = &after[name_length..];
  }
  blinded.push_str(rest);
  blinded
}

pub fn blinded_prompt(input: &JudgeInput<'_>) -> String {
  let diff = truncate_marked(input.diff, MAX_DIFF_BYTES);
  let text = format!(
    "You are reviewing a code change made by a coding agent. Score it against the rubric. Judge only the change and its results; you are not told how the agent worked.\n\n\
## Task given to the agent\n{task}\n\n\
## Validation results (hidden acceptance tests run after the session)\n{validation}\n\n\
## Regression suite results\n{regression}\n\n\
## Files the agent read\n{read}\n\n\
## Files the agent changed\n{changed}\n\n\
## Diff against the starting commit\n```diff\n{diff}\n```\n\n\
## Rubric (each 1-5, with a short rationale)\n\
- correctness: does the change do what the task asks without bugs?\n\
- completeness: are all parts of the task handled, including tests and wiring?\n\
- architectural_consistency: does it follow the existing structure, patterns and conventions of the codebase?\n\
- appropriate_reuse: does it reuse existing helpers instead of re-implementing them, without forcing unrelated reuse?\n\
- blast_radius_handling: are dependents of the changed code updated or verified where needed?\n",
    task = input.task_prompt.trim(),
    validation = suite_summary(input.validation),
    regression = suite_summary(input.regression),
    read = bullet_list(input.files_read),
    changed = bullet_list(input.files_changed),
  );
  blind(&text)
}

pub fn atlas_review_prompt(task_prompt: &str, excerpts: &[(String, String, String)], diff: &str) -> String {
  let mut calls = String::new();
  for (index, (tool, arguments, response)) in excerpts.iter().take(MAX_ATLAS_EXCERPTS).enumerate() {
    calls.push_str(&format!(
      "### Call {} — {tool}\nArguments: {arguments}\n```\n{}\n```\n\n",
      index + 1,
      truncate_marked(response, MAX_ATLAS_EXCERPT_BYTES)
    ));
  }
  format!(
    "A coding agent used Code Atlas, a repository knowledge tool, while doing the task below. Decide whether any Code Atlas output was wrong or misled the agent \
(for example: a missing caller, a wrong definition location, a stale or degraded answer, a heuristic relation presented as certain, or an irrelevant context pack). \
Give concrete examples tied to the calls below; return an empty list when nothing was wrong.\n\n\
## Task\n{task}\n\n## Code Atlas calls\n{calls}## Final diff\n```diff\n{diff}\n```\n",
    task = task_prompt.trim(),
    diff = truncate_marked(diff, MAX_DIFF_BYTES),
  )
}

fn suite_summary(suite: &SuiteResult) -> String {
  if suite.commands.is_empty() {
    return "(none configured)".to_owned();
  }
  suite
    .commands
    .iter()
    .map(|command| {
      let state = if command.passed() { "passed" } else { "FAILED" };
      let counts = command.counts.map_or_else(String::new, |counts| {
        format!(" ({} passed, {} failed, {} ignored)", counts.passed, counts.failed, counts.ignored)
      });
      format!("- `{}`: {state}{counts}", command.command)
    })
    .collect::<Vec<_>>()
    .join("\n")
}

fn bullet_list(items: &[String]) -> String {
  if items.is_empty() {
    return "(none)".to_owned();
  }
  items.iter().map(|item| format!("- {item}")).collect::<Vec<_>>().join("\n")
}

fn truncate_marked(text: &str, limit: usize) -> String {
  if text.len() <= limit {
    return text.to_owned();
  }
  let mut end = limit;
  while !text.is_char_boundary(end) {
    end -= 1;
  }
  format!("{}\n[truncated: {} more bytes omitted]", &text[..end], text.len() - end)
}

pub fn prompt_hash(prompt: &str) -> String {
  blake3::hash(prompt.as_bytes()).to_hex()[..16].to_owned()
}

pub fn run_judge(config: &ClaudeConfig, kind: JudgeKind, prompt: &str, work_dir: &Path) -> JudgeOutcome {
  if let Err(error) = fs::create_dir_all(work_dir) {
    return JudgeOutcome::Failed { reason: error.to_string() };
  }
  let mcp_config = work_dir.join("empty-mcp.json");
  if let Err(error) = fs::write(&mcp_config, EMPTY_MCP_CONFIG) {
    return JudgeOutcome::Failed { reason: error.to_string() };
  }
  let invocation = judge_invocation(config, &mcp_config, &kind.schema().to_string(), work_dir, &work_dir.join("claude-config"), &host_env());
  let output = run_captured(invocation.command(), Some(prompt.as_bytes()), JUDGE_TIMEOUT);
  match output.exit {
    ProcessExit::Exited { code: Some(0) } => parse_judge_output(&output.stdout, prompt, kind),
    exit => JudgeOutcome::Failed {
      reason: format!("{exit:?}: {}", tail(output.stderr.trim(), 2000)),
    },
  }
}

pub fn parse_judge_output(stdout: &str, prompt: &str, kind: JudgeKind) -> JudgeOutcome {
  let Ok(document) = serde_json::from_str::<Value>(stdout.trim()) else {
    return JudgeOutcome::Failed {
      reason: "claude did not print a JSON document".to_owned(),
    };
  };
  if document.get("is_error").and_then(Value::as_bool) == Some(true) {
    return JudgeOutcome::Failed {
      reason: format!("claude reported an error: {}", document.get("result").and_then(Value::as_str).unwrap_or_default()),
    };
  }
  let structured = match document.get("structured_output") {
    Some(value) if !value.is_null() => Some(value.clone()),
    _ => document.get("result").and_then(Value::as_str).and_then(|text| serde_json::from_str(text).ok()),
  };
  let Some(structured) = structured else {
    return JudgeOutcome::Failed {
      reason: "the judge returned no structured output".to_owned(),
    };
  };
  let model = document.get("modelUsage").and_then(Value::as_object).and_then(|models| models.keys().next().cloned());
  let cost_usd = document.get("total_cost_usd").and_then(Value::as_f64);
  let prompt_hash = prompt_hash(prompt);
  if kind == JudgeKind::AtlasReview {
    return JudgeOutcome::Reviewed {
      review: structured,
      model,
      cost_usd,
      prompt_hash,
    };
  }
  let mut scores = std::collections::BTreeMap::new();
  for criterion in RUBRIC_CRITERIA {
    let parsed = structured.get(criterion).and_then(|value| serde_json::from_value::<CriterionScore>(value.clone()).ok());
    match parsed.filter(|score| (1..=5).contains(&score.score)) {
      Some(score) => scores.insert(criterion.to_owned(), score),
      None => {
        return JudgeOutcome::Failed {
          reason: format!("criterion `{criterion}` is missing or out of range"),
        };
      }
    };
  }
  JudgeOutcome::Scored {
    scores,
    model,
    cost_usd,
    prompt_hash,
  }
}

#[cfg(test)]
mod tests {
  use super::super::process::ProcessExit;
  use super::super::validation::{CommandResult, TestCounts};
  use super::*;

  #[test]
  fn should_replace_atlas_tool_names_with_a_neutral_label() {
    assert_eq!(blind("used mcp__code-atlas__atlas_context then Read"), "used knowledge tool then Read");
    assert_eq!(blind("no tools"), "no tools");
  }

  fn suite() -> SuiteResult {
    SuiteResult {
      commands: vec![CommandResult {
        command: "node check.mjs".to_owned(),
        exit: ProcessExit::Exited { code: Some(0) },
        duration_ms: 3,
        counts: Some(TestCounts { passed: 2, failed: 0, ignored: 0 }),
        failing: vec![],
        output_tail: String::new(),
      }],
    }
  }

  #[test]
  fn should_build_a_condition_neutral_prompt() {
    let validation = suite();
    let prompt = blinded_prompt(&JudgeInput {
      task_prompt: "Add deleted_at",
      diff: "+ deletedAt",
      validation: &validation,
      regression: &SuiteResult::default(),
      files_read: &["a.ts".to_owned(), "mcp__code-atlas__atlas_read".to_owned()],
      files_changed: &["a.ts".to_owned()],
    });
    assert!(prompt.contains("- `node check.mjs`: passed (2 passed, 0 failed, 0 ignored)"));
    assert!(prompt.contains("## Regression suite results\n(none configured)"));
    assert!(!prompt.contains("mcp__"));
    assert!(!prompt.to_lowercase().contains("baseline"));
    assert!(!prompt.contains("Code Atlas"));
  }

  #[test]
  fn should_mark_truncated_diffs() {
    let long = "x".repeat(MAX_DIFF_BYTES + 10);
    assert!(truncate_marked(&long, MAX_DIFF_BYTES).ends_with("[truncated: 10 more bytes omitted]"));
  }

  #[test]
  fn should_parse_structured_rubric_scores() {
    let criterion = json!({"score": 4, "rationale": "ok"});
    let structured: serde_json::Map<String, Value> = RUBRIC_CRITERIA.iter().map(|name| ((*name).to_owned(), criterion.clone())).collect();
    let stdout = json!({"is_error": false, "structured_output": structured, "total_cost_usd": 0.02, "modelUsage": {"claude-sonnet-5-5": {}}}).to_string();
    let outcome = parse_judge_output(&stdout, "p", JudgeKind::BlindedRubric);
    assert_eq!(outcome.score("correctness"), Some(4));
    assert_eq!(outcome.cost_usd(), Some(0.02));
  }

  #[test]
  fn should_fail_on_out_of_range_or_missing_scores() {
    let stdout = json!({"structured_output": {"correctness": {"score": 9, "rationale": "?"}}}).to_string();
    assert!(matches!(parse_judge_output(&stdout, "p", JudgeKind::BlindedRubric), JudgeOutcome::Failed { .. }));
    assert!(matches!(parse_judge_output("not json", "p", JudgeKind::BlindedRubric), JudgeOutcome::Failed { .. }));
  }

  #[test]
  fn should_keep_the_unblinded_review_separate() {
    let stdout = json!({"structured_output": {"misleading": true, "examples": [], "summary": "s"}}).to_string();
    assert!(matches!(parse_judge_output(&stdout, "p", JudgeKind::AtlasReview), JudgeOutcome::Reviewed { .. }));
  }
}
