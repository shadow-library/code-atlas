use std::collections::BTreeSet;

use atlas_core::{Certainty, Edge, EdgeKind, Entity, EntityKey, EntityKind, Evidence, HttpMethod};
use atlas_link::ATTR_COMPOSED_FROM;
use atlas_semantic::{LINK_CANDIDATES_PAYLOAD, MAX_LINK_CANDIDATES};
use atlas_store::{EdgeFilter, EntityFilter, ReadTx, StoreResult, StoredInferredFact};

use crate::error::EngineResult;

const LINK_TOP_K: usize = 5;
const ATTR_UNMATCHED: &str = "unmatched";
const PLACEHOLDER: &str = "{}";
const PARAM_SEGMENT_SCORE: f64 = 0.5;

const _: () = assert!(LINK_TOP_K <= MAX_LINK_CANDIDATES);

#[derive(Debug, Clone, PartialEq)]
pub struct EndpointChoice {
  pub endpoint: Entity,
  pub method: String,
  pub route: String,
  pub score: f64,
  pub handler_signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallSite {
  pub caller: Entity,
  pub evidence: Evidence,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkCandidate {
  pub call: Entity,
  pub method: HttpMethod,
  pub target: String,
  pub site: Option<CallSite>,
  pub endpoints: Vec<EndpointChoice>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkPlan {
  pub candidates: Vec<LinkCandidate>,
  pub without_endpoints: u64,
}

fn kind_filter(kind: EdgeKind) -> EdgeFilter {
  EdgeFilter {
    kinds: vec![kind],
    ..EdgeFilter::default()
  }
}

fn same_site(left: &Evidence, right: &Evidence) -> bool {
  left.repo == right.repo && left.path == right.path && left.span.start_byte <= right.span.end_byte && right.span.start_byte <= left.span.end_byte
}

fn site_links(tx: &ReadTx<'_>, call: &Entity, request: &Edge, site: Option<&Evidence>) -> StoreResult<Vec<Edge>> {
  let own = tx.edges_from(call.id, &kind_filter(EdgeKind::CallsEndpoint))?;
  if !own.is_empty() {
    return Ok(own);
  }
  let links = tx.edges_from(request.from, &kind_filter(EdgeKind::CallsEndpoint))?;
  let (call_key, call_id) = (call.key.to_string(), call.id.to_string());
  let attributed: Vec<Edge> = links
    .iter()
    .filter(|link| {
      let names_call = link.attrs.values().any(|value| value.as_str().is_some_and(|text| text == call_key || text == call_id));
      let shares_site = site.is_some_and(|site| link.evidence.iter().any(|evidence| same_site(evidence, site)));
      names_call || shares_site
    })
    .cloned()
    .collect();
  if !attributed.is_empty() {
    return Ok(attributed);
  }
  let unattributable = links.iter().all(|link| link.evidence.is_empty());
  let only_request = tx.edges_from(request.from, &kind_filter(EdgeKind::MakesRequest))?.len() == 1;
  Ok(match unattributable && only_request {
    true => links,
    false => Vec::new(),
  })
}

pub fn client_call_site_linked(tx: &ReadTx<'_>, call: &Entity, request: &Edge, site: Option<&Evidence>) -> StoreResult<bool> {
  Ok(!site_links(tx, call, request, site)?.is_empty())
}

pub fn client_call_flagged_unmatched(call: &Entity) -> bool {
  call.attrs.get(ATTR_UNMATCHED).and_then(serde_json::Value::as_bool).unwrap_or(false)
}

fn client_call_links(tx: &ReadTx<'_>, call: &Entity) -> StoreResult<Vec<Edge>> {
  if client_call_flagged_unmatched(call) {
    return Ok(Vec::new());
  }
  let mut links: Vec<Edge> = Vec::new();
  for request in tx.edges_to(call.id, &kind_filter(EdgeKind::MakesRequest))? {
    let sites: Vec<Option<&Evidence>> = match request.evidence.is_empty() {
      true => vec![None],
      false => request.evidence.iter().map(Some).collect(),
    };
    for site in sites {
      for link in site_links(tx, call, &request, site)? {
        if !links.iter().any(|known| known.id == link.id) {
          links.push(link);
        }
      }
    }
  }
  Ok(links)
}

pub fn client_call_linked(tx: &ReadTx<'_>, call: &Entity) -> StoreResult<bool> {
  Ok(!client_call_links(tx, call)?.is_empty())
}

fn representative_site(tx: &ReadTx<'_>, call: &Entity) -> EngineResult<Option<CallSite>> {
  let mut sites = Vec::new();
  for request in tx.edges_to(call.id, &kind_filter(EdgeKind::MakesRequest))? {
    let Some(caller) = tx.entity(request.from)? else { continue };
    for evidence in request.evidence {
      sites.push(CallSite { caller: caller.clone(), evidence });
    }
  }
  Ok(sites.into_iter().min_by(|a, b| {
    (a.evidence.repo.as_str(), a.evidence.path.as_str(), a.evidence.span.start_byte).cmp(&(b.evidence.repo.as_str(), b.evidence.path.as_str(), b.evidence.span.start_byte))
  }))
}

fn methods_compatible(client: HttpMethod, endpoint: HttpMethod) -> bool {
  !matches!(client, HttpMethod::Any | HttpMethod::Unknown) && (client == endpoint || endpoint == HttpMethod::Any)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
  Literal(String),
  Param,
}

fn client_segments(target: &str) -> Vec<Segment> {
  let path = target.split(['?', '#']).next().unwrap_or_default();
  let path = path
    .strip_prefix("https://")
    .or_else(|| path.strip_prefix("http://"))
    .map_or(path, |rest| rest.find('/').map_or("", |slash| &rest[slash..]));
  let path = path.strip_prefix(PLACEHOLDER).unwrap_or(path);
  path
    .split('/')
    .filter(|segment| !segment.is_empty())
    .map(|segment| match segment.contains(PLACEHOLDER) {
      true => Segment::Param,
      false => Segment::Literal(segment.to_ascii_lowercase()),
    })
    .collect()
}

fn route_segments(route: &str) -> Vec<Segment> {
  route
    .split('/')
    .filter(|segment| !segment.is_empty())
    .map(|segment| match segment.starts_with(':') || segment.starts_with('{') || segment.contains('*') {
      true => Segment::Param,
      false => Segment::Literal(segment.to_ascii_lowercase()),
    })
    .collect()
}

fn segment_score(client: &Segment, route: &Segment) -> f64 {
  match (client, route) {
    (Segment::Literal(left), Segment::Literal(right)) if left == right => 1.0,
    (Segment::Literal(left), Segment::Literal(right)) if left.trim_end_matches('s') == right.trim_end_matches('s') => 0.8,
    (Segment::Literal(_), Segment::Literal(_)) => 0.0,
    (Segment::Param, Segment::Param) => 1.0,
    _ => PARAM_SEGMENT_SCORE,
  }
}

pub(super) fn path_similarity(target: &str, route: &str) -> f64 {
  let (client, endpoint) = (client_segments(target), route_segments(route));
  let longest = client.len().max(endpoint.len());
  if longest == 0 {
    return 1.0;
  }
  let aligned: f64 = client.iter().rev().zip(endpoint.iter().rev()).map(|(left, right)| segment_score(left, right)).sum();
  (aligned / longest as f64 * 1000.0).round() / 1000.0
}

pub fn superseded_fragment_keys(tx: &ReadTx<'_>) -> StoreResult<BTreeSet<String>> {
  Ok(composed_from(&tx.entities_by_kind(&EntityKind::ApiEndpoint, &EntityFilter::default())?))
}

fn handler_signature(tx: &ReadTx<'_>, endpoint: &Entity) -> EngineResult<Option<String>> {
  for edge in tx.edges_to(endpoint.id, &kind_filter(EdgeKind::ExposesEndpoint))? {
    if let Some(signature) = tx.entity(edge.from)?.and_then(|handler| handler.signature) {
      return Ok(Some(signature));
    }
  }
  Ok(None)
}

type Ranked<'e> = (f64, &'e Entity, HttpMethod, &'e str);

fn ranked<'e>(endpoints: &'e [Entity], method: HttpMethod, target: &str) -> Vec<Ranked<'e>> {
  let mut ranked: Vec<Ranked<'e>> = endpoints
    .iter()
    .filter_map(|endpoint| match &endpoint.key {
      EntityKey::Endpoint {
        method: endpoint_method, route, ..
      } if methods_compatible(method, *endpoint_method) => Some((path_similarity(target, route), endpoint, *endpoint_method, route.as_str())),
      _ => None,
    })
    .filter(|(score, ..)| *score > 0.0)
    .collect();
  ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.key.to_string().cmp(&b.1.key.to_string())));
  ranked.truncate(LINK_TOP_K);
  ranked
}

