use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::agent::SymbolTarget;
use crate::quality::{SetOverlap, overlap};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retrieved {
  pub must_read_files: BTreeSet<String>,
  pub pack_files: BTreeSet<String>,
  pub search_files: BTreeSet<String>,
  pub similar_files: BTreeSet<String>,
  pub pack_symbols: BTreeSet<String>,
  pub search_symbols: BTreeSet<String>,
  pub similar_symbols: BTreeSet<String>,
}

impl Retrieved {
  pub fn any_files(&self) -> BTreeSet<String> {
    self.pack_files.iter().chain(&self.search_files).chain(&self.similar_files).cloned().collect()
  }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelScores {
  pub must_read: SetOverlap,
  pub pack: SetOverlap,
  pub search: SetOverlap,
  pub similar: SetOverlap,
  pub any: SetOverlap,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelValues {
  pub must_read: Option<f64>,
  pub pack: Option<f64>,
  pub search: Option<f64>,
  pub similar: Option<f64>,
  pub any: Option<f64>,
}

pub const F_BETA: f64 = 0.5;

pub fn f_beta(overlap: &SetOverlap, beta: f64) -> Option<f64> {
  if overlap.relevant == 0 {
    return None;
  }
  let weight = beta * beta;
  Some((1.0 + weight) * overlap.hits as f64 / (weight * overlap.relevant as f64 + overlap.retrieved as f64))
}

pub fn counted_overlap(retrieved: usize, relevant: usize, hits: usize) -> SetOverlap {
  SetOverlap {
    retrieved,
    relevant,
    hits,
    precision: fraction(hits, retrieved).filter(|_| relevant > 0),
    recall: fraction(hits, relevant),
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaseFile {
  Indexed,
  Unindexable,
  Absent,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileScore {
  pub expected: Vec<String>,
  pub absent_at_base: Vec<String>,
  #[serde(default)]
  pub unindexable: Vec<String>,
  pub channels: ChannelScores,
  pub f05: ChannelValues,
  pub missed: Vec<String>,
}

pub fn score_files(retrieved: &Retrieved, expected: &[String], classify: impl Fn(&str) -> BaseFile) -> FileScore {
  let mut indexed = Vec::new();
  let mut unindexable = Vec::new();
  let mut absent = Vec::new();
  for path in expected {
    match classify(path) {
      BaseFile::Indexed => indexed.push(path.clone()),
      BaseFile::Unindexable => unindexable.push(path.clone()),
      BaseFile::Absent => absent.push(path.clone()),
    }
  }
  let relevant: BTreeSet<String> = indexed.iter().cloned().collect();
  let any = retrieved.any_files();
  let channels = ChannelScores {
    must_read: overlap(&retrieved.must_read_files, &relevant),
    pack: overlap(&retrieved.pack_files, &relevant),
    search: overlap(&retrieved.search_files, &relevant),
    similar: overlap(&retrieved.similar_files, &relevant),
    any: overlap(&any, &relevant),
  };
  FileScore {
    missed: relevant.difference(&any).cloned().collect(),
    f05: ChannelValues {
      must_read: f_beta(&channels.must_read, F_BETA),
      pack: f_beta(&channels.pack, F_BETA),
      search: f_beta(&channels.search, F_BETA),
      similar: f_beta(&channels.similar, F_BETA),
      any: f_beta(&channels.any, F_BETA),
    },
    channels,
    expected: indexed,
    absent_at_base: absent,
    unindexable,
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReuseHit {
  pub target: String,
  pub pack: bool,
  pub search: bool,
  pub similar: bool,
}

impl ReuseHit {
  pub fn anywhere(&self) -> bool {
    self.pack || self.search || self.similar
  }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReuseScore {
  pub targets: Vec<ReuseHit>,
  pub pack: Option<f64>,
  pub search: Option<f64>,
  pub similar: Option<f64>,
  pub any: Option<f64>,
}

pub fn score_reuse(retrieved: &Retrieved, targets: &[String]) -> ReuseScore {
  let found = |target: &SymbolTarget, symbols: &BTreeSet<String>| symbols.iter().any(|symbol| target.matches_symbol(symbol));
  let hits: Vec<ReuseHit> = targets
    .iter()
    .map(|raw| {
      let target = SymbolTarget::parse(raw);
      ReuseHit {
        target: raw.clone(),
        pack: found(&target, &retrieved.pack_symbols),
        search: found(&target, &retrieved.search_symbols),
        similar: found(&target, &retrieved.similar_symbols),
      }
    })
    .collect();
  let share = |hit: fn(&ReuseHit) -> bool| fraction(hits.iter().filter(|candidate| hit(candidate)).count(), hits.len());
  ReuseScore {
    pack: share(|hit| hit.pack),
    search: share(|hit| hit.search),
    similar: share(|hit| hit.similar),
    any: share(ReuseHit::anywhere),
    targets: hits,
  }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RenameSiteScore {
  pub sites: SetOverlap,
  pub missed: Vec<String>,
  pub unexpected: Vec<String>,
}

pub fn score_rename_sites(expected: &BTreeSet<(String, u32)>, planned: &BTreeSet<(String, u32)>) -> RenameSiteScore {
  let label = |(path, line): &(String, u32)| format!("{path}:{line}");
  let expected_labels: BTreeSet<String> = expected.iter().map(label).collect();
  let planned_labels: BTreeSet<String> = planned.iter().map(label).collect();
  RenameSiteScore {
    sites: overlap(&planned_labels, &expected_labels),
    missed: expected_labels.difference(&planned_labels).cloned().collect(),
    unexpected: planned_labels.difference(&expected_labels).cloned().collect(),
  }
}

pub const YIELD_BUDGETS: [usize; 3] = [4_000, 8_000, 16_000];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BudgetYield {
  pub budget: usize,
  pub estimated_tokens: usize,
  pub bytes: usize,
  pub must_read: SetOverlap,
  pub pack: SetOverlap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackCost {
  pub budget: usize,
  pub estimated_tokens: usize,
  pub bytes: usize,
}

pub fn budget_yield(cost: PackCost, retrieved: &Retrieved, must_change: &BTreeSet<String>) -> BudgetYield {
  BudgetYield {
    budget: cost.budget,
    estimated_tokens: cost.estimated_tokens,
    bytes: cost.bytes,
    must_read: overlap(&retrieved.must_read_files, must_change),
    pack: overlap(&retrieved.pack_files, must_change),
  }
}

pub fn fraction(hits: usize, total: usize) -> Option<f64> {
  (total > 0).then(|| hits as f64 / total as f64)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricMean {
  pub metric: String,
  pub tasks: usize,
  pub mean: Option<f64>,
}

pub fn mean_of(metric: &str, values: impl IntoIterator<Item = Option<f64>>) -> MetricMean {
  let present: Vec<f64> = values.into_iter().flatten().collect();
  MetricMean {
    metric: metric.to_owned(),
    tasks: present.len(),
    mean: (!present.is_empty()).then(|| present.iter().sum::<f64>() / present.len() as f64),
  }
}

pub fn tree_path(prefixes: &BTreeMap<String, String>, repo: &str, path: &str) -> Option<String> {
  let prefix = prefixes.get(repo)?;
  Some(match prefix.is_empty() {
    true => path.to_owned(),
    false => format!("{prefix}/{path}"),
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
  }

  fn retrieved() -> Retrieved {
    Retrieved {
      must_read_files: set(&["a.rs"]),
      pack_files: set(&["a.rs", "b.rs", "noise.rs"]),
      search_files: set(&["c.rs", "noise.rs"]),
      similar_files: set(&[]),
      pack_symbols: set(&["rust:tree:atlas-mcp:render::targeted#function"]),
      search_symbols: set(&["rust:tree:atlas-query:engine::QueryEngine::dependents#method"]),
      similar_symbols: set(&["rust:tree:atlas-mcp:render::targeted#function"]),
    }
  }

  #[test]
  fn should_score_each_channel_against_the_files_present_at_base() {
    let expected = vec!["a.rs".to_owned(), "c.rs".to_owned(), "d.rs".to_owned(), "new.rs".to_owned(), "CLAUDE.md".to_owned()];
    let classify = |path: &str| match path {
      "new.rs" => BaseFile::Absent,
      "CLAUDE.md" => BaseFile::Unindexable,
      _ => BaseFile::Indexed,
    };
    let score = score_files(&retrieved(), &expected, classify);
    assert_eq!(score.expected, vec!["a.rs", "c.rs", "d.rs"]);
    assert_eq!(score.absent_at_base, vec!["new.rs"]);
    assert_eq!(score.unindexable, vec!["CLAUDE.md"]);
    assert_eq!(score.missed, vec!["d.rs"]);
    assert_eq!((score.channels.must_read.hits, score.channels.must_read.recall), (1, Some(1.0 / 3.0)));
    assert_eq!(score.channels.must_read.precision, Some(1.0));
    assert_eq!((score.channels.pack.hits, score.channels.pack.precision), (1, Some(1.0 / 3.0)));
    assert_eq!(score.channels.search.recall, Some(1.0 / 3.0));
    assert_eq!((score.channels.similar.recall, score.channels.similar.precision), (Some(0.0), None));
    assert_eq!((score.channels.any.retrieved, score.channels.any.hits), (4, 2));
    assert_eq!(score.channels.any.recall, Some(2.0 / 3.0));
    assert_eq!(score.channels.any.precision, Some(0.5));
    assert_eq!(
      score.f05,
      ChannelValues {
        must_read: Some(1.25 / 1.75),
        pack: Some(1.25 / 3.75),
        search: Some(1.25 / 2.75),
        similar: Some(0.0),
        any: Some(2.5 / 4.75),
      }
    );
  }

  fn counts(retrieved: usize, relevant: usize, hits: usize) -> SetOverlap {
    counted_overlap(retrieved, relevant, hits)
  }

  #[test]
  fn should_weight_precision_twice_as_heavily_as_recall_in_f_half() {
    assert_eq!(f_beta(&counts(4, 8, 2), F_BETA), Some(2.5 / 6.0));
    assert_eq!(f_beta(&counts(1, 2, 1), F_BETA), Some(1.25 / 1.5));
    assert_eq!(f_beta(&counts(2, 1, 1), F_BETA), Some(1.25 / 2.25));
    assert_eq!(f_beta(&counts(3, 3, 3), F_BETA), Some(1.0));
    assert_eq!(f_beta(&counts(2, 1, 1), 1.0), Some(2.0 / 3.0));
  }

  #[test]
  fn should_agree_with_the_precision_and_recall_form_of_f_beta() {
    let overlap = counts(5, 7, 3);
    let (precision, recall) = (overlap.precision.unwrap(), overlap.recall.unwrap());
    let from_rates = 1.25 * precision * recall / (0.25 * precision + recall);
    assert!((f_beta(&overlap, F_BETA).unwrap() - from_rates).abs() < 1e-12);
  }

  #[test]
  fn should_score_f_half_zero_without_hits_and_leave_it_undefined_without_gold() {
    assert_eq!(f_beta(&counts(0, 3, 0), F_BETA), Some(0.0));
    assert_eq!(f_beta(&counts(5, 3, 0), F_BETA), Some(0.0));
    assert_eq!(f_beta(&counts(3, 0, 0), F_BETA), None);
    assert_eq!(f_beta(&counts(0, 0, 0), F_BETA), None);
  }

  #[test]
  fn should_leave_precision_undefined_for_an_empty_retrieval_or_an_empty_gold_set() {
    assert_eq!((counts(0, 3, 0).precision, counts(0, 3, 0).recall), (None, Some(0.0)));
    assert_eq!((counts(4, 0, 0).precision, counts(4, 0, 0).recall), (None, None));
    assert_eq!((counts(4, 2, 1).precision, counts(4, 2, 1).recall), (Some(0.25), Some(0.5)));
  }

  #[test]
  fn should_yield_the_must_change_share_each_pack_section_delivers_at_a_budget() {
    let cost = PackCost {
      budget: 4_000,
      estimated_tokens: 3_650,
      bytes: 19_000,
    };
    let at_budget = budget_yield(cost, &retrieved(), &set(&["a.rs", "b.rs", "c.rs"]));
    assert_eq!((at_budget.budget, at_budget.estimated_tokens, at_budget.bytes), (4_000, 3_650, 19_000));
    assert_eq!((at_budget.must_read.hits, at_budget.must_read.recall), (1, Some(1.0 / 3.0)));
    assert_eq!((at_budget.pack.hits, at_budget.pack.recall), (2, Some(2.0 / 3.0)));
    let nothing_to_change = budget_yield(cost, &retrieved(), &BTreeSet::new());
    assert_eq!((nothing_to_change.must_read.recall, nothing_to_change.pack.recall), (None, None));
    let empty_pack = budget_yield(cost, &Retrieved::default(), &set(&["a.rs"]));
    assert_eq!((empty_pack.must_read.recall, empty_pack.pack.recall), (Some(0.0), Some(0.0)));
  }

  #[test]
  fn should_leave_recall_undefined_without_expected_files() {
    let score = score_files(&retrieved(), &[], |_| BaseFile::Indexed);
    assert_eq!((score.channels.pack.recall, score.channels.pack.precision), (None, None));
    assert_eq!(score.missed, Vec::<String>::new());
  }

  #[test]
  fn should_find_reuse_targets_by_trailing_key_segments_per_channel() {
    let score = score_reuse(
      &retrieved(),
      &["render::targeted".to_owned(), "QueryEngine::dependents".to_owned(), "arguments::depth".to_owned()],
    );
    assert_eq!(
      score.targets,
      vec![
        ReuseHit {
          target: "render::targeted".to_owned(),
          pack: true,
          search: false,
          similar: true
        },
        ReuseHit {
          target: "QueryEngine::dependents".to_owned(),
          pack: false,
          search: true,
          similar: false
        },
        ReuseHit {
          target: "arguments::depth".to_owned(),
          pack: false,
          search: false,
          similar: false
        },
      ]
    );
    assert_eq!(
      (score.pack, score.search, score.similar, score.any),
      (Some(1.0 / 3.0), Some(1.0 / 3.0), Some(1.0 / 3.0), Some(2.0 / 3.0))
    );
    assert_eq!(score_reuse(&retrieved(), &[]).any, None);
  }

  #[test]
  fn should_score_planned_rename_sites_against_the_patch_sites() {
    let site = |path: &str, line: u32| (path.to_owned(), line);
    let expected = BTreeSet::from([site("a.ts", 1), site("a.ts", 7), site("b.ts", 3), site("c.ts", 9)]);
    let planned = BTreeSet::from([site("a.ts", 1), site("a.ts", 7), site("b.ts", 3), site("b.ts", 4), site("d.ts", 1)]);
    let score = score_rename_sites(&expected, &planned);
    assert_eq!((score.sites.relevant, score.sites.retrieved, score.sites.hits), (4, 5, 3));
    assert_eq!((score.sites.recall, score.sites.precision), (Some(0.75), Some(0.6)));
    assert_eq!(score.missed, vec!["c.ts:9"]);
    assert_eq!(score.unexpected, vec!["b.ts:4", "d.ts:1"]);
  }

  #[test]
  fn should_leave_rename_precision_undefined_without_expected_sites() {
    let score = score_rename_sites(&BTreeSet::new(), &BTreeSet::from([("a.ts".to_owned(), 1)]));
    assert_eq!((score.sites.recall, score.sites.precision), (None, None));
  }

  #[test]
  fn should_average_only_tasks_that_define_a_metric() {
    assert_eq!(
      mean_of("recall", [Some(1.0), None, Some(0.5)]),
      MetricMean {
        metric: "recall".to_owned(),
        tasks: 2,
        mean: Some(0.75)
      }
    );
    assert_eq!(mean_of("recall", [None]).mean, None);
  }

  #[test]
  fn should_join_repo_relative_paths_onto_the_repo_directory_in_the_tree() {
    let prefixes = BTreeMap::from([("tree".to_owned(), String::new()), ("acme-api".to_owned(), "acme-api".to_owned())]);
    assert_eq!(tree_path(&prefixes, "tree", "src/a.ts").as_deref(), Some("src/a.ts"));
    assert_eq!(tree_path(&prefixes, "acme-api", "src/a.ts").as_deref(), Some("acme-api/src/a.ts"));
    assert_eq!(tree_path(&prefixes, "other", "src/a.ts"), None);
  }
}
