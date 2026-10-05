use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::access::OutOfTreeAccess;
use super::error::{AgentError, AgentResult};
use super::judge::JudgeOutcome;
use super::metrics::Metrics;
use super::plan::{Condition, PreambleVariant};
use super::record::{
  BATCH_FILE, BatchManifest, CostSource, Environment, METRICS_FILE, RUN_FILE, RunManifest, RunStatus, SCORE_FILE, Score, SessionOutcome, SetupStageKind, TRANSCRIPT_FILE,
  read_json, setup_stage_kind,
};

#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
  pub manifest: RunManifest,
  pub metrics: Option<Metrics>,
  pub score: Option<Score>,
  pub transcript: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricGroup {
  Correctness,
  ReuseAndBlastRadius,
  TokensAndContext,
}

impl MetricGroup {
  fn title(self) -> &'static str {
    match self {
      Self::Correctness => "Correctness",
      Self::ReuseAndBlastRadius => "Reuse and blast radius",
      Self::TokensAndContext => "Tokens and context",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Better {
  Higher,
  Lower,
  Neutral,
}

struct MetricDef {
  name: &'static str,
  group: MetricGroup,
  better: Better,
  extract: fn(&RunRecord) -> Option<f64>,
}

fn metrics_of(record: &RunRecord) -> Option<&Metrics> {
  record.metrics.as_ref()
}

fn score_of(record: &RunRecord) -> Option<&Score> {
  record.score.as_ref()
}

fn judge(record: &RunRecord, criterion: &str) -> Option<f64> {
  score_of(record).and_then(|score| score.judge.score(criterion)).map(f64::from)
}

fn flag(value: bool) -> f64 {
  if value { 1.0 } else { 0.0 }
}

fn count(value: u64) -> f64 {
  value as f64
}

const METRICS: &[MetricDef] = &[
  MetricDef {
    name: "hidden_tests_pass",
    group: MetricGroup::Correctness,
    better: Better::Higher,
    extract: |record| score_of(record)?.hidden_tests.as_ref().map(|hidden| flag(hidden.passed)),
  },
  MetricDef {
    name: "hidden_tests_passed",
    group: MetricGroup::Correctness,
    better: Better::Higher,
    extract: |record| score_of(record)?.hidden_tests.as_ref()?.counts.map(|counts| count(counts.passed)),
  },
  MetricDef {
    name: "regressions",
    group: MetricGroup::Correctness,
    better: Better::Lower,
    extract: |record| score_of(record)?.regression_count.map(|regressions| regressions as f64),
  },
  MetricDef {
    name: "judge_correctness",
    group: MetricGroup::Correctness,
    better: Better::Higher,
    extract: |record| judge(record, "correctness"),
  },
  MetricDef {
    name: "judge_completeness",
    group: MetricGroup::Correctness,
    better: Better::Higher,
    extract: |record| judge(record, "completeness"),
  },
  MetricDef {
    name: "must_change_coverage",
    group: MetricGroup::ReuseAndBlastRadius,
    better: Better::Higher,
    extract: |record| score_of(record)?.must_change_coverage.ratio,
  },
  MetricDef {
    name: "must_consider_coverage",
    group: MetricGroup::ReuseAndBlastRadius,
    better: Better::Higher,
    extract: |record| score_of(record)?.must_consider_coverage.ratio,
  },
  MetricDef {
    name: "reuse_targets_referenced",
    group: MetricGroup::ReuseAndBlastRadius,
    better: Better::Higher,
    extract: |record| {
      let references = &score_of(record)?.reuse_references;
      (!references.is_empty()).then(|| references.iter().filter(|reference| reference.referenced).count() as f64 / references.len() as f64)
    },
  },
  MetricDef {
    name: "duplication_introduced",
    group: MetricGroup::ReuseAndBlastRadius,
    better: Better::Lower,
    extract: |record| score_of(record)?.duplication_introduced.map(flag),
  },
  MetricDef {
    name: "lexical_duplicate_candidates",
    group: MetricGroup::ReuseAndBlastRadius,
    better: Better::Neutral,
    extract: |record| score_of(record)?.duplication.lexical_candidate_count().map(|candidates| candidates as f64),
  },
  MetricDef {
    name: "judge_architectural_consistency",
    group: MetricGroup::ReuseAndBlastRadius,
    better: Better::Higher,
    extract: |record| judge(record, "architectural_consistency"),
  },
  MetricDef {
    name: "judge_appropriate_reuse",
    group: MetricGroup::ReuseAndBlastRadius,
    better: Better::Higher,
    extract: |record| judge(record, "appropriate_reuse"),
  },
  MetricDef {
    name: "judge_blast_radius_handling",
    group: MetricGroup::ReuseAndBlastRadius,
    better: Better::Higher,
    extract: |record| judge(record, "blast_radius_handling"),
  },
  MetricDef {
    name: "total_tokens",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record).map(|metrics| count(metrics.tokens.total.total())),
  },
  MetricDef {
    name: "input_tokens",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record).map(|metrics| count(metrics.tokens.total.input)),
  },
  MetricDef {
    name: "output_tokens",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record).map(|metrics| count(metrics.tokens.total.output)),
  },
  MetricDef {
    name: "cache_read_tokens",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record).map(|metrics| count(metrics.tokens.total.cache_read)),
  },
  MetricDef {
    name: "cache_creation_tokens",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record).map(|metrics| count(metrics.tokens.total.cache_creation)),
  },
  MetricDef {
    name: "cost_usd",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| record.manifest.session_cost_usd,
  },
  MetricDef {
    name: "num_turns",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record)?.tokens.num_turns.map(count),
  },
  MetricDef {
    name: "tool_calls",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record).map(|metrics| count(metrics.tools.total)),
  },
  MetricDef {
    name: "search_calls",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record).map(|metrics| count(metrics.tools.search_calls)),
  },
  MetricDef {
    name: "files_read",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record).map(|metrics| metrics.reads.files.len() as f64),
  },
  MetricDef {
    name: "irrelevant_files_read",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record).map(|metrics| metrics.relevance.irrelevant_read.len() as f64),
  },
  MetricDef {
    name: "relevant_files_missed",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record).map(|metrics| metrics.relevance.relevant_missed.len() as f64),
  },
  MetricDef {
    name: "bytes_read",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record).map(|metrics| count(metrics.reads.bytes)),
  },
  MetricDef {
    name: "lines_read",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record).map(|metrics| count(metrics.reads.lines)),
  },
  MetricDef {
    name: "unparsed_bash_reads",
    group: MetricGroup::TokensAndContext,
    better: Better::Neutral,
    extract: |record| metrics_of(record).map(|metrics| count(metrics.reads.unparsed_bash_reads)),
  },
  MetricDef {
    name: "atlas_calls",
    group: MetricGroup::TokensAndContext,
    better: Better::Neutral,
    extract: |record| metrics_of(record).map(|metrics| count(metrics.atlas.calls)),
  },
  MetricDef {
    name: "atlas_bytes",
    group: MetricGroup::TokensAndContext,
    better: Better::Neutral,
    extract: |record| metrics_of(record).map(|metrics| count(metrics.atlas.bytes)),
  },
  MetricDef {
    name: "information_density",
    group: MetricGroup::TokensAndContext,
    better: Better::Higher,
    extract: |record| metrics_of(record)?.density.ratio,
  },
  MetricDef {
    name: "first_edit_ms",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record)?.timing.first_edit_ms.map(count),
  },
  MetricDef {
    name: "session_ms",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record)?.timing.session_ms.map(count),
  },
  MetricDef {
    name: "wall_ms",
    group: MetricGroup::TokensAndContext,
    better: Better::Lower,
    extract: |record| metrics_of(record)?.timing.wall_ms.map(count),
  },
  MetricDef {
    name: "atlas_index_ms",
    group: MetricGroup::TokensAndContext,
    better: Better::Neutral,
    extract: |record| record.manifest.atlas_index_ms.map(count),
  },
];