fn choices(tx: &ReadTx<'_>, endpoints: &[Entity], method: HttpMethod, target: &str) -> EngineResult<Vec<EndpointChoice>> {
  ranked(endpoints, method, target)
    .into_iter()
    .map(|(score, endpoint, endpoint_method, route)| {
      Ok(EndpointChoice {
        endpoint: endpoint.clone(),
        method: endpoint_method.as_str().to_owned(),
        route: route.to_owned(),
        score,
        handler_signature: handler_signature(tx, endpoint)?,
      })
    })
    .collect()
}

fn composed_from(endpoints: &[Entity]) -> BTreeSet<String> {
  endpoints
    .iter()
    .filter_map(|endpoint| endpoint.attrs.get(ATTR_COMPOSED_FROM).and_then(serde_json::Value::as_str).map(str::to_owned))
    .collect()
}

pub struct LinkContext {
  endpoints: Vec<Entity>,
}

impl LinkContext {
  pub fn load(tx: &ReadTx<'_>) -> StoreResult<Self> {
    let all = tx.entities_by_kind(&EntityKind::ApiEndpoint, &EntityFilter::default())?;
    let superseded = composed_from(&all);
    let endpoints = all
      .into_iter()
      .filter(|endpoint| !superseded.contains(&endpoint.key.to_string()) && endpoint.path.is_some())
      .collect();
    Ok(Self { endpoints })
  }
}

