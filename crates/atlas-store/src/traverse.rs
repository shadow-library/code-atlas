use std::collections::{HashMap, VecDeque};

use atlas_core::{EdgeId, EntityId};
use rusqlite::params;

use crate::error::StoreResult;
use crate::filter::{CertaintyBounds, EdgeFilter};
use crate::tx::ReadTx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
  Out,
  In,
  Both,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraverseSpec {
  pub seeds: Vec<EntityId>,
  pub direction: Direction,
  pub filter: EdgeFilter,
  pub max_depth: u32,
  pub max_nodes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachedNode {
  pub id: EntityId,
  pub depth: u32,
  pub path: Vec<EdgeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completeness {
  Complete,
  TruncatedAtNodeCap { max_nodes: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Traversal {
  pub nodes: Vec<ReachedNode>,
  pub completeness: Completeness,
}

#[derive(Clone, Copy)]
struct Step {
  edge: EdgeId,
  neighbour: EntityId,
}

struct Visit {
  depth: u32,
  parent: Option<(EntityId, EdgeId)>,
}

const OUT_SQL: &str = "SELECT id, kind, dst FROM edges WHERE src = ?1 AND certainty_tier >= ?2 AND (?3 = 0 OR certainty_class <> 'inferred')";
const IN_SQL: &str = "SELECT id, kind, src FROM edges WHERE dst = ?1 AND certainty_tier >= ?2 AND (?3 = 0 OR certainty_class <> 'inferred')";

impl ReadTx<'_> {
  pub fn traverse(&self, spec: &TraverseSpec) -> StoreResult<Traversal> {
    let mut seeds = spec.seeds.clone();
    seeds.sort_unstable();
    seeds.dedup();
    let mut visits: HashMap<EntityId, Visit> = HashMap::new();
    let mut order = Vec::new();
    let mut queue = VecDeque::new();
    let mut completeness = Completeness::Complete;
    let truncated = Completeness::TruncatedAtNodeCap { max_nodes: spec.max_nodes };
    for seed in seeds {
      if order.len() == spec.max_nodes {
        completeness = truncated;
        break;
      }
      visits.insert(seed, Visit { depth: 0, parent: None });
      order.push(seed);
      queue.push_back(seed);
    }
    'search: while let Some(node) = queue.pop_front() {
      if completeness != Completeness::Complete {
        break;
      }
      let depth = visits.get(&node).map_or(0, |visit| visit.depth);
      if depth >= spec.max_depth {
        continue;
      }
      for step in self.steps(node, spec.direction, &spec.filter)? {
        if visits.contains_key(&step.neighbour) {
          continue;
        }
        if order.len() == spec.max_nodes {
          completeness = truncated;
          break 'search;
        }
        visits.insert(
          step.neighbour,
          Visit {
            depth: depth + 1,
            parent: Some((node, step.edge)),
          },
        );
        order.push(step.neighbour);
        queue.push_back(step.neighbour);
      }
    }
    let nodes = order
      .into_iter()
      .map(|id| ReachedNode {
        id,
        depth: visits.get(&id).map_or(0, |visit| visit.depth),
        path: explaining_path(&visits, id),
      })
      .collect();
    Ok(Traversal { nodes, completeness })
  }

  fn steps(&self, node: EntityId, direction: Direction, filter: &EdgeFilter) -> StoreResult<Vec<Step>> {
    let mut steps = match direction {
      Direction::Out => self.steps_along(OUT_SQL, node, filter)?,
      Direction::In => self.steps_along(IN_SQL, node, filter)?,
      Direction::Both => {
        let mut steps = self.steps_along(OUT_SQL, node, filter)?;
        steps.extend(self.steps_along(IN_SQL, node, filter)?);
        steps
      }
    };
    steps.sort_by_key(|step| (step.edge, step.neighbour));
    Ok(steps)
  }

  fn steps_along(&self, sql: &str, node: EntityId, filter: &EdgeFilter) -> StoreResult<Vec<Step>> {
    let CertaintyBounds { min_tier, exclude_inferred } = filter.bounds();
    let mut statement = self.conn().prepare_cached(sql)?;
    let rows = statement.query_map(params![node.as_i64(), min_tier, exclude_inferred], |row| {
      Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
    })?;
    let mut steps = Vec::new();
    for row in rows {
      let (edge, kind, neighbour) = row?;
      if filter.admits_kind(&kind) {
        steps.push(Step {
          edge: EdgeId::from_i64(edge),
          neighbour: EntityId::from_i64(neighbour),
        });
      }
    }
    Ok(steps)
  }
}

fn explaining_path(visits: &HashMap<EntityId, Visit>, target: EntityId) -> Vec<EdgeId> {
  let mut path = Vec::new();
  let mut current = target;
  while let Some((parent, edge)) = visits.get(&current).and_then(|visit| visit.parent) {
    path.push(edge);
    current = parent;
  }
  path.reverse();
  path
}