const TOKEN_METRIC: &str = "total_tokens";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Stat {
  pub mean: f64,
  pub min: f64,
  pub max: f64,
  pub n: usize,
}

impl Stat {
  fn of(values: &[f64]) -> Option<Self> {
    if values.is_empty() {
      return None;
    }
    let sum: f64 = values.iter().sum();
    Some(Self {
      mean: sum / values.len() as f64,
      min: values.iter().copied().fold(f64::INFINITY, f64::min),
      max: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
      n: values.len(),
    })
  }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeCounts {
  pub completed: usize,
  pub max_turns: usize,
  pub reported_error: usize,
  pub timed_out: usize,
  pub no_result: usize,
  pub setup_failed: usize,
  pub contaminated: usize,
}

impl OutcomeCounts {
  fn record(&mut self, status: &RunStatus) {
    let bucket = match status {
      RunStatus::SetupFailed { .. } => &mut self.setup_failed,
      RunStatus::Completed { session } => match session {
        SessionOutcome::Finished { .. } => &mut self.completed,
        SessionOutcome::MaxTurns => &mut self.max_turns,
        SessionOutcome::ReportedError { .. } => &mut self.reported_error,
        SessionOutcome::TimedOut => &mut self.timed_out,
        SessionOutcome::NoResultEvent { .. } => &mut self.no_result,
      },
    };
    *bucket += 1;
  }

