use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::error::{AgentError, AgentResult};
use super::plan::{CONDITION_PAIRS, Condition, arm_legend, pair_label};
use super::process::run_captured;
use super::record::{BatchManifest, Environment, read_json, write_json_dense};
use super::report::{AgentReport, Better, DEV_SECTION, HELD_OUT_CATEGORY, METRICS, MetricDef, MetricGroup, RunRecord, is_scored, load_runs, metrics_of, number};

const CACHE_WRITE_WEIGHT: f64 = 1.25;
const CACHE_READ_WEIGHT: f64 = 0.1;
const STANDARD_ERRORS_IN_BOUND: f64 = 2.0;
const TOLERANCE: f64 = 1e-9;
const SIGNIFICANT_DIGITS: i32 = 4;
const MAX_DECIMALS: i32 = 6;
const SCRATCH_PLACEHOLDER: &str = "<scratch>";
const MACOS_PRIVATE_PREFIX: &str = "/private";
const HIDDEN_TESTS_METRIC: &str = "hidden_tests_pass";
const TOTAL_TOKENS_METRIC: &str = "total_tokens";
const TOKEN_PARTS: [&str; 4] = ["cache_read_tokens", "cache_creation_tokens", "input_tokens", "output_tokens"];
const DOMINANT_PART_SHARE: f64 = 0.5;
const HARNESS_COMMIT_FIELD: &str = "eval repository commit";
const HARNESS_DIR: &str = "crates/atlas-eval";
const GIT_TIMEOUT: Duration = Duration::from_secs(60);

const CACHE_WEIGHTED_TOKENS: MetricDef = MetricDef {
  name: "cache_weighted_tokens",
  group: MetricGroup::TokensAndContext,
  better: Better::Lower,
  extract: |record| {
    let total = metrics_of(record)?.tokens.total;
    Some(total.input as f64 + CACHE_WRITE_WEIGHT * total.cache_creation as f64 + CACHE_READ_WEIGHT * total.cache_read as f64)
  },
};

