use std::collections::BTreeMap;

use atlas_core::{AttributeFact, Edge, EdgeKind, Entity, EntityId, EntityKind, Evidence};

use super::{Basis, Node, Pack, Role, neighbours, production_users};
use crate::error::QueryResult;
use crate::graph::{Graph, certainty_value};
use crate::model::short_name;
use crate::policy::Direction::{Forward, Reverse};
use crate::queries::common::{attributes, file_entity, is_test, package_key};

const SERVING_FACTOR: f64 = 0.7;
const IMPLEMENTATION_HOPS: usize = 3;
const SHARED_KEY_FAN_IN: usize = 6;
const CONSUMERS_PER_DECORATOR: usize = 3;
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
  let handler_name = short_name(&handler.entity.key);
  for (decorator, trust) in route_decorators(graph, pack, &handler.entity, &exposes.evidence)? {
    let implementation = implementation(graph, pack, &decorator, trust)?;
    let decorator_name = short_name(&decorator.key);
    for consumer in consumers(graph, pack, &decorator, &implementation, handler.entity.id)? {
      let factor = SERVING_FACTOR * consumer.score.min(1.0);
      let via = graph.relation(&consumer.best.edge, consumer.entity.id)?;
      let why = format!(
        "shares {} with the route decorator {decorator_name} of {handler_name}: {}",
        consumer.best.key,
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
    }
  }
  Ok(())
}

fn route_decorators(graph: &Graph<'_, '_>, pack: &mut Pack, handler: &Entity, route_sites: &[Evidence]) -> QueryResult<Vec<(Entity, Trust)>> {
  let applied = attributes(handler);
  if applied.is_empty() || route_sites.is_empty() {
    return Ok(Vec::new());
  }
  let mut decorators = Vec::new();
  for (edge, callee) in neighbours(graph, pack, handler.id, Forward, &[EdgeKind::Calls])? {
    let callable = matches!(callee.kind, EntityKind::Function | EntityKind::Method) && !is_test(&callee);
    if callable && decorator_site(&applied, &callee.name, &edge) && at_route_site(&edge, route_sites) {
      let trust = Trust::of(&edge);
      decorators.push((callee, trust));
    }
  }
  Ok(decorators)
}

fn at_route_site(edge: &Edge, route_sites: &[Evidence]) -> bool {
  edge.evidence.iter().any(|call| {
    route_sites
      .iter()
      .any(|site| site.repo == call.repo && site.path == call.path && site.span.start_line <= call.span.start_line && call.span.end_line <= site.span.end_line)
  })
}

fn implementation(graph: &Graph<'_, '_>, pack: &mut Pack, decorator: &Entity, trust: Trust) -> QueryResult<BTreeMap<EntityId, Reached>> {
  let package = package_key(decorator);
  let mut reached: BTreeMap<EntityId, Reached> = BTreeMap::new();
  let mut frontier = vec![(decorator.id, trust)];
  for _ in 0..IMPLEMENTATION_HOPS {
    let mut next = Vec::new();
    for (id, trust) in frontier {
      for (edge, target) in neighbours(graph, pack, id, Forward, &SHARING_EDGES)? {
        let inside = package.is_some() && package_key(&target) == package;
        if !inside || target.id == decorator.id || reached.contains_key(&target.id) || !target.kind.is_code_symbol() || is_test(&target) {
          continue;
        }
        let trust = trust.then(&edge);
        if matches!(target.kind, EntityKind::Function | EntityKind::Method) {
          next.push((target.id, trust));
        }
        reached.insert(target.id, Reached { entity: target, trust });
      }
    }
    frontier = next;
  }
  Ok(reached)
}

fn consumers(graph: &Graph<'_, '_>, pack: &mut Pack, decorator: &Entity, implementation: &BTreeMap<EntityId, Reached>, handler: EntityId) -> QueryResult<Vec<Consumer>> {
  let mut found: BTreeMap<EntityId, Consumer> = BTreeMap::new();
  for key in implementation.values() {
    let users = production_users(graph, key.entity.id, &SHARING_EDGES, SHARED_KEY_FAN_IN + 1)?;
    if users == 0 || users > SHARED_KEY_FAN_IN {
      continue;
    }
    let kind_factor = match key.entity.kind {
      EntityKind::Function | EntityKind::Method | EntityKind::Constant | EntityKind::Variable => 1.0,
      _ => TYPE_KEY_FACTOR,
    };
    let specificity = kind_factor / users.saturating_sub(1).max(1) as f64;
    for (edge, sharer) in neighbours(graph, pack, key.entity.id, Reverse, &SHARING_EDGES)? {
      let excluded = sharer.id == handler
        || sharer.id == decorator.id
        || implementation.contains_key(&sharer.id)
        || file_entity(&sharer) == file_entity(decorator)
        || !matches!(sharer.kind, EntityKind::Function | EntityKind::Method)
        || is_test(&sharer);
      if excluded || applied_as_decorator(graph, pack, &sharer)? {
        continue;
      }
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

fn applied_as_decorator(graph: &Graph<'_, '_>, pack: &mut Pack, entity: &Entity) -> QueryResult<bool> {
  for (edge, caller) in neighbours(graph, pack, entity.id, Reverse, &[EdgeKind::Calls])? {
    if decorator_site(&attributes(&caller), &entity.name, &edge) {
      return Ok(true);
    }
  }
  Ok(false)
}

fn decorator_site(applied: &[AttributeFact], name: &str, edge: &Edge) -> bool {
  applied.iter().filter(|attribute| attribute.is(name)).any(|attribute| {
    edge
      .evidence
      .iter()
      .any(|evidence| attribute.span.start_line <= evidence.span.start_line && evidence.span.end_line <= attribute.span.end_line)
  })
}

#[cfg(test)]
mod tests {
  use atlas_core::{Certainty, Confidence, EntityKey, RepoName};

  use super::*;

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
}
