use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use atlas_query::{ContextItem, ContextRequest};
use serde::{Deserialize, Serialize};

use super::corpus::IndexedProject;
use super::error::{QualityError, QualityResult};
use super::metrics::ratio;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextTask {
  pub id: String,
  pub fixture: String,
  pub prompt: String,
  pub relevant_files: Vec<String>,
  pub relevant_symbols: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedTask {
  pub id: String,
  pub reason: String,
}

#[derive(Debug, Deserialize)]
struct RawTruth {
  #[serde(default)]
  relevant_files: Vec<String>,
  #[serde(default)]
  relevant_symbols: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawTask {
  id: String,
  prompt: String,
  repo: toml::Value,
  #[serde(default)]
  ground_truth: Option<RawTruth>,
}

pub fn parse_context_task(text: &str, origin: &str) -> QualityResult<Result<ContextTask, SkippedTask>> {
  let raw: RawTask = toml::from_str(text).map_err(|error| QualityError::parse(origin, error))?;
  let fixture = raw.repo.get("fixture").and_then(toml::Value::as_str).map(str::to_owned);
  let Some(fixture) = fixture else {
    return Ok(Err(SkippedTask {
      id: raw.id,
      reason: "only `repo = { fixture = \"...\" }` tasks can be indexed by the quality evaluation".to_owned(),
    }));
  };
  let truth = raw.ground_truth.unwrap_or(RawTruth {
    relevant_files: Vec::new(),
    relevant_symbols: Vec::new(),
  });
  if truth.relevant_files.is_empty() && truth.relevant_symbols.is_empty() {
    return Ok(Err(SkippedTask {
      id: raw.id,
      reason: "the task declares neither relevant_files nor relevant_symbols".to_owned(),
    }));
  }
  Ok(Ok(ContextTask {
    id: raw.id,
    fixture,
    prompt: raw.prompt,
    relevant_files: truth.relevant_files,
    relevant_symbols: truth.relevant_symbols,
  }))
}

pub fn load_context_tasks(dir: &Path) -> QualityResult<(Vec<ContextTask>, Vec<SkippedTask>)> {
  let mut tasks = Vec::new();
  let mut skipped = Vec::new();
  let mut entries: Vec<_> = fs::read_dir(dir)
    .map_err(|error| QualityError::io(dir, error))?
    .filter_map(Result::ok)
    .map(|entry| entry.path())
    .collect();
  entries.sort();
  for entry in entries {
    let file = if entry.is_dir() { entry.join("task.toml") } else { entry.clone() };
    if !file.is_file() || file.extension().and_then(|extension| extension.to_str()) != Some("toml") {
      continue;
    }
    let text = fs::read_to_string(&file).map_err(|error| QualityError::io(&file, error))?;
    match parse_context_task(&text, &file.display().to_string())? {
      Ok(task) => tasks.push(task),
      Err(skip) => skipped.push(skip),
    }
  }
  Ok((tasks, skipped))
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SetOverlap {
  pub retrieved: usize,
  pub relevant: usize,
  pub hits: usize,
  pub precision: Option<f64>,
  pub recall: Option<f64>,
}

pub fn overlap(retrieved: &BTreeSet<String>, relevant: &BTreeSet<String>) -> SetOverlap {
  let hits = retrieved.intersection(relevant).count();
  SetOverlap {
    retrieved: retrieved.len(),
    relevant: relevant.len(),
    hits,
    precision: ratio(hits as u64, retrieved.len() as u64).filter(|_| !relevant.is_empty()),
    recall: ratio(hits as u64, relevant.len() as u64),
  }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Selection {
  pub files: SetOverlap,
  pub symbols: SetOverlap,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskContextScore {
  pub id: String,
  pub must_read: Selection,
  pub skim: Selection,
  pub combined: Selection,
}

fn files_of(items: &[&ContextItem]) -> BTreeSet<String> {
  items
    .iter()
    .filter_map(|item| item.entity.location.as_ref())
    .map(|location| format!("{}/{}", location.repo, location.path))
    .collect()
}

fn symbol_names(item: &ContextItem) -> [String; 2] {
  [item.entity.short.clone(), format!("{}:{}", item.entity.repo(), item.entity.short)]
}

pub fn score_selection(items: &[&ContextItem], relevant_files: &BTreeSet<String>, relevant_symbols: &BTreeSet<String>) -> Selection {
  let matched: BTreeSet<String> = items.iter().flat_map(|item| symbol_names(item)).filter(|name| relevant_symbols.contains(name)).collect();
  let useful_items = items.iter().filter(|item| symbol_names(item).iter().any(|name| relevant_symbols.contains(name))).count();
  Selection {
    files: overlap(&files_of(items), relevant_files),
    symbols: SetOverlap {
      retrieved: items.len(),
      relevant: relevant_symbols.len(),
      hits: matched.len(),
      precision: ratio(useful_items as u64, items.len() as u64).filter(|_| !relevant_symbols.is_empty()),
      recall: ratio(matched.len() as u64, relevant_symbols.len() as u64),
    },
  }
}

pub fn score_task(task: &ContextTask, must_read: &[ContextItem], skim: &[ContextItem]) -> TaskContextScore {
  let files: BTreeSet<String> = task.relevant_files.iter().cloned().collect();
  let symbols: BTreeSet<String> = task.relevant_symbols.iter().cloned().collect();
  let read: Vec<&ContextItem> = must_read.iter().collect();
  let skimmed: Vec<&ContextItem> = skim.iter().collect();
  let combined: Vec<&ContextItem> = must_read.iter().chain(skim).collect();
  TaskContextScore {
    id: task.id.clone(),
    must_read: score_selection(&read, &files, &symbols),
    skim: score_selection(&skimmed, &files, &symbols),
    combined: score_selection(&combined, &files, &symbols),
  }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeanOverlap {
  pub tasks: usize,
  pub precision: Option<f64>,
  pub recall: Option<f64>,
}

fn mean(values: impl Iterator<Item = Option<f64>>) -> (usize, Option<f64>) {
  let present: Vec<f64> = values.flatten().collect();
  match present.is_empty() {
    true => (0, None),
    false => (present.len(), Some(present.iter().sum::<f64>() / present.len() as f64)),
  }
}

pub fn macro_average(scores: &[TaskContextScore], select: impl Fn(&TaskContextScore) -> &SetOverlap) -> MeanOverlap {
  let (_, precision) = mean(scores.iter().map(|score| select(score).precision));
  let (tasks, recall) = mean(scores.iter().map(|score| select(score).recall));
  MeanOverlap { tasks, precision, recall }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContextAggregate {
  pub must_read_files: MeanOverlap,
  pub skim_files: MeanOverlap,
  pub combined_files: MeanOverlap,
  pub must_read_symbols: MeanOverlap,
  pub skim_symbols: MeanOverlap,
  pub combined_symbols: MeanOverlap,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ContextReport {
  NotRequested {
    reason: String,
  },
  Scored {
    tasks: Vec<TaskContextScore>,
    skipped: Vec<SkippedTask>,
    aggregate: Box<ContextAggregate>,
  },
}

pub fn aggregate(scores: &[TaskContextScore]) -> ContextAggregate {
  ContextAggregate {
    must_read_files: macro_average(scores, |score| &score.must_read.files),
    skim_files: macro_average(scores, |score| &score.skim.files),
    combined_files: macro_average(scores, |score| &score.combined.files),
    must_read_symbols: macro_average(scores, |score| &score.must_read.symbols),
    skim_symbols: macro_average(scores, |score| &score.skim.symbols),
    combined_symbols: macro_average(scores, |score| &score.combined.symbols),
  }
}

pub fn evaluate_context(tasks: &[ContextTask], skipped: Vec<SkippedTask>, projects: &BTreeMap<String, &IndexedProject>) -> QualityResult<ContextReport> {
  let mut scores = Vec::new();
  let mut skipped = skipped;
  for task in tasks {
    let Some(project) = projects.get(&task.fixture) else {
      skipped.push(SkippedTask {
        id: task.id.clone(),
        reason: format!("fixture `{}` is not indexed", task.fixture),
      });
      continue;
    };
    let answer = project.query.context(&ContextRequest::new(task.prompt.clone()))?.answer;
    scores.push(score_task(task, &answer.must_read, &answer.skim));
  }
  Ok(ContextReport::Scored {
    aggregate: Box::new(aggregate(&scores)),
    tasks: scores,
    skipped,
  })
}

#[cfg(test)]
mod tests {
  use atlas_query::EntityRef;

  use super::*;
  use atlas_query::Location;

  fn item(repo: &str, path: &str, short: &str) -> ContextItem {
    ContextItem {
      entity: EntityRef {
        key: format!("{repo}:{short}"),
        id: "0000000000000001".to_owned(),
        short: short.to_owned(),
        kind: "function".to_owned(),
        location: Some(Location {
          repo: repo.to_owned(),
          path: path.to_owned(),
          line: Some(1),
          column: None,
        }),
        certainty: None,
      },
      score: 1.0,
      why: String::new(),
      certainty: None,
      via: None,
      uncertain: false,
      signature: None,
      doc: None,
      skeleton: Vec::new(),
      lines: None,
      excerpt: None,
      body: None,
      read_directly: false,
      run: None,
    }
  }

  fn task() -> ContextTask {
    ContextTask {
      id: "t".into(),
      fixture: "acme".into(),
      prompt: "p".into(),
      relevant_files: vec!["acme-api/src/a.ts".into(), "acme-api/src/b.ts".into(), "acme-sdk/src/c.ts".into()],
      relevant_symbols: vec!["A.run".into(), "B.go".into()],
    }
  }

  #[test]
  fn should_compute_precision_and_recall_for_must_read_skim_and_combined() {
    let must_read = vec![item("acme-api", "src/a.ts", "A.run"), item("acme-api", "src/x.ts", "X.noise")];
    let skim = vec![item("acme-api", "src/b.ts", "B.go"), item("acme-sdk", "src/c.ts", "C.other")];
    let score = score_task(&task(), &must_read, &skim);
    assert_eq!((score.must_read.files.hits, score.must_read.files.retrieved), (1, 2));
    assert_eq!(score.must_read.files.precision, Some(0.5));
    assert_eq!(score.must_read.files.recall, Some(1.0 / 3.0));
    assert_eq!(score.skim.files.precision, Some(1.0));
    assert_eq!(score.combined.files.recall, Some(1.0));
    assert_eq!(score.combined.files.precision, Some(0.75));
    assert_eq!(score.combined.symbols.recall, Some(1.0));
    assert_eq!(score.combined.symbols.precision, Some(0.5));
    assert_eq!(score.must_read.symbols.recall, Some(0.5));
  }

  #[test]
  fn should_not_score_a_dimension_the_task_does_not_declare() {
    let mut files_only = task();
    files_only.relevant_symbols.clear();
    let score = score_task(&files_only, &[item("acme-api", "src/a.ts", "A.run")], &[]);
    assert_eq!(score.must_read.symbols.precision, None);
    assert_eq!(score.must_read.symbols.recall, None);
    assert_eq!(score.must_read.files.precision, Some(1.0));
  }

  #[test]
  fn should_report_no_precision_for_an_empty_selection() {
    let score = score_task(&task(), &[], &[]);
    assert_eq!(score.combined.files.precision, None);
    assert_eq!(score.combined.files.recall, Some(0.0));
  }

  #[test]
  fn should_average_over_tasks_that_define_the_metric() {
    let first = score_task(&task(), &[item("acme-api", "src/a.ts", "A.run")], &[]);
    let second = score_task(&task(), &[], &[]);
    let average = macro_average(&[first, second], |score| &score.must_read.files);
    assert_eq!(average.tasks, 2);
    assert_eq!(average.precision, Some(1.0));
    assert_eq!(average.recall, Some((1.0 / 3.0) / 2.0));
  }

  #[test]
  fn should_parse_fixture_tasks_and_skip_unsupported_ones() {
    let text = r#"
id = "t01"
title = "x"
repo = { fixture = "acme" }
prompt = "Add a field"
[ground_truth]
relevant_files = ["acme-api/src/users/user.entity.ts"]
relevant_symbols = ["User"]
"#;
    let parsed = parse_context_task(text, "t01").unwrap().unwrap();
    assert_eq!(parsed.fixture, "acme");
    assert_eq!(parsed.relevant_symbols, ["User"]);
    let own = text.replace("{ fixture = \"acme\" }", "\"self\"");
    let skipped = parse_context_task(&own, "t01").unwrap().unwrap_err();
    assert_eq!(skipped.id, "t01");
    let empty = text
      .replace("relevant_files = [\"acme-api/src/users/user.entity.ts\"]", "")
      .replace("relevant_symbols = [\"User\"]", "");
    assert!(parse_context_task(&empty, "t01").unwrap().is_err());
  }
}
