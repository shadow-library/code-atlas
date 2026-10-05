use std::collections::BTreeSet;

use atlas_core::{EdgeKind, Entity, EntityId, EntityKey, EntityKind};
use atlas_engine::{client_call_flagged_unmatched, client_call_site_linked};
use serde::Serialize;

use crate::QueryEngine;
use crate::error::{QueryError, QueryResult};
use crate::graph::{Graph, Step, WalkSpec};
use crate::model::{AttrView, Caveat, CertaintyTag, EntityRef, Hop, Listing, Location, Response};
use crate::policy::{Direction, Policy};
use crate::queries::common::{DEFAULT_LIMIT, MAX_NODES, is_router_relative, link_caveat, package_key, superseded_fragments};
use crate::queries::inferred_links::{LinkEnd, LinkScan, inferred_links};
use crate::target::{TargetFilter, TargetSpec, resolve};

const CONSUMER_CALLER_DEPTH: u32 = 2;
const ENDPOINT_AUTH_ATTRS: [&str; 4] = ["auth_requirement", "auth_combinator", "unclassified_guards", "authorization_guards"];
const SCHEME_LINK_ATTRS: [&str; 4] = ["used", "consumers", "evidence_count", "supported_by_endpoints"];

#[derive(Debug, Clone, PartialEq)]
pub struct ApiRequest {
  pub repo: Option<String>,
  pub endpoint: Option<String>,
  pub limit: usize,
  pub min_confidence: Option<f32>,
}

