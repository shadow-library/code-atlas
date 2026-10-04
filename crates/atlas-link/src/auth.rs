use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{ATTR_TEST, Certainty, Edge, EdgeKind, Entity, EntityId, EntityKey, EntityKind, Evidence, LanguageFamily, RepoName, SymbolKey, is_test_code};
use serde_json::{Value, json};

use crate::compose::{Routes, entity_evidence};
use crate::error::LinkResult;
use crate::facts::Facts;
use crate::http::{ClientLinks, Site};
use crate::output::{Output, absence, annotated, attr_str, confidence_of, derived, heuristic, relabel, rounded, weaker};

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
const AUTHORIZATION_GUARD_MARKERS: [&str; 7] = ["permission", "role", "polic", "abilit", "acl", "authoriz", "claim"];

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

struct GuardAuth {
  name: String,
  requires: Edge,
  alternatives: Vec<Alternative>,
  kind: GuardKind,
  classification: Vec<Certainty>,
}

impl GuardAuth {
  fn inputs(&self) -> impl Iterator<Item = &Certainty> {
    std::iter::once(&self.requires.certainty)
      .chain(self.alternatives.iter().map(|alternative| &alternative.certainty))
      .chain(&self.classification)
  }

  fn names_scheme(&self) -> bool {
    matches!(self.kind, GuardKind::Direct | GuardKind::Composite)
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

pub(crate) fn link_endpoint_auth(facts: &Facts<'_, '_>, routes: &Routes, output: &mut Output) -> LinkResult<EndpointAuth> {
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
    let guards = endpoint_guards(facts, route.guard_source)?;
    if guards.is_empty() {
      continue;
    }
    let requirement = requirement(&auth, &guards);
    let combinator = match guards.as_slice() {
      [single] if distinct_schemes(single).len() > 1 => "any_of",
      _ => "all_of",
    };
    let unclassified = guard_names(&guards, GuardKind::Unknown);
    let authorization = guard_names(&guards, GuardKind::AuthorizationOnly);
    let mut inputs: Vec<Certainty> = guards.iter().flat_map(GuardAuth::inputs).cloned().collect();
    if guards.iter().any(|guard| guard.kind == GuardKind::Unknown) {
      inputs.push(absence(RULE_NO_SCHEME)?);
    }
    if guards.iter().any(|guard| guard.kind == GuardKind::AuthorizationOnly) {
      inputs.push(heuristic(NAME_CLASSIFICATION_CONFIDENCE, RULE_NAME_CLASSIFICATION)?);
    }
    let certainty = derived(&route.entity.certainty, &inputs);
    output.extend_entity(
      &route.entity,
      certainty,
      [
        ("auth_requirement", requirement.clone()),
        ("auth_combinator", json!(combinator)),
        ("unclassified_guards", json!(unclassified)),
        ("authorization_guards", json!(authorization)),
      ],
    );
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
    for (scheme, support) in by_scheme {
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

fn endpoint_guards(facts: &Facts<'_, '_>, endpoint: EntityId) -> LinkResult<Vec<GuardAuth>> {
  let mut requires = facts.edges_from(endpoint, &EdgeKind::RequiresGuard)?;
  requires.sort_by_key(|edge| (edge.attrs.get("order").and_then(Value::as_u64).unwrap_or(u64::MAX), edge.id));
  let mut guards = Vec::new();
  for edge in requires {
    let Some(guard) = facts.entity(edge.to)? else { continue };
    let (alternatives, mut kind) = guard_alternatives(facts, &guard, 0)?;
    let mut classification = Vec::new();
    if alternatives.is_empty() {
      kind = GuardKind::Unknown;
      if is_authorization_guard(&guard) {
        classification = permission_certainties(facts, endpoint)?;
        if !classification.is_empty() {
          kind = GuardKind::AuthorizationOnly;
        }
      }
    }
    guards.push(GuardAuth {
      name: guard.name.clone(),
      requires: edge,
      alternatives,
      kind,
      classification,
    });
  }
  Ok(guards)
}

fn is_authorization_guard(guard: &Entity) -> bool {
  let name = guard.name.to_ascii_lowercase();
  AUTHORIZATION_GUARD_MARKERS.iter().any(|marker| name.contains(marker))
}

fn permission_certainties(facts: &Facts<'_, '_>, endpoint: EntityId) -> LinkResult<Vec<Certainty>> {
  let mut certainties = Vec::new();
  for exposure in facts.edges_to(endpoint, &EdgeKind::ExposesEndpoint)? {
    certainties.extend(facts.edges_from(exposure.from, &EdgeKind::RequiresPermission)?.into_iter().map(|edge| edge.certainty));
  }
  Ok(certainties)
}

fn guard_names(guards: &[GuardAuth], kind: GuardKind) -> Vec<&str> {
  guards.iter().filter(|guard| guard.kind == kind).map(|guard| guard.name.as_str()).collect()
}

fn guard_alternatives(facts: &Facts<'_, '_>, guard: &Entity, depth: usize) -> LinkResult<(Vec<Alternative>, GuardKind)> {
  let mut alternatives = Vec::new();
  for implementation in facts.edges_from(guard.id, &EdgeKind::ImplementsAuth)? {
    alternatives.push(Alternative {
      scheme: implementation.to,
      confidence: confidence_of(&implementation.certainty),
      certainty: implementation.certainty.clone(),
      via: vec![guard.name.clone()],
      evidence: implementation
        .evidence
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
    let (inner, _) = guard_alternatives(facts, &delegate, depth + 1)?;
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

fn requirement(auth: &EndpointAuth, guards: &[GuardAuth]) -> Value {
  let group = |guard: &GuardAuth| -> Vec<String> {
    let names: BTreeSet<String> = distinct_schemes(guard).into_iter().map(|scheme| scheme_name(auth, scheme)).collect();
    names.into_iter().collect()
  };
  if let [single] = guards
    && distinct_schemes(single).len() > 1
  {
    return json!({ "any_of": group(single) });
  }
  let parts: Vec<Value> = guards
    .iter()
    .map(|guard| match guard.kind {
      GuardKind::AuthorizationOnly => json!(format!("authorization_only:{}", guard.name)),
      GuardKind::Unknown => json!(format!("unknown_guard:{}", guard.name)),
      GuardKind::Direct | GuardKind::Composite => match group(guard).as_slice() {
        [one] => json!(one),
        many => json!({ "any_of": many }),
      },
    })
    .collect();
  json!({ "all_of": parts })
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
