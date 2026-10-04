use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};

use atlas_core::{Certainty, Edge, EdgeId, EdgeKind, Entity, EntityId, EntityKind, Owner};
use atlas_store::{AttrSources, EdgeFilter, EntityFilter, ReadTx};

use crate::error::QueryResult;
use crate::model::{AttrView, CertaintyTag, EntityRef, Hop};
use crate::policy::{Direction, Policy, phrase};

pub(crate) struct Graph<'a, 'c> {
  tx: &'a ReadTx<'c>,
  cache: RefCell<HashMap<EntityId, Option<Entity>>>,
  pub policy: Policy,
}

impl<'a, 'c> Graph<'a, 'c> {
  pub fn new(tx: &'a ReadTx<'c>, policy: Policy) -> Self {
    Self {
      tx,
      cache: RefCell::new(HashMap::new()),
      policy,
    }
  }

  pub fn tx(&self) -> &'a ReadTx<'c> {
    self.tx
  }

  pub fn entity(&self, id: EntityId) -> QueryResult<Option<Entity>> {
    if let Some(cached) = self.cache.borrow().get(&id) {
      return Ok(cached.clone());
    }
    let entity = self.tx.entity(id)?;
    self.cache.borrow_mut().insert(id, entity.clone());
    Ok(entity)
  }

  pub fn entity_attrs(&self, id: EntityId, names: &[&str]) -> QueryResult<Vec<AttrView>> {
    let Some(merged) = self.tx.entity_with_sources(id)? else { return Ok(Vec::new()) };
    Ok(attr_views(&merged.entity.attrs, &merged.attr_sources, names))
  }

  pub fn edge_attrs(&self, id: EdgeId, names: &[&str]) -> QueryResult<Vec<AttrView>> {
    let Some(merged) = self.tx.edge_with_sources(id)? else { return Ok(Vec::new()) };
    Ok(attr_views(&merged.edge.attrs, &merged.attr_sources, names))
  }

  pub fn entity_ref(&self, id: EntityId) -> QueryResult<Option<EntityRef>> {
    Ok(self.entity(id)?.as_ref().map(EntityRef::of))
  }

  pub fn edges_in(&self, id: EntityId, kinds: &[EdgeKind]) -> QueryResult<Vec<Edge>> {
    Ok(self.tx.edges_to(id, &kinds_filter(kinds))?)
  }

  pub fn edges_out(&self, id: EntityId, kinds: &[EdgeKind]) -> QueryResult<Vec<Edge>> {
    Ok(self.tx.edges_from(id, &kinds_filter(kinds))?)
  }

  pub fn edges(&self, id: EntityId, direction: Direction, kinds: &[EdgeKind]) -> QueryResult<Vec<Edge>> {
    match direction {
      Direction::Forward => self.edges_out(id, kinds),
      Direction::Reverse => self.edges_in(id, kinds),
    }
  }

  pub fn admitted(&self, id: EntityId, direction: Direction, kinds: &[EdgeKind], hidden: &mut usize) -> QueryResult<Vec<Edge>> {
    let edges = self.edges(id, direction, kinds)?;
    let before = edges.len();
    let admitted: Vec<Edge> = edges.into_iter().filter(|edge| self.policy.admits(&edge.certainty)).collect();
    *hidden += before - admitted.len();
    Ok(admitted)
  }

  pub fn entities_of_kind(&self, kind: &EntityKind) -> QueryResult<Vec<Entity>> {
    let entities = self.tx.entities_by_kind(kind, &EntityFilter::default())?;
    let mut cache = self.cache.borrow_mut();
    for entity in &entities {
      cache.insert(entity.id, Some(entity.clone()));
    }
    Ok(entities)
  }

  pub fn parent(&self, id: EntityId) -> QueryResult<Option<Entity>> {
    for edge in self.edges_in(id, &[EdgeKind::Contains])? {
      let Some(parent) = self.entity(edge.from)? else { continue };
      if parent.kind.is_code_symbol() {
        return Ok(Some(parent));
      }
    }
    Ok(None)
  }

  pub fn children(&self, id: EntityId) -> QueryResult<Vec<Entity>> {
    let mut children = Vec::new();
    for edge in self.edges_out(id, &[EdgeKind::Contains])? {
      if let Some(child) = self.entity(edge.to)? {
        children.push(child);
      }
    }
    children.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(children)
  }

