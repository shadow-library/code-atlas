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
  FileScore {
    missed: relevant.difference(&any).cloned().collect(),
    channels: ChannelScores {
      must_read: overlap(&retrieved.must_read_files, &relevant),
      pack: overlap(&retrieved.pack_files, &relevant),
      search: overlap(&retrieved.search_files, &relevant),
      similar: overlap(&retrieved.similar_files, &relevant),
      any: overlap(&any, &relevant),
    },
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
