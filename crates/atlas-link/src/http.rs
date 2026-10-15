use std::collections::BTreeMap;

use atlas_core::{Attrs, Certainty, Edge, EdgeKind, Entity, EntityId, EntityKey, EntityKind, Evidence, HttpMethod, RepoName};
use serde_json::{Value, json};

use crate::ATTR_CLIENT_CALL;
use crate::base::{BaseSource, RepoNames};
use crate::compose::{Route, Routes, entity_evidence};
use crate::error::LinkResult;
use crate::facts::Facts;
use crate::output::{Output, absence, attr_str, confidence_of, heuristic, relabel, rounded, weaker};
use crate::route::{BaseKind, ClientPath, RouteMatch, anchored_client_target, match_route, parse_client_target};
use crate::scopes::{PackageIndex, package_of_specifier};

const RULE: &str = "http-route-match";
const ANY_METHOD_FACTOR: f32 = 0.85;
const NAMED_BASE_BONUS: f32 = 0.05;
const MAX_CONFIDENCE: f32 = 0.9;
const WEAK_REQUEST_THRESHOLD: f32 = 0.6;
const WEAK_REQUEST_ALLOWANCE: f32 = 0.15;
const MIN_SHARE: f32 = 0.1;
const TIE_EPSILON: f32 = 1e-4;
const OTHER_REPO_WITH_EVIDENCE_WEIGHT: f32 = 0.15;
const RELATIVE_OTHER_REPO_WEIGHT: f32 = 0.25;
const ABSOLUTE_SAME_REPO_WEIGHT: f32 = 0.4;
const ATTR_SITE_ATTRS: &str = "site_attrs";
const CLIENT_DEFINITION_KIND: &str = "typed_client";
const INHERITED_ATTRS: [&str; 3] = ["base_source", "types_module", "types_path"];
const ATTR_CLIENT_BASE: &str = "client_base";

pub(crate) struct Site {
  pub caller: Entity,
  pub client_package: Option<String>,
  pub definition: Option<Entity>,
  pub client: EntityKey,
  pub attrs: Attrs,
  pub evidence: Evidence,
  pub request_confidence: f32,
  pub links: Vec<SiteLink>,
}

pub(crate) struct SiteLink {
  pub endpoint: EntityId,
  pub share: f32,
}

pub(crate) struct ClientLinks {
  pub sites: Vec<Site>,
  pub client_count: usize,
  pub unmatched_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unmatched {
  UnknownMethod,
  NoEndpoint,
}

impl Unmatched {
  fn label(self) -> &'static str {
    match self {
      Self::UnknownMethod => "unknown_method",
      Self::NoEndpoint => "no_matching_endpoint",
    }
  }
}

struct Candidate<'r> {
  route: &'r Route,
  found: RouteMatch,
  confidence: f32,
}

struct Placement<'r> {
  candidate: &'r Candidate<'r>,
  share: f32,
  confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageRule {
  GeneratedTypes,
  Configured,
  SameOrigin,
}

impl PackageRule {
  fn label(self) -> &'static str {
    match self {
      Self::GeneratedTypes => "types-package",
      Self::Configured => "configured-package",
      Self::SameOrigin => "same-package",
    }
  }
}

struct Preference {
  package: String,
  rule: PackageRule,
}

struct ClientDefinitions {
  by_module: BTreeMap<(RepoName, String, String), Entity>,
}

impl ClientDefinitions {
  fn load(facts: &Facts<'_, '_>) -> LinkResult<Self> {
    let mut by_module = BTreeMap::new();
    for definition in facts.entities(&EntityKind::custom(CLIENT_DEFINITION_KIND)?)? {
      let (Some(repo), Some(export)) = (definition.repo.clone(), attr_str(&definition.attrs, "export").map(str::to_owned)) else {
        continue;
      };
      let modules = definition.attrs.get("modules").and_then(Value::as_array).cloned().unwrap_or_default();
      for module in modules.iter().filter_map(Value::as_str) {
        by_module.entry((repo.clone(), module.to_owned(), export.clone())).or_insert_with(|| definition.clone());
      }
    }
    Ok(Self { by_module })
  }