  pub fn relation(&self, edge: &Edge, subject: EntityId) -> QueryResult<Hop> {
    let (direction, other) = match edge.from == subject {
      true => (Direction::Forward, edge.to),
      false => (Direction::Reverse, edge.from),
    };
    let entity = match self.entity(other)? {
      Some(entity) => crate::model::short_name(&entity.key),
      None => format!("<missing {other}>"),
    };
    Ok(Hop {
      edge: edge.id.to_string(),
      kind: edge.kind.as_str().to_owned(),
      phrase: phrase(&edge.kind, direction, &edge.certainty),
      entity,
      certainty: CertaintyTag::of(&edge.certainty),
    })
  }

  pub fn path(&self, reached: &Reached) -> QueryResult<Vec<Hop>> {
    let mut hops = Vec::with_capacity(reached.hops.len());
    for hop in reached.hops.iter().rev() {
      hops.push(self.relation(&hop.edge, hop.toward)?);
    }
    Ok(hops)
  }

  pub fn walk(&self, spec: &WalkSpec<'_>, expand: impl Fn(&Entity) -> bool) -> QueryResult<Walk> {
    let mut states: HashMap<EntityId, NodeState> = HashMap::new();
    let mut order: Vec<EntityId> = Vec::new();
    let mut queue = VecDeque::new();
    let mut hidden = 0;
    let mut truncated = false;
    let mut seeds = spec.seeds.to_vec();
    seeds.sort_unstable();
    seeds.dedup();
    for seed in seeds {
      states.insert(seed, NodeState::seed());
      queue.push_back(seed);
    }
    let forward: Vec<EdgeKind> = spec.kinds(Direction::Forward, false);
    let reverse: Vec<EdgeKind> = spec.kinds(Direction::Reverse, false);
    let free_forward: Vec<EdgeKind> = spec.kinds(Direction::Forward, true);
    let free_reverse: Vec<EdgeKind> = spec.kinds(Direction::Reverse, true);
    while let Some(node) = queue.pop_front() {
      let depth = states.get(&node).map_or(0, |state| state.depth);
      let at_limit = depth >= spec.max_depth;
      if at_limit && free_forward.is_empty() && free_reverse.is_empty() {
        continue;
      }
      if depth > 0 && !self.entity(node)?.as_ref().is_some_and(&expand) {
        continue;
      }
      let (strength, authoritative) = states.get(&node).map_or((1.0, true), |state| (state.strength, state.authoritative));
      let mut steps: Vec<(Edge, Direction, EntityId)> = Vec::new();
      let directions = match at_limit {
        true => [(Direction::Forward, &free_forward), (Direction::Reverse, &free_reverse)],
        false => [(Direction::Forward, &forward), (Direction::Reverse, &reverse)],
      };
      for (direction, kinds) in directions {
        if kinds.is_empty() {
          continue;
        }
        for edge in self.admitted(node, direction, kinds, &mut hidden)? {
          let neighbour = match direction {
            Direction::Forward => edge.to,
            Direction::Reverse => edge.from,
          };
          steps.push((edge, direction, neighbour));
        }
      }
      steps.sort_by_key(|(edge, _, neighbour)| (edge.id, *neighbour));
      for (edge, direction, neighbour) in steps {
        let step = spec.step(&edge.kind, direction);
        let next = depth + step.map_or(1, |step| step.cost);
        let candidate = strength * step.map_or(1.0, |step| step.weight) * certainty_value(&edge.certainty);
        let candidate_authoritative = authoritative && edge.certainty.is_authoritative();
        if let Some(state) = states.get_mut(&neighbour) {
          if state.depth > next && state.parent.is_some() {
            state.depth = next;
            state.strength = candidate;
            state.authoritative = candidate_authoritative;
            state.parent = Some((node, edge));
            queue.push_front(neighbour);
            continue;
          }
          if state.depth == next {
            state.in_degree += 1;
            if (candidate_authoritative, candidate) > (state.authoritative, state.strength) {
              state.strength = candidate;
              state.authoritative = candidate_authoritative;
              state.parent = Some((node, edge));
            }
          }
          continue;
        }
        if order.len() >= spec.max_nodes {
          truncated = true;
          continue;
        }
        states.insert(
          neighbour,
          NodeState {
            depth: next,
            parent: Some((node, edge)),
            strength: candidate,
            authoritative: candidate_authoritative,
            in_degree: 1,
          },
        );
        order.push(neighbour);
        match next == depth {
          true => queue.push_front(neighbour),
          false => queue.push_back(neighbour),
        }
      }
    }
    let reached = order
      .into_iter()
      .map(|id| {
        let state = &states[&id];
        Reached {
          id,
          depth: state.depth,
          hops: path_to(&states, id),
          strength: state.strength,
          in_degree: state.in_degree,
        }
      })
      .collect();
    Ok(Walk {
      reached,
      truncated,
      hidden_below_floor: hidden,
    })
  }
}

