use std::collections::HashSet;

use atlas_core::{EdgeKind, Entity, EntityId};
use serde::Serialize;

use crate::QueryEngine;
use crate::error::{QueryError, QueryResult};
use crate::graph::Graph;
use crate::model::{Caveat, CertaintyTag, EntityRef, Response, Targeted};
use crate::policy::{Direction, Policy, phrase};
use crate::queries::symbols::{SymbolTarget, resolve_symbol_target};

pub const DEFAULT_HIERARCHY_DEPTH: u32 = 3;
pub const MAX_HIERARCHY_DEPTH: u32 = 5;
const MAX_HIERARCHY_NODES: usize = 200;
const HIERARCHY_KINDS: [EdgeKind; 2] = [EdgeKind::Extends, EdgeKind::Implements];

#[derive(Debug, Clone, PartialEq)]
pub struct HierarchyRequest {
  pub target: String,
  pub depth: u32,
}

impl HierarchyRequest {
  pub fn new(target: impl Into<String>) -> Self {
    Self {
      target: target.into(),
      depth: DEFAULT_HIERARCHY_DEPTH,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HierarchyNode {
  pub entity: EntityRef,
  pub edge_kind: String,
  pub phrase: String,
  pub certainty: CertaintyTag,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub via: Option<String>,
  pub children: Vec<HierarchyNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HierarchyAnswer {
  pub target: EntityRef,
  pub supertypes: Vec<HierarchyNode>,
  pub subtypes: Vec<HierarchyNode>,
  pub caveats: Vec<Caveat>,
}

impl QueryEngine {
  pub fn hierarchy(&self, request: &HierarchyRequest) -> QueryResult<Response<Targeted<HierarchyAnswer>>> {
    if !(1..=MAX_HIERARCHY_DEPTH).contains(&request.depth) {
      return Err(QueryError::invalid(format!("hierarchy depth must be between 1 and {MAX_HIERARCHY_DEPTH}")));
    }
    let target = SymbolTarget::parse(&request.target)?;
    let freshness = self.barrier()?;
    let repos = self.repos();
    let answer = self.read(Policy::with_floor(0.0), |graph| {
      Ok(match resolve_symbol_target(graph, &repos, &target, &request.target)? {
        Targeted::Found(entity) => Targeted::Found(hierarchy_of(graph, &entity, request.depth)?),
        Targeted::Ambiguous { spec, candidates } => Targeted::Ambiguous { spec, candidates },
        Targeted::NotFound { spec, hint } => Targeted::NotFound { spec, hint },
      })
    })?;
    Ok(Response { freshness, answer })
  }
}

fn hierarchy_of(graph: &Graph<'_, '_>, target: &Entity, depth: u32) -> QueryResult<HierarchyAnswer> {
  let mut walk = Walk {
    graph,
    budget: MAX_HIERARCHY_NODES,
    truncated: false,
    cut_at_depth: false,
  };
  let mut path = vec![target.id];
  let supertypes = walk.level(target.id, Direction::Forward, depth, &mut path)?;
  let subtypes = walk.level(target.id, Direction::Reverse, depth, &mut path)?;
  let mut caveats = Vec::new();
  if walk.truncated {
    caveats.push(Caveat::Truncated { max_nodes: MAX_HIERARCHY_NODES });
  }
  if walk.cut_at_depth {
    caveats.push(Caveat::Note {
      message: format!("the hierarchy continues past depth {depth}; raise depth to see more"),
    });
  }
  Ok(HierarchyAnswer {
    target: EntityRef::of(target),
    supertypes,
    subtypes,
    caveats,
  })
}

struct Walk<'g, 'a, 'c> {
  graph: &'g Graph<'a, 'c>,
  budget: usize,
  truncated: bool,
  cut_at_depth: bool,
}

impl Walk<'_, '_, '_> {
  fn level(&mut self, from: EntityId, direction: Direction, depth: u32, path: &mut Vec<EntityId>) -> QueryResult<Vec<HierarchyNode>> {
    let mut steps = Vec::new();
    let mut seen = HashSet::new();
    for edge in self.graph.edges(from, direction, &HIERARCHY_KINDS)? {
      let other = match direction {
        Direction::Forward => edge.to,
        Direction::Reverse => edge.from,
      };
      if path.contains(&other) || !seen.insert((other, edge.kind.clone())) {
        continue;
      }
      let Some(entity) = self.graph.entity(other)? else { continue };
      steps.push((edge, entity));
    }
    if depth == 0 {
      self.cut_at_depth |= !steps.is_empty();
      return Ok(Vec::new());
    }
    steps.sort_by(|(left_edge, left), (right_edge, right)| (EntityRef::of(left).sort_key(), &left_edge.kind).cmp(&(EntityRef::of(right).sort_key(), &right_edge.kind)));
    let mut nodes = Vec::new();
    for (edge, entity) in steps {
      if self.budget == 0 {
        self.truncated = true;
        break;
      }
      self.budget -= 1;
      path.push(entity.id);
      let children = self.level(entity.id, direction, depth - 1, path)?;
      path.pop();
      nodes.push(HierarchyNode {
        entity: EntityRef::of(&entity),
        edge_kind: edge.kind.as_str().to_owned(),
        phrase: phrase(&edge.kind, direction, &edge.certainty),
        certainty: CertaintyTag::of(&edge.certainty),
        via: edge.attrs.get("via").and_then(serde_json::Value::as_str).map(str::to_owned),
        children,
      });
    }
    Ok(nodes)
  }
}
