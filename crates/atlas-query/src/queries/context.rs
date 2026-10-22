use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{Edge, EdgeKind, Entity, EntityId, EntityKey, EntityKind, HttpMethod, normalize_route};
use atlas_similarity::{SimilarityIndex, analyze_terms, names_related};
use serde::Serialize;

use crate::QueryEngine;
use crate::engine::{Snippet, SnippetMode, SourceReader};
use crate::error::{QueryError, QueryResult};
use crate::graph::{Graph, certainty_value};
use crate::model::{Caveat, CertaintyTag, EntityRef, Hop, Listing, Response, short_name};
use crate::policy::{Policy, hedge};
use crate::queries::common::{file_entity, first_line, is_test};
use crate::queries::reuse::access;
use crate::queries::search::{lexical_hits, query_words, unstemmed};
use crate::queries::tests_for::{TestItem, direct_tests, is_mock_edge, run_hint};
use crate::target::{Resolution, TargetFilter, resolve};
use crate::text::context::{IMPACT, MUST_READ, SKIM, TESTS, UNCERTAIN, excerpt_lines, must_read_item, omitted_section, section_title, skim_item};
use crate::text::{RenderText, rendered_chars};

mod serving;

const DEFAULT_BUDGET: usize = 6_000;
const DEFAULT_FLOOR: f32 = 0.3;
const SEARCH_SEEDS: usize = 8;
const SEARCH_FRACTION: f64 = 0.35;
const SEARCH_WEIGHT: f64 = 0.5;
const OFF_ANCHOR_FACTOR: f64 = 0.5;
const SCOPE_BOOST: f64 = 1.5;
const SCOPE_PENALTY: f64 = 0.6;
const SCOPE_SEED_PENALTY: f64 = 0.3;
const SCOPE_ENTRY_WEIGHT: f64 = 0.6;
const TEST_SUBJECT_FACTOR: f64 = 0.6;
pub const DEFAULT_CONTEXT_DEPTH: u32 = 3;
pub const MAX_CONTEXT_DEPTH: u32 = 3;
const KNOWN_FILE_WEIGHT: f64 = 1.0;
const EXCERPT_LINES: u32 = 5;
const SITE_EDGES: [EdgeKind; 8] = [
  EdgeKind::Calls,
  EdgeKind::Instantiates,
  EdgeKind::References,
  EdgeKind::ReadsEnv,
  EdgeKind::ReadsConfig,
  EdgeKind::CallsEndpoint,
  EdgeKind::MakesRequest,
  EdgeKind::UsesAuth,
];
const EXPAND_THRESHOLD: f64 = 0.25;
const CALLEE_FACTOR: f64 = 0.5;
const MEMBERS_PER_TYPE: usize = 8;
const SKELETON_MEMBERS: usize = 15;
const PAGERANK_ITERATIONS: usize = 30;
const DAMPING: f64 = 0.85;
const MUST_READ_SHARE: f64 = 0.8;
const MUST_READ_RELEVANCE: f64 = 0.2;
const BODY_LINES: usize = 80;
const SMALL_BODY_LINES: u32 = 15;
const LARGE_TYPE_LINES: u32 = 40;
const LOW_RELEVANCE: f64 = 0.04;
const MAX_CODE_ITEMS: usize = 40;
const SIMILAR_THRESHOLD: f64 = 0.6;
const SIMILAR_PER_NODE: usize = 3;
const SIMILAR_CANDIDATES: usize = 12;
const AMBIGUOUS_SEED_LIMIT: usize = 4;
const OMITTED_CANDIDATES_SHOWN: usize = 8;
const CANDIDATE_RANK_HITS: usize = 500;
const TEST_CANDIDATES: usize = 12;
const TESTS_SHOWN: usize = 8;
const IMPACT_SHOWN: usize = 15;
const UNCERTAIN_SHOWN: usize = 10;
const PATTERN_ANCHORS: usize = 3;
const PATTERN_SIBLINGS: usize = 2;
const REGISTRY_MIN_SIBLINGS: usize = 2;
const HUB_FAN_IN: usize = 4;
const HUB_USERS_COUNTED: usize = 256;
const USERS_EXAMINED: usize = 1_024;
const BEHAVIOUR_CALLERS: usize = 4;
const BEHAVIOUR_HITS: usize = 2;
const SIBLING_ROUTE_WEIGHT: f64 = 0.5;
const SIBLING_ROUTES: usize = 5;
const PLATFORM_TYPES: [&str; 20] = [
  "Response", "Request", "Headers", "Error", "Date", "Map", "Set", "Promise", "URL", "Record", "Array", "Object", "JSON", "String", "Vec", "Option", "Result", "Box", "Arc",
  "HashMap",
];
const AUTH_WORDS: [(&str, &str); 7] = [
  ("api-key", "api_key"),
  ("apikey", "api_key"),
  ("api_key", "api_key"),
  ("bearer", "bearer"),
  ("jwt", "bearer"),
  ("session", "session_cookie"),
  ("cookie", "session_cookie"),
];
const GENERIC_WORDS: [&str; 52] = [
  "a",
  "an",
  "the",
  "to",
  "of",
  "in",
  "on",
  "for",
  "with",
  "and",
  "or",
  "from",
  "by",
  "into",
  "that",
  "this",
  "it",
  "is",
  "be",
  "so",
  "add",
  "adding",
  "new",
  "create",
  "update",
  "fix",
  "change",
  "rename",
  "implement",
  "support",
  "handle",
  "use",
  "make",
  "remove",
  "delete",
  "refactor",
  "move",
  "extract",
  "report",
  "number",
  "code",
  "function",
  "method",
  "class",
  "file",
  "files",
  "variant",
  "instead",
  "should",
  "when",
  "all",
  "every",
];

#[derive(Debug, Clone, PartialEq)]
pub struct ContextRequest {
  pub task: String,
  pub budget: usize,
  pub seeds: Vec<String>,
  pub known: Vec<String>,
  pub depth: u32,
  pub explain: bool,
  pub min_confidence: Option<f32>,
}

