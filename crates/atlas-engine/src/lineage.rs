use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{BodyFingerprintInput, Certainty, Confidence, ContentHash, Edge, EdgeKind, EntityId, EntityKind, Evidence, Producer, RelPath, RepoName, RunId, Span, SymbolKey};
use atlas_similarity::{SimilarityConfig, SimilarityDoc, SimilarityIndex};
use serde_json::json;

use crate::artifact::FileKey;
use crate::owner::{ENGINE_PRODUCER, ENGINE_PRODUCER_VERSION};

pub(crate) const RULE_IDENTICAL_BODY: &str = "identical-body";
pub(crate) const RULE_SIMILAR_BODY: &str = "similar-body";
pub(crate) const RULE_MOVED_FILE: &str = "moved-file";
const MIN_JACCARD: f64 = 0.85;
const MIN_IDENTICAL_TOKENS: u32 = 12;
const MIN_IDENTICAL_TOKENS_ACROSS_FILES: u32 = 30;
const SMALL_BODY_TOKENS: u32 = 30;
const SMALL_BODY_CONFIDENCE: f64 = 0.6;
const MIN_NAME_TOKEN_LEN: usize = 3;
pub(crate) const MIN_MIGRATION_CONFIDENCE: f32 = 0.85;
const MAX_CANDIDATES: usize = 8;
pub(crate) const MAX_LINEAGE_EDGES: usize = 10_000;

