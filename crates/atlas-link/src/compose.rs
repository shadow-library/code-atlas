use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{Certainty, Edge, EdgeKind, Entity, EntityId, EntityKey, EntityKind, Evidence, HttpMethod, RepoName, Span, normalize_route};
use serde_json::json;

use crate::error::LinkResult;
use crate::facts::Facts;
use crate::output::{Output, attr_str, confidence_of, evidence, heuristic, relabel, weaker};
use crate::route::{RouteSegment, parse_route};

const MOUNTS_KIND: &str = "mounts";
const GLOBAL_PREFIX_KIND: &str = "http_global_prefix";
const MAX_MOUNT_DEPTH: usize = 6;
const NEST_PREFIX_CONFIDENCE: f32 = 0.85;
const NEST_AMBIGUOUS_PREFIX_CONFIDENCE: f32 = 0.5;
const DYNAMIC_ROUTE_CONFIDENCE: f32 = 0.4;
const UNMOUNTED_RULES: [&str; 2] = ["express-router-unmounted", "axum-router-parameter"];

#[derive(Debug, Clone)]
pub(crate) struct Route {
  pub id: EntityId,
  pub entity: Entity,
  pub repo: RepoName,
  pub method: HttpMethod,
  pub route: String,
  pub segments: Vec<RouteSegment>,
  pub guard_source: EntityId,
}

pub(crate) struct Routes {
  pub all: Vec<Route>,
  pub matchable: Vec<Route>,
  composed: usize,
}

impl Routes {
  pub fn composed_count(&self) -> usize {
    self.composed
  }
}

#[derive(Debug, Clone)]
struct Chain {
  prefix: String,
  confidence: f32,
  rule: &'static str,
  evidence: Vec<Evidence>,
}

pub(crate) fn compose_routes(facts: &Facts<'_, '_>, output: &mut Output) -> LinkResult<Routes> {
  let mounts = EdgeKind::custom(MOUNTS_KIND)?;
  let prefixes = global_prefixes(facts)?;
  let mut routes = Routes {
    all: Vec::new(),
    matchable: Vec::new(),
    composed: 0,
  };
  for endpoint in facts.entities(&EntityKind::ApiEndpoint)? {
    let Some(base) = route_of(&endpoint, endpoint.id) else { continue };
    let chains = chains_for(facts, &mounts, &prefixes, &endpoint)?;
    if chains.is_empty() && attr_str(&endpoint.attrs, "route_scope") != Some("router_relative") {
      routes.matchable.push(base.clone());
    }
    for chain in chains {
      let composed = compose(facts, &endpoint, &base, &chain, output)?;
      routes.composed += 1;
      routes.all.push(composed.clone());
      routes.matchable.push(composed);
    }
    routes.all.push(base);
  }
  routes.matchable.sort_by_key(|route| route.id);
  routes.matchable.dedup_by_key(|route| route.id);
  routes.all.sort_by_key(|route| route.id);
  routes.all.dedup_by_key(|route| route.id);
  Ok(routes)
}

fn route_of(entity: &Entity, guard_source: EntityId) -> Option<Route> {
  let EntityKey::Endpoint { repo, method, route } = &entity.key else { return None };
  Some(Route {
    id: entity.id,
    entity: entity.clone(),
    repo: repo.clone(),
    method: *method,
    route: route.clone(),
    segments: parse_route(route),
    guard_source,
  })
}

fn global_prefixes(facts: &Facts<'_, '_>) -> LinkResult<BTreeMap<RepoName, Vec<Entity>>> {
  let mut prefixes: BTreeMap<RepoName, Vec<Entity>> = BTreeMap::new();
  for entity in facts.entities(&EntityKind::custom(GLOBAL_PREFIX_KIND)?)? {
    let Some(repo) = entity.repo.clone() else { continue };
    if entity.name == "/" {
      continue;
    }
    prefixes.entry(repo).or_default().push(entity);
  }
  Ok(prefixes)
}

