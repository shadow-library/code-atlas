use std::collections::{BTreeSet, HashMap};

use atlas_core::{CertaintyClass, EdgeId, EdgeKind, Entity, EntityId, EntityKind};
use serde::Serialize;

use crate::QueryEngine;
use crate::error::{QueryError, QueryResult};
use crate::graph::Graph;
use crate::model::{Caveat, EntityRef, Hop, Response, Targeted};
use crate::policy::{Policy, is_authoritative_class};
use crate::queries::common::resolve_one;
use crate::target::TargetFilter;

pub const DEFAULT_PATH_DEPTH: u32 = 6;
pub const MAX_PATH_DEPTH: u32 = 12;
const MAX_VISITED: usize = 50_000;
const AUTHORITATIVE_COST: u32 = 1;
const HEURISTIC_COST: u32 = 2;
const INFERRED_COST: u32 = 3;
const STRUCTURAL_COST: u32 = 3;
const STRUCTURAL_EDGES: [EdgeKind; 8] = [
  EdgeKind::Contains,
  EdgeKind::Defines,
  EdgeKind::Exports,
  EdgeKind::ReExports,
  EdgeKind::Imports,
  EdgeKind::DependsOnPackage,
  EdgeKind::PartOfApp,
  EdgeKind::CoversFile,
];
const ENDPOINT_ONLY: [EntityKind; 4] = [EntityKind::File, EntityKind::Package, EntityKind::Repository, EntityKind::Application];

#[derive(Debug, Clone, PartialEq)]
pub struct PathRequest {
  pub from: String,
  pub to: String,
  pub max_depth: u32,
  pub filter: TargetFilter,
}