#[derive(Debug, Clone)]
pub(crate) struct LineageSide {
  pub id: EntityId,
  pub key: SymbolKey,
  pub kind: EntityKind,
  pub file: FileKey,
  pub body: Option<BodyFingerprintInput>,
  pub span: Span,
  pub content_hash: ContentHash,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RenamePair {
  pub new: EntityId,
  pub old: EntityId,
  pub old_key: String,
  pub confidence: f32,
  pub rule: &'static str,
  pub evidence: (RepoName, RelPath, Span, ContentHash),
}

fn body_hash(body: &BodyFingerprintInput) -> [u8; 32] {
  let mut hasher = blake3::Hasher::new();
  for token in &body.normalized_tokens {
    hasher.update(&token.to_le_bytes());
  }
  *hasher.finalize().as_bytes()
}

struct Scored {
  old: usize,
  new: usize,
  score: f64,
  rule: &'static str,
}

pub(crate) fn match_renames(disappeared: &[LineageSide], appeared: &[LineageSide], moved_files: &BTreeSet<(FileKey, FileKey)>) -> Vec<RenamePair> {
  if disappeared.is_empty() || appeared.is_empty() {
    return Vec::new();
  }
  let moved = |old: &LineageSide, new: &LineageSide| moved_files.contains(&(old.file.clone(), new.file.clone()));
  let mut scored: Vec<Scored> = Vec::new();
  let appeared_hashes: Vec<Option<[u8; 32]>> = appeared.iter().map(|side| side.body.as_ref().map(body_hash)).collect();
  let mut fuzzy = SimilarityIndex::new(SimilarityConfig::default()).ok();
  if let Some(index) = fuzzy.as_mut() {
    for side in appeared.iter().filter(|side| side.body.is_some()) {
      let doc = SimilarityDoc {
        entity: side.id,
        kind: side.kind.clone(),
        repo: side.file.0.clone(),
        name: side.key.name().to_owned(),
        path: side.file.1.clone(),
        signature: None,
        doc: None,
        referenced_names: Vec::new(),
        body: side.body.clone(),
      };
      if index.upsert(doc).is_err() {
        fuzzy = None;
        break;
      }
    }
  }
  let appeared_by_id: BTreeMap<EntityId, usize> = appeared.iter().enumerate().map(|(index, side)| (side.id, index)).collect();
  for (old_index, old) in disappeared.iter().enumerate() {
    let old_hash = old.body.as_ref().map(body_hash);
    for (new_index, new) in appeared.iter().enumerate() {
      if new.kind != old.kind {
        continue;
      }
      let same_place = old.file == new.file || moved(old, new);
      if moved(old, new) && old.key.item_path() == new.key.item_path() {
        scored.push(Scored {
          old: old_index,
          new: new_index,
          score: 1.0,
          rule: RULE_MOVED_FILE,
        });
        continue;
      }
      let identical = old_hash.is_some() && old_hash == appeared_hashes[new_index];
      let tokens = old.body.as_ref().map_or(0, |body| body.token_count);
      let eligible = tokens >= MIN_IDENTICAL_TOKENS && (same_place || tokens >= MIN_IDENTICAL_TOKENS_ACROSS_FILES);
      if identical && eligible {
        let score = match tokens >= SMALL_BODY_TOKENS || names_overlap(old.key.name(), new.key.name()) {
          true => 1.0,
          false => SMALL_BODY_CONFIDENCE,
        };
        scored.push(Scored {
          old: old_index,
          new: new_index,
          score,
          rule: RULE_IDENTICAL_BODY,
        });
      }
    }
    let (Some(index), Some(body)) = (fuzzy.as_ref(), old.body.as_ref()) else { continue };
    for found in index.near_duplicates_of_tokens(&body.normalized_tokens, MIN_JACCARD, MAX_CANDIDATES) {
      let Some(&new_index) = appeared_by_id.get(&found.entity) else { continue };
      let already = scored.iter().any(|entry| entry.old == old_index && entry.new == new_index);
      if appeared[new_index].kind != old.kind || already || found.exact_jaccard >= 1.0 {
        continue;
      }
      scored.push(Scored {
        old: old_index,
        new: new_index,
        score: found.exact_jaccard,
        rule: RULE_SIMILAR_BODY,
      });
    }
  }
  unique_best_pairs(&scored, disappeared, appeared)
}

fn name_tokens(name: &str) -> BTreeSet<String> {
  let mut tokens = BTreeSet::new();
  let mut current = String::new();
  let mut previous_lower = false;
  for character in name.chars() {
    let boundary = !character.is_ascii_alphanumeric() || (character.is_ascii_uppercase() && previous_lower);
    if boundary && !current.is_empty() {
      tokens.insert(std::mem::take(&mut current));
    }
    if character.is_ascii_alphanumeric() {
      current.push(character.to_ascii_lowercase());
    }
    previous_lower = character.is_ascii_lowercase() || character.is_ascii_digit();
  }
  if !current.is_empty() {
    tokens.insert(current);
  }
  tokens.retain(|token| token.len() >= MIN_NAME_TOKEN_LEN);
  tokens
}

fn names_overlap(left: &str, right: &str) -> bool {
  !name_tokens(left).is_disjoint(&name_tokens(right))
}

fn unique_best(scored: &[&Scored]) -> Option<usize> {
  let best = scored.iter().map(|entry| entry.score).fold(f64::NEG_INFINITY, f64::max);
  let mut winners = scored.iter().enumerate().filter(|(_, entry)| entry.score == best);
  let (index, _) = winners.next()?;
  winners.next().is_none().then_some(index)
}

fn unique_best_pairs(scored: &[Scored], disappeared: &[LineageSide], appeared: &[LineageSide]) -> Vec<RenamePair> {
  let mut pairs = Vec::new();
  for (old_index, old) in disappeared.iter().enumerate() {
    let for_old: Vec<&Scored> = scored.iter().filter(|entry| entry.old == old_index).collect();
    let Some(choice) = unique_best(&for_old) else { continue };
    let chosen = for_old[choice];
    let for_new: Vec<&Scored> = scored.iter().filter(|entry| entry.new == chosen.new).collect();
    let Some(reverse) = unique_best(&for_new) else { continue };
    if for_new[reverse].old != old_index {
      continue;
    }
    let new = &appeared[chosen.new];
    pairs.push(RenamePair {
      new: new.id,
      old: old.id,
      old_key: old.key.to_string(),
      confidence: chosen.score as f32,
      rule: chosen.rule,
      evidence: (new.file.0.clone(), new.file.1.clone(), new.span, new.content_hash),
    });
  }
  pairs.sort_by_key(|pair| (pair.new, pair.old));
  pairs
}

pub(crate) fn lineage_edge(pair: &RenamePair, run: RunId) -> Option<Edge> {
  let confidence = Confidence::new(pair.confidence).ok()?;
  let (repo, path, span, content_hash) = pair.evidence.clone();
  let mut edge = Edge::new(EdgeKind::RenamedFrom, pair.new, pair.old, "", Certainty::heuristic(confidence, pair.rule)).with_evidence(Evidence {
    repo,
    path,
    span,
    content_hash,
    producer: Producer::new(ENGINE_PRODUCER, ENGINE_PRODUCER_VERSION),
    note: None,
  });
  edge.attrs.insert("old_key".to_owned(), json!(pair.old_key));
  edge.attrs.insert("run".to_owned(), json!(run.0));
  edge.attrs.insert("confidence".to_owned(), json!(pair.confidence));
  Some(edge)
}

pub(crate) fn retained_lineage(previous: Vec<Edge>, fresh: Vec<Edge>, exists: impl Fn(EntityId) -> bool) -> Vec<Edge> {
  let mut edges: BTreeMap<_, Edge> = previous.into_iter().chain(fresh).map(|edge| (edge.id, edge)).collect();
  loop {
    let targets: BTreeSet<EntityId> = edges.values().map(|edge| edge.to).collect();
    let before = edges.len();
    edges.retain(|_, edge| exists(edge.from) || targets.contains(&edge.from));
    if edges.len() == before {
      break;
    }
  }
  let mut edges: Vec<Edge> = edges.into_values().collect();
  if edges.len() > MAX_LINEAGE_EDGES {
    edges.sort_by_key(|edge| std::cmp::Reverse(edge.attrs.get("run").and_then(serde_json::Value::as_i64).unwrap_or(0)));
    edges.truncate(MAX_LINEAGE_EDGES);
    edges.sort_by_key(|edge| edge.id);
  }
  edges
}

#[cfg(test)]
mod tests {
  use super::{name_tokens, names_overlap};

  #[test]
  fn should_split_identifiers_into_meaningful_name_tokens() {
    assert_eq!(name_tokens("formatMoney").into_iter().collect::<Vec<_>>(), vec!["format", "money"]);
    assert_eq!(name_tokens("to_cents_v2").into_iter().collect::<Vec<_>>(), vec!["cents"]);
    assert!(names_overlap("formatMoney", "formatCents"));
    assert!(!names_overlap("isAdmin", "isArchived"));
  }
}
