use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{ATTR_TEST, Certainty, Edge, EdgeKind, Entity, EntityId, EntityKey, EntityKind, Evidence, LanguageFamily, RelPath, RepoName, Span, SymbolKey, is_test_code};
use serde_json::{Value, json};

use crate::compose::{Route, Routes, entity_evidence};
use crate::error::LinkResult;
use crate::facts::Facts;
use crate::http::{ClientLinks, Site};
use crate::output::{Output, absence, annotated, attr_str, confidence_of, derived, heuristic, relabel, rounded, weaker};
use crate::scopes::PackageIndex;

const MAX_DELEGATION_DEPTH: usize = 4;
const DIRECT_HINT_CONFIDENCE: f32 = 0.75;
const HELPER_HINT_CONFIDENCE: f32 = 0.7;
const UNKNOWN_DETAIL_FACTOR: f32 = 0.7;
const ANY_COOKIE_FACTOR: f32 = 0.8;
const OPTION_WITHOUT_HINTS_FACTOR: f32 = 0.8;
const RULE_NO_SCHEME: &str = "no-detected-auth-scheme";
const RULE_NAME_CLASSIFICATION: &str = "guard-name-classification";
const NAME_CLASSIFICATION_CONFIDENCE: f32 = 0.5;
const RULE_NO_CLIENT: &str = "no-detected-client";
const RULE_GUARD: &str = "guard-auth";
const RULE_COMPOSITE: &str = "composite-guard-auth";
const RULE_CLIENT: &str = "client-auth-hint";
const RULE_OPTION: &str = "sdk-auth-option";
const ATTR_ACCESS: &str = "auth";
const OPTIONAL: &str = "optional";
const ATTR_MARKER: &str = "auth_marker";
const CONTESTED_MARKER_CONFIDENCE: f32 = 0.5;
const RULE_CONTESTED_MARKER: &str = "class-marker-method-guard";
const ACCESS_MODES: [&str; 2] = ["public", OPTIONAL];
const COMBINATOR_PER_HANDLER: &str = "per_handler";
const AUTHORIZATION_GUARD_MARKERS: [&str; 7] = ["permission", "role", "polic", "abilit", "acl", "authoriz", "claim"];
const MAX_IMPLEMENTATION_SITES: usize = 3;

#[derive(Default)]
struct SchemeDetail {
  headers: BTreeSet<String>,
  cookies: BTreeSet<String>,
}

pub(crate) struct EndpointAuth {
  endpoints: BTreeMap<EntityId, String>,
  schemes: BTreeMap<EntityId, Entity>,
  details: BTreeMap<EntityId, SchemeDetail>,
  supported: BTreeMap<EntityId, BTreeSet<EntityId>>,
  support_certainties: BTreeMap<EntityId, Vec<Certainty>>,
}

struct Alternative {
  scheme: EntityId,
  confidence: f32,
  certainty: Certainty,
  via: Vec<String>,
  evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuardKind {
  Direct,
  Composite,
  AuthorizationOnly,
  Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Credential {
  Required,
  Optional,
}

struct GuardAuth {
  name: String,
  requires: Edge,
  alternatives: Vec<Alternative>,
  kind: GuardKind,
  classification: Vec<Certainty>,
  credential: Credential,
}

impl GuardAuth {
  fn enforced(&self) -> bool {
    self.credential == Credential::Required
  }

  fn counts(&self) -> bool {
    self.enforced() || self.names_scheme()
  }

  fn inputs(&self) -> impl Iterator<Item = &Certainty> {
    std::iter::once(&self.requires.certainty)
      .chain(self.alternatives.iter().map(|alternative| &alternative.certainty))
      .chain(&self.classification)
  }

  fn names_scheme(&self) -> bool {
    matches!(self.kind, GuardKind::Direct | GuardKind::Composite)
  }
}

struct Exposure {
  path: RelPath,
  package: Option<String>,
  access: Option<String>,
  marker_contested: bool,
  handlers: Vec<EntityId>,
  guards: Vec<GuardAuth>,
}

struct Derived {
  requirement: Option<Value>,
  combinator: Option<&'static str>,
}

#[derive(PartialEq, Eq)]
enum Part {
  Schemes(Vec<String>),
  Label(String),
}

struct GuardSite<'s> {
  repo: &'s RepoName,
  path: &'s RelPath,
  package: Option<&'s str>,
  packages: &'s PackageIndex,
  anchor: Option<Span>,
  handlers: Vec<Span>,
}

impl GuardSite<'_> {
  fn closeness(&self, evidence: &Evidence) -> u8 {
    let same_repo = evidence.repo == *self.repo;
    if same_repo && evidence.path == *self.path {
      return 0;
    }
    if same_repo && self.package.is_some() && self.packages.nearest(&evidence.repo, evidence.path.as_str()) == self.package {
      return 1;
    }
    match same_repo {
      true => 2,
      false => 3,
    }
  }

