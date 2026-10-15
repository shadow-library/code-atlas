use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{Certainty, Edge, EdgeKind, Entity, EntityId, EntityKey, EntityKind, Evidence, HttpMethod, RepoName, Span, normalize_route};
use serde_json::json;

use crate::ATTR_COMPOSED_FROM;
use crate::error::LinkResult;
use crate::facts::Facts;
use crate::output::{Output, attr_str, confidence_of, evidence, heuristic, relabel, weaker};
use crate::route::{RouteSegment, parse_route};
use crate::scopes::PackageIndex;

const MOUNTS_KIND: &str = "mounts";
const GLOBAL_PREFIX_KIND: &str = "http_global_prefix";
const FAMILY_PREFIX_KIND: &str = "http_family_prefix";
const NEST_FRAMEWORK: &str = "nestjs";

const MAX_MOUNT_DEPTH: usize = 6;
const GLOBAL_PREFIX_CONFIDENCE: f32 = 0.85;
const AMBIGUOUS_GLOBAL_PREFIX_CONFIDENCE: f32 = 0.5;
const DYNAMIC_ROUTE_CONFIDENCE: f32 = 0.4;
const UNMOUNTED_RULES: [&str; 2] = ["express-router-unmounted", "axum-router-parameter"];

type GlobalPrefixes = BTreeMap<(RepoName, String), Vec<Prefix>>;

#[derive(Debug, Clone)]
struct Prefix {
  value: String,
  entity: Entity,
  packages: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
enum PrefixSource {
  Nest,
  DeclaredFamily,
}

impl PrefixSource {
  fn rule(self, candidates: usize) -> (f32, &'static str) {
    match (self, candidates) {
      (Self::Nest, 1) => (GLOBAL_PREFIX_CONFIDENCE, "nest-global-prefix"),
      (Self::Nest, _) => (AMBIGUOUS_GLOBAL_PREFIX_CONFIDENCE, "nest-global-prefix-ambiguous"),
      (Self::DeclaredFamily, 1) => (GLOBAL_PREFIX_CONFIDENCE, "declared-global-prefix"),
      (Self::DeclaredFamily, _) => (AMBIGUOUS_GLOBAL_PREFIX_CONFIDENCE, "declared-global-prefix-ambiguous"),
    }
  }

  fn note(self, prefix: &str) -> String {
    match self {
      Self::Nest => format!("setGlobalPrefix({prefix})"),
      Self::DeclaredFamily => format!("global prefix {prefix}"),
    }
  }
}

#[derive(Debug, Clone)]
pub(crate) struct Route {
  pub id: EntityId,
  pub entity: Entity,
  pub repo: RepoName,
  pub method: HttpMethod,
  pub route: String,
  pub segments: Vec<RouteSegment>,
  pub guard_source: EntityId,
  pub packages: Vec<String>,
}

pub(crate) struct Routes {
  pub all: Vec<Route>,
  pub matchable: Vec<Route>,
  packages: BTreeMap<EntityId, Vec<String>>,
  composed: usize,
}

impl Routes {
  pub fn composed_count(&self) -> usize {
    self.composed
  }

