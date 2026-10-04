use atlas_core::{EdgeKind, Entity, Visibility};
use serde::Serialize;

use crate::QueryEngine;
use crate::engine::{Snippet, SnippetMode};
use crate::error::QueryResult;
use crate::graph::Graph;
use crate::model::{CertaintyTag, EntityRef, Response, Targeted};
use crate::policy::{Direction, Policy};
use crate::queries::common::{first_line, resolve_one};
use crate::queries::tests_for::direct_tests;
use crate::target::TargetFilter;

#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRequest {
  pub target: String,
  pub filter: TargetFilter,
  pub body: bool,
}

impl SymbolRequest {
  pub fn new(target: impl Into<String>) -> Self {
    Self {
      target: target.into(),
      filter: TargetFilter::default(),
      body: false,
    }
  }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RelationCount {
  pub total: usize,
  pub uncertain: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SymbolCounts {
  pub callers: RelationCount,
  pub callees: RelationCount,
  pub references: RelationCount,
  pub tests: RelationCount,
  pub implementors: RelationCount,
  pub children: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SymbolAnswer {
  pub entity: EntityRef,
  pub certainty: CertaintyTag,
  pub signature: Option<String>,
  pub doc: Option<String>,
  pub visibility: Option<String>,
  pub parent: Option<EntityRef>,
  pub counts: SymbolCounts,
  pub body: Option<Snippet>,
}

impl QueryEngine {
  pub fn symbol(&self, request: &SymbolRequest) -> QueryResult<Response<Targeted<SymbolAnswer>>> {
    let mut freshness = self.barrier()?;
    let answer = self.read(Policy::with_floor(0.0), |graph| -> QueryResult<Targeted<SymbolAnswer>> {
      let entity = match resolve_one(self, graph, &request.target, &request.filter)? {
        Ok(entity) => entity,
        Err(outcome) => return Ok(outcome),
      };
      describe(graph, &entity).map(Targeted::Found)
    })?;
    let answer = match (answer, request.body) {
      (Targeted::Found(mut found), true) => {
        found.body = Some(self.sources(&mut freshness).snippet(found.entity.entity_id()?, SnippetMode::Body { max_lines: None }, 0)?);
        Targeted::Found(found)
      }
      (answer, _) => answer,
    };
    Ok(Response { freshness, answer })
  }
}

fn describe(graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<SymbolAnswer> {
  let callers = count(graph, entity, Direction::Reverse, &[EdgeKind::Calls, EdgeKind::Instantiates])?;
  let callees = count(graph, entity, Direction::Forward, &[EdgeKind::Calls, EdgeKind::Instantiates])?;
  let references = count(graph, entity, Direction::Reverse, &[EdgeKind::References, EdgeKind::TypeOf])?;
  let implementors = count(graph, entity, Direction::Reverse, &[EdgeKind::Implements, EdgeKind::Extends])?;
  let tests = direct_tests(graph, entity)?;
  let tests = RelationCount {
    total: tests.len(),
    uncertain: tests.iter().filter(|test| !test.relation.certainty.authoritative).count(),
  };
  Ok(SymbolAnswer {
    entity: EntityRef::of(entity),
    certainty: CertaintyTag::of(&entity.certainty),
    signature: entity.signature.clone(),
    doc: first_line(entity.doc.as_deref()),
    visibility: entity.visibility.as_ref().map(visibility_label),
    parent: graph.parent(entity.id)?.as_ref().map(EntityRef::of),
    counts: SymbolCounts {
      callers,
      callees,
      references,
      tests,
      implementors,
      children: graph.children(entity.id)?.len(),
    },
    body: None,
  })
}

fn count(graph: &Graph<'_, '_>, entity: &Entity, direction: Direction, kinds: &[EdgeKind]) -> QueryResult<RelationCount> {
  let edges = graph.edges(entity.id, direction, kinds)?;
  Ok(RelationCount {
    total: edges.len(),
    uncertain: edges.iter().filter(|edge| !edge.certainty.is_authoritative()).count(),
  })
}

pub(crate) fn visibility_label(visibility: &Visibility) -> String {
  match visibility {
    Visibility::Public => "public".to_owned(),
    Visibility::Crate => "crate".to_owned(),
    Visibility::Restricted(scope) => format!("restricted({scope})"),
    Visibility::Protected => "protected".to_owned(),
    Visibility::Private => "private".to_owned(),
    Visibility::Exported => "exported".to_owned(),
    Visibility::ExportedDefault => "exported default".to_owned(),
    Visibility::Module => "module".to_owned(),
  }
}