  fn nearness(&self, evidence: &Evidence) -> (u8, bool, u32) {
    let tier = self.closeness(evidence);
    if tier > 0 {
      return (tier, true, 0);
    }
    let span = evidence.span;
    let outside = !self
      .handlers
      .iter()
      .any(|handler| handler.start_byte <= span.start_byte && span.end_byte <= handler.end_byte);
    let distance = self.anchor.map_or(0, |anchor| anchor.start_byte.abs_diff(span.start_byte));
    (tier, outside, distance)
  }

  fn closest(&self, evidence: &[Evidence]) -> Vec<Evidence> {
    let Some(best) = evidence.iter().map(|evidence| self.closeness(evidence)).min() else {
      return Vec::new();
    };
    let mut chosen: Vec<&Evidence> = evidence.iter().filter(|evidence| self.closeness(evidence) == best).collect();
    chosen.sort_by_key(|evidence| (self.nearness(evidence), evidence.repo.as_str(), evidence.path.as_str(), evidence.span));
    chosen.dedup();
    chosen.into_iter().take(MAX_IMPLEMENTATION_SITES).cloned().collect()
  }
}

struct Support {
  confidence: f32,
  via: BTreeSet<String>,
  evidence: Vec<Evidence>,
  rule: &'static str,
}

enum Detail<'h> {
  None,
  Header(&'h str),
  Cookie(&'h str),
}

pub(crate) fn link_endpoint_auth(facts: &Facts<'_, '_>, routes: &Routes, packages: &PackageIndex, output: &mut Output) -> LinkResult<EndpointAuth> {
  let mut auth = EndpointAuth {
    endpoints: routes.all.iter().map(|route| (route.id, route.entity.key.to_string())).collect(),
    schemes: BTreeMap::new(),
    details: BTreeMap::new(),
    supported: BTreeMap::new(),
    support_certainties: BTreeMap::new(),
  };
  for scheme in facts.entities(&EntityKind::AuthScheme)? {
    let detail = auth.details.entry(scheme.id).or_default();
    for implementation in facts.edges_to(scheme.id, &EdgeKind::ImplementsAuth)? {
      detail.headers.extend(attr_str(&implementation.attrs, "header").map(str::to_ascii_lowercase));
      detail.cookies.extend(attr_str(&implementation.attrs, "cookie").map(str::to_owned));
    }
    auth.schemes.insert(scheme.id, scheme);
  }
  for route in &routes.all {
    let exposures = exposures(facts, packages, route)?;
    let per_file: Vec<Derived> = exposures.iter().map(|exposure| derive(&auth, exposure)).collect();
    let Some((requirement, combinator)) = combined(&exposures, &per_file) else { continue };
    let guards: Vec<&GuardAuth> = exposures.iter().flat_map(|exposure| &exposure.guards).filter(|guard| guard.counts()).collect();
    let enforced: Vec<&GuardAuth> = guards.iter().copied().filter(|guard| guard.enforced()).collect();
    let unclassified = guard_names(&exposures, GuardKind::Unknown);
    let authorization = guard_names(&exposures, GuardKind::AuthorizationOnly);
    let mut inputs: Vec<Certainty> = guards.iter().flat_map(|guard| guard.inputs()).cloned().collect();
    if enforced.iter().any(|guard| guard.kind == GuardKind::Unknown) {
      inputs.push(absence(RULE_NO_SCHEME)?);
    }
    if exposures.iter().any(|exposure| exposure.marker_contested) {
      inputs.push(heuristic(CONTESTED_MARKER_CONFIDENCE, RULE_CONTESTED_MARKER)?);
    }
    if enforced.iter().any(|guard| guard.kind == GuardKind::AuthorizationOnly) {
      inputs.push(heuristic(NAME_CLASSIFICATION_CONFIDENCE, RULE_NAME_CLASSIFICATION)?);
    }
    let certainty = derived(&route.entity.certainty, &inputs);
    let mut attrs = vec![("auth_requirement", requirement.clone())];
    if let Some(combinator) = combinator {
      attrs.push(("auth_combinator", json!(combinator)));
    }
    if !enforced.is_empty() {
      attrs.push(("unclassified_guards", json!(unclassified)));
      attrs.push(("authorization_guards", json!(authorization)));
    }
    output.extend_entity(&route.entity, certainty, attrs);
    let mut by_scheme: BTreeMap<EntityId, Support> = BTreeMap::new();
    for guard in guards.iter().filter(|guard| guard.names_scheme()) {
      for alternative in &guard.alternatives {
        let support = by_scheme.entry(alternative.scheme).or_insert_with(|| Support {
          confidence: 0.0,
          via: BTreeSet::new(),
          evidence: Vec::new(),
          rule: RULE_GUARD,
        });
        support.confidence = support.confidence.max(alternative.confidence.min(confidence_of(&guard.requires.certainty)));
        support.via.insert(alternative.via.join(" -> "));
        support
          .evidence
          .extend(guard.requires.evidence.iter().map(|evidence| relabel(evidence, Some("guard".to_owned()))));
        support.evidence.extend(alternative.evidence.iter().cloned());
        if guard.kind == GuardKind::Composite {
          support.rule = RULE_COMPOSITE;
        }
      }
    }
    for (scheme, mut support) in by_scheme {
      let mut seen = BTreeSet::new();
      support
        .evidence
        .retain(|evidence| seen.insert((evidence.repo.clone(), evidence.path.clone(), evidence.span, evidence.note.clone())));
      let certainty = heuristic(support.confidence, support.rule)?;
      auth.support_certainties.entry(scheme).or_default().push(certainty.clone());
      let mut edge = Edge::new(EdgeKind::SupportsAuth, route.id, scheme, "", certainty);
      edge.attrs.insert("rule".to_owned(), json!(support.rule));
      edge.attrs.insert("via_guards".to_owned(), json!(support.via));
      edge.attrs.insert("combinator".to_owned(), json!(combinator));
      edge.attrs.insert("requirement".to_owned(), requirement.clone());
      edge.attrs.insert("unclassified_guards".to_owned(), json!(unclassified));
      edge.attrs.insert("authorization_guards".to_owned(), json!(authorization));
      edge.evidence = support.evidence;
      output.edge(edge);
      auth.supported.entry(route.id).or_default().insert(scheme);
    }
  }
  Ok(auth)
}

fn exposures(facts: &Facts<'_, '_>, packages: &PackageIndex, route: &Route) -> LinkResult<Vec<Exposure>> {
  let mut by_file: BTreeMap<(RepoName, RelPath), Exposure> = BTreeMap::new();
  for (repo, path, asserted) in facts.file_assertions(route.guard_source)? {
    let exposure = exposure_at(&mut by_file, packages, repo, path);
    exposure.access = attr_str(&asserted.attrs, ATTR_ACCESS).filter(|mode| ACCESS_MODES.contains(mode)).map(str::to_owned);
    exposure.marker_contested = asserted.attrs.get(ATTR_MARKER).is_some_and(|marker| marker.get("contested_by").is_some());
  }
  for (repo, path, exposed) in facts.file_edges_to(route.guard_source, &EdgeKind::ExposesEndpoint)? {
    exposure_at(&mut by_file, packages, repo, path).handlers.push(exposed.from);
  }
  let mut requires = facts.file_edges_from(route.guard_source, &EdgeKind::RequiresGuard)?;
  requires.sort_by_key(|(_, _, edge)| (edge.attrs.get("order").and_then(Value::as_u64).unwrap_or(u64::MAX), edge.id));
  for (repo, path, edge) in requires {
    let exposure = exposure_at(&mut by_file, packages, repo.clone(), path.clone());
    let site = GuardSite {
      repo: &repo,
      path: &path,
      package: exposure.package.as_deref(),
      packages,
      anchor: edge
        .evidence
        .iter()
        .find(|evidence| evidence.repo == repo && evidence.path == path)
        .map(|evidence| evidence.span),
      handlers: handler_spans(facts, &exposure.handlers, &repo, &path)?,
    };
    if let Some(guard) = guard_auth(facts, edge, &exposure.handlers, &site)? {
      exposure.guards.push(guard);
    }
  }
  let mut exposures: Vec<Exposure> = by_file.into_values().collect();
  let in_route_packages = |exposure: &Exposure| exposure.package.as_ref().is_some_and(|package| route.packages.contains(package));
  if exposures.iter().any(in_route_packages) {
    exposures.retain(in_route_packages);
  }
  Ok(exposures)
}

fn handler_spans(facts: &Facts<'_, '_>, handlers: &[EntityId], repo: &RepoName, path: &RelPath) -> LinkResult<Vec<Span>> {
  let mut spans = Vec::new();
  for handler in handlers {
    let Some(entity) = facts.entity(*handler)? else { continue };
    if entity.repo.as_ref() == Some(repo) && entity.path.as_ref() == Some(path) {
      spans.extend(entity.span);
    }
  }
  Ok(spans)
}

fn exposure_at<'m>(by_file: &'m mut BTreeMap<(RepoName, RelPath), Exposure>, packages: &PackageIndex, repo: RepoName, path: RelPath) -> &'m mut Exposure {
  let package = packages.nearest(&repo, path.as_str()).map(str::to_owned);
  by_file.entry((repo, path.clone())).or_insert_with(|| Exposure {
    path,
    package,
    access: None,
    marker_contested: false,
    handlers: Vec::new(),
    guards: Vec::new(),
  })
}

