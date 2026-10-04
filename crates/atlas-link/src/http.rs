use std::collections::BTreeMap;

use atlas_core::{Attrs, Edge, EdgeKind, Entity, EntityId, EntityKey, EntityKind, Evidence, HttpMethod, RepoName};
use serde_json::{Value, json};

use crate::base::{BaseSource, RepoNames};
use crate::compose::{Route, Routes, entity_evidence};
use crate::error::LinkResult;
use crate::facts::Facts;
use crate::output::{Output, absence, annotated, attr_str, confidence_of, heuristic, relabel, rounded, weaker};
use crate::route::{BaseKind, ClientPath, RouteMatch, match_route, parse_client_target};

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

pub(crate) struct Site {
  pub caller: Entity,
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

const RULE_NO_ROUTE: &str = "no-matching-route";

pub(crate) fn link_clients(facts: &Facts<'_, '_>, routes: &Routes, output: &mut Output) -> LinkResult<ClientLinks> {
  let names = RepoNames::load(facts, routes.all.iter().map(|route| route.repo.clone()))?;
  let mut links = ClientLinks {
    sites: Vec::new(),
    client_count: 0,
    unmatched_count: 0,
  };
  for client in facts.entities(&EntityKind::ApiClientCall)? {
    let EntityKey::ClientCall { repo, method, target } = &client.key else { continue };
    links.client_count += 1;
    let path = parse_client_target(target);
    let sites = sites_of(facts, &client)?;
    let candidates = match candidates_for(routes, *method, &path) {
      Ok(candidates) => candidates,
      Err(reason) => {
        links.unmatched_count += 1;
        output.entity(annotated(
          &client,
          weaker(&client.certainty, &absence(RULE_NO_ROUTE)?),
          [("unmatched", json!(true)), ("unmatched_reason", json!(reason.label()))],
        ));
        links.sites.extend(sites);
        continue;
      }
    };
    for mut site in sites {
      let source = BaseSource::parse(attr_str(&site.attrs, "base_source"));
      let named = names.evidence(facts, repo, &source)?;
      let placements = place(&candidates, repo, &path.base, &named, site.request_confidence);
      for placement in &placements {
        output.edge(calls_endpoint(facts, &site, placement, &named, &alternatives(&candidates, &placements, placement))?);
        site.links.push(SiteLink {
          endpoint: placement.candidate.route.id,
          share: placement.share,
        });
      }
      links.sites.push(site);
    }
  }
  Ok(links)
}

fn sites_of(facts: &Facts<'_, '_>, client: &Entity) -> LinkResult<Vec<Site>> {
  let mut sites = Vec::new();
  for request in facts.edges_to(client.id, &EdgeKind::MakesRequest)? {
    let Some(caller) = facts.entity(request.from)? else { continue };
    let mut primary = request.attrs.clone();
    primary.remove(ATTR_SITE_ATTRS);
    let overrides = site_overrides(&request.attrs);
    for evidence in &request.evidence {
      sites.push(Site {
        caller: caller.clone(),
        client: client.key.clone(),
        attrs: overrides.get(&evidence.span.start_byte).cloned().unwrap_or_else(|| primary.clone()),
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
  if candidates.iter().any(|candidate| candidate.found.base_path_segments == 0) {
    candidates.retain(|candidate| candidate.found.base_path_segments == 0);
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

fn place<'r>(candidates: &'r [Candidate<'r>], client_repo: &RepoName, base: &BaseKind, named: &BTreeMap<RepoName, f32>, request_confidence: f32) -> Vec<Placement<'r>> {
  let mut best_by_repo: BTreeMap<&RepoName, Vec<&Candidate<'_>>> = BTreeMap::new();
  for candidate in candidates {
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

fn calls_endpoint(facts: &Facts<'_, '_>, site: &Site, placement: &Placement<'_>, named: &BTreeMap<RepoName, f32>, alternatives: &[Value]) -> LinkResult<Edge> {
  let route = placement.candidate.route;
  let found = placement.candidate.found;
  let mut edge = Edge::new(EdgeKind::CallsEndpoint, site.caller.id, route.id, "", heuristic(placement.confidence, RULE)?);
  let attrs = &mut edge.attrs;
  attrs.insert("rule".to_owned(), json!(found.rule()));
  attrs.insert("client_call".to_owned(), json!(site.client.to_string()));
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
    }),
  );
  attrs.insert("alternatives".to_owned(), json!(alternatives));
  let note = format!("{} -> {} {}", site.client, route.method, route.route);
  edge.evidence.push(relabel(&site.evidence, Some(note)));
  edge.evidence.extend(entity_evidence(facts, &route.entity, Some("endpoint".to_owned()))?);
  Ok(edge)
}
