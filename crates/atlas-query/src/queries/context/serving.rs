use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{AttributeFact, Edge, EdgeKind, Entity, EntityId, EntityKey, EntityKind, Evidence, Span};

use super::{Basis, Node, Pack, Role, neighbours, production_users_where};
use crate::error::QueryResult;
use crate::graph::{Graph, certainty_value};
use crate::model::short_name;
use crate::policy::Direction::{Forward, Reverse};
use crate::queries::common::{attributes, file_entity, is_test, package_key, parameter_attributes};

const ROUTE_FACTOR: f64 = 0.7;
const DECORATOR_FACTOR: f64 = 0.35;
const CLASS_DECORATOR_FACTOR: f64 = 0.25;
const IMPLEMENTATION_HOPS: usize = 3;
const SHARED_KEY_FAN_IN: usize = 6;
const CONSUMERS_PER_DECORATOR: usize = 3;
const CONSUMERS: usize = 6;
const TYPE_KEY_FACTOR: f64 = 0.25;
const SHARING_EDGES: [EdgeKind; 2] = [EdgeKind::Calls, EdgeKind::References];

#[derive(Clone, Copy)]
struct Trust {
  certainty: f64,
  authoritative: bool,
}

impl Trust {
  fn of(edge: &Edge) -> Self {
    Self {
      certainty: certainty_value(&edge.certainty),
      authoritative: edge.certainty.is_authoritative(),
    }
  }

  fn then(self, edge: &Edge) -> Self {
    let next = Self::of(edge);
    Self {
      certainty: self.certainty * next.certainty,
      authoritative: self.authoritative && next.authoritative,
    }
  }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Site {
  Route,
  Method,
  Parameter,
  Class,
}

impl Site {
  fn factor(self) -> f64 {
    match self {
      Self::Route => ROUTE_FACTOR,
      Self::Method | Self::Parameter => DECORATOR_FACTOR,
      Self::Class => CLASS_DECORATOR_FACTOR,
    }
  }

  fn label(self) -> &'static str {
    match self {
      Self::Route => "route decorator",
      Self::Method => "decorator",
      Self::Parameter => "parameter decorator",
      Self::Class => "class decorator",
    }
  }
}

struct Decorator {
  applied: Entity,
  target: Entity,
  site: Site,
  owner: String,
  trust: Trust,
}

struct Walk {
  decorator: Decorator,
  implementation: BTreeMap<EntityId, Reached>,
}

struct Reached {
  entity: Entity,
  trust: Trust,
}

struct Shared {
  key: String,
  edge: Edge,
  weight: f64,
  authoritative: bool,
}

struct Consumer {
  entity: Entity,
  score: f64,
  best: Shared,
}

pub(super) fn serving_path(graph: &Graph<'_, '_>, pack: &mut Pack, handler: &Node, exposes: &Edge) -> QueryResult<()> {
  let mut walks = Vec::new();
  for decorator in decorators(graph, pack, &handler.entity, &exposes.evidence)? {
    let implementation = implementation(graph, pack, &decorator)?;
    walks.push(Walk { decorator, implementation });
  }
  let walked: BTreeSet<EntityId> = walks
    .iter()
    .flat_map(|walk| [walk.decorator.applied.id, walk.decorator.target.id].into_iter().chain(walk.implementation.keys().copied()))
    .collect();
  let mut chosen: BTreeMap<EntityId, (f64, &Walk, Consumer)> = BTreeMap::new();
  for walk in &walks {
    for consumer in consumers(graph, pack, walk, &walked, handler.entity.id)? {
      let factor = walk.decorator.site.factor() * consumer.score.min(1.0);
      let stronger = |(best, _, held): &(f64, &Walk, Consumer)| factor.total_cmp(best).then(consumer.score.total_cmp(&held.score)).is_gt();
      if chosen.get(&consumer.entity.id).is_none_or(stronger) {
        chosen.insert(consumer.entity.id, (factor, walk, consumer));
      }
    }
  }
  let mut chosen: Vec<(f64, &Walk, Consumer)> = chosen.into_values().collect();
  chosen.sort_by(|left, right| right.0.total_cmp(&left.0).then_with(|| left.2.entity.key.cmp(&right.2.entity.key)));
  chosen.truncate(CONSUMERS);
  for (factor, walk, consumer) in chosen {
    add_consumer(graph, pack, handler, &walk.decorator, consumer, factor)?;
  }
  Ok(())
}

