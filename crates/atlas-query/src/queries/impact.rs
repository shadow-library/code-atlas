use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{EdgeKind, Entity, EntityId, EntityKey, EntityKind};
use serde::Serialize;

use crate::QueryEngine;
use crate::dynamic::dynamic_member_calls;
use crate::engine::unresolved_risk;
use crate::error::QueryResult;
use crate::graph::{Graph, Reached, Step, WalkSpec};
use crate::model::{Caveat, EntityRef, Hop, Response, Targeted};
use crate::policy::{Direction, Policy};
use crate::queries::common::{MAX_NODES, file_entity, file_symbols, is_test, package_key, resolve_one};
use crate::queries::traverse::{dispatch_caveat, risk_name};
use crate::target::TargetFilter;

const DEFAULT_DEPTH: u32 = 3;
const DEFAULT_GROUP_LIMIT: usize = 15;

#[derive(Debug, Clone, PartialEq)]
pub struct ImpactRequest {
  pub targets: Vec<String>,
  pub filter: TargetFilter,
  pub depth: u32,
  pub limit: usize,
  pub min_confidence: Option<f32>,
  pub max_nodes: usize,
}

impl ImpactRequest {
  pub fn new(targets: impl IntoIterator<Item = impl Into<String>>) -> Self {
    Self {
      targets: targets.into_iter().map(Into::into).collect(),
      filter: TargetFilter::default(),
      depth: DEFAULT_DEPTH,
      limit: DEFAULT_GROUP_LIMIT,
      min_confidence: None,
      max_nodes: MAX_NODES,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImpactItem {
  pub entity: EntityRef,
  pub depth: u32,
  pub score: f64,
  pub uncertain: bool,
  pub why: Vec<Hop>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ImpactGroup {
  pub total: usize,
  pub uncertain: usize,
  pub items: Vec<ImpactItem>,
  pub collapsed: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RepoImpact {
  pub repo: String,
  pub items: usize,
  pub only_uncertain: bool,
  pub applications: Vec<EntityRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImpactAnswer {
  pub targets: Vec<EntityRef>,
  pub depth: u32,
  pub direct: ImpactGroup,
  pub transitive: ImpactGroup,
  pub endpoints: ImpactGroup,
  pub consumers: ImpactGroup,
  pub repos: Vec<RepoImpact>,
  pub tests: ImpactGroup,
  pub config: ImpactGroup,
  pub unresolved_risk: Vec<Caveat>,
  pub caveats: Vec<Caveat>,
}

pub(crate) fn impact_steps() -> Vec<Step> {
  let reverse = [
    (EdgeKind::Calls, 1.0),
    (EdgeKind::Instantiates, 0.9),
    (EdgeKind::References, 0.8),
    (EdgeKind::Implements, 0.9),
    (EdgeKind::Extends, 0.9),
    (EdgeKind::TypeOf, 0.7),
    (EdgeKind::Imports, 0.4),
    (EdgeKind::ReExports, 0.8),
    (EdgeKind::Tests, 1.0),
    (EdgeKind::CallsEndpoint, 0.9),
    (EdgeKind::MakesRequest, 0.9),
    (EdgeKind::SupportsAuth, 0.6),
    (EdgeKind::UsesAuth, 0.6),
    (EdgeKind::ImplementsAuth, 0.8),
    (EdgeKind::RequiresGuard, 0.9),
    (EdgeKind::RequiresPermission, 0.7),
    (EdgeKind::ReadsEnv, 0.7),
    (EdgeKind::ReadsConfig, 0.7),
    (EdgeKind::DependsOnPackage, 0.5),
    (EdgeKind::Subscribes, 0.8),
    (EdgeKind::ReadsTable, 0.7),
    (EdgeKind::WritesTable, 0.7),
    (EdgeKind::MapsToTable, 0.7),
  ];
  reverse
    .into_iter()
    .map(|(kind, weight)| Step::new(kind, Direction::Reverse, weight))
    .chain([
      Step::free(EdgeKind::ExposesEndpoint, Direction::Forward, 1.0),
      Step::new(EdgeKind::Publishes, Direction::Forward, 0.6),
    ])
    .collect()
}

pub(crate) fn expands(entity: &Entity) -> bool {
  !is_test(entity) && !matches!(entity.kind, EntityKind::File | EntityKind::Repository | EntityKind::Application)
}

impl QueryEngine {
  pub fn impact(&self, request: &ImpactRequest) -> QueryResult<Response<Targeted<ImpactAnswer>>> {
    let freshness = self.barrier()?;
    let policy = Policy::with_floor(request.min_confidence.unwrap_or(0.0));
    let answer = self.read(policy, |graph| -> QueryResult<Targeted<ImpactAnswer>> {
      let mut targets = Vec::new();
      for spec in &request.targets {
        match resolve_one(self, graph, spec, &request.filter)? {
          Ok(target) => targets.push(target),
          Err(outcome) => return Ok(outcome),
        }
      }
      impact(graph, &targets, request).map(Targeted::Found)
    })?;
    let answer = self.with_problem_files(answer, freshness.run, |answer| &mut answer.caveats)?;
    Ok(Response { freshness, answer })
  }
}

fn seeds_of(graph: &Graph<'_, '_>, targets: &[Entity]) -> QueryResult<Vec<EntityId>> {
  let mut seeds = Vec::new();
  for target in targets {
    seeds.push(target.id);
    seeds.extend(file_symbols(graph, target)?.into_iter().flatten());
  }
  Ok(seeds)
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Bucket {
  Direct,
  Transitive,
  Endpoint,
  Consumer,
  Test,
  Config,
  Hidden,
}

fn bucket(entity: &Entity, reached: &Reached) -> Bucket {
  if is_test(entity) {
    return Bucket::Test;
  }
  match entity.kind {
    EntityKind::ApiEndpoint => return Bucket::Endpoint,
    EntityKind::ApiClientCall => return Bucket::Hidden,
    EntityKind::EnvVar | EntityKind::ConfigKey => return Bucket::Config,
    _ => {}
  }
  if reached.hops.iter().any(|hop| hop.edge.kind == EdgeKind::CallsEndpoint) {
    return Bucket::Consumer;
  }
  match reached.depth {
    1 => Bucket::Direct,
    _ => Bucket::Transitive,
  }
}

fn score(reached: &Reached) -> f64 {
  let raw = reached.strength / f64::from(reached.depth.max(1)) + 0.05 * (1.0 + reached.in_degree as f64).ln();
  (raw * 1000.0).round() / 1000.0
}

fn impact(graph: &Graph<'_, '_>, targets: &[Entity], request: &ImpactRequest) -> QueryResult<ImpactAnswer> {
  let seeds = seeds_of(graph, targets)?;
  let steps = impact_steps();
  let walk = graph.walk(
    &WalkSpec {
      seeds: &seeds,
      steps: &steps,
      max_depth: request.depth.max(1),
      max_nodes: request.max_nodes,
    },
    expands,
  )?;
  let mut buckets: BTreeMap<Bucket, Vec<ImpactItem>> = BTreeMap::new();
  let mut repos: BTreeMap<String, (usize, bool, BTreeSet<EntityId>)> = BTreeMap::new();
  let mut impacted_files: BTreeMap<EntityId, (Reached, EntityKey)> = BTreeMap::new();
  let mut seen_tests: BTreeSet<EntityId> = BTreeSet::new();
  for reached in &walk.reached {
    let Some(entity) = graph.entity(reached.id)? else { continue };
    let bucket = bucket(&entity, reached);
    if bucket == Bucket::Test {
      seen_tests.insert(entity.id);
    }
    if bucket != Bucket::Hidden {
      buckets.entry(bucket).or_default().push(ImpactItem {
        entity: EntityRef::of(&entity),
        depth: reached.depth,
        score: score(reached),
        uncertain: reached.is_uncertain(),
        why: graph.path(reached)?,
      });
    }
    if let Some(repo) = &entity.repo {
      let entry = repos.entry(repo.to_string()).or_insert((0, true, BTreeSet::new()));
      entry.0 += 1;
      entry.1 &= reached.is_uncertain();
      if let Some(package) = package_key(&entity) {
        entry.2.insert(EntityId::of(&package));
      }
    }
    if let Some(file) = file_entity(&entity).filter(|_| entity.kind.is_code_symbol() && !is_test(&entity)) {
      impacted_files.entry(EntityId::of(&file)).or_insert_with(|| (reached.clone(), file));
    }
  }
  for target in targets {
    if let Some(file) = file_entity(target) {
      impacted_files.entry(EntityId::of(&file)).or_insert_with(|| {
        (
          Reached {
            id: target.id,
            depth: 0,
            hops: Vec::new(),
            strength: 1.0,
            in_degree: 0,
          },
          file,
        )
      });
    }
  }
  let mut hidden = walk.hidden_below_floor;
  let mut covering = graph.admitted_into(impacted_files.keys().copied(), EdgeKind::CoversFile, &mut hidden)?;
  for (file, (via, _)) in &impacted_files {
    for edge in covering.remove(file).unwrap_or_default() {
      let Some(test) = graph.entity(edge.from)? else { continue };
      if !seen_tests.insert(test.id) {
        continue;
      }
      let mut why = vec![graph.relation(&edge, test.id)?];
      why.extend(graph.path(via)?);
      let uncertain = !edge.certainty.is_authoritative() || via.is_uncertain();
      buckets.entry(Bucket::Test).or_default().push(ImpactItem {
        entity: EntityRef::of(&test),
        depth: via.depth + 1,
        score: ((via.strength * crate::graph::certainty_value(&edge.certainty) / f64::from(via.depth + 1)) * 1000.0).round() / 1000.0,
        uncertain,
        why,
      });
    }
  }
  let mut config = buckets.remove(&Bucket::Config).unwrap_or_default();
  for target in targets {
    for edge in graph.admitted(target.id, Direction::Forward, &[EdgeKind::ReadsEnv, EdgeKind::ReadsConfig], &mut hidden)? {
      let Some(read) = graph.entity(edge.to)? else { continue };
      if config.iter().any(|item| item.entity.id == read.id.to_string()) {
        continue;
      }
      config.push(ImpactItem {
        entity: EntityRef::of(&read),
        depth: 1,
        score: 0.0,
        uncertain: !edge.certainty.is_authoritative(),
        why: vec![graph.relation(&edge, target.id)?],
      });
    }
  }
  let mut affected = Vec::new();
  for (repo, (items, only_uncertain, packages)) in repos {
    let mut applications = Vec::new();
    for package in packages {
      for edge in graph.edges_out(package, &[EdgeKind::PartOfApp])? {
        if let Some(app) = graph.entity(edge.to)? {
          applications.push(EntityRef::of(&app));
        }
      }
    }
    applications.sort_by_key(EntityRef::sort_key);
    applications.dedup();
    affected.push(RepoImpact {
      repo,
      items,
      only_uncertain,
      applications,
    });
  }
  let mut risk = Vec::new();
  for target in targets {
    risk.extend(unresolved_risk(graph, target.id, risk_name(target))?);
    risk.extend(dynamic_member_calls(graph, target)?);
    risk.extend(dispatch_caveat(graph, target)?);
  }
  let mut caveats = Vec::new();
  if walk.truncated {
    caveats.push(Caveat::Truncated { max_nodes: request.max_nodes });
  }
  if hidden > 0 {
    caveats.push(Caveat::BelowConfidenceFloor {
      hidden,
      floor: graph.policy.min_confidence,
    });
  }
  caveats.extend(crate::queries::common::link_caveat(
    graph,
    EdgeKind::CallsEndpoint,
    "no client->endpoint links (calls_endpoint) exist in this index; cross-repo API consumers cannot be shown",
  )?);
  let mut take = |bucket: Bucket| group(buckets.remove(&bucket).unwrap_or_default(), request.limit);
  Ok(ImpactAnswer {
    targets: targets.iter().map(EntityRef::of).collect(),
    depth: request.depth.max(1),
    direct: take(Bucket::Direct),
    transitive: take(Bucket::Transitive),
    endpoints: take(Bucket::Endpoint),
    consumers: take(Bucket::Consumer),
    repos: affected,
    tests: take(Bucket::Test),
    config: group(config, request.limit),
    unresolved_risk: risk,
    caveats,
  })
}

fn group(mut items: Vec<ImpactItem>, limit: usize) -> ImpactGroup {
  items.sort_by(|left, right| right.score.total_cmp(&left.score).then_with(|| left.entity.sort_key().cmp(&right.entity.sort_key())));
  let total = items.len();
  let uncertain = items.iter().filter(|item| item.uncertain).count();
  let mut shown = Vec::new();
  for (rank, item) in items.into_iter().enumerate() {
    if rank < limit || item.uncertain {
      shown.push(item);
    }
  }
  ImpactGroup {
    total,
    uncertain,
    collapsed: total - shown.len(),
    items: shown,
  }
}