fn guard_auth(facts: &Facts<'_, '_>, edge: Edge, handlers: &[EntityId], site: &GuardSite<'_>) -> LinkResult<Option<GuardAuth>> {
  let Some(guard) = facts.entity(edge.to)? else { return Ok(None) };
  let (alternatives, mut kind) = guard_alternatives(facts, &guard, site, 0)?;
  let mut classification = Vec::new();
  if alternatives.is_empty() {
    kind = GuardKind::Unknown;
    if is_authorization_guard(&guard) {
      classification = permission_certainties(facts, handlers)?;
      if !classification.is_empty() {
        kind = GuardKind::AuthorizationOnly;
      }
    }
  }
  let credential = match attr_str(&edge.attrs, ATTR_ACCESS) {
    Some(OPTIONAL) => Credential::Optional,
    _ => Credential::Required,
  };
  Ok(Some(GuardAuth {
    name: guard.name.clone(),
    requires: edge,
    alternatives,
    kind,
    classification,
    credential,
  }))
}

fn derive(auth: &EndpointAuth, exposure: &Exposure) -> Derived {
  let enforced: Vec<&GuardAuth> = exposure.guards.iter().filter(|guard| guard.enforced()).collect();
  if !enforced.is_empty() {
    let (requirement, combinator) = requirement(auth, &enforced);
    return Derived {
      requirement: Some(requirement),
      combinator: Some(combinator),
    };
  }
  let accepts_scheme = exposure.guards.iter().any(GuardAuth::names_scheme);
  Derived {
    requirement: exposure.access.as_ref().map(|mode| json!(mode)),
    combinator: exposure.access.as_deref().filter(|mode| *mode == OPTIONAL && accepts_scheme).map(|_| OPTIONAL),
  }
}

