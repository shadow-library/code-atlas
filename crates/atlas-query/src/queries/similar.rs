use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{EdgeKind, Entity, EntityId, EntityKey, EntityKind, LanguageFamily, RelPath};
use atlas_semantic::{RELATION_EQUIVALENT_TO, RELATION_SIMILAR_TO};
use atlas_similarity::{SMALL_BODY_TOKENS, SMALL_PAIR_MIN_JACCARD, SimilarityIndex};
use atlas_store::{EntityFilter, FreshnessPolicy, InferredQuery};
use serde::Serialize;

use crate::QueryEngine;
use crate::error::{QueryError, QueryResult};
use crate::graph::Graph;
use crate::model::{Caveat, CertaintyTag, EntityRef, Response, Targeted};
use crate::policy::{Direction, InferredAdmission, Policy};
use crate::queries::common::{is_test, language, package_key, resolve_one};
use crate::queries::reuse::{CandidateQuery, lexical_and_signature_candidates, signature_shape};
use crate::queries::search::rounded;
use crate::queries::tests_for::direct_tests;
use crate::target::TargetFilter;

const DEFAULT_THRESHOLD: f64 = 0.5;
const IDENTICAL_JACCARD: f64 = 0.95;
const STRUCTURAL_CANDIDATE_FACTOR: usize = 4;
const SHAPE_ONLY_LIMIT: usize = 5;
const SHAPE_ONLY_NOTE: &str = "shape-only (small body, likely boilerplate)";
const SHARED_SEGMENTS: [&str; 6] = ["shared", "common", "util", "utils", "lib", "core"];

#[derive(Debug, Clone, PartialEq)]
pub struct SimilarRequest {
  pub target: String,
  pub filter: TargetFilter,
  pub limit: usize,
  pub threshold: f64,
  pub include_stale: bool,
}