fn add_consumer(graph: &Graph<'_, '_>, pack: &mut Pack, handler: &Node, decorator: &Decorator, consumer: Consumer, factor: f64) -> QueryResult<()> {
  let via = graph.relation(&consumer.best.edge, consumer.entity.id)?;
  let why = format!(
    "shares {} with the {} {} of {}: {}",
    consumer.best.key,
    decorator.site.label(),
    short_name(&decorator.applied.key),
    decorator.owner,
    crate::text::hop_text(&via)
  );
  pack
    .expansion_log
    .push(format!("{} -> {} ({why}, weight {:.3})", handler.entity.key, consumer.entity.key, handler.weight * factor));
  pack.links.push((handler.entity.id, consumer.entity.id, factor));
  pack.insert(Node {
    role: super::role_of(&consumer.entity, Role::Code),
    entity: consumer.entity,
    basis: Basis::Relation,
    weight: handler.weight * factor,
    uncertain: handler.uncertain || !consumer.best.authoritative,
    why,
    via: Some(via),
    site: None,
    terms: Vec::new(),
    round: handler.round,
    named: false,
    seed: false,
    near_named: handler.named,
    reuse: None,
    hub_factor: handler.hub_factor,
  });
  Ok(())
}

fn decorators(graph: &Graph<'_, '_>, pack: &mut Pack, handler: &Entity, route_sites: &[Evidence]) -> QueryResult<Vec<Decorator>> {
  let handler_name = short_name(&handler.key);
  let on_method = attributes(handler);
  let on_parameters = parameter_attributes(handler);
  let mut found: BTreeMap<EntityId, Decorator> = BTreeMap::new();
  if !on_method.is_empty() || !on_parameters.is_empty() {
    for (edge, callee) in neighbours(graph, pack, handler.id, Forward, &[EdgeKind::Calls])? {
      let site = match () {
        _ if decorator_site(&on_method, &callee.name, &edge) && at_route_site(&edge, route_sites) => Site::Route,
        _ if decorator_site(&on_method, &callee.name, &edge) => Site::Method,
        _ if decorator_site(&on_parameters, &callee.name, &edge) => Site::Parameter,
        _ => continue,
      };
      let decorator = applied_decorator(graph, pack, callee, site, &handler_name, Trust::of(&edge))?;
      keep_strongest(&mut found, decorator);
    }
  }
  for (contains, class) in neighbours(graph, pack, handler.id, Reverse, &[EdgeKind::Contains])? {
    let on_class = attributes(&class);
    if class.kind != EntityKind::Class || on_class.is_empty() {
      continue;
    }
    let class_name = short_name(&class.key);
    for (edge, callee) in neighbours(graph, pack, class.id, Forward, &[EdgeKind::Calls])? {
      if decorator_site(&on_class, &callee.name, &edge) {
        let decorator = applied_decorator(graph, pack, callee, Site::Class, &class_name, Trust::of(&contains).then(&edge))?;
        keep_strongest(&mut found, decorator);
      }
    }
  }
  let mut decorators: Vec<Decorator> = found.into_values().collect();
  decorators.sort_by(|left, right| left.site.cmp(&right.site).then_with(|| left.target.key.cmp(&right.target.key)));
  Ok(decorators)
}

fn keep_strongest(found: &mut BTreeMap<EntityId, Decorator>, decorator: Option<Decorator>) {
  let Some(decorator) = decorator else { return };
  let stronger = |existing: &Decorator| (decorator.site, -decorator.trust.certainty) < (existing.site, -existing.trust.certainty);
  if found.get(&decorator.target.id).is_none_or(stronger) {
    found.insert(decorator.target.id, decorator);
  }
}

fn applied_decorator(graph: &Graph<'_, '_>, pack: &mut Pack, applied: Entity, site: Site, owner: &str, trust: Trust) -> QueryResult<Option<Decorator>> {
  if is_test(&applied) {
    return Ok(None);
  }
  let owner = owner.to_owned();
  if is_callable(&applied) {
    let target = applied.clone();
    return Ok(Some(Decorator {
      applied,
      target,
      site,
      owner,
      trust,
    }));
  }
  let Some(name) = aliased_function(&applied).map(str::to_owned) else { return Ok(None) };
  for (edge, target) in neighbours(graph, pack, applied.id, Forward, &SHARING_EDGES)? {
    if target.name == name && is_callable(&target) && !is_test(&target) {
      let trust = trust.then(&edge);
      return Ok(Some(Decorator {
        applied,
        target,
        site,
        owner,
        trust,
      }));
    }
  }
  Ok(None)
}

fn is_callable(entity: &Entity) -> bool {
  matches!(entity.kind, EntityKind::Function | EntityKind::Method)
}

fn aliased_function(constant: &Entity) -> Option<&str> {
  if constant.kind != EntityKind::Constant {
    return None;
  }
  let (_, value) = constant.signature.as_deref()?.rsplit_once(" = ")?;
  let value = value.trim();
  let mut chars = value.chars();
  let first = chars.next()?;
  let identifier = (first.is_alphabetic() || matches!(first, '_' | '$')) && chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | '$'));
  identifier.then_some(value)
}