  fn incomplete_kinds(&self) -> [(&'static str, usize); 3] {
    [("timed_out", self.timed_out), ("max_turns", self.max_turns), ("no_result", self.no_result)]
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionStats {
  pub runs: usize,
  pub outcomes: OutcomeCounts,
  pub estimated_costs: usize,
  pub metrics: BTreeMap<String, Stat>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskReport {
  pub task_id: String,
  pub conditions: BTreeMap<Condition, ConditionStats>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NegativeResult {
  pub task_id: String,
  pub metric: String,
  pub baseline_mean: f64,
  pub atlas_mean: f64,
  pub transcripts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContaminatedRun {
  pub run_id: String,
  pub task_id: String,
  pub condition: Condition,
  pub accesses: Vec<OutOfTreeAccess>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupFailure {
  pub run_id: String,
  pub task_id: String,
  pub condition: Condition,
  pub stage: String,
  pub kind: SetupStageKind,
  pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
  Improved,
  Regressed,
  Unchanged,
  Neutral,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delta {
  pub scope: String,
  pub condition: Condition,
  pub metric: String,
  pub previous: f64,
  pub current: f64,
  pub direction: Direction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentReport {
  pub environment: Option<Environment>,
  pub tasks: Vec<TaskReport>,
  pub paired_tasks: Vec<String>,
  pub aggregates: BTreeMap<Condition, BTreeMap<String, Stat>>,
  pub negative_results: Vec<NegativeResult>,
  pub setup_failures: Vec<SetupFailure>,
  pub contaminated_runs: Vec<ContaminatedRun>,
  pub skipped_for_budget: Vec<String>,
  pub judge_failures: Vec<String>,
  pub spent_usd: Option<f64>,
  pub comparison: Vec<Delta>,
}

pub fn load_runs(batch_dir: &Path) -> AgentResult<(Option<BatchManifest>, Vec<RunRecord>)> {
  let batch_path = batch_dir.join(BATCH_FILE);
  let batch = batch_path.is_file().then(|| read_json::<BatchManifest>(&batch_path)).transpose()?;
  let entries = fs::read_dir(batch_dir).map_err(|error| AgentError::io(batch_dir, error))?;
  let mut dirs: Vec<PathBuf> = entries
    .filter_map(Result::ok)
    .map(|entry| entry.path())
    .filter(|path| path.join(RUN_FILE).is_file())
    .collect();
  dirs.sort();
  let mut records = Vec::new();
  for dir in dirs {
    let optional = |name: &str| dir.join(name).is_file().then(|| dir.join(name));
    records.push(RunRecord {
      manifest: read_json(&dir.join(RUN_FILE))?,
      metrics: optional(METRICS_FILE).map(|path| read_json(&path)).transpose()?,
      score: optional(SCORE_FILE).map(|path| read_json::<Score>(&path)).transpose()?.map(Score::rescored),
      transcript: dir.strip_prefix(batch_dir).unwrap_or(&dir).join(TRANSCRIPT_FILE).display().to_string(),
    });
  }
  Ok((batch, records))
}

pub fn build_report(batch: Option<&BatchManifest>, records: &[RunRecord], previous: Option<&AgentReport>) -> AgentReport {
  let mut task_ids: Vec<String> = records.iter().map(|record| record.manifest.task_id.clone()).collect();
  task_ids.sort();
  task_ids.dedup();
  let tasks: Vec<TaskReport> = task_ids.iter().map(|task_id| task_report(task_id, records)).collect();
  let paired_tasks: Vec<String> = tasks.iter().filter(|task| is_paired(task)).map(|task| task.task_id.clone()).collect();
  let aggregates = aggregates(&tasks);
  let negative_results = tasks.iter().flat_map(|task| negative_results(task, records)).collect();
  let mut report = AgentReport {
    environment: batch.map(|batch| batch.environment.clone()),
    tasks,
    paired_tasks,
    aggregates,
    negative_results,
    setup_failures: setup_failures(records),
    contaminated_runs: contaminated_runs(records),
    skipped_for_budget: batch.map(|batch| batch.skipped_for_budget.clone()).unwrap_or_default(),
    judge_failures: judge_failures(records),
    spent_usd: batch.map(|batch| batch.spent_usd),
    comparison: Vec::new(),
  };
  if let Some(previous) = previous {
    report.comparison = compare(previous, &report);
  }
  report
}

fn task_report(task_id: &str, records: &[RunRecord]) -> TaskReport {
  let mut conditions = BTreeMap::new();
  for condition in Condition::ALL {
    let runs: Vec<&RunRecord> = records
      .iter()
      .filter(|record| record.manifest.task_id == task_id && record.manifest.condition == condition)
      .collect();
    if runs.is_empty() {
      continue;
    }
    let mut outcomes = OutcomeCounts::default();
    runs.iter().for_each(|record| outcomes.record(&record.manifest.status));
    outcomes.contaminated = runs.iter().filter(|record| is_contaminated(record)).count();
    let scored: Vec<&&RunRecord> = runs
      .iter()
      .filter(|record| matches!(record.manifest.status, RunStatus::Completed { .. }) && !is_contaminated(record))
      .collect();
    let metrics = METRICS
      .iter()
      .filter_map(|def| {
        let values: Vec<f64> = scored.iter().filter_map(|record| (def.extract)(record)).collect();
        Stat::of(&values).map(|stat| (def.name.to_owned(), stat))
      })
      .collect();
    conditions.insert(
      condition,
      ConditionStats {
        runs: runs.len(),
        outcomes,
        estimated_costs: runs.iter().filter(|record| record.manifest.cost_source == CostSource::EstimatedFromUsage).count(),
        metrics,
      },
    );
  }
  TaskReport {
    task_id: task_id.to_owned(),
    conditions,
  }
}

fn is_contaminated(record: &RunRecord) -> bool {
  record.metrics.as_ref().is_some_and(|metrics| metrics.contaminated)
}

fn contaminated_runs(records: &[RunRecord]) -> Vec<ContaminatedRun> {
  records
    .iter()
    .filter(|record| is_contaminated(record))
    .map(|record| ContaminatedRun {
      run_id: record.manifest.run_id.clone(),
      task_id: record.manifest.task_id.clone(),
      condition: record.manifest.condition,
      accesses: record
        .metrics
        .iter()
        .flat_map(|metrics| metrics.out_of_tree_accesses.iter().filter(|access| access.contamination.is_some()).cloned())
        .collect(),
    })
    .collect()
}

fn is_paired(task: &TaskReport) -> bool {
  Condition::ALL
    .iter()
    .all(|condition| task.conditions.get(condition).is_some_and(|stats| !stats.metrics.is_empty()))
}

fn aggregates(tasks: &[TaskReport]) -> BTreeMap<Condition, BTreeMap<String, Stat>> {
  let paired: Vec<&TaskReport> = tasks.iter().filter(|task| is_paired(task)).collect();
  let mut aggregates = BTreeMap::new();
  for condition in Condition::ALL {
    let per_metric: BTreeMap<String, Stat> = METRICS
      .iter()
      .filter_map(|def| {
        let task_means: Vec<f64> = paired
          .iter()
          .filter_map(|task| task.conditions.get(&condition)?.metrics.get(def.name))
          .map(|stat| stat.mean)
          .collect();
        Stat::of(&task_means).map(|stat| (def.name.to_owned(), stat))
      })
      .collect();
    if !per_metric.is_empty() {
      aggregates.insert(condition, per_metric);
    }
  }
  aggregates
}

fn setup_failures(records: &[RunRecord]) -> Vec<SetupFailure> {
  records
    .iter()
    .filter_map(|record| match &record.manifest.status {
      RunStatus::SetupFailed { stage, reason } => Some(SetupFailure {
        run_id: record.manifest.run_id.clone(),
        task_id: record.manifest.task_id.clone(),
        condition: record.manifest.condition,
        stage: stage.clone(),
        kind: setup_stage_kind(stage),
        reason: reason.clone(),
      }),
      RunStatus::Completed { .. } => None,
    })
    .collect()
}

fn worse(better: Better, baseline: f64, atlas: f64) -> bool {
  match better {
    Better::Higher => atlas < baseline,
    Better::Lower => atlas > baseline,
    Better::Neutral => false,
  }
}

fn negative_results(task: &TaskReport, records: &[RunRecord]) -> Vec<NegativeResult> {
  let (Some(baseline), Some(atlas)) = (task.conditions.get(&Condition::Baseline), task.conditions.get(&Condition::Atlas)) else {
    return Vec::new();
  };
  let transcripts: Vec<String> = records
    .iter()
    .filter(|record| record.manifest.task_id == task.task_id && matches!(record.manifest.status, RunStatus::Completed { .. }))
    .map(|record| record.transcript.clone())
    .collect();
  let negative = |metric: String, baseline_mean: f64, atlas_mean: f64| NegativeResult {
    task_id: task.task_id.clone(),
    metric,
    baseline_mean,
    atlas_mean,
    transcripts: transcripts.clone(),
  };
  let mut found: Vec<NegativeResult> = METRICS
    .iter()
    .filter(|def| def.group == MetricGroup::Correctness || def.name == TOKEN_METRIC)
    .filter_map(|def| {
      let (base, with_atlas) = (baseline.metrics.get(def.name)?, atlas.metrics.get(def.name)?);
      worse(def.better, base.mean, with_atlas.mean).then(|| negative(def.name.to_owned(), base.mean, with_atlas.mean))
    })
    .collect();
  let rate = |count: usize, runs: usize| count as f64 / runs.max(1) as f64;
  for ((kind, base_count), (_, atlas_count)) in baseline.outcomes.incomplete_kinds().into_iter().zip(atlas.outcomes.incomplete_kinds()) {
    let (base_rate, atlas_rate) = (rate(base_count, baseline.runs), rate(atlas_count, atlas.runs));
    if atlas_rate > base_rate {
      found.push(negative(format!("{kind}_rate"), base_rate, atlas_rate));
    }
  }
  found
}

fn compare(previous: &AgentReport, current: &AgentReport) -> Vec<Delta> {
  let mut deltas = Vec::new();
  let mut push = |scope: &str, condition: Condition, previous: &BTreeMap<String, Stat>, current: &BTreeMap<String, Stat>| {
    for def in METRICS {
      let (Some(before), Some(after)) = (previous.get(def.name), current.get(def.name)) else {
        continue;
      };
      let direction = match def.better {
        Better::Neutral => Direction::Neutral,
        _ if (before.mean - after.mean).abs() < 1e-9 => Direction::Unchanged,
        better if worse(better, before.mean, after.mean) => Direction::Regressed,
        _ => Direction::Improved,
      };
      deltas.push(Delta {
        scope: scope.to_owned(),
        condition,
        metric: def.name.to_owned(),
        previous: before.mean,
        current: after.mean,
        direction,
      });
    }
  };
  for (condition, current_metrics) in &current.aggregates {
    if let Some(previous_metrics) = previous.aggregates.get(condition) {
      push("aggregate", *condition, previous_metrics, current_metrics);
    }
  }
  for task in &current.tasks {
    let Some(previous_task) = previous.tasks.iter().find(|candidate| candidate.task_id == task.task_id) else {
      continue;
    };
    for (condition, stats) in &task.conditions {
      if let Some(previous_stats) = previous_task.conditions.get(condition) {
        push(&task.task_id, *condition, &previous_stats.metrics, &stats.metrics);
      }
    }
  }
  deltas
}

fn number(value: f64) -> String {
  if value.fract().abs() < 1e-9 {
    return format!("{value:.0}");
  }
  let text = format!("{value:.3}");
  text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn cell(stat: Option<&Stat>, expected: usize, unit: &str) -> String {
  match stat {
    None => "–".to_owned(),
    Some(stat) if stat.n == 1 && expected <= 1 => number(stat.mean),
    Some(stat) if stat.n == 1 => format!("{} ({unit}=1 of {expected})", number(stat.mean)),
    Some(stat) if stat.n < expected => format!("{} ({}–{}, {unit}={} of {expected})", number(stat.mean), number(stat.min), number(stat.max), stat.n),
    Some(stat) => format!("{} ({}–{}, {unit}={})", number(stat.mean), number(stat.min), number(stat.max), stat.n),
  }
}

fn outcome_summary(stats: Option<&ConditionStats>) -> String {
  let Some(stats) = stats else {
    return "no runs".to_owned();
  };
  let outcomes = &stats.outcomes;
  format!(
    "{} runs: completed {}, max_turns {}, reported_error {}, timed_out {}, no_result {}, setup_failed {}; contaminated {}; estimated costs {}",
    stats.runs,
    outcomes.completed,
    outcomes.max_turns,
    outcomes.reported_error,
    outcomes.timed_out,
    outcomes.no_result,
    outcomes.setup_failed,
    outcomes.contaminated,
    stats.estimated_costs
  )
}

pub fn render_markdown(report: &AgentReport) -> String {
  let mut out = String::new();
  let _ = writeln!(out, "# Agent A/B report\n");
  render_environment(&mut out, report);
  render_contamination(&mut out, report);
  let paired = report.paired_tasks.len();
  let _ = writeln!(out, "## Aggregates (mean of per-task means over {paired} paired tasks)\n");
  let _ = writeln!(
    out,
    "Only tasks with scored, uncontaminated runs in both conditions are aggregated ({} contaminated runs excluded). Times (`first_edit_ms`, `session_ms`, `wall_ms`) exclude Code Atlas indexing, which is reported separately as `atlas_index_ms`.\n",
    report.contaminated_runs.len()
  );
  for group in [MetricGroup::Correctness, MetricGroup::ReuseAndBlastRadius, MetricGroup::TokensAndContext] {
    let _ = writeln!(out, "### {}\n", group.title());
    let _ = writeln!(out, "| metric | baseline ({paired} tasks) | atlas ({paired} tasks) |");
    let _ = writeln!(out, "|---|---|---|");
    for def in METRICS.iter().filter(|def| def.group == group) {
      let baseline = report.aggregates.get(&Condition::Baseline).and_then(|metrics| metrics.get(def.name));
      let atlas = report.aggregates.get(&Condition::Atlas).and_then(|metrics| metrics.get(def.name));
      if baseline.is_none() && atlas.is_none() {
        continue;
      }
      let _ = writeln!(out, "| {} | {} | {} |", def.name, cell(baseline, paired, "tasks"), cell(atlas, paired, "tasks"));
    }
    out.push('\n');
  }
  let _ = writeln!(out, "## Per task\n");
  for task in &report.tasks {
    let _ = writeln!(out, "### {}\n", task.task_id);
    let baseline_stats = task.conditions.get(&Condition::Baseline);
    let atlas_stats = task.conditions.get(&Condition::Atlas);
    let _ = writeln!(out, "- baseline: {}", outcome_summary(baseline_stats));
    let _ = writeln!(out, "- atlas: {}\n", outcome_summary(atlas_stats));
    let _ = writeln!(out, "| metric | baseline | atlas |");
    let _ = writeln!(out, "|---|---|---|");
    for def in METRICS {
      let baseline = baseline_stats.and_then(|stats| stats.metrics.get(def.name));
      let atlas = atlas_stats.and_then(|stats| stats.metrics.get(def.name));
      if baseline.is_none() && atlas.is_none() {
        continue;
      }
      let runs = |stats: Option<&ConditionStats>| stats.map_or(0, |stats| stats.runs);
      let _ = writeln!(
        out,
        "| {} | {} | {} |",
        def.name,
        cell(baseline, runs(baseline_stats), "n"),
        cell(atlas, runs(atlas_stats), "n")
      );
    }
    out.push('\n');
  }
  let _ = writeln!(out, "## Negative results\n");
  if report.negative_results.is_empty() && report.setup_failures.is_empty() {
    let _ = writeln!(
      out,
      "None: atlas was not worse on any correctness metric or incomplete-session rate, did not use more tokens on any task, and no run failed setup.\n"
    );
  }
  for negative in &report.negative_results {
    let _ = writeln!(
      out,
      "- **{}** `{}`: baseline {} vs atlas {} — transcripts: {}",
      negative.task_id,
      negative.metric,
      number(negative.baseline_mean),
      number(negative.atlas_mean),
      negative.transcripts.join(", ")
    );
  }
  if !report.negative_results.is_empty() {
    out.push('\n');
  }
  for kind in [SetupStageKind::Product, SetupStageKind::Infrastructure] {
    let failures: Vec<&SetupFailure> = report.setup_failures.iter().filter(|failure| failure.kind == kind).collect();
    if failures.is_empty() {
      continue;
    }
    let title = match kind {
      SetupStageKind::Product => "Setup failures caused by Code Atlas (excluded from the statistics)",
      SetupStageKind::Infrastructure => "Setup failures caused by the harness or environment (excluded from the statistics)",
    };
    let _ = writeln!(out, "### {title}\n");
    for failure in failures {
      let _ = writeln!(out, "- `{}` ({}) stage `{}`: {}", failure.run_id, failure.condition, failure.stage, failure.reason);
    }
    out.push('\n');
  }
  if !report.judge_failures.is_empty() {
    let _ = writeln!(out, "## Judge failures\n");
    for failure in &report.judge_failures {
      let _ = writeln!(out, "- {failure}");
    }
    out.push('\n');
  }
  if !report.comparison.is_empty() {
    let _ = writeln!(out, "## Comparison with the previous report\n");
    let _ = writeln!(out, "| scope | condition | metric | previous | current | direction |");
    let _ = writeln!(out, "|---|---|---|---|---|---|");
    for delta in report
      .comparison
      .iter()
      .filter(|delta| matches!(delta.direction, Direction::Improved | Direction::Regressed))
    {
      let direction = match delta.direction {
        Direction::Regressed => "**regressed**",
        _ => "improved",
      };
      let _ = writeln!(
        out,
        "| {} | {} | {} | {} | {} | {direction} |",
        delta.scope,
        delta.condition,
        delta.metric,
        number(delta.previous),
        number(delta.current)
      );
    }
    out.push('\n');
  }
  out
}

fn render_contamination(out: &mut String, report: &AgentReport) {
  if report.contaminated_runs.is_empty() {
    return;
  }
  let _ = writeln!(out, "## Contaminated runs (excluded from all statistics)\n");
  let _ = writeln!(
    out,
    "These sessions reached the repository, the eval home or another run's scratch directory, so they may have seen hidden tests, ground truth or the other condition.\n"
  );
  for run in &report.contaminated_runs {
    let paths: Vec<String> = run.accesses.iter().map(|access| format!("{} `{}`", access.tool, access.path)).collect();
    let _ = writeln!(out, "- `{}` ({}): {}", run.run_id, run.condition, paths.join(", "));
  }
  out.push('\n');
}

pub fn ensure_single_preamble(batch: Option<&BatchManifest>, records: &[RunRecord]) -> AgentResult<()> {
  let mut seen: BTreeSet<(u32, PreambleVariant)> = records
    .iter()
    .filter(|record| record.manifest.condition == Condition::Atlas)
    .filter_map(|record| {
      record
        .manifest
        .preamble_version
        .map(|version| (version, record.manifest.preamble_variant.unwrap_or_default()))
    })
    .collect();
  if let Some(batch) = batch
    && !seen.is_empty()
  {
    seen.insert((batch.environment.preamble_version, batch.environment.preamble_variant));
  }
  if seen.len() <= 1 {
    return Ok(());
  }
  let listed: Vec<String> = seen.iter().map(|(version, variant)| format!("{version} ({variant})")).collect();
  Err(AgentError::MixedPreamble(listed.join(", ")))
}

fn render_environment(out: &mut String, report: &AgentReport) {
  let Some(environment) = &report.environment else {
    let _ = writeln!(out, "Environment: unknown (no batch.json).\n");
    return;
  };
  let _ = writeln!(out, "## Environment\n");
  let optional = |value: &Option<String>| value.clone().unwrap_or_else(|| "unknown".to_owned());
  let rows = [
    ("date", environment.date.clone()),
    ("seed", environment.seed.to_string()),
    ("claude CLI", optional(&environment.claude_version)),
    ("session model", environment.model.clone()),
    ("judge model", optional(&environment.judge_model)),
    ("claude config", format!("{:?}", environment.config_isolation).to_lowercase()),
    ("eval repository commit", optional(&environment.code_atlas_commit)),
    ("code-atlas binary version (build commit)", optional(&environment.code_atlas_version)),
    ("code-atlas binary blake3", optional(&environment.atlas_bin_blake3)),
    ("preamble version", environment.preamble_variant.label()),
    ("repo override", environment.repo_override.clone().unwrap_or_else(|| "none".to_owned())),
    ("spent (reported)", report.spent_usd.map_or_else(|| "unknown".to_owned(), |spent| format!("${spent:.2}"))),
  ];
  let _ = writeln!(out, "| key | value |");
  let _ = writeln!(out, "|---|---|");
  for (key, value) in rows {
    let _ = writeln!(out, "| {key} | {value} |");
  }
  out.push('\n');
  if !report.skipped_for_budget.is_empty() {
    let _ = writeln!(out, "Runs not started because the budget was exhausted: {}\n", report.skipped_for_budget.join(", "));
  }
  if let Some(probe) = &environment.probe {
    let _ = writeln!(out, "### Isolation probe\n");
    let list = |items: &[String]| if items.is_empty() { "none".to_owned() } else { items.join(", ") };
    let _ = writeln!(out, "- tools: {}", list(&probe.tools));
    let _ = writeln!(out, "- MCP servers: {}", list(&probe.mcp_servers));
    let _ = writeln!(out, "- skills: {}; slash commands: {}", list(&probe.skills), list(&probe.slash_commands));
    let _ = writeln!(out, "- plugins: {}", list(&probe.plugins));
    let _ = writeln!(out, "- user CLAUDE.md: {:?}; project CLAUDE.md: {:?}", probe.user_claude_md, probe.project_claude_md);
    let _ = writeln!(out, "- models reported: {}\n", list(&probe.model_reported));
  }
}

fn judge_failures(records: &[RunRecord]) -> Vec<String> {
  records
    .iter()
    .filter_map(|record| match record.score.as_ref().map(|score| &score.judge) {
      Some(JudgeOutcome::Failed { reason }) => Some(format!("{}: {reason}", record.manifest.run_id)),
      _ => None,
    })
    .collect()
}

pub fn write_report(report: &AgentReport, out_base: &Path) -> AgentResult<(PathBuf, PathBuf)> {
  if let Some(parent) = out_base.parent() {
    fs::create_dir_all(parent).map_err(|error| AgentError::io(parent, error))?;
  }
  let markdown = out_base.with_extension("md");
  let json = out_base.with_extension("json");
  fs::write(&markdown, render_markdown(report)).map_err(|error| AgentError::io(&markdown, error))?;
  super::record::write_json(&json, report)?;
  Ok((markdown, json))
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use super::super::claude::ConfigIsolation;
  use super::super::judge::CriterionScore;
  use super::super::metrics::{DensityMetrics, ReadMetrics, RelevanceMetrics, TimingMetrics, TokenMetrics, ToolMetrics};
  use super::super::record::{Coverage, DuplicationCheck, HiddenTests, SessionOutcome};
  use super::super::transcript::TokenUsage;
  use super::super::validation::{Regressions, SuiteResult};
  use super::super::workspace::TreeSource;
  use super::*;

  struct Sample {
    task: &'static str,
    condition: Condition,
    index: u32,
    passed: bool,
    correctness: u8,
    tokens: u64,
    files: usize,
    status: RunStatus,
  }

  fn finished() -> RunStatus {
    RunStatus::Completed {
      session: SessionOutcome::Finished { subtype: "success".to_owned() },
    }
  }

  fn record(sample: &Sample) -> RunRecord {
    let run_id = format!("{}.{}.r{}", sample.task, sample.condition, sample.index);
    let manifest = RunManifest {
      run_id: run_id.clone(),
      task_id: sample.task.to_owned(),
      condition: sample.condition,
      run_index: sample.index,
      order: 0,
      spec_hash: "h".to_owned(),
      source: TreeSource::Directory { path: PathBuf::from("/src") },
      base_sha: Some("b".to_owned()),
      tree: None,
      project_instructions: Vec::new(),
      model_requested: "claude-haiku-4-5-20251001".to_owned(),
      models_reported: vec!["claude-haiku-4-5-20251001".to_owned()],
      claude_code_version: Some("2.1.289".to_owned()),
      preamble_version: None,
      preamble_variant: None,
      args: Vec::new(),
      env_names: Vec::new(),
      atlas_project: None,
      atlas_index_ms: (sample.condition == Condition::Atlas).then_some(1200),
      wall_ms: Some(60_000),
      session_cost_usd: Some(0.5),
      cost_source: match sample.status {
        RunStatus::Completed {
          session: SessionOutcome::TimedOut,
        } => CostSource::EstimatedFromUsage,
        _ => CostSource::Reported,
      },
      cost_usd: 0.5,
      status: sample.status.clone(),
    };
    let metrics = Metrics {
      tokens: TokenMetrics {
        total: TokenUsage {
          input: sample.tokens / 10,
          output: sample.tokens / 10,
          cache_read: sample.tokens * 7 / 10,
          cache_creation: sample.tokens / 10,
        },
        cost_usd: Some(0.5),
        num_turns: Some(12),
        duration_ms: Some(50_000),
        ..TokenMetrics::default()
      },
      tools: ToolMetrics {
        total: 20,
        search_calls: 4,
        ..ToolMetrics::default()
      },
      reads: ReadMetrics {
        files: (0..sample.files).map(|index| format!("src/f{index}.ts")).collect(),
        bytes: 1000,
        lines: 50,
        ..ReadMetrics::default()
      },
      relevance: RelevanceMetrics {
        irrelevant_read: vec!["src/f0.ts".to_owned()],
        ..RelevanceMetrics::default()
      },
      timing: TimingMetrics {
        first_edit_ms: Some(20_000),
        session_ms: Some(50_000),
        wall_ms: Some(60_000),
      },
      density: DensityMetrics {
        useful_bytes: 250,
        context_bytes: 1000,
        ratio: Some(0.25),
      },
      ..Metrics::default()
    };
    let scores = super::super::judge::RUBRIC_CRITERIA
      .iter()
      .map(|criterion| {
        (
          (*criterion).to_owned(),
          CriterionScore {
            score: sample.correctness,
            rationale: "r".to_owned(),
          },
        )
      })
      .collect();
    let score = Score {
      hidden_tests: Some(HiddenTests {
        passed: sample.passed,
        commands_passed: usize::from(sample.passed),
        commands_total: 1,
        counts: None,
      }),
      validation: SuiteResult::default(),
      regression: SuiteResult::default(),
      regressions: Regressions::default(),
      regression_count: Some(0),
      must_change_coverage: Coverage {
        covered: vec!["a".to_owned()],
        missing: vec![],
        ratio: Some(1.0),
      },
      must_consider_coverage: Coverage {
        covered: vec![],
        missing: vec!["b".to_owned()],
        ratio: Some(0.0),
      },
      reuse_references: Vec::new(),
      duplication: DuplicationCheck::NotApplicable { reason: "none".to_owned() },
      duplication_introduced: None,
      new_functions_named_like_targets: Vec::new(),
      judge: JudgeOutcome::Scored {
        scores,
        model: None,
        cost_usd: None,
        prompt_hash: "p".to_owned(),
      },
      atlas_accuracy: None,
    };
    RunRecord {
      manifest,
      metrics: Some(metrics),
      score: Some(score),
      transcript: format!("{run_id}/transcript.jsonl"),
    }
  }

  fn records() -> Vec<RunRecord> {
    let sample = |condition, index, passed, correctness, tokens, files, status| Sample {
      task: "t00-smoke",
      condition,
      index,
      passed,
      correctness,
      tokens,
      files,
      status,
    };
    [
      sample(Condition::Baseline, 0, true, 4, 100_000, 6, finished()),
      sample(Condition::Baseline, 1, true, 5, 120_000, 8, finished()),
      sample(
        Condition::Atlas,
        0,
        false,
        3,
        90_000,
        3,
        RunStatus::Completed {
          session: SessionOutcome::MaxTurns,
        },
      ),
      sample(Condition::Atlas, 1, true, 5, 80_000, 2, finished()),
      sample(
        Condition::Atlas,
        2,
        true,
        5,
        1,
        1,
        RunStatus::SetupFailed {
          stage: "atlas-index".to_owned(),
          reason: "index crashed".to_owned(),
        },
      ),
      Sample {
        task: "t01-unpaired",
        ..sample(Condition::Baseline, 0, true, 5, 10, 1, finished())
      },
      Sample {
        task: "t01-unpaired",
        ..sample(
          Condition::Atlas,
          0,
          true,
          5,
          10,
          1,
          RunStatus::SetupFailed {
            stage: "materialize".to_owned(),
            reason: "disk full".to_owned(),
          },
        )
      },
    ]
    .iter()
    .map(record)
    .collect()
  }

  fn batch() -> BatchManifest {
    BatchManifest {
      environment: Environment {
        date: "2026-10-04".to_owned(),
        seed: 7,
        claude_version: Some("2.1.289 (Claude Code)".to_owned()),
        model: "claude-haiku-4-5-20251001".to_owned(),
        judge_model: Some("claude-sonnet-5-5".to_owned()),
        config_isolation: ConfigIsolation::User,
        code_atlas_commit: Some("9b1fc2d".to_owned()),
        code_atlas_version: None,
        atlas_bin_blake3: None,
        atlas_bin: "code-atlas".to_owned(),
        preamble_version: 1,
        preamble_variant: PreambleVariant::Standard,
        repo_override: None,
        probe: None,
      },
      tasks: vec!["t00-smoke".to_owned()],
      truths: Vec::new(),
      planned: Vec::new(),
      skipped_for_budget: Vec::new(),
      spent_usd: 2.0,
    }
  }

  #[test]
  fn should_aggregate_mean_min_and_max_per_condition() {
    let report = build_report(None, &records(), None);
    let baseline = &report.tasks[0].conditions[&Condition::Baseline];
    let tokens = baseline.metrics["total_tokens"];
    assert_eq!((tokens.mean, tokens.min, tokens.max, tokens.n), (110_000.0, 100_000.0, 120_000.0, 2));
    let atlas = &report.tasks[0].conditions[&Condition::Atlas];
    assert_eq!(atlas.metrics["hidden_tests_pass"].mean, 0.5);
    assert_eq!(atlas.metrics["hidden_tests_pass"].n, 2);
    assert_eq!(atlas.metrics["atlas_index_ms"].mean, 1200.0);
    assert!(!baseline.metrics.contains_key("atlas_index_ms"));
  }

  #[test]
  fn should_count_every_session_outcome_per_condition() {
    let report = build_report(None, &records(), None);
    let atlas = &report.tasks[0].conditions[&Condition::Atlas];
    assert_eq!(atlas.runs, 3);
    assert_eq!(
      atlas.outcomes,
      OutcomeCounts {
        completed: 1,
        max_turns: 1,
        setup_failed: 1,
        ..OutcomeCounts::default()
      }
    );
  }

  #[test]
  fn should_aggregate_only_tasks_scored_in_both_conditions() {
    let report = build_report(None, &records(), None);
    assert_eq!(report.paired_tasks, vec!["t00-smoke"]);
    assert_eq!(report.aggregates[&Condition::Baseline]["total_tokens"].mean, 110_000.0);
    assert_eq!(report.aggregates[&Condition::Baseline]["total_tokens"].n, 1);
  }

  #[test]
  fn should_list_contaminated_runs_and_exclude_them_from_statistics() {
    let mut runs = records();
    let mut leaked = runs[0].clone();
    leaked.manifest.run_id = "t00-smoke.baseline.r2".to_owned();
    leaked.manifest.run_index = 2;
    if let Some(metrics) = leaked.metrics.as_mut() {
      metrics.tokens.total.cache_read = 9_000_000;
      metrics.contaminated = true;
      metrics.out_of_tree_accesses = vec![
        OutOfTreeAccess {
          tool: "Read".to_owned(),
          path: "/repo/eval/tasks/t00-smoke/hidden/check.mjs".to_owned(),
          contamination: Some(super::super::access::Contamination::EvalHome),
        },
        OutOfTreeAccess {
          tool: "Bash".to_owned(),
          path: "/etc/hosts".to_owned(),
          contamination: None,
        },
      ];
    }
    runs.push(leaked);
    let report = build_report(None, &runs, None);
    let baseline = &report.tasks[0].conditions[&Condition::Baseline];
    assert_eq!(baseline.runs, 3);
    assert_eq!(baseline.outcomes.contaminated, 1);
    assert_eq!(baseline.metrics["total_tokens"].n, 2);
    assert_eq!(report.aggregates[&Condition::Baseline]["total_tokens"].mean, 110_000.0);
    assert_eq!(report.contaminated_runs.len(), 1);
    assert_eq!(report.contaminated_runs[0].run_id, "t00-smoke.baseline.r2");
    assert_eq!(report.contaminated_runs[0].accesses.len(), 1);
    let markdown = render_markdown(&report);
    assert!(markdown.contains("## Contaminated runs (excluded from all statistics)"));
    assert!(markdown.contains("- `t00-smoke.baseline.r2` (baseline): Read `/repo/eval/tasks/t00-smoke/hidden/check.mjs`"));
    assert!(markdown.contains("(1 contaminated runs excluded)"));
  }

  #[test]
  fn should_list_setup_failures_by_stage_kind() {
    let report = build_report(None, &records(), None);
    let failures: Vec<(&str, &str, SetupStageKind)> = report
      .setup_failures
      .iter()
      .map(|failure| (failure.run_id.as_str(), failure.stage.as_str(), failure.kind))
      .collect();
    assert_eq!(
      failures,
      vec![
        ("t00-smoke.atlas.r2", "atlas-index", SetupStageKind::Product),
        ("t01-unpaired.atlas.r0", "materialize", SetupStageKind::Infrastructure)
      ]
    );
  }

  #[test]
  fn should_list_correctness_regressions_as_negative_results_but_not_token_wins() {
    let report = build_report(None, &records(), None);
    let negatives: Vec<&str> = report.negative_results.iter().map(|negative| negative.metric.as_str()).collect();
    assert_eq!(negatives, vec!["hidden_tests_pass", "judge_correctness", "judge_completeness", "max_turns_rate"]);
    assert_eq!(report.negative_results[0].transcripts.len(), 4);
    assert_eq!(report.negative_results[3].atlas_mean, 1.0 / 3.0);
  }

  #[test]
  fn should_compare_against_a_previous_report() {
    let previous = build_report(None, &records()[..4], None);
    let mut improved = records();
    for record in &mut improved {
      if let Some(metrics) = record.metrics.as_mut() {
        metrics.tokens.total.cache_read /= 2;
      }
    }
    let current = build_report(None, &improved[..4], Some(&previous));
    let tokens: Vec<&Delta> = current.comparison.iter().filter(|delta| delta.metric == "total_tokens").collect();
    assert_eq!(tokens.len(), 4);
    assert!(tokens.iter().all(|delta| delta.direction == Direction::Improved));
    assert!(
      current
        .comparison
        .iter()
        .any(|delta| delta.metric == "files_read" && delta.direction == Direction::Unchanged)
    );
  }

  #[test]
  fn should_render_a_stable_markdown_report() {
    let report = build_report(Some(&batch()), &records(), None);
    insta::assert_snapshot!(render_markdown(&report));
  }

  fn atlas_with_preamble(variant: PreambleVariant) -> RunRecord {
    let mut run = records().into_iter().find(|record| record.manifest.condition == Condition::Atlas).unwrap();
    run.manifest.preamble_version = Some(variant.version());
    run.manifest.preamble_variant = Some(variant);
    run
  }

  #[test]
  fn should_render_the_directed_preamble_with_its_variant_in_the_environment_table() {
    let mut directed = batch();
    directed.environment.preamble_version = 2;
    directed.environment.preamble_variant = PreambleVariant::Directed;
    let markdown = render_markdown(&build_report(Some(&directed), &records(), None));
    assert!(markdown.contains("| preamble version | 2 (directed) |"));
    let standard = render_markdown(&build_report(Some(&batch()), &records(), None));
    assert!(standard.contains("| preamble version | 1 |"));
  }

  #[test]
  fn should_refuse_a_batch_mixing_preamble_versions() {
    let mixed = vec![atlas_with_preamble(PreambleVariant::Standard), atlas_with_preamble(PreambleVariant::Directed)];
    let error = ensure_single_preamble(None, &mixed).unwrap_err();
    assert_eq!(
      error.to_string(),
      "batch mixes atlas preamble variants (1 (standard), 2 (directed)); runs under different preambles are not comparable, rerun them under one"
    );
  }

  #[test]
  fn should_refuse_atlas_runs_whose_preamble_differs_from_the_batch_environment() {
    let runs = vec![atlas_with_preamble(PreambleVariant::Directed)];
    assert!(matches!(ensure_single_preamble(Some(&batch()), &runs), Err(AgentError::MixedPreamble(_))));
  }

  #[test]
  fn should_accept_a_batch_with_one_preamble_and_ignore_baseline_runs() {
    let runs = vec![atlas_with_preamble(PreambleVariant::Directed), atlas_with_preamble(PreambleVariant::Directed)];
    let mut directed = batch();
    directed.environment.preamble_version = 2;
    directed.environment.preamble_variant = PreambleVariant::Directed;
    assert!(ensure_single_preamble(Some(&directed), &runs).is_ok());
    assert!(ensure_single_preamble(Some(&batch()), &records()).is_ok());
  }

  #[test]
  fn should_round_trip_the_json_report() {
    let report = build_report(Some(&batch()), &records(), None);
    let text = serde_json::to_string(&report).unwrap();
    let parsed: AgentReport = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed, report);
  }
}
