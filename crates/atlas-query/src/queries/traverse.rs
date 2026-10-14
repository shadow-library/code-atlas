use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{Certainty, Edge, EdgeId, EdgeKind, Entity, EntityId, EntityKind};
use atlas_semantic::RELATION_CALLS_ENDPOINT;
use atlas_store::{FreshnessPolicy, InferredQuery};
use serde::Serialize;

use crate::QueryEngine;
use crate::dynamic::dynamic_member_calls;
use crate::engine::{macro_generated_refs, unresolved_risk};
use crate::error::QueryResult;
use crate::graph::{Bridge, Graph, Reached, Step, Walk, WalkSpec, certainty_value, group_by_repo};
use crate::model::{Caveat, CertaintyTag, EntityRef, Hop, MacroGapScope, Response, Targeted, short_name};
use crate::policy::{Direction, Policy};
use crate::queries::common::{DEFAULT_LIMIT, MAX_NODES, file_symbols, link_caveat, resolve_one};
use crate::queries::inferred_links::{LinkEnd, LinkScan, inferred_links};
use crate::target::TargetFilter;

const DEFAULT_FLOOR: f32 = 0.25;
const HTTP_HOP_KIND: &str = "via_http";
const NAMED_ENDPOINTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraversalKind {
  Callers,
  Callees,
  Dependents,
  Dependencies,
}

impl TraversalKind {
  fn direction(self) -> Direction {
    match self {
      Self::Callers | Self::Dependents => Direction::Reverse,
      Self::Callees | Self::Dependencies => Direction::Forward,
    }
  }

  fn default_kinds(self) -> Vec<EdgeKind> {
    match self {
      Self::Callers | Self::Callees => vec![EdgeKind::Calls, EdgeKind::Instantiates],
      Self::Dependents | Self::Dependencies => usage_kinds(),
    }
  }

  pub fn as_str(self) -> &'static str {
    match self {
      Self::Callers => "callers",
      Self::Callees => "callees",
      Self::Dependents => "dependents",
      Self::Dependencies => "dependencies",
    }
  }
}