fn chains_for(facts: &Facts<'_, '_>, mounts: &EdgeKind, prefixes: &BTreeMap<RepoName, Vec<Entity>>, endpoint: &Entity) -> LinkResult<Vec<Chain>> {
  let Some(repo) = &endpoint.repo else { return Ok(Vec::new()) };
  match attr_str(&endpoint.attrs, "framework") {
    Some("nestjs") => nest_chains(facts, prefixes.get(repo).map(Vec::as_slice).unwrap_or_default()),
    Some("express") => match (attr_str(&endpoint.attrs, "route_scope"), attr_str(&endpoint.attrs, "router"), &endpoint.path) {
      (Some("router_relative"), Some(router), Some(path)) => match router_symbol(facts, repo, path, router)? {
        Some(router) => express_chains(facts, mounts, &router, &mut BTreeSet::new()),
        None => Ok(Vec::new()),
      },
      _ => Ok(Vec::new()),
    },
    Some("axum") => match enclosing_function(facts, endpoint)? {
      Some(function) => axum_chains(facts, mounts, &function, &mut BTreeSet::new()),
      None => Ok(Vec::new()),
    },
    _ => Ok(Vec::new()),
  }
}

fn nest_chains(facts: &Facts<'_, '_>, prefixes: &[Entity]) -> LinkResult<Vec<Chain>> {
  let (confidence, rule) = match prefixes.len() {
    1 => (NEST_PREFIX_CONFIDENCE, "nest-global-prefix"),
    _ => (NEST_AMBIGUOUS_PREFIX_CONFIDENCE, "nest-global-prefix-ambiguous"),
  };
  let mut chains = Vec::new();
  for prefix in prefixes {
    chains.push(Chain {
      prefix: prefix.name.clone(),
      confidence,
      rule,
      evidence: entity_evidence(facts, prefix, Some(format!("setGlobalPrefix({})", prefix.name)))?.into_iter().collect(),
    });
  }
  Ok(chains)
}

fn express_chains(facts: &Facts<'_, '_>, mounts: &EdgeKind, router: &Entity, visited: &mut BTreeSet<EntityId>) -> LinkResult<Vec<Chain>> {
  if visited.len() >= MAX_MOUNT_DEPTH || !visited.insert(router.id) {
    return Ok(Vec::new());
  }
  let mut chains = Vec::new();
  for mount in facts.edges_to(router.id, mounts)? {
    let Some(prefix) = attr_str(&mount.attrs, "mount_prefix") else { continue };
    let hop = mount_chain(&mount, prefix);
    match attr_str(&mount.attrs, "mounted_on_scope") {
      Some("absolute") => chains.push(hop),
      Some("router_relative") => {
        let (Some(parent), Some(site)) = (attr_str(&mount.attrs, "mounted_on"), mount.evidence.first()) else {
          continue;
        };
        let Some(parent) = router_symbol(facts, &site.repo, &site.path, parent)? else {
          continue;
        };
        for outer in express_chains(facts, mounts, &parent, visited)? {
          chains.push(join(&outer, &hop));
        }
      }
      _ => {}
    }
  }
  visited.remove(&router.id);
  Ok(chains)
}

fn axum_chains(facts: &Facts<'_, '_>, mounts: &EdgeKind, function: &Entity, visited: &mut BTreeSet<EntityId>) -> LinkResult<Vec<Chain>> {
  if visited.len() >= MAX_MOUNT_DEPTH || !visited.insert(function.id) {
    return Ok(Vec::new());
  }
  let mut chains = Vec::new();
  for mount in facts.edges_to(function.id, mounts)? {
    if attr_str(&mount.attrs, "framework") != Some("axum") {
      continue;
    }
    let Some(prefix) = attr_str(&mount.attrs, "mount_prefix") else { continue };
    let hop = mount_chain(&mount, prefix);
    let outer = match facts.entity(mount.from)? {
      Some(parent) => axum_chains(facts, mounts, &parent, visited)?,
      None => Vec::new(),
    };
    match outer.is_empty() {
      true => chains.push(hop),
      false => chains.extend(outer.iter().map(|outer| join(outer, &hop))),
    }
  }
  visited.remove(&function.id);
  Ok(chains)
}

fn mount_chain(mount: &Edge, prefix: &str) -> Chain {
  Chain {
    prefix: prefix.to_owned(),
    confidence: confidence_of(&mount.certainty),
    rule: "route-composition",
    evidence: mount.evidence.iter().map(|evidence| relabel(evidence, Some(format!("mount {prefix}")))).collect(),
  }
}

fn join(outer: &Chain, inner: &Chain) -> Chain {
  Chain {
    prefix: normalize_route(&format!("{}/{}", outer.prefix, inner.prefix)),
    confidence: outer.confidence.min(inner.confidence),
    rule: inner.rule,
    evidence: outer.evidence.iter().chain(&inner.evidence).cloned().collect(),
  }
}