fn combined(exposures: &[Exposure], derived: &[Derived]) -> Option<(Value, Option<&'static str>)> {
  if derived.iter().all(|derived| derived.requirement.is_none()) {
    return None;
  }
  let first = derived.first()?;
  if derived.iter().all(|other| other.requirement == first.requirement) {
    return Some((first.requirement.clone()?, first.combinator));
  }
  let handlers: Vec<Value> = exposures
    .iter()
    .zip(derived)
    .map(|(exposure, derived)| {
      let mut handler = json!({ "file": exposure.path.as_str(), "requirement": derived.requirement });
      if let Some(package) = &exposure.package {
        handler["package"] = json!(package);
      }
      handler
    })
    .collect();
  Some((json!({ "per_handler": handlers }), Some(COMBINATOR_PER_HANDLER)))
}

fn is_authorization_guard(guard: &Entity) -> bool {
  let name = guard.name.to_ascii_lowercase();
  AUTHORIZATION_GUARD_MARKERS.iter().any(|marker| name.contains(marker))
}

fn permission_certainties(facts: &Facts<'_, '_>, handlers: &[EntityId]) -> LinkResult<Vec<Certainty>> {
  let mut certainties = Vec::new();
  for handler in handlers {
    certainties.extend(facts.edges_from(*handler, &EdgeKind::RequiresPermission)?.into_iter().map(|edge| edge.certainty));
  }
  Ok(certainties)
}