fn metric_defs() -> impl Iterator<Item = &'static MetricDef> {
  METRICS.iter().chain(std::iter::once(&CACHE_WEIGHTED_TOKENS))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairDifference {
  pub task_id: String,
  pub a: f64,
  pub b: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairSpread {
  pub pairs: usize,
  pub median_absolute: f64,
  pub median_relative: Option<f64>,
  pub min: f64,
  pub max: f64,
  pub mean_a: f64,
  pub mean_b: f64,
  pub paired_difference: f64,
  pub paired_relative: Option<f64>,
  pub per_task: Vec<PairDifference>,
}

fn mean(values: &[f64]) -> f64 {
  values.iter().sum::<f64>() / values.len().max(1) as f64
}

fn median(values: &[f64]) -> Option<f64> {
  let mut sorted = values.to_vec();
  sorted.sort_by(f64::total_cmp);
  let middle = sorted.len() / 2;
  match sorted.len() {
    0 => None,
    odd if odd % 2 == 1 => Some(sorted[middle]),
    _ => Some((sorted[middle - 1] + sorted[middle]) / 2.0),
  }
}

impl PairSpread {
  pub fn of(per_task: Vec<PairDifference>) -> Option<Self> {
    let differences: Vec<f64> = per_task.iter().map(|pair| pair.b - pair.a).collect();
    let absolute: Vec<f64> = differences.iter().map(|difference| difference.abs()).collect();
    let relative: Vec<f64> = per_task
      .iter()
      .filter(|pair| pair.a.abs() + pair.b.abs() > TOLERANCE)
      .map(|pair| (pair.b - pair.a).abs() / ((pair.a.abs() + pair.b.abs()) / 2.0))
      .collect();
    let mean_a = mean(&per_task.iter().map(|pair| pair.a).collect::<Vec<_>>());
    let mean_b = mean(&per_task.iter().map(|pair| pair.b).collect::<Vec<_>>());
    Some(Self {
      pairs: per_task.len(),
      median_absolute: median(&absolute)?,
      median_relative: median(&relative),
      min: differences.iter().copied().fold(f64::INFINITY, f64::min),
      max: differences.iter().copied().fold(f64::NEG_INFINITY, f64::max),
      mean_a,
      mean_b,
      paired_difference: mean_b - mean_a,
      paired_relative: (mean_a.abs() > TOLERANCE).then(|| (mean_b - mean_a) / mean_a),
      per_task,
    })
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
  pub label: String,
  pub scale_to_effect: f64,
  pub spread: PairSpread,
}

impl Comparison {
  fn on_effect_scale(&self) -> f64 {
    self.spread.paired_difference.abs() * self.scale_to_effect
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseClass {
  NoDifference,
  Inside,
  Outside,
  Undetermined,
}

impl NoiseClass {
  fn label(self) -> &'static str {
    match self {
      Self::NoDifference => "no difference",
      Self::Inside => "inside",
      Self::Outside => "**outside**",
      Self::Undetermined => "undetermined",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectDirection {
  SecondBetter,
  SecondWorse,
  Neutral,
}

impl EffectDirection {
  fn of(better: Better, effect: f64) -> Self {
    match better {
      Better::Neutral => Self::Neutral,
      _ if effect.abs() <= TOLERANCE => Self::Neutral,
      Better::Higher if effect > 0.0 => Self::SecondBetter,
      Better::Lower if effect < 0.0 => Self::SecondBetter,
      Better::Higher | Better::Lower => Self::SecondWorse,
    }
  }

  fn label(self, second: Condition) -> String {
    match self {
      Self::SecondBetter => format!("{} better", second.label()),
      Self::SecondWorse => format!("{} worse", second.label()),
      Self::Neutral => "no preferred direction".to_owned(),
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
  Equal,
  SecondBetter,
  SecondWorse,
  Different,
  Undetermined,
}

impl Verdict {
  fn of(class: NoiseClass, direction: EffectDirection) -> Self {
    match (class, direction) {
      (NoiseClass::NoDifference | NoiseClass::Inside, _) => Self::Equal,
      (NoiseClass::Undetermined, _) => Self::Undetermined,
      (NoiseClass::Outside, EffectDirection::SecondBetter) => Self::SecondBetter,
      (NoiseClass::Outside, EffectDirection::SecondWorse) => Self::SecondWorse,
      (NoiseClass::Outside, EffectDirection::Neutral) => Self::Different,
    }
  }

  fn label(self, second: Condition) -> String {
    match self {
      Self::Equal => "equal".to_owned(),
      Self::SecondBetter => format!("**{} better**", second.label()),
      Self::SecondWorse => format!("**{} worse**", second.label()),
      Self::Different => "**different**".to_owned(),
      Self::Undetermined => "undetermined".to_owned(),
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundTerm {
  BatchStandardError,
  PooledStandardError,
  SameConditionDifference,
}

impl BoundTerm {
  fn label(self) -> &'static str {
    match self {
      Self::BatchStandardError => "2 × SE of the batch",
      Self::PooledStandardError => "2 × pooled SE",
      Self::SameConditionDifference => "the largest A/A difference",
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaveOneOut {
  pub tasks: usize,
  pub crossing_the_bound: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReportedMeans {
  pub first: f64,
  pub second: f64,
  pub matches_recomputation: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effect {
  pub batch: String,
  pub first: Condition,
  pub second: Condition,
  pub first_mean: f64,
  pub second_mean: f64,
  pub first_tasks: usize,
  pub second_tasks: usize,
  pub paired_tasks: usize,
  pub effect: f64,
  pub relative: Option<f64>,
  pub reported: Option<ReportedMeans>,
  pub standard_error: Option<f64>,
  pub replicate_tasks: usize,
  pub pooled_standard_error: Option<f64>,
  pub effect_over_standard_error: Option<f64>,
  pub effect_over_pooled_standard_error: Option<f64>,
  pub largest_same_condition_difference: Option<f64>,
  pub noise_bound: Option<f64>,
  pub binding: Option<BoundTerm>,
  pub bound_relative_to_baseline: Option<f64>,
  pub class: NoiseClass,
  pub leave_one_out: LeaveOneOut,
  pub direction: EffectDirection,
  pub verdict: Verdict,
}

impl Effect {
  fn pair(&self) -> Pair {
    (self.first, self.second)
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricNoise {
  pub metric: String,
  pub group: MetricGroup,
  pub better: Better,
  pub repetitions: Vec<Comparison>,
  pub baseline_across_batches: Vec<Comparison>,
  pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldCheck {
  pub field: String,
  pub identical: bool,
  pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDifference {
  pub task_id: String,
  pub fields: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineVerdict {
  Identical,
  Restricted,
  NotComparable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineIdentity {
  pub batch_a: String,
  pub batch_b: String,
  pub verdict: BaselineVerdict,
  pub session_fields: Vec<FieldCheck>,
  pub harness_fields: Vec<FieldCheck>,
  pub run_fields: Vec<String>,
  pub tasks_differing: Vec<TaskDifference>,
  pub comparable_tasks: Vec<String>,
  pub harness_files_changed: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoiseBatch {
  pub name: String,
  pub date: Option<String>,
  pub preamble: Option<String>,
  pub scored_runs: usize,
  pub excluded_runs: Vec<String>,
  pub tasks: usize,
  pub reported_metrics_compared: usize,
  pub reported_metrics_differing: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoiseReport {
  pub batches: Vec<NoiseBatch>,
  pub baseline_identity: Vec<BaselineIdentity>,
  pub metrics: Vec<MetricNoise>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub held_out: Vec<String>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub sections: Vec<NoiseSection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoiseSection {
  pub name: String,
  pub tasks: Vec<String>,
  pub metrics: Vec<MetricNoise>,
}

#[derive(Clone)]
pub struct NoiseInput {
  pub name: String,
  pub manifest: Option<BatchManifest>,
  pub records: Vec<RunRecord>,
  pub reported: Option<AgentReport>,
}

type Cells = BTreeMap<(String, Condition), Vec<f64>>;
type Pair = (Condition, Condition);
type PairedTasks = BTreeMap<Pair, BTreeSet<String>>;

fn cells(def: &MetricDef, records: &[RunRecord]) -> Cells {
  let mut ordered: Vec<&RunRecord> = records.iter().filter(|record| is_scored(record)).collect();
  ordered.sort_by_key(|record| record.manifest.run_index);
  let mut cells = Cells::new();
  for record in ordered {
    if let Some(value) = (def.extract)(record) {
      cells.entry((record.manifest.task_id.clone(), record.manifest.condition)).or_default().push(value);
    }
  }
  cells
}

fn paired_tasks(records: &[RunRecord]) -> PairedTasks {
  let scored = |condition: Condition| -> BTreeSet<String> {
    records
      .iter()
      .filter(|record| is_scored(record) && record.manifest.condition == condition)
      .map(|record| record.manifest.task_id.clone())
      .collect()
  };
  CONDITION_PAIRS
    .into_iter()
    .map(|(first, second)| ((first, second), scored(first).intersection(&scored(second)).cloned().collect::<BTreeSet<String>>()))
    .filter(|(_, tasks)| !tasks.is_empty())
    .collect()
}

fn repetition_spread(cells: &Cells, condition: Condition) -> Option<PairSpread> {
  PairSpread::of(
    cells
      .iter()
      .filter(|((_, cell_condition), values)| *cell_condition == condition && values.len() >= 2)
      .map(|((task_id, _), values)| PairDifference {
        task_id: task_id.clone(),
        a: values[0],
        b: values[1],
      })
      .collect(),
  )
}

fn across_batches_spread(a: &Cells, b: &Cells, tasks: &[String]) -> Option<PairSpread> {
  PairSpread::of(
    tasks
      .iter()
      .filter_map(|task_id| {
        let key = (task_id.clone(), Condition::Baseline);
        Some(PairDifference {
          task_id: task_id.clone(),
          a: mean(a.get(&key)?),
          b: mean(b.get(&key)?),
        })
      })
      .collect(),
  )
}

fn variance_of_mean(values: &[f64]) -> Option<f64> {
  if values.len() < 2 {
    return None;
  }
  let center = mean(values);
  let sample_variance = values.iter().map(|value| (value - center).powi(2)).sum::<f64>() / (values.len() - 1) as f64;
  Some(sample_variance / values.len() as f64)
}

fn both_cells<'a>(cells: &'a Cells, paired: &BTreeSet<String>, (first, second): Pair) -> Vec<(&'a Vec<f64>, &'a Vec<f64>)> {
  let cell = |task_id: &String, condition: Condition| cells.get(&(task_id.clone(), condition));
  paired.iter().filter_map(|task_id| cell(task_id, first).zip(cell(task_id, second))).collect()
}

fn replicate_variances(cells: &Cells, paired: &BTreeSet<String>, pair: Pair) -> Vec<f64> {
  both_cells(cells, paired, pair)
    .iter()
    .filter_map(|(first, second)| Some(variance_of_mean(first)? + variance_of_mean(second)?))
    .collect()
}

fn standard_error(variances: &[f64], tasks: usize) -> Option<f64> {
  (!variances.is_empty() && tasks > 0).then(|| (mean(variances) / tasks as f64).sqrt())
}

struct EffectInput<'a> {
  batch: &'a str,
  pair: Pair,
  def: &'a MetricDef,
  cells: &'a Cells,
  paired: &'a BTreeSet<String>,
  reported: Option<&'a AgentReport>,
  pooled_variances: &'a [f64],
  largest_same_condition_difference: Option<f64>,
}

fn effect(input: &EffectInput<'_>) -> Option<Effect> {
  let (first, second) = input.pair;
  let task_means = |condition: Condition| -> Vec<f64> {
    input
      .paired
      .iter()
      .filter_map(|task_id| input.cells.get(&(task_id.clone(), condition)))
      .map(|values| mean(values))
      .collect()
  };
  let (first_means, second_means) = (task_means(first), task_means(second));
  let both = both_cells(input.cells, input.paired, input.pair);
  if both.is_empty() {
    return None;
  }
  let paired_first = mean(&both.iter().map(|(first, _)| mean(first)).collect::<Vec<_>>());
  let effect = mean(&both.iter().map(|(first, second)| mean(second) - mean(first)).collect::<Vec<_>>());
  let variances = replicate_variances(input.cells, input.paired, input.pair);
  let own = standard_error(&variances, both.len());
  let pooled = standard_error(input.pooled_variances, both.len());
  let terms = [
    (BoundTerm::BatchStandardError, own.map(|error| STANDARD_ERRORS_IN_BOUND * error)),
    (BoundTerm::PooledStandardError, pooled.map(|error| STANDARD_ERRORS_IN_BOUND * error)),
    (BoundTerm::SameConditionDifference, input.largest_same_condition_difference),
  ];
  let largest_term = terms
    .into_iter()
    .filter_map(|(term, bound)| bound.map(|bound| (term, bound)))
    .reduce(|largest, next| if next.1 > largest.1 { next } else { largest });
  let noise_bound = largest_term.map(|(_, bound)| bound);
  let in_errors = |error: Option<f64>| error.filter(|error| *error > TOLERANCE).map(|error| effect / error);
  let class = match noise_bound {
    _ if effect.abs() <= TOLERANCE => NoiseClass::NoDifference,
    None => NoiseClass::Undetermined,
    Some(bound) if effect.abs() <= bound + TOLERANCE => NoiseClass::Inside,
    Some(_) => NoiseClass::Outside,
  };
  let (first_mean, second_mean) = (mean(&first_means), mean(&second_means));
  let reported = input.reported.and_then(|report| {
    let stat = |condition: Condition| report.aggregates.get(&condition)?.get(input.def.name).map(|stat| stat.mean);
    let (first, second) = match report.pairwise.is_empty() {
      true => (stat(first)?, stat(second)?),
      false => report
        .pairwise
        .iter()
        .find(|delta| (delta.first, delta.second) == (first, second) && delta.metric == input.def.name)
        .map(|delta| (delta.first_mean, delta.second_mean))?,
    };
    Some(ReportedMeans {
      first,
      second,
      matches_recomputation: (first - first_mean).abs() <= TOLERANCE && (second - second_mean).abs() <= TOLERANCE,
    })
  });
  let against_first = |value: f64| (paired_first.abs() > TOLERANCE).then(|| value / paired_first.abs());
  let direction = EffectDirection::of(input.def.better, effect);
  Some(Effect {
    batch: input.batch.to_owned(),
    first,
    second,
    first_mean,
    second_mean,
    first_tasks: first_means.len(),
    second_tasks: second_means.len(),
    paired_tasks: both.len(),
    effect,
    relative: against_first(effect),
    reported,
    standard_error: own,
    replicate_tasks: variances.len(),
    pooled_standard_error: pooled,
    effect_over_standard_error: in_errors(own),
    effect_over_pooled_standard_error: in_errors(pooled),
    largest_same_condition_difference: input.largest_same_condition_difference,
    noise_bound,
    binding: largest_term.map(|(term, _)| term),
    bound_relative_to_baseline: noise_bound.and_then(against_first),
    class,
    leave_one_out: LeaveOneOut::default(),
    direction,
    verdict: Verdict::of(class, direction),
  })
}

fn rendered(value: &impl Serialize) -> String {
  match serde_json::to_value(value) {
    Ok(serde_json::Value::String(text)) => text,
    Ok(serde_json::Value::Null) => "none".to_owned(),
    Ok(other) => other.to_string(),
    Err(_) => "unreadable".to_owned(),
  }
}

fn field<T: Serialize + PartialEq>(name: &str, a: &T, b: &T) -> FieldCheck {
  FieldCheck {
    field: name.to_owned(),
    identical: a == b,
    values: vec![rendered(a), rendered(b)],
  }
}

fn session_fields(a: &Environment, b: &Environment) -> Vec<FieldCheck> {
  let mut fields = vec![
    field("session model", &a.model, &b.model),
    field("claude CLI", &a.claude_version, &b.claude_version),
    field("claude config", &a.config_isolation, &b.config_isolation),
    field("judge model", &a.judge_model, &b.judge_model),
    field("repo override in use", &a.repo_override.is_some(), &b.repo_override.is_some()),
    field("probe recorded", &a.probe.is_some(), &b.probe.is_some()),
  ];
  if let (Some(a), Some(b)) = (&a.probe, &b.probe) {
    fields.extend([
      field("probe: Claude Code version", &a.claude_code_version, &b.claude_code_version),
      field("probe: models reported", &a.model_reported, &b.model_reported),
      field("probe: permission mode", &a.permission_mode, &b.permission_mode),
      field("probe: tools", &a.tools, &b.tools),
      field("probe: MCP servers", &a.mcp_servers, &b.mcp_servers),
      field("probe: skills", &a.skills, &b.skills),
      field("probe: slash commands", &a.slash_commands, &b.slash_commands),
      field("probe: plugins", &a.plugins, &b.plugins),
      field("probe: agents", &a.agents, &b.agents),
      field("probe: project CLAUDE.md seen", &a.project_claude_md, &b.project_claude_md),
      field("probe: user CLAUDE.md seen", &a.user_claude_md, &b.user_claude_md),
    ]);
  }
  fields
}

fn harness_fields(a: &Environment, b: &Environment) -> Vec<FieldCheck> {
  vec![
    field("date recorded", &a.date, &b.date),
    field("seed", &a.seed, &b.seed),
    field(HARNESS_COMMIT_FIELD, &a.code_atlas_commit, &b.code_atlas_commit),
  ]
}

fn normalized_args(record: &RunRecord) -> Vec<String> {
  let scratch = record
    .manifest
    .tree
    .as_deref()
    .and_then(|tree| Path::new(tree).parent())
    .map(|scratch| scratch.display().to_string());
  let Some(scratch) = scratch.filter(|scratch| !scratch.is_empty()) else {
    return record.manifest.args.clone();
  };
  let private = format!("{MACOS_PRIVATE_PREFIX}{scratch}");
  record
    .manifest
    .args
    .iter()
    .map(|arg| arg.replace(&private, SCRATCH_PLACEHOLDER).replace(&scratch, SCRATCH_PLACEHOLDER))
    .collect()
}

type RunField = (&'static str, fn(&RunRecord) -> String);

const RUN_FIELDS: [RunField; 10] = [
  ("task spec hash (prompt, ground truth, validation, base patch)", |record| record.manifest.spec_hash.clone()),
  ("tree source (repository, commit, subdirectory)", |record| rendered(&record.manifest.source)),
  ("base commit of the tree", |record| rendered(&record.manifest.base_sha)),
  ("injected project instructions", |record| rendered(&record.manifest.project_instructions)),
  ("model requested", |record| record.manifest.model_requested.clone()),
  ("models reported", |record| rendered(&record.manifest.models_reported)),
  ("Claude Code version", |record| rendered(&record.manifest.claude_code_version)),
  ("command line with the scratch directory masked (flags, system prompt, prompt)", |record| {
    rendered(&normalized_args(record))
  }),
  ("environment variable names", |record| rendered(&record.manifest.env_names)),
  ("preamble", |record| rendered(&(record.manifest.preamble_version, record.manifest.preamble_variant))),
];

fn baseline_runs<'a>(input: &'a NoiseInput, task_id: &str) -> Vec<&'a RunRecord> {
  input
    .records
    .iter()
    .filter(|record| is_scored(record) && record.manifest.condition == Condition::Baseline && record.manifest.task_id == task_id)
    .collect()
}

fn baseline_identity(a: &NoiseInput, b: &NoiseInput) -> BaselineIdentity {
  let environments = a
    .manifest
    .as_ref()
    .map(|manifest| &manifest.environment)
    .zip(b.manifest.as_ref().map(|manifest| &manifest.environment));
  let (session, harness) = match environments {
    Some((a, b)) => (session_fields(a, b), harness_fields(a, b)),
    None => (
      vec![FieldCheck {
        field: "batch.json present in both batches".to_owned(),
        identical: false,
        values: vec![rendered(&a.manifest.is_some()), rendered(&b.manifest.is_some())],
      }],
      Vec::new(),
    ),
  };
  let task_ids: BTreeSet<&String> = a.records.iter().chain(&b.records).map(|record| &record.manifest.task_id).collect();
  let mut tasks_differing = Vec::new();
  let mut comparable_tasks = Vec::new();
  for task_id in task_ids {
    let (runs_a, runs_b) = (baseline_runs(a, task_id), baseline_runs(b, task_id));
    let runs: Vec<&RunRecord> = runs_a.iter().chain(&runs_b).copied().collect();
    let mut fields: Vec<String> = RUN_FIELDS
      .iter()
      .filter(|(_, extract)| runs.iter().map(|record| extract(record)).collect::<BTreeSet<_>>().len() > 1)
      .map(|(name, _)| (*name).to_owned())
      .collect();
    if runs_a.is_empty() || runs_b.is_empty() {
      fields.push("no scored baseline run in one of the batches".to_owned());
    }
    match fields.is_empty() {
      true => comparable_tasks.push(task_id.clone()),
      false => tasks_differing.push(TaskDifference { task_id: task_id.clone(), fields }),
    }
  }
  let verdict = match (session.iter().all(|check| check.identical), tasks_differing.is_empty()) {
    (false, _) => BaselineVerdict::NotComparable,
    (true, true) => BaselineVerdict::Identical,
    (true, false) => BaselineVerdict::Restricted,
  };
  if verdict == BaselineVerdict::NotComparable {
    comparable_tasks.clear();
  }
  BaselineIdentity {
    batch_a: a.name.clone(),
    batch_b: b.name.clone(),
    verdict,
    session_fields: session,
    harness_fields: harness,
    run_fields: RUN_FIELDS.iter().map(|(name, _)| (*name).to_owned()).collect(),
    tasks_differing,
    comparable_tasks,
    harness_files_changed: None,
  }
}

struct MetricCore {
  repetitions: Vec<Comparison>,
  baseline_across_batches: Vec<Comparison>,
  effects: Vec<Effect>,
}

struct Measured<'a> {
  inputs: &'a [NoiseInput],
  identities: &'a [(usize, usize, BaselineIdentity)],
  paired: &'a [PairedTasks],
}

fn single_run_scale(cells: &Cells) -> f64 {
  let inverse_runs: Vec<f64> = cells.values().filter(|values| !values.is_empty()).map(|values| 1.0 / values.len() as f64).collect();
  if inverse_runs.is_empty() { 1.0 } else { mean(&inverse_runs).sqrt() }
}

fn metric_core(def: &MetricDef, measured: &Measured<'_>, per_batch: &[Cells], excluded: Option<&str>) -> MetricCore {
  let kept = |task_id: &String| excluded != Some(task_id.as_str());
  let per_batch: Vec<Cells> = per_batch
    .iter()
    .map(|cells| {
      cells
        .iter()
        .filter(|((task_id, _), _)| kept(task_id))
        .map(|(key, values)| (key.clone(), values.clone()))
        .collect()
    })
    .collect();
  let paired: Vec<PairedTasks> = measured
    .paired
    .iter()
    .map(|pairs| {
      pairs
        .iter()
        .map(|(pair, tasks)| (*pair, tasks.iter().filter(|task_id| kept(task_id)).cloned().collect()))
        .collect()
    })
    .collect();
  let mut repetitions = Vec::new();
  for (input, cells) in measured.inputs.iter().zip(&per_batch) {
    for condition in Condition::EVERY {
      if let Some(spread) = repetition_spread(cells, condition) {
        repetitions.push(Comparison {
          label: format!("{condition}, second run − first run, batch {}", input.name),
          scale_to_effect: single_run_scale(cells),
          spread,
        });
      }
    }
  }
  let baseline_across_batches: Vec<Comparison> = measured
    .identities
    .iter()
    .filter_map(|(a, b, identity)| {
      Some(Comparison {
        label: format!("baseline, batch {} − batch {} (per-task means)", identity.batch_b, identity.batch_a),
        scale_to_effect: 1.0,
        spread: across_batches_spread(&per_batch[*a], &per_batch[*b], &identity.comparable_tasks)?,
      })
    })
    .collect();
  let largest = repetitions.iter().chain(&baseline_across_batches).map(Comparison::on_effect_scale).reduce(f64::max);
  let mut effects = Vec::new();
  for (index, input) in measured.inputs.iter().enumerate() {
    for pair in CONDITION_PAIRS {
      let Some(tasks) = paired[index].get(&pair) else {
        continue;
      };
      let pooled_variances: Vec<f64> = per_batch
        .iter()
        .zip(&paired)
        .filter_map(|(cells, pairs)| pairs.get(&pair).map(|tasks| replicate_variances(cells, tasks, pair)))
        .flatten()
        .collect();
      effects.extend(effect(&EffectInput {
        batch: &input.name,
        pair,
        def,
        cells: &per_batch[index],
        paired: tasks,
        reported: input.reported.as_ref(),
        pooled_variances: &pooled_variances,
        largest_same_condition_difference: largest,
      }));
    }
  }
  MetricCore {
    repetitions,
    baseline_across_batches,
    effects,
  }
}

fn leave_one_out(def: &MetricDef, measured: &Measured<'_>, per_batch: &[Cells], effects: &mut [Effect]) {
  let tasks: BTreeSet<&String> = measured.paired.iter().flat_map(|pairs| pairs.values().flatten()).collect();
  for task_id in tasks {
    let without = metric_core(def, measured, per_batch, Some(task_id));
    for effect in effects.iter_mut() {
      let pair = effect.pair();
      let measures = |condition: Condition| {
        let batch = measured.inputs.iter().position(|input| input.name == effect.batch);
        batch.is_some_and(|index| measured.paired[index].get(&pair).is_some_and(|tasks| tasks.contains(task_id)) && per_batch[index].contains_key(&(task_id.clone(), condition)))
      };
      if ![pair.0, pair.1].into_iter().all(measures) {
        continue;
      }
      let outside_without = without
        .effects
        .iter()
        .any(|other| other.batch == effect.batch && other.pair() == pair && other.class == NoiseClass::Outside);
      effect.leave_one_out.tasks += 1;
      if outside_without != (effect.class == NoiseClass::Outside) {
        effect.leave_one_out.crossing_the_bound.push(task_id.clone());
      }
    }
  }
}

pub fn build_noise(inputs: &[NoiseInput]) -> NoiseReport {
  let identities: Vec<(usize, usize, BaselineIdentity)> = (0..inputs.len())
    .flat_map(|a| (a + 1..inputs.len()).map(move |b| (a, b)))
    .map(|(a, b)| (a, b, baseline_identity(&inputs[a], &inputs[b])))
    .collect();
  let paired: Vec<PairedTasks> = inputs.iter().map(|input| paired_tasks(&input.records)).collect();
  let mut batches: Vec<NoiseBatch> = inputs
    .iter()
    .map(|input| NoiseBatch {
      name: input.name.clone(),
      date: input.manifest.as_ref().map(|manifest| manifest.environment.date.clone()),
      preamble: input.manifest.as_ref().map(|manifest| manifest.environment.preamble_variant.label()),
      scored_runs: input.records.iter().filter(|record| is_scored(record)).count(),
      excluded_runs: input
        .records
        .iter()
        .filter(|record| !is_scored(record))
        .map(|record| record.manifest.run_id.clone())
        .collect(),
      tasks: input.records.iter().map(|record| &record.manifest.task_id).collect::<BTreeSet<_>>().len(),
      reported_metrics_compared: 0,
      reported_metrics_differing: Vec::new(),
    })
    .collect();
  let measured = Measured {
    inputs,
    identities: &identities,
    paired: &paired,
  };
  let mut metrics = Vec::new();
  for def in metric_defs() {
    let per_batch: Vec<Cells> = inputs.iter().map(|input| cells(def, &input.records)).collect();
    let mut core = metric_core(def, &measured, &per_batch, None);
    leave_one_out(def, &measured, &per_batch, &mut core.effects);
    for batch in batches.iter_mut() {
      for reported in core.effects.iter().filter(|effect| effect.batch == batch.name).filter_map(|effect| effect.reported) {
        batch.reported_metrics_compared += 1;
        if !reported.matches_recomputation && !batch.reported_metrics_differing.iter().any(|metric| metric == def.name) {
          batch.reported_metrics_differing.push(def.name.to_owned());
        }
      }
    }
    if core.repetitions.is_empty() && core.effects.is_empty() {
      continue;
    }
    metrics.push(MetricNoise {
      metric: def.name.to_owned(),
      group: def.group,
      better: def.better,
      repetitions: core.repetitions,
      baseline_across_batches: core.baseline_across_batches,
      effects: core.effects,
    });
  }
  NoiseReport {
    batches,
    baseline_identity: identities.into_iter().map(|(_, _, identity)| identity).collect(),
    metrics,
    held_out: Vec::new(),
    sections: Vec::new(),
  }
}

pub fn build_split_noise(inputs: &[NoiseInput], held_out: &[String]) -> NoiseReport {
  let mut report = build_noise(inputs);
  let held: BTreeSet<String> = inputs
    .iter()
    .filter_map(|input| input.manifest.as_ref())
    .flat_map(|manifest| manifest.held_out.iter().cloned())
    .chain(held_out.iter().cloned())
    .collect();
  if held.is_empty() {
    return report;
  }
  report.held_out = held.iter().cloned().collect();
  for (name, keep_held) in [(DEV_SECTION, false), (HELD_OUT_CATEGORY, true)] {
    let filtered: Vec<NoiseInput> = inputs
      .iter()
      .map(|input| NoiseInput {
        records: input
          .records
          .iter()
          .filter(|record| held.contains(&record.manifest.task_id) == keep_held)
          .cloned()
          .collect(),
        reported: None,
        ..input.clone()
      })
      .collect();
    let tasks: BTreeSet<String> = filtered
      .iter()
      .flat_map(|input| input.records.iter().map(|record| record.manifest.task_id.clone()))
      .collect();
    report.sections.push(NoiseSection {
      name: name.to_owned(),
      tasks: tasks.into_iter().collect(),
      metrics: build_noise(&filtered).metrics,
    });
  }
  report
}

fn harness_files_changed(repo_root: &Path, identity: &BaselineIdentity) -> Option<Vec<String>> {
  let commits = identity.harness_fields.iter().find(|check| check.field == HARNESS_COMMIT_FIELD && !check.identical)?;
  let (from, to) = (commits.values.first()?, commits.values.get(1)?);
  let mut command = Command::new("git");
  command.current_dir(repo_root).args(["diff", "--name-only", from, to, "--", HARNESS_DIR]);
  let output = run_captured(command, None, GIT_TIMEOUT);
  output.exit.succeeded().then(|| output.stdout.lines().map(str::to_owned).collect())
}

pub fn load_noise(batch_dirs: &[PathBuf], report_paths: &[PathBuf], repo_root: Option<&Path>, held_out: &[String]) -> AgentResult<NoiseReport> {
  let mut reports: Vec<(&PathBuf, AgentReport, bool)> = Vec::new();
  for path in report_paths {
    reports.push((path, read_json(path)?, false));
  }
  let mut inputs = Vec::new();
  for batch_dir in batch_dirs {
    let (manifest, records) = load_runs(batch_dir)?;
    let environment = manifest.as_ref().map(|manifest| &manifest.environment);
    let matching = reports
      .iter_mut()
      .find(|(_, report, _)| environment.is_some() && report.environment.as_ref() == environment);
    let reported = matching.map(|(_, report, used)| {
      *used = true;
      report.clone()
    });
    inputs.push(NoiseInput {
      name: batch_dir.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned(),
      manifest,
      records,
      reported,
    });
  }
  if let Some((path, ..)) = reports.iter().find(|(.., used)| !used) {
    return Err(AgentError::InvalidArgument(format!(
      "{} was not produced from any of the given batches (its environment matches none of their batch.json files)",
      path.display()
    )));
  }
  let mut report = build_split_noise(&inputs, held_out);
  for identity in &mut report.baseline_identity {
    identity.harness_files_changed = repo_root.and_then(|root| harness_files_changed(root, identity));
  }
  Ok(report)
}

fn figure(value: f64) -> String {
  if value.abs() <= TOLERANCE {
    return "0".to_owned();
  }
  let integer_digits = value.abs().log10().floor() as i32 + 1;
  let decimals = (SIGNIFICANT_DIGITS - integer_digits).clamp(0, MAX_DECIMALS) as usize;
  let text = format!("{value:.decimals$}");
  if text.contains('.') {
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
  } else {
    text
  }
}

fn percent(value: f64) -> String {
  format!("{}%", number((value * 1000.0).round() / 10.0))
}

fn signed(value: f64) -> String {
  if value > TOLERANCE { format!("+{}", figure(value)) } else { figure(value) }
}

fn signed_percent(value: f64) -> String {
  if value > TOLERANCE { format!("+{}", percent(value)) } else { percent(value) }
}

fn optional(value: Option<f64>, show: fn(f64) -> String) -> String {
  value.map_or_else(|| "–".to_owned(), show)
}

const DEFINITIONS: &str = "\
- **Runs.** Completed, uncontaminated runs, the filter `agent report` applies. Metrics are the ones `agent report` aggregates, read through the same extraction code, plus `cache_weighted_tokens` = input + 1.25 × cache-creation + 0.1 × cache-read tokens (output tokens are not part of it).
- **A/A difference.** A difference between two measurements of the same task under the same condition. Two kinds are computed. *Repetitions*: the second run minus the first run of a task within one batch and condition (one run on each side). *Across batches*: the baseline per-task mean of one batch minus that of the other (two runs on each side), only for tasks whose baseline is identical in both batches. Run indices and batch order carry no meaning, so only the size of an A/A difference matters, not its sign.
- **Per comparison.** `pairs`: tasks with a value on both sides. `median |diff|`: the median absolute per-task difference. `median relative`: the median of |b − a| / ((|a| + |b|) / 2) over the tasks where that is defined. `range`: the smallest and largest signed per-task difference. `paired difference`: mean over the paired tasks of side b minus mean over the same tasks of side a, the statistic `agent report` uses for A/B (difference of the means of per-task means), with its size relative to side a.
- **A/B effect.** For every pair of arms a batch holds (C vs B = atlas − Serena + graphify, C vs A = atlas − baseline, B vs A = Serena + graphify − baseline), the second arm minus the first, as the mean over the tasks that have the metric in both arms of (second per-task mean − first per-task mean); `relative` divides it by the first arm's mean of those same tasks. The two arm columns are the aggregate means `agent report` prints. When both arms cover the same tasks, the effect equals their difference; where they do not, the `tasks` column gives the three counts.
- **Replicate standard error (SE).** For each task, the variance of each condition's per-task mean is estimated from its repetitions (sample variance / runs; with two runs that is (r1 − r0)² / 4). SE = √(mean over tasks of (first-arm variance + second-arm variance) / tasks). It uses only differences between repetitions of one condition. `this batch` takes the variances from the batch whose effect is judged, `pooled` from all batches given.
- **One scale.** A repetition difference compares one run with one run, an A/B effect compares the mean of the runs of each cell. With two runs per cell the first has √2 times the standard deviation of the second, so every repetition difference is multiplied by √(mean over the cells of 1 / runs) before it is compared with an effect. The across-batch baseline difference compares cell means and needs no scaling.
- **Noise bound.** The largest of 2 × SE of the batch, 2 × pooled SE, and the largest paired A/A difference on the A/B scale observed for the metric in any comparison (repetitions of either condition in any batch, and baseline across batches). The third term is a guard: where repetitions tie often, as judged 1 to 5 scores do, the SE can come out too small, and a difference that large between two runs of one condition was nevertheless observed. An effect is called outside only if it exceeds every one of the three.
- **Effect / SE.** The effect divided by the SE of the batch, and by the pooled SE, printed beside every class so that a label can be checked against the usual 2.
- **Leave one task out.** The whole classification of a metric is repeated with each task removed in turn (its runs leave the effect, both SEs and every A/A difference). The last column counts the tasks whose removal moves the effect across the bound, from outside to not outside or the reverse; the JSON names them.
- **Classes.** `no difference`: the effect is exactly 0. `inside`: |effect| ≤ noise bound. `outside`: |effect| > noise bound. `undetermined`: no repetition exists to estimate noise from. In the pairwise verdicts `no difference` and `inside` both read `equal`; an `outside` effect reads as the second arm better or worse, or `different` for a metric without a preferred direction.";

const SCOPE: &str = "\
This is the noise floor of the task set that exists: 11 tasks written by the authors of Code Atlas, on which the main outcome sits at its ceiling. It is not the A/A run on a harder task set that the roadmap asks for before any arm is compared; that needs a task set which does not exist yet. A harder, more varied task set will have a different and probably larger variance, so none of the bounds below transfer to it.";

const LIMITS: &str = "\
- **Two repetitions per cell.** Each task and condition has two runs, so each per-task variance rests on a single difference. The SE pools those differences over the tasks and both conditions; it is a rough scale, not a confidence interval, and no significance is claimed for any effect.
- **The bound is not a test.** About one pure-noise effect in twenty exceeds 2 × SE, and every batch classifies a few dozen metrics, many of them correlated (tokens, cost, turns and tool calls move together). The SE of one metric can differ by a factor of two between batches, which shows how uncertain the noise scale itself is. An `outside` label is a lead to replicate, not a result, and one that a single task can remove is weaker still.
- **Borderline labels.** A class is a threshold on a noisy scale. The lists above name every `outside` effect that one task can turn, and every `inside` effect that exceeds 2 × SE of its batch together with the term that keeps it inside.
- **Inside does not mean no effect.** It means the design cannot tell the effect from run-to-run variation. The column `bound / baseline` says how large an effect would have to be to leave the noise with this task set and two runs per cell; the SE shrinks with the square root of the number of tasks and of the repetitions.
- **Across batches is more than repetition.** Batches run at different times and may come from different harness commits (see the table above). The across-batch baseline difference therefore includes drift between batches (machine load, service latency) that a within-batch A/B comparison does not contain. It is listed as an A/A difference because the baseline sessions were configured identically.
- **Metrics that only one condition can move** (`atlas_calls`, `atlas_bytes`, `serena_calls`, `serena_bytes`, `graphify_calls`, `graphify_bytes`) differ by construction, so their class says nothing about noise, and `atlas_index_ms` and `graph_build_ms` have a value in one arm only.
- **Judged scores** come from one judge call per run on a 1 to 5 scale; runs whose judge call failed have no score and drop out of the pairs.";

fn render_identity(out: &mut String, identity: &BaselineIdentity) {
  let _ = writeln!(out, "### {} and {}\n", identity.batch_a, identity.batch_b);
  let verdict = match identity.verdict {
    BaselineVerdict::Identical => format!(
      "**Identical.** Every field that defines a baseline session is the same in both batches for all {} tasks, so the two baselines are compared task by task.",
      identity.comparable_tasks.len()
    ),
    BaselineVerdict::Restricted => format!(
      "**Identical for {} tasks only.** The across-batch comparison is restricted to them; the tasks below differ.",
      identity.comparable_tasks.len()
    ),
    BaselineVerdict::NotComparable => {
      "**Not comparable.** A field that defines the baseline session differs between the batches, so no across-batch comparison is made.".to_owned()
    }
  };
  let _ = writeln!(out, "{verdict}\n");
  let _ = writeln!(out, "| compared | {} | {} | |", identity.batch_a, identity.batch_b);
  let _ = writeln!(out, "|---|---|---|---|");
  for check in identity.session_fields.iter().chain(&identity.harness_fields) {
    let values = |index: usize| check.values.get(index).map_or("–", String::as_str);
    let _ = writeln!(
      out,
      "| {} | {} | {} | {} |",
      check.field,
      values(0),
      values(1),
      if check.identical { "same" } else { "**differs**" }
    );
  }
  out.push('\n');
  let _ = writeln!(
    out,
    "Per task, over every scored baseline run of both batches, these recorded fields were compared: {}.\n",
    identity.run_fields.join("; ")
  );
  for task in &identity.tasks_differing {
    let _ = writeln!(out, "- `{}` differs in: {}", task.task_id, task.fields.join("; "));
  }
  if !identity.tasks_differing.is_empty() {
    out.push('\n');
  }
  let differing: Vec<&str> = identity.harness_fields.iter().filter(|check| !check.identical).map(|check| check.field.as_str()).collect();
  if differing.is_empty() {
    return;
  }
  let changed = match &identity.harness_files_changed {
    Some(files) if files.is_empty() => format!("`git diff` between the two commits lists no changed file under `{HARNESS_DIR}`."),
    Some(files) => format!(
      "`git diff --name-only` between the two commits lists these files under `{HARNESS_DIR}`: {}. Metrics the harness derives (reads, judged scores) are comparable across the batches only as far as those files leave the scoring unchanged.",
      files
        .iter()
        .map(|file| format!("`{}`", file.trim_start_matches(HARNESS_DIR).trim_start_matches('/')))
        .collect::<Vec<_>>()
        .join(", ")
    ),
    None => "The two commits could not be compared here (no repository, or a commit it does not have), so whether the scoring code changed between them is unknown.".to_owned(),
  };
  let _ = writeln!(
    out,
    "Differences that do not define the baseline session but do describe the harness that ran and scored it: {}. {changed}\n",
    differing.join(", ")
  );
}

fn ratio(value: Option<f64>) -> String {
  value.map_or_else(|| "–".to_owned(), |value| format!("{value:+.2}"))
}

fn leave_one_out_cell(effect: &Effect) -> String {
  match effect.leave_one_out.crossing_the_bound.len() {
    0 => format!("0 of {}", effect.leave_one_out.tasks),
    crossing => format!("**{crossing} of {}**", effect.leave_one_out.tasks),
  }
}

fn token_composition(report: &NoiseReport, batch: &NoiseBatch) -> Option<String> {
  let baseline_mean = |name: &str| {
    let metric = report.metrics.iter().find(|metric| metric.metric == name)?;
    metric
      .effects
      .iter()
      .find(|effect| effect.batch == batch.name && effect.first == Condition::Baseline)
      .map(|effect| effect.first_mean)
  };
  let total = baseline_mean(TOTAL_TOKENS_METRIC).filter(|total| *total > TOLERANCE)?;
  let (part, share) = TOKEN_PARTS
    .iter()
    .filter_map(|part| Some((*part, baseline_mean(part)? / total)))
    .find(|(_, share)| *share > DOMINANT_PART_SHARE)?;
  Some(format!(
    "`{part}` is {} of `{TOTAL_TOKENS_METRIC}` in the baseline of this batch, so the two rows are one signal, not two.",
    percent(share)
  ))
}

fn batch_pairs(report: &NoiseReport, batch: &NoiseBatch) -> Vec<Pair> {
  CONDITION_PAIRS
    .into_iter()
    .filter(|pair| {
      report
        .metrics
        .iter()
        .any(|metric| metric.effects.iter().any(|effect| effect.batch == batch.name && effect.pair() == *pair))
    })
    .collect()
}

fn render_effects(out: &mut String, report: &NoiseReport, batch: &NoiseBatch) {
  let pairs = batch_pairs(report, batch);
  let _ = writeln!(out, "### Batch {} (preamble {})\n", batch.name, batch.preamble.as_deref().unwrap_or("unknown"));
  if let Some(note) = token_composition(report, batch) {
    let _ = writeln!(out, "{note}\n");
  }
  match batch.reported_metrics_compared {
    0 => {
      let _ = writeln!(out, "No committed A/B report was given for this batch; the means are recomputed from the run records.\n");
    }
    compared => {
      let _ = writeln!(
        out,
        "The committed A/B report of this batch has {compared} of these effects; {} metrics differ from the recomputation{}.\n",
        batch.reported_metrics_differing.len(),
        if batch.reported_metrics_differing.is_empty() {
          String::new()
        } else {
          format!(": {}", batch.reported_metrics_differing.join(", "))
        }
      );
    }
  }
  for pair in pairs.iter().copied() {
    if pairs.len() > 1 {
      let _ = writeln!(out, "#### {}: {} − {}\n", pair_label(pair.0, pair.1), pair.1.label(), pair.0.label());
    }
    render_pair_effects(out, report, batch, pair);
  }
}

fn render_verdicts(out: &mut String, report: &NoiseReport) {
  let mut present: Vec<Condition> = report
    .metrics
    .iter()
    .flat_map(|metric| metric.effects.iter().flat_map(|effect| [effect.first, effect.second]))
    .collect::<BTreeSet<_>>()
    .into_iter()
    .collect();
  present.sort_by_key(|condition| condition.arm());
  let verdict = match present.contains(&Condition::Atlas) && present.contains(&Condition::Incumbent) {
    true => " C vs B is the verdict.",
    false => "",
  };
  let _ = writeln!(
    out,
    "Every effect between two arms ({}), over the tasks scored in both, next to the same-condition spread (the largest A/A difference on the A/B scale) and the noise bound. An effect inside the bound is `equal`.{verdict}\n",
    arm_legend(&present)
  );
  match report.sections.is_empty() {
    true => render_verdict_tables(out, &report.batches, &report.metrics, "###"),
    false => {
      for section in &report.sections {
        let _ = writeln!(out, "### {} tasks ({})\n", section.name, section.tasks.join(", "));
        render_verdict_tables(out, &report.batches, &section.metrics, "####");
      }
    }
  }
}

fn render_verdict_tables(out: &mut String, batches: &[NoiseBatch], metrics: &[MetricNoise], heading: &str) {
  for batch in batches {
    let _ = writeln!(out, "{heading} Batch {}\n", batch.name);
    let _ = writeln!(out, "| metric | comparison | effect | same-condition spread | noise bound | verdict |");
    let _ = writeln!(out, "|---|---|---|---|---|---|");
    for pair in CONDITION_PAIRS {
      for metric in metrics {
        let Some(effect) = metric.effects.iter().find(|effect| effect.batch == batch.name && effect.pair() == pair) else {
          continue;
        };
        let _ = writeln!(
          out,
          "| {} | {} | {} | {} | {} | {} |",
          metric.metric,
          pair_label(pair.0, pair.1),
          signed(effect.effect),
          optional(effect.largest_same_condition_difference, figure),
          optional(effect.noise_bound, figure),
          effect.verdict.label(pair.1)
        );
      }
    }
    out.push('\n');
  }
}

fn render_pair_effects(out: &mut String, report: &NoiseReport, batch: &NoiseBatch, pair: Pair) {
  let (first, second) = pair;
  let effects: Vec<(&MetricNoise, &Effect)> = report
    .metrics
    .iter()
    .filter_map(|metric| {
      metric
        .effects
        .iter()
        .find(|effect| effect.batch == batch.name && effect.pair() == pair)
        .map(|effect| (metric, effect))
    })
    .collect();
  let count = |class: NoiseClass| effects.iter().filter(|(_, effect)| effect.class == class).count();
  let _ = writeln!(
    out,
    "{} metrics with a value in both conditions: {} outside the noise, {} inside, {} with no difference, {} undetermined.\n",
    effects.len(),
    count(NoiseClass::Outside),
    count(NoiseClass::Inside),
    count(NoiseClass::NoDifference),
    count(NoiseClass::Undetermined)
  );
  for (metric, effect) in effects.iter().filter(|(_, effect)| effect.class == NoiseClass::Outside) {
    let robustness = match effect.leave_one_out.crossing_the_bound.len() {
      0 => "stays outside whichever task is left out".to_owned(),
      crossing => format!(
        "**falls inside the bound when any one of {crossing} of its {} tasks is left out**",
        effect.leave_one_out.tasks
      ),
    };
    let _ = writeln!(
      out,
      "- `{}`: {} ({}), {} SE of the batch and {} pooled SE, noise bound {}; {}; {}.",
      metric.metric,
      signed(effect.effect),
      optional(effect.relative, signed_percent),
      ratio(effect.effect_over_standard_error),
      ratio(effect.effect_over_pooled_standard_error),
      optional(effect.noise_bound, figure),
      effect.direction.label(second),
      robustness
    );
  }
  out.push('\n');
  let above: Vec<&(&MetricNoise, &Effect)> = effects
    .iter()
    .filter(|(_, effect)| effect.class == NoiseClass::Inside)
    .filter(|(_, effect)| {
      effect
        .standard_error
        .is_some_and(|error| effect.effect.abs() > STANDARD_ERRORS_IN_BOUND * error + TOLERANCE)
    })
    .collect();
  if !above.is_empty() {
    let _ = writeln!(out, "Inside the bound although the effect exceeds 2 × SE of this batch:\n");
  }
  for (metric, effect) in &above {
    let _ = writeln!(
      out,
      "- `{}`: {} ({}), {} SE of the batch and {} pooled SE; kept inside by {} ({}); {}.",
      metric.metric,
      signed(effect.effect),
      optional(effect.relative, signed_percent),
      ratio(effect.effect_over_standard_error),
      ratio(effect.effect_over_pooled_standard_error),
      effect.binding.map_or("no term", BoundTerm::label),
      optional(effect.noise_bound, figure),
      effect.direction.label(second)
    );
  }
  if !above.is_empty() {
    out.push('\n');
  }
  let _ = writeln!(
    out,
    "| metric | {} | {} | effect | relative | tasks | SE, this batch | SE, pooled | largest A/A on the A/B scale | noise bound | bound / {} | effect / SE | effect / pooled SE | class | tasks whose removal moves it across the bound |",
    first.label(),
    second.label(),
    first.label()
  );
  let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
  for (metric, effect) in &effects {
    let unequal_sets = effect.first_tasks != effect.paired_tasks || effect.second_tasks != effect.paired_tasks;
    let tasks = match unequal_sets {
      true => format!(
        "{} ({} {}, {} {})",
        effect.paired_tasks,
        first.label(),
        effect.first_tasks,
        second.label(),
        effect.second_tasks
      ),
      false => effect.paired_tasks.to_string(),
    };
    let _ = writeln!(
      out,
      "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
      metric.metric,
      figure(effect.first_mean),
      figure(effect.second_mean),
      signed(effect.effect),
      optional(effect.relative, signed_percent),
      tasks,
      optional(effect.standard_error, figure),
      optional(effect.pooled_standard_error, figure),
      optional(effect.largest_same_condition_difference, figure),
      optional(effect.noise_bound, figure),
      optional(effect.bound_relative_to_baseline, percent),
      ratio(effect.effect_over_standard_error),
      ratio(effect.effect_over_pooled_standard_error),
      effect.class.label(),
      leave_one_out_cell(effect)
    );
  }
  out.push('\n');
}

fn render_spreads(out: &mut String, report: &NoiseReport) {
  let _ = writeln!(
    out,
    "| metric | comparison | pairs | median \\|diff\\| | median relative | range | paired difference | relative | size on the A/B scale |"
  );
  let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|");
  for metric in &report.metrics {
    for comparison in metric.repetitions.iter().chain(&metric.baseline_across_batches) {
      let spread = &comparison.spread;
      let _ = writeln!(
        out,
        "| {} | {} | {} | {} | {} | {} to {} | {} | {} | {} |",
        metric.metric,
        comparison.label,
        spread.pairs,
        figure(spread.median_absolute),
        optional(spread.median_relative, percent),
        signed(spread.min),
        signed(spread.max),
        signed(spread.paired_difference),
        optional(spread.paired_relative, signed_percent),
        figure(comparison.on_effect_scale())
      );
    }
  }
  out.push('\n');
}

fn ceiling_note(report: &NoiseReport) -> String {
  let Some(metric) = report.metrics.iter().find(|metric| metric.metric == HIDDEN_TESTS_METRIC) else {
    return String::new();
  };
  let pairs: Vec<&PairDifference> = metric.repetitions.iter().flat_map(|comparison| &comparison.spread.per_task).collect();
  let passed = pairs.iter().map(|pair| pair.a + pair.b).sum::<f64>();
  format!(
    " In the repetition pairs of these batches the hidden tests passed in {} of {} runs.",
    figure(passed),
    pairs.len() * 2
  )
}

pub fn render_noise_markdown(report: &NoiseReport) -> String {
  let mut out = String::new();
  let _ = writeln!(out, "# Noise floor: how much do two runs of the same condition differ?\n");
  let _ = writeln!(
    out,
    "Computed from recorded runs only; no session was run for this report. An A/B effect smaller than the difference between two runs of the same condition is not evidence, so this report measures that difference for every metric of the agent report and places the reported A/B effects next to it.\n"
  );
  let _ = writeln!(out, "{SCOPE}{}\n", ceiling_note(report));
  let _ = writeln!(out, "| batch | date recorded | preamble | tasks | scored runs | excluded |");
  let _ = writeln!(out, "|---|---|---|---|---|---|");
  for batch in &report.batches {
    let _ = writeln!(
      out,
      "| {} | {} | {} | {} | {} | {} |",
      batch.name,
      batch.date.as_deref().unwrap_or("unknown"),
      batch.preamble.as_deref().unwrap_or("unknown"),
      batch.tasks,
      batch.scored_runs,
      if batch.excluded_runs.is_empty() {
        "none".to_owned()
      } else {
        batch.excluded_runs.join(", ")
      }
    );
  }
  let _ = writeln!(out, "\n## Is the baseline the same in every batch?\n");
  if report.baseline_identity.is_empty() {
    let _ = writeln!(out, "Only one batch was given, so there is no across-batch comparison.\n");
  }
  for identity in &report.baseline_identity {
    render_identity(&mut out, identity);
  }
  let _ = writeln!(out, "## Definitions\n\n{DEFINITIONS}\n");
  let _ = writeln!(out, "## Pairwise verdicts\n");
  render_verdicts(&mut out, report);
  let _ = writeln!(out, "## A/B effects against the noise\n");
  for batch in &report.batches {
    render_effects(&mut out, report, batch);
  }
  let _ = writeln!(out, "## What the data supports\n\n{LIMITS}\n");
  let _ = writeln!(out, "## A/A differences per metric\n");
  render_spreads(&mut out, report);
  out
}

pub fn write_noise(report: &NoiseReport, out_base: &Path) -> AgentResult<(PathBuf, PathBuf)> {
  if let Some(parent) = out_base.parent() {
    fs::create_dir_all(parent).map_err(|error| AgentError::io(parent, error))?;
  }
  let markdown = out_base.with_extension("md");
  let json = out_base.with_extension("json");
  fs::write(&markdown, render_noise_markdown(report)).map_err(|error| AgentError::io(&markdown, error))?;
  write_json_dense(&json, report)?;
  Ok((markdown, json))
}

#[cfg(test)]
mod tests {
  use super::super::claude::ConfigIsolation;
  use super::super::plan::{AtlasSurface, PreambleVariant};
  use super::super::testing::run_record;
  use super::super::transcript::TokenUsage;
  use super::*;

  fn pair(task_id: &str, a: f64, b: f64) -> PairDifference {
    PairDifference {
      task_id: task_id.to_owned(),
      a,
      b,
    }
  }

  fn close(actual: Option<f64>, expected: f64) -> bool {
    actual.is_some_and(|actual| (actual - expected).abs() < 1e-12)
  }

  #[test]
  fn should_summarise_paired_differences() {
    let spread = PairSpread::of(vec![pair("t1", 10.0, 14.0), pair("t2", 20.0, 18.0), pair("t3", 0.0, 0.0), pair("t4", 5.0, 5.0)]).unwrap();
    assert_eq!(spread.pairs, 4);
    assert_eq!(spread.median_absolute, 1.0);
    assert!(close(spread.median_relative, 2.0 / 19.0), "{:?}", spread.median_relative);
    assert_eq!((spread.min, spread.max), (-2.0, 4.0));
    assert_eq!((spread.mean_a, spread.mean_b, spread.paired_difference), (8.75, 9.25, 0.5));
    assert!(close(spread.paired_relative, 0.5 / 8.75));
    assert_eq!(PairSpread::of(Vec::new()), None);
    let zeros = PairSpread::of(vec![pair("t1", 0.0, 0.0)]).unwrap();
    assert_eq!((zeros.median_relative, zeros.paired_relative), (None, None));
  }

  #[test]
  fn should_estimate_the_variance_of_a_mean_from_repetitions() {
    assert_eq!(variance_of_mean(&[2.0, 4.0]), Some(1.0));
    assert_eq!(variance_of_mean(&[1.0, 2.0, 3.0]), Some(1.0 / 3.0));
    assert_eq!(variance_of_mean(&[5.0]), None);
  }

  fn cells(rows: &[(&str, Condition, &[f64])]) -> Cells {
    rows
      .iter()
      .map(|(task_id, condition, values)| (((*task_id).to_owned(), *condition), values.to_vec()))
      .collect()
  }

  fn classified(cells: &Cells, pooled_variances: &[f64], largest: Option<f64>) -> Effect {
    let paired: BTreeSet<String> = ["t1", "t2"].map(str::to_owned).into();
    effect(&EffectInput {
      batch: "b1",
      pair: (Condition::Baseline, Condition::Atlas),
      def: &CACHE_WEIGHTED_TOKENS,
      cells,
      paired: &paired,
      reported: None,
      pooled_variances,
      largest_same_condition_difference: largest,
    })
    .unwrap()
  }

  #[test]
  fn should_classify_an_effect_against_every_noise_estimate() {
    let measured = cells(&[
      ("t1", Condition::Baseline, &[10.0, 12.0]),
      ("t1", Condition::Atlas, &[15.0, 17.0]),
      ("t2", Condition::Baseline, &[20.0, 20.0]),
      ("t2", Condition::Atlas, &[26.0, 24.0]),
    ]);
    let outside = classified(&measured, &[], Some(1.0));
    assert_eq!((outside.effect, outside.paired_tasks, outside.replicate_tasks), (5.0, 2, 2));
    assert!(close(outside.standard_error, 0.75_f64.sqrt()));
    assert!(close(outside.noise_bound, 2.0 * 0.75_f64.sqrt()));
    assert!(close(outside.relative, 5.0 / 15.5));
    assert!(close(outside.effect_over_standard_error, 5.0 / 0.75_f64.sqrt()));
    assert_eq!((outside.effect_over_pooled_standard_error, outside.binding), (None, Some(BoundTerm::BatchStandardError)));
    assert_eq!((outside.class, outside.direction), (NoiseClass::Outside, EffectDirection::SecondWorse));
    let guarded = classified(&measured, &[], Some(5.0));
    assert_eq!((guarded.class, guarded.binding), (NoiseClass::Inside, Some(BoundTerm::SameConditionDifference)));
    let pooled = classified(&measured, &[18.0, 18.0], Some(1.0));
    assert!(close(pooled.pooled_standard_error, 3.0));
    assert!(close(pooled.effect_over_pooled_standard_error, 5.0 / 3.0));
    assert_eq!(
      (pooled.noise_bound, pooled.class, pooled.binding),
      (Some(6.0), NoiseClass::Inside, Some(BoundTerm::PooledStandardError))
    );
  }

  #[test]
  fn should_separate_no_difference_and_undetermined_effects() {
    let equal = cells(&[("t1", Condition::Baseline, &[10.0, 12.0]), ("t1", Condition::Atlas, &[11.0, 11.0])]);
    assert_eq!(classified(&equal, &[], Some(3.0)).class, NoiseClass::NoDifference);
    let single = cells(&[("t1", Condition::Baseline, &[10.0]), ("t1", Condition::Atlas, &[14.0]), ("t2", Condition::Baseline, &[1.0])]);
    let undetermined = classified(&single, &[], None);
    assert_eq!(
      (undetermined.class, undetermined.noise_bound, undetermined.standard_error),
      (NoiseClass::Undetermined, None, None)
    );
    assert_eq!((undetermined.first_tasks, undetermined.second_tasks, undetermined.paired_tasks), (2, 1, 1));
    assert_eq!((undetermined.first_mean, undetermined.effect), (5.5, 4.0));
    assert_eq!(EffectDirection::of(Better::Higher, 4.0), EffectDirection::SecondBetter);
    assert_eq!(EffectDirection::of(Better::Neutral, 4.0), EffectDirection::Neutral);
  }

  fn environment(model: &str, commit: &str) -> Environment {
    Environment {
      date: "2026-10-04".to_owned(),
      seed: 1,
      claude_version: Some("2.1.289 (Claude Code)".to_owned()),
      model: model.to_owned(),
      judge_model: Some("claude-sonnet-5-5".to_owned()),
      config_isolation: ConfigIsolation::User,
      code_atlas_commit: Some(commit.to_owned()),
      code_atlas_version: None,
      atlas_bin_blake3: None,
      atlas_bin: "/bin/catl".to_owned(),
      preamble_version: 1,
      preamble_variant: PreambleVariant::Standard,
      repo_override: None,
      atlas_config: None,
      atlas_hooks: None,
      atlas_surface: AtlasSurface::Standard,
      probe: None,
      linked_paths: BTreeMap::new(),
      incumbent: None,
    }
  }

  fn tokens(total: u64) -> TokenUsage {
    TokenUsage {
      input: 100,
      output: total - 1140,
      cache_read: 1000,
      cache_creation: 40,
    }
  }

  fn batch(name: &str, environment: Environment, totals: &[[u64; 4]]) -> NoiseInput {
    let records = ["t1", "t2", "t3"]
      .iter()
      .zip(totals)
      .flat_map(|(task, [baseline_0, baseline_1, atlas_0, atlas_1])| {
        [
          run_record(task, Condition::Baseline, 0, tokens(*baseline_0)),
          run_record(task, Condition::Baseline, 1, tokens(*baseline_1)),
          run_record(task, Condition::Atlas, 0, tokens(*atlas_0)),
          run_record(task, Condition::Atlas, 1, tokens(*atlas_1)),
        ]
      })
      .collect();
    NoiseInput {
      name: name.to_owned(),
      manifest: Some(BatchManifest {
        environment,
        tasks: vec!["t1".to_owned(), "t2".to_owned()],
        truths: Vec::new(),
        planned: Vec::new(),
        skipped_for_budget: Vec::new(),
        skipped_after_hooks_failure: Vec::new(),
        spent_usd: 1.0,
        instruction_sections_dropped: BTreeMap::new(),
        held_out: Vec::new(),
      }),
      records,
      reported: None,
    }
  }

  fn two_batches() -> Vec<NoiseInput> {
    vec![
      batch("b1", environment("claude-sonnet-5-5", "aaa"), &[[2000, 2400, 3000, 3000], [5000, 4000, 6000, 7000]]),
      batch("b2", environment("claude-sonnet-5-5", "bbb"), &[[2200, 2200, 2000, 2000], [4000, 4000, 4000, 4000]]),
    ]
  }

  fn metric<'a>(report: &'a NoiseReport, name: &str) -> &'a MetricNoise {
    report.metrics.iter().find(|metric| metric.metric == name).unwrap()
  }

  #[test]
  fn should_compare_repetitions_batches_and_conditions_per_metric() {
    let report = build_noise(&two_batches());
    assert_eq!(report.baseline_identity[0].verdict, BaselineVerdict::Identical);
    assert_eq!(report.baseline_identity[0].comparable_tasks, vec!["t1", "t2"]);
    let harness: Vec<(&str, bool)> = report.baseline_identity[0]
      .harness_fields
      .iter()
      .map(|check| (check.field.as_str(), check.identical))
      .collect();
    assert_eq!(harness, vec![("date recorded", true), ("seed", true), ("eval repository commit", false)]);
    let total = metric(&report, "total_tokens");
    let labels: Vec<&str> = total.repetitions.iter().map(|comparison| comparison.label.as_str()).collect();
    assert_eq!(
      labels,
      vec![
        "baseline, second run − first run, batch b1",
        "atlas, second run − first run, batch b1",
        "baseline, second run − first run, batch b2",
        "atlas, second run − first run, batch b2"
      ]
    );
    let baseline = &total.repetitions[0].spread;
    assert_eq!(
      (baseline.pairs, baseline.median_absolute, baseline.min, baseline.max, baseline.paired_difference),
      (2, 700.0, -1000.0, 400.0, -300.0)
    );
    let across = &total.baseline_across_batches[0];
    assert_eq!(across.label, "baseline, batch b2 − batch b1 (per-task means)");
    assert_eq!((across.spread.mean_a, across.spread.mean_b, across.spread.paired_difference), (3350.0, 3100.0, -250.0));
    let effect = &total.effects[0];
    assert_eq!((effect.first_mean, effect.second_mean, effect.effect), (3350.0, 4750.0, 1400.0));
    assert!(close(
      effect.standard_error,
      (((200.0_f64.powi(2)) + (500.0_f64.powi(2) + 500.0_f64.powi(2))) / 2.0 / 2.0).sqrt()
    ));
    assert_eq!(
      total.repetitions.iter().map(|comparison| comparison.scale_to_effect).collect::<Vec<_>>(),
      vec![0.5_f64.sqrt(); 4]
    );
    assert_eq!(across.scale_to_effect, 1.0);
    assert!(close(effect.largest_same_condition_difference, 500.0 * 0.5_f64.sqrt()));
    assert_eq!(
      (effect.class, effect.direction, effect.binding),
      (NoiseClass::Outside, EffectDirection::SecondWorse, Some(BoundTerm::BatchStandardError))
    );
    assert_eq!(
      effect.leave_one_out,
      LeaveOneOut {
        tasks: 2,
        crossing_the_bound: Vec::new()
      }
    );
    assert_eq!((total.effects[1].effect, total.effects[1].class), (-100.0, NoiseClass::Inside));
    let weighted = metric(&report, "cache_weighted_tokens");
    assert_eq!((weighted.effects[0].first_mean, weighted.effects[0].class), (250.0, NoiseClass::NoDifference));
    assert_eq!(report.batches[0].scored_runs, 8);
  }

  #[test]
  fn should_name_the_task_whose_removal_moves_an_effect_inside_the_bound() {
    let carried_by_one_task = batch(
      "b1",
      environment("claude-sonnet-5-5", "aaa"),
      &[[2000, 2004, 2002, 2002], [2000, 1996, 2000, 2000], [2000, 2000, 2030, 2030]],
    );
    let report = build_noise(&[carried_by_one_task]);
    let effect = &metric(&report, "total_tokens").effects[0];
    assert!(close(Some(effect.effect), 32.0 / 3.0));
    assert!(close(effect.standard_error, (8.0_f64 / 3.0 / 3.0).sqrt()));
    assert_eq!((effect.largest_same_condition_difference, effect.class), (Some(0.0), NoiseClass::Outside));
    assert_eq!(
      effect.leave_one_out,
      LeaveOneOut {
        tasks: 3,
        crossing_the_bound: vec!["t3".to_owned()]
      }
    );
    let markdown = render_noise_markdown(&report);
    assert!(
      markdown.contains("- `total_tokens`: +10.67 (+0.5%), +11.31 SE of the batch and +11.31 pooled SE, noise bound 1.886; atlas worse; **falls inside the bound when any one of 1 of its 3 tasks is left out**."),
      "{markdown}"
    );
  }

  #[test]
  fn should_list_inside_effects_above_two_standard_errors_with_the_term_that_holds_them() {
    let steady_batch = batch("b1", environment("claude-sonnet-5-5", "aaa"), &[[2000, 2000, 2100, 2100], [2000, 2000, 2100, 2100]]);
    let noisy_batch = batch("b2", environment("claude-sonnet-5-5", "aaa"), &[[2000, 2400, 2000, 2000], [2000, 2000, 2000, 2400]]);
    let report = build_noise(&[steady_batch, noisy_batch]);
    let effect = &metric(&report, "total_tokens").effects[0];
    assert_eq!((effect.effect, effect.standard_error, effect.effect_over_standard_error), (100.0, Some(0.0), None));
    assert!(close(effect.pooled_standard_error, 100.0));
    assert_eq!((effect.class, effect.binding), (NoiseClass::Inside, Some(BoundTerm::PooledStandardError)));
    let turned = &metric(&report, "total_tokens").effects[1];
    assert_eq!((turned.effect, turned.class), (0.0, NoiseClass::NoDifference));
    let markdown = render_noise_markdown(&report);
    assert!(
      markdown.contains(
        "Inside the bound although the effect exceeds 2 × SE of this batch:\n\n- `total_tokens`: +100 (+5%), – SE of the batch and +1.00 pooled SE; kept inside by 2 × pooled SE (200); atlas worse."
      ),
      "{markdown}"
    );
    assert!(!markdown.contains("one signal"), "{markdown}");
  }

  #[test]
  fn should_say_when_one_token_kind_makes_up_most_of_the_total() {
    let report = build_noise(&[batch("b1", environment("claude-sonnet-5-5", "aaa"), &[[1500, 1500, 1500, 1500]])]);
    let markdown = render_noise_markdown(&report);
    assert!(
      markdown.contains("`cache_read_tokens` is 66.7% of `total_tokens` in the baseline of this batch, so the two rows are one signal, not two."),
      "{markdown}"
    );
  }

  #[test]
  fn should_restrict_or_refuse_the_comparison_across_batches_when_the_baseline_differs() {
    let mut restricted = two_batches();
    restricted[1].records[0].manifest.spec_hash = "edited".to_owned();
    restricted[1].records[6].manifest.args[7] = "Fix it differently.".to_owned();
    let report = build_noise(&restricted);
    let identity = &report.baseline_identity[0];
    assert_eq!((identity.verdict, identity.comparable_tasks.clone()), (BaselineVerdict::Restricted, vec!["t2".to_owned()]));
    assert_eq!(
      identity.tasks_differing,
      vec![TaskDifference {
        task_id: "t1".to_owned(),
        fields: vec!["task spec hash (prompt, ground truth, validation, base patch)".to_owned()]
      }]
    );
    assert_eq!(metric(&report, "total_tokens").baseline_across_batches[0].spread.pairs, 1);

    let mut refused = two_batches();
    refused[1].manifest = Some(BatchManifest {
      environment: environment("claude-haiku-4-5", "bbb"),
      ..refused[1].manifest.clone().unwrap()
    });
    let report = build_noise(&refused);
    assert_eq!(report.baseline_identity[0].verdict, BaselineVerdict::NotComparable);
    assert_eq!(report.baseline_identity[0].session_fields[0].values, vec!["claude-sonnet-5-5", "claude-haiku-4-5"]);
    assert_eq!(metric(&report, "total_tokens").baseline_across_batches, Vec::new());
  }

  #[test]
  fn should_mask_the_scratch_directory_when_comparing_command_lines() {
    let record = run_record("t1", Condition::Baseline, 0, TokenUsage::default());
    assert_eq!(
      normalized_args(&record),
      vec![
        "--mcp-config",
        "<scratch>/mcp.json",
        "--add-dir",
        "<scratch>/tree",
        "--append-system-prompt",
        "Read CLAUDE.md first.",
        "-p",
        "Fix it."
      ]
    );
  }

  #[test]
  fn should_render_classes_bounds_and_the_scope_statement() {
    let markdown = render_noise_markdown(&build_noise(&two_batches()));
    assert!(markdown.contains("It is not the A/A run on a harder task set"), "{markdown}");
    assert!(
      markdown.contains("| total_tokens | 3350 | 4750 | +1400 | +41.8% | 2 | 367.4 | 259.8 | 353.6 | 734.8 | 21.9% | +3.81 | +5.39 | **outside** | 0 of 2 |"),
      "{markdown}"
    );
    assert!(
      markdown.contains("| total_tokens | baseline, batch b2 − batch b1 (per-task means) | 2 | 250 | 5.9% | -500 to 0 | -250 | -7.5% | 250 |"),
      "{markdown}"
    );
    assert!(markdown.contains("| eval repository commit | aaa | bbb | **differs** |"), "{markdown}");
    assert!(!markdown.contains("/var/folders") && !markdown.contains("/repo"), "{markdown}");
  }

  fn arms(name: &str, rows: &[(&str, Condition, [u64; 2])]) -> NoiseInput {
    let records = rows
      .iter()
      .flat_map(|(task, condition, [first, second])| [run_record(task, *condition, 0, tokens(*first)), run_record(task, *condition, 1, tokens(*second))])
      .collect();
    NoiseInput {
      records,
      ..batch(name, environment("claude-sonnet-5-5", "aaa"), &[])
    }
  }

  #[test]
  fn should_judge_every_pair_of_arms_against_the_spread_of_two_baseline_batches() {
    let three_arms = arms(
      "b1",
      &[
        ("t1", Condition::Baseline, [2000, 2200]),
        ("t1", Condition::Incumbent, [2100, 2300]),
        ("t1", Condition::Atlas, [3000, 3000]),
        ("t2", Condition::Baseline, [4000, 4000]),
        ("t2", Condition::Incumbent, [4100, 3900]),
        ("t2", Condition::Atlas, [6000, 6200]),
      ],
    );
    let second_baseline = arms("b2", &[("t1", Condition::Baseline, [2100, 2100]), ("t2", Condition::Baseline, [4000, 4200])]);
    let report = build_noise(&[three_arms, second_baseline]);
    assert_eq!(report.baseline_identity[0].verdict, BaselineVerdict::Identical);
    let total = metric(&report, "total_tokens");
    let effects: Vec<(&str, Condition, Condition, f64, Verdict)> = total
      .effects
      .iter()
      .map(|effect| (effect.batch.as_str(), effect.first, effect.second, effect.effect, effect.verdict))
      .collect();
    assert_eq!(
      effects,
      vec![
        ("b1", Condition::Incumbent, Condition::Atlas, 1450.0, Verdict::SecondWorse),
        ("b1", Condition::Baseline, Condition::Atlas, 1500.0, Verdict::SecondWorse),
        ("b1", Condition::Baseline, Condition::Incumbent, 50.0, Verdict::Equal),
      ]
    );
    assert!(close(total.effects[0].noise_bound, 2.0 * 7500.0_f64.sqrt()));
    assert!(close(total.effects[1].noise_bound, 2.0 * 5000.0_f64.sqrt()));
    assert!(close(total.effects[2].largest_same_condition_difference, 100.0 * 0.5_f64.sqrt()));
    assert_eq!(total.baseline_across_batches[0].spread.paired_difference, 50.0);
    let labels: Vec<&str> = total.repetitions.iter().map(|comparison| comparison.label.as_str()).collect();
    assert_eq!(
      labels,
      vec![
        "baseline, second run − first run, batch b1",
        "atlas, second run − first run, batch b1",
        "incumbent, second run − first run, batch b1",
        "baseline, second run − first run, batch b2"
      ]
    );
    let markdown = render_noise_markdown(&report);
    for line in [
      "## Pairwise verdicts\n\nEvery effect between two arms (A = baseline, B = Serena + graphify, C = atlas)",
      "| total_tokens | C vs B (verdict) | +1450 | 70.71 | 173.2 | **atlas worse** |\n",
      "| total_tokens | C vs A | +1500 | 70.71 | 141.4 | **atlas worse** |\n",
      "| total_tokens | B vs A | +50 | 70.71 | 173.2 | equal |\n",
      "#### C vs B (verdict): atlas − Serena + graphify\n",
      "| metric | Serena + graphify | atlas | effect | relative | tasks |",
      "| metric | baseline | Serena + graphify | effect | relative | tasks |",
      "### Batch b2\n\n| metric | comparison | effect | same-condition spread | noise bound | verdict |\n|---|---|---|---|---|---|\n\n",
    ] {
      assert!(markdown.contains(line), "{line}\n{markdown}");
    }
  }

  #[test]
  fn should_judge_dev_and_held_out_tasks_separately_when_tasks_are_held_out() {
    let mut three_arms = arms(
      "b1",
      &[
        ("t1", Condition::Baseline, [2000, 2200]),
        ("t1", Condition::Incumbent, [2100, 2300]),
        ("t1", Condition::Atlas, [3000, 3000]),
        ("t2", Condition::Baseline, [4000, 4000]),
        ("t2", Condition::Incumbent, [4100, 3900]),
        ("t2", Condition::Atlas, [6000, 6200]),
      ],
    );
    if let Some(manifest) = three_arms.manifest.as_mut() {
      manifest.held_out = vec!["t2".to_owned()];
    }
    let second_baseline = arms("b2", &[("t1", Condition::Baseline, [2100, 2100]), ("t2", Condition::Baseline, [4000, 4200])]);
    let report = build_split_noise(&[three_arms, second_baseline], &[]);
    assert_eq!(report.held_out, vec!["t2"]);
    let sections: Vec<(&str, Vec<&str>)> = report
      .sections
      .iter()
      .map(|section| (section.name.as_str(), section.tasks.iter().map(String::as_str).collect()))
      .collect();
    assert_eq!(sections, vec![("dev", vec!["t1"]), ("held-out", vec!["t2"])]);
    let dev = report.sections[0].metrics.iter().find(|metric| metric.metric == "total_tokens").unwrap();
    assert_eq!((dev.effects[0].effect, dev.effects[0].verdict), (800.0, Verdict::SecondWorse));
    let markdown = render_noise_markdown(&report);
    for line in [
      "### dev tasks (t1)\n\n#### Batch b1\n",
      "| total_tokens | C vs B (verdict) | +800 | 141.4 | 200 | **atlas worse** |\n",
      "### held-out tasks (t2)\n\n#### Batch b1\n",
    ] {
      assert!(markdown.contains(line), "{line}\n{markdown}");
    }
    assert_eq!(build_split_noise(&[arms("b3", &[("t1", Condition::Baseline, [2000, 2100])])], &[]).sections, Vec::new());
  }

  #[test]
  fn should_print_four_significant_digits() {
    assert_eq!(figure(207066.227), "207066");
    assert_eq!(figure(12.636), "12.64");
    assert_eq!(figure(0.010149), "0.01015");
    assert_eq!(figure(-229.409), "-229.4");
    assert_eq!(figure(4.5), "4.5");
    assert_eq!(figure(0.0), "0");
    assert_eq!(signed(1.455), "+1.455");
  }
}