fn router_symbol(facts: &Facts<'_, '_>, repo: &RepoName, path: &atlas_core::RelPath, name: &str) -> LinkResult<Option<Entity>> {
  Ok(facts.entities_in_file(repo, path)?.into_iter().find(|entity| {
    entity.name == name && matches!(entity.kind, EntityKind::Constant | EntityKind::Variable) && entity.key.as_symbol().is_some_and(|key| key.item_path().len() == 1)
  }))
}

fn enclosing_function(facts: &Facts<'_, '_>, endpoint: &Entity) -> LinkResult<Option<Entity>> {
  let (Some(repo), Some(path), Some(span)) = (&endpoint.repo, &endpoint.path, endpoint.span) else {
    return Ok(None);
  };
  Ok(
    facts
      .entities_in_file(repo, path)?
      .into_iter()
      .filter(|entity| matches!(entity.kind, EntityKind::Function | EntityKind::Method))
      .filter(|entity| entity.span.is_some_and(|outer| contains(outer, span)))
      .min_by_key(|entity| entity.span.map_or(u32::MAX, |outer| outer.end_byte - outer.start_byte)),
  )
}

fn contains(outer: Span, inner: Span) -> bool {
  outer.start_byte <= inner.start_byte && inner.end_byte <= outer.end_byte
}

fn is_unmounted_cap(certainty: &Certainty) -> bool {
  matches!(certainty, Certainty::Heuristic { rule, .. } if UNMOUNTED_RULES.contains(&rule.as_str()))
}

fn underlying_confidence(endpoint: &Entity) -> f32 {
  if !is_unmounted_cap(&endpoint.certainty) {
    return confidence_of(&endpoint.certainty);
  }
  match attr_str(&endpoint.attrs, "route_source") {
    Some("dynamic") => DYNAMIC_ROUTE_CONFIDENCE,
    _ => 1.0,
  }
}

fn compose(facts: &Facts<'_, '_>, endpoint: &Entity, base: &Route, chain: &Chain, output: &mut Output) -> LinkResult<Route> {
  let route = normalize_route(&format!("{}/{}", chain.prefix, base.route));
  let key = EntityKey::Endpoint {
    repo: base.repo.clone(),
    method: base.method,
    route: route.clone(),
  };
  let certainty = heuristic(underlying_confidence(endpoint).min(chain.confidence), chain.rule)?;
  let mut composed = Entity::new(key, certainty.clone());
  composed.path = endpoint.path.clone();
  composed.span = endpoint.span;
  composed.attrs = endpoint.attrs.clone();
  composed.attrs.insert("route_scope".to_owned(), json!("composed"));
  composed.attrs.insert("composed_from".to_owned(), json!(endpoint.key.to_string()));
  composed.attrs.insert("mount_prefix".to_owned(), json!(chain.prefix));
  composed.attrs.insert("rule".to_owned(), json!(chain.rule));
  composed.attrs.remove("router");
  for exposure in facts.edges_to(endpoint.id, &EdgeKind::ExposesEndpoint)? {
    let exposure_certainty = match is_unmounted_cap(&exposure.certainty) {
      true => certainty.clone(),
      false => weaker(&exposure.certainty, &certainty),
    };
    let mut edge = Edge::new(EdgeKind::ExposesEndpoint, exposure.from, composed.id, "", exposure_certainty);
    edge.attrs.insert("composed_from".to_owned(), json!(endpoint.key.to_string()));
    edge.evidence = exposure
      .evidence
      .iter()
      .map(|evidence| relabel(evidence, None))
      .chain(chain.evidence.iter().cloned())
      .collect();
    output.edge(edge);
  }
  let composed_route = Route {
    id: composed.id,
    entity: composed.clone(),
    repo: base.repo.clone(),
    method: base.method,
    segments: parse_route(&route),
    route,
    guard_source: base.id,
  };
  output.entity(composed);
  Ok(composed_route)
}

pub(crate) fn entity_evidence(facts: &Facts<'_, '_>, entity: &Entity, note: Option<String>) -> LinkResult<Option<Evidence>> {
  let (Some(repo), Some(path), Some(span)) = (&entity.repo, &entity.path, entity.span) else {
    return Ok(None);
  };
  Ok(facts.file_hash(repo, path)?.map(|hash| evidence(repo, path, span, hash, note)))
}
