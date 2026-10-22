use std::collections::HashMap;

use atlas_core::{EntityId, EntityKind, Language, LanguageFamily, RelPath, RepoName};
use serde::Serialize;

use crate::config::SimilarityConfig;
use crate::doc::SimilarityDoc;
use crate::error::SimilarityError;
use crate::lexical::{ExpansionMatch, FieldMatch, LexicalQuery, LexicalStore};
use crate::structural::{
  ClusterReport, StructuralCluster, StructuralDoc, StructuralMatch, StructuralPair, StructuralState, StructuralStatus, StructuralStore, UnionFind, size_ratio,
};

const DEFAULT_SEARCH_LIMIT: usize = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchOptions {
  pub k: usize,
  pub kinds: Vec<EntityKind>,
  pub repos: Vec<RepoName>,
  pub families: Vec<LanguageFamily>,
  pub path_prefix: Option<String>,
}

impl Default for SearchOptions {
  fn default() -> Self {
    Self {
      k: DEFAULT_SEARCH_LIMIT,
      kinds: Vec::new(),
      repos: Vec::new(),
      families: Vec::new(),
      path_prefix: None,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchHit {
  pub entity: EntityId,
  pub kind: EntityKind,
  pub repo: RepoName,
  pub name: String,
  pub path: RelPath,
  pub score: f64,
  pub direct_terms: Vec<String>,
  pub matched_fields: Vec<FieldMatch>,
  pub via_expansion: Vec<ExpansionMatch>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryEstimate {
  pub structural_bytes: usize,
  pub lexical_bytes: usize,
  pub metadata_bytes: usize,
}

impl MemoryEstimate {
  pub fn total(&self) -> usize {
    self.structural_bytes + self.lexical_bytes + self.metadata_bytes
  }
}

pub(crate) struct DocMeta {
  pub entity: EntityId,
  pub kind: EntityKind,
  pub repo: RepoName,
  pub name: String,
  pub path: RelPath,
}

pub struct SimilarityIndex {
  pub(crate) config: SimilarityConfig,
  pub(crate) metas: Vec<Option<DocMeta>>,
  pub(crate) free: Vec<u32>,
  pub(crate) by_entity: HashMap<EntityId, u32>,
  pub(crate) structural: StructuralStore,
  pub(crate) lexical: LexicalStore,
}

impl Default for SimilarityIndex {
  fn default() -> Self {
    Self::unchecked(SimilarityConfig::default())
  }
}

impl SimilarityIndex {
  pub fn new(config: SimilarityConfig) -> Result<Self, SimilarityError> {
    config.validate()?;
    Ok(Self::unchecked(config))
  }

  fn unchecked(config: SimilarityConfig) -> Self {
    Self {
      config,
      metas: Vec::new(),
      free: Vec::new(),
      by_entity: HashMap::new(),
      structural: StructuralStore::new(config),
      lexical: LexicalStore::default(),
    }
  }

  pub fn config(&self) -> SimilarityConfig {
    self.config
  }

  pub fn len(&self) -> usize {
    self.by_entity.len()
  }

  pub fn is_empty(&self) -> bool {
    self.by_entity.is_empty()
  }

  pub fn contains(&self, entity: EntityId) -> bool {
    self.by_entity.contains_key(&entity)
  }

  pub(crate) fn allocate(&mut self, meta: DocMeta) -> Result<u32, SimilarityError> {
    let entity = meta.entity;
    let slot = match self.free.pop() {
      Some(slot) => {
        self.metas[slot as usize] = Some(meta);
        slot
      }
      None => {
        let slot = u32::try_from(self.metas.len()).map_err(|_| SimilarityError::CapacityExceeded)?;
        self.metas.push(Some(meta));
        slot
      }
    };
    self.by_entity.insert(entity, slot);
    Ok(slot)
  }

  pub fn upsert(&mut self, doc: SimilarityDoc) -> Result<(), SimilarityError> {
    self.remove(doc.entity);
    let structural = self.structural.classify(doc.body.as_ref());
    let lexical = self.lexical.analyze(&doc);
    let meta = DocMeta {
      entity: doc.entity,
      kind: doc.kind,
      repo: doc.repo,
      name: doc.name,
      path: doc.path,
    };
    let slot = self.allocate(meta)?;
    self.structural.set(slot, structural);
    self.lexical.insert(slot, lexical);
    Ok(())
  }

  pub fn remove(&mut self, entity: EntityId) -> bool {
    let Some(slot) = self.by_entity.remove(&entity) else { return false };
    self.structural.set(slot, StructuralState::Absent);
    self.lexical.remove(slot);
    self.metas[slot as usize] = None;
    self.free.push(slot);
    true
  }

  pub fn structural_status(&self, entity: EntityId) -> StructuralStatus {
    let Some(&slot) = self.by_entity.get(&entity) else { return StructuralStatus::Unknown };
    match self.structural.state(slot) {
      StructuralState::Absent => StructuralStatus::NoBody,
      StructuralState::TooSmall(tokens) => StructuralStatus::TooSmall { tokens: *tokens },
      StructuralState::LowEntropy(tokens) => StructuralStatus::LowEntropy { tokens: *tokens },
      StructuralState::Indexed(doc) => StructuralStatus::Indexed { tokens: doc.tokens },
    }
  }

  pub fn near_duplicates(&self, entity: EntityId, threshold: f64, k: usize) -> Vec<StructuralMatch> {
    let Some(&slot) = self.by_entity.get(&entity) else { return Vec::new() };
    let StructuralState::Indexed(query) = self.structural.state(slot) else {
      return Vec::new();
    };
    self.rank_matches(query, Some(slot), threshold, k)
  }

  pub fn near_duplicates_of_tokens(&self, tokens: &[u32], threshold: f64, k: usize) -> Vec<StructuralMatch> {
    let Some(query) = self.structural.query_doc(tokens) else { return Vec::new() };
    self.rank_matches(&query, None, threshold, k)
  }

  fn rank_matches(&self, query: &StructuralDoc, exclude: Option<u32>, threshold: f64, k: usize) -> Vec<StructuralMatch> {
    if threshold.is_nan() || k == 0 {
      return Vec::new();
    }
    let mut matches: Vec<StructuralMatch> = self
      .structural
      .verified(query, exclude, threshold)
      .into_iter()
      .filter_map(|verified| {
        let meta = self.metas[verified.slot as usize].as_ref()?;
        Some(StructuralMatch {
          entity: meta.entity,
          kind: meta.kind.clone(),
          repo: meta.repo.clone(),
          name: meta.name.clone(),
          path: meta.path.clone(),
          estimated_jaccard: verified.estimated,
          exact_jaccard: verified.exact,
          query_tokens: query.tokens,
          match_tokens: verified.tokens,
          size_ratio: size_ratio(query.tokens, verified.tokens),
          shingle_size: query.shingle_size,
          small: query.is_small(),
        })
      })
      .collect();
    matches.sort_by(|a, b| b.exact_jaccard.total_cmp(&a.exact_jaccard).then(a.entity.cmp(&b.entity)));
    matches.truncate(k);
    matches
  }

  pub fn clusters(&self, threshold: f64) -> ClusterReport {
    if threshold.is_nan() {
      return ClusterReport {
        clusters: Vec::new(),
        oversized_buckets: 0,
        sampled: false,
      };
    }
    let verified = self.structural.pairs(threshold);
    let pairs = verified.pairs;
    let mut groups = UnionFind::new(self.metas.len());
    for pair in &pairs {
      groups.union(pair.left, pair.right);
    }
    let mut accumulated: HashMap<u32, (Vec<u32>, usize, f64, bool)> = HashMap::new();
    for pair in &pairs {
      let root = groups.find(pair.left);
      let entry = accumulated.entry(root).or_insert_with(|| (Vec::new(), 0, f64::INFINITY, pair.small));
      entry.0.extend([pair.left, pair.right]);
      entry.1 += 1;
      entry.2 = entry.2.min(pair.exact);
    }
    let mut clusters: Vec<StructuralCluster> = accumulated
      .into_values()
      .map(|(mut slots, verified_pairs, min_pair_jaccard, small)| {
        slots.sort_unstable();
        slots.dedup();
        let mut members: Vec<EntityId> = slots.into_iter().filter_map(|slot| self.metas[slot as usize].as_ref().map(|meta| meta.entity)).collect();
        members.sort_unstable();
        StructuralCluster {
          members,
          verified_pairs,
          min_pair_jaccard,
          small,
        }
      })
      .collect();
    clusters.sort_by(|a, b| b.members.len().cmp(&a.members.len()).then_with(|| a.members.cmp(&b.members)));
    ClusterReport {
      clusters,
      oversized_buckets: verified.oversized_buckets,
      sampled: verified.oversized_buckets > 0,
    }
  }

  pub fn structural_pairs(&self, threshold: f64) -> Vec<StructuralPair> {
    if threshold.is_nan() {
      return Vec::new();
    }
    self
      .structural
      .pairs(threshold)
      .pairs
      .into_iter()
      .filter_map(|pair| {
        let (left, right) = (self.metas[pair.left as usize].as_ref()?, self.metas[pair.right as usize].as_ref()?);
        Some(StructuralPair {
          left: left.entity,
          right: right.entity,
          jaccard: pair.exact,
          small: pair.small,
        })
      })
      .collect()
  }

  pub fn search(&self, query: &str, options: &SearchOptions) -> Vec<SearchHit> {
    let parsed = LexicalQuery::parse(query);
    if parsed.direct.is_empty() {
      return Vec::new();
    }
    let family_of = |meta: &DocMeta| {
      meta
        .path
        .as_str()
        .rsplit_once('.')
        .and_then(|(_, extension)| Language::from_extension(extension))
        .map(Language::family)
    };
    let accepts = |meta: &DocMeta| {
      (options.kinds.is_empty() || options.kinds.contains(&meta.kind))
        && (options.repos.is_empty() || options.repos.contains(&meta.repo))
        && (options.families.is_empty() || family_of(meta).is_some_and(|family| options.families.contains(&family)))
        && options.path_prefix.as_deref().is_none_or(|prefix| meta.path.as_str().starts_with(prefix))
    };
    let resolve = |slot: u32| self.metas[slot as usize].as_ref().filter(|meta| accepts(meta)).map(|meta| meta.entity);
    self
      .lexical
      .search(&parsed, options.k, resolve)
      .into_iter()
      .filter_map(|ranked| {
        let meta = self.metas[ranked.slot as usize].as_ref()?;
        let explanation = self.lexical.explain(ranked.slot, &parsed);
        Some(SearchHit {
          entity: ranked.entity,
          kind: meta.kind.clone(),
          repo: meta.repo.clone(),
          name: meta.name.clone(),
          path: meta.path.clone(),
          score: ranked.score,
          direct_terms: explanation.direct_terms,
          matched_fields: explanation.fields,
          via_expansion: explanation.via_expansion,
        })
      })
      .collect()
  }

  pub fn estimated_memory(&self) -> MemoryEstimate {
    let metadata_bytes: usize = self
      .metas
      .iter()
      .flatten()
      .map(|meta| size_of::<DocMeta>() + meta.name.len() + meta.path.as_str().len() + meta.repo.as_str().len() + size_of::<(EntityId, u32)>() * 2)
      .sum();
    MemoryEstimate {
      structural_bytes: self.structural.estimated_bytes(),
      lexical_bytes: self.lexical.estimated_bytes(),
      metadata_bytes,
    }
  }
}
