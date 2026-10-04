use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{BodyFingerprintInput, Certainty, Confidence, ContentHash, Edge, EdgeKind, EntityId, EntityKind, Evidence, Producer, RelPath, RepoName, RunId, Span, SymbolKey};
use atlas_similarity::{SimilarityConfig, SimilarityDoc, SimilarityIndex, names_related};
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
pub(crate) const MIN_MIGRATION_CONFIDENCE: f32 = 0.85;
const MAX_CANDIDATES: usize = 8;
const FUZZY_MIN_TOKENS: usize = 40;
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
  let mut fuzzy = SimilarityIndex::new(SimilarityConfig {
    min_tokens: FUZZY_MIN_TOKENS,
    ..SimilarityConfig::default()
  })
  .ok();
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
        let score = match tokens >= SMALL_BODY_TOKENS || names_related(old.key.name(), new.key.name()) {
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
  use atlas_core::{ContentHash, EntityId, LanguageFamily, RelPath, RepoName, Span, SymbolKey};

  use super::*;

  fn side(id: u64, name: &str, tokens: Vec<u32>) -> LineageSide {
    let repo = RepoName::new("repo").unwrap();
    let key = SymbolKey::new(
      LanguageFamily::Ts,
      repo.clone(),
      "pkg",
      vec!["m".to_owned()],
      vec![name.to_owned()],
      EntityKind::Function,
      0,
    )
    .unwrap();
    LineageSide {
      id: EntityId::from_raw(id),
      key,
      kind: EntityKind::Function,
      file: (repo, RelPath::new("src/a.ts").unwrap()),
      body: Some(BodyFingerprintInput {
        token_count: tokens.len() as u32,
        line_count: 1,
        normalized_tokens: tokens,
      }),
      span: Span::from_zero_based_rows(0, 1, 0, 0, 0, 1),
      content_hash: ContentHash::of(name.as_bytes()),
    }
  }

  fn body_of(len: u32) -> Vec<u32> {
    (0..len).map(|i| 16 + (i.wrapping_mul(2_654_435_761) >> 7) % 97).collect()
  }

  fn with_last_token_changed(mut tokens: Vec<u32>) -> Vec<u32> {
    *tokens.last_mut().unwrap() = 900;
    tokens
  }

  #[test]
  fn should_not_fuzzy_match_small_bodies_as_renames() {
    let old = side(1, "alpha", body_of(30));
    let new = side(2, "omega", with_last_token_changed(body_of(30)));
    assert!(match_renames(&[old], &[new], &BTreeSet::new()).is_empty());
  }

  #[test]
  fn should_still_fuzzy_match_large_bodies_as_renames() {
    let old = side(1, "alpha", body_of(60));
    let new = side(2, "omega", with_last_token_changed(body_of(60)));
    let pairs = match_renames(&[old], &[new], &BTreeSet::new());
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].rule, RULE_SIMILAR_BODY);
  }

  #[test]
  fn should_match_identical_small_bodies_only_through_the_hash_rule() {
    let old = side(1, "alpha", body_of(30));
    let new = side(2, "omega", body_of(30));
    let pairs = match_renames(&[old], &[new], &BTreeSet::new());
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].rule, RULE_IDENTICAL_BODY);
  }

  #[test]
  fn should_trust_an_identical_small_body_fully_only_when_the_names_share_a_meaningful_word() {
    let related = match_renames(&[side(1, "getUserName", body_of(20))], &[side(2, "loadUserName", body_of(20))], &BTreeSet::new());
    assert_eq!(related[0].confidence, 1.0);
    let generic = match_renames(&[side(1, "getUser", body_of(20))], &[side(2, "getOrder", body_of(20))], &BTreeSet::new());
    assert_eq!(generic[0].confidence, SMALL_BODY_CONFIDENCE as f32);
    let accessors = match_renames(&[side(1, "get_by_id", body_of(20))], &[side(2, "set_by_id", body_of(20))], &BTreeSet::new());
    assert_eq!(accessors[0].confidence, SMALL_BODY_CONFIDENCE as f32);
  }
}
