use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{EdgeKind, Entity, EntityId, EntityKind};
use atlas_similarity::{Field, SearchHit, SearchOptions, SimilarityIndex, StructuralStatus, name_words, names_related};
use atlas_store::{EdgeFilter, EntityFilter, ReadTx};

use super::eligibility::{implements_trait, is_production_code};
use super::signature::{Shape, SignatureMatch};
use crate::error::EngineResult;

const STRUCTURAL_THRESHOLD: f64 = 0.6;
const LEXICAL_NEIGHBOURS: usize = 8;
const MAX_LEXICAL_PAIRS_PER_ENTITY: usize = 3;
const MIN_LEXICAL_BODY_TOKENS: usize = 8;
const MIN_LEXICAL_SCORE: f64 = 0.7;
const BM25_WEIGHT: f64 = 0.4;
const NAME_WEIGHT: f64 = 0.35;
const SIGNATURE_WEIGHT: f64 = 0.25;
const CONCEPT_NAME_SCORE: f64 = 0.75;
const MIN_WORD_PREFIX: usize = 3;
const ENTRY_POINTS: [&str; 1] = ["main"];

#[derive(Debug, Clone, PartialEq)]
pub enum NameSignal {
  SharedWords { words: Vec<String>, overlap: f64 },
  RelatedConcepts(Vec<String>),
}

