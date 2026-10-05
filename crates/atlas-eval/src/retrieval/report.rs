use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::{RetrievalError, RetrievalResult};
use super::phrases::Phrase;
use super::score::{FileScore, MetricMean, ReuseScore, mean_of};
use crate::quality::SetOverlap;

pub const DEV_SET_CAVEAT: &str = "Dev set, same authors: the tasks, their ground truth and Code Atlas were written by the same people, and the tasks were \
used while developing the product. Read these numbers as regression signals for the retrieval surface, not as an unbiased estimate of how it \
performs on unseen code.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalSettings {
  pub context_budget: usize,
  pub search_limit: usize,
  pub max_code_spans: usize,
  pub max_prose_phrases: usize,
  pub max_similar_seeds: usize,
  pub impact_group_limit: usize,
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
pub struct TaskRetrieval {
  pub index: IndexSummary,
  pub queries: QueryLog,
  pub pack: PackSize,
  pub must_change: FileScore,
  pub relevant_files: FileScore,
  pub must_consider: FileScore,
  pub reuse: Option<ReuseScore>,
  pub blast_radius: Option<BlastRadius>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievalReport {
  pub caveat: String,
  pub code_atlas_revision: Option<String>,
  pub settings: RetrievalSettings,
  pub tasks: Vec<TaskEntry>,
  pub aggregate: Vec<MetricMean>,
}

type FileSelector = fn(&TaskRetrieval) -> &FileScore;
type OverlapSelector = fn(&FileScore) -> &SetOverlap;
type ReuseSelector = fn(&ReuseScore) -> Option<f64>;

const FILE_SETS: [(&str, FileSelector); 3] = [
  ("must_change", |task| &task.must_change),
  ("relevant_files", |task| &task.relevant_files),
  ("must_consider", |task| &task.must_consider),
];

const CHANNELS: [(&str, OverlapSelector); 5] = [
  ("must_read", |score| &score.channels.must_read),
  ("pack", |score| &score.channels.pack),
  ("search", |score| &score.channels.search),
  ("similar", |score| &score.channels.similar),
  ("any", |score| &score.channels.any),
];

pub fn aggregate(tasks: &[TaskEntry]) -> Vec<MetricMean> {
  let scored: Vec<&TaskRetrieval> = tasks.iter().filter_map(TaskEntry::scored).collect();
  let mut means = Vec::new();
  for (set, select) in FILE_SETS {
    for (channel, overlap) in CHANNELS {
      means.push(mean_of(&format!("{set}.recall.{channel}"), scored.iter().map(|task| overlap(select(task)).recall)));
    }
  }
  for (channel, overlap) in CHANNELS {
    means.push(mean_of(
      &format!("relevant_files.precision.{channel}"),
      scored.iter().map(|task| overlap(&task.relevant_files).precision),
    ));
  }
  let reuse: [(&str, ReuseSelector); 4] = [
    ("pack", |score| score.pack),
    ("search", |score| score.search),
    ("similar", |score| score.similar),
    ("any", |score| score.any),
  ];
  for (channel, value) in reuse {
    means.push(mean_of(
      &format!("reuse_targets.recall.{channel}"),
      scored.iter().map(|task| task.reuse.as_ref().and_then(value)),
    ));
  }
  for (name, view) in [("visible", ImpactView::Visible), ("full", ImpactView::Full)] {
    means.push(mean_of(
      &format!("blast_radius.recall.{name}"),
      scored.iter().map(|task| task.blast_radius.as_ref().and_then(|blast| blast.recall(view))),
    ));
  }
  means.push(mean_of("pack.bytes", scored.iter().map(|task| Some(task.pack.bytes as f64))));
  means.push(mean_of("pack.estimated_tokens", scored.iter().map(|task| Some(task.pack.estimated_tokens as f64))));
  means
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

fn hits(overlap: &SetOverlap) -> String {
  match overlap.relevant {
    0 => "n/a".to_owned(),
    relevant => format!("{}/{relevant}", overlap.hits),
  }
}

pub fn render_markdown(report: &RetrievalReport) -> String {
  let mut out = String::new();
  let _ = writeln!(out, "# Retrieval report\n");
  let _ = writeln!(out, "> {}\n", report.caveat);
  let _ = writeln!(
    out,
    "Deterministic and LLM-free: every task's base tree is materialised like the agent harness does it (no setup commands), indexed in process, \
and queried only with the task prompt, the way a careful agent would before editing. Ground truth is used only to score, and to pick the \
blast-radius seed symbols.\n"
  );
  render_settings(&mut out, report);
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
  let rows = [
    ("code atlas revision", revision.to_owned()),
    ("context budget (CLI and MCP default)", settings.context_budget.to_string()),
    ("search limit per query", settings.search_limit.to_string()),
    (
      "searches",
      format!(
        "the prompt, up to {} code spans, up to {} prose phrases",
        settings.max_code_spans, settings.max_prose_phrases
      ),
    ),
    ("similar seeds", format!("up to {} top function/method search hits", settings.max_similar_seeds)),
    ("impact group limit (visible)", settings.impact_group_limit.to_string()),
  ];
  for (key, value) in rows {
    let _ = writeln!(out, "| {key} | {value} |");
  }
  let _ = writeln!(out);
}

fn render_aggregate(out: &mut String, report: &RetrievalReport) {
  let scored = report.tasks.iter().filter(|task| task.scored().is_some()).count();
  let _ = writeln!(out, "## Aggregate (mean over the {scored} of {} tasks that define each metric)\n", report.tasks.len());
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
  let pack = &retrieval.pack;
  let _ = writeln!(
    out,
    "Context pack: {} bytes, ~{} of {} tokens; items: must read {}, skim {}, patterns {}, reuse {}, impact {}, tests {}, uncertain {}, omitted {}.\n",
    pack.bytes, pack.estimated_tokens, pack.budget, pack.must_read, pack.skim, pack.patterns, pack.reuse, pack.impact, pack.tests, pack.uncertain, pack.omitted
  );
  render_queries(out, &retrieval.queries);
  render_files(out, retrieval);
  if let Some(reuse) = &retrieval.reuse {
    render_reuse(out, reuse);
  }
  if let Some(blast) = &retrieval.blast_radius {
    render_blast(out, blast);
  }
}

fn render_queries(out: &mut String, queries: &QueryLog) {
  let phrases: Vec<String> = queries.search_phrases.iter().map(|phrase| format!("`{}`", phrase.text.replace('`', "'"))).collect();
  let _ = writeln!(
    out,
    "Searches: the full prompt, then {}.",
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

fn render_files(out: &mut String, retrieval: &TaskRetrieval) {
  let _ = writeln!(out, "| files | must read | pack | search | similar | any |\n|---|---|---|---|---|---|");
  for (set, select) in FILE_SETS {
    let score = select(retrieval);
    let cells: Vec<String> = CHANNELS
      .iter()
      .map(|(_, overlap)| format!("{} ({})", number(overlap(score).recall), hits(overlap(score))))
      .collect();
    let _ = writeln!(out, "| {set} recall | {} |", cells.join(" | "));
  }
  let precision: Vec<String> = CHANNELS.iter().map(|(_, overlap)| number(overlap(&retrieval.relevant_files).precision)).collect();
  let _ = writeln!(out, "| relevant_files precision | {} |\n", precision.join(" | "));
  let mut notes = Vec::new();
  for (set, select) in FILE_SETS {
    let score = select(retrieval);
    if !score.missed.is_empty() {
      notes.push(format!("- {set} not retrieved anywhere: {}", code_list(&score.missed)));
    }
    if !score.absent_at_base.is_empty() {
      notes.push(format!("- {set} absent from the base tree (not scored): {}", code_list(&score.absent_at_base)));
    }
    if !score.unindexable.is_empty() {
      notes.push(format!("- {set} unindexable, never in the index (not scored): {}", code_list(&score.unindexable)));
    }
  }
  if !notes.is_empty() {
    let _ = writeln!(out, "{}\n", notes.join("\n"));
  }
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
  use crate::retrieval::score::{BaseFile, Retrieved, score_files};

  fn scored_task(blast_radius: Option<BlastRadius>) -> TaskRetrieval {
    let retrieved = Retrieved {
      must_read_files: ["a.rs".to_owned()].into(),
      pack_files: ["a.rs".to_owned(), "b.rs".to_owned()].into(),
      ..Retrieved::default()
    };
    let expected = ["a.rs".to_owned(), "b.rs".to_owned()];
    TaskRetrieval {
      index: IndexSummary {
        repos: vec!["tree".to_owned()],
        files: 2,
        degraded: None,
      },
      queries: QueryLog {
        search_phrases: Vec::new(),
        similar_seeds: Vec::new(),
      },
      pack: PackSize {
        bytes: 100,
        estimated_tokens: 30,
        budget: 6000,
        ..PackSize::default()
      },
      must_change: score_files(&retrieved, &expected[..1], |_| BaseFile::Indexed),
      relevant_files: score_files(&retrieved, &expected, |_| BaseFile::Indexed),
      must_consider: score_files(&retrieved, &[], |_| BaseFile::Indexed),
      reuse: None,
      blast_radius,
    }
  }

  fn entry(id: &str, outcome: TaskOutcome) -> TaskEntry {
    TaskEntry {
      id: id.to_owned(),
      title: format!("{id} title"),
      categories: vec!["blast-radius".to_owned()],
      outcome,
    }
  }

  fn mean<'a>(means: &'a [MetricMean], metric: &str) -> &'a MetricMean {
    means.iter().find(|mean| mean.metric == metric).unwrap()
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
      entry("t1", TaskOutcome::Scored(Box::new(scored_task(Some(blast))))),
      entry(
        "t2",
        TaskOutcome::Scored(Box::new(scored_task(Some(BlastRadius::Unavailable { reason: "no patch".to_owned() })))),
      ),
      entry(
        "t3",
        TaskOutcome::Failed {
          stage: "source".to_owned(),
          reason: "offline".to_owned(),
        },
      ),
    ];
    let means = aggregate(&tasks);
    assert_eq!(
      (mean(&means, "must_change.recall.must_read").tasks, mean(&means, "must_change.recall.must_read").mean),
      (2, Some(1.0))
    );
    assert_eq!(mean(&means, "relevant_files.recall.must_read").mean, Some(0.5));
    assert_eq!(mean(&means, "relevant_files.precision.pack").mean, Some(1.0));
    assert_eq!((mean(&means, "must_consider.recall.pack").tasks, mean(&means, "must_consider.recall.pack").mean), (0, None));
    assert_eq!(
      (mean(&means, "blast_radius.recall.full").tasks, mean(&means, "blast_radius.recall.full").mean),
      (1, Some(0.5))
    );
    assert_eq!(mean(&means, "pack.bytes").mean, Some(100.0));
  }

  #[test]
  fn should_render_failed_tasks_and_unavailable_blast_radius_without_scores() {
    let tasks = vec![
      entry(
        "t2",
        TaskOutcome::Scored(Box::new(scored_task(Some(BlastRadius::Unavailable { reason: "no patch".to_owned() })))),
      ),
      entry(
        "t3",
        TaskOutcome::Failed {
          stage: "source".to_owned(),
          reason: "offline".to_owned(),
        },
      ),
    ];
    let report = RetrievalReport {
      caveat: DEV_SET_CAVEAT.to_owned(),
      code_atlas_revision: None,
      settings: RetrievalSettings {
        context_budget: 6000,
        search_limit: 10,
        max_code_spans: 8,
        max_prose_phrases: 4,
        max_similar_seeds: 5,
        impact_group_limit: 15,
      },
      aggregate: aggregate(&tasks),
      tasks,
    };
    let markdown = render_markdown(&report);
    assert!(
      markdown.contains("| must_change recall | 1 (1/1) | 1 (1/1) | 0 (0/1) | 0 (0/1) | 1 (1/1) |\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("| must_consider recall | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) |\n"),
      "{markdown}"
    );
    assert!(markdown.contains("Blast radius: not scored (no patch).\n"), "{markdown}");
    assert!(
      markdown.contains("## t3: t3 title\n\nCategories: blast-radius.\n\nNot scored: `source` failed: offline\n"),
      "{markdown}"
    );
    assert!(markdown.contains("## Aggregate (mean over the 1 of 2 tasks that define each metric)"), "{markdown}");
  }
}