fn guard_names(exposures: &[Exposure], kind: GuardKind) -> Vec<&str> {
  let mut names: Vec<&str> = Vec::new();
  for exposure in exposures {
    let earlier = names.len();
    for guard in exposure.guards.iter().filter(|guard| guard.enforced() && guard.kind == kind) {
      if !names[..earlier].contains(&guard.name.as_str()) {
        names.push(&guard.name);
      }
    }
  }
  names
}

fn guard_alternatives(facts: &Facts<'_, '_>, guard: &Entity, site: &GuardSite<'_>, depth: usize) -> LinkResult<(Vec<Alternative>, GuardKind)> {
  let mut alternatives = Vec::new();
  for implementation in facts.edges_from(guard.id, &EdgeKind::ImplementsAuth)? {
    alternatives.push(Alternative {
      scheme: implementation.to,
      confidence: confidence_of(&implementation.certainty),
      certainty: implementation.certainty.clone(),
      via: vec![guard.name.clone()],
      evidence: site
        .closest(&implementation.evidence)
        .iter()
        .map(|evidence| relabel(evidence, Some("implements auth".to_owned())))
        .collect(),
    });
  }
  if !alternatives.is_empty() || depth >= MAX_DELEGATION_DEPTH {
    return Ok((alternatives, GuardKind::Direct));
  }
  for delegation in facts.edges_from(guard.id, &EdgeKind::References)? {
    if delegation.attrs.get("delegates_to") != Some(&json!(true)) {
      continue;
    }
    let Some(delegate) = facts.entity(delegation.to)? else { continue };
    let (inner, _) = guard_alternatives(facts, &delegate, site, depth + 1)?;
    for alternative in inner {
      let mut via = vec![guard.name.clone()];
      via.extend(alternative.via);
      let mut evidence: Vec<Evidence> = delegation.evidence.iter().map(|evidence| relabel(evidence, Some("delegates".to_owned()))).collect();
      evidence.extend(alternative.evidence);
      alternatives.push(Alternative {
        scheme: alternative.scheme,
        confidence: alternative.confidence.min(confidence_of(&delegation.certainty)),
        certainty: weaker(&alternative.certainty, &delegation.certainty),
        via,
        evidence,
      });
    }
  }
  Ok((alternatives, GuardKind::Composite))
}

fn distinct_schemes(guard: &GuardAuth) -> BTreeSet<EntityId> {
  guard.alternatives.iter().map(|alternative| alternative.scheme).collect()
}

fn scheme_name(auth: &EndpointAuth, scheme: EntityId) -> String {
  auth.schemes.get(&scheme).map_or_else(|| scheme.to_string(), |entity| entity.name.clone())
}

fn requirement(auth: &EndpointAuth, guards: &[&GuardAuth]) -> (Value, &'static str) {
  let mut parts: Vec<Part> = Vec::new();
  for guard in guards {
    let part = match guard.kind {
      GuardKind::AuthorizationOnly => Part::Label(format!("authorization_only:{}", guard.name)),
      GuardKind::Unknown => Part::Label(format!("unknown_guard:{}", guard.name)),
      GuardKind::Direct | GuardKind::Composite => {
        let names: BTreeSet<String> = distinct_schemes(guard).into_iter().map(|scheme| scheme_name(auth, scheme)).collect();
        Part::Schemes(names.into_iter().collect())
      }
    };
    if parts.contains(&part) {
      continue;
    }
    parts.push(part);
  }
  if let [Part::Schemes(schemes)] = parts.as_slice()
    && schemes.len() > 1
  {
    return (json!({ "any_of": schemes }), "any_of");
  }
  let parts: Vec<Value> = parts
    .into_iter()
    .map(|part| match part {
      Part::Label(label) => json!(label),
      Part::Schemes(schemes) => match schemes.as_slice() {
        [one] => json!(one),
        many => json!({ "any_of": many }),
      },
    })
    .collect();
  (json!({ "all_of": parts }), "all_of")
}

