use std::collections::BTreeMap;

use atlas_core::{EdgeKind, Entity, EntityKind};
use serde::Serialize;

use crate::QueryEngine;
use crate::dynamic::dynamic_member_calls;
use crate::engine::{macro_generated_refs, unresolved_risk};
use crate::error::QueryResult;
use crate::graph::{Graph, Reached, Step, WalkSpec, group_by_repo};
use crate::model::{Caveat, EntityRef, Hop, MacroGapScope, Response, Targeted};
use crate::policy::{Direction, Policy};
use crate::queries::common::{DEFAULT_LIMIT, MAX_NODES, file_symbols, resolve_one};
use crate::target::TargetFilter;

const DEFAULT_FLOOR: f32 = 0.25;

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
  let walk = graph.walk(
    &WalkSpec {
      seeds: &seeds,
      steps: &steps,
      max_depth: depth,
      max_nodes: request.max_nodes,
    },
    |_| true,
  )?;
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
  if walk.hidden_below_floor > 0 {
    caveats.push(Caveat::BelowConfidenceFloor {
      hidden: walk.hidden_below_floor,
      floor: graph.policy.min_confidence,
    });
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
  let relation = graph.relation(&last.edge, reached.id)?;
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