impl ContextRequest {
  pub fn new(task: impl Into<String>) -> Self {
    Self {
      task: task.into(),
      budget: DEFAULT_BUDGET,
      seeds: Vec::new(),
      known: Vec::new(),
      depth: DEFAULT_CONTEXT_DEPTH,
      explain: false,
      min_confidence: None,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RangeBasis {
  Symbol,
  Site,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LineRange {
  pub start: u32,
  pub end: u32,
  pub basis: RangeBasis,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContextItem {
  pub entity: EntityRef,
  pub score: f64,
  pub why: String,
  pub certainty: Option<CertaintyTag>,
  pub via: Option<Hop>,
  pub uncertain: bool,
  pub signature: Option<String>,
  pub doc: Option<String>,
  pub skeleton: Vec<String>,
  pub lines: Option<LineRange>,
  pub excerpt: Option<Snippet>,
  pub body: Option<Snippet>,
  pub read_directly: bool,
  pub run: Option<String>,
  #[serde(skip)]
  pub named: bool,
  #[serde(skip)]
  pub search_terms: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OmissionReason {
  Held,
  Budget,
  ShownInEnclosingItem,
  ShownInOutline,
  LowRelevance,
  BelowConfidenceFloor,
  AmbiguousSeed,
  UnknownSeed,
  AmbiguousKnown,
  UnknownKnown,
}

impl OmissionReason {
  pub(crate) fn is_seed_diagnostic(self) -> bool {
    matches!(self, Self::AmbiguousSeed | Self::UnknownSeed | Self::AmbiguousKnown | Self::UnknownKnown)
  }

  pub(crate) fn is_listed_when_collapsed(self) -> bool {
    self.is_seed_diagnostic() || self == Self::Held
  }

  pub fn as_str(self) -> &'static str {
    match self {
      Self::Held => "already held by the caller",
      Self::Budget => "budget",
      Self::ShownInEnclosingItem => "shown inside an enclosing MUST READ body",
      Self::ShownInOutline => "shown in the outline of an enclosing MUST READ item",
      Self::LowRelevance => "low relevance",
      Self::BelowConfidenceFloor => "below confidence floor",
      Self::AmbiguousSeed => "ambiguous seed",
      Self::UnknownSeed => "unknown seed",
      Self::AmbiguousKnown => "ambiguous known",
      Self::UnknownKnown => "unknown known",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OmittedItem {
  pub entity: Option<EntityRef>,
  pub label: String,
  pub reason: OmissionReason,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SectionSummary {
  pub section: String,
  pub omitted: usize,
  pub uncertain: usize,
  pub by_repo: BTreeMap<String, usize>,
  pub hint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScoredKey {
  pub key: String,
  pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContextExplain {
  pub seeds: Vec<String>,
  pub expansions: Vec<String>,
  pub scores: Vec<ScoredKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContextAnswer {
  pub task: String,
  pub budget: usize,
  pub estimated_tokens: usize,
  pub must_read: Vec<ContextItem>,
  pub skim: Vec<ContextItem>,
  pub patterns: Vec<ContextItem>,
  pub reuse: Vec<ContextItem>,
  pub impact: Vec<ContextItem>,
  pub tests: Vec<ContextItem>,
  pub uncertain: Vec<ContextItem>,
  pub summaries: Vec<SectionSummary>,
  pub omitted: Listing<OmittedItem>,
  pub omitted_by_reason: BTreeMap<OmissionReason, usize>,
  pub explain: Option<ContextExplain>,
  pub caveats: Vec<Caveat>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Role {
  Code,
  Pattern,
  Impact,
  Reuse,
  Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Basis {
  SearchHit,
  Relation,
}

enum Placement {
  Merged,
  KeepExisting,
  TakeIncoming,
}

#[derive(Clone)]
struct Node {
  entity: Entity,
  basis: Basis,
  role: Role,
  weight: f64,
  uncertain: bool,
  why: String,
  via: Option<Hop>,
  site: Option<(u32, u32)>,
  terms: Vec<String>,
  round: u32,
  named: bool,
  seed: bool,
  near_named: bool,
  reuse: Option<String>,
  hub_factor: f64,
}

struct Held {
  spec: String,
  entity: Entity,
}

impl Held {
  fn is_file(&self) -> bool {
    self.entity.kind == EntityKind::File
  }

  fn covers(&self, entity: &Entity) -> bool {
    if self.entity.id == entity.id {
      return true;
    }
    let held_file = match self.is_file() {
      true => Some(self.entity.key.clone()),
      false => file_entity(&self.entity),
    };
    if held_file.is_none() || held_file != file_entity(entity) {
      return false;
    }
    match (self.is_file(), self.entity.span, entity.span) {
      (true, _, _) => true,
      (false, Some(held), Some(span)) => held.start_byte <= span.start_byte && span.end_byte <= held.end_byte,
      (false, _, _) => false,
    }
  }
}

#[derive(Default)]
struct Pack {
  nodes: BTreeMap<EntityId, Node>,
  links: Vec<(EntityId, EntityId, f64)>,
  personalisation: BTreeMap<EntityId, f64>,
  seed_log: Vec<String>,
  expansion_log: Vec<String>,
  omitted: Vec<OmittedItem>,
  caveats: Vec<Caveat>,
  file_scores: Option<BTreeMap<(String, String), f64>>,
  hidden: usize,
  words: Vec<String>,
  scope: Scope,
  held: Vec<Held>,
}

#[derive(Default)]
struct Scope {
  repos: BTreeSet<String>,
  packages: BTreeSet<String>,
}

impl Scope {
  fn is_empty(&self) -> bool {
    self.repos.is_empty() && self.packages.is_empty()
  }

  fn contains(&self, entity: &Entity) -> bool {
    let repo = entity.repo.as_ref().is_some_and(|repo| self.repos.contains(repo.as_str()));
    let package = crate::queries::common::package_key(entity).is_some_and(|key| self.packages.contains(&key.display_name()));
    repo || package
  }

  fn label(&self, entity: &Entity) -> String {
    match entity.repo.as_ref().filter(|repo| self.repos.contains(repo.as_str())) {
      Some(repo) => repo.to_string(),
      None => crate::queries::common::package_key(entity).map_or_else(String::new, |key| key.display_name()),
    }
  }

  fn factor(&self, entity: &Entity, inside: f64, outside: f64) -> f64 {
    match (self.is_empty(), self.contains(entity)) {
      (true, _) => 1.0,
      (false, true) => inside,
      (false, false) => outside,
    }
  }
}

fn named_scope(query: &QueryEngine, graph: &Graph<'_, '_>, task: &str) -> QueryResult<Scope> {
  let words: BTreeSet<String> = task
    .split_whitespace()
    .map(|word| word.trim_matches(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '@' | '/'))).to_lowercase())
    .filter(|word| !word.is_empty())
    .collect();
  let mut scope = Scope::default();
  for repo in query.repos() {
    if words.contains(&repo.as_str().to_lowercase()) {
      scope.repos.insert(repo.to_string());
    }
  }
  for package in graph.entities_of_kind(&EntityKind::Package)? {
    if words.contains(&package.name.to_lowercase()) {
      scope.packages.insert(package.name);
    }
  }
  Ok(scope)
}

struct Ranked {
  node: Node,
  score: f64,
  skeleton: Vec<(u32, String)>,
  tests: Vec<TestItem>,
}

struct Planned {
  ranked: Vec<Ranked>,
  omitted: Vec<OmittedItem>,
  explain: ContextExplain,
  caveats: Vec<Caveat>,
  hidden: usize,
  held: Vec<Held>,
  test_spans: BTreeMap<String, (u32, u32)>,
}

impl Pack {
  fn seed(&mut self, entity: Entity, weight: f64, why: String, named: bool) {
    self.seed_as(entity, weight, why, named, Basis::Relation);
  }

  fn seed_search_hit(&mut self, entity: Entity, weight: f64, why: String, terms: &[String]) {
    let id = entity.id;
    self.seed_as(entity, weight, why, false, Basis::SearchHit);
    let Some(node) = self.nodes.get_mut(&id) else { return };
    for term in terms {
      if !node.terms.contains(term) {
        node.terms.push(term.clone());
      }
    }
  }

  fn seed_as(&mut self, entity: Entity, weight: f64, why: String, named: bool, basis: Basis) {
    self.seed_weighted(entity, weight, weight, why, named, basis);
  }

  fn seed_weighted(&mut self, entity: Entity, weight: f64, personal: f64, why: String, named: bool, basis: Basis) {
    self.seed_log.push(format!("{} ({why}, weight {weight:.2})", entity.key));
    let entry = self.personalisation.entry(entity.id).or_insert(0.0);
    *entry = entry.max(personal);
    let role = role_of(&entity, Role::Code);
    self.insert(Node {
      entity,
      basis,
      role,
      weight,
      uncertain: false,
      why,
      via: None,
      site: None,
      terms: Vec::new(),
      round: 0,
      named,
      seed: true,
      near_named: named,
      reuse: None,
      hub_factor: 1.0,
    });
  }

  fn insert(&mut self, incoming: Node) {
    match self.nodes.get_mut(&incoming.entity.id) {
      Some(node) => {
        match placement(node, &incoming) {
          Placement::Merged => {
            let firmer = node.uncertain && !incoming.uncertain;
            let stronger = node.uncertain == incoming.uncertain && incoming.weight > node.weight;
            if firmer || stronger {
              node.why = incoming.why;
              node.via = incoming.via;
              node.site = incoming.site;
            }
            node.uncertain &= incoming.uncertain;
            node.role = node.role.min(incoming.role);
          }
          Placement::KeepExisting => {}
          Placement::TakeIncoming => {
            node.why = incoming.why;
            node.via = incoming.via;
            node.site = incoming.site;
            node.uncertain = incoming.uncertain;
            node.role = incoming.role;
          }
        }
        if incoming.basis == Basis::Relation {
          node.basis = Basis::Relation;
        }
        node.weight = node.weight.max(incoming.weight);
        node.round = node.round.min(incoming.round);
        node.named |= incoming.named;
        node.seed |= incoming.seed;
        node.near_named |= incoming.near_named;
        node.hub_factor = node.hub_factor.max(incoming.hub_factor);
        if node.reuse.is_none() {
          node.reuse = incoming.reuse;
        }
        if node.terms.is_empty() {
          node.terms = incoming.terms;
        }
      }
      None => {
        self.nodes.insert(incoming.entity.id, incoming);
      }
    }
  }

  fn expand_callee(&mut self, graph: &Graph<'_, '_>, from: &Node, to: Entity, edge: &Edge, hub: f64) -> QueryResult<()> {
    let id = to.id;
    let before = self.nodes.get(&id).map(|node| node.hub_factor);
    self.expand_to(graph, from, to, edge, CALLEE_FACTOR, Role::Code)?;
    let reached = from.hub_factor * hub;
    if let Some(node) = self.nodes.get_mut(&id) {
      node.hub_factor = before.map_or(reached, |before| before.max(reached));
    }
    Ok(())
  }

  fn expand_to(&mut self, graph: &Graph<'_, '_>, from: &Node, to: Entity, edge: &Edge, factor: f64, role: Role) -> QueryResult<()> {
    let via = graph.relation(edge, to.id)?;
    let certainty = certainty_value(&edge.certainty);
    let uncertain = from.uncertain || !edge.certainty.is_authoritative();
    let weight = from.weight * factor * certainty;
    let why = crate::text::hop_text(&via);
    self.expansion_log.push(format!("{} -> {} ({why}, weight {weight:.3})", from.entity.key, to.key));
    self.links.push((from.entity.id, to.id, factor * certainty));
    let role = role_of(&to, role);
    let site = site_in(&to, edge);
    self.insert(Node {
      entity: to,
      basis: Basis::Relation,
      role,
      weight,
      uncertain,
      why,
      via: Some(via),
      site,
      terms: Vec::new(),
      round: from.round + 1,
      named: false,
      seed: false,
      near_named: from.named,
      reuse: None,
      hub_factor: from.hub_factor,
    });
    Ok(())
  }

  fn implemented_by(&mut self, graph: &Graph<'_, '_>, from: &Node, code: Entity, edge: &Edge, factor: f64) -> QueryResult<()> {
    let id = code.id;
    self.expand_to(graph, from, code, edge, factor, Role::Code)?;
    if let Some(node) = self.nodes.get_mut(&id) {
      node.round = node.round.min(from.round);
    }
    Ok(())
  }

  fn mention(&mut self, from: &Node, to: Entity, weight: f64, role: Role, why: String, uncertain: bool) {
    self.expansion_log.push(format!("{} -> {} ({why})", from.entity.key, to.key));
    self.links.push((from.entity.id, to.id, weight));
    let role = role_of(&to, role);
    self.insert(Node {
      entity: to,
      basis: Basis::Relation,
      role,
      weight: from.weight * weight,
      uncertain: from.uncertain || uncertain,
      via: None,
      site: None,
      terms: Vec::new(),
      round: from.round + 1,
      named: false,
      seed: false,
      near_named: from.named,
      reuse: (role == Role::Reuse).then(|| why.clone()),
      hub_factor: from.hub_factor,
      why,
    });
  }
}

fn site_in(entity: &Entity, edge: &Edge) -> Option<(u32, u32)> {
  if edge.from != entity.id || !SITE_EDGES.contains(&edge.kind) {
    return None;
  }
  let (span, repo, path) = (entity.span?, entity.repo.as_ref()?, entity.path.as_ref()?);
  edge
    .evidence
    .iter()
    .filter(|evidence| &evidence.repo == repo && &evidence.path == path)
    .filter(|evidence| span.start_line <= evidence.span.start_line && evidence.span.end_line <= span.end_line)
    .map(|evidence| (evidence.span.start_line, evidence.span.end_line))
    .reduce(|(start, end), (other_start, other_end)| (start.min(other_start), end.max(other_end)))
    .filter(|site| *site != (span.start_line, span.end_line))
}

fn placement(existing: &Node, incoming: &Node) -> Placement {
  let impact_or_uncertain = |node: &Node| node.role == Role::Impact || node.uncertain;
  match (existing.basis, incoming.basis) {
    (Basis::SearchHit, Basis::Relation) if impact_or_uncertain(incoming) => Placement::TakeIncoming,
    (Basis::Relation, Basis::SearchHit) if impact_or_uncertain(existing) => Placement::KeepExisting,
    _ => Placement::Merged,
  }
}

fn role_of(entity: &Entity, role: Role) -> Role {
  match is_test(entity) {
    true => Role::Test,
    false => role,
  }
}

impl QueryEngine {
  pub fn context(&self, request: &ContextRequest) -> QueryResult<Response<ContextAnswer>> {
    if request.depth > MAX_CONTEXT_DEPTH {
      return Err(QueryError::invalid(format!("context depth {} is above the maximum of {MAX_CONTEXT_DEPTH}", request.depth)));
    }
    let mut freshness = self.barrier()?;
    let index = self.engine().similarity();
    let policy = Policy::with_floor(request.min_confidence.unwrap_or(DEFAULT_FLOOR));
    let planned = self.read(policy, |graph| plan(self, graph, &index, request))?;
    let Planned {
      ranked,
      mut omitted,
      explain,
      caveats,
      hidden,
      held,
      test_spans,
    } = planned;
    let mut answer = ContextAnswer {
      task: request.task.clone(),
      budget: request.budget,
      estimated_tokens: 0,
      must_read: Vec::new(),
      skim: Vec::new(),
      patterns: Vec::new(),
      reuse: Vec::new(),
      impact: Vec::new(),
      tests: Vec::new(),
      uncertain: Vec::new(),
      summaries: Vec::new(),
      omitted: Listing::default(),
      omitted_by_reason: BTreeMap::new(),
      explain: None,
      caveats: Vec::new(),
    };
    let top = ranked.first().map_or(0.0, |item| item.score);
    let mut code: Vec<Ranked> = Vec::new();
    let mut tests_by_owner: Vec<(bool, f64, Vec<TestItem>)> = Vec::new();
    let mut impact = Vec::new();
    let mut uncertain = Vec::new();
    let mut held_counts = vec![0_usize; held.len()];
    for item in ranked {
      if item.node.named {
        tests_by_owner.push((true, item.score, item.tests.clone()));
      }
      let code_item = item.node.role == Role::Code && !item.node.uncertain;
      if let Some(index) = held.iter().position(|held| code_item && held.covers(&item.node.entity)) {
        if item.score >= LOW_RELEVANCE * top {
          held_counts[index] += 1;
          if !item.node.named {
            tests_by_owner.push((false, item.score, item.tests.clone()));
          }
        }
        continue;
      }
      if let Some(reason) = item.node.reuse.clone().filter(|_| !item.node.named && item.node.role != Role::Test) {
        let mut entry = item_of(&item);
        entry.why = reason;
        answer.reuse.push(entry);
        continue;
      }
      match (item.node.role, item.node.uncertain) {
        (Role::Test, _) => {}
        (Role::Reuse, _) => answer.reuse.push(item_of(&item)),
        (Role::Pattern, _) => answer.patterns.push(item_of(&item)),
        (Role::Impact, _) => impact.push((item.node.near_named, item_of(&item))),
        (Role::Code, true) => uncertain.push(item_of(&item)),
        (Role::Code, false) if item.score < LOW_RELEVANCE * top || code.len() >= MAX_CODE_ITEMS => {
          let entry = item_of(&item);
          omitted.push(OmittedItem {
            label: crate::text::entity_line(&entry.entity),
            entity: Some(entry.entity),
            reason: OmissionReason::LowRelevance,
          })
        }
        (Role::Code, false) => code.push(item),
      }
    }
    code.sort_by(|left, right| {
      right
        .node
        .named
        .cmp(&left.node.named)
        .then_with(|| right.score.total_cmp(&left.score))
        .then_with(|| left.node.entity.key.cmp(&right.node.entity.key))
    });
    impact.sort_by(|left, right| {
      right
        .0
        .cmp(&left.0)
        .then_with(|| right.1.score.total_cmp(&left.1.score))
        .then_with(|| left.1.entity.key.cmp(&right.1.entity.key))
    });
    let impact = impact.into_iter().map(|(_, item)| item).collect();
    let (shown, impact_summary) = capped(IMPACT, impact, IMPACT_SHOWN, "run impact on the seeds for the full blast radius");
    answer.impact = shown;
    let (shown, uncertain_summary) = capped(
      UNCERTAIN,
      uncertain,
      UNCERTAIN_SHOWN,
      "relations resting on heuristic or inferred edges; verify before relying on them",
    );
    answer.uncertain = shown;
    if hidden > 0 {
      omitted.push(OmittedItem {
        entity: None,
        label: format!("{hidden} relations below confidence {:.2}", policy.min_confidence),
        reason: OmissionReason::BelowConfidenceFloor,
      });
    }
    for (held, count) in held.iter().zip(held_counts).filter(|(_, count)| *count > 0) {
      let entity = EntityRef::of(&held.entity);
      omitted.push(OmittedItem {
        label: format!("{} as {} ({})", crate::text::entity_line(&entity), held.spec, crate::text::counted(count, "pack item")),
        entity: Some(entity),
        reason: OmissionReason::Held,
      });
    }
    let run = freshness.run;
    answer.caveats = caveats.iter().cloned().chain(self.problem_files(run)?).collect();
    let reserved = tests_by_owner
      .iter()
      .cloned()
      .chain(code.iter().map(|item| (item.node.named, item.score, item.tests.clone())));
    let (tests, test_summary) = select_tests(reserved.collect(), &test_spans);
    answer.tests = tests;
    answer.summaries = [test_summary, impact_summary.clone(), uncertain_summary.clone()].into_iter().flatten().collect();
    answer.estimated_tokens = request.budget;
    answer.set_omitted(&omitted);
    let budget = request.budget * 4;
    let must_read_budget = (request.budget as f64 * MUST_READ_SHARE) as usize * 4;
    let mut used = pack_chars(&answer);
    let must_read_floor = code.iter().map(|item| item.score).fold(0.0_f64, f64::max) * MUST_READ_RELEVANCE;
    let mut included: Vec<(String, String, u32, u32, bool)> = Vec::new();
    let mut skim_candidates = Vec::new();
    let mut excerpts: Vec<PlannedExcerpt> = Vec::new();
    let terms = key_terms(&request.task);
    {
      let mut reader = self.sources(&mut freshness);
      for item in code {
        let entity = &item.node.entity;
        let location = match (&entity.repo, &entity.path, entity.span) {
          (Some(repo), Some(path), Some(span)) if entity.kind.is_code_symbol() => Some((repo.to_string(), path.to_string(), span.start_line, span.end_line)),
          _ => None,
        };
        let cover = location.as_ref().and_then(|(repo, path, start, end)| {
          included
            .iter()
            .find(|(other_repo, other_path, other_start, other_end, _)| other_repo == repo && other_path == path && other_start <= start && end <= other_end)
        });
        if let Some((.., outline)) = cover {
          let reason = match outline {
            true => OmissionReason::ShownInOutline,
            false => OmissionReason::ShownInEnclosingItem,
          };
          used += omit(&mut omitted, item_of(&item), reason);
          continue;
        }
        let relevant = item.node.named || item.score >= must_read_floor;
        let Some((repo, path, start, end)) = location.clone().filter(|_| relevant) else {
          skim_candidates.push(item);
          continue;
        };
        let contains_included = included
          .iter()
          .any(|(other_repo, other_path, other_start, other_end, _)| *other_repo == repo && *other_path == path && start <= *other_start && *other_end <= end);
        let type_like = matches!(
          entity.kind,
          EntityKind::Class | EntityKind::Struct | EntityKind::Trait | EntityKind::Interface | EntityKind::Enum
        );
        let skeleton_only = type_like && (contains_included || end.saturating_sub(start) > LARGE_TYPE_LINES) && !item.skeleton.is_empty();
        let edit_target = item.node.named && end - start + 1 > SMALL_BODY_LINES;
        let mut entry = code_item_of(&item);
        match (edit_target, skeleton_only) {
          (true, _) => entry.read_directly = true,
          (false, true) => {}
          (false, false) => {
            entry.body = Some(reader.snippet(entity.id, SnippetMode::Body { max_lines: Some(BODY_LINES) }, 0)?);
            entry.skeleton = item
              .skeleton
              .iter()
              .filter(|(line, _)| !(start..=end).contains(line))
              .map(|(_, text)| text.clone())
              .collect();
          }
        }
        let cost = title_chars(MUST_READ, answer.must_read.len()) + rendered_chars(|out| must_read_item(out, &entry));
        if used + cost > must_read_budget {
          skim_candidates.push(item);
          continue;
        }
        if entry.body.is_none() {
          excerpts.extend(PlannedExcerpt::of(Section::MustRead, answer.must_read.len(), &item.node, &terms));
        }
        if !skeleton_only {
          included.push((repo, path, start, end, edit_target));
        }
        used += cost;
        tests_by_owner.push((item.node.named, item.score, item.tests.clone()));
        answer.must_read.push(entry);
      }
    }
    let (tests, test_summary) = select_tests(tests_by_owner, &test_spans);
    answer.tests = tests;
    answer.summaries = [test_summary, impact_summary, uncertain_summary].into_iter().flatten().collect();
    answer.set_omitted(&omitted);
    used = pack_chars(&answer);
    for item in skim_candidates {
      let entry = code_item_of(&item);
      let cost = title_chars(SKIM, answer.skim.len()) + rendered_chars(|out| skim_item(out, &entry));
      if used + cost + omission_growth(&omitted, OmissionReason::Budget) <= budget {
        used += cost;
        excerpts.extend(PlannedExcerpt::of(Section::Skim, answer.skim.len(), &item.node, &terms));
        answer.skim.push(entry);
        continue;
      }
      used += omit(&mut omitted, entry, OmissionReason::Budget);
    }
    answer.set_omitted(&omitted);
    used = pack_chars(&answer);
    {
      let mut reader = self.sources(&mut freshness);
      for planned in excerpts {
        let Some(excerpt) = planned.read(&mut reader)? else { continue };
        let cost = rendered_chars(|out| excerpt_lines(out, &excerpt));
        if used + cost > budget {
          continue;
        }
        used += cost;
        let section = match planned.section {
          Section::MustRead => &mut answer.must_read,
          Section::Skim => &mut answer.skim,
        };
        if let Some(entry) = section.get_mut(planned.index) {
          entry.excerpt = Some(excerpt);
        }
      }
    }
    if freshness.run != run {
      answer.caveats = caveats.into_iter().chain(self.problem_files(freshness.run)?).collect();
    }
    let over_budget = pack_chars(&answer) > budget;
    if over_budget {
      answer.caveats.insert(0, over_budget_note(request.budget, request.budget));
    }
    answer.settle_estimate(over_budget);
    answer.explain = request.explain.then_some(explain);
    answer.settle_estimate(over_budget);
    Ok(Response { freshness, answer })
  }
}

#[derive(Clone, Copy)]
enum Section {
  MustRead,
  Skim,
}

enum KeySite {
  Lines(u32),
  Terms(BTreeSet<String>),
}

struct PlannedExcerpt {
  section: Section,
  index: usize,
  entity: EntityId,
  span: (u32, u32),
  key: KeySite,
}

impl PlannedExcerpt {
  fn of(section: Section, index: usize, node: &Node, task_terms: &BTreeSet<String>) -> Option<Self> {
    let span = node.entity.span.filter(|span| span.start_line < span.end_line)?;
    let key = match (node.site, node.named) {
      (Some((line, _)), _) => KeySite::Lines(line),
      (None, true) => {
        let own: BTreeSet<String> = analyze_terms(&node.entity.name).into_iter().collect();
        KeySite::Terms(task_terms.difference(&own).cloned().collect())
      }
      (None, false) => KeySite::Terms(node.terms.iter().cloned().collect()),
    };
    if matches!(&key, KeySite::Terms(terms) if terms.is_empty()) {
      return None;
    }
    Some(Self {
      section,
      index,
      entity: node.entity.id,
      span: (span.start_line, span.end_line),
      key,
    })
  }

  fn read(&self, reader: &mut SourceReader<'_>) -> QueryResult<Option<Snippet>> {
    let Snippet::Text { start_line, lines, .. } = reader.snippet(self.entity, SnippetMode::Body { max_lines: None }, 0)? else {
      return Ok(None);
    };
    let key = match &self.key {
      KeySite::Lines(line) => *line,
      KeySite::Terms(terms) => match key_line(start_line, &lines, terms) {
        Some(line) => line,
        None => return Ok(None),
      },
    };
    let (start, end) = self.span;
    let first = key.saturating_sub(1).max(start).max(start_line);
    let last = (first + EXCERPT_LINES - 1).min(end);
    let selected: Vec<String> = lines
      .iter()
      .skip((first - start_line) as usize)
      .take((last + 1).saturating_sub(first) as usize)
      .cloned()
      .collect();
    if selected.is_empty() {
      return Ok(None);
    }
    Ok(Some(Snippet::Text {
      start_line: first,
      lines: selected,
      omitted_lines: 0,
    }))
  }
}

fn key_terms(task: &str) -> BTreeSet<String> {
  analyze_terms(&search_text(task)).into_iter().collect()
}

fn key_line(start_line: u32, lines: &[String], terms: &BTreeSet<String>) -> Option<u32> {
  let mut best: Option<(usize, u32)> = None;
  for (offset, line) in lines.iter().enumerate().skip(1) {
    let found: BTreeSet<String> = analyze_terms(line).into_iter().filter(|term| terms.contains(term)).collect();
    if found.is_empty() || best.is_some_and(|(count, _)| count >= found.len()) {
      continue;
    }
    best = Some((found.len(), start_line.saturating_add(u32::try_from(offset).unwrap_or(u32::MAX))));
  }
  best.map(|(_, line)| line)
}

fn lines_of(entity: &Entity, site: Option<(u32, u32)>) -> Option<LineRange> {
  entity.path.as_ref()?;
  match (site, entity.span) {
    (Some((start, end)), _) => Some(LineRange {
      start,
      end,
      basis: RangeBasis::Site,
    }),
    (None, Some(span)) => Some(LineRange {
      start: span.start_line,
      end: span.end_line,
      basis: RangeBasis::Symbol,
    }),
    (None, None) => None,
  }
}

fn select_tests(owners: Vec<(bool, f64, Vec<TestItem>)>, spans: &BTreeMap<String, (u32, u32)>) -> (Vec<ContextItem>, Option<SectionSummary>) {
  let mut best: BTreeMap<String, (TestItem, bool, f64)> = BTreeMap::new();
  for (named, score, tests) in owners {
    for test in tests {
      let replace = best.get(&test.test.key).is_none_or(|(existing, existing_named, existing_score)| {
        (test.reach, test.uncertain, !named, -score) < (existing.reach, existing.uncertain, !existing_named, -existing_score)
      });
      if replace {
        best.insert(test.test.key.clone(), (test, named, score));
      }
    }
  }
  let mut ranked: Vec<(TestItem, bool, f64)> = best.into_values().collect();
  ranked.sort_by(|left, right| {
    (left.0.uncertain, left.0.reach, !left.1)
      .cmp(&(right.0.uncertain, right.0.reach, !right.1))
      .then_with(|| right.2.total_cmp(&left.2))
      .then_with(|| left.0.test.key.cmp(&right.0.test.key))
  });
  let items: Vec<ContextItem> = ranked
    .into_iter()
    .map(|(test, _, score)| ContextItem {
      why: format!("{} ({})", crate::text::hop_text(&test.relation), test.reach.as_str()),
      certainty: Some(test.relation.certainty.clone()),
      via: Some(test.relation.clone()),
      uncertain: test.uncertain,
      signature: None,
      doc: None,
      skeleton: Vec::new(),
      lines: spans.get(&test.test.key).map(|(start, end)| LineRange {
        start: *start,
        end: *end,
        basis: RangeBasis::Symbol,
      }),
      excerpt: None,
      body: None,
      read_directly: false,
      run: test.run.clone(),
      named: false,
      search_terms: Vec::new(),
      score,
      entity: test.test,
    })
    .collect();
  capped(TESTS, items, TESTS_SHOWN, "run tests-for on the MUST READ items for the full list")
}

fn capped(section: &str, items: Vec<ContextItem>, shown: usize, hint: &str) -> (Vec<ContextItem>, Option<SectionSummary>) {
  let mut items = items;
  if items.len() <= shown {
    return (items, None);
  }
  let rest = items.split_off(shown);
  let mut by_repo: BTreeMap<String, usize> = BTreeMap::new();
  for item in &rest {
    *by_repo.entry(item.entity.repo().to_owned()).or_default() += 1;
  }
  let summary = SectionSummary {
    section: section.to_owned(),
    omitted: rest.len(),
    uncertain: rest.iter().filter(|item| item.uncertain).count(),
    by_repo,
    hint: hint.to_owned(),
  };
  (items, Some(summary))
}

fn tokens(chars: usize) -> usize {
  chars.div_ceil(4)
}

fn pack_chars(answer: &ContextAnswer) -> usize {
  rendered_chars(|out| answer.render_text(out))
}

fn title_chars(title: &str, count: usize) -> usize {
  rendered_chars(|out| section_title(out, title, count + 1)) - rendered_chars(|out| section_title(out, title, count))
}

fn omitted_chars(omitted: &[OmittedItem]) -> usize {
  let mut listed: Vec<OmittedItem> = omitted.iter().filter(|item| listed_when_collapsed(item.reason)).cloned().collect();
  listed.sort_by(omission_order);
  listed.truncate(MAX_CODE_ITEMS);
  let listing = Listing {
    total: omitted.len(),
    items: listed,
  };
  rendered_chars(|out| omitted_section(out, &listing, &reason_counts(omitted), false))
}

fn omission_growth(omitted: &[OmittedItem], reason: OmissionReason) -> usize {
  let mut grown = omitted.to_vec();
  grown.push(OmittedItem {
    entity: None,
    label: String::new(),
    reason,
  });
  omitted_chars(&grown).saturating_sub(omitted_chars(omitted))
}

fn omit(omitted: &mut Vec<OmittedItem>, entry: ContextItem, reason: OmissionReason) -> usize {
  let before = omitted_chars(omitted);
  omitted.push(OmittedItem {
    label: crate::text::entity_line(&entry.entity),
    entity: Some(entry.entity),
    reason,
  });
  omitted_chars(omitted).saturating_sub(before)
}

fn listed_when_collapsed(reason: OmissionReason) -> bool {
  reason.is_listed_when_collapsed() || reason == OmissionReason::BelowConfidenceFloor
}

fn omission_order(left: &OmittedItem, right: &OmittedItem) -> std::cmp::Ordering {
  let key = |item: &OmittedItem| {
    let reason = item.reason;
    (
      !reason.is_seed_diagnostic(),
      reason != OmissionReason::Held,
      reason != OmissionReason::BelowConfidenceFloor,
      reason,
    )
  };
  key(left).cmp(&key(right)).then_with(|| left.label.cmp(&right.label))
}

fn reason_counts(omitted: &[OmittedItem]) -> BTreeMap<OmissionReason, usize> {
  let mut counts = BTreeMap::new();
  for item in omitted {
    *counts.entry(item.reason).or_default() += 1;
  }
  counts
}

impl ContextAnswer {
  fn set_omitted(&mut self, omitted: &[OmittedItem]) {
    let mut sorted = omitted.to_vec();
    sorted.sort_by(omission_order);
    self.omitted_by_reason = reason_counts(omitted);
    self.omitted = Listing::truncated(sorted, MAX_CODE_ITEMS);
  }

  fn settle_estimate(&mut self, over_budget: bool) {
    for _ in 0..4 {
      let estimate = tokens(pack_chars(self));
      if estimate == self.estimated_tokens {
        return;
      }
      self.estimated_tokens = estimate;
      if over_budget {
        self.caveats[0] = over_budget_note(estimate, self.budget);
      }
    }
  }
}

fn over_budget_note(estimate: usize, budget: usize) -> Caveat {
  Caveat::Note {
    message: format!("mandatory mentions exceed the budget ({estimate} > {budget} tokens); uncertain and impact items are never dropped silently"),
  }
}

fn code_item_of(item: &Ranked) -> ContextItem {
  ContextItem {
    lines: lines_of(&item.node.entity, None),
    ..item_of(item)
  }
}

fn item_of(item: &Ranked) -> ContextItem {
  let entity = &item.node.entity;
  ContextItem {
    entity: EntityRef::of(entity),
    score: item.score,
    why: item.node.why.clone(),
    certainty: item.node.via.as_ref().map(|via| via.certainty.clone()),
    via: item.node.via.clone(),
    uncertain: item.node.uncertain,
    signature: entity.signature.clone(),
    doc: first_line(entity.doc.as_deref()),
    skeleton: item.skeleton.iter().map(|(_, line)| line.clone()).collect(),
    lines: lines_of(entity, item.node.site),
    excerpt: None,
    body: None,
    read_directly: false,
    run: match item.node.role {
      Role::Test => run_hint(entity),
      _ => None,
    },
    named: item.node.named,
    search_terms: match item.node.basis {
      Basis::SearchHit => item.node.terms.clone(),
      Basis::Relation => Vec::new(),
    },
  }
}

fn plan(query: &QueryEngine, graph: &Graph<'_, '_>, index: &SimilarityIndex, request: &ContextRequest) -> QueryResult<Planned> {
  let mut pack = Pack {
    words: query_words(&request.task),
    scope: named_scope(query, graph, &request.task)?,
    ..Pack::default()
  };
  seed(query, graph, index, request, &mut pack)?;
  expand(graph, index, &mut pack, request.depth)?;
  exemplars(graph, &mut pack)?;
  let scope = std::mem::take(&mut pack.scope);
  let mut ranked = rank(&mut pack);
  if !scope.is_empty() {
    for item in &mut ranked {
      if !item.node.named {
        item.score = (item.score * scope.factor(&item.node.entity, SCOPE_BOOST, SCOPE_PENALTY) * 1_000_000.0).round() / 1_000_000.0;
      }
    }
    ranked.sort_by(|left, right| right.score.total_cmp(&left.score).then_with(|| left.node.entity.key.cmp(&right.node.entity.key)));
  }
  let mut test_owners = 0;
  let mut test_spans = BTreeMap::new();
  for item in &mut ranked {
    if is_type_like(&item.node.entity.kind) {
      item.skeleton = skeleton(graph, &item.node.entity)?;
    }
    let wants_tests = item.node.seed || (item.node.role == Role::Code && !item.node.uncertain && test_owners < TEST_CANDIDATES);
    if wants_tests && item.node.entity.kind.is_code_symbol() && !is_test(&item.node.entity) {
      test_owners += usize::from(!item.node.seed);
      let own_name = short_name(&item.node.entity.key);
      let type_like = is_type_like(&item.node.entity.kind);
      item.tests = direct_tests(graph, &item.node.entity)?
        .into_iter()
        .filter(|test| !type_like || test.relation.entity == own_name || test.reach == crate::queries::TestReach::CoversFile)
        .collect();
      for test in &item.tests {
        if let Some(span) = graph.entity(test.test.entity_id()?)?.and_then(|entity| entity.span) {
          test_spans.insert(test.test.key.clone(), (span.start_line, span.end_line));
        }
      }
    }
  }
  let explain = ContextExplain {
    seeds: std::mem::take(&mut pack.seed_log),
    expansions: std::mem::take(&mut pack.expansion_log),
    scores: ranked
      .iter()
      .map(|item| ScoredKey {
        key: item.node.entity.key.to_string(),
        score: item.score,
      })
      .collect(),
  };
  Ok(Planned {
    ranked,
    omitted: std::mem::take(&mut pack.omitted),
    explain,
    caveats: std::mem::take(&mut pack.caveats),
    hidden: pack.hidden,
    held: std::mem::take(&mut pack.held),
    test_spans,
  })
}

fn is_type_like(kind: &EntityKind) -> bool {
  matches!(kind, EntityKind::Class | EntityKind::Struct | EntityKind::Trait | EntityKind::Interface | EntityKind::Enum)
}

fn skeleton(graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<Vec<(u32, String)>> {
  let mut members: Vec<Entity> = graph.children(entity.id)?.into_iter().filter(|child| !is_test(child)).collect();
  members.sort_by_key(|member| (member.path.clone(), member.span.map_or(0, |span| span.start_byte)));
  let total = members.len();
  let mut lines: Vec<(u32, String)> = members
    .iter()
    .take(SKELETON_MEMBERS)
    .map(|member| {
      let line = match member.path == entity.path {
        true => member.span.map_or(0, |span| span.start_line),
        false => 0,
      };
      (line, member.signature.clone().unwrap_or_else(|| format!("{} {}", member.kind, member.name)))
    })
    .collect();
  if total > SKELETON_MEMBERS {
    lines.push((0, format!("... {} more members", total - SKELETON_MEMBERS)));
  }
  Ok(lines)
}

fn seed(query: &QueryEngine, graph: &Graph<'_, '_>, index: &SimilarityIndex, request: &ContextRequest, pack: &mut Pack) -> QueryResult<()> {
  let repos = query.repos();
  for spec in &request.seeds {
    match resolve(graph, &repos, spec, &TargetFilter::default())? {
      Resolution::Found(entity) => pack.seed(*entity, 1.0, "explicit seed".to_owned(), true),
      Resolution::Ambiguous(candidates) => {
        let total = candidates.len();
        let used = limit_candidates(graph, index, &request.task, spec, candidates, pack)?;
        let named: Vec<String> = used.iter().map(|entity| crate::text::entity_line(&EntityRef::of(entity))).collect();
        pack.caveats.push(Caveat::Note {
          message: format!("seed {spec} matches {total} entities; seeded {}: {}", used.len(), named.join("; ")),
        });
        for entity in used {
          pack.seed(entity, 1.0, format!("explicit seed {spec} (one of {total} matches)"), true);
        }
      }
      Resolution::NotFound(hint) => pack.omitted.push(OmittedItem {
        entity: None,
        label: format!("{spec}: {hint}"),
        reason: OmissionReason::UnknownSeed,
      }),
    }
  }
  for spec in &request.known {
    match resolve(graph, &repos, spec, &TargetFilter::default())? {
      Resolution::Found(entity) => hold(graph, pack, spec, *entity)?,
      Resolution::Ambiguous(candidates) => ambiguous_known(graph, index, &request.task, spec, candidates, pack)?,
      Resolution::NotFound(hint) => pack.omitted.push(OmittedItem {
        entity: None,
        label: format!("known {spec}: {hint}"),
        reason: OmissionReason::UnknownKnown,
      }),
    }
  }
  for identifier in identifiers(&request.task) {
    let platform = PLATFORM_TYPES.contains(&identifier.as_str());
    let candidates = match resolve(graph, &repos, &identifier, &TargetFilter::default()) {
      Ok(Resolution::Found(entity)) if !platform => {
        pack.seed(*entity, 1.0, format!("named in task as {identifier}"), true);
        continue;
      }
      Ok(Resolution::Found(entity)) => vec![*entity],
      Ok(Resolution::Ambiguous(candidates)) => candidates,
      _ => continue,
    };
    let scoped: Vec<&Entity> = candidates.iter().filter(|entity| pack.scope.contains(entity)).collect();
    if let [entity] = scoped.as_slice() {
      let why = format!("named in task as {identifier} (the only match in {})", pack.scope.label(entity));
      pack.seed((*entity).clone(), 1.0, why, true);
      continue;
    }
    if platform {
      continue;
    }
    for entity in limit_candidates(graph, index, &request.task, &identifier, candidates, pack)? {
      pack.seed(entity, 0.4, format!("named in task as {identifier} (ambiguous)"), false);
    }
  }
  route_seeds(graph, &request.task, pack)?;
  auth_seeds(graph, &request.task, pack)?;
  scope_seeds(graph, pack)?;
  search_seeds(graph, index, &request.task, pack)
}

fn ambiguous_known(graph: &Graph<'_, '_>, index: &SimilarityIndex, task: &str, spec: &str, candidates: Vec<Entity>, pack: &mut Pack) -> QueryResult<()> {
  let total = candidates.len();
  let file_scores = pack.file_scores.get_or_insert_with(|| file_scores(index, task));
  let ranked = rank_candidates(graph, file_scores, &pack.scope, candidates)?;
  pack.omitted.push(OmittedItem {
    entity: None,
    label: format!("known {spec}: {total} candidates, none held; pass a key or a path"),
    reason: OmissionReason::AmbiguousKnown,
  });
  for entity in ranked.iter().take(OMITTED_CANDIDATES_SHOWN) {
    let entry = EntityRef::of(entity);
    pack.omitted.push(OmittedItem {
      label: format!("known {spec}: candidate {}", crate::text::entity_line(&entry)),
      entity: Some(entry),
      reason: OmissionReason::AmbiguousKnown,
    });
  }
  Ok(())
}

fn hold(graph: &Graph<'_, '_>, pack: &mut Pack, spec: &str, entity: Entity) -> QueryResult<()> {
  let why = format!("held by the caller as {spec}");
  match entity.kind {
    EntityKind::File => {
      let symbols: Vec<Entity> = neighbours(graph, pack, entity.id, crate::policy::Direction::Forward, &[EdgeKind::Contains])?
        .into_iter()
        .map(|(_, symbol)| symbol)
        .filter(|symbol| symbol.kind.is_code_symbol() && !is_test(symbol))
        .collect();
      let personal = KNOWN_FILE_WEIGHT / symbols.len().max(1) as f64;
      for symbol in symbols {
        pack.seed_weighted(symbol, 1.0, personal, why.clone(), false, Basis::Relation);
      }
    }
    _ => pack.seed(entity.clone(), 1.0, why, true),
  }
  pack.held.push(Held { spec: spec.to_owned(), entity });
  Ok(())
}

fn limit_candidates(graph: &Graph<'_, '_>, index: &SimilarityIndex, task: &str, name: &str, candidates: Vec<Entity>, pack: &mut Pack) -> QueryResult<Vec<Entity>> {
  if candidates.len() <= AMBIGUOUS_SEED_LIMIT {
    return Ok(candidates);
  }
  let total = candidates.len();
  let file_scores = pack.file_scores.get_or_insert_with(|| file_scores(index, task));
  let mut ranked = rank_candidates(graph, file_scores, &pack.scope, candidates)?;
  let rest = ranked.split_off(AMBIGUOUS_SEED_LIMIT);
  pack.omitted.push(OmittedItem {
    entity: None,
    label: format!("{name}: {total} candidates, {AMBIGUOUS_SEED_LIMIT} used"),
    reason: OmissionReason::AmbiguousSeed,
  });
  for entity in rest.iter().take(OMITTED_CANDIDATES_SHOWN) {
    let entry = EntityRef::of(entity);
    pack.omitted.push(OmittedItem {
      label: format!("{name} candidate {}", crate::text::entity_line(&entry)),
      entity: Some(entry),
      reason: OmissionReason::AmbiguousSeed,
    });
  }
  Ok(ranked)
}

fn file_scores(index: &SimilarityIndex, task: &str) -> BTreeMap<(String, String), f64> {
  let mut scores: BTreeMap<(String, String), f64> = BTreeMap::new();
  for hit in lexical_hits(index, &search_text(task), &[], None, CANDIDATE_RANK_HITS) {
    let score = scores.entry((hit.repo.to_string(), hit.path.to_string())).or_insert(0.0);
    *score = score.max(hit.score);
  }
  scores
}

fn rank_candidates(graph: &Graph<'_, '_>, file_scores: &BTreeMap<(String, String), f64>, scope: &Scope, candidates: Vec<Entity>) -> QueryResult<Vec<Entity>> {
  let mut scored: Vec<(bool, f64, usize, Entity)> = Vec::with_capacity(candidates.len());
  for entity in candidates {
    let file = match (&entity.repo, &entity.path) {
      (Some(repo), Some(path)) => file_scores.get(&(repo.to_string(), path.to_string())).copied().unwrap_or(0.0),
      _ => 0.0,
    };
    let fan_in = graph.edges_in(entity.id, &[EdgeKind::Calls, EdgeKind::References, EdgeKind::Instantiates])?.len();
    scored.push((!scope.is_empty() && scope.contains(&entity), file, fan_in, entity));
  }
  scored.sort_by(|left, right| {
    right
      .0
      .cmp(&left.0)
      .then_with(|| right.1.total_cmp(&left.1))
      .then_with(|| right.2.cmp(&left.2))
      .then_with(|| left.3.key.cmp(&right.3.key))
  });
  Ok(scored.into_iter().map(|(_, _, _, entity)| entity).collect())
}

const PACKAGE_ROOTS: [&str; 5] = ["src/lib.rs", "src/main.rs", "src/index.ts", "index.ts", "src/index.js"];

fn scope_seeds(graph: &Graph<'_, '_>, pack: &mut Pack) -> QueryResult<()> {
  for package in graph.entities_of_kind(&EntityKind::Package)? {
    if !pack.scope.packages.contains(&package.name) {
      continue;
    }
    let (Some(repo), Some(manifest)) = (crate::queries::common::attr_str(&package, "repo"), crate::queries::common::attr_str(&package, "manifest")) else {
      continue;
    };
    let Ok(repo) = atlas_core::RepoName::new(repo) else { continue };
    let directory = manifest.rsplit_once('/').map(|(directory, _)| format!("{directory}/")).unwrap_or_default();
    for root in PACKAGE_ROOTS {
      let Ok(path) = atlas_core::RelPath::new(&format!("{directory}{root}")) else { continue };
      let Some(file) = graph.tx().entity_by_key(&EntityKey::File { repo: repo.clone(), path })? else {
        continue;
      };
      for edge in graph.edges_out(file.id, &[EdgeKind::ReExports, EdgeKind::Exports])? {
        let Some(exported) = graph.entity(edge.to)? else { continue };
        if exported.kind.is_code_symbol() && !is_test(&exported) {
          let why = format!("public API of {} named in the task", package.name);
          pack.seed(exported, SCOPE_ENTRY_WEIGHT, why, false);
        }
      }
    }
  }
  Ok(())
}

fn search_text(task: &str) -> String {
  task
    .split_whitespace()
    .filter(|word| {
      let structured = word.contains('_') || word.contains("::") || word.chars().skip(1).any(char::is_uppercase);
      let bare = word.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
      structured || !GENERIC_WORDS.contains(&bare.as_str())
    })
    .collect::<Vec<_>>()
    .join(" ")
}

fn is_test_support(entity: &Entity) -> bool {
  entity.path.as_ref().is_some_and(|path| {
    let file = path.file_name();
    path.segments().any(|segment| matches!(segment, "tests" | "test" | "__tests__" | "spec")) || file.contains(".spec.") || file.contains(".test.") || file.ends_with("_test.rs")
  })
}

fn search_seeds(graph: &Graph<'_, '_>, index: &SimilarityIndex, task: &str, pack: &mut Pack) -> QueryResult<()> {
  let about_tests = pack.words.iter().any(|word| word.starts_with("test"));
  let anchored: BTreeSet<String> = pack.nodes.values().filter_map(|node| node.entity.repo.as_ref().map(ToString::to_string)).collect();
  let text = search_text(task);
  let scoped = !pack.scope.is_empty();
  let fetched = lexical_hits(index, &text, &[], None, SEARCH_SEEDS * if scoped { 10 } else { 3 });
  let mut inside = Vec::new();
  let mut outside = Vec::new();
  for hit in fetched {
    match graph.entity(hit.entity)?.is_some_and(|entity| pack.scope.contains(&entity)) {
      true => inside.push(hit),
      false => outside.push(hit),
    }
  }
  let inside_top = inside.first().map_or(0.0, |hit| hit.score);
  let outside_top = outside.first().map_or(0.0, |hit| hit.score);
  let hits: Vec<(atlas_similarity::SearchHit, f64)> = inside
    .into_iter()
    .map(|hit| (hit, inside_top))
    .chain(outside.into_iter().map(|hit| (hit, outside_top)))
    .collect();
  let mut code_terms: BTreeSet<String> = BTreeSet::new();
  for (hit, _) in &hits {
    if graph.entity(hit.entity)?.is_some_and(|entity| !is_test(&entity)) {
      code_terms.extend(hit.direct_terms.iter().cloned());
    }
  }
  let weigh = |pack: &Pack, entity: &Entity, hit: &atlas_similarity::SearchHit, top: f64| {
    let off_anchor = !anchored.is_empty() && entity.repo.as_ref().is_none_or(|repo| !anchored.contains(repo.as_str()));
    let anchor_factor = if off_anchor { OFF_ANCHOR_FACTOR } else { 1.0 };
    SEARCH_WEIGHT * hit.score / top * anchor_factor * pack.scope.factor(entity, SCOPE_BOOST, SCOPE_SEED_PENALTY)
  };
  let mut taken = 0;
  let mut test_subjects: BTreeSet<EntityId> = BTreeSet::new();
  for (hit, top) in &hits {
    if taken >= SEARCH_SEEDS {
      break;
    }
    if hit.score < SEARCH_FRACTION * top {
      continue;
    }
    let Some(entity) = graph.entity(hit.entity)? else { continue };
    if matches!(entity.kind, EntityKind::Field | EntityKind::Variable) || (!about_tests && !is_test(&entity) && is_test_support(&entity)) {
      continue;
    }
    let weight = weigh(pack, &entity, hit, *top);
    if !is_test(&entity) {
      taken += 1;
      let terms = unstemmed(&hit.direct_terms, &pack.words).join(" ");
      pack.seed_search_hit(entity, weight, format!("search hit on {terms}"), &hit.direct_terms);
      continue;
    }
    if test_subjects.len() >= SEARCH_SEEDS {
      continue;
    }
    let novel: Vec<String> = hit.direct_terms.iter().filter(|term| !code_terms.contains(*term)).cloned().collect();
    if novel.is_empty() {
      continue;
    }
    let terms = unstemmed(&novel, &pack.words).join(" ");
    let mut hidden = 0;
    let test_name = short_name(&entity.key);
    for edge in graph.admitted(
      entity.id,
      crate::policy::Direction::Forward,
      &[EdgeKind::Calls, EdgeKind::Instantiates, EdgeKind::References],
      &mut hidden,
    )? {
      let Some(subject) = graph.entity(edge.to)? else { continue };
      if is_test(&subject) || is_test_support(&subject) || !subject.kind.is_code_symbol() || subject.repo != entity.repo {
        continue;
      }
      let why = format!("exercised by test {test_name} matching {terms}");
      let subject_id = subject.id;
      pack.seed_search_hit(subject, weight * TEST_SUBJECT_FACTOR, why, &[]);
      test_subjects.insert(subject_id);
    }
    pack.hidden += hidden;
  }
  if !pack.nodes.is_empty() {
    return Ok(());
  }
  for (hit, top) in &hits {
    let Some(entity) = graph.entity(hit.entity)? else { continue };
    if is_test(&entity) || matches!(entity.kind, EntityKind::Field | EntityKind::Variable) || (!about_tests && is_test_support(&entity)) {
      continue;
    }
    let weight = weigh(pack, &entity, hit, *top);
    let terms = unstemmed(&hit.direct_terms, &pack.words).join(" ");
    pack.seed_search_hit(entity, weight, format!("search hit on {terms}"), &hit.direct_terms);
    break;
  }
  Ok(())
}

fn identifiers(task: &str) -> Vec<String> {
  let mut found: BTreeSet<String> = code_span_names(task).collect();
  for word in task.split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '$'))) {
    let word = word.trim_matches(|c: char| matches!(c, '.' | ':'));
    if word.len() < 3 || word.chars().all(|c| c.is_uppercase() || !c.is_alphabetic()) {
      continue;
    }
    let inner_upper = word.chars().skip(1).any(char::is_uppercase) && word.chars().any(char::is_lowercase);
    let structured = word.contains('_') || word.contains("::") || (word.contains('.') && !word.ends_with(".ts"));
    if inner_upper || structured {
      found.insert(word.to_owned());
    }
  }
  found.into_iter().collect()
}

fn code_span_names(task: &str) -> impl Iterator<Item = String> + '_ {
  task.split('`').skip(1).step_by(2).filter_map(|span| {
    let span = span.trim();
    let name = span.strip_suffix("()").unwrap_or(span);
    let identifier = name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '$')) && name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_');
    let shouted = name.chars().all(|c| c.is_uppercase() || !c.is_alphabetic());
    let code_like = name.starts_with(char::is_uppercase) || name.len() < span.len();
    (identifier && code_like && name.len() >= 3 && !shouted).then(|| name.to_owned())
  })
}

struct RouteMention {
  method: Option<HttpMethod>,
  route: String,
}

fn route_mentions(task: &str) -> Vec<RouteMention> {
  let words: Vec<&str> = task
    .split_whitespace()
    .map(|word| word.trim_matches(|c: char| matches!(c, ',' | ';' | '.' | ')' | '(' | '`' | '"' | '\'')))
    .collect();
  let mut mentions = Vec::new();
  for (index, word) in words.iter().enumerate() {
    if !word.starts_with('/') || word.len() < 2 {
      continue;
    }
    let method = index
      .checked_sub(1)
      .and_then(|previous| words.get(previous))
      .filter(|previous| previous.chars().all(|c| c.is_ascii_uppercase()))
      .and_then(|previous| HttpMethod::parse_lenient(previous));
    mentions.push(RouteMention {
      method,
      route: normalize_route(word),
    });
  }
  mentions
}

fn route_shape(route: &str) -> Vec<String> {
  normalize_route(route)
    .split('/')
    .filter(|segment| !segment.is_empty())
    .map(|segment| if segment.starts_with('{') { "{}".to_owned() } else { segment.to_owned() })
    .collect()
}

fn first_static(shape: &[String]) -> Option<&str> {
  shape.iter().map(String::as_str).find(|segment| *segment != "{}")
}

fn same_skeleton(left: &[String], right: &[String]) -> bool {
  left.len() == right.len() && left.iter().zip(right).all(|(left, right)| (left == "{}") == (right == "{}"))
}

fn only_method(endpoints: &[Entity], wanted: &[String]) -> Option<HttpMethod> {
  let mut methods = endpoints.iter().filter_map(|endpoint| match &endpoint.key {
    EntityKey::Endpoint { method, route, .. } if route_shape(route) == wanted => Some(*method),
    _ => None,
  });
  let method = methods.next()?;
  methods.next().is_none().then_some(method)
}

fn route_seeds(graph: &Graph<'_, '_>, task: &str, pack: &mut Pack) -> QueryResult<()> {
  let mentions = route_mentions(task);
  if mentions.is_empty() {
    return Ok(());
  }
  let endpoints = graph.entities_of_kind(&EntityKind::ApiEndpoint)?;
  let calls = graph.entities_of_kind(&EntityKind::ApiClientCall)?;
  for mention in &mentions {
    let wanted = route_shape(&mention.route);
    let resource = first_static(&wanted).map(str::to_owned);
    let mentioned = mention.method.or_else(|| only_method(&endpoints, &wanted));
    let mut named: Option<Node> = None;
    let mut siblings = 0;
    for endpoint in &endpoints {
      let EntityKey::Endpoint { method, route, .. } = &endpoint.key else { continue };
      if route_shape(route) == wanted && mentioned.is_some_and(|wanted| wanted == *method) {
        pack.seed(endpoint.clone(), 1.0, format!("the endpoint {method} {route} named in the task"), true);
        named = named.or_else(|| pack.nodes.get(&endpoint.id).cloned());
      }
    }
    for endpoint in &endpoints {
      let EntityKey::Endpoint { method, route, .. } = &endpoint.key else { continue };
      let shape = route_shape(route);
      let same_method = mentioned.is_some_and(|wanted| wanted == *method);
      let sibling = match (shape == wanted, same_method) {
        (true, true) => continue,
        (true, false) => {
          pack.seed(endpoint.clone(), 0.9, format!("same route as {} (another method)", mention.route), false);
          continue;
        }
        (false, _) if resource.is_some() && first_static(&shape) == resource.as_deref() => format!("same resource as {}", mention.route),
        (false, true) if same_skeleton(&shape, &wanted) => format!("pattern exemplar: {method} on a route shaped like {}", mention.route),
        (false, _) => continue,
      };
      match &named {
        Some(_) if siblings >= SIBLING_ROUTES => {}
        Some(named) => {
          siblings += 1;
          pack.mention(named, endpoint.clone(), SIBLING_ROUTE_WEIGHT, Role::Pattern, sibling, false);
        }
        None => pack.seed(endpoint.clone(), 0.6, sibling, false),
      }
    }
    let Some(resource) = resource else { continue };
    for call in &calls {
      let EntityKey::ClientCall { target, .. } = &call.key else { continue };
      let shape: Vec<String> = route_shape(&target.replace("{}", "{x}"));
      if first_static(&shape) != Some(resource.as_str()) {
        continue;
      }
      for edge in graph.edges_in(call.id, &[EdgeKind::MakesRequest])? {
        if crate::queries::api::site_linked(graph, call, &edge, edge.evidence.first())? {
          continue;
        }
        let Some(caller) = graph.entity(edge.from)? else { continue };
        let method = call.name.split(' ').next().unwrap_or("");
        let site = site_in(&caller, &edge);
        pack.insert(Node {
          entity: caller,
          basis: Basis::Relation,
          role: Role::Impact,
          weight: 0.5,
          uncertain: true,
          why: format!("possibly calls the same resource: {method} {target} [heuristic route-text match, unverified]"),
          via: None,
          site,
          terms: Vec::new(),
          round: 1,
          named: false,
          seed: false,
          near_named: false,
          reuse: None,
          hub_factor: 1.0,
        });
      }
    }
  }
  Ok(())
}

fn auth_seeds(graph: &Graph<'_, '_>, task: &str, pack: &mut Pack) -> QueryResult<()> {
  let lowered = task.to_ascii_lowercase();
  let schemes: BTreeSet<&str> = AUTH_WORDS.iter().filter(|(word, _)| lowered.contains(word)).map(|(_, scheme)| *scheme).collect();
  if schemes.is_empty() {
    return Ok(());
  }
  for scheme in graph.entities_of_kind(&EntityKind::AuthScheme)? {
    if schemes.contains(scheme.name.as_str()) {
      let why = format!("auth scheme {} named in the task", scheme.name);
      pack.seed(scheme, 0.8, why, false);
    }
  }
  Ok(())
}

fn expand(graph: &Graph<'_, '_>, index: &SimilarityIndex, pack: &mut Pack, depth: u32) -> QueryResult<()> {
  if depth > 0 {
    behaviour_sites(graph, pack)?;
  }
  let mut expanded: BTreeSet<EntityId> = BTreeSet::new();
  for round in 0..depth {
    loop {
      let frontier: Vec<EntityId> = pack
        .nodes
        .iter()
        .filter(|(id, node)| node.round == round && node.role == Role::Code && node.weight >= EXPAND_THRESHOLD && !expanded.contains(*id))
        .map(|(id, _)| *id)
        .collect();
      if frontier.is_empty() {
        break;
      }
      for id in frontier {
        expanded.insert(id);
        expand_node(graph, index, pack, id)?;
      }
    }
  }
  let mut structured = expanded;
  loop {
    let leaves: Vec<EntityId> = pack
      .nodes
      .iter()
      .filter(|(id, node)| node.role == Role::Code && node.weight >= EXPAND_THRESHOLD && !structured.contains(*id))
      .map(|(id, _)| *id)
      .collect();
    if leaves.is_empty() {
      return Ok(());
    }
    for id in leaves {
      structured.insert(id);
      let Some(node) = pack.nodes.get(&id).cloned() else { continue };
      expand_structure(graph, pack, &node)?;
    }
  }
}

fn behaviour_sites(graph: &Graph<'_, '_>, pack: &mut Pack) -> QueryResult<()> {
  use crate::policy::Direction::Reverse;
  let hits: Vec<Node> = pack
    .nodes
    .values()
    .filter(|node| node.seed && node.basis == Basis::SearchHit && node.weight >= EXPAND_THRESHOLD)
    .filter(|node| matches!(node.entity.kind, EntityKind::Function | EntityKind::Method))
    .cloned()
    .collect();
  let mut sites: BTreeMap<EntityId, Vec<(Node, Edge, Entity)>> = BTreeMap::new();
  for hit in hits {
    if production_users(graph, hit.entity.id, &[EdgeKind::Calls], BEHAVIOUR_CALLERS + 1)? > BEHAVIOUR_CALLERS {
      continue;
    }
    let callers: Vec<(Edge, Entity)> = neighbours(graph, pack, hit.entity.id, Reverse, &[EdgeKind::Calls])?
      .into_iter()
      .filter(|(edge, caller)| edge.certainty.is_authoritative() && caller.kind.is_code_symbol() && !is_test(caller) && !is_test_support(caller))
      .collect();
    for (edge, caller) in callers {
      sites.entry(caller.id).or_default().push((hit.clone(), edge, caller));
    }
  }
  for (_, applied) in sites.into_iter().filter(|(_, applied)| applied.len() >= BEHAVIOUR_HITS) {
    for (hit, edge, caller) in applied {
      pack.expand_to(graph, &hit, caller, &edge, 0.5, Role::Code)?;
    }
  }
  Ok(())
}

fn expand_structure(graph: &Graph<'_, '_>, pack: &mut Pack, node: &Node) -> QueryResult<()> {
  use crate::policy::Direction::Reverse;
  let id = node.entity.id;
  for (edge, parent) in neighbours(graph, pack, id, Reverse, &[EdgeKind::Contains])? {
    if parent.kind.is_code_symbol() {
      pack.expand_to(graph, node, parent, &edge, 0.6, Role::Code)?;
    }
  }
  if node.entity.kind == EntityKind::ApiEndpoint {
    for (edge, handler) in neighbours(graph, pack, id, Reverse, &[EdgeKind::ExposesEndpoint])? {
      let handler_id = handler.id;
      pack.implemented_by(graph, node, handler, &edge, 0.9)?;
      let handler = pack.nodes.get(&handler_id).filter(|_| node.named || node.near_named).cloned();
      if let Some(handler) = handler {
        serving::serving_path(graph, pack, &handler, &edge)?;
      }
    }
    for (edge, guard) in neighbours(graph, pack, id, crate::policy::Direction::Forward, &[EdgeKind::RequiresGuard])? {
      pack.implemented_by(graph, node, guard, &edge, 0.6)?;
    }
  }
  if node.entity.kind == EntityKind::AuthScheme {
    for (edge, guard) in neighbours(graph, pack, id, Reverse, &[EdgeKind::ImplementsAuth])? {
      pack.implemented_by(graph, node, guard, &edge, 1.0)?;
    }
  }
  Ok(())
}

fn neighbours(graph: &Graph<'_, '_>, pack: &mut Pack, id: EntityId, direction: crate::policy::Direction, kinds: &[EdgeKind]) -> QueryResult<Vec<(Edge, Entity)>> {
  let mut found = Vec::new();
  for edge in graph.admitted(id, direction, kinds, &mut pack.hidden)? {
    let other = match direction {
      crate::policy::Direction::Forward => edge.to,
      crate::policy::Direction::Reverse => edge.from,
    };
    if let Some(entity) = graph.entity(other)? {
      found.push((edge, entity));
    }
  }
  Ok(found)
}

fn expand_node(graph: &Graph<'_, '_>, index: &SimilarityIndex, pack: &mut Pack, id: EntityId) -> QueryResult<()> {
  use crate::policy::Direction::{Forward, Reverse};
  let Some(node) = pack.nodes.get(&id).cloned() else { return Ok(()) };
  let name = short_name(&node.entity.key);
  expand_structure(graph, pack, &node)?;
  if is_type_like(&node.entity.kind) {
    let members: Vec<(Edge, Entity)> = neighbours(graph, pack, id, Forward, &[EdgeKind::Contains])?
      .into_iter()
      .filter(|(_, child)| child.kind == EntityKind::Method)
      .take(MEMBERS_PER_TYPE)
      .collect();
    let factor = if node.seed { 0.6 } else { 0.3 };
    for (edge, child) in members {
      pack.expand_to(graph, &node, child, &edge, factor, Role::Code)?;
    }
  }
  for (edge, callee) in neighbours(graph, pack, id, Forward, &[EdgeKind::Calls, EdgeKind::Instantiates])? {
    let hub = hub_factor(graph, callee.id)?;
    pack.expand_callee(graph, &node, callee, &edge, hub)?;
  }
  if node.round == 0 && node.named {
    for (edge, caller) in neighbours(graph, pack, id, Reverse, &[EdgeKind::Calls, EdgeKind::Instantiates, EdgeKind::References])? {
      pack.expand_to(graph, &node, caller, &edge, 0.5, Role::Impact)?;
    }
  }
  for (edge, other) in neighbours(graph, pack, id, Forward, &[EdgeKind::Implements, EdgeKind::Extends])? {
    pack.expand_to(graph, &node, other, &edge, 0.4, Role::Code)?;
  }
  for (edge, other) in neighbours(graph, pack, id, Reverse, &[EdgeKind::Implements, EdgeKind::Extends])? {
    pack.expand_to(graph, &node, other, &edge, 0.4, Role::Code)?;
  }
  for (edge, endpoint) in neighbours(graph, pack, id, Forward, &[EdgeKind::ExposesEndpoint])? {
    pack.expand_to(graph, &node, endpoint, &edge, 0.7, Role::Code)?;
  }
  if node.entity.kind == EntityKind::ApiEndpoint {
    for (edge, scheme) in neighbours(graph, pack, id, Forward, &[EdgeKind::SupportsAuth])? {
      pack.expand_to(graph, &node, scheme, &edge, 0.6, Role::Code)?;
    }
    for (edge, consumer) in neighbours(graph, pack, id, Reverse, &[EdgeKind::CallsEndpoint])? {
      let consumers = match consumer.kind {
        EntityKind::ApiClientCall => neighbours(graph, pack, consumer.id, Reverse, &[EdgeKind::MakesRequest])?
          .into_iter()
          .map(|(_, caller)| caller)
          .collect(),
        _ => vec![consumer],
      };
      for caller in consumers {
        pack.expand_to(graph, &node, caller, &edge, 0.6, Role::Impact)?;
      }
    }
  }
  if node.entity.kind == EntityKind::AuthScheme {
    for (edge, client) in neighbours(graph, pack, id, Reverse, &[EdgeKind::UsesAuth])? {
      pack.expand_to(graph, &node, client, &edge, 0.3, Role::Impact)?;
    }
  }
  if node.seed {
    for (edge, endpoint) in neighbours(graph, pack, id, Reverse, &[EdgeKind::RequiresGuard, EdgeKind::SupportsAuth])? {
      pack.expand_to(graph, &node, endpoint, &edge, 0.5, Role::Code)?;
    }
  }
  for (edge, read) in neighbours(graph, pack, id, Forward, &[EdgeKind::ReadsEnv, EdgeKind::ReadsConfig])? {
    pack.expand_to(graph, &node, read, &edge, 0.3, Role::Code)?;
  }
  if matches!(node.entity.kind, EntityKind::Function | EntityKind::Method) && node.round == 0 {
    let mut kept = 0;
    for found in index.near_duplicates(id, SIMILAR_THRESHOLD, SIMILAR_CANDIDATES) {
      if kept == SIMILAR_PER_NODE {
        break;
      }
      let Some(entity) = graph.entity(found.entity)? else { continue };
      let why = if found.small {
        let corroborated = names_related(&node.entity.name, &entity.name) || pack.nodes.get(&found.entity).is_some_and(|other| other.seed);
        if !corroborated {
          continue;
        }
        format!("similar small body to {name} [structural {:.2}, small body, name-corroborated]", found.exact_jaccard)
      } else {
        format!("same shape as {name} [structural {:.2}]", found.exact_jaccard)
      };
      let why = format!("{why}; {}; behaviour unverified", access(graph, &entity, Some(&node.entity))?);
      pack.mention(&node, entity, 0.3, Role::Reuse, why, false);
      kept += 1;
    }
  }
  Ok(())
}

fn hub_factor(graph: &Graph<'_, '_>, id: EntityId) -> QueryResult<f64> {
  let users = production_users(graph, id, &[EdgeKind::Calls, EdgeKind::Instantiates, EdgeKind::References], HUB_USERS_COUNTED)?;
  Ok(match users <= HUB_FAN_IN {
    true => 1.0,
    false => (HUB_FAN_IN as f64 / users as f64).sqrt(),
  })
}

fn production_users(graph: &Graph<'_, '_>, id: EntityId, kinds: &[EdgeKind], cap: usize) -> QueryResult<usize> {
  production_users_where(graph, &[id], kinds, cap, |_| true)
}

fn production_users_where(graph: &Graph<'_, '_>, ids: &[EntityId], kinds: &[EdgeKind], cap: usize, counted: impl Fn(&Entity) -> bool) -> QueryResult<usize> {
  let mut users: BTreeSet<EntityId> = BTreeSet::new();
  let mut examined: BTreeSet<EntityId> = BTreeSet::new();
  let mut links = Vec::new();
  for id in ids {
    links.extend(graph.links_in(*id, kinds)?);
  }
  for link in links {
    if users.len() >= cap {
      break;
    }
    if users.contains(&link.from) {
      continue;
    }
    if examined.insert(link.from) && examined.len() > USERS_EXAMINED {
      return Ok(cap);
    }
    let Some(user) = graph.entity(link.from)? else { continue };
    if is_test(&user) || is_test_support(&user) || !counted(&user) || graph.edge(link.id)?.is_some_and(|edge| is_mock_edge(&edge)) {
      continue;
    }
    users.insert(link.from);
  }
  Ok(users.len())
}

fn name_suffix(name: &str) -> Option<&str> {
  let start = name
    .char_indices()
    .filter(|(index, c)| *index > 0 && c.is_uppercase())
    .map(|(index, _)| index)
    .next_back()?;
  Some(&name[start..])
}

fn anchors(graph: &Graph<'_, '_>, pack: &Pack) -> QueryResult<Vec<Node>> {
  let mut seeds: Vec<&Node> = pack.nodes.values().filter(|node| node.seed && node.role == Role::Code).collect();
  seeds.sort_by(|left, right| right.weight.total_cmp(&left.weight).then_with(|| left.entity.key.cmp(&right.entity.key)));
  let mut anchors: Vec<Node> = Vec::new();
  for seed in seeds {
    if anchors.len() >= PATTERN_ANCHORS {
      break;
    }
    let anchor = match is_type_like(&seed.entity.kind) {
      true => Some(seed.clone()),
      false => graph
        .parent(seed.entity.id)?
        .filter(|parent| is_type_like(&parent.kind))
        .map(|parent| Node { entity: parent, ..seed.clone() }),
    };
    if let Some(anchor) = anchor.filter(|anchor| !anchors.iter().any(|existing| existing.entity.id == anchor.entity.id)) {
      anchors.push(anchor);
    }
  }
  Ok(anchors)
}

fn exemplars(graph: &Graph<'_, '_>, pack: &mut Pack) -> QueryResult<()> {
  use crate::policy::Direction::{Forward, Reverse};
  for anchor in anchors(graph, pack)? {
    let name = short_name(&anchor.entity.key);
    for (edge, contract) in neighbours(graph, pack, anchor.entity.id, Forward, &[EdgeKind::Implements, EdgeKind::Extends])? {
      let contract_name = short_name(&contract.key);
      let siblings: Vec<(Edge, Entity)> = neighbours(graph, pack, contract.id, Reverse, std::slice::from_ref(&edge.kind))?
        .into_iter()
        .filter(|(_, sibling)| sibling.id != anchor.entity.id)
        .take(PATTERN_SIBLINGS)
        .collect();
      pack.expand_to(graph, &anchor, contract, &edge, 0.8, Role::Pattern)?;
      for (sibling_edge, sibling) in siblings {
        let why = format!(
          "sibling of {name}: {} {contract_name} {}",
          hedge(&sibling_edge.kind.forward_label(), &sibling_edge.certainty),
          sibling_edge.certainty.tag()
        );
        pack.mention(&anchor, sibling, 0.6, Role::Pattern, why, !sibling_edge.certainty.is_authoritative());
      }
    }
    let registered = registry_exemplars(graph, pack, &anchor)?;
    macro_exemplars(graph, pack, &anchor)?;
    if registered {
      suffix_contracts(graph, pack, &anchor)?;
    }
  }
  Ok(())
}

fn suffix_contracts(graph: &Graph<'_, '_>, pack: &mut Pack, anchor: &Node) -> QueryResult<()> {
  let Some(suffix) = name_suffix(&anchor.entity.name) else { return Ok(()) };
  let declared: Vec<EntityId> = graph
    .edges_out(anchor.entity.id, &[EdgeKind::Implements, EdgeKind::Extends])?
    .iter()
    .map(|edge| edge.to)
    .collect();
  for contract in graph.tx().entities_by_name(suffix, &atlas_store::EntityFilter::default())? {
    if !matches!(contract.kind, EntityKind::Trait | EntityKind::Interface) || declared.contains(&contract.id) {
      continue;
    }
    let why = format!(
      "possibly the contract {} and its siblings implement [heuristic name-suffix match, no implements edge indexed]",
      short_name(&anchor.entity.key)
    );
    pack.mention(anchor, contract, 0.6, Role::Pattern, why, true);
  }
  Ok(())
}

fn registry_exemplars(graph: &Graph<'_, '_>, pack: &mut Pack, anchor: &Node) -> QueryResult<bool> {
  use crate::policy::Direction::{Forward, Reverse};
  let Some(suffix) = name_suffix(&anchor.entity.name).map(str::to_owned) else {
    return Ok(false);
  };
  let mut registered = false;
  for (edge, registry) in neighbours(graph, pack, anchor.entity.id, Reverse, &[EdgeKind::References, EdgeKind::Instantiates, EdgeKind::Calls])? {
    if is_test(&registry) || !registry.kind.is_code_symbol() {
      continue;
    }
    let siblings: Vec<Entity> = neighbours(graph, pack, registry.id, Forward, &[EdgeKind::References, EdgeKind::Instantiates, EdgeKind::Calls])?
      .into_iter()
      .map(|(_, sibling)| sibling)
      .filter(|sibling| sibling.id != anchor.entity.id && sibling.kind == anchor.entity.kind && name_suffix(&sibling.name) == Some(suffix.as_str()))
      .collect();
    if siblings.len() < REGISTRY_MIN_SIBLINGS {
      continue;
    }
    let count = siblings.len();
    registered = true;
    pack.expand_to(graph, anchor, registry.clone(), &edge, 0.7, Role::Pattern)?;
    if let Some(node) = pack.nodes.get_mut(&registry.id) {
      node.why = format!("registry: references {} and {count} siblings {}", short_name(&anchor.entity.key), edge.certainty.tag());
    }
    for sibling in siblings.into_iter().take(PATTERN_SIBLINGS) {
      let why = format!("sibling of {} registered in {}", short_name(&anchor.entity.key), short_name(&registry.key));
      pack.mention(anchor, sibling, 0.5, Role::Pattern, why, false);
    }
  }
  Ok(registered)
}

fn macro_exemplars(graph: &Graph<'_, '_>, pack: &mut Pack, anchor: &Node) -> QueryResult<()> {
  use crate::policy::Direction::{Forward, Reverse};
  let Some(file) = crate::queries::common::file_entity(&anchor.entity) else {
    return Ok(());
  };
  for (_, registration) in neighbours(graph, pack, EntityId::of(&file), Forward, &[EdgeKind::References])? {
    if registration.kind != EntityKind::Macro {
      continue;
    }
    let users = neighbours(graph, pack, registration.id, Reverse, &[EdgeKind::References])?.len();
    if users <= REGISTRY_MIN_SIBLINGS {
      continue;
    }
    let why = format!(
      "possibly a registration macro (used by {} files, including {}) [heuristic shared-macro usage]",
      users,
      file.display_name()
    );
    pack.mention(anchor, registration, 0.7, Role::Pattern, why, true);
  }
  Ok(())
}

fn rank(pack: &mut Pack) -> Vec<Ranked> {
  let ids: Vec<EntityId> = pack.nodes.keys().copied().collect();
  let position: BTreeMap<EntityId, usize> = ids.iter().enumerate().map(|(index, id)| (*id, index)).collect();
  let mut adjacency: Vec<Vec<(usize, f64)>> = vec![Vec::new(); ids.len()];
  for (from, to, weight) in &pack.links {
    let (Some(&from), Some(&to)) = (position.get(from), position.get(to)) else { continue };
    if from == to {
      continue;
    }
    adjacency[from].push((to, *weight));
    adjacency[to].push((from, *weight));
  }
  let mut personal = vec![0.0; ids.len()];
  for (id, weight) in &pack.personalisation {
    if let Some(&index) = position.get(id) {
      personal[index] = *weight;
    }
  }
  let total: f64 = personal.iter().sum();
  match total > 0.0 {
    true => personal.iter_mut().for_each(|value| *value /= total),
    false => personal.iter_mut().for_each(|value| *value = 1.0 / ids.len().max(1) as f64),
  }
  let out_weight: Vec<f64> = adjacency.iter().map(|links| links.iter().map(|(_, weight)| weight).sum()).collect();
  let mut scores = personal.clone();
  for _ in 0..PAGERANK_ITERATIONS {
    let mut next: Vec<f64> = personal.iter().map(|value| (1.0 - DAMPING) * value).collect();
    let mut dangling = 0.0;
    for (index, links) in adjacency.iter().enumerate() {
      if out_weight[index] <= 0.0 {
        dangling += scores[index];
        continue;
      }
      for (neighbour, weight) in links {
        next[*neighbour] += DAMPING * scores[index] * weight / out_weight[index];
      }
    }
    for (index, value) in next.iter_mut().enumerate() {
      *value += DAMPING * dangling * personal[index];
    }
    scores = next;
  }
  let mut ranked: Vec<Ranked> = std::mem::take(&mut pack.nodes)
    .into_values()
    .map(|node| {
      let score = position.get(&node.entity.id).map_or(0.0, |&index| scores[index]) * node.hub_factor;
      Ranked {
        score: (score * 1_000_000.0).round() / 1_000_000.0,
        node,
        skeleton: Vec::new(),
        tests: Vec::new(),
      }
    })
    .collect();
  ranked.sort_by(|left, right| right.score.total_cmp(&left.score).then_with(|| left.node.entity.key.cmp(&right.node.entity.key)));
  ranked
}

#[cfg(test)]
mod tests {
  use atlas_core::Certainty;

  use super::*;

  fn node(basis: Basis, role: Role, uncertain: bool, why: &str, weight: f64) -> Node {
    let key: EntityKey = "ts:acme-sdk:acme-sdk:src/client::AcmeClient::getOrder#method".parse().unwrap();
    Node {
      entity: Entity::new(key, Certainty::Syntactic),
      basis,
      role,
      weight,
      uncertain,
      why: why.to_owned(),
      via: None,
      site: None,
      terms: Vec::new(),
      round: 1,
      named: false,
      seed: basis == Basis::SearchHit,
      near_named: false,
      reuse: None,
      hub_factor: 1.0,
    }
  }

  fn placed(first: Node, second: Node) -> (Role, bool, String, f64) {
    let mut pack = Pack::default();
    pack.insert(first);
    pack.insert(second);
    let node = pack.nodes.into_values().next().unwrap();
    (node.role, node.uncertain, node.why, node.weight)
  }

  #[test]
  fn should_read_capitalised_and_called_code_spans_as_identifiers() {
    let task = "add `deletedAt` to the `User` entity, keep `serialize()` and `target`, `GET`, `Accept-Encoding`, `id` and plain User untouched";

    assert_eq!(identifiers(task), ["User", "deletedAt", "serialize"]);
  }

  #[test]
  fn should_keep_an_impact_placement_when_a_stronger_search_hit_arrives_later() {
    let impact = node(Basis::Relation, Role::Impact, true, "possibly calls endpoint GET /orders/{id} [heuristic 0.77]", 0.3);
    let search = node(Basis::SearchHit, Role::Code, false, "search hit on orders", 0.5);

    assert_eq!(
      placed(impact, search),
      (Role::Impact, true, "possibly calls endpoint GET /orders/{id} [heuristic 0.77]".to_owned(), 0.5)
    );
  }

  #[test]
  fn should_move_an_earlier_search_hit_into_the_impact_placement() {
    let search = node(Basis::SearchHit, Role::Code, false, "search hit on orders", 0.5);
    let impact = node(Basis::Relation, Role::Impact, true, "possibly calls endpoint GET /orders/{id} [heuristic 0.77]", 0.3);

    assert_eq!(
      placed(search, impact),
      (Role::Impact, true, "possibly calls endpoint GET /orders/{id} [heuristic 0.77]".to_owned(), 0.5)
    );
  }

  #[test]
  fn should_move_an_earlier_search_hit_into_the_uncertain_placement() {
    let search = node(Basis::SearchHit, Role::Code, false, "search hit on orders", 0.5);
    let heuristic = node(Basis::Relation, Role::Code, true, "possibly calls getOrder [heuristic 0.6]", 0.2);

    assert_eq!(placed(search, heuristic), (Role::Code, true, "possibly calls getOrder [heuristic 0.6]".to_owned(), 0.5));
  }

  #[test]
  fn should_let_a_firm_relation_replace_an_uncertain_one_regardless_of_search_hits() {
    let mut pack = Pack::default();
    pack.insert(node(Basis::SearchHit, Role::Code, false, "search hit on orders", 0.5));
    pack.insert(node(Basis::Relation, Role::Code, true, "possibly calls getOrder [heuristic 0.6]", 0.2));
    pack.insert(node(Basis::Relation, Role::Code, false, "called by OrdersController.findOne [resolved]", 0.1));
    let node = pack.nodes.into_values().next().unwrap();

    assert_eq!(
      (node.role, node.uncertain, node.why.as_str()),
      (Role::Code, false, "called by OrdersController.findOne [resolved]")
    );
  }
}