impl EndpointAuth {
  fn matching_schemes(&self, endpoint: EntityId, hint: &str) -> Vec<(EntityId, f32)> {
    let Some(supported) = self.supported.get(&endpoint) else { return Vec::new() };
    let Some((kind, detail)) = parse_hint(hint) else { return Vec::new() };
    supported
      .iter()
      .filter(|scheme| self.scheme_kind(**scheme) == Some(kind))
      .filter_map(|scheme| {
        let known = self.details.get(scheme);
        let factor = match detail {
          Detail::None => 1.0,
          Detail::Header(header) => detail_factor(known.map(|detail| &detail.headers), &header.to_ascii_lowercase())?,
          Detail::Cookie("*") => ANY_COOKIE_FACTOR,
          Detail::Cookie(cookie) => detail_factor(known.map(|detail| &detail.cookies), cookie)?,
        };
        Some((*scheme, factor))
      })
      .collect()
  }

  fn scheme_kind(&self, scheme: EntityId) -> Option<&str> {
    match &self.schemes.get(&scheme)?.key {
      EntityKey::AuthScheme { scheme, .. } => Some(scheme.as_str()),
      _ => None,
    }
  }

  fn schemes_of_kind(&self, endpoint: EntityId, kind: &str) -> Vec<EntityId> {
    self
      .supported
      .get(&endpoint)
      .into_iter()
      .flatten()
      .filter(|scheme| self.scheme_kind(**scheme) == Some(kind))
      .copied()
      .collect()
  }
}

fn detail_factor(known: Option<&BTreeSet<String>>, value: &str) -> Option<f32> {
  match known {
    Some(values) if !values.is_empty() => values.contains(value).then_some(1.0),
    _ => Some(UNKNOWN_DETAIL_FACTOR),
  }
}

fn parse_hint(hint: &str) -> Option<(&'static str, Detail<'_>)> {
  match hint.split_once(':') {
    Some(("api_key", header)) => Some(("api_key", Detail::Header(header))),
    Some(("cookie", cookie)) => Some(("session_cookie", Detail::Cookie(cookie))),
    Some(_) => None,
    None => match hint {
      "bearer" => Some(("bearer", Detail::None)),
      "basic" => Some(("basic", Detail::None)),
      _ => None,
    },
  }
}

fn hints(site: &Site) -> Vec<&str> {
  match site.attrs.get("auth_hint") {
    Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).collect(),
    _ => Vec::new(),
  }
}

#[derive(Default)]
struct Usage {
  repo: Option<RepoName>,
  confidence: f32,
  sources: BTreeSet<&'static str>,
  hints: BTreeSet<String>,
  endpoints: BTreeSet<EntityId>,
  evidence: Vec<Evidence>,
}

#[derive(Default)]
struct Consumption {
  repos: BTreeSet<RepoName>,
  evidence_count: usize,
  certainties: Vec<Certainty>,
}

struct UsageSource<'a> {
  from: &'a Entity,
  label: &'static str,
  hint: &'a str,
  endpoint: EntityId,
  confidence: f32,
  evidence: Vec<Evidence>,
}

pub(crate) fn link_client_auth(facts: &Facts<'_, '_>, clients: &ClientLinks, auth: &EndpointAuth, output: &mut Output) -> LinkResult<()> {
  let mut usages: BTreeMap<(EntityId, EntityId), Usage> = BTreeMap::new();
  for site in &clients.sites {
    direct_usages(facts, site, auth, &mut usages)?;
  }
  option_usages(facts, clients, auth, &mut usages)?;
  let mut consumers: BTreeMap<EntityId, Consumption> = BTreeMap::new();
  for ((from, scheme), usage) in usages {
    let rule = if usage.sources.iter().any(|source| *source != RULE_OPTION) {
      RULE_CLIENT
    } else {
      RULE_OPTION
    };
    let certainty = heuristic(usage.confidence, rule)?;
    let mut edge = Edge::new(EdgeKind::UsesAuth, from, scheme, "", certainty.clone());
    edge.attrs.insert("sources".to_owned(), json!(usage.sources));
    edge.attrs.insert("hints".to_owned(), json!(usage.hints));
    let endpoints: BTreeSet<&str> = usage.endpoints.iter().filter_map(|endpoint| auth.endpoints.get(endpoint).map(String::as_str)).collect();
    edge.attrs.insert("endpoints".to_owned(), json!(endpoints));
    edge.evidence = usage.evidence;
    let entry = consumers.entry(scheme).or_default();
    entry.repos.extend(usage.repo);
    entry.evidence_count += edge.evidence.len();
    entry.certainties.push(certainty);
    output.edge(edge);
  }
  for (id, scheme) in &auth.schemes {
    let Consumption {
      repos,
      evidence_count,
      certainties,
    } = consumers.remove(id).unwrap_or_default();
    let endpoints = auth.supported.values().filter(|schemes| schemes.contains(id)).count();
    let supports = auth.support_certainties.get(id).into_iter().flatten();
    let mut certainty = derived(&scheme.certainty, certainties.iter().chain(supports));
    if certainties.is_empty() {
      certainty = weaker(&certainty, &absence(RULE_NO_CLIENT)?);
    }
    output.entity(annotated(
      scheme,
      certainty,
      [
        ("consumers", json!(repos.iter().map(RepoName::as_str).collect::<Vec<_>>())),
        ("used", json!(!repos.is_empty())),
        ("evidence_count", json!(evidence_count)),
        ("supported_by_endpoints", json!(endpoints)),
      ],
    ));
  }
  Ok(())
}

