use atlas_core::{Edge, EdgeKind, InferredFact};
use atlas_store::StoredInferredFact;

pub const LOW_CONFIDENCE_ATTR: &str = "low_confidence";

pub const RELATION_EQUIVALENT_TO: &str = "equivalent_to";
pub const RELATION_SIMILAR_TO: &str = "similar_to";
pub const RELATION_CALLS_ENDPOINT: &str = "calls_endpoint";
pub const RELATION_CAPABILITY_SUMMARY: &str = "capability_summary";

pub fn is_low_confidence(fact: &StoredInferredFact) -> bool {
  fact.payload.get(LOW_CONFIDENCE_ATTR).and_then(serde_json::Value::as_bool).unwrap_or(false)
}

pub fn included_in_default_results(fact: &StoredInferredFact) -> bool {
  fact.freshness.is_current() && !is_low_confidence(fact)
}

pub fn inferred_edge(fact: &StoredInferredFact) -> Option<InferredFact<Edge>> {
  let kind = match fact.relation.as_str() {
    RELATION_EQUIVALENT_TO => EdgeKind::EquivalentTo,
    RELATION_SIMILAR_TO => EdgeKind::SimilarTo,
    RELATION_CALLS_ENDPOINT => EdgeKind::CallsEndpoint,
    _ => return None,
  };
  let [from, to] = fact.subjects[..] else { return None };
  let mut edge = Edge::new(kind, from, to, "", fact.certainty.clone());
  edge.evidence = fact.evidence.clone();
  if let Some(payload) = fact.payload.as_object() {
    edge.attrs = payload.iter().map(|(name, value)| (name.clone(), value.clone())).collect();
  }
  Some(InferredFact {
    fact: edge,
    freshness: fact.freshness.clone(),
  })
}
