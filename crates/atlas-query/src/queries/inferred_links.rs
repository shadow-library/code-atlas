use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{Edge, Entity, EntityId, EntityKind, Freshness};
use atlas_engine::{Agreement, LinkContext, LinkFactState, link_fact_state};
use atlas_semantic::{RELATION_CALLS_ENDPOINT, inferred_edge};
use atlas_store::{FreshnessPolicy, InferredFactId, InferredQuery, StoredInferredFact};

use crate::error::QueryResult;
use crate::graph::Graph;
use crate::model::{Caveat, CertaintyTag, Hop};
use crate::policy::InferredAdmission;

const CANDIDATES_CHANGED: &str = "candidate endpoints changed since the model chose";
const VANISHED: &str = "client call or endpoint no longer exists";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkEnd {
  Call,
  Endpoint,
}

impl LinkEnd {
  fn position(self) -> usize {
    match self {
      Self::Call => 0,
      Self::Endpoint => 1,
    }
  }
}

pub(crate) struct InferredLink {
  pub call: Entity,
  pub endpoint: Entity,
  pub edge: Edge,
  pub hop: Hop,
  pub rationale: String,
}

#[derive(Default)]
pub(crate) struct LinkScan {
  context: Option<LinkContext>,
  states: BTreeMap<InferredFactId, LinkFactState>,
  stale: BTreeSet<InferredFactId>,
  low_confidence: BTreeSet<InferredFactId>,
  below_floor: BTreeSet<InferredFactId>,
}

impl LinkScan {
  fn state(&mut self, graph: &Graph<'_, '_>, fact: &StoredInferredFact) -> QueryResult<LinkFactState> {
    if let Some(state) = self.states.get(&fact.id) {
      return Ok(state.clone());
    }
    let context = match self.context.take() {
      Some(context) => context,
      None => LinkContext::load(graph.tx())?,
    };
    let state = link_fact_state(graph.tx(), &context, fact);
    self.context = Some(context);
    let state = state?;
    self.states.insert(fact.id, state.clone());
    Ok(state)
  }

  pub(crate) fn explained_freshness(&mut self, graph: &Graph<'_, '_>, fact: &StoredInferredFact) -> QueryResult<Freshness> {
    let settled = |reason: &str| Freshness::Stale { reason: reason.to_owned() };
    Ok(match self.state(graph, fact)? {
      LinkFactState::Vanished => settled(VANISHED),
      LinkFactState::Superseded { agreement, link } => {
        let outcome = match agreement {
          Agreement::SameEndpoint => ", which agrees",
          Agreement::DifferentEndpoint => " to a different endpoint",
        };
        settled(&format!("superseded by a static-analysis link for this call{outcome} {}", CertaintyTag::of(&link).tag))
      }
      LinkFactState::CandidatesChanged if fact.freshness.is_current() => Freshness::NeedsRevalidation {
        reason: CANDIDATES_CHANGED.to_owned(),
      },
      LinkFactState::CandidatesChanged | LinkFactState::Current => fact.freshness.clone(),
    })
  }

  fn effective_freshness(&mut self, graph: &Graph<'_, '_>, fact: &StoredInferredFact) -> QueryResult<Option<Freshness>> {
    if self.state(graph, fact)?.is_settled() {
      return Ok(None);
    }
    self.explained_freshness(graph, fact).map(Some)
  }

  pub(crate) fn inferred_hidden(&self) -> Option<Caveat> {
    (!self.stale.is_empty() || !self.low_confidence.is_empty()).then_some(Caveat::InferredHidden {
      stale: self.stale.len(),
      low_confidence: self.low_confidence.len(),
    })
  }

  pub(crate) fn below_floor(&self) -> usize {
    self.below_floor.len()
  }

  pub(crate) fn caveats(&self, floor: f32) -> Vec<Caveat> {
    let below = (self.below_floor() > 0).then_some(Caveat::BelowConfidenceFloor {
      hidden: self.below_floor(),
      fallback_tests: 0,
      floor,
    });
    self.inferred_hidden().into_iter().chain(below).collect()
  }
}

fn admitted(graph: &Graph<'_, '_>, fact: &StoredInferredFact, hidden: &mut LinkScan) -> bool {
  match graph.policy.inferred_admission(fact) {
    InferredAdmission::StaleHidden => {
      hidden.stale.insert(fact.id);
      false
    }
    InferredAdmission::LowConfidence => {
      hidden.low_confidence.insert(fact.id);
      false
    }
    InferredAdmission::Current | InferredAdmission::StaleShown if graph.policy.admits(&fact.certainty) => true,
    InferredAdmission::Current | InferredAdmission::StaleShown => {
      hidden.below_floor.insert(fact.id);
      false
    }
  }
}

pub(crate) fn inferred_links(graph: &Graph<'_, '_>, subject: EntityId, end: LinkEnd, hidden: &mut LinkScan) -> QueryResult<Vec<InferredLink>> {
  let facts = graph.tx().inferred_facts(&InferredQuery {
    subject: Some(subject),
    relation: Some(RELATION_CALLS_ENDPOINT.to_owned()),
    freshness: FreshnessPolicy::IncludeStale,
  })?;
  let mut links = Vec::new();
  for mut fact in facts.into_iter().filter(|fact| fact.subjects.get(end.position()) == Some(&subject)) {
    let Some(freshness) = hidden.effective_freshness(graph, &fact)? else { continue };
    fact.freshness = freshness;
    let Some(inferred) = inferred_edge(&fact) else { continue };
    let edge = inferred.fact;
    let (Some(call), Some(endpoint)) = (graph.entity(edge.from)?, graph.entity(edge.to)?) else {
      continue;
    };
    if call.kind != EntityKind::ApiClientCall || !admitted(graph, &fact, hidden) {
      continue;
    }
    let mut hop = graph.relation(&edge, call.id)?;
    hop.edge = format!("inferred:{}", fact.id.0);
    hop.certainty = CertaintyTag::inferred(&fact.certainty, &fact.freshness);
    let rationale = fact.payload.get("rationale").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
    links.push(InferredLink {
      call,
      endpoint,
      edge,
      hop,
      rationale,
    });
  }
  links.sort_by(|left, right| {
    let confidence = |link: &InferredLink| link.hop.certainty.confidence.unwrap_or(0.0);
    confidence(right)
      .total_cmp(&confidence(left))
      .then_with(|| left.call.key.to_string().cmp(&right.call.key.to_string()))
  });
  Ok(links)
}
