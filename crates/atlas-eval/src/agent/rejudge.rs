use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::claude::{ClaudeConfig, ConfigIsolation, DEFAULT_JUDGE_MODEL, DEFAULT_SESSION_MODEL};
use super::error::{AgentError, AgentResult};
use super::judge::{JudgeOutcome, judge_atlas_review, judge_rubric};
use super::metrics::Metrics;
use super::plan::Condition;
use super::record::{
  ATLAS_REVIEW_FILE, BATCH_FILE, BatchManifest, DIFF_FILE, Environment, METRICS_FILE, OFFSETS_FILE, RUN_FILE, RunManifest, SCORE_FILE, Score, TRANSCRIPT_FILE, read_json,
  write_json,
};
use super::runner::lock_batch;
use super::spec::LoadedTask;
use super::transcript::Transcript;

pub struct RejudgeRequest<'a> {
  pub batch_dir: &'a Path,
  pub tasks: &'a [LoadedTask],
  pub only: &'a [String],
  pub all: bool,
  pub claude: &'a ClaudeConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RejudgeSummary {
  pub rubrics: usize,
  pub reviews: usize,
  pub still_failed: Vec<String>,
  pub skipped: Vec<String>,
  pub spent_usd: f64,
}

pub struct JudgeSettings<'a> {
  pub binary: &'a Path,
  pub judge_model: Option<&'a str>,
  pub config_isolation: Option<ConfigIsolation>,
}

pub fn rejudge_claude(recorded: Option<&Environment>, settings: &JudgeSettings<'_>) -> AgentResult<ClaudeConfig> {
  let recorded_model = recorded.and_then(|environment| environment.judge_model.as_deref());
  let judge_model = match (settings.judge_model, recorded_model) {
    (Some(given), Some(recorded)) if given != recorded => {
      return Err(AgentError::JudgeSettingConflict {
        flag: "--judge-model".to_owned(),
        given: given.to_owned(),
        recorded: recorded.to_owned(),
      });
    }
    (given, recorded) => given.or(recorded).unwrap_or(DEFAULT_JUDGE_MODEL),
  };
  let recorded_isolation = recorded.map(|environment| environment.config_isolation);
  let config_isolation = match (settings.config_isolation, recorded_isolation) {
    (Some(given), Some(recorded)) if given != recorded => {
      return Err(AgentError::JudgeSettingConflict {
        flag: "--claude-config".to_owned(),
        given: format!("{given:?}").to_lowercase(),
        recorded: format!("{recorded:?}").to_lowercase(),
      });
    }
    (given, recorded) => given.or(recorded).unwrap_or(ConfigIsolation::User),
  };
  Ok(ClaudeConfig {
    binary: settings.binary.to_owned(),
    model: recorded.map_or_else(|| DEFAULT_SESSION_MODEL.to_owned(), |environment| environment.model.clone()),
    judge_model: judge_model.to_owned(),
    config_isolation,
  })
}

pub fn recorded_environment(batch_dir: &Path) -> AgentResult<Option<Environment>> {
  let path = batch_dir.join(BATCH_FILE);
  Ok(path.is_file().then(|| read_json::<BatchManifest>(&path)).transpose()?.map(|batch| batch.environment))
}

pub fn rejudge(request: &RejudgeRequest<'_>) -> AgentResult<RejudgeSummary> {
  let batch_dir = std::path::absolute(request.batch_dir).map_err(|error| AgentError::io(request.batch_dir, error))?;
  let _lock = lock_batch(&batch_dir)?;
  let mut summary = RejudgeSummary::default();
  for dir in run_dirs(&batch_dir)? {
    let mut manifest: RunManifest = read_json(&dir.join(RUN_FILE))?;
    if !request.only.is_empty() && !request.only.contains(&manifest.task_id) {
      continue;
    }
    let mut score: Score = read_json(&dir.join(SCORE_FILE))?;
    let review_path = dir.join(ATLAS_REVIEW_FILE);
    let mut review: Option<JudgeOutcome> = match manifest.condition {
      Condition::Atlas if review_path.is_file() => Some(read_json(&review_path)?),
      _ => None,
    };
    let due = |outcome: &JudgeOutcome| request.all || matches!(outcome, JudgeOutcome::Failed { .. });
    let rubric_due = due(&score.judge);
    let review_due = manifest.condition == Condition::Atlas && review.as_ref().is_none_or(due);
    if !rubric_due && !review_due {
      continue;
    }
    let Some(task) = request.tasks.iter().find(|task| task.spec.id == manifest.task_id && task.spec_hash == manifest.spec_hash) else {
      summary
        .skipped
        .push(format!("{}: no task spec with id `{}` and the recorded spec hash", manifest.run_id, manifest.task_id));
      continue;
    };
    let diff = fs::read_to_string(dir.join(DIFF_FILE)).unwrap_or_default();
    if rubric_due {
      let metrics: Metrics = read_json(&dir.join(METRICS_FILE))?;
      score.judge = judge_rubric(request.claude, &task.spec.prompt, &diff, &score.validation, &score.regression, &metrics, &dir);
      write_json(&dir.join(SCORE_FILE), &score)?;
      summary.rubrics += 1;
      summary.note(&manifest.run_id, "rubric", &score.judge);
    }
    if review_due {
      let outcome = judge_atlas_review(request.claude, &task.spec.prompt, &transcript(&dir)?, &diff, &dir);
      write_json(&review_path, &outcome)?;
      summary.reviews += 1;
      summary.note(&manifest.run_id, "atlas review", &outcome);
      review = Some(outcome);
    }
    manifest.cost_usd = manifest.session_cost_usd.unwrap_or(0.0) + score.judge.cost_usd().unwrap_or(0.0) + review.as_ref().and_then(JudgeOutcome::cost_usd).unwrap_or(0.0);
    write_json(&dir.join(RUN_FILE), &manifest)?;
  }
  let batch_path = batch_dir.join(BATCH_FILE);
  if summary.spent_usd > 0.0 && batch_path.is_file() {
    let mut batch: BatchManifest = read_json(&batch_path)?;
    batch.spent_usd += summary.spent_usd;
    write_json(&batch_path, &batch)?;
  }
  Ok(summary)
}

impl RejudgeSummary {
  fn note(&mut self, run_id: &str, kind: &str, outcome: &JudgeOutcome) {
    self.spent_usd += outcome.cost_usd().unwrap_or(0.0);
    if let JudgeOutcome::Failed { reason } = outcome {
      self.still_failed.push(format!("{run_id} ({kind}): {reason}"));
    }
  }
}

fn run_dirs(batch_dir: &Path) -> AgentResult<Vec<PathBuf>> {
  let entries = fs::read_dir(batch_dir).map_err(|error| AgentError::io(batch_dir, error))?;
  let mut dirs: Vec<PathBuf> = entries
    .filter_map(Result::ok)
    .map(|entry| entry.path())
    .filter(|path| path.join(RUN_FILE).is_file() && path.join(SCORE_FILE).is_file())
    .collect();
  dirs.sort();
  Ok(dirs)
}

fn transcript(dir: &Path) -> AgentResult<Transcript> {
  let path = dir.join(TRANSCRIPT_FILE);
  let text = fs::read_to_string(&path).map_err(|error| AgentError::io(&path, error))?;
  let offsets: Vec<u64> = read_json(&dir.join(OFFSETS_FILE)).unwrap_or_default();
  Ok(Transcript::parse(&text, &offsets))
}
