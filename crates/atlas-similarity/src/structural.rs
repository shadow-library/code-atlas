use std::collections::{HashMap, HashSet};

use atlas_core::{BodyFingerprintInput, EntityId, EntityKind, MAX_BODY_TOKENS, RelPath, RepoName};
use serde::Serialize;

use crate::config::SimilarityConfig;
use crate::hashing::IdentityBuild;
use crate::minhash::{self, LANES, SMALL_SHINGLE_SIZE, Signature};

const MAX_CLUSTER_BUCKET: usize = 512;
const OVERSIZED_BUCKET_SAMPLE: usize = 16;
const MIN_SMALL_DISTINCT_TOKENS: usize = 5;
const MIN_SMALL_DISTINCT_SHINGLES: usize = 6;
pub const SMALL_PAIR_MIN_JACCARD: f64 = 0.85;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StructuralMatch {
  pub entity: EntityId,
  pub kind: EntityKind,
  pub repo: RepoName,
  pub name: String,
  pub path: RelPath,
  pub estimated_jaccard: f64,
  pub exact_jaccard: f64,
  pub query_tokens: usize,
  pub match_tokens: usize,
  pub size_ratio: f64,
  pub shingle_size: usize,
  pub small: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StructuralCluster {
  pub members: Vec<EntityId>,
  pub verified_pairs: usize,
  pub min_pair_jaccard: f64,
  pub small: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StructuralPair {
  pub left: EntityId,
  pub right: EntityId,
  pub jaccard: f64,
  pub small: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClusterReport {
  pub clusters: Vec<StructuralCluster>,
  pub oversized_buckets: usize,
  pub sampled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StructuralStatus {
  Unknown,
  NoBody,
  TooSmall { tokens: usize },
  LowEntropy { tokens: usize },
  Indexed { tokens: usize },
}

pub(crate) struct StructuralDoc {
  pub tokens: usize,
  pub shingle_size: usize,
  pub shingles: Vec<u64>,
  pub signature: Signature,
}

impl StructuralDoc {
  pub fn from_shingles(tokens: usize, shingle_size: usize, shingles: Vec<u64>) -> Self {
    let signature = minhash::signature(&shingles);
    Self {
      tokens,
      shingle_size,
      shingles,
      signature,
    }
  }

  fn from_tokens(tokens: &[u32]) -> Option<Self> {
    let shingle_size = minhash::shingle_size(tokens.len());
    let shingles = minhash::shingles(tokens, shingle_size);
    let doc = Self::from_shingles(tokens.len(), shingle_size, shingles);
    (!doc.is_small() || doc.is_substantial(tokens)).then_some(doc)
  }

  pub fn is_small(&self) -> bool {
    self.shingle_size == SMALL_SHINGLE_SIZE
  }

  fn is_substantial(&self, tokens: &[u32]) -> bool {
    let mut distinct = tokens.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    distinct.len() >= MIN_SMALL_DISTINCT_TOKENS && self.shingles.len() >= MIN_SMALL_DISTINCT_SHINGLES
  }

  fn exact_against(&self, other: &Self, threshold: f64) -> Option<f64> {
    // Bodies of 39 and 40 tokens use different shingle sizes, so their shingle sets are not comparable and such pairs are skipped.
    if self.shingle_size != other.shingle_size {
      return None;
    }
    let threshold = if self.is_small() { threshold.max(SMALL_PAIR_MIN_JACCARD) } else { threshold };
    let exact = minhash::exact_jaccard(&self.shingles, &other.shingles);
    (exact >= threshold).then_some(exact)
  }
}

#[derive(Default)]
pub(crate) enum StructuralState {
  #[default]
  Absent,
  TooSmall(usize),
  LowEntropy(usize),
  Indexed(StructuralDoc),
}

enum Bucket {
  One(u32),
  Many(Vec<u32>),
}

impl Bucket {
  fn slots(&self) -> &[u32] {
    match self {
      Self::One(slot) => std::slice::from_ref(slot),
      Self::Many(slots) => slots,
    }
  }

  fn insert(&mut self, slot: u32) {
    match self {
      Self::One(existing) => *self = Self::Many(vec![*existing, slot]),
      Self::Many(slots) => slots.push(slot),
    }
  }

  fn remove(&mut self, slot: u32) -> bool {
    match self {
      Self::One(existing) => *existing == slot,
      Self::Many(slots) => {
        if let Some(position) = slots.iter().position(|&candidate| candidate == slot) {
          slots.swap_remove(position);
        }
        slots.is_empty()
      }
    }
  }
}

pub(crate) struct Verified {
  pub slot: u32,
  pub estimated: f64,
  pub exact: f64,
  pub tokens: usize,
}

pub(crate) struct VerifiedPairs {
  pub pairs: Vec<VerifiedPair>,
  pub oversized_buckets: usize,
}

pub(crate) struct VerifiedPair {
  pub left: u32,
  pub right: u32,
  pub exact: f64,
  pub small: bool,
}

pub(crate) struct StructuralStore {
  config: SimilarityConfig,
  buckets: Vec<HashMap<u64, Bucket, IdentityBuild>>,
  states: Vec<StructuralState>,
}

impl StructuralStore {
  pub fn new(config: SimilarityConfig) -> Self {
    Self {
      config,
      buckets: (0..config.bands).map(|_| HashMap::default()).collect(),
      states: Vec::new(),
    }
  }

  pub fn effective_min_tokens(&self) -> usize {
    self.config.min_tokens.max(SMALL_SHINGLE_SIZE)
  }

  pub fn classify(&self, body: Option<&BodyFingerprintInput>) -> StructuralState {
    let Some(body) = body else { return StructuralState::Absent };
    let tokens = &body.normalized_tokens[..body.normalized_tokens.len().min(MAX_BODY_TOKENS)];
    if tokens.len() < self.effective_min_tokens() {
      return StructuralState::TooSmall(tokens.len());
    }
    StructuralDoc::from_tokens(tokens).map_or(StructuralState::LowEntropy(tokens.len()), StructuralState::Indexed)
  }

  pub fn query_doc(&self, tokens: &[u32]) -> Option<StructuralDoc> {
    let tokens = &tokens[..tokens.len().min(MAX_BODY_TOKENS)];
    if tokens.len() < self.effective_min_tokens() {
      return None;
    }
    StructuralDoc::from_tokens(tokens)
  }

  pub fn state(&self, slot: u32) -> &StructuralState {
    self.states.get(slot as usize).unwrap_or(&StructuralState::Absent)
  }

  pub fn set(&mut self, slot: u32, state: StructuralState) {
    let index = slot as usize;
    if self.states.len() <= index {
      self.states.resize_with(index + 1, StructuralState::default);
    }
    if let StructuralState::Indexed(previous) = std::mem::take(&mut self.states[index]) {
      self.unlink(slot, &previous.signature);
    }
    if let StructuralState::Indexed(doc) = &state {
      self.link(slot, &doc.signature);
    }
    self.states[index] = state;
  }

  fn band_keys<'a>(&self, signature: &'a [u32; LANES]) -> impl Iterator<Item = u64> + 'a {
    let rows = self.config.rows;
    (0..self.config.bands).map(move |band| minhash::band_key(band, &signature[band * rows..(band + 1) * rows]))
  }

  fn link(&mut self, slot: u32, signature: &[u32; LANES]) {
    let keys: Vec<u64> = self.band_keys(signature).collect();
    for (band, key) in keys.into_iter().enumerate() {
      self.buckets[band].entry(key).and_modify(|bucket| bucket.insert(slot)).or_insert(Bucket::One(slot));
    }
  }

  fn unlink(&mut self, slot: u32, signature: &[u32; LANES]) {
    let keys: Vec<u64> = self.band_keys(signature).collect();
    for (band, key) in keys.into_iter().enumerate() {
      let Some(bucket) = self.buckets[band].get_mut(&key) else { continue };
      if bucket.remove(slot) {
        self.buckets[band].remove(&key);
      }
    }
  }

  fn candidates(&self, signature: &[u32; LANES]) -> Vec<u32> {
    let mut slots: Vec<u32> = Vec::new();
    for (band, key) in self.band_keys(signature).enumerate() {
      if let Some(bucket) = self.buckets[band].get(&key) {
        slots.extend_from_slice(bucket.slots());
      }
    }
    slots.sort_unstable();
    slots.dedup();
    slots
  }

  pub fn verified(&self, query: &StructuralDoc, exclude: Option<u32>, threshold: f64) -> Vec<Verified> {
    let mut found = Vec::new();
    for slot in self.candidates(&query.signature) {
      if Some(slot) == exclude {
        continue;
      }
      let StructuralState::Indexed(doc) = self.state(slot) else { continue };
      let Some(exact) = query.exact_against(doc, threshold) else { continue };
      found.push(Verified {
        slot,
        estimated: minhash::estimated_jaccard(&query.signature, &doc.signature),
        exact,
        tokens: doc.tokens,
      });
    }
    found
  }

  pub fn pairs(&self, threshold: f64) -> VerifiedPairs {
    let mut seen: HashSet<u64> = HashSet::new();
    let mut oversized_buckets = 0;
    let mut link = |left: u32, right: u32| {
      let (low, high) = (left.min(right), left.max(right));
      seen.insert(u64::from(low) << 32 | u64::from(high));
    };
    for (band, buckets) in self.buckets.iter().enumerate() {
      for bucket in buckets.values() {
        let slots = bucket.slots();
        if slots.len() <= MAX_CLUSTER_BUCKET {
          for (i, &left) in slots.iter().enumerate() {
            for &right in &slots[i + 1..] {
              link(left, right);
            }
          }
          continue;
        }
        oversized_buckets += 1;
        tracing::warn!(
          band,
          bucket_size = slots.len(),
          limit = MAX_CLUSTER_BUCKET,
          "sampling oversized lsh bucket while clustering"
        );
        let mut ordered = slots.to_vec();
        ordered.sort_unstable();
        for (i, &slot) in ordered.iter().enumerate() {
          if i > 0 {
            link(ordered[i - 1], slot);
          }
          for &anchor in ordered.iter().take(OVERSIZED_BUCKET_SAMPLE).filter(|&&anchor| anchor != slot) {
            link(anchor, slot);
          }
        }
      }
    }
    let mut keys: Vec<u64> = seen.into_iter().collect();
    keys.sort_unstable();
    let pairs = keys
      .into_iter()
      .filter_map(|key| {
        let (left, right) = ((key >> 32) as u32, key as u32);
        let (StructuralState::Indexed(a), StructuralState::Indexed(b)) = (self.state(left), self.state(right)) else {
          return None;
        };
        a.exact_against(b, threshold).map(|exact| VerifiedPair {
          left,
          right,
          exact,
          small: a.is_small(),
        })
      })
      .collect();
    VerifiedPairs { pairs, oversized_buckets }
  }

  pub fn estimated_bytes(&self) -> usize {
    let bucket_entries: usize = self.buckets.iter().map(HashMap::len).sum();
    let spilled: usize = self
      .buckets
      .iter()
      .flat_map(HashMap::values)
      .map(|bucket| match bucket {
        Bucket::One(_) => 0,
        Bucket::Many(slots) => slots.capacity() * size_of::<u32>(),
      })
      .sum();
    let docs: usize = self
      .states
      .iter()
      .map(|state| match state {
        StructuralState::Indexed(doc) => doc.shingles.capacity() * size_of::<u64>() + size_of::<[u32; LANES]>(),
        _ => 0,
      })
      .sum();
    bucket_entries * (size_of::<u64>() + size_of::<Bucket>() + 1) + spilled + docs + self.states.capacity() * size_of::<StructuralState>()
  }
}

pub(crate) struct UnionFind {
  parent: Vec<u32>,
}

impl UnionFind {
  pub fn new(len: usize) -> Self {
    Self {
      parent: (0..len as u32).collect(),
    }
  }

  pub fn find(&mut self, node: u32) -> u32 {
    let mut root = node;
    while self.parent[root as usize] != root {
      root = self.parent[root as usize];
    }
    let mut current = node;
    while self.parent[current as usize] != root {
      let next = self.parent[current as usize];
      self.parent[current as usize] = root;
      current = next;
    }
    root
  }

  pub fn union(&mut self, left: u32, right: u32) {
    let (left, right) = (self.find(left), self.find(right));
    if left == right {
      return;
    }
    let (low, high) = (left.min(right), left.max(right));
    self.parent[high as usize] = low;
  }
}

pub(crate) fn size_ratio(left: usize, right: usize) -> f64 {
  let (small, large) = (left.min(right), left.max(right));
  if large == 0 {
    return 0.0;
  }
  small as f64 / large as f64
}
