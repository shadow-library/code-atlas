use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::rc::Rc;

use atlas_core::{Certainty, CertaintyClass, Edge, EdgeId, EdgeKind, Entity, EntityId, EntityKind, Owner, RelPath, RepoName};
use atlas_store::{AttrSources, EdgeFilter, EdgeLink, EntityFilter, ReadTx};

use crate::dynamic::{COMPUTED_PREFIX, all_dynamic_sites, load_dynamic_sites};
use crate::error::QueryResult;
use crate::model::{AttrView, CertaintyTag, EntityRef, Hop, UnresolvedSite};
use crate::policy::{Direction, Policy, is_authoritative_class, phrase};

pub(crate) type FileKey = (RepoName, RelPath);
pub(crate) type FileSites = HashMap<FileKey, Rc<Vec<UnresolvedSite>>>;

pub(crate) struct Graph<'a, 'c> {
  tx: &'a ReadTx<'c>,
  cache: RefCell<HashMap<EntityId, Option<Entity>>>,
  edges: RefCell<HashMap<EdgeId, Option<Edge>>>,
  dynamic: RefCell<DynamicCache>,
  pub policy: Policy,
}

enum DynamicCache {
  Unknown,
  PerFile(FileSites),
  Complete(FileSites),
}

impl<'a, 'c> Graph<'a, 'c> {
  pub fn new(tx: &'a ReadTx<'c>, policy: Policy) -> Self {
    Self {
      tx,
      cache: RefCell::new(HashMap::new()),
      edges: RefCell::new(HashMap::new()),
      dynamic: RefCell::new(DynamicCache::Unknown),
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

  pub fn edge(&self, id: EdgeId) -> QueryResult<Option<Edge>> {
    if let Some(cached) = self.edges.borrow().get(&id) {
      return Ok(cached.clone());
    }
    let edge = self.tx.edge(id)?;
    self.edges.borrow_mut().insert(id, edge.clone());
    Ok(edge)
  }

  pub fn dynamic_sites(&self, repo: &RepoName, path: &RelPath) -> QueryResult<Rc<Vec<UnresolvedSite>>> {
    let key = (repo.clone(), path.clone());
    if !self.may_have_dynamic_sites()? {
      return Ok(Rc::new(Vec::new()));
    }
    match &*self.dynamic.borrow() {
      DynamicCache::Complete(files) => return Ok(files.get(&key).map_or_else(|| Rc::new(Vec::new()), Rc::clone)),
      DynamicCache::PerFile(files) if files.contains_key(&key) => return Ok(Rc::clone(&files[&key])),
      _ => {}
    }
    let sites = load_dynamic_sites(self, repo, path)?;
    if let DynamicCache::PerFile(files) = &mut *self.dynamic.borrow_mut() {
      files.insert(key, Rc::clone(&sites));
    }
    Ok(sites)
  }

  pub fn may_have_dynamic_sites(&self) -> QueryResult<bool> {
    let unknown = matches!(&*self.dynamic.borrow(), DynamicCache::Unknown);
    if unknown {
      let cache = match self.tx.has_unresolved_with_name_prefix(COMPUTED_PREFIX)? {
        true => DynamicCache::PerFile(HashMap::new()),
        false => DynamicCache::Complete(HashMap::new()),
      };
      *self.dynamic.borrow_mut() = cache;
    }
    Ok(!matches!(&*self.dynamic.borrow(), DynamicCache::Complete(files) if files.is_empty()))
  }

  pub fn preload_dynamic_sites(&self) -> QueryResult<()> {
    let files = all_dynamic_sites(self)?;
    *self.dynamic.borrow_mut() = DynamicCache::Complete(files);
    Ok(())
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

  pub fn links_in(&self, id: EntityId, kinds: &[EdgeKind]) -> QueryResult<Vec<EdgeLink>> {
    Ok(self.tx.edge_links_to(id, &kinds_filter(kinds))?)
  }

  pub fn links_out(&self, id: EntityId, kinds: &[EdgeKind]) -> QueryResult<Vec<EdgeLink>> {
    Ok(self.tx.edge_links_from(id, &kinds_filter(kinds))?)
  }

  pub fn edges(&self, id: EntityId, direction: Direction, kinds: &[EdgeKind]) -> QueryResult<Vec<Edge>> {
    match direction {
      Direction::Forward => self.edges_out(id, kinds),
      Direction::Reverse => self.edges_in(id, kinds),
    }
  }

  pub fn admitted(&self, id: EntityId, direction: Direction, kinds: &[EdgeKind], hidden: &mut usize) -> QueryResult<Vec<Edge>> {
    let mut admitted = Vec::new();
    for link in self.admitted_links(id, direction, kinds, hidden)? {
      admitted.extend(self.edge(link.id)?);
    }
    Ok(admitted)
  }

  pub fn admitted_into(&self, targets: impl IntoIterator<Item = EntityId>, kind: EdgeKind, hidden: &mut usize) -> QueryResult<HashMap<EntityId, Vec<Edge>>> {
    let targets: Vec<EntityId> = targets.into_iter().collect();
    let mut per_target = 0;
    for target in &targets {
      per_target += self.tx.edge_count_to(*target)?;
    }
    let mut links: HashMap<EntityId, Vec<EdgeLink>> = HashMap::new();
    match self.tx.edge_count_of_kind(&kind)? < per_target {
      true => {
        let wanted: HashSet<EntityId> = targets.iter().copied().collect();
        for link in self.tx.edge_links_of_kind(&kind, &EdgeFilter::default())? {
          if wanted.contains(&link.to) {
            links.entry(link.to).or_default().push(link);
          }
        }
      }
      false => {
        for target in &targets {
          links.insert(*target, self.links_in(*target, std::slice::from_ref(&kind))?);
        }
      }
    }
    let mut admitted = HashMap::new();
    for target in targets {
      let mut edges = Vec::new();
      for link in links.remove(&target).unwrap_or_default() {
        match self.policy.admits_link(&link) {
          true => edges.extend(self.edge(link.id)?),
          false => *hidden += 1,
        }
      }
      admitted.insert(target, edges);
    }
    Ok(admitted)
  }

  pub fn admitted_distinct(&self, id: EntityId, direction: Direction, kinds: &[EdgeKind], hidden: &mut BTreeSet<EdgeId>) -> QueryResult<Vec<Edge>> {
    let links = match direction {
      Direction::Forward => self.links_out(id, kinds)?,
      Direction::Reverse => self.links_in(id, kinds)?,
    };
    let mut admitted = Vec::new();
    for link in links {
      match self.policy.admits_link(&link) {
        true => admitted.extend(self.edge(link.id)?),
        false => {
          hidden.insert(link.id);
        }
      }
    }
    Ok(admitted)
  }

  fn admitted_links(&self, id: EntityId, direction: Direction, kinds: &[EdgeKind], hidden: &mut usize) -> QueryResult<Vec<EdgeLink>> {
    let links = match direction {
      Direction::Forward => self.links_out(id, kinds)?,
      Direction::Reverse => self.links_in(id, kinds)?,
    };
    let before = links.len();
    let admitted: Vec<EdgeLink> = links.into_iter().filter(|link| self.policy.admits_link(link)).collect();
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
    for edge in self.links_in(id, &[EdgeKind::Contains])? {
      let Some(parent) = self.entity(edge.from)? else { continue };
      if parent.kind.is_code_symbol() {
        return Ok(Some(parent));
      }
    }
    Ok(None)
  }

  pub fn children(&self, id: EntityId) -> QueryResult<Vec<Entity>> {
    let mut children = Vec::new();
    for edge in self.links_out(id, &[EdgeKind::Contains])? {
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

  pub fn hop(&self, hop: &PathHop) -> QueryResult<Hop> {
    match &hop.bridged {
      Some(bridged) => Ok(bridged.clone()),
      None => self.relation(&hop.edge, hop.toward),
    }
  }

  pub fn path(&self, reached: &Reached) -> QueryResult<Vec<Hop>> {
    let mut hops = Vec::with_capacity(reached.hops.len());
    for hop in reached.hops.iter().rev() {
      hops.push(self.hop(hop)?);
    }
    Ok(hops)
  }

  pub fn walk(&self, spec: &WalkSpec<'_>, expand: impl Fn(&Entity) -> bool) -> QueryResult<Walk> {
    self.walk_bridged(spec, expand, |_| Ok(Vec::new()))
  }

  pub fn walk_bridged(&self, spec: &WalkSpec<'_>, expand: impl Fn(&Entity) -> bool, mut bridges: impl FnMut(EntityId) -> QueryResult<Vec<Bridge>>) -> QueryResult<Walk> {
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
      let mut steps: Vec<(EdgeLink, Direction, EntityId)> = Vec::new();
      let directions = match at_limit {
        true => [(Direction::Forward, &free_forward), (Direction::Reverse, &free_reverse)],
        false => [(Direction::Forward, &forward), (Direction::Reverse, &reverse)],
      };
      for (direction, kinds) in directions {
        if kinds.is_empty() {
          continue;
        }
        for link in self.admitted_links(node, direction, kinds, &mut hidden)? {
          let neighbour = match direction {
            Direction::Forward => link.to,
            Direction::Reverse => link.from,
          };
          steps.push((link, direction, neighbour));
        }
      }
      steps.sort_by_key(|(link, _, neighbour)| (link.id, *neighbour));
      let mut offers: Vec<Offer> = steps
        .into_iter()
        .map(|(link, direction, neighbour)| {
          let step = spec.step(&link.kind, direction);
          Offer {
            neighbour,
            depth: depth + step.map_or(1, |step| step.cost),
            strength: strength * step.map_or(1.0, |step| step.weight) * link_value(&link),
            authoritative: authoritative && is_authoritative_class(link.certainty_class),
            via: Via::Link(link.id),
          }
        })
        .collect();
      if !at_limit {
        let mut bridged = bridges(node)?;
        if !bridged.is_empty() {
          let direct: HashSet<(EdgeId, EntityId)> = offers
            .iter()
            .filter_map(|offer| match offer.via {
              Via::Link(id) => Some((id, offer.neighbour)),
              Via::Bridge(_) => None,
            })
            .collect();
          bridged.retain(|bridge| !direct.contains(&(bridge.edge.id, bridge.to)));
        }
        bridged.sort_by(|left, right| (left.to, left.edge.id, &left.hop.edge).cmp(&(right.to, right.edge.id, &right.hop.edge)));
        offers.extend(bridged.into_iter().map(|bridge| Offer {
          neighbour: bridge.to,
          depth: depth + 1,
          strength: strength * bridge.value,
          authoritative: false,
          via: Via::Bridge(Box::new(bridge)),
        }));
      }
      for offer in offers {
        let Offer {
          neighbour,
          depth: next,
          strength: candidate,
          authoritative: candidate_authoritative,
          via,
        } = offer;
        if let Some(state) = states.get_mut(&neighbour) {
          if state.depth > next && state.parent.is_some() {
            let Some(arrival) = self.arrival(via)? else { continue };
            state.depth = next;
            state.strength = candidate;
            state.authoritative = candidate_authoritative;
            state.parent = Some((node, arrival));
            queue.push_front(neighbour);
            continue;
          }
          if state.depth == next {
            state.in_degree += 1;
            if (candidate_authoritative, candidate) > (state.authoritative, state.strength) {
              let Some(arrival) = self.arrival(via)? else { continue };
              state.strength = candidate;
              state.authoritative = candidate_authoritative;
              state.parent = Some((node, arrival));
            }
          }
          continue;
        }
        if order.len() >= spec.max_nodes {
          truncated = true;
          continue;
        }
        let Some(arrival) = self.arrival(via)? else { continue };
        states.insert(
          neighbour,
          NodeState {
            depth: next,
            parent: Some((node, arrival)),
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

  fn arrival(&self, via: Via) -> QueryResult<Option<Arrival>> {
    Ok(match via {
      Via::Link(id) => self.edge(id)?.map(|edge| Arrival { edge, bridged: None }),
      Via::Bridge(bridge) => Some(Arrival {
        edge: bridge.edge,
        bridged: Some(bridge.hop),
      }),
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

fn link_value(link: &EdgeLink) -> f64 {
  match (link.certainty_class, link.confidence) {
    (CertaintyClass::Heuristic, Some(confidence)) => f64::from(confidence),
    (CertaintyClass::Inferred, Some(confidence)) => 0.9 * f64::from(confidence),
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

#[derive(Debug, Clone)]
pub(crate) struct Bridge {
  pub to: EntityId,
  pub edge: Edge,
  pub hop: Hop,
  pub value: f64,
}

enum Via {
  Link(EdgeId),
  Bridge(Box<Bridge>),
}

struct Offer {
  neighbour: EntityId,
  depth: u32,
  strength: f64,
  authoritative: bool,
  via: Via,
}

#[derive(Clone)]
struct Arrival {
  edge: Edge,
  bridged: Option<Hop>,
}

struct NodeState {
  depth: u32,
  parent: Option<(EntityId, Arrival)>,
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
  pub bridged: Option<Hop>,
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
    self.hops.iter().any(|hop| hop.bridged.is_some() || !hop.edge.certainty.is_authoritative())
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
  while let Some((parent, Arrival { edge, bridged })) = states.get(&current).and_then(|state| state.parent.clone()) {
    hops.push(PathHop { edge, toward: current, bridged });
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