fn at_route_site(edge: &Edge, route_sites: &[Evidence]) -> bool {
  edge.evidence.iter().any(|call| {
    route_sites
      .iter()
      .any(|site| site.repo == call.repo && site.path == call.path && encloses(&site.span, &call.span))
  })
}

fn implementation(graph: &Graph<'_, '_>, pack: &mut Pack, decorator: &Decorator) -> QueryResult<BTreeMap<EntityId, Reached>> {
  let start = &decorator.target;
  let package = package_key(start);
  let mut reached: BTreeMap<EntityId, Reached> = BTreeMap::new();
  let mut frontier = vec![(start.id, decorator.trust)];
  for _ in 0..IMPLEMENTATION_HOPS {
    let mut next = Vec::new();
    for (id, trust) in frontier {
      for (edge, target) in neighbours(graph, pack, id, Forward, &SHARING_EDGES)? {
        let inside = package.is_some() && package_key(&target) == package;
        let applied = target.id == start.id || target.id == decorator.applied.id;
        if !inside || applied || reached.contains_key(&target.id) || !target.kind.is_code_symbol() || is_test(&target) {
          continue;
        }
        let trust = trust.then(&edge);
        if is_callable(&target) {
          next.push((target.id, trust));
        }
        if target.kind == EntityKind::Variant {
          roll_up(graph, pack, &target, trust, &mut reached)?;
        }
        reached.insert(target.id, Reached { entity: target, trust });
      }
    }
    frontier = next;
  }
  Ok(reached)
}

fn roll_up(graph: &Graph<'_, '_>, pack: &mut Pack, variant: &Entity, trust: Trust, reached: &mut BTreeMap<EntityId, Reached>) -> QueryResult<()> {
  for (contains, parent) in neighbours(graph, pack, variant.id, Reverse, &[EdgeKind::Contains])? {
    if parent.kind == EntityKind::Enum && !reached.contains_key(&parent.id) {
      let trust = trust.then(&contains);
      reached.insert(parent.id, Reached { entity: parent, trust });
    }
  }
  Ok(())
}

fn shared_ids(graph: &Graph<'_, '_>, pack: &mut Pack, key: &Entity) -> QueryResult<Vec<EntityId>> {
  let mut ids = vec![key.id];
  if key.kind == EntityKind::Enum {
    let variants = neighbours(graph, pack, key.id, Forward, &[EdgeKind::Contains])?;
    ids.extend(
      variants
        .into_iter()
        .filter(|(_, variant)| variant.kind == EntityKind::Variant)
        .map(|(_, variant)| variant.id),
    );
  }
  Ok(ids)
}

fn consumers(graph: &Graph<'_, '_>, pack: &mut Pack, walk: &Walk, walked: &BTreeSet<EntityId>, handler: EntityId) -> QueryResult<Vec<Consumer>> {
  let decorator = &walk.decorator;
  let own_files: Vec<EntityKey> = [file_entity(&decorator.applied), file_entity(&decorator.target)].into_iter().flatten().collect();
  let outside = |entity: &Entity| !walked.contains(&entity.id) && file_entity(entity).is_none_or(|file| !own_files.contains(&file));
  let mut found: BTreeMap<EntityId, Consumer> = BTreeMap::new();
  for key in walk.implementation.values() {
    let ids = shared_ids(graph, pack, &key.entity)?;
    let users = production_users_where(graph, &ids, &SHARING_EDGES, SHARED_KEY_FAN_IN + 1, outside)?;
    if users == 0 || users > SHARED_KEY_FAN_IN {
      continue;
    }
    let kind_factor = match key.entity.kind {
      EntityKind::Function | EntityKind::Method | EntityKind::Constant | EntityKind::Variable => 1.0,
      _ => TYPE_KEY_FACTOR,
    };
    let specificity = kind_factor / users as f64;
    let mut sharers: Vec<(Edge, Entity)> = Vec::new();
    for id in ids {
      sharers.extend(neighbours(graph, pack, id, Reverse, &SHARING_EDGES)?);
    }
    sharers.sort_by(|left, right| certainty_value(&right.0.certainty).total_cmp(&certainty_value(&left.0.certainty)));
    let mut seen: BTreeSet<EntityId> = BTreeSet::new();
    for (edge, sharer) in sharers {
      let excluded = seen.contains(&sharer.id) || sharer.id == handler || !outside(&sharer) || !is_callable(&sharer) || is_test(&sharer);
      if excluded || at_own_decorator_site(&sharer, &edge) || applied_as_decorator(graph, pack, &sharer)? {
        continue;
      }
      seen.insert(sharer.id);
      let trust = key.trust.then(&edge);
      let shared = Shared {
        key: short_name(&key.entity.key),
        weight: specificity * trust.certainty,
        authoritative: trust.authoritative,
        edge,
      };
      match found.get_mut(&sharer.id) {
        Some(consumer) => {
          consumer.score += shared.weight;
          if shared.weight > consumer.best.weight {
            consumer.best = shared;
          }
        }
        None => {
          found.insert(
            sharer.id,
            Consumer {
              score: shared.weight,
              entity: sharer,
              best: shared,
            },
          );
        }
      }
    }
  }
  let mut ranked: Vec<Consumer> = found.into_values().collect();
  ranked.sort_by(|left, right| right.score.total_cmp(&left.score).then_with(|| left.entity.key.cmp(&right.entity.key)));
  ranked.truncate(CONSUMERS_PER_DECORATOR);
  Ok(ranked)
}