pub(super) fn candidate_for(tx: &ReadTx<'_>, call: &Entity, context: &LinkContext) -> EngineResult<Option<LinkCandidate>> {
  let EntityKey::ClientCall { method, target, .. } = &call.key else { return Ok(None) };
  if client_call_linked(tx, call)? {
    return Ok(None);
  }
  Ok(Some(LinkCandidate {
    call: call.clone(),
    method: *method,
    target: target.clone(),
    site: representative_site(tx, call)?,
    endpoints: choices(tx, &context.endpoints, *method, target)?,
  }))
}

pub(super) fn plan(tx: &ReadTx<'_>) -> EngineResult<LinkPlan> {
  let context = LinkContext::load(tx)?;
  let mut calls = tx.entities_by_kind(&EntityKind::ApiClientCall, &EntityFilter::default())?;
  calls.sort_by_cached_key(|call| call.key.to_string());
  let mut plan = LinkPlan::default();
  for call in &calls {
    let Some(candidate) = candidate_for(tx, call, &context)? else { continue };
    match candidate.endpoints.is_empty() {
      true => plan.without_endpoints += 1,
      false => plan.candidates.push(candidate),
    }
  }
  Ok(plan)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agreement {
  SameEndpoint,
  DifferentEndpoint,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkFactState {
  Current,
  CandidatesChanged,
  Vanished,
  Superseded { agreement: Agreement, link: Certainty },
}

impl LinkFactState {
  pub fn is_settled(&self) -> bool {
    matches!(self, Self::Vanished | Self::Superseded { .. })
  }
}

fn recorded_candidates(fact: &StoredInferredFact) -> Option<BTreeSet<String>> {
  let recorded = fact.payload.get(LINK_CANDIDATES_PAYLOAD)?.as_array()?;
  recorded.iter().map(|value| value.as_str().map(str::to_owned)).collect()
}

pub fn link_fact_state(tx: &ReadTx<'_>, context: &LinkContext, fact: &StoredInferredFact) -> StoreResult<LinkFactState> {
  let [call, endpoint] = fact.subjects[..] else { return Ok(LinkFactState::Vanished) };
  let (Some(call), Some(_)) = (tx.entity(call)?, tx.entity(endpoint)?) else {
    return Ok(LinkFactState::Vanished);
  };
  let EntityKey::ClientCall { method, target, .. } = &call.key else {
    return Ok(LinkFactState::Vanished);
  };
  let links = client_call_links(tx, &call)?;
  let strongest = |links: &mut dyn Iterator<Item = &Edge>| links.map(|link| &link.certainty).max_by(|a, b| a.cmp_strength(b)).cloned();
  if let Some(link) = strongest(&mut links.iter().filter(|link| link.to == endpoint)) {
    return Ok(LinkFactState::Superseded {
      agreement: Agreement::SameEndpoint,
      link,
    });
  }
  if let Some(link) = strongest(&mut links.iter()) {
    return Ok(LinkFactState::Superseded {
      agreement: Agreement::DifferentEndpoint,
      link,
    });
  }
  let current: BTreeSet<String> = ranked(&context.endpoints, *method, target)
    .iter()
    .map(|(_, endpoint, ..)| endpoint.id.to_string())
    .collect();
  Ok(match recorded_candidates(fact) == Some(current) {
    true => LinkFactState::Current,
    false => LinkFactState::CandidatesChanged,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_rank_routes_by_suffix_aligned_segments() {
    assert_eq!(path_similarity("{}/users/{}", "/users/{id}"), 1.0);
    assert_eq!(path_similarity("https://api.acme.dev/v1/orders/{}", "/orders/:id"), 0.667);
    assert_eq!(path_similarity("{}/user/{}", "/users/{id}"), 0.9);
    assert_eq!(path_similarity("{}/health", "/users/{id}"), 0.25);
    assert_eq!(path_similarity("{}/health", "/admin/stats"), 0.0);
  }

  #[test]
  fn should_only_pair_identical_or_any_methods() {
    assert!(methods_compatible(HttpMethod::Get, HttpMethod::Get));
    assert!(methods_compatible(HttpMethod::Get, HttpMethod::Any));
    assert!(!methods_compatible(HttpMethod::Get, HttpMethod::Post));
    assert!(!methods_compatible(HttpMethod::Unknown, HttpMethod::Any));
  }
}