  fn of(&self, repo: &RepoName, attrs: &Attrs) -> Option<&Entity> {
    let module = attr_str(attrs, "client_module")?;
    let export = attr_str(attrs, "client_export")?;
    self.by_module.get(&(repo.clone(), module.to_owned(), export.to_owned()))
  }
}

const RULE_NO_ROUTE: &str = "no-matching-route";

pub(crate) fn link_clients(facts: &Facts<'_, '_>, routes: &Routes, packages: &PackageIndex, output: &mut Output) -> LinkResult<ClientLinks> {
  let names = RepoNames::load(facts, routes.all.iter().map(|route| route.repo.clone()))?;
  let definitions = ClientDefinitions::load(facts)?;
  let mut links = ClientLinks {
    sites: Vec::new(),
    client_count: 0,
    unmatched_count: 0,
  };
  for client in facts.entities(&EntityKind::ApiClientCall)? {
    let EntityKey::ClientCall { repo, method, target } = &client.key else { continue };
    links.client_count += 1;
    let path = parse_client_target(target);
    let sites = sites_of(facts, &client, packages, &definitions)?;
    let client_packages = packages.of_entity(facts, client.id)?;
    if !client_packages.is_empty() && packages.has_several(repo) {
      output.extend_entity(&client, weaker(&client.certainty, &Certainty::Syntactic), [("packages", json!(client_packages))]);
    }
    let client_candidates = candidates_for(routes, *method, &path);
    let mut unmatched = match (&client_candidates, sites.is_empty()) {
      (Err(reason), true) => Some(*reason),
      _ => None,
    };
    let mut matched = false;
    for mut site in sites {
      let site_path = site_target(&site, target);
      let joined_candidates = site_path.as_ref().map(|site_path| candidates_for(routes, *method, site_path));
      let (candidates, base) = match (&joined_candidates, &site_path) {
        (Some(joined), Some(site_path)) => (joined, &site_path.base),
        _ => (&client_candidates, &path.base),
      };
      let candidates = match candidates {
        Ok(candidates) => candidates,
        Err(reason) => {
          unmatched.get_or_insert(*reason);
          links.sites.push(site);
          continue;
        }
      };
      matched = true;
      let source = BaseSource::parse(attr_str(&site.attrs, "base_source"));
      let named = names.evidence(facts, repo, &source)?;
      let preference = preferred_package(candidates, routes, packages, repo, &site, base);
      let joined_base = site_path.as_ref().and_then(|_| site_base(&site)).map(str::to_owned);
      let pool: Vec<&Candidate<'_>> = candidates
        .iter()
        .filter(|candidate| preference.as_ref().is_none_or(|preference| has_package(routes, candidate.route, &preference.package)))
        .collect();
      let placements = place(&pool, repo, base, &named, site.request_confidence);
      for placement in &placements {
        let scope = SiteScope {
          endpoint_packages: routes.packages_of(placement.candidate.route),
          preference: preference.as_ref(),
          joined_base: joined_base.as_deref(),
        };
        output.edge(calls_endpoint(facts, &site, placement, &named, &scope, &alternatives(candidates, &placements, placement))?);
        site.links.push(SiteLink {
          endpoint: placement.candidate.route.id,
          share: placement.share,
        });
      }
      links.sites.push(site);
    }
    if let Some(reason) = unmatched.filter(|_| !matched) {
      links.unmatched_count += 1;
      output.extend_entity(
        &client,
        weaker(&client.certainty, &absence(RULE_NO_ROUTE)?),
        [("unmatched", json!(true)), ("unmatched_reason", json!(reason.label()))],
      );
    }
  }
  Ok(links)
}