impl Default for ApiRequest {
  fn default() -> Self {
    Self {
      repo: None,
      endpoint: None,
      limit: DEFAULT_LIMIT,
      min_confidence: None,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Related {
  pub entity: EntityRef,
  pub relation: Hop,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GuardView {
  pub guard: Related,
  pub schemes: Vec<Related>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EndpointScheme {
  pub scheme: Related,
  pub combinator: Option<AttrView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConsumerView {
  pub consumer: Related,
  pub client_call: Option<EntityRef>,
  pub callers: Vec<Related>,
  pub repos: Vec<String>,
  pub applications: Vec<EntityRef>,
  pub auth_used: Vec<Related>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EndpointView {
  pub endpoint: EntityRef,
  pub method: String,
  pub route: String,
  pub certainty: CertaintyTag,
  pub handler: Option<Related>,
  pub guards: Vec<GuardView>,
  pub auth: Vec<EndpointScheme>,
  pub auth_attrs: Vec<AttrView>,
  pub permissions: Vec<Related>,
  pub consumers: Vec<ConsumerView>,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub inferred_consumers: Vec<ConsumerView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageState {
  Used,
  Unused,
  Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportBasis {
  SupportsAuth,
  DerivedFromGuards,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SchemeSupport {
  pub endpoints: usize,
  pub basis: SupportBasis,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SchemeUsage {
  pub scheme: EntityRef,
  pub supported: Option<SchemeSupport>,
  pub implemented_by: Vec<Related>,
  pub used: UsageState,
  pub used_by: Vec<Related>,
  pub used_by_repos: Vec<String>,
  pub link_attrs: Vec<AttrView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuthMatrix {
  pub repo: String,
  pub schemes: Vec<SchemeUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClientCallView {
  pub call: EntityRef,
  pub caller: Related,
  pub site: Option<Location>,
  pub reason: String,
  pub unmatched_reason: Option<AttrView>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub inferred_link: Option<InferredLinkView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InferredLinkView {
  pub endpoint: EntityRef,
  pub relation: Hop,
  pub rationale: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
  Linked,
  NotLinked,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApiAnswer {
  pub consumer_links: LinkState,
  pub endpoints: Listing<EndpointView>,
  pub router_fragments: Listing<EndpointView>,
  pub auth_matrix: Vec<AuthMatrix>,
  pub unmatched_calls: Listing<ClientCallView>,
  pub caveats: Vec<Caveat>,
}

impl QueryEngine {
  pub fn api(&self, request: &ApiRequest) -> QueryResult<Response<ApiAnswer>> {
    let freshness = self.barrier()?;
    let policy = Policy::with_floor(request.min_confidence.unwrap_or(0.0));
    let mut answer = self.read(policy, |graph| api(self, graph, request))?;
    answer.caveats.extend(self.problem_files(freshness.run)?);
    Ok(Response { freshness, answer })
  }
}

fn api(query: &QueryEngine, graph: &Graph<'_, '_>, request: &ApiRequest) -> QueryResult<ApiAnswer> {
  let selected = selected_endpoints(query, graph, request)?;
  let mut caveats = Vec::new();
  let calls_missing = link_caveat(
    graph,
    EdgeKind::CallsEndpoint,
    "no client->endpoint links (calls_endpoint) exist in this index; consumers and unmatched calls are unknown",
  )?;
  let uses_missing = link_caveat(
    graph,
    EdgeKind::UsesAuth,
    "no client auth usage facts (uses_auth) exist in this index; scheme usage is unknown",
  )?;
  let supports_missing = link_caveat(
    graph,
    EdgeKind::SupportsAuth,
    "no endpoint->scheme facts (supports_auth) exist in this index; supported schemes come from guards only",
  )?;
  let (mounted, fragments) = split_router_fragments(graph, &selected, request.endpoint.is_some())?;
  let mut hidden = LinkScan::default();
  let mut endpoints = Vec::with_capacity(mounted.len());
  for endpoint in &mounted {
    endpoints.push(endpoint_view(graph, endpoint, &mut hidden)?);
  }
  let mut router_fragments = Vec::with_capacity(fragments.len());
  for endpoint in &fragments {
    router_fragments.push(endpoint_view(graph, endpoint, &mut hidden)?);
  }
  let api_repos: BTreeSet<String> = selected.iter().filter_map(|endpoint| endpoint.repo.as_ref().map(ToString::to_string)).collect();
  let mut auth_matrix = Vec::new();
  for repo in &api_repos {
    auth_matrix.push(matrix(graph, repo, uses_missing.is_some(), supports_missing.is_none())?);
  }
  let unmatched = match request.endpoint {
    Some(_) => Vec::new(),
    None => unmatched_calls(graph, calls_missing.is_some(), &mut hidden)?,
  };
  let consumer_links = match calls_missing {
    Some(_) => LinkState::NotLinked,
    None => LinkState::Linked,
  };
  caveats.extend(calls_missing);
  caveats.extend(uses_missing);
  caveats.extend(supports_missing);
  caveats.extend(hidden.caveats(graph.policy.min_confidence));
  Ok(ApiAnswer {
    consumer_links,
    endpoints: Listing::truncated(endpoints, request.limit),
    router_fragments: Listing::truncated(router_fragments, request.limit),
    auth_matrix,
    unmatched_calls: Listing::truncated(unmatched, request.limit),
    caveats,
  })
}

fn split_router_fragments(graph: &Graph<'_, '_>, selected: &[Entity], explicit: bool) -> QueryResult<(Vec<Entity>, Vec<Entity>)> {
  if explicit {
    return Ok((selected.to_vec(), Vec::new()));
  }
  let superseded = superseded_fragments(graph)?;
  let mut mounted = Vec::new();
  let mut fragments = Vec::new();
  for endpoint in selected {
    match (is_router_relative(endpoint), superseded.contains(&endpoint.key.to_string())) {
      (true, true) => {}
      (true, false) => fragments.push(endpoint.clone()),
      (false, _) => mounted.push(endpoint.clone()),
    }
  }
  Ok((mounted, fragments))
}

fn selected_endpoints(query: &QueryEngine, graph: &Graph<'_, '_>, request: &ApiRequest) -> QueryResult<Vec<Entity>> {
  let mut endpoints = match &request.endpoint {
    Some(spec) => {
      let filter = TargetFilter {
        kinds: vec![EntityKind::ApiEndpoint],
        repo: request.repo.clone(),
      };
      if !matches!(TargetSpec::parse(spec)?, TargetSpec::Endpoint { .. } | TargetSpec::Key(_)) {
        return Err(QueryError::invalid(format!("{spec:?} is not an endpoint; use `METHOD /route` or an endpoint key")));
      }
      match resolve(graph, &query.repos(), spec, &filter)? {
        crate::target::Resolution::Found(endpoint) => vec![*endpoint],
        crate::target::Resolution::Ambiguous(candidates) => candidates,
        crate::target::Resolution::NotFound(_) => Vec::new(),
      }
    }
    None => graph.entities_of_kind(&EntityKind::ApiEndpoint)?,
  };
  endpoints.retain(|endpoint| {
    request
      .repo
      .as_ref()
      .is_none_or(|repo| endpoint.repo.as_ref().is_some_and(|candidate| candidate.as_str() == repo))
  });
  endpoints.sort_by_key(endpoint_order);
  Ok(endpoints)
}

fn endpoint_order(endpoint: &Entity) -> (String, String, String) {
  match &endpoint.key {
    EntityKey::Endpoint { repo, method, route } => (repo.to_string(), route.clone(), method.to_string()),
    other => (String::new(), other.to_string(), String::new()),
  }
}

fn related(graph: &Graph<'_, '_>, edge: &atlas_core::Edge, subject: EntityId, other: EntityId) -> QueryResult<Option<Related>> {
  let Some(entity) = graph.entity(other)? else { return Ok(None) };
  Ok(Some(Related {
    entity: EntityRef::of(&entity),
    relation: graph.relation(edge, subject)?,
  }))
}

fn admitted(graph: &Graph<'_, '_>, id: EntityId, direction: Direction, kind: EdgeKind) -> QueryResult<Vec<atlas_core::Edge>> {
  let mut hidden = 0;
  graph.admitted(id, direction, &[kind], &mut hidden)
}

fn endpoint_view(graph: &Graph<'_, '_>, endpoint: &Entity, hidden: &mut LinkScan) -> QueryResult<EndpointView> {
  let (method, route) = match &endpoint.key {
    EntityKey::Endpoint { method, route, .. } => (method.to_string(), route.clone()),
    _ => (String::new(), endpoint.name.clone()),
  };
  let mut handler = None;
  let mut permissions = Vec::new();
  for edge in admitted(graph, endpoint.id, Direction::Reverse, EdgeKind::ExposesEndpoint)? {
    handler = related(graph, &edge, endpoint.id, edge.from)?;
    for permission in admitted(graph, edge.from, Direction::Forward, EdgeKind::RequiresPermission)? {
      permissions.extend(related(graph, &permission, edge.from, permission.to)?);
    }
  }
  let mut guards = Vec::new();
  let mut guard_edges = admitted(graph, endpoint.id, Direction::Forward, EdgeKind::RequiresGuard)?;
  guard_edges.sort_by_key(|edge| edge.attrs.get("order").and_then(serde_json::Value::as_i64).unwrap_or(i64::MAX));
  for edge in guard_edges {
    let Some(guard) = related(graph, &edge, endpoint.id, edge.to)? else { continue };
    let mut schemes = Vec::new();
    for implemented in admitted(graph, edge.to, Direction::Forward, EdgeKind::ImplementsAuth)? {
      schemes.extend(related(graph, &implemented, edge.to, implemented.to)?);
    }
    guards.push(GuardView { guard, schemes });
  }
  let mut auth = Vec::new();
  for edge in admitted(graph, endpoint.id, Direction::Forward, EdgeKind::SupportsAuth)? {
    let Some(scheme) = related(graph, &edge, endpoint.id, edge.to)? else { continue };
    auth.push(EndpointScheme {
      scheme,
      combinator: graph.edge_attrs(edge.id, &["combinator"])?.into_iter().next(),
    });
  }
  let mut consumers = Vec::new();
  for edge in admitted(graph, endpoint.id, Direction::Reverse, EdgeKind::CallsEndpoint)? {
    let Some(source) = graph.entity(edge.from)? else { continue };
    match source.kind {
      EntityKind::ApiClientCall => {
        for request in admitted(graph, source.id, Direction::Reverse, EdgeKind::MakesRequest)? {
          let Some(caller) = graph.entity(request.from)? else { continue };
          consumers.push(consumer(graph, &caller, &edge, Some(&source))?);
        }
      }
      _ => consumers.push(consumer(graph, &source, &edge, None)?),
    }
  }
  consumers.sort_by_key(|consumer| consumer.consumer.entity.sort_key());
  let mut inferred_consumers = Vec::new();
  for link in inferred_links(graph, endpoint.id, LinkEnd::Endpoint, hidden)? {
    for request in admitted(graph, link.call.id, Direction::Reverse, EdgeKind::MakesRequest)? {
      let Some(caller) = graph.entity(request.from)? else { continue };
      let mut view = consumer(graph, &caller, &link.edge, Some(&link.call))?;
      view.consumer.relation = link.hop.clone();
      inferred_consumers.push(view);
    }
  }
  Ok(EndpointView {
    endpoint: EntityRef::of(endpoint),
    method,
    route,
    certainty: CertaintyTag::of(&endpoint.certainty),
    handler,
    guards,
    auth,
    auth_attrs: graph.entity_attrs(endpoint.id, &ENDPOINT_AUTH_ATTRS)?,
    permissions,
    consumers,
    inferred_consumers,
  })
}

fn consumer(graph: &Graph<'_, '_>, consumer: &Entity, link: &atlas_core::Edge, client_call: Option<&Entity>) -> QueryResult<ConsumerView> {
  let relation = graph.relation(link, link.from)?;
  let steps = [
    Step::new(EdgeKind::Calls, Direction::Reverse, 1.0),
    Step::new(EdgeKind::Instantiates, Direction::Reverse, 1.0),
  ];
  let walk = graph.walk(
    &WalkSpec {
      seeds: &[consumer.id],
      steps: &steps,
      max_depth: CONSUMER_CALLER_DEPTH,
      max_nodes: MAX_NODES,
    },
    |_| true,
  )?;
  let mut callers = Vec::new();
  let mut involved = vec![consumer.clone()];
  for reached in &walk.reached {
    let (Some(entity), Some(last)) = (graph.entity(reached.id)?, reached.last_hop()) else {
      continue;
    };
    callers.push(Related {
      entity: EntityRef::of(&entity),
      relation: graph.relation(&last.edge, reached.id)?,
    });
    involved.push(entity);
  }
  callers.sort_by_key(|caller| caller.entity.sort_key());
  let mut auth_used = Vec::new();
  let mut repos = BTreeSet::new();
  let mut packages = BTreeSet::new();
  for entity in &involved {
    for edge in admitted(graph, entity.id, Direction::Forward, EdgeKind::UsesAuth)? {
      auth_used.extend(related(graph, &edge, entity.id, edge.to)?);
    }
    repos.extend(entity.repo.as_ref().map(ToString::to_string));
    packages.extend(package_key(entity).map(|key| EntityId::of(&key)));
  }
  let mut applications = Vec::new();
  for package in packages {
    for edge in graph.edges_out(package, &[EdgeKind::PartOfApp])? {
      applications.extend(graph.entity(edge.to)?.as_ref().map(EntityRef::of));
    }
  }
  applications.sort_by_key(EntityRef::sort_key);
  applications.dedup();
  Ok(ConsumerView {
    consumer: Related {
      entity: EntityRef::of(consumer),
      relation,
    },
    client_call: client_call.map(EntityRef::of),
    callers,
    repos: repos.into_iter().collect(),
    applications,
    auth_used,
  })
}

fn derived_support(graph: &Graph<'_, '_>, scheme: &Entity) -> QueryResult<Option<usize>> {
  let mut endpoints: BTreeSet<EntityId> = BTreeSet::new();
  let mut guards_known = false;
  for implemented in admitted(graph, scheme.id, Direction::Reverse, EdgeKind::ImplementsAuth)? {
    for required in admitted(graph, implemented.from, Direction::Reverse, EdgeKind::RequiresGuard)? {
      guards_known = true;
      endpoints.insert(required.from);
    }
  }
  if !guards_known && !any_guard_requirement(graph)? {
    return Ok(None);
  }
  Ok(Some(endpoints.len()))
}

fn any_guard_requirement(graph: &Graph<'_, '_>) -> QueryResult<bool> {
  for endpoint in graph.entities_of_kind(&EntityKind::ApiEndpoint)? {
    if !graph.edges_out(endpoint.id, &[EdgeKind::RequiresGuard])?.is_empty() {
      return Ok(true);
    }
  }
  Ok(false)
}

fn matrix(graph: &Graph<'_, '_>, repo: &str, uses_missing: bool, supports_known: bool) -> QueryResult<AuthMatrix> {
  let mut schemes = Vec::new();
  for scheme in graph.entities_of_kind(&EntityKind::AuthScheme)? {
    if scheme.repo.as_ref().is_none_or(|candidate| candidate.as_str() != repo) {
      continue;
    }
    let supported = match supports_known {
      true => Some(SchemeSupport {
        endpoints: admitted(graph, scheme.id, Direction::Reverse, EdgeKind::SupportsAuth)?.len(),
        basis: SupportBasis::SupportsAuth,
      }),
      false => derived_support(graph, &scheme)?.map(|endpoints| SchemeSupport {
        endpoints,
        basis: SupportBasis::DerivedFromGuards,
      }),
    };
    let mut implemented_by = Vec::new();
    for edge in admitted(graph, scheme.id, Direction::Reverse, EdgeKind::ImplementsAuth)? {
      implemented_by.extend(related(graph, &edge, edge.from, edge.from)?);
    }
    let mut used_by = Vec::new();
    for edge in admitted(graph, scheme.id, Direction::Reverse, EdgeKind::UsesAuth)? {
      used_by.extend(related(graph, &edge, edge.from, edge.from)?);
    }
    used_by.sort_by_key(|user| user.entity.sort_key());
    let used_by_repos: BTreeSet<String> = used_by.iter().map(|user| user.entity.repo().to_owned()).collect();
    let used = match (used_by.is_empty(), uses_missing) {
      (false, _) => UsageState::Used,
      (true, true) => UsageState::Unknown,
      (true, false) => UsageState::Unused,
    };
    schemes.push(SchemeUsage {
      scheme: EntityRef::of(&scheme),
      supported,
      implemented_by,
      used,
      used_by,
      used_by_repos: used_by_repos.into_iter().collect(),
      link_attrs: graph.entity_attrs(scheme.id, &SCHEME_LINK_ATTRS)?,
    });
  }
  schemes.sort_by(|left, right| left.scheme.key.cmp(&right.scheme.key));
  Ok(AuthMatrix { repo: repo.to_owned(), schemes })
}

pub(crate) fn site_linked(graph: &Graph<'_, '_>, call: &Entity, request: &atlas_core::Edge, site: Option<&atlas_core::Evidence>) -> QueryResult<bool> {
  Ok(client_call_site_linked(graph.tx(), call, request, site)?)
}

fn inferred_link_view(graph: &Graph<'_, '_>, call: &Entity, hidden: &mut LinkScan) -> QueryResult<Option<InferredLinkView>> {
  let Some(link) = inferred_links(graph, call.id, LinkEnd::Call, hidden)?.into_iter().next() else {
    return Ok(None);
  };
  Ok(Some(InferredLinkView {
    endpoint: EntityRef::of(&link.endpoint),
    relation: link.hop,
    rationale: link.rationale,
  }))
}

fn unmatched_calls(graph: &Graph<'_, '_>, calls_missing: bool, hidden: &mut LinkScan) -> QueryResult<Vec<ClientCallView>> {
  let mut unmatched = Vec::new();
  for call in graph.entities_of_kind(&EntityKind::ApiClientCall)? {
    let flagged = client_call_flagged_unmatched(&call);
    let inferred_link = inferred_link_view(graph, &call, hidden)?;
    for request in graph.edges_in(call.id, &[EdgeKind::MakesRequest])? {
      let sites: Vec<Option<&atlas_core::Evidence>> = match request.evidence.is_empty() {
        true => vec![None],
        false => request.evidence.iter().map(Some).collect(),
      };
      for site in sites {
        if !flagged && site_linked(graph, &call, &request, site)? {
          continue;
        }
        let reason = match (flagged, calls_missing) {
          (true, _) => "no endpoint in the project matches this call",
          (false, true) => "not linked: the index has no client->endpoint links",
          (false, false) => "no calls_endpoint link for this call site",
        };
        let Some(caller) = related(graph, &request, request.from, request.from)? else {
          continue;
        };
        unmatched.push(ClientCallView {
          call: EntityRef::of(&call),
          caller,
          site: site.map(|evidence| Location {
            repo: evidence.repo.to_string(),
            path: evidence.path.to_string(),
            line: Some(evidence.span.start_line),
            column: None,
          }),
          reason: reason.to_owned(),
          unmatched_reason: graph.entity_attrs(call.id, &["unmatched_reason"])?.into_iter().next(),
          inferred_link: inferred_link.clone(),
        });
      }
    }
  }
  unmatched.sort_by(|left, right| (&left.call.key, &left.site, &left.caller.entity.key).cmp(&(&right.call.key, &right.site, &right.caller.entity.key)));
  Ok(unmatched)
}
