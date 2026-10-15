use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::{RetrievalError, RetrievalResult};
use super::lines::{LineChannel, LineGoldSummary, LineScores};
use super::phrases::Phrase;
use super::queries::{Provenance, QueryVariant, VariantQuery};
use super::score::{BudgetYield, ChannelValues, F_BETA, FileScore, MetricMean, RenameSiteScore, ReuseScore, YIELD_BUDGETS, mean_of};
use crate::agent::AtlasConfigRecord;
use crate::quality::SetOverlap;

pub const DEV_SET_CAVEAT: &str = "Dev set, same authors: the tasks, their ground truth and Code Atlas were written by the same people, and the tasks were \
used while developing the product. Read these numbers as regression signals for the retrieval surface, not as an unbiased estimate of how it \
performs on unseen code.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalSettings {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub atlas_config: Option<AtlasConfigRecord>,
  pub context_budget: usize,
  pub search_limit: usize,
  pub max_code_spans: usize,
  pub max_prose_phrases: usize,
  pub max_similar_seeds: usize,
  pub impact_group_limit: usize,
  pub yield_budgets: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexSummary {
  pub repos: Vec<String>,
  pub files: usize,
  pub degraded: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SeedOutcome {
  Found { members: usize, shape_only: usize },
  NotFound,
  Ambiguous { candidates: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimilarSeed {
  pub key: String,
  pub outcome: SeedOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryLog {
  pub search_phrases: Vec<Phrase>,
  pub similar_seeds: Vec<SimilarSeed>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSize {
  pub bytes: usize,
  pub estimated_tokens: usize,
  pub budget: usize,
  pub must_read: usize,
  pub skim: usize,
  pub patterns: usize,
  pub reuse: usize,
  pub impact: usize,
  pub tests: usize,
  pub uncertain: usize,
  pub omitted: usize,
}

impl PackSize {
  pub fn items(&self) -> usize {
    self.must_read + self.skim + self.patterns + self.reuse + self.impact + self.tests + self.uncertain
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BlastRadius {
  Unavailable {
    reason: String,
  },
  Scored {
    definition_file: String,
    seeds: Vec<String>,
    expected: Vec<String>,
    visible: SetOverlap,
    full: SetOverlap,
    collapsed_visible: usize,
    missed: Vec<String>,
  },
}

impl BlastRadius {
  fn recall(&self, view: ImpactView) -> Option<f64> {
    match (self, view) {
      (Self::Scored { visible, .. }, ImpactView::Visible) => visible.recall,
      (Self::Scored { full, .. }, ImpactView::Full) => full.recall,
      (Self::Unavailable { .. }, _) => None,
    }
  }
}

#[derive(Debug, Clone, Copy)]
enum ImpactView {
  Visible,
  Full,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RenameRetrieval {
  Unavailable {
    reason: String,
  },
  Scored {
    target: String,
    new_name: String,
    servers: Vec<String>,
    score: RenameSiteScore,
  },
}

impl RenameRetrieval {
  fn score(&self) -> Option<&RenameSiteScore> {
    match self {
      Self::Scored { score, .. } => Some(score),
      Self::Unavailable { .. } => None,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryScore {
  pub query: VariantQuery,
  pub searches: QueryLog,
  pub pack: PackSize,
  pub must_change: FileScore,
  pub relevant_files: FileScore,
  pub must_consider: FileScore,
  pub reuse: Option<ReuseScore>,
  pub lines: Option<LineScores>,
  pub yields: Vec<BudgetYield>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VariantOutcome {
  Unavailable { reason: String },
  Scored { queries: Vec<QueryScore> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantResult {
  pub variant: QueryVariant,
  pub outcome: VariantOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRetrieval {
  pub index: IndexSummary,
  pub line_gold: LineGoldSummary,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub no_call: Vec<Provenance>,
  pub variants: Vec<VariantResult>,
  pub blast_radius: Option<BlastRadius>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub rename: Option<RenameRetrieval>,
}

impl TaskRetrieval {
  pub fn queries(&self, variant: QueryVariant) -> &[QueryScore] {
    match self.variants.iter().find(|result| result.variant == variant).map(|result| &result.outcome) {
      Some(VariantOutcome::Scored { queries }) => queries,
      Some(VariantOutcome::Unavailable { .. }) | None => &[],
    }
  }

  fn value(&self, variant: QueryVariant, metric: impl Fn(&QueryScore) -> Option<f64>) -> Option<f64> {
    mean_of("", self.queries(variant).iter().map(metric)).mean
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TaskOutcome {
  Scored(Box<TaskRetrieval>),
  Failed { stage: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskEntry {
  pub id: String,
  pub title: String,
  pub categories: Vec<String>,
  pub outcome: TaskOutcome,
}

impl TaskEntry {
  fn scored(&self) -> Option<&TaskRetrieval> {
    match &self.outcome {
      TaskOutcome::Scored(retrieval) => Some(retrieval),
      TaskOutcome::Failed { .. } => None,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackSection {
  MustRead,
  Pack,
}

impl PackSection {
  pub const ALL: [Self; 2] = [Self::MustRead, Self::Pack];

  fn key(self) -> &'static str {
    match self {
      Self::MustRead => "must_read",
      Self::Pack => "pack",
    }
  }

  fn title(self) -> &'static str {
    match self {
      Self::MustRead => "MUST READ",
      Self::Pack => "whole pack",
    }
  }

  fn files(self, score: &FileScore) -> &SetOverlap {
    match self {
      Self::MustRead => &score.channels.must_read,
      Self::Pack => &score.channels.pack,
    }
  }

  fn f05(self, values: &ChannelValues) -> Option<f64> {
    match self {
      Self::MustRead => values.must_read,
      Self::Pack => values.pack,
    }
  }

  fn lines(self, scores: &LineScores) -> &LineChannel {
    match self {
      Self::MustRead => &scores.must_read,
      Self::Pack => &scores.pack,
    }
  }

  fn yielded(self, at: &BudgetYield) -> &SetOverlap {
    match self {
      Self::MustRead => &at.must_read,
      Self::Pack => &at.pack,
    }
  }
}

fn line_channel(query: &QueryScore, section: PackSection) -> Option<&LineChannel> {
  query.lines.as_ref().map(|lines| section.lines(lines))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeadlineMetric {
  MustChangeRecall,
  RelevantPrecision,
  FileF05,
  LineF05,
  Yield(usize),
}

impl HeadlineMetric {
  fn all() -> Vec<Self> {
    [Self::MustChangeRecall, Self::RelevantPrecision, Self::FileF05, Self::LineF05]
      .into_iter()
      .chain(YIELD_BUDGETS.map(Self::Yield))
      .collect()
  }

  fn key(self) -> String {
    match self {
      Self::MustChangeRecall => "must_change.recall".to_owned(),
      Self::RelevantPrecision => "relevant_files.precision".to_owned(),
      Self::FileF05 => "relevant_files.f05".to_owned(),
      Self::LineF05 => "lines.f05".to_owned(),
      Self::Yield(budget) => format!("yield.{budget}"),
    }
  }

  fn title(self) -> String {
    match self {
      Self::MustChangeRecall => "must_change recall".to_owned(),
      Self::RelevantPrecision => "relevant_files precision".to_owned(),
      Self::FileF05 => "file F0.5".to_owned(),
      Self::LineF05 => "line F0.5".to_owned(),
      Self::Yield(budget) => format!("yield@{budget}"),
    }
  }

  fn value(self, section: PackSection, query: &QueryScore) -> Option<f64> {
    match self {
      Self::MustChangeRecall => section.files(&query.must_change).recall,
      Self::RelevantPrecision => section.files(&query.relevant_files).precision,
      Self::FileF05 => section.f05(&query.relevant_files.f05),
      Self::LineF05 => line_channel(query, section)?.f05,
      Self::Yield(budget) => query.yields.iter().find(|at| at.budget == budget).and_then(|at| section.yielded(at).recall),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeadlineRow {
  pub variant: QueryVariant,
  pub section: PackSection,
  pub cells: Vec<MetricMean>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairedComparison {
  pub variant: QueryVariant,
  pub section: PackSection,
  pub metric: String,
  pub tasks: usize,
  pub full: Option<f64>,
  pub variant_mean: Option<f64>,
  pub difference: Option<f64>,
  pub higher: usize,
  pub equal: usize,
  pub lower: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievalReport {
  pub caveat: String,
  pub code_atlas_revision: Option<String>,
  pub settings: RetrievalSettings,
  pub headline: Vec<HeadlineRow>,
  pub comparison: Vec<PairedComparison>,
  pub tasks: Vec<TaskEntry>,
  pub aggregate: Vec<MetricMean>,
}

impl RetrievalReport {
  pub fn new(code_atlas_revision: Option<String>, settings: RetrievalSettings, tasks: Vec<TaskEntry>) -> Self {
    Self {
      caveat: DEV_SET_CAVEAT.to_owned(),
      code_atlas_revision,
      settings,
      headline: headline(&tasks),
      comparison: compare_with_full(&tasks),
      aggregate: aggregate(&tasks),
      tasks,
    }
  }
}

type QueryMetric = Box<dyn Fn(&QueryScore) -> Option<f64>>;
type FileSelector = fn(&QueryScore) -> &FileScore;
type OverlapSelector = fn(&FileScore) -> &SetOverlap;
type F05Selector = fn(&FileScore) -> Option<f64>;
type ReuseSelector = fn(&ReuseScore) -> Option<f64>;
type RenameSelector = fn(&RenameSiteScore) -> Option<f64>;

const MUST_CHANGE: (&str, FileSelector) = ("must_change", |query| &query.must_change);
const RELEVANT_FILES: (&str, FileSelector) = ("relevant_files", |query| &query.relevant_files);
const MUST_CONSIDER: (&str, FileSelector) = ("must_consider", |query| &query.must_consider);
const FILE_SETS: [(&str, FileSelector); 3] = [MUST_CHANGE, RELEVANT_FILES, MUST_CONSIDER];
const F05_SETS: [(&str, FileSelector); 2] = [RELEVANT_FILES, MUST_CHANGE];

const CHANNELS: [(&str, OverlapSelector, F05Selector); 5] = [
  ("must_read", |score| &score.channels.must_read, |score| score.f05.must_read),
  ("pack", |score| &score.channels.pack, |score| score.f05.pack),
  ("search", |score| &score.channels.search, |score| score.f05.search),
  ("similar", |score| &score.channels.similar, |score| score.f05.similar),
  ("any", |score| &score.channels.any, |score| score.f05.any),
];

const REUSE_CHANNELS: [(&str, ReuseSelector); 4] = [
  ("pack", |score| score.pack),
  ("search", |score| score.search),
  ("similar", |score| score.similar),
  ("any", |score| score.any),
];

const TIE: f64 = 1e-9;

fn query_metric(name: String, value: impl Fn(&QueryScore) -> Option<f64> + 'static) -> (String, QueryMetric) {
  (name, Box::new(value))
}

fn query_metrics() -> Vec<(String, QueryMetric)> {
  let mut metrics = Vec::new();
  for (set, select) in FILE_SETS {
    for (channel, overlap, _) in CHANNELS {
      metrics.push(query_metric(format!("{set}.recall.{channel}"), move |query| overlap(select(query)).recall));
    }
  }
  for (channel, overlap, _) in CHANNELS {
    metrics.push(query_metric(format!("relevant_files.precision.{channel}"), move |query| {
      overlap(&query.relevant_files).precision
    }));
  }
  for (set, select) in F05_SETS {
    for (channel, _, f05) in CHANNELS {
      metrics.push(query_metric(format!("{set}.f05.{channel}"), move |query| f05(select(query))));
    }
  }
  for section in PackSection::ALL {
    let key = section.key();
    metrics.push(query_metric(format!("lines.precision.{key}"), move |query| line_channel(query, section)?.lines.precision));
    metrics.push(query_metric(format!("lines.recall.{key}"), move |query| line_channel(query, section)?.lines.recall));
    metrics.push(query_metric(format!("lines.f05.{key}"), move |query| line_channel(query, section)?.f05));
  }
  for budget in YIELD_BUDGETS {
    for section in PackSection::ALL {
      metrics.push(query_metric(format!("yield.{budget}.{}", section.key()), move |query| {
        HeadlineMetric::Yield(budget).value(section, query)
      }));
    }
  }
  for (channel, value) in REUSE_CHANNELS {
    metrics.push(query_metric(format!("reuse_targets.recall.{channel}"), move |query| query.reuse.as_ref().and_then(value)));
  }
  metrics.push(query_metric("pack.bytes".to_owned(), |query| Some(query.pack.bytes as f64)));
  metrics.push(query_metric("pack.estimated_tokens".to_owned(), |query| Some(query.pack.estimated_tokens as f64)));
  metrics
}

fn scored_tasks(tasks: &[TaskEntry]) -> Vec<&TaskRetrieval> {
  tasks.iter().filter_map(TaskEntry::scored).collect()
}

pub fn aggregate(tasks: &[TaskEntry]) -> Vec<MetricMean> {
  let scored = scored_tasks(tasks);
  let metrics = query_metrics();
  let mut means = Vec::new();
  for variant in QueryVariant::ALL {
    for (name, value) in &metrics {
      means.push(mean_of(&format!("{}.{name}", variant.as_str()), scored.iter().map(|task| task.value(variant, value))));
    }
  }
  for (name, view) in [("visible", ImpactView::Visible), ("full", ImpactView::Full)] {
    means.push(mean_of(
      &format!("blast_radius.recall.{name}"),
      scored.iter().map(|task| task.blast_radius.as_ref().and_then(|blast| blast.recall(view))),
    ));
  }
  if scored.iter().any(|task| task.rename.is_some()) {
    let sites: [(&str, RenameSelector); 2] = [("recall", |score| score.sites.recall), ("precision", |score| score.sites.precision)];
    for (name, value) in sites {
      means.push(mean_of(
        &format!("rename.{name}"),
        scored.iter().map(|task| task.rename.as_ref().and_then(RenameRetrieval::score).and_then(value)),
      ));
    }
  }
  means
}

pub fn headline(tasks: &[TaskEntry]) -> Vec<HeadlineRow> {
  let scored = scored_tasks(tasks);
  let mut rows = Vec::new();
  for variant in QueryVariant::ALL {
    for section in PackSection::ALL {
      let cells = HeadlineMetric::all()
        .into_iter()
        .map(|metric| mean_of(&metric.key(), scored.iter().map(|task| task.value(variant, |query| metric.value(section, query)))))
        .collect();
      rows.push(HeadlineRow { variant, section, cells });
    }
  }
  rows
}

pub fn compare_with_full(tasks: &[TaskEntry]) -> Vec<PairedComparison> {
  let scored = scored_tasks(tasks);
  let mut rows = Vec::new();
  for variant in QueryVariant::ALL.into_iter().filter(|variant| *variant != QueryVariant::Full) {
    for section in PackSection::ALL {
      for metric in HeadlineMetric::all() {
        let value = |task: &TaskRetrieval, of: QueryVariant| task.value(of, |query| metric.value(section, query));
        let pairs: Vec<(f64, f64)> = scored.iter().filter_map(|task| Some((value(task, QueryVariant::Full)?, value(task, variant)?))).collect();
        let full = mean_of("", pairs.iter().map(|(full, _)| Some(*full))).mean;
        let variant_mean = mean_of("", pairs.iter().map(|(_, other)| Some(*other))).mean;
        rows.push(PairedComparison {
          variant,
          section,
          metric: metric.key(),
          tasks: pairs.len(),
          full,
          variant_mean,
          difference: full.zip(variant_mean).map(|(full, other)| other - full),
          higher: pairs.iter().filter(|(full, other)| other - full > TIE).count(),
          equal: pairs.iter().filter(|(full, other)| (other - full).abs() <= TIE).count(),
          lower: pairs.iter().filter(|(full, other)| full - other > TIE).count(),
        });
      }
    }
  }
  rows
}

fn number(value: Option<f64>) -> String {
  match value {
    None => "n/a".to_owned(),
    Some(value) => {
      let rendered = format!("{value:.3}");
      rendered.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
  }
}

fn signed(value: Option<f64>) -> String {
  let rendered = number(value);
  match value {
    Some(value) if value > 0.0 && rendered != "0" => format!("+{rendered}"),
    Some(_) if rendered == "-0" => "0".to_owned(),
    _ => rendered,
  }
}

fn hits(overlap: &SetOverlap) -> String {
  match overlap.relevant {
    0 => "n/a".to_owned(),
    relevant => format!("{}/{relevant}", overlap.hits),
  }
}

fn scored_cell(overlap: &SetOverlap) -> String {
  format!("{} ({})", number(overlap.recall), hits(overlap))
}

fn counted_mean(mean: &MetricMean) -> String {
  format!("{} ({})", number(mean.mean), mean.tasks)
}

fn plural(count: usize, noun: &str) -> String {
  match count {
    1 => format!("1 {noun}"),
    many => format!("{many} {noun}s"),
  }
}

fn inline_code(text: &str) -> String {
  format!("`{}`", text.replace('`', "'").split_whitespace().collect::<Vec<_>>().join(" "))
}

pub fn render_markdown(report: &RetrievalReport) -> String {
  let mut out = String::new();
  let _ = writeln!(out, "# Retrieval report\n");
  let _ = writeln!(out, "> {}\n", report.caveat);
  let _ = writeln!(
    out,
    "Deterministic and LLM-free: every task's base tree is materialised like the agent harness does it (no setup commands), indexed in process, \
and queried once per query variant below. Ground truth is used only to score, and to pick the blast-radius seed symbols.\n"
  );
  render_settings(&mut out, report);
  render_variants(&mut out, report);
  render_definitions(&mut out, report);
  render_headline(&mut out, report);
  render_comparison(&mut out, report);
  render_aggregate(&mut out, report);
  for task in &report.tasks {
    render_task(&mut out, task);
  }
  out
}

fn render_settings(out: &mut String, report: &RetrievalReport) {
  let settings = &report.settings;
  let _ = writeln!(out, "## Settings\n");
  let _ = writeln!(out, "| key | value |\n|---|---|");
  let revision = report.code_atlas_revision.as_deref().unwrap_or("unknown");
  let budgets: Vec<String> = settings.yield_budgets.iter().map(usize::to_string).collect();
  let rows = [
    ("code atlas revision", revision.to_owned()),
    ("context budget (CLI and MCP default)", settings.context_budget.to_string()),
    ("budgeted-yield budgets", budgets.join(", ")),
    ("search limit per query", settings.search_limit.to_string()),
    (
      "searches",
      format!(
        "the query, up to {} code spans, up to {} prose phrases",
        settings.max_code_spans, settings.max_prose_phrases
      ),
    ),
    ("similar seeds", format!("up to {} top function/method search hits", settings.max_similar_seeds)),
    ("impact group limit (visible)", settings.impact_group_limit.to_string()),
  ];
  for (key, value) in rows {
    let _ = writeln!(out, "| {key} | {value} |");
  }
  if let Some(config) = &settings.atlas_config {
    let _ = writeln!(out, "| atlas configuration | {} (blake3 {}) |", config.files.join(", "), config.blake3);
  }
  let _ = writeln!(out);
}

fn render_variants(out: &mut String, report: &RetrievalReport) {
  let scored = scored_tasks(&report.tasks);
  let _ = writeln!(out, "## Query variants\n");
  let _ = writeln!(
    out,
    "Every variant runs the same pipeline with its own query text in place of the prompt. A task with several queries for a variant is scored on \
each and its scores are averaged; a task with none is `n/a` for that variant and leaves its means.\n"
  );
  let _ = writeln!(
    out,
    "| variant | query | tasks with a query | queries | queries with an empty MUST READ | queries with an empty pack |\n|---|---|---|---|---|---|"
  );
  for variant in QueryVariant::ALL {
    let with_query = scored.iter().filter(|task| !task.queries(variant).is_empty()).count();
    let queries: Vec<&QueryScore> = scored.iter().flat_map(|task| task.queries(variant)).collect();
    let _ = writeln!(
      out,
      "| {} | {} | {with_query} of {} | {} | {} | {} |",
      variant.as_str(),
      variant.definition(),
      scored.len(),
      queries.len(),
      queries.iter().filter(|query| query.pack.must_read == 0).count(),
      queries.iter().filter(|query| query.pack.items() == 0).count()
    );
  }
  let _ = writeln!(out);
  let sent: BTreeSet<&Provenance> = scored
    .iter()
    .flat_map(|task| task.queries(QueryVariant::AgentSeeded))
    .flat_map(|query| &query.query.sent_by)
    .collect();
  let silent: BTreeSet<&Provenance> = scored.iter().flat_map(|task| &task.no_call).collect();
  let batches: BTreeSet<&str> = sent.iter().chain(&silent).map(|run| run.batch.as_str()).collect();
  let _ = writeln!(
    out,
    "- Recorded calls: `eval/tasks/<task>/queries.toml`, from {} ({} sent an `atlas_context` call, {} sent none).",
    match batches.is_empty() {
      true => "no batch".to_owned(),
      false => batches.iter().map(|batch| format!("batch `{batch}`")).collect::<Vec<_>>().join(", "),
    },
    plural(sent.len(), "run"),
    plural(silent.len(), "run")
  );
  let _ = writeln!(
    out,
    "- `agent-seeded` differs from `agent` only in `context`, which receives the seeds; `search` and `similar` take the same text. It equals `agent` \
on a task whose recorded calls carry no seeds."
  );
  let _ = writeln!(
    out,
    "- First sentence: the prompt's first paragraph (the text before the first blank line, lines joined by a space), cut before the first `.`, `?` \
or `!` that follows a letter or digit, lies outside a backtick code span and is followed by whitespace or the end of the paragraph. Without such \
a character the whole paragraph is the query.\n"
  );
}

fn render_definitions(out: &mut String, report: &RetrievalReport) {
  let weight = F_BETA * F_BETA;
  let _ = writeln!(out, "## Metric definitions\n");
  let _ = writeln!(
    out,
    "- Recall of `must_change` and precision against `relevant_files` are per channel and unchanged from earlier reports. Precision is undefined \
(`n/a`) for a channel that returned no file, so an empty MUST READ section leaves the precision mean instead of lowering it."
  );
  let _ = writeln!(
    out,
    "- F{F_BETA} = {} x hits / ({weight} x gold + retrieved), which equals (1 + b^2) x P x R / (b^2 x P + R) with b = {F_BETA} and weights precision \
twice as heavily as recall. It is 0 when no gold item is retrieved, an empty channel included, and `n/a` only when the gold set is empty.",
    1.0 + weight
  );
  let _ = writeln!(
    out,
    "- File F{F_BETA} takes one gold set for both precision and recall: the `relevant_files` present in the index. It is the only labelled set these \
reports already score for both, so the value follows from the `relevant_files` precision and recall columns. A `must_change` file that the task \
does not also list in `relevant_files` counts as a false positive here, as it does in the precision column. `must_change.f05.*` in the aggregate \
is the strict reading with `must_change` as the gold set."
  );
  let _ = writeln!(
    out,
    "- Line F{F_BETA} (MUST READ and whole pack only). Gold: the old-side lines `reference.patch` changes in files of the base tree, that is every \
removed line plus, for an insertion that replaces nothing, the old lines directly before and after it. Files the patch adds have no base lines; \
they are left out and listed per task. Gold lines in base files the index does not hold stay in the gold set and are counted per task. Retrieved: \
the union of the line ranges the pack items state, whatever part of them the pack prints: the symbol span for MUST READ, SKIM and TESTS items, and \
for a one-line mention the reference site that put it in the pack when that site lies inside it; an item that states no range retrieves its \
entity's indexed span, and one whose entity has no span retrieves no line and is counted. Precision, recall and F{F_BETA} are over (file, line) pairs."
  );
  let budgets: Vec<String> = report.settings.yield_budgets.iter().map(usize::to_string).collect();
  let _ = writeln!(
    out,
    "- Budgeted yield: recall of `must_change` when `context` is asked again with `budget` set to {} tokens, for MUST READ and for the whole pack. \
The budget is the query's own parameter and the product's own token estimate decides what fits; each task lists the estimate and the rendered \
bytes at every budget.\n",
    budgets.join(", ")
  );
}

fn render_headline(out: &mut String, report: &RetrievalReport) {
  let scored = scored_tasks(&report.tasks).len();
  let _ = writeln!(out, "## Headline: `context` by query variant\n");
  let _ = writeln!(
    out,
    "Mean over the tasks that define the metric, with that task count in parentheses ({scored} of {} tasks were scored). The first four metrics \
are at the default budget.\n",
    report.tasks.len()
  );
  let titles: Vec<String> = HeadlineMetric::all().into_iter().map(HeadlineMetric::title).collect();
  let _ = writeln!(out, "| variant | section | {} |", titles.join(" | "));
  let _ = writeln!(out, "|---|---|{}", "---|".repeat(titles.len()));
  for row in &report.headline {
    let cells: Vec<String> = row.cells.iter().map(counted_mean).collect();
    let _ = writeln!(out, "| {} | {} | {} |", row.variant.as_str(), row.section.title(), cells.join(" | "));
  }
  let _ = writeln!(
    out,
    "\nWhat the budget changes, counted over queries: a pack estimate or a `must_change` recall changes when it is not the same at every yield budget.\n"
  );
  let _ = writeln!(
    out,
    "| variant | queries | pack estimate changes | MUST READ recall changes | whole-pack recall changes |\n|---|---|---|---|---|"
  );
  for variant in QueryVariant::ALL {
    let queries: Vec<&QueryScore> = scored_tasks(&report.tasks).into_iter().flat_map(|task| task.queries(variant)).collect();
    let recall_changes = |section: PackSection| queries.iter().filter(|query| varies(query.yields.iter().map(|at| section.yielded(at).hits))).count();
    let _ = writeln!(
      out,
      "| {} | {} | {} | {} | {} |",
      variant.as_str(),
      queries.len(),
      queries.iter().filter(|query| varies(query.yields.iter().map(|at| at.estimated_tokens))).count(),
      recall_changes(PackSection::MustRead),
      recall_changes(PackSection::Pack)
    );
  }
  let _ = writeln!(out);
}

fn varies(values: impl Iterator<Item = usize>) -> bool {
  let values: Vec<usize> = values.collect();
  values.windows(2).any(|pair| pair[0] != pair[1])
}

fn verdict(row: &PairedComparison) -> &'static str {
  match row.difference {
    None => "cannot be compared with",
    Some(difference) if difference > TIE => "is higher than",
    Some(difference) if difference < -TIE => "is lower than",
    Some(_) => "equals",
  }
}

fn render_comparison(out: &mut String, report: &RetrievalReport) {
  let _ = writeln!(out, "## Short queries against the full prompt\n");
  let _ = writeln!(
    out,
    "Paired: each row uses only the tasks that define the metric for both `full` and the variant, so its `full` mean can differ from the headline. \
The last three columns count the tasks on which the variant is higher than, equal to and lower than `full`.\n"
  );
  let stated = [HeadlineMetric::MustChangeRecall.key(), HeadlineMetric::FileF05.key()];
  for row in report.comparison.iter().filter(|row| stated.contains(&row.metric)) {
    let _ = writeln!(
      out,
      "- `{}` {} `full` on {} {}: {} against {} over {} (higher on {}, equal on {}, lower on {}).",
      row.variant.as_str(),
      verdict(row),
      row.section.title(),
      row.metric,
      number(row.variant_mean),
      number(row.full),
      plural(row.tasks, "task"),
      row.higher,
      row.equal,
      row.lower
    );
  }
  let _ = writeln!(
    out,
    "\n| variant | section | metric | tasks | full | variant | difference | higher | equal | lower |\n|---|---|---|---|---|---|---|---|---|---|"
  );
  for row in &report.comparison {
    let _ = writeln!(
      out,
      "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
      row.variant.as_str(),
      row.section.title(),
      row.metric,
      row.tasks,
      number(row.full),
      number(row.variant_mean),
      signed(row.difference),
      row.higher,
      row.equal,
      row.lower
    );
  }
  let _ = writeln!(out);
}

fn render_aggregate(out: &mut String, report: &RetrievalReport) {
  let scored = scored_tasks(&report.tasks).len();
  let _ = writeln!(out, "## Aggregate (mean over the {scored} of {} tasks that define each metric)\n", report.tasks.len());
  let _ = writeln!(
    out,
    "Metric names start with the query variant; `full.*` is what earlier reports listed without a prefix.\n"
  );
  let _ = writeln!(out, "| metric | mean | tasks |\n|---|---|---|");
  for mean in &report.aggregate {
    let _ = writeln!(out, "| {} | {} | {} |", mean.metric, number(mean.mean), mean.tasks);
  }
  let _ = writeln!(out);
}

fn render_task(out: &mut String, task: &TaskEntry) {
  let _ = writeln!(out, "## {}: {}\n", task.id, task.title);
  let _ = writeln!(out, "Categories: {}.\n", task.categories.join(", "));
  let retrieval = match &task.outcome {
    TaskOutcome::Failed { stage, reason } => {
      let _ = writeln!(out, "Not scored: `{stage}` failed: {reason}\n");
      return;
    }
    TaskOutcome::Scored(retrieval) => retrieval,
  };
  let index = &retrieval.index;
  let _ = writeln!(out, "Index: {} files in {}.", index.files, index.repos.join(", "));
  if let Some(reason) = &index.degraded {
    let _ = writeln!(out, "**Index degraded:** {reason}.");
  }
  render_line_gold(out, &retrieval.line_gold);
  if !retrieval.no_call.is_empty() {
    let _ = writeln!(out, "Runs that sent no `atlas_context` call: {}.", runs(&retrieval.no_call));
  }
  let _ = writeln!(out);
  render_task_summary(out, retrieval);
  for result in &retrieval.variants {
    render_variant(out, retrieval, result);
  }
  if let Some(blast) = &retrieval.blast_radius {
    render_blast(out, blast);
  }
  if let Some(rename) = &retrieval.rename {
    render_rename(out, rename);
  }
}

fn runs(runs: &[Provenance]) -> String {
  runs.iter().map(|run| format!("`{}/{}`", run.batch, run.run)).collect::<Vec<_>>().join(", ")
}

fn render_line_gold(out: &mut String, gold: &LineGoldSummary) {
  match gold {
    LineGoldSummary::Unavailable { reason } => {
      let _ = writeln!(out, "Line gold: none ({reason}).");
    }
    LineGoldSummary::Available {
      lines,
      lines_outside_index,
      files,
      added_files,
      absent_files,
    } => {
      let _ = writeln!(
        out,
        "Line gold: {lines} old-side lines in {} base files of `reference.patch`, {lines_outside_index} of them in files outside the index.",
        files.len()
      );
      let outside: Vec<String> = files.iter().filter(|file| !file.indexed).map(|file| file.path.clone()).collect();
      for (label, paths) in [
        ("- gold files outside the index", &outside),
        ("- added by the patch, no base lines (not scored)", added_files),
        ("- changed by the patch but absent from the base tree (not scored)", absent_files),
      ] {
        if !paths.is_empty() {
          let _ = writeln!(out, "{label}: {}", code_list(paths));
        }
      }
    }
  }
}

fn render_task_summary(out: &mut String, retrieval: &TaskRetrieval) {
  let titles: Vec<String> = HeadlineMetric::all().into_iter().map(HeadlineMetric::title).collect();
  let _ = writeln!(out, "| variant | queries | section | {} |", titles.join(" | "));
  let _ = writeln!(out, "|---|---|---|{}", "---|".repeat(titles.len()));
  for result in &retrieval.variants {
    let variant = result.variant;
    let count = retrieval.queries(variant).len();
    for section in PackSection::ALL {
      let cells: Vec<String> = HeadlineMetric::all()
        .into_iter()
        .map(|metric| number(retrieval.value(variant, |query| metric.value(section, query))))
        .collect();
      let _ = writeln!(out, "| {} | {count} | {} | {} |", variant.as_str(), section.title(), cells.join(" | "));
    }
  }
  let _ = writeln!(out);
}

fn render_variant(out: &mut String, retrieval: &TaskRetrieval, result: &VariantResult) {
  let name = result.variant.as_str();
  let queries = match &result.outcome {
    VariantOutcome::Unavailable { reason } => {
      let _ = writeln!(out, "### `{name}`\n\nn/a: {reason}.\n");
      return;
    }
    VariantOutcome::Scored { queries } => queries,
  };
  if result.variant == QueryVariant::AgentSeeded && queries.iter().all(|query| query.query.seeds.is_empty()) {
    let _ = writeln!(out, "### `{name}`\n\nSame as `agent`: no recorded call of this task carries seeds.\n");
    return;
  }
  for (index, query) in queries.iter().enumerate() {
    match queries.len() {
      1 => {
        let _ = writeln!(out, "### `{name}`\n");
      }
      total => {
        let _ = writeln!(out, "### `{name}`, query {} of {total}\n", index + 1);
      }
    }
    render_query(out, result.variant, query, &retrieval.line_gold);
  }
}

fn render_query(out: &mut String, variant: QueryVariant, score: &QueryScore, gold: &LineGoldSummary) {
  let query = &score.query;
  let text = match variant {
    QueryVariant::Full => format!("the task prompt ({} characters)", query.text.chars().count()),
    _ => inline_code(&query.text),
  };
  let seeds = match query.seeds.is_empty() {
    true => String::new(),
    false => format!("; seeds {}", code_list(&query.seeds)),
  };
  let sent_by = match query.sent_by.is_empty() {
    true => String::new(),
    false => format!("; sent by {}", runs(&query.sent_by)),
  };
  let _ = writeln!(out, "Query: {text}{seeds}{sent_by}.");
  let pack = &score.pack;
  let _ = writeln!(
    out,
    "Context pack: {} bytes, ~{} of {} tokens; items: must read {}, skim {}, patterns {}, reuse {}, impact {}, tests {}, uncertain {}, omitted {}.\n",
    pack.bytes, pack.estimated_tokens, pack.budget, pack.must_read, pack.skim, pack.patterns, pack.reuse, pack.impact, pack.tests, pack.uncertain, pack.omitted
  );
  render_searches(out, &score.searches);
  render_files(out, score);
  if let Some(lines) = &score.lines {
    render_lines(out, lines, gold);
  }
  render_yields(out, &score.yields);
  if let Some(reuse) = &score.reuse {
    render_reuse(out, reuse);
  }
}

fn render_rename(out: &mut String, rename: &RenameRetrieval) {
  match rename {
    RenameRetrieval::Unavailable { reason } => {
      let _ = writeln!(out, "Rename: not scored ({reason}).\n");
    }
    RenameRetrieval::Scored { target, new_name, servers, score } => {
      let _ = writeln!(
        out,
        "Rename `{target}` -> `{new_name}` as a dry run ({}) against the old-side lines of `reference.patch` where the old name becomes the new one: recall {} ({}), precision {}.",
        if servers.is_empty() { "no server".to_owned() } else { servers.join(", ") },
        number(score.sites.recall),
        hits(&score.sites),
        number(score.sites.precision)
      );
      if !score.missed.is_empty() {
        let _ = writeln!(out, "- patch sites the plan does not edit: {}", code_list(&score.missed));
      }
      if !score.unexpected.is_empty() {
        let _ = writeln!(out, "- planned edits the patch does not make: {}", code_list(&score.unexpected));
      }
      let _ = writeln!(out);
    }
  }
}

fn render_searches(out: &mut String, queries: &QueryLog) {
  let phrases: Vec<String> = queries.search_phrases.iter().map(|phrase| format!("`{}`", phrase.text.replace('`', "'"))).collect();
  let _ = writeln!(
    out,
    "Searches: the query, then {}.",
    if phrases.is_empty() { "nothing else".to_owned() } else { phrases.join(", ") }
  );
  let seeds: Vec<String> = queries
    .similar_seeds
    .iter()
    .map(|seed| match &seed.outcome {
      SeedOutcome::Found { members, shape_only } => format!("`{}` (members {members}, shape-only {shape_only})", seed.key),
      SeedOutcome::NotFound => format!("`{}` (not found)", seed.key),
      SeedOutcome::Ambiguous { candidates } => format!("`{}` (ambiguous, {candidates} candidates)", seed.key),
    })
    .collect();
  let _ = writeln!(out, "Similar seeds: {}.\n", if seeds.is_empty() { "none".to_owned() } else { seeds.join(", ") });
}

fn render_files(out: &mut String, score: &QueryScore) {
  let _ = writeln!(out, "| files | must read | pack | search | similar | any |\n|---|---|---|---|---|---|");
  for (set, select) in FILE_SETS {
    let files = select(score);
    let cells: Vec<String> = CHANNELS.iter().map(|(_, overlap, _)| scored_cell(overlap(files))).collect();
    let _ = writeln!(out, "| {set} recall | {} |", cells.join(" | "));
  }
  let precision: Vec<String> = CHANNELS.iter().map(|(_, overlap, _)| number(overlap(&score.relevant_files).precision)).collect();
  let _ = writeln!(out, "| relevant_files precision | {} |", precision.join(" | "));
  for (set, select) in F05_SETS {
    let cells: Vec<String> = CHANNELS.iter().map(|(_, _, f05)| number(f05(select(score)))).collect();
    let _ = writeln!(out, "| {set} F{F_BETA} | {} |", cells.join(" | "));
  }
  let _ = writeln!(out);
  let mut notes = Vec::new();
  for (set, select) in FILE_SETS {
    let files = select(score);
    if !files.missed.is_empty() {
      notes.push(format!("- {set} not retrieved anywhere: {}", code_list(&files.missed)));
    }
    if !files.absent_at_base.is_empty() {
      notes.push(format!("- {set} absent from the base tree (not scored): {}", code_list(&files.absent_at_base)));
    }
    if !files.unindexable.is_empty() {
      notes.push(format!("- {set} unindexable, never in the index (not scored): {}", code_list(&files.unindexable)));
    }
  }
  if !notes.is_empty() {
    let _ = writeln!(out, "{}\n", notes.join("\n"));
  }
}

fn render_lines(out: &mut String, lines: &LineScores, gold: &LineGoldSummary) {
  if !matches!(gold, LineGoldSummary::Available { lines, .. } if *lines > 0) {
    return;
  }
  let _ = writeln!(
    out,
    "| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F{F_BETA} |\n|---|---|---|---|---|---|---|---|"
  );
  for section in PackSection::ALL {
    let channel = section.lines(lines);
    let _ = writeln!(
      out,
      "| {} | {} | {} | {} | {} | {} | {} | {} |",
      section.title(),
      channel.items,
      channel.items_without_span,
      channel.lines.retrieved,
      hits(&channel.lines),
      number(channel.lines.precision),
      number(channel.lines.recall),
      number(channel.f05)
    );
  }
  let _ = writeln!(out);
}

fn render_yields(out: &mut String, yields: &[BudgetYield]) {
  let _ = writeln!(
    out,
    "| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |\n|---|---|---|---|---|"
  );
  for at in yields {
    let _ = writeln!(
      out,
      "| {} | {} | {} | {} | {} |",
      at.budget,
      at.estimated_tokens,
      at.bytes,
      scored_cell(&at.must_read),
      scored_cell(&at.pack)
    );
  }
  let _ = writeln!(out);
}

fn render_reuse(out: &mut String, reuse: &ReuseScore) {
  let _ = writeln!(out, "| reuse target | pack | search | similar |\n|---|---|---|---|");
  let mark = |found: bool| if found { "yes" } else { "no" };
  for hit in &reuse.targets {
    let _ = writeln!(out, "| `{}` | {} | {} | {} |", hit.target, mark(hit.pack), mark(hit.search), mark(hit.similar));
  }
  let _ = writeln!(out);
}

fn render_blast(out: &mut String, blast: &BlastRadius) {
  match blast {
    BlastRadius::Unavailable { reason } => {
      let _ = writeln!(out, "Blast radius: not scored ({reason}).\n");
    }
    BlastRadius::Scored {
      definition_file,
      seeds,
      expected,
      visible,
      full,
      collapsed_visible,
      missed,
    } => {
      let _ = writeln!(
        out,
        "Blast radius: `impact` on {} (symbols in `{definition_file}` that `reference.patch` changes) against the {} other must_change files. \
Precision is against must_change only, so it is low by design; recall is the score.\n",
        code_list(seeds),
        expected.len()
      );
      let _ = writeln!(out, "| impact view | recall | hits | files retrieved | precision |\n|---|---|---|---|---|");
      let views = [(format!("visible ({collapsed_visible} items collapsed)"), visible), ("full".to_owned(), full)];
      for (view, overlap) in views {
        let _ = writeln!(
          out,
          "| {view} | {} | {} | {} | {} |",
          number(overlap.recall),
          hits(overlap),
          overlap.retrieved,
          number(overlap.precision)
        );
      }
      if !missed.is_empty() {
        let _ = writeln!(out, "\n- not reached: {}", code_list(missed));
      }
      let _ = writeln!(out);
    }
  }
}

fn code_list(items: &[String]) -> String {
  items.iter().map(|item| format!("`{item}`")).collect::<Vec<_>>().join(", ")
}

pub fn write_report(report: &RetrievalReport, out_base: &Path) -> RetrievalResult<(PathBuf, PathBuf)> {
  if let Some(parent) = out_base.parent().filter(|parent| !parent.as_os_str().is_empty()) {
    fs::create_dir_all(parent).map_err(|error| RetrievalError::io(parent, error))?;
  }
  let markdown = out_base.with_extension("md");
  let json = out_base.with_extension("json");
  fs::write(&markdown, render_markdown(report)).map_err(|error| RetrievalError::io(&markdown, error))?;
  fs::write(&json, serde_json::to_string_pretty(report)? + "\n").map_err(|error| RetrievalError::io(&json, error))?;
  Ok((markdown, json))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::retrieval::lines::{ItemSpans, LineSet, score_lines};
  use crate::retrieval::queries::NO_RECORDED_QUERY;
  use crate::retrieval::score::{BaseFile, PackCost, Retrieved, budget_yield, counted_overlap, score_files};

  fn files(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|path| (*path).to_owned()).collect()
  }

  fn query(text: &str, must_read: &[&str], pack: &[&str]) -> QueryScore {
    let retrieved = Retrieved {
      must_read_files: files(must_read),
      pack_files: files(pack),
      ..Retrieved::default()
    };
    let expected = ["a.rs".to_owned(), "b.rs".to_owned()];
    let cost = |budget| PackCost {
      budget,
      estimated_tokens: 30,
      bytes: 100,
    };
    QueryScore {
      query: VariantQuery {
        text: text.to_owned(),
        seeds: Vec::new(),
        sent_by: Vec::new(),
      },
      searches: QueryLog {
        search_phrases: Vec::new(),
        similar_seeds: Vec::new(),
      },
      pack: PackSize {
        bytes: 100,
        estimated_tokens: 30,
        budget: 6000,
        must_read: must_read.len(),
        skim: pack.len().saturating_sub(must_read.len()),
        ..PackSize::default()
      },
      must_change: score_files(&retrieved, &expected[..1], |_| BaseFile::Indexed),
      relevant_files: score_files(&retrieved, &expected, |_| BaseFile::Indexed),
      must_consider: score_files(&retrieved, &[], |_| BaseFile::Indexed),
      reuse: None,
      lines: None,
      yields: YIELD_BUDGETS.iter().map(|budget| budget_yield(cost(*budget), &retrieved, &files(&["a.rs"]))).collect(),
    }
  }

  fn scored(queries: Vec<QueryScore>) -> VariantOutcome {
    VariantOutcome::Scored { queries }
  }

  fn unavailable() -> VariantOutcome {
    VariantOutcome::Unavailable {
      reason: NO_RECORDED_QUERY.to_owned(),
    }
  }

  fn task_with(agent: VariantOutcome, blast_radius: Option<BlastRadius>) -> TaskRetrieval {
    let outcomes = [
      scored(vec![query("the prompt", &["a.rs"], &["a.rs", "b.rs"])]),
      agent.clone(),
      agent,
      scored(vec![query("first sentence", &["a.rs"], &["a.rs", "b.rs"])]),
    ];
    TaskRetrieval {
      index: IndexSummary {
        repos: vec!["tree".to_owned()],
        files: 2,
        degraded: None,
      },
      line_gold: LineGoldSummary::Unavailable {
        reason: "the task has no readable reference.patch".to_owned(),
      },
      no_call: Vec::new(),
      variants: QueryVariant::ALL
        .into_iter()
        .zip(outcomes)
        .map(|(variant, outcome)| VariantResult { variant, outcome })
        .collect(),
      blast_radius,
      rename: None,
    }
  }

  fn scored_task(blast_radius: Option<BlastRadius>) -> TaskRetrieval {
    task_with(unavailable(), blast_radius)
  }

  fn entry(id: &str, outcome: TaskOutcome) -> TaskEntry {
    TaskEntry {
      id: id.to_owned(),
      title: format!("{id} title"),
      categories: vec!["blast-radius".to_owned()],
      outcome,
    }
  }

  fn scored_entry(id: &str, task: TaskRetrieval) -> TaskEntry {
    entry(id, TaskOutcome::Scored(Box::new(task)))
  }

  fn mean<'a>(means: &'a [MetricMean], metric: &str) -> &'a MetricMean {
    means.iter().find(|mean| mean.metric == metric).unwrap()
  }

  fn counted(means: &[MetricMean], metric: &str) -> (usize, Option<f64>) {
    (mean(means, metric).tasks, mean(means, metric).mean)
  }

  fn settings() -> RetrievalSettings {
    RetrievalSettings {
      atlas_config: None,
      context_budget: 6000,
      search_limit: 10,
      max_code_spans: 8,
      max_prose_phrases: 4,
      max_similar_seeds: 5,
      impact_group_limit: 15,
      yield_budgets: YIELD_BUDGETS.to_vec(),
    }
  }

  #[test]
  fn should_aggregate_scored_tasks_only_and_skip_undefined_metrics() {
    let full = SetOverlap {
      retrieved: 1,
      relevant: 2,
      hits: 1,
      precision: Some(1.0),
      recall: Some(0.5),
    };
    let blast = BlastRadius::Scored {
      definition_file: "a.rs".to_owned(),
      seeds: vec!["k".to_owned()],
      expected: vec!["b.rs".to_owned(), "c.rs".to_owned()],
      visible: full.clone(),
      full,
      collapsed_visible: 0,
      missed: vec!["c.rs".to_owned()],
    };
    let tasks = vec![
      scored_entry("t1", scored_task(Some(blast))),
      scored_entry("t2", scored_task(Some(BlastRadius::Unavailable { reason: "no patch".to_owned() }))),
      entry(
        "t3",
        TaskOutcome::Failed {
          stage: "source".to_owned(),
          reason: "offline".to_owned(),
        },
      ),
    ];
    let means = aggregate(&tasks);
    assert_eq!(counted(&means, "full.must_change.recall.must_read"), (2, Some(1.0)));
    assert_eq!(mean(&means, "full.relevant_files.recall.must_read").mean, Some(0.5));
    assert_eq!(mean(&means, "full.relevant_files.precision.pack").mean, Some(1.0));
    assert_eq!(mean(&means, "full.relevant_files.f05.must_read").mean, Some(1.25 / 1.5));
    assert_eq!(mean(&means, "full.must_change.f05.pack").mean, Some(1.25 / 2.25));
    assert_eq!(counted(&means, "full.must_consider.recall.pack"), (0, None));
    assert_eq!(counted(&means, "full.yield.4000.must_read"), (2, Some(1.0)));
    assert_eq!(counted(&means, "full.lines.f05.pack"), (0, None));
    assert_eq!(counted(&means, "blast_radius.recall.full"), (1, Some(0.5)));
    assert_eq!(mean(&means, "full.pack.bytes").mean, Some(100.0));
    assert_eq!(counted(&means, "first-line.must_change.recall.must_read"), (2, Some(1.0)));
  }

  #[test]
  fn should_average_a_task_over_its_agent_queries_and_leave_tasks_without_one_out_of_the_mean() {
    let two_queries = scored(vec![query("one", &["a.rs"], &["a.rs"]), query("two", &[], &["c.rs"])]);
    let tasks = vec![scored_entry("t1", task_with(two_queries, None)), scored_entry("t2", scored_task(None))];

    let means = aggregate(&tasks);

    assert_eq!(counted(&means, "agent.must_change.recall.must_read"), (1, Some(0.5)));
    assert_eq!(counted(&means, "agent.relevant_files.precision.must_read"), (1, Some(1.0)));
    assert_eq!(counted(&means, "agent.relevant_files.f05.must_read"), (1, Some(1.25 / 1.5 / 2.0)));
    assert_eq!(counted(&means, "agent.relevant_files.f05.pack"), (1, Some(1.25 / 1.5 / 2.0)));
    assert_eq!(counted(&means, "agent.yield.16000.pack"), (1, Some(0.5)));
    assert_eq!(counted(&means, "agent-seeded.must_change.recall.pack"), (1, Some(0.5)));
    assert_eq!(counted(&means, "full.must_change.recall.must_read"), (2, Some(1.0)));
    let mut markdown = String::new();
    render_task(&mut markdown, &tasks[1]);
    assert!(
      markdown.contains("### `agent`\n\nn/a: no recorded `atlas_context` call: queries.toml is missing or lists none.\n"),
      "{markdown}"
    );
    assert!(markdown.contains("| agent | 0 | MUST READ | n/a | n/a | n/a | n/a | n/a | n/a | n/a |\n"), "{markdown}");
    let mut averaged = String::new();
    render_task(&mut averaged, &tasks[0]);
    assert!(averaged.contains("| agent | 2 | MUST READ | 0.5 | 1 | 0.417 | n/a | 0.5 | 0.5 | 0.5 |\n"), "{averaged}");
    assert!(averaged.contains("### `agent`, query 2 of 2\n\nQuery: `two`.\n"), "{averaged}");
    assert!(
      averaged.contains("### `agent-seeded`\n\nSame as `agent`: no recorded call of this task carries seeds.\n"),
      "{averaged}"
    );
  }

  #[test]
  fn should_compare_each_short_variant_with_full_on_the_tasks_that_define_both() {
    let worse = scored(vec![query("one", &[], &["a.rs", "c.rs", "d.rs"])]);
    let tasks = vec![scored_entry("t1", task_with(worse, None)), scored_entry("t2", scored_task(None))];
    let report = RetrievalReport::new(None, settings(), tasks);
    let row = |variant: QueryVariant, section: PackSection, metric: &str| {
      report
        .comparison
        .iter()
        .find(|row| row.variant == variant && row.section == section && row.metric == metric)
        .unwrap()
    };

    let recall = row(QueryVariant::Agent, PackSection::MustRead, "must_change.recall");
    assert_eq!((recall.tasks, recall.full, recall.variant_mean, recall.difference), (1, Some(1.0), Some(0.0), Some(-1.0)));
    assert_eq!((recall.higher, recall.equal, recall.lower), (0, 0, 1));
    let precision = row(QueryVariant::Agent, PackSection::MustRead, "relevant_files.precision");
    assert_eq!((precision.tasks, precision.full, precision.variant_mean, precision.difference), (0, None, None, None));
    let pack = row(QueryVariant::Agent, PackSection::Pack, "relevant_files.f05");
    assert_eq!((pack.tasks, pack.full, pack.variant_mean), (1, Some(1.0), Some(1.25 / 3.5)));
    let same = row(QueryVariant::FirstLine, PackSection::Pack, "yield.8000");
    assert_eq!((same.tasks, same.difference, same.higher, same.equal, same.lower), (2, Some(0.0), 0, 2, 0));
    assert_eq!(row(QueryVariant::Agent, PackSection::Pack, "lines.f05").tasks, 0);

    let markdown = render_markdown(&report);
    assert!(
      markdown.contains("- `agent` is lower than `full` on MUST READ must_change.recall: 0 against 1 over 1 task (higher on 0, equal on 0, lower on 1).\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("- `first-line` equals `full` on whole pack relevant_files.f05: 1 against 1 over 2 tasks (higher on 0, equal on 2, lower on 0).\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("| agent | whole pack | relevant_files.f05 | 1 | 1 | 0.357 | -0.643 | 0 | 0 | 1 |\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("| agent | MUST READ | relevant_files.precision | 0 | n/a | n/a | n/a | 0 | 0 | 0 |\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("| agent | MUST READ | 0 (1) | n/a (0) | 0 (1) | n/a (0) | 0 (1) | 0 (1) | 0 (1) |\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("| full | whole pack | 1 (2) | 1 (2) | 1 (2) | n/a (0) | 1 (2) | 1 (2) | 1 (2) |\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("| agent | the `task` argument of each distinct recorded `atlas_context` call, without its seeds | 1 of 2 | 1 | 1 | 0 |\n"),
      "{markdown}"
    );
    assert!(markdown.contains("| full | the whole task prompt | 2 of 2 | 2 | 0 | 0 |\n"), "{markdown}");
    assert!(markdown.contains("| full | 2 | 0 | 0 | 0 |\n| agent | 1 | 0 | 0 | 0 |\n"), "{markdown}");
  }

  #[test]
  fn should_render_line_scores_with_the_gold_summary_and_the_recorded_runs() {
    let gold: LineSet = [("a.rs".to_owned(), 3), ("a.rs".to_owned(), 4), ("docs.md".to_owned(), 9), ("docs.md".to_owned(), 10)].into();
    let mut must_read = ItemSpans::default();
    must_read.add(Some(("a.rs".to_owned(), 1, 4)));
    let mut pack = must_read.clone();
    pack.add(None);
    let mut agent = query("Replace the `reason`\nString", &["a.rs"], &["a.rs"]);
    agent.query.seeds = vec!["FreshnessReport".to_owned()];
    agent.query.sent_by = vec![Provenance {
      batch: "b1".to_owned(),
      run: "t1.atlas.r0".to_owned(),
    }];
    agent.lines = Some(LineScores {
      must_read: score_lines(&must_read, &gold),
      pack: score_lines(&pack, &gold),
    });
    let mut task = task_with(scored(vec![agent]), None);
    task.line_gold = LineGoldSummary::Available {
      lines: 4,
      lines_outside_index: 2,
      files: vec![
        crate::retrieval::lines::GoldFile {
          path: "a.rs".to_owned(),
          lines: 2,
          indexed: true,
        },
        crate::retrieval::lines::GoldFile {
          path: "docs.md".to_owned(),
          lines: 2,
          indexed: false,
        },
      ],
      added_files: vec!["new.rs".to_owned()],
      absent_files: Vec::new(),
    };
    task.no_call = vec![Provenance {
      batch: "b1".to_owned(),
      run: "t1.atlas.r1".to_owned(),
    }];
    let tasks = vec![scored_entry("t1", task)];

    let means = aggregate(&tasks);
    assert_eq!(counted(&means, "agent-seeded.lines.precision.pack"), (1, Some(0.5)));
    assert_eq!(counted(&means, "agent-seeded.lines.recall.pack"), (1, Some(0.5)));
    assert_eq!(counted(&means, "agent-seeded.lines.f05.must_read"), (1, Some(0.5)));
    assert_eq!(counted(&means, "full.lines.f05.must_read"), (0, None));

    let markdown = render_markdown(&RetrievalReport::new(Some("abc".to_owned()), settings(), tasks));
    assert!(
      markdown.contains(
        "Line gold: 4 old-side lines in 2 base files of `reference.patch`, 2 of them in files outside the index.\n\
- gold files outside the index: `docs.md`\n\
- added by the patch, no base lines (not scored): `new.rs`\n\
Runs that sent no `atlas_context` call: `b1/t1.atlas.r1`.\n"
      ),
      "{markdown}"
    );
    assert!(
      markdown.contains("### `agent-seeded`\n\nQuery: `Replace the 'reason' String`; seeds `FreshnessReport`; sent by `b1/t1.atlas.r0`.\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("| MUST READ | 1 | 0 | 4 | 2/4 | 0.5 | 0.5 | 0.5 |\n| whole pack | 2 | 1 | 4 | 2/4 | 0.5 | 0.5 | 0.5 |\n"),
      "{markdown}"
    );
    assert!(markdown.contains("| 4000 | 30 | 100 | 1 (1/1) | 1 (1/1) |\n"), "{markdown}");
    assert!(
      markdown.contains("- Recorded calls: `eval/tasks/<task>/queries.toml`, from batch `b1` (1 run sent an `atlas_context` call, 1 run sent none).\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("| code atlas revision | abc |\n| context budget (CLI and MCP default) | 6000 |\n| budgeted-yield budgets | 4000, 8000, 16000 |\n"),
      "{markdown}"
    );
    assert!(markdown.contains("- F0.5 = 1.25 x hits / (0.25 x gold + retrieved), which equals"), "{markdown}");
  }

  #[test]
  fn should_name_the_seeded_atlas_configuration_in_the_settings() {
    let mut configured = settings();
    configured.atlas_config = Some(AtlasConfigRecord {
      blake3: "d1".to_owned(),
      files: vec!["config.toml".to_owned(), "detectors/corvid-http.toml".to_owned()],
      keys: vec!["defaults.detect.route_families".to_owned()],
    });

    let markdown = render_markdown(&RetrievalReport::new(None, configured, Vec::new()));

    assert!(
      markdown.contains("| impact group limit (visible) | 15 |\n| atlas configuration | config.toml, detectors/corvid-http.toml (blake3 d1) |\n"),
      "{markdown}"
    );
  }

  #[test]
  fn should_render_failed_tasks_and_unavailable_blast_radius_without_scores() {
    let tasks = vec![
      scored_entry("t2", scored_task(Some(BlastRadius::Unavailable { reason: "no patch".to_owned() }))),
      entry(
        "t3",
        TaskOutcome::Failed {
          stage: "source".to_owned(),
          reason: "offline".to_owned(),
        },
      ),
    ];
    let markdown = render_markdown(&RetrievalReport::new(None, settings(), tasks));
    assert!(
      markdown.contains("| must_change recall | 1 (1/1) | 1 (1/1) | 0 (0/1) | 0 (0/1) | 1 (1/1) |\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("| must_consider recall | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) |\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("| relevant_files F0.5 | 0.833 | 1 | 0 | 0 | 1 |\n| must_change F0.5 | 1 | 0.556 | 0 | 0 | 0.556 |\n"),
      "{markdown}"
    );
    assert!(markdown.contains("Line gold: none (the task has no readable reference.patch).\n"), "{markdown}");
    assert!(markdown.contains("### `full`\n\nQuery: the task prompt (10 characters).\n"), "{markdown}");
    assert!(markdown.contains("Blast radius: not scored (no patch).\n"), "{markdown}");
    assert!(
      markdown.contains("## t3: t3 title\n\nCategories: blast-radius.\n\nNot scored: `source` failed: offline\n"),
      "{markdown}"
    );
    assert!(markdown.contains("## Aggregate (mean over the 1 of 2 tasks that define each metric)"), "{markdown}");
    assert!(markdown.contains("| code atlas revision | unknown |\n"), "{markdown}");
  }

  fn rename_task(rename: RenameRetrieval) -> TaskRetrieval {
    TaskRetrieval {
      rename: Some(rename),
      ..scored_task(None)
    }
  }

  fn site_score(expected: &[(&str, u32)], planned: &[(&str, u32)]) -> RenameSiteScore {
    let sites = |items: &[(&str, u32)]| items.iter().map(|(path, line)| ((*path).to_owned(), *line)).collect();
    crate::retrieval::score::score_rename_sites(&sites(expected), &sites(planned))
  }

  #[test]
  fn should_aggregate_and_render_rename_scores_and_skip_unavailable_ones() {
    let scored = RenameRetrieval::Scored {
      target: "UsersService.findOne".to_owned(),
      new_name: "getById".to_owned(),
      servers: vec!["typescript-language-server 5.1.3".to_owned()],
      score: site_score(&[("a.ts", 1), ("a.ts", 2), ("b.ts", 3), ("c.ts", 4)], &[("a.ts", 1), ("a.ts", 2), ("b.ts", 3), ("d.ts", 9)]),
    };
    let tasks = vec![
      scored_entry("t1", rename_task(scored)),
      scored_entry(
        "t2",
        rename_task(RenameRetrieval::Unavailable {
          reason: "no language server: typescript-language-server is unavailable".to_owned(),
        }),
      ),
      scored_entry("t3", scored_task(None)),
    ];

    let means = aggregate(&tasks);

    assert_eq!(counted(&means, "rename.recall"), (1, Some(0.75)));
    assert_eq!(counted(&means, "rename.precision"), (1, Some(0.75)));
    let mut markdown = String::new();
    render_task(&mut markdown, &tasks[0]);
    render_task(&mut markdown, &tasks[1]);
    assert!(
      markdown.contains("Rename `UsersService.findOne` -> `getById` as a dry run (typescript-language-server 5.1.3) against the old-side lines of `reference.patch` where the old name becomes the new one: recall 0.75 (3/4), precision 0.75.\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("- patch sites the plan does not edit: `c.ts:4`\n- planned edits the patch does not make: `d.ts:9`\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("Rename: not scored (no language server: typescript-language-server is unavailable).\n"),
      "{markdown}"
    );
  }

  #[test]
  fn should_not_add_rename_metrics_when_no_task_renames() {
    let means = aggregate(&[scored_entry("t1", scored_task(None))]);
    assert!(means.iter().all(|mean| !mean.metric.starts_with("rename.")));
  }

  #[test]
  fn should_count_the_queries_whose_pack_or_recall_moves_with_the_budget() {
    let mut moving = query("one", &["a.rs"], &["a.rs"]);
    moving.yields[0].estimated_tokens = 20;
    moving.yields[0].must_read = counted_overlap(0, 1, 0);
    let steady = query("two", &["a.rs"], &["a.rs"]);
    let tasks = vec![scored_entry("t1", task_with(scored(vec![moving, steady]), None))];

    let markdown = render_markdown(&RetrievalReport::new(None, settings(), tasks));

    assert!(markdown.contains("| full | 1 | 0 | 0 | 0 |\n| agent | 2 | 1 | 1 | 0 |\n"), "{markdown}");
  }

  #[test]
  fn should_sign_differences_and_never_print_a_negative_zero() {
    assert_eq!(signed(Some(0.25)), "+0.25");
    assert_eq!(signed(Some(-0.25)), "-0.25");
    assert_eq!(signed(Some(0.0)), "0");
    assert_eq!(signed(Some(-0.0001)), "0");
    assert_eq!(signed(Some(0.0001)), "0");
    assert_eq!(signed(None), "n/a");
  }
}