fn direct_usages(facts: &Facts<'_, '_>, site: &Site, auth: &EndpointAuth, usages: &mut BTreeMap<(EntityId, EntityId), Usage>) -> LinkResult<()> {
  let hints = hints(site);
  if hints.is_empty() || site.links.is_empty() {
    return Ok(());
  }
  let helper = match attr_str(&site.attrs, "auth_hint_source") {
    Some(source) => helper_symbol(facts, &site.caller, source)?,
    None => None,
  };
  let (from, label, base) = match &helper {
    Some(helper) => (helper, "helper", HELPER_HINT_CONFIDENCE),
    None => (&site.caller, "header", DIRECT_HINT_CONFIDENCE),
  };
  if in_test_code(facts, from)? {
    return Ok(());
  }
  let mut evidence = vec![relabel(&site.evidence, Some("request".to_owned()))];
  if let Some(helper) = &helper {
    evidence.extend(entity_evidence(facts, helper, Some("auth header helper".to_owned()))?);
  }
  for link in &site.links {
    for hint in &hints {
      for (scheme, factor) in auth.matching_schemes(link.endpoint, hint) {
        record(
          usages,
          scheme,
          UsageSource {
            from,
            label,
            hint,
            endpoint: link.endpoint,
            confidence: base * factor * link.share,
            evidence: evidence.clone(),
          },
        );
      }
    }
  }
  Ok(())
}

fn option_usages(facts: &Facts<'_, '_>, clients: &ClientLinks, auth: &EndpointAuth, usages: &mut BTreeMap<(EntityId, EntityId), Usage>) -> LinkResult<()> {
  let mut classes: BTreeMap<EntityId, (Entity, Vec<&Site>)> = BTreeMap::new();
  for site in &clients.sites {
    if let Some(class) = owning_class(facts, &site.caller)? {
      classes.entry(class.id).or_insert_with(|| (class, Vec::new())).1.push(site);
    }
  }
  for (class, sites) in classes.values() {
    let hint_kinds: BTreeSet<&str> = sites
      .iter()
      .flat_map(|site| hints(site))
      .filter_map(|hint| parse_hint(hint).map(|(kind, _)| kind))
      .collect();
    let mut shares: BTreeMap<EntityId, f32> = BTreeMap::new();
    for link in sites.iter().flat_map(|site| &site.links) {
      let share = shares.entry(link.endpoint).or_default();
      *share = share.max(link.share);
    }
    for option in facts.edges_to(class.id, &EdgeKind::References)? {
      let Some(kind) = attr_str(&option.attrs, "uses_auth_option") else { continue };
      let consistency = match (hint_kinds.is_empty(), hint_kinds.contains(kind)) {
        (true, _) => OPTION_WITHOUT_HINTS_FACTOR,
        (false, true) => 1.0,
        (false, false) => continue,
      };
      let Some(from) = facts.entity(option.from)? else { continue };
      if in_test_code(facts, &from)? {
        continue;
      }
      let evidence: Vec<Evidence> = option.evidence.iter().map(|evidence| relabel(evidence, Some(format!("auth option {kind}")))).collect();
      for (endpoint, share) in &shares {
        for scheme in auth.schemes_of_kind(*endpoint, kind) {
          record(
            usages,
            scheme,
            UsageSource {
              from: &from,
              label: RULE_OPTION,
              hint: kind,
              endpoint: *endpoint,
              confidence: confidence_of(&option.certainty) * consistency * share,
              evidence: evidence.clone(),
            },
          );
        }
      }
    }
  }
  Ok(())
}