fn site_target(site: &Site, target: &str) -> Option<ClientPath> {
  let base = site_base(site)?;
  match target.starts_with('/') {
    true => Some(parse_client_target(&format!("{base}{target}"))),
    false => anchored_client_target(base, target),
  }
}

fn site_base(site: &Site) -> Option<&str> {
  match &site.definition {
    Some(definition) => attr_str(&definition.attrs, "base").or_else(|| attr_str(&site.attrs, ATTR_CLIENT_BASE)),
    None if attr_str(&site.attrs, "client_export").is_some_and(|export| export.contains('.')) => None,
    None => attr_str(&site.attrs, ATTR_CLIENT_BASE),
  }
}

fn has_package(routes: &Routes, route: &Route, package: &str) -> bool {
  routes.packages_of(route).iter().any(|candidate| candidate == package)
}

fn sites_of(facts: &Facts<'_, '_>, client: &Entity, packages: &PackageIndex, definitions: &ClientDefinitions) -> LinkResult<Vec<Site>> {
  let mut sites = Vec::new();
  for request in facts.edges_to(client.id, &EdgeKind::MakesRequest)? {
    let Some(caller) = facts.entity(request.from)? else { continue };
    let client_package = match (&caller.repo, &caller.path) {
      (Some(repo), Some(path)) => packages.nearest(repo, path.as_str()).map(str::to_owned),
      _ => None,
    };
    let mut primary = request.attrs.clone();
    primary.remove(ATTR_SITE_ATTRS);
    let overrides = site_overrides(&request.attrs);
    for evidence in &request.evidence {
      let mut attrs = overrides.get(&evidence.span.start_byte).cloned().unwrap_or_else(|| primary.clone());
      let definition = caller.repo.as_ref().and_then(|repo| definitions.of(repo, &attrs)).cloned();
      if let Some(definition) = &definition {
        for name in INHERITED_ATTRS {
          if let (false, Some(value)) = (attrs.contains_key(name), definition.attrs.get(name)) {
            attrs.insert(name.to_owned(), value.clone());
          }
        }
      }
      sites.push(Site {
        caller: caller.clone(),
        client_package: client_package.clone(),
        definition,
        client: client.key.clone(),
        attrs,
        evidence: evidence.clone(),
        request_confidence: confidence_of(&request.certainty),
        links: Vec::new(),
      });
    }
  }
  Ok(sites)
}

fn site_overrides(attrs: &Attrs) -> BTreeMap<u32, Attrs> {
  let Some(Value::Array(sites)) = attrs.get(ATTR_SITE_ATTRS) else {
    return BTreeMap::new();
  };
  sites
    .iter()
    .filter_map(|site| {
      let span = site.get("span")?.as_u64()?;
      let attrs = site.get("attrs")?.as_object()?;
      Some((u32::try_from(span).ok()?, attrs.iter().map(|(name, value)| (name.clone(), value.clone())).collect()))
    })
    .collect()
}

fn candidates_for<'r>(routes: &'r Routes, method: HttpMethod, path: &ClientPath) -> Result<Vec<Candidate<'r>>, Unmatched> {
  if matches!(method, HttpMethod::Unknown | HttpMethod::Any) {
    return Err(Unmatched::UnknownMethod);
  }
  let mut candidates: Vec<Candidate<'r>> = routes
    .matchable
    .iter()
    .filter_map(|route| {
      let factor = method_factor(method, route.method)?;
      let found = match_route(path, &route.segments)?;
      Some(Candidate {
        route,
        found,
        confidence: found.confidence() * factor,
      })
    })
    .collect();
  if let Some(tier) = candidates.iter().map(|candidate| candidate.found.prefix_tier()).min() {
    candidates.retain(|candidate| candidate.found.prefix_tier() == tier);
  }
  match candidates.is_empty() {
    true => Err(Unmatched::NoEndpoint),
    false => Ok(candidates),
  }
}

