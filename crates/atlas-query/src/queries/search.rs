use std::collections::BTreeSet;

use atlas_core::{Entity, EntityId, EntityKind, LanguageFamily, RepoName};
use atlas_semantic::RELATION_CAPABILITY_SUMMARY;
use atlas_similarity::{Field, SearchHit, SearchOptions, SimilarityIndex};
use atlas_store::{FreshnessPolicy, InferredQuery};
use serde::Serialize;

use crate::QueryEngine;
use crate::error::QueryResult;
use crate::graph::Graph;
use crate::model::{Caveat, CertaintyTag, EntityRef, Response};
use crate::policy::{InferredAdmission, Policy};
use crate::queries::common::{first_line, is_test};

const DEFAULT_SEARCH_LIMIT: usize = 10;
const STRONG_FRACTION: f64 = 0.6;
const COVERAGE_WEIGHT: f64 = 1.0;
const STRUCTURAL_THRESHOLD: f64 = 0.6;
const STRUCTURAL_SEEDS: usize = 3;
const STRUCTURAL_PER_SEED: usize = 5;
const STRUCTURAL_CANDIDATE_FACTOR: usize = 4;
const NAME_COVERAGE_WEIGHT: f64 = 0.5;
const STOPWORDS: [&str; 10] = ["a", "an", "the", "of", "and", "or", "for", "in", "on", "with"];

#[derive(Debug, Clone, PartialEq)]
pub struct SearchRequest {
  pub text: String,
  pub kinds: Vec<EntityKind>,
  pub repo: Option<String>,
  pub path_prefix: Option<String>,
  pub limit: usize,
  pub include_stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRestriction {
  pub repo: String,
  pub path_prefix: Option<String>,
}

impl SearchRequest {
  fn admits(&self, entity: &Entity) -> bool {
    let Some(prefix) = &self.path_prefix else { return true };
    let in_repo = self
      .repo
      .as_deref()
      .is_none_or(|repo| entity.repo.as_ref().is_some_and(|candidate| candidate.as_str() == repo));
    in_repo && entity.path.as_ref().is_some_and(|path| path.as_str().starts_with(prefix.as_str()))
  }