fn at_own_decorator_site(sharer: &Entity, edge: &Edge) -> bool {
  let sites: Vec<AttributeFact> = attributes(sharer).into_iter().chain(parameter_attributes(sharer)).collect();
  let in_file = |evidence: &Evidence| sharer.repo.as_ref() == Some(&evidence.repo) && sharer.path.as_ref() == Some(&evidence.path);
  let at_site = |evidence: &Evidence| sites.iter().any(|site| encloses(&site.span, &evidence.span));
  !sites.is_empty() && !edge.evidence.is_empty() && edge.evidence.iter().all(|evidence| in_file(evidence) && at_site(evidence))
}

fn applied_as_decorator(graph: &Graph<'_, '_>, pack: &mut Pack, entity: &Entity) -> QueryResult<bool> {
  for (edge, caller) in neighbours(graph, pack, entity.id, Reverse, &[EdgeKind::Calls])? {
    if decorator_site(&attributes(&caller), &entity.name, &edge) || decorator_site(&parameter_attributes(&caller), &entity.name, &edge) {
      return Ok(true);
    }
  }
  Ok(false)
}

fn decorator_site(applied: &[AttributeFact], name: &str, edge: &Edge) -> bool {
  applied
    .iter()
    .filter(|attribute| attribute.is(name))
    .any(|attribute| edge.evidence.iter().any(|evidence| encloses(&attribute.span, &evidence.span)))
}

fn encloses(outer: &Span, inner: &Span) -> bool {
  outer.start_byte <= inner.start_byte && inner.end_byte <= outer.end_byte
}

#[cfg(test)]
mod tests {
  use atlas_core::{Certainty, Confidence, EntityKey, RepoName};

  use super::*;
  use crate::queries::context::CALLEE_FACTOR;

  fn edge(certainty: Certainty) -> Edge {
    let entity = |name: &str| {
      EntityId::of(&EntityKey::Repository {
        repo: RepoName::new(name).unwrap(),
      })
    };
    Edge::new(EdgeKind::Calls, entity("a"), entity("b"), "", certainty)
  }

  #[test]
  fn should_keep_a_heuristic_decorator_call_uncertain_through_resolved_hops() {
    let decorator_call = edge(Certainty::heuristic(Confidence::new(0.6).unwrap(), "name-only"));

    let trust = Trust::of(&decorator_call).then(&edge(Certainty::Resolved)).then(&edge(Certainty::Resolved));

    assert_eq!((trust.authoritative, (trust.certainty * 1000.0).round() / 1000.0), (false, 0.6));
  }

  #[test]
  fn should_weigh_other_decorators_below_a_direct_callee_and_a_direct_callee_below_the_route_decorator() {
    let factors = [Site::Route, Site::Method, Site::Parameter, Site::Class].map(Site::factor);

    assert_eq!(factors, [0.7, 0.35, 0.35, 0.25]);
    assert!(factors[1..].iter().all(|factor| *factor < CALLEE_FACTOR) && CALLEE_FACTOR < factors[0]);
  }

  fn constant(kind: EntityKind, signature: &str) -> Entity {
    let mut entity = Entity::new(
      EntityKey::Repository {
        repo: RepoName::new("web").unwrap(),
      },
      Certainty::Syntactic,
    );
    entity.kind = kind;
    entity.signature = Some(signature.to_owned());
    entity
  }

  #[test]
  fn should_read_only_a_bare_identifier_as_the_function_a_constant_aliases() {
    let constants = [
      constant(EntityKind::Constant, "const Reply = ReplyParam"),
      constant(EntityKind::Constant, "const Reply: () => ParameterDecorator = ReplyParam"),
      constant(EntityKind::Constant, "const Cookie = bindInput.bind(null, 'cookie')"),
      constant(EntityKind::Constant, "const Header = (name: string) => bindInput('header', name)"),
      constant(EntityKind::Constant, "const ROUTES = 3"),
      constant(EntityKind::Variable, "let Reply = ReplyParam"),
    ];

    let aliased: Vec<Option<&str>> = constants.iter().map(aliased_function).collect();

    assert_eq!(aliased, [Some("ReplyParam"), Some("ReplyParam"), None, None, None, None]);
  }
}