pub(crate) fn usage_kinds() -> Vec<EdgeKind> {
  vec![
    EdgeKind::Calls,
    EdgeKind::References,
    EdgeKind::Instantiates,
    EdgeKind::TypeOf,
    EdgeKind::Implements,
    EdgeKind::Extends,
    EdgeKind::Imports,
    EdgeKind::ReExports,
    EdgeKind::DependsOnPackage,
    EdgeKind::CallsEndpoint,
    EdgeKind::MakesRequest,
    EdgeKind::UsesAuth,
    EdgeKind::SupportsAuth,
    EdgeKind::ReadsEnv,
    EdgeKind::ReadsConfig,
    EdgeKind::Publishes,
    EdgeKind::Subscribes,
    EdgeKind::ReadsTable,
    EdgeKind::WritesTable,
    EdgeKind::Tests,
  ]
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraverseRequest {
  pub target: String,
  pub filter: TargetFilter,
  pub depth: u32,
  pub kinds: Vec<EdgeKind>,
  pub limit: usize,
  pub min_confidence: Option<f32>,
  pub max_nodes: usize,
  pub via_http: bool,
}

impl TraverseRequest {
  pub fn new(target: impl Into<String>) -> Self {
    Self {
      target: target.into(),
      filter: TargetFilter::default(),
      depth: 1,
      kinds: Vec::new(),
      limit: DEFAULT_LIMIT,
      min_confidence: None,
      max_nodes: MAX_NODES,
      via_http: false,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraversalItem {
  pub entity: EntityRef,
  pub depth: u32,
  pub relation: Hop,
  pub path: Vec<Hop>,
  pub uncertain: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RepoGroup {
  pub repo: String,
  pub total: usize,
  pub items: Vec<TraversalItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraversalAnswer {
  pub kind: TraversalKind,
  pub target: EntityRef,
  pub depth: u32,
  pub edge_kinds: Vec<String>,
  #[serde(skip_serializing_if = "std::ops::Not::not")]
  pub via_http: bool,
  pub total: usize,
  pub shown: usize,
  pub groups: Vec<RepoGroup>,
  pub aggregated_symbols: Option<usize>,
  pub caveats: Vec<Caveat>,
}

impl QueryEngine {
  pub fn callers(&self, request: &TraverseRequest) -> QueryResult<Response<Targeted<TraversalAnswer>>> {
    self.traverse(TraversalKind::Callers, request)
  }

  pub fn callees(&self, request: &TraverseRequest) -> QueryResult<Response<Targeted<TraversalAnswer>>> {
    self.traverse(TraversalKind::Callees, request)
  }

  pub fn dependents(&self, request: &TraverseRequest) -> QueryResult<Response<Targeted<TraversalAnswer>>> {
    self.traverse(TraversalKind::Dependents, request)
  }

  pub fn dependencies(&self, request: &TraverseRequest) -> QueryResult<Response<Targeted<TraversalAnswer>>> {
    self.traverse(TraversalKind::Dependencies, request)
  }

  pub fn traverse(&self, kind: TraversalKind, request: &TraverseRequest) -> QueryResult<Response<Targeted<TraversalAnswer>>> {
    let freshness = self.barrier()?;
    let policy = Policy::with_floor(request.min_confidence.unwrap_or(DEFAULT_FLOOR));
    let answer = self.read(policy, |graph| -> QueryResult<Targeted<TraversalAnswer>> {
      let target = match resolve_one(self, graph, &request.target, &request.filter)? {
        Ok(target) => target,
        Err(outcome) => return Ok(outcome),
      };
      traversal(graph, kind, request, &target).map(Targeted::Found)
    })?;
    let answer = self.with_problem_files(answer, freshness.run, |answer| &mut answer.caveats)?;
    Ok(Response { freshness, answer })
  }
}

fn traversal(graph: &Graph<'_, '_>, kind: TraversalKind, request: &TraverseRequest, target: &Entity) -> QueryResult<TraversalAnswer> {
  let kinds = match request.kinds.is_empty() {
    true => kind.default_kinds(),
    false => request.kinds.clone(),
  };
  let steps: Vec<Step> = kinds.iter().map(|edge| Step::new(edge.clone(), kind.direction(), 1.0)).collect();
  let depth = request.depth.max(1);
  let aggregated = file_symbols(graph, target)?;
  let mut seeds = vec![target.id];
  seeds.extend(aggregated.iter().flatten().copied());
  let spec = WalkSpec {
    seeds: &seeds,
    steps: &steps,
    max_depth: depth,
    max_nodes: request.max_nodes,
  };
  let mut http = HttpBridges::new(graph, kind.direction(), &steps);
  let walk = match request.via_http {
    true => graph.walk_bridged(&spec, |_| true, |node| http.bridges(node))?,
    false => graph.walk(&spec, |_| true)?,
  };
  let mut items = Vec::with_capacity(walk.reached.len());
  for reached in &walk.reached {
    if let Some(item) = item(graph, reached)? {
      items.push(item);
    }
  }
  items.sort_by_key(|item| (item.depth, item.uncertain, item.entity.sort_key()));
  let total = items.len();
  let mut totals: BTreeMap<String, usize> = BTreeMap::new();
  for item in &items {
    *totals.entry(item.entity.repo().to_owned()).or_default() += 1;
  }
  items.truncate(request.limit);
  let shown = items.len();
  let groups = group_by_repo(items, |item| item.entity.repo().to_owned())
    .into_iter()
    .map(|(repo, items)| RepoGroup {
      total: totals.get(&repo).copied().unwrap_or(items.len()),
      repo,
      items,
    })
    .collect();
  let mut caveats = Vec::new();
  if walk.truncated {
    caveats.push(Caveat::Truncated { max_nodes: request.max_nodes });
  }
  caveats.extend(http.links.inferred_hidden());
  let hidden = walk.hidden_below_floor + http.hidden.len() + http.links.below_floor();
  if hidden > 0 {
    caveats.push(Caveat::BelowConfidenceFloor {
      hidden,
      fallback_tests: 0,
      floor: graph.policy.min_confidence,
    });
  }
  if request.via_http {
    caveats.extend(http.none_found(&walk, depth)?);
  }
  if kind.direction() == Direction::Reverse {
    caveats.extend(unresolved_risk(graph, target.id, risk_name(target))?);
    caveats.extend(dynamic_member_calls(graph, target)?);
    caveats.extend(dispatch_caveat(graph, target)?);
  } else {
    let scope = match kind {
      TraversalKind::Dependencies => MacroGapScope::Dependencies,
      _ => MacroGapScope::Callees,
    };
    caveats.extend(macro_generated_refs(graph, target, &seeds, scope)?);
  }
  Ok(TraversalAnswer {
    kind,
    target: EntityRef::of(target),
    depth,
    edge_kinds: kinds.iter().map(|kind| kind.as_str().to_owned()).collect(),
    via_http: request.via_http,
    total,
    shown,
    groups,
    aggregated_symbols: aggregated.map(|symbols| symbols.len()),
    caveats,
  })
}

pub(crate) fn risk_name(target: &Entity) -> &str {
  match target.key.as_symbol() {
    Some(_) => target.name.as_str(),
    None => "",
  }
}

fn item(graph: &Graph<'_, '_>, reached: &Reached) -> QueryResult<Option<TraversalItem>> {
  let Some(entity) = graph.entity(reached.id)? else { return Ok(None) };
  let Some(last) = reached.last_hop() else { return Ok(None) };
  let relation = graph.hop(last)?;
  let path = match reached.depth > 1 {
    true => graph.path(reached)?,
    false => Vec::new(),
  };
  Ok(Some(TraversalItem {
    entity: EntityRef::of(&entity),
    depth: reached.depth,
    relation,
    path,
    uncertain: reached.is_uncertain(),
  }))
}

pub(crate) fn dispatch_caveat(graph: &Graph<'_, '_>, target: &Entity) -> QueryResult<Option<Caveat>> {
  let mut callers = 0;
  let mut traits = Vec::new();
  let mut declarations: Vec<Entity> = Vec::new();
  for edge in graph.edges_out(target.id, &[EdgeKind::Implements])? {
    declarations.extend(graph.entity(edge.to)?);
  }
  if target.kind == EntityKind::Method
    && let Some(owner) = graph.parent(target.id)?
  {
    for edge in graph.edges_out(owner.id, &[EdgeKind::Extends, EdgeKind::Implements])? {
      declarations.extend(
        graph
          .children(edge.to)?
          .into_iter()
          .filter(|member| member.name == target.name && member.kind == EntityKind::Method),
      );
    }
  }
  declarations.sort_by(|left, right| left.key.cmp(&right.key));
  declarations.dedup_by(|left, right| left.id == right.id);
  for declaration in declarations {
    let count = graph.edges_in(declaration.id, &[EdgeKind::Calls])?.len();
    if count > 0 {
      callers += count;
      traits.push(crate::model::short_name(&declaration.key));
    }
  }
  if callers == 0 {
    return Ok(None);
  }
  Ok(Some(Caveat::Note {
    message: format!(
      "{callers} callers of the implemented or overridden declaration {} may dispatch here dynamically (run callers on it)",
      traits.join(", ")
    ),
  }))
}

struct HttpBridges<'g, 'a, 'c> {
  graph: &'g Graph<'a, 'c>,
  direction: Direction,
  walked: Vec<(EdgeKind, Direction)>,
  links: LinkScan,
  hidden: BTreeSet<EdgeId>,
  hidden_by_walk: BTreeSet<EdgeId>,
  crossed: bool,
  endpoints: BTreeSet<EntityId>,
  client_calls: BTreeSet<EntityId>,
}

struct Crossing<'e> {
  link: Edge,
  tag: CertaintyTag,
  hop_edge: String,
  endpoint: &'e Entity,
}

impl<'g, 'a, 'c> HttpBridges<'g, 'a, 'c> {
  fn new(graph: &'g Graph<'a, 'c>, direction: Direction, steps: &[Step]) -> Self {
    Self {
      graph,
      direction,
      walked: steps.iter().map(|step| (step.kind.clone(), step.direction)).collect(),
      links: LinkScan::default(),
      hidden: BTreeSet::new(),
      hidden_by_walk: BTreeSet::new(),
      crossed: false,
      endpoints: BTreeSet::new(),
      client_calls: BTreeSet::new(),
    }
  }

  fn bridges(&mut self, node: EntityId) -> QueryResult<Vec<Bridge>> {
    let Some(entity) = self.graph.entity(node)? else { return Ok(Vec::new()) };
    let bridges = match self.direction {
      Direction::Reverse => self.clients_of(&entity)?,
      Direction::Forward => self.handlers_of(&entity)?,
    };
    self.crossed |= !bridges.is_empty();
    Ok(bridges)
  }

  fn clients_of(&mut self, handler: &Entity) -> QueryResult<Vec<Bridge>> {
    let graph = self.graph;
    let mut exposed: Vec<(Entity, Option<Certainty>)> = Vec::new();
    match handler.kind {
      EntityKind::ApiEndpoint => exposed.push((handler.clone(), None)),
      _ => {
        for exposes in self.admitted(handler.id, Direction::Forward, EdgeKind::ExposesEndpoint, Origin::Node)? {
          exposed.extend(graph.entity(exposes.to)?.map(|endpoint| (endpoint, Some(exposes.certainty))));
        }
      }
    }
    let mut bridges = Vec::new();
    for (endpoint, exposes) in &exposed {
      self.endpoints.insert(endpoint.id);
      for link in self.admitted(endpoint.id, Direction::Reverse, EdgeKind::CallsEndpoint, origin_of(handler, endpoint))? {
        let client = link.from;
        let crossing = Crossing::of_edge(link, endpoint);
        bridges.push(self.bridge(client, handler, &crossing, exposes.as_slice()));
      }
      for inferred in inferred_links(graph, endpoint.id, LinkEnd::Endpoint, &mut self.links)? {
        let crossing = Crossing::of_inferred(inferred.edge, inferred.hop, endpoint);
        for request in self.admitted(inferred.call.id, Direction::Reverse, EdgeKind::MakesRequest, Origin::Beyond)? {
          let others: Vec<Certainty> = exposes.iter().cloned().chain([request.certainty]).collect();
          bridges.push(self.bridge(request.from, handler, &crossing, &others));
        }
      }
    }
    Ok(bridges)
  }

  fn handlers_of(&mut self, client: &Entity) -> QueryResult<Vec<Bridge>> {
    let graph = self.graph;
    let mut bridges = Vec::new();
    for link in self.admitted(client.id, Direction::Forward, EdgeKind::CallsEndpoint, Origin::Node)? {
      let Some(endpoint) = graph.entity(link.to)? else { continue };
      let crossing = Crossing::of_edge(link, &endpoint);
      for exposes in self.admitted(endpoint.id, Direction::Reverse, EdgeKind::ExposesEndpoint, Origin::Beyond)? {
        bridges.push(self.bridge(exposes.from, client, &crossing, &[exposes.certainty]));
      }
    }
    for request in self.admitted(client.id, Direction::Forward, EdgeKind::MakesRequest, Origin::Node)? {
      self.client_calls.insert(request.to);
      for inferred in inferred_links(graph, request.to, LinkEnd::Call, &mut self.links)? {
        let crossing = Crossing::of_inferred(inferred.edge, inferred.hop, &inferred.endpoint);
        for exposes in self.admitted(inferred.endpoint.id, Direction::Reverse, EdgeKind::ExposesEndpoint, Origin::Beyond)? {
          bridges.push(self.bridge(exposes.from, client, &crossing, &[request.certainty.clone(), exposes.certainty]));
        }
      }
    }
    Ok(bridges)
  }

  fn admitted(&mut self, id: EntityId, direction: Direction, kind: EdgeKind, origin: Origin) -> QueryResult<Vec<Edge>> {
    let graph = self.graph;
    let walked = origin == Origin::Node && self.walked.contains(&(kind.clone(), direction));
    let hidden = match walked {
      true => &mut self.hidden_by_walk,
      false => &mut self.hidden,
    };
    graph.admitted_distinct(id, direction, &[kind], hidden)
  }

  fn bridge(&self, to: EntityId, from: &Entity, crossing: &Crossing<'_>, others: &[Certainty]) -> Bridge {
    let mut weakest = (&crossing.link.certainty, crossing.tag.clone());
    for other in others {
      if other.cmp_strength(weakest.0) == Ordering::Less {
        weakest = (other, CertaintyTag::of(other));
      }
    }
    let route = short_name(&crossing.endpoint.key);
    let (phrase, entity) = match (from.id == crossing.endpoint.id, self.direction) {
      (true, _) => ("via HTTP".to_owned(), route),
      (false, Direction::Reverse) => (format!("via HTTP {route} to"), short_name(&from.key)),
      (false, Direction::Forward) => (format!("via HTTP {route} from"), short_name(&from.key)),
    };
    Bridge {
      to,
      edge: crossing.link.clone(),
      hop: Hop {
        edge: crossing.hop_edge.clone(),
        kind: HTTP_HOP_KIND.to_owned(),
        phrase,
        entity,
        certainty: weakest.1,
      },
      value: others.iter().map(certainty_value).product::<f64>() * certainty_value(&crossing.link.certainty),
    }
  }

  fn none_found(&self, walk: &Walk, max_depth: u32) -> QueryResult<Option<Caveat>> {
    if self.crossed {
      return Ok(None);
    }
    let (beyond, probe) = self.beyond_depth(walk, max_depth)?;
    if let Some(beyond) = beyond {
      return Ok(Some(Caveat::Note {
        message: format!("via_http: HTTP hop beyond depth {max_depth} ({beyond}); raise depth to follow it"),
      }));
    }
    let hidden: BTreeSet<&EdgeId> = self.hidden.iter().chain(&self.hidden_by_walk).chain(&probe.hidden).collect();
    let below_floor = hidden.len() + self.links.below_floor() + probe.links.below_floor();
    if below_floor > 0 {
      return Ok(Some(Caveat::Note {
        message: format!(
          "via_http found no HTTP hop above the confidence floor: {} on HTTP paths {} below {:.2} (lower min_confidence to follow them)",
          crate::text::counted(below_floor, "link"),
          if below_floor == 1 { "is" } else { "are" },
          self.graph.policy.min_confidence
        ),
      }));
    }
    if self.links.inferred_hidden().is_some() {
      return Ok(None);
    }
    if probe.links.inferred_hidden().is_some() {
      return Ok(Some(Caveat::Note {
        message: format!("via_http found no current HTTP hop: inferred links beyond depth {max_depth} are stale or low-confidence"),
      }));
    }
    if walk.truncated {
      return Ok(Some(Caveat::Note {
        message: "via_http found no HTTP hop among the reached symbols, but the traversal was truncated; raise max_nodes to look further".to_owned(),
      }));
    }
    let endpoints: BTreeSet<EntityId> = self.endpoints.union(&probe.endpoints).copied().collect();
    let client_calls = self.client_calls.union(&probe.client_calls).count();
    let reached = match self.direction {
      Direction::Reverse => !endpoints.is_empty(),
      Direction::Forward => client_calls > 0,
    };
    if reached && let Some(missing) = self.missing_links()? {
      return Ok(Some(missing));
    }
    let message = match (self.direction, reached) {
      (Direction::Reverse, false) => "via_http found no HTTP hop: no reached symbol exposes an endpoint".to_owned(),
      (Direction::Reverse, true) => format!(
        "via_http found no HTTP hop: no client in this project is linked to {} (external clients may exist)",
        self.endpoint_names(&endpoints)?
      ),
      (Direction::Forward, false) => "via_http found no HTTP hop: no reached symbol makes an HTTP request".to_owned(),
      (Direction::Forward, true) => format!(
        "via_http found no HTTP hop: {} made by reached symbols {} no endpoint handled in this project",
        crate::text::counted(client_calls, "client call"),
        if client_calls == 1 { "matches" } else { "match" }
      ),
    };
    Ok(Some(Caveat::Note { message }))
  }

  fn beyond_depth(&self, walk: &Walk, max_depth: u32) -> QueryResult<(Option<String>, HttpBridges<'g, 'a, 'c>)> {
    let mut probe = HttpBridges::new(self.graph, self.direction, &[]);
    let mut found: Vec<String> = Vec::new();
    for reached in walk.reached.iter().filter(|reached| reached.depth >= max_depth) {
      let Some(bridge) = probe.bridges(reached.id)?.into_iter().next() else { continue };
      let (Some(node), Some(endpoint)) = (self.graph.entity(reached.id)?, self.graph.entity(bridge.edge.to)?) else {
        continue;
      };
      let route = short_name(&endpoint.key);
      found.push(match (node.id == endpoint.id, self.direction) {
        (true, _) => format!("{route} has linked clients"),
        (false, Direction::Reverse) => format!("{} exposes {route}", short_name(&node.key)),
        (false, Direction::Forward) => format!("{} requests {route}", short_name(&node.key)),
      });
    }
    found.sort();
    let beyond = found.first().map(|first| match found.len() {
      1 => first.clone(),
      many => format!("{first}, and {} more", many - 1),
    });
    Ok((beyond, probe))
  }

  fn missing_links(&self) -> QueryResult<Option<Caveat>> {
    if link_caveat(self.graph, EdgeKind::CallsEndpoint, "")?.is_none() {
      return Ok(None);
    }
    let facts = self.graph.tx().inferred_facts(&InferredQuery {
      subject: None,
      relation: Some(RELATION_CALLS_ENDPOINT.to_owned()),
      freshness: FreshnessPolicy::IncludeStale,
    })?;
    let mut scan = LinkScan::default();
    let mut current = 0;
    for fact in &facts {
      if scan.explained_freshness(self.graph, fact)?.is_current() {
        current += 1;
      }
    }
    let detail = match current {
      0 => "via_http found no HTTP hop: no client->endpoint links (calls_endpoint), static or current inferred, exist in this index".to_owned(),
      count => format!(
        "via_http found no HTTP hop: no static client->endpoint links (calls_endpoint) exist in this index, and none of its current inferred links ({count}) reach the traversed symbols"
      ),
    };
    Ok(Some(Caveat::MissingFacts { detail }))
  }

  fn endpoint_names(&self, endpoints: &BTreeSet<EntityId>) -> QueryResult<String> {
    let mut names = Vec::new();
    for id in endpoints {
      names.extend(self.graph.entity(*id)?.map(|endpoint| short_name(&endpoint.key)));
    }
    names.sort();
    let more = names.len().saturating_sub(NAMED_ENDPOINTS);
    names.truncate(NAMED_ENDPOINTS);
    if more > 0 {
      names.push(format!("{more} more"));
    }
    Ok(names.join(", "))
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
  Node,
  Beyond,
}

fn origin_of(node: &Entity, endpoint: &Entity) -> Origin {
  match node.id == endpoint.id {
    true => Origin::Node,
    false => Origin::Beyond,
  }
}

impl<'e> Crossing<'e> {
  fn of_edge(link: Edge, endpoint: &'e Entity) -> Self {
    Self {
      tag: CertaintyTag::of(&link.certainty),
      hop_edge: link.id.to_string(),
      link,
      endpoint,
    }
  }

  fn of_inferred(link: Edge, hop: Hop, endpoint: &'e Entity) -> Self {
    Self {
      link,
      tag: hop.certainty,
      hop_edge: hop.edge,
      endpoint,
    }
  }
}