  pub fn packages_of(&self, route: &Route) -> &[String] {
    self.packages.get(&route.id).map_or(&[], Vec::as_slice)
  }
}

#[derive(Debug, Clone)]
struct Chain {
  prefix: String,
  confidence: f32,
  rule: &'static str,
  evidence: Vec<Evidence>,
  packages: Vec<String>,
}

pub(crate) fn compose_routes(facts: &Facts<'_, '_>, packages: &PackageIndex, output: &mut Output) -> LinkResult<Routes> {
  let mounts = EdgeKind::custom(MOUNTS_KIND)?;
  let prefixes = global_prefixes(facts, packages)?;
  let mut routes = Routes {
    all: Vec::new(),
    matchable: Vec::new(),
    packages: BTreeMap::new(),
    composed: 0,
  };
  for endpoint in facts.entities(&EntityKind::ApiEndpoint)? {
    let endpoint_packages = packages.of_entity(facts, endpoint.id)?;
    let Some(base) = route_of(&endpoint, endpoint.id, endpoint_packages) else { continue };
    let chains = chains_for(facts, &mounts, &prefixes, &endpoint, &base.packages)?;
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
  for route in &routes.all {
    let union = routes.packages.entry(route.id).or_default();
    union.extend(route.packages.iter().cloned());
    union.sort();
    union.dedup();
  }
  for route in &routes.all {
    let route_packages = &routes.packages[&route.id];
    if !route_packages.is_empty() && packages.has_several(&route.repo) {
      output.extend_entity(&route.entity, weaker(&route.entity.certainty, &Certainty::Syntactic), [("packages", json!(route_packages))]);
    }
  }
  routes.matchable.sort_by_key(|route| route.id);
  routes.matchable.dedup_by_key(|route| route.id);
  routes.all.sort_by_key(|route| route.id);
  routes.all.dedup_by_key(|route| route.id);
  Ok(routes)
}

fn route_of(entity: &Entity, guard_source: EntityId, packages: Vec<String>) -> Option<Route> {
  let EntityKey::Endpoint { repo, method, route } = &entity.key else { return None };
  Some(Route {
    id: entity.id,
    entity: entity.clone(),
    repo: repo.clone(),
    method: *method,
    route: route.clone(),
    segments: parse_route(route),
    guard_source,
    packages,
  })
}

fn global_prefixes(facts: &Facts<'_, '_>, packages: &PackageIndex) -> LinkResult<GlobalPrefixes> {
  let mut prefixes = GlobalPrefixes::new();
  for entity in facts.entities(&EntityKind::custom(GLOBAL_PREFIX_KIND)?)? {
    let Some(repo) = entity.repo.clone() else { continue };
    let value = entity.name.clone();
    let packages = packages.of_entity(facts, entity.id)?;
    prefixes.entry((repo, NEST_FRAMEWORK.to_owned())).or_default().push(Prefix { value, entity, packages });
  }
  for entity in facts.entities(&EntityKind::custom(FAMILY_PREFIX_KIND)?)? {
    let Some(repo) = entity.repo.clone() else { continue };
    let Some((family, value)) = entity.name.split_once(':') else { continue };
    let (family, value) = (family.to_owned(), value.to_owned());
    let packages = packages.of_entity(facts, entity.id)?;
    prefixes.entry((repo, family)).or_default().push(Prefix { value, entity, packages });
  }
  prefixes.values_mut().for_each(|candidates| candidates.retain(|prefix| prefix.value != "/"));
  Ok(prefixes)
}

fn chains_for(facts: &Facts<'_, '_>, mounts: &EdgeKind, prefixes: &GlobalPrefixes, endpoint: &Entity, endpoint_packages: &[String]) -> LinkResult<Vec<Chain>> {
  let Some(repo) = &endpoint.repo else { return Ok(Vec::new()) };
  let prefixes_of = |framework: &str| {
    let all = prefixes.get(&(repo.clone(), framework.to_owned())).map(Vec::as_slice).unwrap_or_default();
    let own: Vec<Prefix> = all
      .iter()
      .filter(|prefix| prefix.packages.iter().any(|package| endpoint_packages.contains(package)))
      .cloned()
      .collect();
    match own.is_empty() {
      true => all.to_vec(),
      false => own,
    }
  };
  match attr_str(&endpoint.attrs, "framework") {
    Some(NEST_FRAMEWORK) => prefix_chains(facts, &prefixes_of(NEST_FRAMEWORK), PrefixSource::Nest),
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
    Some(family) => prefix_chains(facts, &prefixes_of(family), PrefixSource::DeclaredFamily),
    None => Ok(Vec::new()),
  }
}

fn prefix_chains(facts: &Facts<'_, '_>, prefixes: &[Prefix], source: PrefixSource) -> LinkResult<Vec<Chain>> {
  let (confidence, rule) = source.rule(prefixes.len());
  let mut chains = Vec::new();
  for prefix in prefixes {
    chains.push(Chain {
      prefix: prefix.value.clone(),
      confidence,
      rule,
      evidence: entity_evidence(facts, &prefix.entity, Some(source.note(&prefix.value)))?.into_iter().collect(),
      packages: prefix.packages.clone(),
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
    packages: Vec::new(),
  }
}

fn join(outer: &Chain, inner: &Chain) -> Chain {
  Chain {
    prefix: normalize_route(&format!("{}/{}", outer.prefix, inner.prefix)),
    confidence: outer.confidence.min(inner.confidence),
    rule: inner.rule,
    evidence: outer.evidence.iter().chain(&inner.evidence).cloned().collect(),
    packages: Vec::new(),
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
  composed.attrs.insert(ATTR_COMPOSED_FROM.to_owned(), json!(endpoint.key.to_string()));
  composed.attrs.insert("mount_prefix".to_owned(), json!(chain.prefix));
  composed.attrs.insert("rule".to_owned(), json!(chain.rule));
  composed.attrs.remove("router");
  for exposure in facts.edges_to(endpoint.id, &EdgeKind::ExposesEndpoint)? {
    let exposure_certainty = match is_unmounted_cap(&exposure.certainty) {
      true => certainty.clone(),
      false => weaker(&exposure.certainty, &certainty),
    };
    let mut edge = Edge::new(EdgeKind::ExposesEndpoint, exposure.from, composed.id, "", exposure_certainty);
    edge.attrs.insert(ATTR_COMPOSED_FROM.to_owned(), json!(endpoint.key.to_string()));
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
    packages: chain_packages(&base.packages, &chain.packages),
  };
  output.entity(composed);
  Ok(composed_route)
}

fn chain_packages(base: &[String], chain: &[String]) -> Vec<String> {
  let shared: Vec<String> = base.iter().filter(|package| chain.contains(package)).cloned().collect();
  match shared.is_empty() {
    true => base.to_vec(),
    false => shared,
  }
}

pub(crate) fn entity_evidence(facts: &Facts<'_, '_>, entity: &Entity, note: Option<String>) -> LinkResult<Option<Evidence>> {
  let (Some(repo), Some(path), Some(span)) = (&entity.repo, &entity.path, entity.span) else {
    return Ok(None);
  };
  Ok(facts.file_hash(repo, path)?.map(|hash| evidence(repo, path, span, hash, note)))
}