fn in_test_code(facts: &Facts<'_, '_>, entity: &Entity) -> LinkResult<bool> {
  if is_test_code(entity) {
    return Ok(true);
  }
  let Some(key) = entity.key.as_symbol().filter(|key| key.family() == LanguageFamily::Rust) else {
    return Ok(false);
  };
  let modules = key.module_path();
  for depth in 1..=modules.len() {
    let Ok(module) = SymbolKey::new(
      LanguageFamily::Rust,
      key.repo().clone(),
      key.package(),
      modules[..depth].to_vec(),
      Vec::new(),
      EntityKind::Module,
      0,
    ) else {
      continue;
    };
    if facts
      .entity(EntityId::of(&EntityKey::Symbol(module)))?
      .is_some_and(|module| module.attrs.contains_key(ATTR_TEST))
    {
      return Ok(true);
    }
  }
  Ok(false)
}

fn record(usages: &mut BTreeMap<(EntityId, EntityId), Usage>, scheme: EntityId, source: UsageSource<'_>) {
  let usage = usages.entry((source.from.id, scheme)).or_default();
  usage.repo = source.from.repo.clone();
  usage.confidence = usage.confidence.max(rounded(source.confidence));
  usage.sources.insert(source.label);
  usage.hints.insert(source.hint.to_owned());
  usage.endpoints.insert(source.endpoint);
  usage.evidence.extend(source.evidence);
}

fn helper_symbol(facts: &Facts<'_, '_>, caller: &Entity, qualified: &str) -> LinkResult<Option<Entity>> {
  let (Some(repo), Some(path)) = (&caller.repo, &caller.path) else { return Ok(None) };
  Ok(facts.entities_in_file(repo, path)?.into_iter().find(|entity| {
    entity.key.as_symbol().is_some_and(|key| {
      let items = key.item_path();
      items.join(".") == qualified || items.join("::") == qualified
    })
  }))
}

fn owning_class(facts: &Facts<'_, '_>, caller: &Entity) -> LinkResult<Option<Entity>> {
  let Some(key) = caller.key.as_symbol() else { return Ok(None) };
  let Some((_, parent)) = key.item_path().split_last() else { return Ok(None) };
  if parent.is_empty() || !matches!(key.kind(), EntityKind::Method) {
    return Ok(None);
  }
  for kind in [EntityKind::Class, EntityKind::Struct] {
    let candidate = SymbolKey::new(key.family(), key.repo().clone(), key.package(), key.module_path().to_vec(), parent.to_vec(), kind, 0)?;
    if let Some(class) = facts.entity(EntityId::of(&EntityKey::Symbol(candidate)))? {
      return Ok(Some(class));
    }
  }
  Ok(None)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn auth() -> EndpointAuth {
    EndpointAuth {
      endpoints: BTreeMap::new(),
      schemes: BTreeMap::new(),
      details: BTreeMap::new(),
      supported: BTreeMap::new(),
      support_certainties: BTreeMap::new(),
    }
  }

  fn unknown(name: &str, target: &str) -> GuardAuth {
    let id = |key: &str| {
      EntityId::of(&EntityKey::Other {
        kind: EntityKind::Custom("guard_decorator".to_owned()),
        repo: None,
        name: key.to_owned(),
      })
    };
    GuardAuth {
      name: name.to_owned(),
      requires: Edge::new(EdgeKind::RequiresGuard, id("route"), id(target), "", Certainty::Syntactic),
      alternatives: Vec::new(),
      kind: GuardKind::Unknown,
      classification: Vec::new(),
      credential: Credential::Required,
    }
  }

  #[test]
  fn should_list_an_unknown_guard_once_when_two_guards_share_its_name() {
    let first = unknown("SessionGuard", "a/SessionGuard");
    let second = unknown("SessionGuard", "b/SessionGuard");
    let other = unknown("AdminGuard", "AdminGuard");
    assert_eq!(
      requirement(&auth(), &[&first, &second, &other]),
      (json!({ "all_of": ["unknown_guard:SessionGuard", "unknown_guard:AdminGuard"] }), "all_of")
    );
  }
}
