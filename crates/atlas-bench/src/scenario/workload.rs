use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use atlas_core::{EntityKind, Language};
use atlas_engine::Engine;
use atlas_store::EntityFilter;

use crate::error::BenchResult;
use crate::source::{self, SourceFile, bump_numbers};

const MIN_HUB_NAME_LEN: usize = 6;
const HUB_TARGETS: usize = 10;
const STRIDE_SHARE_PERCENT: usize = 70;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
  pub key: String,
  pub name: String,
  pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editable {
  pub abs: PathBuf,
  pub rel: String,
  pub language: Language,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameCandidate {
  pub name: String,
  pub files: usize,
}

#[derive(Debug, Clone)]
pub struct Workload {
  pub targets: Vec<Target>,
  pub editable: Vec<Editable>,
  pub rename: Option<RenameCandidate>,
}

fn identifiers(text: &str) -> impl Iterator<Item = &str> {
  let bytes = text.as_bytes();
  let mut index = 0;
  std::iter::from_fn(move || {
    while index < bytes.len() {
      if !(bytes[index].is_ascii_alphabetic() || bytes[index] == b'_') {
        index += 1;
        continue;
      }
      let start = index;
      while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_') {
        index += 1;
      }
      return Some(&text[start..index]);
    }
    None
  })
}

pub fn rank_by_file_usage(texts: impl Iterator<Item = String>, names: &HashSet<String>) -> Vec<(String, usize)> {
  let mut usage: HashMap<&str, usize> = HashMap::new();
  let texts: Vec<String> = texts.collect();
  for text in &texts {
    let seen: HashSet<&str> = identifiers(text).filter(|word| names.contains(*word)).collect();
    for word in seen {
      *usage.entry(word).or_default() += 1;
    }
  }
  let mut ranked: Vec<(String, usize)> = usage.into_iter().map(|(name, files)| (name.to_owned(), files)).collect();
  ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
  ranked
}

fn stride_pick<T: Clone>(items: &[T], count: usize) -> Vec<T> {
  if items.len() <= count {
    return items.to_vec();
  }
  (0..count).map(|index| items[index * items.len() / count].clone()).collect()
}

impl Workload {
  pub fn prepare(engine: &Engine, sources: &[SourceFile], total_targets: usize) -> BenchResult<Option<Self>> {
    let filter = EntityFilter::default();
    let (functions, methods) = engine
      .store()
      .read(|tx| -> BenchResult<_> { Ok((tx.entities_by_kind(&EntityKind::Function, &filter)?, tx.entities_by_kind(&EntityKind::Method, &filter)?)) })?;
    let mut candidates: BTreeMap<String, Target> = BTreeMap::new();
    for entity in functions.iter().chain(methods.iter()) {
      let Some(repo) = &entity.repo else { continue };
      candidates.insert(
        entity.key.to_string(),
        Target {
          key: entity.key.to_string(),
          name: entity.name.clone(),
          repo: repo.to_string(),
        },
      );
    }
    if candidates.is_empty() {
      return Ok(None);
    }
    let mut editable = Vec::new();
    let mut texts = Vec::with_capacity(sources.len());
    for file in sources {
      let text = source::read(&file.abs)?;
      if bump_numbers(&text, 1).is_some() {
        editable.push(Editable {
          abs: file.abs.clone(),
          rel: file.rel.clone(),
          language: file.language,
        });
      }
      texts.push(text);
    }
    let function_names: HashSet<String> = functions
      .iter()
      .filter(|entity| entity.name.len() >= MIN_HUB_NAME_LEN)
      .map(|entity| entity.name.clone())
      .collect();
    let ranked = rank_by_file_usage(texts.into_iter(), &function_names);
    let by_name: HashMap<&str, &Target> = candidates.values().map(|target| (target.name.as_str(), target)).collect();
    let hubs: Vec<Target> = ranked
      .iter()
      .filter_map(|(name, _)| by_name.get(name.as_str()).map(|target| (*target).clone()))
      .take(HUB_TARGETS)
      .collect();
    let pool: Vec<Target> = candidates.values().cloned().collect();
    let strided = stride_pick(&pool, total_targets * STRIDE_SHARE_PERCENT / 100);
    let mut targets: Vec<Target> = Vec::new();
    for target in hubs.into_iter().chain(strided).chain(pool.iter().cloned()) {
      if targets.len() >= total_targets {
        break;
      }
      if targets.iter().all(|existing| existing.key != target.key) {
        targets.push(target);
      }
    }
    let rename = ranked.first().filter(|(_, files)| *files >= 2).map(|(name, files)| RenameCandidate {
      name: name.clone(),
      files: *files,
    });
    Ok(Some(Self { targets, editable, rename }))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_tokenise_identifiers() {
    let found: Vec<&str> = identifiers("let a_b = c9(1) + 2x;").collect();
    assert_eq!(found, vec!["let", "a_b", "c9", "x"]);
  }

  #[test]
  fn should_rank_names_by_the_number_of_files_using_them() {
    let names: HashSet<String> = ["alpha_fn", "beta_fn"].iter().map(|name| (*name).to_owned()).collect();
    let texts = vec!["alpha_fn(); alpha_fn(); beta_fn();".to_owned(), "alpha_fn();".to_owned(), "other();".to_owned()];
    assert_eq!(rank_by_file_usage(texts.into_iter(), &names), vec![("alpha_fn".to_owned(), 2), ("beta_fn".to_owned(), 1)]);
  }

  #[test]
  fn should_break_ranking_ties_by_name() {
    let names: HashSet<String> = ["zeta_fn", "alpha_fn"].iter().map(|name| (*name).to_owned()).collect();
    let ranked = rank_by_file_usage(vec!["zeta_fn alpha_fn".to_owned()].into_iter(), &names);
    assert_eq!(ranked[0].0, "alpha_fn");
  }

  #[test]
  fn should_pick_evenly_spaced_items() {
    assert_eq!(stride_pick(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9], 5), vec![0, 2, 4, 6, 8]);
    assert_eq!(stride_pick(&[1, 2], 5), vec![1, 2]);
  }
}