fn attr_views(attrs: &atlas_core::Attrs, sources: &AttrSources, names: &[&str]) -> Vec<AttrView> {
  attrs
    .iter()
    .filter(|(name, _)| names.is_empty() || names.contains(&name.as_str()))
    .filter_map(|(name, value)| {
      let source = sources.get(name)?;
      Some(AttrView {
        name: name.clone(),
        value: value.clone(),
        owner: owner_label(&source.owner),
        certainty: CertaintyTag::of(&source.certainty),
      })
    })
    .collect()
}

pub(crate) fn owner_label(owner: &Owner) -> String {
  match owner {
    Owner::File { .. } => "file".to_owned(),
    Owner::Project { pass } => pass.clone(),
  }
}

fn kinds_filter(kinds: &[EdgeKind]) -> EdgeFilter {
  EdgeFilter {
    kinds: kinds.to_vec(),
    ..EdgeFilter::default()
  }
}

pub(crate) fn certainty_value(certainty: &Certainty) -> f64 {
  match certainty {
    Certainty::Heuristic { confidence, .. } => f64::from(f32::from(*confidence)),
    Certainty::Inferred { confidence, .. } => 0.9 * f64::from(f32::from(*confidence)),
    _ => 1.0,
  }
}

#[derive(Debug, Clone)]
pub(crate) struct Step {
  pub kind: EdgeKind,
  pub direction: Direction,
  pub weight: f64,
  pub cost: u32,
}

impl Step {
  pub fn new(kind: EdgeKind, direction: Direction, weight: f64) -> Self {
    Self { kind, direction, weight, cost: 1 }
  }

  pub fn free(kind: EdgeKind, direction: Direction, weight: f64) -> Self {
    Self { kind, direction, weight, cost: 0 }
  }
}

pub(crate) struct WalkSpec<'s> {
  pub seeds: &'s [EntityId],
  pub steps: &'s [Step],
  pub max_depth: u32,
  pub max_nodes: usize,
}

impl WalkSpec<'_> {
  fn kinds(&self, direction: Direction, free_only: bool) -> Vec<EdgeKind> {
    let mut kinds: Vec<EdgeKind> = self
      .steps
      .iter()
      .filter(|step| step.direction == direction && (!free_only || step.cost == 0))
      .map(|step| step.kind.clone())
      .collect();
    kinds.sort();
    kinds.dedup();
    kinds
  }

  fn step(&self, kind: &EdgeKind, direction: Direction) -> Option<&Step> {
    self.steps.iter().find(|step| step.kind == *kind && step.direction == direction)
  }
}

struct NodeState {
  depth: u32,
  parent: Option<(EntityId, Edge)>,
  strength: f64,
  authoritative: bool,
  in_degree: usize,
}

impl NodeState {
  fn seed() -> Self {
    Self {
      depth: 0,
      parent: None,
      strength: 1.0,
      authoritative: true,
      in_degree: 0,
    }
  }
}

#[derive(Debug, Clone)]
pub(crate) struct PathHop {
  pub edge: Edge,
  pub toward: EntityId,
}

#[derive(Debug, Clone)]
pub(crate) struct Reached {
  pub id: EntityId,
  pub depth: u32,
  pub hops: Vec<PathHop>,
  pub strength: f64,
  pub in_degree: usize,
}

impl Reached {
  pub fn is_uncertain(&self) -> bool {
    self.hops.iter().any(|hop| !hop.edge.certainty.is_authoritative())
  }

  pub fn last_hop(&self) -> Option<&PathHop> {
    self.hops.last()
  }
}

pub(crate) struct Walk {
  pub reached: Vec<Reached>,
  pub truncated: bool,
  pub hidden_below_floor: usize,
}

fn path_to(states: &HashMap<EntityId, NodeState>, target: EntityId) -> Vec<PathHop> {
  let mut hops = Vec::new();
  let mut current = target;
  while let Some((parent, edge)) = states.get(&current).and_then(|state| state.parent.clone()) {
    hops.push(PathHop { edge, toward: current });
    current = parent;
  }
  hops.reverse();
  hops
}

pub(crate) fn group_by_repo<T>(items: Vec<T>, repo: impl Fn(&T) -> String) -> BTreeMap<String, Vec<T>> {
  let mut groups: BTreeMap<String, Vec<T>> = BTreeMap::new();
  for item in items {
    groups.entry(repo(&item)).or_default().push(item);
  }
  groups
}