fn method_factor(client: HttpMethod, endpoint: HttpMethod) -> Option<f32> {
  match endpoint {
    _ if endpoint == client => Some(1.0),
    HttpMethod::Any => Some(ANY_METHOD_FACTOR),
    _ => None,
  }
}

fn repo_weight(repo: &RepoName, client_repo: &RepoName, base: &BaseKind, named: &BTreeMap<RepoName, f32>) -> f32 {
  if !named.is_empty() {
    return named.get(repo).copied().unwrap_or(OTHER_REPO_WITH_EVIDENCE_WEIGHT);
  }
  match (base, repo == client_repo) {
    (BaseKind::Relative, true) | (BaseKind::Dynamic | BaseKind::Absolute { .. }, false) => 1.0,
    (BaseKind::Relative, false) => RELATIVE_OTHER_REPO_WEIGHT,
    (BaseKind::Dynamic | BaseKind::Absolute { .. }, true) => ABSOLUTE_SAME_REPO_WEIGHT,
  }
}

fn preferred_package(candidates: &[Candidate<'_>], routes: &Routes, packages: &PackageIndex, client_repo: &RepoName, site: &Site, base: &BaseKind) -> Option<Preference> {
  let types = match attr_str(&site.attrs, "types_path") {
    Some(path) => packages.nearest(client_repo, path),
    None => attr_str(&site.attrs, "types_module").and_then(package_of_specifier).filter(|name| packages.knows(name)),
  };
  let configured = attr_str(&site.attrs, "target_package").filter(|name| packages.knows(name));
  let same = site.client_package.as_deref().filter(|_| *base == BaseKind::Relative);
  [(types, PackageRule::GeneratedTypes), (configured, PackageRule::Configured), (same, PackageRule::SameOrigin)]
    .into_iter()
    .filter_map(|(package, rule)| package.map(|package| (package, rule)))
    .find(|(package, _)| candidates.iter().any(|candidate| has_package(routes, candidate.route, package)))
    .map(|(package, rule)| Preference {
      package: package.to_owned(),
      rule,
    })
}

fn place<'r>(candidates: &[&'r Candidate<'r>], client_repo: &RepoName, base: &BaseKind, named: &BTreeMap<RepoName, f32>, request_confidence: f32) -> Vec<Placement<'r>> {
  let mut best_by_repo: BTreeMap<&RepoName, Vec<&'r Candidate<'r>>> = BTreeMap::new();
  for &candidate in candidates {
    let best = best_by_repo.entry(&candidate.route.repo).or_default();
    match best.first().map(|current| candidate.confidence - current.confidence) {
      None => best.push(candidate),
      Some(delta) if delta > TIE_EPSILON => *best = vec![candidate],
      Some(delta) if delta.abs() <= TIE_EPSILON => best.push(candidate),
      Some(_) => {}
    }
  }
  let weights: BTreeMap<&RepoName, f32> = best_by_repo.keys().map(|repo| (*repo, repo_weight(repo, client_repo, base, named))).collect();
  let total: f32 = weights.values().sum();
  let mut placements = Vec::new();
  for (repo, best) in &best_by_repo {
    let share = weights[repo] / total;
    if share < MIN_SHARE {
      continue;
    }
    let bonus = if named.contains_key(*repo) { NAMED_BASE_BONUS } else { 0.0 };
    for candidate in best {
      let per_endpoint = share / best.len() as f32;
      let mut confidence = ((candidate.confidence + bonus) * per_endpoint).min(MAX_CONFIDENCE);
      if request_confidence < WEAK_REQUEST_THRESHOLD {
        confidence = confidence.min(request_confidence + WEAK_REQUEST_ALLOWANCE);
      }
      placements.push(Placement {
        candidate,
        share: per_endpoint,
        confidence,
      });
    }
  }
  placements
}

fn alternatives(candidates: &[Candidate<'_>], placements: &[Placement<'_>], current: &Placement<'_>) -> Vec<Value> {
  let mut others: Vec<(String, Value)> = candidates
    .iter()
    .filter(|candidate| candidate.route.id != current.candidate.route.id)
    .map(|candidate| {
      let placed = placements.iter().find(|placement| placement.candidate.route.id == candidate.route.id);
      let key = candidate.route.entity.key.to_string();
      let value = json!({
        "endpoint": key,
        "route_confidence": rounded(candidate.confidence),
        "linked": placed.is_some(),
        "share": placed.map_or(0.0, |placement| rounded(placement.share)),
      });
      (key, value)
    })
    .collect();
  others.sort_by(|left, right| left.0.cmp(&right.0));
  others.into_iter().map(|(_, value)| value).collect()
}

struct SiteScope<'s> {
  endpoint_packages: &'s [String],
  preference: Option<&'s Preference>,
  joined_base: Option<&'s str>,
}

