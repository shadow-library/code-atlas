use atlas_core::{Certainty, ContentHash, EdgeId, EdgeKind, EntityId, Evidence, Owner, RelPath, RepoName, Span};
use atlas_semantic::RELATION_CALLS_ENDPOINT;
use atlas_store::{FreshnessPolicy, InferredQuery};
use serde::Serialize;

use crate::QueryEngine;
use crate::engine::Snippet;
use crate::error::QueryResult;
use crate::graph::Graph;
use crate::model::{AttrView, CertaintyTag, EntityRef, Location, Response, Targeted, short_name};
use crate::policy::Policy;
use crate::queries::common::resolve_one;
use crate::queries::inferred_links::LinkScan;
use crate::target::TargetFilter;

const LINEAGE_LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct ExplainRequest {
  pub target: String,
  pub filter: TargetFilter,
}

impl ExplainRequest {
  pub fn new(target: impl Into<String>) -> Self {
    Self {
      target: target.into(),
      filter: TargetFilter::default(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvidenceView {
  pub location: Location,
  pub producer: String,
  pub note: Option<String>,
  pub excerpt: Option<Snippet>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssertionView {
  pub owner: String,
  pub certainty: CertaintyTag,
  pub derived_in: String,
  pub evidence: Vec<EvidenceView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LineageStep {
  pub old_key: String,
  pub certainty: String,
  pub run: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InferredView {
  pub relation: String,
  pub subjects: Vec<String>,
  pub certainty: CertaintyTag,
  pub provider: String,
  pub model: String,
  pub rationale: String,
  pub llm_call: Option<i64>,
  pub created_run: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "subject", rename_all = "snake_case")]
pub enum ExplainAnswer {
  Entity {
    entity: EntityRef,
    certainty: CertaintyTag,
    attrs: Vec<AttrView>,
    assertions: Vec<AssertionView>,
    lineage: Vec<LineageStep>,
    inferred: Vec<InferredView>,
  },
  Edge {
    id: String,
    kind: String,
    from: Option<EntityRef>,
    to: Option<EntityRef>,
    certainty: CertaintyTag,
    attrs: Vec<AttrView>,
    assertions: Vec<AssertionView>,
  },
}

struct Pending {
  assertion: usize,
  evidence: usize,
  repo: RepoName,
  path: RelPath,
  span: Span,
  hash: ContentHash,
}

#[derive(Default)]
struct Collected {
  assertions: Vec<AssertionView>,
  pending: Vec<Pending>,
}

impl QueryEngine {
  pub fn explain(&self, request: &ExplainRequest) -> QueryResult<Response<Targeted<ExplainAnswer>>> {
    let mut freshness = self.barrier()?;
    let edge_spec = request.target.strip_prefix("edge:").unwrap_or(&request.target).parse::<EdgeId>().ok();
    let gathered = self.read(Policy::with_floor(0.0), |graph| -> QueryResult<Targeted<(ExplainAnswer, Vec<Pending>)>> {
      if let Some(id) = edge_spec
        && (request.target.starts_with("edge:") || graph.entity(EntityId::from_raw(id.raw()))?.is_none())
        && let Some(found) = explain_edge(graph, id)?
      {
        return Ok(Targeted::Found(found));
      }
      let entity = match resolve_one(self, graph, &request.target, &request.filter)? {
        Ok(entity) => entity,
        Err(outcome) => return Ok(outcome),
      };
      explain_entity(graph, &entity).map(Targeted::Found)
    })?;
    let answer = match gathered {
      Targeted::Found((mut answer, pending)) => {
        let assertions = match &mut answer {
          ExplainAnswer::Entity { assertions, .. } | ExplainAnswer::Edge { assertions, .. } => assertions,
        };
        let mut reader = self.sources(&mut freshness);
        for item in pending {
          let excerpt = reader.excerpt(&item.repo, &item.path, item.span.start_line, item.span.end_line, item.hash)?;
          if let Some(evidence) = assertions.get_mut(item.assertion).and_then(|assertion| assertion.evidence.get_mut(item.evidence)) {
            evidence.excerpt = Some(excerpt);
          }
        }
        Targeted::Found(answer)
      }
      Targeted::Ambiguous { spec, candidates } => Targeted::Ambiguous { spec, candidates },
      Targeted::NotFound { spec, hint } => Targeted::NotFound { spec, hint },
    };
    Ok(Response { freshness, answer })
  }
}

fn owner_text(owner: &Owner) -> String {
  match owner {
    Owner::File { repo, path } => format!("file {repo}:{path}"),
    Owner::Project { pass } => format!("project pass {pass}"),
  }
}

fn derived_in(graph: &Graph<'_, '_>, owner: &Owner) -> QueryResult<String> {
  Ok(match owner {
    Owner::File { repo, path } => match graph.tx().file(repo, path)?.and_then(|record| record.indexed_run) {
      Some(run) => format!("file last indexed in run {run}"),
      None => "file run unknown".to_owned(),
    },
    Owner::Project { pass } => format!("re-derived by pass {pass} on every run"),
  })
}

fn evidence_view(collected: &mut Collected, assertion: usize, evidence: &Evidence) {
  let index = collected.assertions[assertion].evidence.len();
  collected.assertions[assertion].evidence.push(EvidenceView {
    location: Location {
      repo: evidence.repo.to_string(),
      path: evidence.path.to_string(),
      line: Some(evidence.span.start_line),
      column: None,
    },
    producer: evidence.producer.to_string(),
    note: evidence.note.clone(),
    excerpt: None,
  });
  collected.pending.push(Pending {
    assertion,
    evidence: index,
    repo: evidence.repo.clone(),
    path: evidence.path.clone(),
    span: evidence.span,
    hash: evidence.content_hash,
  });
}

fn explain_edge(graph: &Graph<'_, '_>, id: EdgeId) -> QueryResult<Option<(ExplainAnswer, Vec<Pending>)>> {
  let Some(edge) = graph.tx().edge(id)? else { return Ok(None) };
  let mut collected = Collected::default();
  for assertion in graph.tx().edge_assertions(id)? {
    let index = collected.assertions.len();
    collected.assertions.push(AssertionView {
      owner: owner_text(&assertion.owner),
      certainty: CertaintyTag::of(&assertion.fact.certainty),
      derived_in: derived_in(graph, &assertion.owner)?,
      evidence: Vec::new(),
    });
    for evidence in &assertion.fact.evidence {
      evidence_view(&mut collected, index, evidence);
    }
  }
  let answer = ExplainAnswer::Edge {
    id: edge.id.to_string(),
    kind: edge.kind.as_str().to_owned(),
    from: graph.entity_ref(edge.from)?,
    to: graph.entity_ref(edge.to)?,
    certainty: CertaintyTag::of(&edge.certainty),
    attrs: graph.edge_attrs(edge.id, &[])?,
    assertions: collected.assertions,
  };
  Ok(Some((answer, collected.pending)))
}

fn explain_entity(graph: &Graph<'_, '_>, entity: &atlas_core::Entity) -> QueryResult<(ExplainAnswer, Vec<Pending>)> {
  let mut collected = Collected::default();
  for assertion in graph.tx().entity_assertions(entity.id)? {
    let index = collected.assertions.len();
    collected.assertions.push(AssertionView {
      owner: owner_text(&assertion.owner),
      certainty: CertaintyTag::of(&assertion.fact.certainty),
      derived_in: derived_in(graph, &assertion.owner)?,
      evidence: Vec::new(),
    });
    let fact = &assertion.fact;
    if let (Some(repo), Some(path), Some(span)) = (&fact.repo, &fact.path, fact.span)
      && let Some(record) = graph.tx().file(repo, path)?
    {
      let evidence = Evidence {
        repo: repo.clone(),
        path: path.clone(),
        span,
        content_hash: record.content_hash,
        producer: record.analyzer.unwrap_or_else(|| atlas_core::Producer::new("unknown", 0)),
        note: None,
      };
      evidence_view(&mut collected, index, &evidence);
    }
  }
  let mut lineage = Vec::new();
  let mut current = entity.id;
  for _ in 0..LINEAGE_LIMIT {
    let Some(edge) = graph.edges_out(current, &[EdgeKind::RenamedFrom])?.into_iter().next() else {
      break;
    };
    lineage.push(LineageStep {
      old_key: edge
        .attrs
        .get("old_key")
        .and_then(serde_json::Value::as_str)
        .map_or_else(|| edge.to.to_string(), str::to_owned),
      certainty: edge.certainty.tag(),
      run: edge.attrs.get("run").and_then(serde_json::Value::as_i64),
    });
    current = edge.to;
  }
  let mut inferred = Vec::new();
  let mut links = LinkScan::default();
  for fact in graph.tx().inferred_facts(&InferredQuery {
    subject: Some(entity.id),
    relation: None,
    freshness: FreshnessPolicy::IncludeStale,
  })? {
    let (provider, model, rationale) = match &fact.certainty {
      Certainty::Inferred { provider, model, rationale, .. } => (provider.clone(), model.clone(), rationale.clone()),
      _ => (String::new(), String::new(), String::new()),
    };
    let mut subjects = Vec::new();
    for subject in &fact.subjects {
      subjects.push(graph.entity(*subject)?.map_or_else(|| subject.to_string(), |entity| short_name(&entity.key)));
    }
    let freshness = match fact.relation == RELATION_CALLS_ENDPOINT {
      true => links.explained_freshness(graph, &fact)?,
      false => fact.freshness.clone(),
    };
    inferred.push(InferredView {
      relation: fact.relation.clone(),
      subjects,
      certainty: CertaintyTag::inferred(&fact.certainty, &freshness),
      provider,
      model,
      rationale,
      llm_call: fact.llm_call.map(|call| call.0),
      created_run: fact.created_run.map(|run| run.0),
    });
  }
  let answer = ExplainAnswer::Entity {
    entity: EntityRef::of(entity),
    certainty: CertaintyTag::of(&entity.certainty),
    attrs: graph.entity_attrs(entity.id, &[])?,
    assertions: collected.assertions,
    lineage,
    inferred,
  };
  Ok((answer, collected.pending))
}