impl PathRequest {
  pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
    Self {
      from: from.into(),
      to: to.into(),
      max_depth: DEFAULT_PATH_DEPTH,
      filter: TargetFilter::default(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PathStep {
  pub relation: Hop,
  pub entity: EntityRef,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PathAnswer {
  pub from: EntityRef,
  pub to: EntityRef,
  pub max_depth: u32,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub cost: Option<u32>,
  pub steps: Vec<PathStep>,
  pub visited: usize,
  pub caveats: Vec<Caveat>,
}

impl PathAnswer {
  pub fn found(&self) -> bool {
    self.cost.is_some()
  }
}

#[derive(Clone, Copy)]
struct Reach {
  cost: u32,
  previous: EntityId,
  edge: Option<EdgeId>,
}

fn cost_of(kind: &EdgeKind, class: CertaintyClass) -> u32 {
  if STRUCTURAL_EDGES.contains(kind) {
    return STRUCTURAL_COST;
  }
  match class {
    CertaintyClass::Heuristic => HEURISTIC_COST,
    CertaintyClass::Inferred => INFERRED_COST,
    other if is_authoritative_class(other) => AUTHORITATIVE_COST,
    _ => INFERRED_COST,
  }
}

impl QueryEngine {
  pub fn path(&self, request: &PathRequest) -> QueryResult<Response<Targeted<PathAnswer>>> {
    if !(1..=MAX_PATH_DEPTH).contains(&request.max_depth) {
      return Err(QueryError::invalid(format!("max depth must be between 1 and {MAX_PATH_DEPTH}")));
    }
    let freshness = self.barrier()?;
    let answer = self.read(Policy::with_floor(0.0), |graph| -> QueryResult<Targeted<PathAnswer>> {
      let from = match resolve_one(self, graph, &request.from, &request.filter)? {
        Ok(entity) => entity,
        Err(outcome) => return Ok(outcome),
      };
      let to = match resolve_one(self, graph, &request.to, &request.filter)? {
        Ok(entity) => entity,
        Err(outcome) => return Ok(outcome),
      };
      shortest(graph, &from, &to, request.max_depth).map(Targeted::Found)
    })?;
    Ok(Response { freshness, answer })
  }
}

fn shortest(graph: &Graph<'_, '_>, from: &Entity, to: &Entity, max_depth: u32) -> QueryResult<PathAnswer> {
  let mut answer = PathAnswer {
    from: EntityRef::of(from),
    to: EntityRef::of(to),
    max_depth,
    cost: None,
    steps: Vec::new(),
    visited: 1,
    caveats: Vec::new(),
  };
  if from.id == to.id {
    answer.cost = Some(0);
    return Ok(answer);
  }
  let mut best: HashMap<EntityId, u32> = HashMap::from([(from.id, 0)]);
  let mut layers: Vec<HashMap<EntityId, Reach>> = vec![HashMap::from([(
    from.id,
    Reach {
      cost: 0,
      previous: from.id,
      edge: None,
    },
  )])];
  let mut truncated = false;
  for _ in 0..max_depth {
    let frontier: BTreeSet<(EntityId, u32)> = layers.last().map(|layer| layer.iter().map(|(id, reach)| (*id, reach.cost)).collect()).unwrap_or_default();
    let mut next: HashMap<EntityId, Reach> = HashMap::new();
    'nodes: for (node, cost) in frontier {
      let mut links = graph.links_out(node, &[])?;
      links.extend(graph.links_in(node, &[])?);
      links.sort_by_key(|link| (link.id, link.from, link.to));
      for link in links {
        let other = if link.from == node { link.to } else { link.from };
        if link.kind == EdgeKind::RenamedFrom || other == node {
          continue;
        }
        let reached = cost + cost_of(&link.kind, link.certainty_class);
        if best.get(&other).is_some_and(|known| *known <= reached) {
          continue;
        }
        if other != to.id && graph.entity(other)?.is_some_and(|entity| ENDPOINT_ONLY.contains(&entity.kind)) {
          continue;
        }
        best.insert(other, reached);
        next.insert(
          other,
          Reach {
            cost: reached,
            previous: node,
            edge: Some(link.id),
          },
        );
        if best.len() >= MAX_VISITED {
          truncated = true;
          break 'nodes;
        }
      }
    }
    if next.is_empty() {
      break;
    }
    layers.push(next);
    if truncated {
      break;
    }
  }
  answer.visited = best.len();
  if truncated {
    answer.caveats.push(Caveat::Note {
      message: format!("the search stopped after visiting {MAX_VISITED} entities; a shorter path may exist"),
    });
  }
  let Some(found) = layers.iter().rposition(|layer| layer.contains_key(&to.id)) else {
    return Ok(answer);
  };
  let mut chain = Vec::new();
  let mut node = to.id;
  for layer in layers[1..=found].iter().rev() {
    let Some((reach, edge)) = layer.get(&node).and_then(|reach| Some((reach, reach.edge?))) else {
      break;
    };
    chain.push((reach.previous, edge, node));
    node = reach.previous;
  }
  chain.reverse();
  answer.cost = layers[found].get(&to.id).map(|reach| reach.cost);
  for (subject, edge, reached) in chain {
    let (Some(edge), Some(entity)) = (graph.edge(edge)?, graph.entity(reached)?) else {
      return Err(QueryError::invalid(format!("edge {edge} on the path vanished during the query")));
    };
    answer.steps.push(PathStep {
      relation: graph.relation(&edge, subject)?,
      entity: EntityRef::of(&entity),
    });
  }
  Ok(answer)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_weigh_hops_by_certainty_class_and_structural_hops_at_three() {
    assert_eq!(cost_of(&EdgeKind::Calls, CertaintyClass::Resolved), 1);
    assert_eq!(cost_of(&EdgeKind::Calls, CertaintyClass::CompilerVerified), 1);
    assert_eq!(cost_of(&EdgeKind::References, CertaintyClass::Declared), 1);
    assert_eq!(cost_of(&EdgeKind::Calls, CertaintyClass::Heuristic), 2);
    assert_eq!(cost_of(&EdgeKind::Calls, CertaintyClass::Inferred), 3);
    assert_eq!(cost_of(&EdgeKind::Contains, CertaintyClass::Syntactic), 3);
    assert_eq!(cost_of(&EdgeKind::Defines, CertaintyClass::Resolved), 3);
  }
}