impl NameSignal {
  fn weight(&self) -> f64 {
    match self {
      Self::SharedWords { overlap, .. } => *overlap,
      Self::RelatedConcepts(_) => CONCEPT_NAME_SCORE,
    }
  }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PairSignal {
  Structural { jaccard: f64, small: bool },
  Lexical { bm25: f64, name: NameSignal, signature: SignatureMatch },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PairCandidate {
  pub left: EntityId,
  pub right: EntityId,
  pub left_key: String,
  pub right_key: String,
  pub score: f64,
  pub signal: PairSignal,
}

impl PairCandidate {
  pub fn ids(&self) -> (EntityId, EntityId) {
    (self.left, self.right)
  }
}

fn ordered(a: EntityId, b: EntityId) -> (EntityId, EntityId) {
  if a <= b { (a, b) } else { (b, a) }
}

fn rounded(value: f64) -> f64 {
  (value * 1000.0).round() / 1000.0
}

fn callable(kind: &EntityKind) -> bool {
  matches!(kind, EntityKind::Function | EntityKind::Method)
}

pub(super) fn structural(tx: &ReadTx<'_>, index: &SimilarityIndex) -> EngineResult<Vec<PairCandidate>> {
  let mut pairs = index.structural_pairs(STRUCTURAL_THRESHOLD);
  pairs.sort_by(|a, b| {
    a.small
      .cmp(&b.small)
      .then(b.jaccard.total_cmp(&a.jaccard))
      .then(a.left.cmp(&b.left))
      .then(a.right.cmp(&b.right))
  });
  let mut entities: BTreeMap<EntityId, Option<Entity>> = BTreeMap::new();
  let mut candidates = Vec::new();
  for pair in pairs {
    for id in [pair.left, pair.right] {
      if let Entry::Vacant(slot) = entities.entry(id) {
        slot.insert(tx.entity(id)?);
      }
    }
    let (Some(Some(left)), Some(Some(right))) = (entities.get(&pair.left), entities.get(&pair.right)) else {
      continue;
    };
    let comparable = callable(&left.kind) && left.kind == right.kind && is_production_code(left) && is_production_code(right);
    if !comparable || (pair.small && !names_related(&left.name, &right.name)) {
      continue;
    }
    let (first, second) = if left.id <= right.id { (left, right) } else { (right, left) };
    candidates.push(PairCandidate {
      left: first.id,
      right: second.id,
      left_key: first.key.to_string(),
      right_key: second.key.to_string(),
      score: rounded(pair.jaccard),
      signal: PairSignal::Structural {
        jaccard: rounded(pair.jaccard),
        small: pair.small,
      },
    });
  }
  Ok(candidates)
}

struct Profile {
  entity: Entity,
  words: BTreeSet<String>,
  shape: Option<Shape>,
}

fn body_tokens(status: StructuralStatus) -> usize {
  match status {
    StructuralStatus::TooSmall { tokens } | StructuralStatus::LowEntropy { tokens } | StructuralStatus::Indexed { tokens } => tokens,
    StructuralStatus::Unknown | StructuralStatus::NoBody => 0,
  }
}

fn profiles(tx: &ReadTx<'_>, index: &SimilarityIndex) -> EngineResult<BTreeMap<EntityId, Profile>> {
  let mut profiles = BTreeMap::new();
  for kind in [EntityKind::Function, EntityKind::Method] {
    for entity in tx.entities_by_kind(&kind, &EntityFilter::default())? {
      let words = name_words(&entity.name);
      let eligible = is_production_code(&entity)
        && !implements_trait(&entity)
        && !ENTRY_POINTS.contains(&entity.name.as_str())
        && !words.is_empty()
        && body_tokens(index.structural_status(entity.id)) >= MIN_LEXICAL_BODY_TOKENS;
      if !eligible {
        continue;
      }
      let shape = Shape::of(&entity);
      profiles.insert(entity.id, Profile { entity, words, shape });
    }
  }
  Ok(profiles)
}

fn stems_word(stem: &str, words: &BTreeSet<String>) -> bool {
  words.iter().any(|word| word.starts_with(stem))
}

fn same_word(left: &str, right: &str) -> bool {
  let (short, long) = if left.len() <= right.len() { (left, right) } else { (right, left) };
  short.len() >= MIN_WORD_PREFIX && long.starts_with(short)
}

fn shared_words(left: &BTreeSet<String>, right: &BTreeSet<String>) -> Option<NameSignal> {
  let words: Vec<String> = left.iter().filter(|word| right.iter().any(|other| same_word(word, other))).cloned().collect();
  if words.is_empty() {
    return None;
  }
  let union = left.len() + right.len() - words.len();
  Some(NameSignal::SharedWords {
    overlap: rounded(words.len() as f64 / union.max(1) as f64),
    words,
  })
}

fn name_signal(query: &Profile, other: &Profile, hit: &SearchHit) -> Option<NameSignal> {
  if let Some(shared) = shared_words(&query.words, &other.words) {
    return Some(shared);
  }
  let in_name: BTreeSet<&String> = hit
    .matched_fields
    .iter()
    .filter(|field| field.field == Field::Name)
    .flat_map(|field| &field.terms)
    .collect();
  let mut concepts: Vec<String> = hit
    .via_expansion
    .iter()
    .filter(|expansion| in_name.contains(&expansion.term) && stems_word(&expansion.term, &other.words) && expansion.from.iter().any(|from| stems_word(from, &query.words)))
    .map(|expansion| expansion.term.clone())
    .collect();
  concepts.sort();
  concepts.dedup();
  (!concepts.is_empty()).then_some(NameSignal::RelatedConcepts(concepts))
}

const OPPOSITES: [(&str, &str); 24] = [
  ("to", "from"),
  ("in", "out"),
  ("get", "set"),
  ("add", "remove"),
  ("added", "removed"),
  ("insert", "delete"),
  ("open", "close"),
  ("start", "end"),
  ("begin", "end"),
  ("push", "pop"),
  ("min", "max"),
  ("first", "last"),
  ("before", "after"),
  ("prev", "next"),
  ("encode", "decode"),
  ("serialize", "deserialize"),
  ("read", "write"),
  ("load", "save"),
  ("lock", "unlock"),
  ("enable", "disable"),
  ("show", "hide"),
  ("leading", "trailing"),
  ("head", "tail"),
  ("forward", "reverse"),
];

fn lower_words(name: &str) -> BTreeSet<String> {
  let mut words = BTreeSet::new();
  let mut current = String::new();
  let mut previous_lower = false;
  for ch in name.chars() {
    if (!ch.is_alphanumeric() || (previous_lower && ch.is_uppercase())) && !current.is_empty() {
      words.insert(std::mem::take(&mut current));
    }
    previous_lower = ch.is_lowercase() || ch.is_ascii_digit();
    if ch.is_alphanumeric() {
      current.extend(ch.to_lowercase());
    }
  }
  if !current.is_empty() {
    words.insert(current);
  }
  words
}

fn mirrored(left: &str, right: &str) -> bool {
  let (left, right) = (lower_words(left), lower_words(right));
  let only_left: Vec<&String> = left.difference(&right).collect();
  let only_right: Vec<&String> = right.difference(&left).collect();
  let opposite =
    |a: &str, b: &str| OPPOSITES.iter().any(|(x, y)| (a == *x && b == *y) || (a == *y && b == *x)) || a.strip_prefix("un") == Some(b) || b.strip_prefix("un") == Some(a);
  only_left.iter().any(|a| only_right.iter().any(|b| opposite(a, b)))
}

fn disambiguated_siblings(left: &Entity, right: &Entity) -> bool {
  let (Some(left), Some(right)) = (left.key.as_symbol(), right.key.as_symbol()) else {
    return false;
  };
  left.disambiguator() != right.disambiguator()
    && left.family() == right.family()
    && left.repo() == right.repo()
    && left.package() == right.package()
    && left.module_path() == right.module_path()
    && left.item_path() == right.item_path()
    && left.kind() == right.kind()
}

fn same_name_methods(query: &Profile, other: &Profile) -> bool {
  query.entity.kind == EntityKind::Method && query.entity.name == other.entity.name
}

fn delegates(tx: &ReadTx<'_>, left: EntityId, right: EntityId) -> EngineResult<bool> {
  let calls = EdgeFilter {
    kinds: vec![EdgeKind::Calls],
    ..EdgeFilter::default()
  };
  Ok(tx.edges_from(left, &calls)?.iter().any(|edge| edge.to == right) || tx.edges_from(right, &calls)?.iter().any(|edge| edge.to == left))
}

fn signature_match(query: &Profile, other: &Profile) -> SignatureMatch {
  match (&query.shape, &other.shape) {
    (Some(left), Some(right)) => left.compare(right),
    _ => SignatureMatch::SameArity,
  }
}

pub(super) fn lexical(tx: &ReadTx<'_>, index: &SimilarityIndex, exclude: &BTreeSet<(EntityId, EntityId)>) -> EngineResult<Vec<PairCandidate>> {
  let profiles = profiles(tx, index)?;
  let mut best: BTreeMap<(EntityId, EntityId), PairCandidate> = BTreeMap::new();
  for (id, query) in &profiles {
    let options = SearchOptions {
      k: LEXICAL_NEIGHBOURS + 1,
      kinds: vec![query.entity.kind.clone()],
      repos: Vec::new(),
      ..SearchOptions::default()
    };
    let hits = index.search(&query.entity.name, &options);
    let Some(top) = hits.iter().find(|hit| hit.entity == *id).or(hits.first()).map(|hit| hit.score).filter(|score| *score > 0.0) else {
      continue;
    };
    for hit in hits.iter().filter(|hit| hit.entity != *id) {
      let pair = ordered(*id, hit.entity);
      let Some(other) = profiles.get(&hit.entity).filter(|other| {
        !exclude.contains(&pair) && !same_name_methods(query, other) && !mirrored(&query.entity.name, &other.entity.name) && !disambiguated_siblings(&query.entity, &other.entity)
      }) else {
        continue;
      };
      let Some(name) = name_signal(query, other, hit) else { continue };
      let signature = signature_match(query, other);
      if signature == SignatureMatch::Incompatible {
        continue;
      }
      let bm25 = rounded((hit.score / top).min(1.0));
      let score = rounded(BM25_WEIGHT * bm25 + NAME_WEIGHT * name.weight() + SIGNATURE_WEIGHT * signature.weight());
      if score < MIN_LEXICAL_SCORE || best.get(&pair).is_some_and(|current| current.score >= score) || delegates(tx, pair.0, pair.1)? {
        continue;
      }
      let (first, second) = if *id == pair.0 { (query, other) } else { (other, query) };
      best.insert(
        pair,
        PairCandidate {
          left: pair.0,
          right: pair.1,
          left_key: first.entity.key.to_string(),
          right_key: second.entity.key.to_string(),
          score,
          signal: PairSignal::Lexical { bm25, name, signature },
        },
      );
    }
  }
  let mut ranked: Vec<PairCandidate> = best.into_values().collect();
  ranked.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.left.cmp(&b.left)).then(a.right.cmp(&b.right)));
  let mut per_entity: BTreeMap<EntityId, usize> = BTreeMap::new();
  ranked.retain(|candidate| {
    let full = [candidate.left, candidate.right]
      .iter()
      .any(|id| per_entity.get(id).copied().unwrap_or(0) >= MAX_LEXICAL_PAIRS_PER_ENTITY);
    if full {
      return false;
    }
    for id in [candidate.left, candidate.right] {
      *per_entity.entry(id).or_default() += 1;
    }
    true
  });
  Ok(ranked)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn words(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|word| (*word).to_owned()).collect()
  }

  #[test]
  fn should_weigh_shared_name_words_by_overlap_and_prefix() {
    assert_eq!(
      shared_words(&words(&["sum", "loop"]), &words(&["sum", "reduce"])),
      Some(NameSignal::SharedWords {
        words: vec!["sum".to_owned()],
        overlap: 0.333
      })
    );
    assert_eq!(
      shared_words(&words(&["tag"]), &words(&["tagged", "short"])),
      Some(NameSignal::SharedWords {
        words: vec!["tag".to_owned()],
        overlap: 0.5
      })
    );
    assert_eq!(shared_words(&words(&["cents"]), &words(&["money"])), None);
  }

  fn function(module: &str, name: &str, disambiguator: u32) -> Entity {
    let repo = atlas_core::RepoName::new("repo").unwrap();
    let key = atlas_core::SymbolKey::new(
      atlas_core::LanguageFamily::Rust,
      repo,
      "pkg",
      vec![module.to_owned()],
      vec![name.to_owned()],
      EntityKind::Function,
      disambiguator,
    )
    .unwrap();
    Entity::new(atlas_core::EntityKey::Symbol(key), atlas_core::Certainty::Syntactic)
  }

  #[test]
  fn should_treat_only_disambiguated_twins_in_one_module_as_siblings() {
    assert!(disambiguated_siblings(
      &function("runtime", "termination_signal", 0),
      &function("runtime", "termination_signal", 1)
    ));
    assert!(!disambiguated_siblings(
      &function("runtime", "termination_signal", 0),
      &function("signals", "termination_signal", 0)
    ));
    assert!(!disambiguated_siblings(
      &function("runtime", "termination_signal", 0),
      &function("runtime", "termination_hook", 1)
    ));
  }

  #[test]
  fn should_recognise_mirrored_names_but_not_variants() {
    assert!(mirrored("edges_to", "edges_from"));
    assert!(mirrored("isOpenDelimiter", "isCloseDelimiter"));
    assert!(mirrored("index_modules", "unindex_modules"));
    assert!(!mirrored("toCents", "to_cents"));
    assert!(!mirrored("sumLoop", "sumReduce"));
    assert!(!mirrored("getUser", "fetchUser"));
  }
}