  pub fn new(text: impl Into<String>) -> Self {
    Self {
      text: text.into(),
      kinds: Vec::new(),
      repo: None,
      path_prefix: None,
      limit: DEFAULT_SEARCH_LIMIT,
      include_stale: false,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchHitView {
  pub entity: EntityRef,
  pub signature: Option<String>,
  pub doc: Option<String>,
  pub score: f64,
  pub matched_fields: Vec<String>,
  pub direct_terms: Vec<String>,
  pub expansion_terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StructuralHit {
  pub entity: EntityRef,
  pub similar_to: String,
  pub jaccard: f64,
  pub small_body: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InferredHit {
  pub entity: EntityRef,
  pub summary: String,
  pub matched_terms: Vec<String>,
  pub certainty: CertaintyTag,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchAnswer {
  pub query: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub scope: Option<String>,
  pub total_hits: usize,
  pub exact_name: Vec<SearchHitView>,
  pub strong: Vec<SearchHitView>,
  pub related: Vec<SearchHitView>,
  pub structurally_similar: Vec<StructuralHit>,
  pub inferred_capabilities: Vec<InferredHit>,
  pub caveats: Vec<Caveat>,
}

impl QueryEngine {
  pub fn search(&self, request: &SearchRequest) -> QueryResult<Response<SearchAnswer>> {
    let freshness = self.barrier()?;
    let index = self.engine().similarity();
    let policy = Policy {
      min_confidence: 0.0,
      include_stale: request.include_stale,
    };
    let answer = self.read(policy, |graph| search(graph, &index, request))?;
    Ok(Response { freshness, answer })
  }

  pub fn search_scope(&self, value: &str) -> QueryResult<Option<SearchRestriction>> {
    let repos = self.repos();
    let known = |name: &str| repos.iter().find(|repo| repo.as_str() == name);
    if known(value).is_some() {
      return Ok(Some(SearchRestriction {
        repo: value.to_owned(),
        path_prefix: None,
      }));
    }
    let (candidates, dir): (Vec<&RepoName>, &str) = match value.split_once(':') {
      Some((repo, dir)) => match known(repo) {
        Some(repo) => (vec![repo], dir),
        None => return Ok(None),
      },
      None => (repos.iter().collect(), value),
    };
    let dir = dir.trim_start_matches("./").trim_end_matches('/');
    if dir.is_empty() || dir.starts_with('/') {
      return Ok(None);
    }
    let prefix = format!("{dir}/");
    self.barrier()?;
    let holding = self.read(Policy::with_floor(0.0), |graph| {
      let mut holding = Vec::new();
      for repo in candidates {
        if graph.tx().files_in_repo(repo)?.iter().any(|file| file.path.as_str().starts_with(&prefix)) {
          holding.push(repo.to_string());
        }
      }
      Ok(holding)
    })?;
    Ok(match holding.as_slice() {
      [repo] => Some(SearchRestriction {
        repo: repo.clone(),
        path_prefix: Some(prefix),
      }),
      _ => None,
    })
  }
}

pub(crate) fn normalised(text: &str) -> String {
  text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

pub(crate) fn identifier_tokens(text: &str) -> BTreeSet<String> {
  let mut tokens = BTreeSet::new();
  for word in text.split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty()) {
    let mut current = String::new();
    let mut previous_lower = false;
    for c in word.chars() {
      if c.is_uppercase() && previous_lower && !current.is_empty() {
        tokens.insert(std::mem::take(&mut current));
      }
      previous_lower = c.is_lowercase() || c.is_ascii_digit();
      current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
      tokens.insert(current);
    }
  }
  tokens
}

pub(crate) fn query_terms(text: &str) -> BTreeSet<String> {
  let mut terms = identifier_tokens(text);
  terms.retain(|term| term.len() >= 3);
  terms
}

pub(crate) fn lexical_hits(index: &SimilarityIndex, text: &str, kinds: &[EntityKind], repo: Option<&str>, k: usize) -> Vec<SearchHit> {
  lexical_hits_in(index, text, kinds, repo, None, k)
}

pub(crate) fn lexical_hits_in(index: &SimilarityIndex, text: &str, kinds: &[EntityKind], repo: Option<&str>, family: Option<LanguageFamily>, k: usize) -> Vec<SearchHit> {
  let repos = repo.and_then(|repo| RepoName::new(repo).ok()).into_iter().collect();
  index.search(
    text,
    &SearchOptions {
      k,
      kinds: kinds.to_vec(),
      repos,
      families: family.into_iter().collect(),
      path_prefix: None,
    },
  )
}

fn field_name(field: Field) -> &'static str {
  match field {
    Field::Name => "name",
    Field::Signature => "signature",
    Field::Doc => "doc",
    Field::Path => "path",
    Field::References => "references",
  }
}

pub(crate) fn query_words(text: &str) -> Vec<String> {
  let mut words = Vec::new();
  for word in text.split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty()) {
    words.push(word.to_lowercase());
    let mut current = String::new();
    let mut previous_lower = false;
    for c in word.chars() {
      if c.is_uppercase() && previous_lower && !current.is_empty() {
        words.push(std::mem::take(&mut current));
      }
      previous_lower = c.is_lowercase() || c.is_ascii_digit();
      current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
      words.push(current);
    }
  }
  words.sort();
  words.dedup();
  words
}

pub(crate) fn unstemmed(terms: &[String], words: &[String]) -> Vec<String> {
  terms
    .iter()
    .map(|term| {
      words
        .iter()
        .filter(|word| word.starts_with(term.as_str()))
        .min_by_key(|word| (word.len(), word.as_str()))
        .cloned()
        .unwrap_or_else(|| term.clone())
    })
    .collect()
}

pub(crate) fn rounded(value: f64) -> f64 {
  (value * 1000.0).round() / 1000.0
}

fn coverage_boost(hit: &SearchHit, significant: usize) -> f64 {
  if significant == 0 {
    return 1.0;
  }
  let covered = hit.direct_terms.len().min(significant) as f64 / significant as f64;
  (1.0 + COVERAGE_WEIGHT * covered) / (1.0 + COVERAGE_WEIGHT)
}

struct QueryShape {
  phrase: String,
  tokens: BTreeSet<String>,
  significant: BTreeSet<String>,
  ranking: BTreeSet<String>,
}

impl QueryShape {
  fn of(text: &str) -> Self {
    let tokens = identifier_tokens(text);
    let significant: BTreeSet<String> = tokens.iter().filter(|token| !STOPWORDS.contains(&token.as_str())).cloned().collect();
    let ranking = significant.iter().filter(|token| token.len() >= 3).cloned().collect();
    Self {
      phrase: normalised(text),
      tokens,
      significant,
      ranking,
    }
  }

  fn is_exact_name(&self, name: &str, qualified: &str) -> bool {
    if !self.phrase.is_empty() && (normalised(name) == self.phrase || normalised(qualified) == self.phrase) {
      return true;
    }
    let tokens = identifier_tokens(name);
    !tokens.is_empty() && (tokens == self.tokens || tokens == self.significant)
  }

  fn name_coverage(&self, name: &str) -> f64 {
    if self.ranking.is_empty() {
      return 0.0;
    }
    let covered = identifier_tokens(name).intersection(&self.ranking).count();
    covered as f64 / self.ranking.len() as f64
  }
}

fn search(graph: &Graph<'_, '_>, index: &SimilarityIndex, request: &SearchRequest) -> QueryResult<SearchAnswer> {
  let hits = index.search(
    &request.text,
    &SearchOptions {
      k: request.limit.max(1) * 3,
      kinds: request.kinds.clone(),
      repos: request.repo.as_deref().and_then(|repo| RepoName::new(repo).ok()).into_iter().collect(),
      families: Vec::new(),
      path_prefix: request.path_prefix.clone(),
    },
  );
  let words = query_words(&request.text);
  let shape = QueryShape::of(&request.text);
  let significant: BTreeSet<&str> = hits.iter().flat_map(|hit| hit.direct_terms.iter().map(String::as_str)).collect();
  let boosted: Vec<f64> = hits
    .iter()
    .map(|hit| hit.score * coverage_boost(hit, significant.len()) * (1.0 + NAME_COVERAGE_WEIGHT * shape.name_coverage(&hit.name)))
    .collect();
  let mut order: Vec<usize> = (0..hits.len()).collect();
  order.sort_by(|left, right| boosted[*right].total_cmp(&boosted[*left]).then(left.cmp(right)));
  let top = order.first().map_or(0.0, |index| boosted[*index]);
  let mut exact_name = Vec::new();
  let mut strong = Vec::new();
  let mut related = Vec::new();
  let mut tests = Vec::new();
  let mut seeds = Vec::new();
  for index_of in order {
    let hit = &hits[index_of];
    let score = boosted[index_of];
    let Some(entity) = graph.entity(hit.entity)? else { continue };
    let view = SearchHitView {
      entity: EntityRef::of(&entity),
      signature: entity.signature.clone(),
      doc: first_line(entity.doc.as_deref()),
      score: rounded(score),
      matched_fields: hit
        .matched_fields
        .iter()
        .map(|field| format!("{}: {}", field_name(field.field), unstemmed(&field.terms, &words).join(" ")))
        .collect(),
      direct_terms: unstemmed(&hit.direct_terms, &words),
      expansion_terms: hit
        .via_expansion
        .iter()
        .map(|expansion| format!("{} (from {})", expansion.term, unstemmed(&expansion.from, &words).join(", ")))
        .collect(),
    };
    if is_test(&entity) {
      tests.push(view);
    } else if shape.is_exact_name(&hit.name, &view.entity.short) {
      if seeds.len() < STRUCTURAL_SEEDS {
        seeds.push((entity.id, view.entity.short.clone()));
      }
      exact_name.push(view);
    } else if score >= STRONG_FRACTION * top {
      strong.push(view);
    } else {
      related.push(view);
    }
  }
  related.extend(tests);
  let total_hits = exact_name.len() + strong.len() + related.len();
  let mut remaining = request.limit;
  for group in [&mut exact_name, &mut strong, &mut related] {
    group.truncate(remaining);
    remaining -= group.len();
  }
  let mut structurally_similar = Vec::new();
  let shown: BTreeSet<String> = exact_name.iter().chain(&strong).chain(&related).map(|hit| hit.entity.id.clone()).collect();
  let named: BTreeSet<EntityId> = hits
    .iter()
    .filter(|hit| hit.matched_fields.iter().any(|field| field.field == Field::Name))
    .map(|hit| hit.entity)
    .collect();
  for (seed, name) in &seeds {
    let mut kept = 0;
    for found in index.near_duplicates(*seed, STRUCTURAL_THRESHOLD, STRUCTURAL_PER_SEED * STRUCTURAL_CANDIDATE_FACTOR) {
      if kept == STRUCTURAL_PER_SEED {
        break;
      }
      if found.small && !named.contains(&found.entity) {
        continue;
      }
      let Some(entity) = graph.entity(found.entity)? else { continue };
      if shown.contains(&entity.id.to_string()) || !request.admits(&entity) {
        continue;
      }
      structurally_similar.push(StructuralHit {
        entity: EntityRef::of(&entity),
        similar_to: name.clone(),
        jaccard: rounded(found.exact_jaccard),
        small_body: found.small,
      });
      kept += 1;
    }
  }
  let (inferred_capabilities, hidden) = capability_matches(graph, request)?;
  let mut caveats = Vec::new();
  if let Some(hidden) = hidden {
    caveats.push(hidden);
  }
  if total_hits > exact_name.len() + strong.len() + related.len() {
    caveats.push(Caveat::Note {
      message: format!("{} more lexical hits omitted (raise limit)", total_hits - exact_name.len() - strong.len() - related.len()),
    });
  }
  Ok(SearchAnswer {
    query: request.text.clone(),
    scope: request
      .path_prefix
      .as_ref()
      .map(|prefix| format!("{}:{prefix}", request.repo.as_deref().unwrap_or_default())),
    total_hits,
    exact_name,
    strong,
    related,
    structurally_similar,
    inferred_capabilities,
    caveats,
  })
}

fn capability_matches(graph: &Graph<'_, '_>, request: &SearchRequest) -> QueryResult<(Vec<InferredHit>, Option<Caveat>)> {
  let terms = query_terms(&request.text);
  let facts = graph.tx().inferred_facts(&InferredQuery {
    subject: None,
    relation: Some(RELATION_CAPABILITY_SUMMARY.to_owned()),
    freshness: FreshnessPolicy::IncludeStale,
  })?;
  let mut hits = Vec::new();
  let (mut stale, mut low) = (0, 0);
  for fact in facts {
    let summary = fact.payload.get("summary").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
    let capabilities: Vec<String> = fact
      .payload
      .get("capabilities")
      .and_then(serde_json::Value::as_array)
      .map(|values| values.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect())
      .unwrap_or_default();
    let described = query_terms(&format!("{summary} {}", capabilities.join(" ")));
    let matched: Vec<String> = terms.intersection(&described).cloned().collect();
    if matched.is_empty() {
      continue;
    }
    let Some(subject) = fact.subjects.first() else { continue };
    let entity = graph.entity(*subject)?;
    if request.path_prefix.is_some() && !entity.as_ref().is_some_and(|entity| request.admits(entity)) {
      continue;
    }
    match graph.policy.inferred_admission(&fact) {
      InferredAdmission::StaleHidden => {
        stale += 1;
        continue;
      }
      InferredAdmission::LowConfidence => {
        low += 1;
        continue;
      }
      InferredAdmission::Current | InferredAdmission::StaleShown => {}
    }
    let Some(entity) = entity else { continue };
    hits.push(InferredHit {
      entity: EntityRef::of(&entity),
      summary,
      matched_terms: matched,
      certainty: CertaintyTag::inferred(&fact.certainty, &fact.freshness),
    });
  }
  hits.sort_by(|left, right| {
    right
      .matched_terms
      .len()
      .cmp(&left.matched_terms.len())
      .then_with(|| left.entity.sort_key().cmp(&right.entity.sort_key()))
  });
  let hidden = (stale + low > 0).then_some(Caveat::InferredHidden { stale, low_confidence: low });
  Ok((hits, hidden))
}