fn calls_endpoint(
  facts: &Facts<'_, '_>,
  site: &Site,
  placement: &Placement<'_>,
  named: &BTreeMap<RepoName, f32>,
  scope: &SiteScope<'_>,
  alternatives: &[Value],
) -> LinkResult<Edge> {
  let route = placement.candidate.route;
  let found = placement.candidate.found;
  let mut edge = Edge::new(EdgeKind::CallsEndpoint, site.caller.id, route.id, "", heuristic(placement.confidence, RULE)?);
  let attrs = &mut edge.attrs;
  attrs.insert("rule".to_owned(), json!(found.rule()));
  attrs.insert(ATTR_CLIENT_CALL.to_owned(), json!(site.client.to_string()));
  attrs.insert("route".to_owned(), json!(route.route));
  attrs.insert("same_repo".to_owned(), json!(site.caller.repo.as_ref() == Some(&route.repo)));
  attrs.insert("base_source".to_owned(), site.attrs.get("base_source").cloned().unwrap_or(Value::Null));
  attrs.insert("base_evidence".to_owned(), json!(named.keys().map(RepoName::as_str).collect::<Vec<_>>()));
  attrs.insert("share".to_owned(), json!(rounded(placement.share)));
  attrs.insert(
    "match".to_owned(),
    json!({
      "static_segments": found.statics,
      "param_segments": found.params,
      "literal_on_param": found.literal_on_param,
      "dynamic_on_static": found.dynamic_on_static,
      "partial_segments": found.partial_segments,
      "catch_all": found.catch_all,
      "skipped_optional": found.skipped_optional,
      "base_path_segments": found.base_path_segments,
      "dynamic_head_segments": found.dynamic_head_segments,
    }),
  );
  attrs.insert("alternatives".to_owned(), json!(alternatives));
  if let Some(package) = &site.client_package {
    attrs.insert("client_package".to_owned(), json!(package));
  }
  if !scope.endpoint_packages.is_empty() {
    attrs.insert("endpoint_packages".to_owned(), json!(scope.endpoint_packages));
  }
  if let Some(preference) = scope.preference {
    attrs.insert("target_package".to_owned(), json!(preference.package));
    attrs.insert("package_rule".to_owned(), json!(preference.rule.label()));
  }
  if let Some(definition) = &site.definition {
    attrs.insert("client_defined_in".to_owned(), json!(definition.key.to_string()));
  }
  if let Some(base) = scope.joined_base {
    attrs.insert(ATTR_CLIENT_BASE.to_owned(), json!(base));
  }
  let note = format!("{} -> {} {}", site.client, route.method, route.route);
  edge.evidence.push(relabel(&site.evidence, Some(note)));
  edge.evidence.extend(entity_evidence(facts, &route.entity, Some("endpoint".to_owned()))?);
  if let Some(definition) = &site.definition {
    edge.evidence.extend(entity_evidence(facts, definition, Some("client definition".to_owned()))?);
  }
  Ok(edge)
}