impl SimilarRequest {
  pub fn new(target: impl Into<String>) -> Self {
    Self {
      target: target.into(),
      filter: TargetFilter::default(),
      limit: 10,
      threshold: DEFAULT_THRESHOLD,
      include_stale: false,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum SimilaritySignal {
  Structural {
    jaccard: f64,
  },
  SmallStructural {
    jaccard: f64,
  },
  Lexical {
    score: f64,
    terms: Vec<String>,
  },
  SignatureShape {
    shape: String,
  },
  Inferred {
    verdict: String,
    rationale: String,
    differences: Vec<String>,
    certainty: CertaintyTag,
  },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConsumerSummary {
  pub callers: usize,
  pub uncertain: usize,
  pub repos: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClusterMember {
  pub entity: EntityRef,
  pub language: String,
  pub signals: Vec<SimilaritySignal>,
  pub verdict: String,
  pub consumers: ConsumerSummary,
  pub tests: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SharedHome {
  pub language: String,
  pub package: Option<String>,
  pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShapeOnlyMatch {
  pub entity: EntityRef,
  pub jaccard: f64,
  pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimilarAnswer {
  pub target: ClusterMember,
  pub members: Vec<ClusterMember>,
  pub shape_only: Vec<ShapeOnlyMatch>,
  pub shared_home: Vec<SharedHome>,
  pub coupling_risks: Vec<String>,
  pub caveats: Vec<Caveat>,
}

impl QueryEngine {
  pub fn similar(&self, request: &SimilarRequest) -> QueryResult<Response<Targeted<SimilarAnswer>>> {
    let freshness = self.barrier()?;
    let index = self.engine().similarity();
    let policy = Policy {
      min_confidence: 0.0,
      include_stale: request.include_stale,
    };
    let answer = self.read(policy, |graph| -> QueryResult<Targeted<SimilarAnswer>> {
      let target = match snippet_target(self, graph, &request.target)? {
        Some(target) => target,
        None => match resolve_one(self, graph, &request.target, &request.filter)? {
          Ok(target) => target,
          Err(outcome) => return Ok(outcome),
        },
      };
      similar(graph, &index, &target, request).map(Targeted::Found)
    })?;
    Ok(Response { freshness, answer })
  }
}

fn snippet_target(query: &QueryEngine, graph: &Graph<'_, '_>, spec: &str) -> QueryResult<Option<Entity>> {
  let Some((location, range)) = spec.rsplit_once(':') else { return Ok(None) };
  let Some((start, end)) = range.split_once('-') else { return Ok(None) };
  let (Ok(start), Ok(end)) = (start.parse::<u32>(), end.parse::<u32>()) else {
    return Ok(None);
  };
  let (repo, path) = match location.split_once(':') {
    Some((repo, path)) => (Some(repo), path),
    None => (None, location),
  };
  let path = RelPath::new(path).map_err(|_| QueryError::invalid(format!("invalid snippet path {path:?}")))?;
  let mut best: Option<Entity> = None;
  for candidate in query.repos().iter().filter(|candidate| repo.is_none_or(|repo| candidate.as_str() == repo)) {
    for entity in graph.tx().entities_in_file(candidate, &path, &EntityFilter::default())? {
      let Some(span) = entity.span.filter(|span| entity.kind.is_code_symbol() && span.start_line <= start && end <= span.end_line) else {
        continue;
      };
      let size = span.end_byte - span.start_byte;
      if best.as_ref().and_then(|best| best.span).is_none_or(|current| size < current.end_byte - current.start_byte) {
        best = Some(entity);
      }
    }
  }
  best
    .map(Some)
    .ok_or_else(|| QueryError::invalid(format!("no indexed symbol encloses {spec}; snippets must lie inside a function or type")))
}

fn callable(kind: &EntityKind) -> bool {
  matches!(kind, EntityKind::Function | EntityKind::Method)
}

fn comparable(target: &Entity, candidate: &Entity) -> bool {
  candidate.id != target.id
    && !is_test(candidate)
    && (callable(&target.kind) == callable(&candidate.kind))
    && candidate.kind.is_code_symbol()
    && candidate.kind != EntityKind::Field
}

fn structural_reason(status: &atlas_similarity::StructuralStatus) -> String {
  match status {
    atlas_similarity::StructuralStatus::Unknown => "the target is not in the structural index".to_owned(),
    atlas_similarity::StructuralStatus::NoBody => "the target has no body".to_owned(),
    atlas_similarity::StructuralStatus::TooSmall { tokens } => format!("too small for structural comparison ({tokens} tokens)"),
    atlas_similarity::StructuralStatus::LowEntropy { tokens } => format!("too repetitive for structural comparison ({tokens} tokens, few distinct tokens)"),
    atlas_similarity::StructuralStatus::Indexed { tokens } => format!("indexed ({tokens} tokens)"),
  }
}

fn similar(graph: &Graph<'_, '_>, index: &SimilarityIndex, target: &Entity, request: &SimilarRequest) -> QueryResult<SimilarAnswer> {
  let mut signals: BTreeMap<EntityId, Vec<SimilaritySignal>> = BTreeMap::new();
  let mut caveats = Vec::new();
  let mut small_found: BTreeMap<EntityId, f64> = BTreeMap::new();
  for found in index.near_duplicates(target.id, request.threshold, request.limit * STRUCTURAL_CANDIDATE_FACTOR) {
    if found.small {
      small_found.insert(found.entity, rounded(found.exact_jaccard));
      continue;
    }
    signals.entry(found.entity).or_default().push(SimilaritySignal::Structural {
      jaccard: rounded(found.exact_jaccard),
    });
  }
  let structural = index.structural_status(target.id);
  if let atlas_similarity::StructuralStatus::Indexed { tokens } = structural
    && tokens < SMALL_BODY_TOKENS
    && request.threshold < SMALL_PAIR_MIN_JACCARD
  {
    caveats.push(Caveat::Note {
      message: format!(
        "the target is a small body ({tokens} tokens): small-body structural matches are held to a minimum score of {SMALL_PAIR_MIN_JACCARD:.2}, overriding the requested threshold {:.2}",
        request.threshold
      ),
    });
  }
  if !matches!(structural, atlas_similarity::StructuralStatus::Indexed { .. }) {
    caveats.push(Caveat::Note {
      message: format!(
        "no structural fingerprint for the target: {}; only lexical and inferred signals apply",
        structural_reason(&structural)
      ),
    });
  }
  let text = format!("{} {}", target.name, target.signature.clone().unwrap_or_default());
  let target_shape = signature_shape(target);
  let query = CandidateQuery {
    text: &text,
    shape: target_shape.as_deref(),
    exclude: Some(target.id),
    fetch: request.limit * 3,
  };
  for candidate in lexical_and_signature_candidates(graph, index, &query)? {
    let found = signals.entry(candidate.entity.id).or_default();
    found.push(SimilaritySignal::Lexical {
      score: rounded(candidate.score),
      terms: candidate.terms,
    });
    if let Some(shape) = target_shape.clone().filter(|_| candidate.same_shape) {
      found.push(SimilaritySignal::SignatureShape { shape });
    }
  }
  for (id, found) in signals.iter_mut() {
    add_signature_shape(graph, target_shape.as_ref(), *id, found)?;
  }
  let (hidden_stale, hidden_low) = inferred_signals(graph, target, &mut signals)?;
  let mut shape_only = Vec::new();
  for (id, jaccard) in small_found {
    let corroborated = signals.get(&id).is_some_and(|found| {
      found
        .iter()
        .any(|signal| matches!(signal, SimilaritySignal::Lexical { .. } | SimilaritySignal::Inferred { .. }))
    });
    if !corroborated {
      let Some(entity) = graph.entity(id)? else { continue };
      if comparable(target, &entity) {
        shape_only.push(ShapeOnlyMatch {
          entity: EntityRef::of(&entity),
          jaccard,
          note: SHAPE_ONLY_NOTE.to_owned(),
        });
      }
      continue;
    }
    let found = signals.entry(id).or_default();
    found.push(SimilaritySignal::SmallStructural { jaccard });
    add_signature_shape(graph, target_shape.as_ref(), id, found)?;
  }
  shape_only.sort_by(|left, right| right.jaccard.total_cmp(&left.jaccard).then_with(|| left.entity.id.cmp(&right.entity.id)));
  shape_only.truncate(SHAPE_ONLY_LIMIT);
  if hidden_stale + hidden_low > 0 {
    caveats.push(Caveat::InferredHidden {
      stale: hidden_stale,
      low_confidence: hidden_low,
    });
  }
  let mut members = Vec::new();
  for (id, signals) in signals {
    let Some(entity) = graph.entity(id)? else { continue };
    if !comparable(target, &entity) && !signals.iter().any(|signal| matches!(signal, SimilaritySignal::Inferred { .. })) {
      continue;
    }
    members.push((entity, signals));
  }
  members.sort_by(|(left, left_signals), (right, right_signals)| strength(right_signals).total_cmp(&strength(left_signals)).then_with(|| left.key.cmp(&right.key)));
  members.truncate(request.limit);
  let target_member = member(graph, target, Vec::new())?;
  let mut views = Vec::new();
  for (entity, signals) in &members {
    views.push(member(graph, entity, signals.clone())?);
  }
  let mut cluster: Vec<&Entity> = vec![target];
  cluster.extend(
    members
      .iter()
      .filter(|(entity, signals)| consolidation_candidate(target, entity, signals))
      .map(|(entity, _)| entity),
  );
  let (shared_home, coupling_risks) = consolidation(graph, &cluster)?;
  caveats.push(Caveat::Note {
    message: "similarity is evidence, not a decision: only an inferred equivalence verdict speaks to behaviour".to_owned(),
  });
  Ok(SimilarAnswer {
    target: target_member,
    members: views,
    shape_only,
    shared_home,
    coupling_risks,
    caveats,
  })
}

fn add_signature_shape(graph: &Graph<'_, '_>, target_shape: Option<&String>, id: EntityId, found: &mut Vec<SimilaritySignal>) -> QueryResult<()> {
  if found.iter().any(|signal| matches!(signal, SimilaritySignal::SignatureShape { .. })) {
    return Ok(());
  }
  let Some(entity) = graph.entity(id)? else { return Ok(()) };
  if let Some(shape) = target_shape.filter(|shape| signature_shape(&entity).as_ref() == Some(*shape)) {
    found.push(SimilaritySignal::SignatureShape { shape: shape.clone() });
  }
  Ok(())
}

fn consolidation_candidate(target: &Entity, entity: &Entity, signals: &[SimilaritySignal]) -> bool {
  let strong = signals.iter().any(|signal| {
    matches!(
      signal,
      SimilaritySignal::Structural { .. } | SimilaritySignal::SmallStructural { .. } | SimilaritySignal::SignatureShape { .. }
    ) || matches!(signal, SimilaritySignal::Inferred { certainty, .. } if certainty.freshness.is_none())
  });
  strong || language(entity) != language(target)
}

fn strength(signals: &[SimilaritySignal]) -> f64 {
  signals
    .iter()
    .map(|signal| match signal {
      SimilaritySignal::Inferred { certainty, verdict, .. } => 3.0 + f64::from(certainty.confidence.unwrap_or(0.0)) - if verdict == "equivalent" { 0.0 } else { 0.5 },
      SimilaritySignal::Structural { jaccard } => 2.0 + jaccard,
      SimilaritySignal::SmallStructural { jaccard } => 1.0 + jaccard,
      SimilaritySignal::Lexical { score, .. } => score.min(10.0) / 10.0,
      SimilaritySignal::SignatureShape { .. } => 1.0,
    })
    .sum()
}

fn inferred_signals(graph: &Graph<'_, '_>, target: &Entity, signals: &mut BTreeMap<EntityId, Vec<SimilaritySignal>>) -> QueryResult<(usize, usize)> {
  let (mut stale, mut low) = (0, 0);
  for relation in [RELATION_EQUIVALENT_TO, RELATION_SIMILAR_TO] {
    let facts = graph.tx().inferred_facts(&InferredQuery {
      subject: Some(target.id),
      relation: Some(relation.to_owned()),
      freshness: FreshnessPolicy::IncludeStale,
    })?;
    for fact in facts {
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
      let Some(other) = fact.subjects.iter().copied().find(|subject| *subject != target.id) else {
        continue;
      };
      let text = |name: &str| fact.payload.get(name).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
      let differences = fact
        .payload
        .get("behavioural_differences")
        .and_then(serde_json::Value::as_array)
        .map(|values| values.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
      signals.entry(other).or_default().push(SimilaritySignal::Inferred {
        verdict: text("verdict"),
        rationale: text("rationale"),
        differences,
        certainty: CertaintyTag::inferred(&fact.certainty, &fact.freshness),
      });
    }
  }
  Ok((stale, low))
}

fn verdict(signals: &[SimilaritySignal]) -> String {
  for signal in signals {
    if let SimilaritySignal::Inferred {
      verdict,
      rationale,
      differences,
      certainty,
    } = signal
    {
      let confidence = certainty.confidence.map(|value| format!("{value:.2}")).unwrap_or_default();
      let stale = certainty.freshness.as_ref().map(|state| format!(" [{state}]")).unwrap_or_default();
      return match verdict.as_str() {
        "equivalent" => format!("inferred equivalent ({confidence}){stale}: {rationale}"),
        _ => format!("inferred similar, not equivalent ({confidence}){stale}: {}", differences.join("; ")),
      };
    }
  }
  let jaccard = signals.iter().find_map(|signal| match signal {
    SimilaritySignal::Structural { jaccard } => Some(*jaccard),
    _ => None,
  });
  let same_signature = signals.iter().any(|signal| matches!(signal, SimilaritySignal::SignatureShape { .. }));
  if jaccard.is_none() && signals.iter().any(|signal| matches!(signal, SimilaritySignal::SmallStructural { .. })) {
    return "small bodies with a related name and similar token structure, behaviour unverified".to_owned();
  }
  match (jaccard, same_signature) {
    (Some(jaccard), _) if jaccard >= IDENTICAL_JACCARD => "structurally identical, behaviour unverified".to_owned(),
    (Some(_), _) => "same shape, behaviour unverified".to_owned(),
    (None, true) => "same signature shape and related name, behaviour unverified".to_owned(),
    (None, false) => "related by name only, behaviour unverified".to_owned(),
  }
}

fn language_label(entity: &Entity) -> String {
  match language(entity) {
    Some(LanguageFamily::Rust) => "rust".to_owned(),
    Some(_) => "ts".to_owned(),
    None => "unknown".to_owned(),
  }
}

fn member(graph: &Graph<'_, '_>, entity: &Entity, signals: Vec<SimilaritySignal>) -> QueryResult<ClusterMember> {
  let mut hidden = 0;
  let mut repos = BTreeSet::new();
  let (mut callers, mut uncertain) = (0, 0);
  for edge in graph.admitted(entity.id, Direction::Reverse, &[EdgeKind::Calls, EdgeKind::Instantiates, EdgeKind::References], &mut hidden)? {
    let Some(caller) = graph.entity(edge.from)? else { continue };
    if is_test(&caller) {
      continue;
    }
    callers += 1;
    uncertain += usize::from(!edge.certainty.is_authoritative());
    repos.extend(caller.repo.as_ref().map(ToString::to_string));
  }
  Ok(ClusterMember {
    entity: EntityRef::of(entity),
    language: language_label(entity),
    verdict: verdict(&signals),
    signals,
    consumers: ConsumerSummary {
      callers,
      uncertain,
      repos: repos.into_iter().collect(),
    },
    tests: direct_tests(graph, entity)?.len(),
  })
}

fn package_dependencies(graph: &Graph<'_, '_>, package: EntityId) -> QueryResult<BTreeSet<EntityId>> {
  Ok(graph.edges_out(package, &[EdgeKind::DependsOnPackage])?.into_iter().map(|edge| edge.to).collect())
}

fn consolidation(graph: &Graph<'_, '_>, cluster: &[&Entity]) -> QueryResult<(Vec<SharedHome>, Vec<String>)> {
  let mut by_language: BTreeMap<String, Vec<&Entity>> = BTreeMap::new();
  for entity in cluster {
    by_language.entry(language_label(entity)).or_default().push(entity);
  }
  let mut homes = Vec::new();
  let mut risks = Vec::new();
  if by_language.len() > 1 {
    let languages: Vec<&str> = by_language.keys().map(String::as_str).collect();
    risks.push(format!(
      "members span languages ({}); they cannot share one implementation without a service boundary or code generation",
      languages.join(", ")
    ));
  }
  for (language, members) in &by_language {
    if members.len() < 2 {
      continue;
    }
    let mut consumer_packages: BTreeSet<EntityId> = BTreeSet::new();
    let mut member_packages: BTreeMap<EntityId, String> = BTreeMap::new();
    for member in members {
      if let Some(package) = package_key(member) {
        member_packages.insert(EntityId::of(&package), package_name(&package));
      }
      for edge in graph.edges_in(member.id, &[EdgeKind::Calls, EdgeKind::Instantiates, EdgeKind::References])? {
        let Some(caller) = graph.entity(edge.from)? else { continue };
        if let Some(package) = package_key(&caller) {
          consumer_packages.insert(EntityId::of(&package));
        }
      }
    }
    let mut best: Option<(usize, EntityId, String)> = None;
    for (package, name) in &member_packages {
      let mut reaches = 0;
      for consumer in consumer_packages.iter().chain(member_packages.keys()) {
        if consumer == package || package_dependencies(graph, *consumer)?.contains(package) {
          reaches += 1;
        }
      }
      let shared = members
        .iter()
        .filter(|member| package_key(member).is_some_and(|key| EntityId::of(&key) == *package))
        .any(|member| member.path.as_ref().is_some_and(|path| path.segments().any(|segment| SHARED_SEGMENTS.contains(&segment))));
      let rank = reaches * 2 + usize::from(shared);
      if best
        .as_ref()
        .is_none_or(|(current, _, current_name)| rank > *current || (rank == *current && name < current_name))
      {
        best = Some((rank, *package, name.clone()));
      }
    }
    let Some((_, home, name)) = best else { continue };
    let total = consumer_packages.union(&member_packages.keys().copied().collect()).count();
    let mut unreachable = Vec::new();
    for consumer in consumer_packages.iter().chain(member_packages.keys()).collect::<BTreeSet<_>>() {
      if *consumer != home && !package_dependencies(graph, *consumer)?.contains(&home) {
        let label = graph.entity(*consumer)?.map_or_else(|| consumer.to_string(), |entity| entity.name);
        unreachable.push(label);
      }
    }
    let reason = match unreachable.is_empty() {
      true => format!("every consumer and member package ({total}) already depends on {name} or lives in it"),
      false => format!("{name} already hosts a member and is reachable from {} of {total} packages", total - unreachable.len()),
    };
    for package in &unreachable {
      risks.push(format!("consolidating into {name} adds a dependency {package} -> {name} that does not exist yet"));
    }
    homes.push(SharedHome {
      language: language.clone(),
      package: Some(name),
      reason,
    });
  }
  Ok((homes, risks))
}

fn package_name(key: &EntityKey) -> String {
  key.display_name()
}
