mod context;
mod domains;
mod edges;
mod entities;
mod queries;
mod unresolved;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use self::context::Ctx;
use super::corpus::{Corpus, IndexedProject};
use super::error::QualityResult;
use super::metrics::{Calibration, Prohibition, Recall};
use super::selector::BindingError;
use super::truth::Truth;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
  pub section: String,
  pub kind: String,
  pub label: String,
  pub outcome: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeKindScore {
  pub recall: Recall,
  pub optional_expected: u64,
  pub optional_present: u64,
  pub optional_wrong_certainty: u64,
  pub forbidden: Prohibition,
  pub exhaustive_unexpected: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityScore {
  pub by_kind: BTreeMap<String, Recall>,
  pub total: Recall,
  pub handlers: Recall,
  pub best_effort_expected: u64,
  pub best_effort_found: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedScore {
  pub presence: Recall,
  pub approximate_reason: u64,
  pub fabrication: Prohibition,
  pub not_checkable: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetScore {
  pub expected: u64,
  pub predicted: u64,
  pub correct: u64,
}

impl SetScore {
  pub fn precision(&self) -> Option<f64> {
    super::metrics::ratio(self.correct, self.predicted)
  }

  pub fn recall(&self) -> Option<f64> {
    super::metrics::ratio(self.correct, self.expected)
  }

  pub fn absorb(&mut self, other: &SetScore) {
    self.expected += other.expected;
    self.predicted += other.predicted;
    self.correct += other.correct;
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthScore {
  pub supported: SetScore,
  pub used: SetScore,
  pub unused: SetScore,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnusedScore {
  pub expected_unused: Recall,
  pub reported_likely: u64,
  pub reported_possibly: u64,
  pub not_unused: Prohibition,
  pub not_unused_hedged: u64,
  pub ambiguous_reported: u64,
  pub ambiguous_total: u64,
}

impl UnusedScore {
  pub fn precision(&self) -> Option<f64> {
    super::metrics::ratio(self.reported_likely, self.reported_likely + self.not_unused.violations)
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimilarFinding {
  pub a: String,
  pub b: String,
  pub expected: String,
  pub acceptable: Vec<String>,
  pub surfaced: bool,
  pub structural_jaccard: Option<f64>,
  pub system_verdict: Option<String>,
  pub inferred_verdict: Option<String>,
  pub deterministic_claim: bool,
  pub scope: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SimilarScore {
  pub pairs: Vec<SimilarFinding>,
  pub claims_checked: u64,
  pub claims_violated: u64,
  pub verdicts_inferred: u64,
  pub verdicts_correct: u64,
  pub verdicts_undecided: u64,
  pub not_evaluable: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlastScore {
  pub must_include: Recall,
  pub must_not_include: Prohibition,
  pub may_include_expected: u64,
  pub may_include_present: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IsolationScore {
  pub checked: Prohibition,
  pub not_evaluated: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixtureReport {
  pub name: String,
  pub repos: Vec<String>,
  pub files: usize,
  pub entities_indexed: usize,
  pub edges_indexed: usize,
  pub index_degraded: Option<String>,
  pub entities: EntityScore,
  pub edge_kinds: BTreeMap<String, EdgeKindScore>,
  pub edges: Recall,
  pub forbidden: Prohibition,
  pub exhaustive_kinds: Vec<String>,
  pub unresolved: UnresolvedScore,
  pub unmatched_calls: Recall,
  pub auth: AuthScore,
  pub endpoint_auth: Recall,
  pub tests_of: Recall,
  pub unused: UnusedScore,
  pub similar: SimilarScore,
  pub blast_radius: BlastScore,
  pub isolation: IsolationScore,
  pub file_flags: Recall,
  pub calibration: Calibration,
  pub binding_errors: Vec<BindingError>,
  pub findings: Vec<Finding>,
}

pub fn evaluate_fixture(name: &str, truth: &Truth, project: &IndexedProject, corpus: &Corpus) -> QualityResult<FixtureReport> {
  let ctx = Ctx::new(name, truth, project, corpus);
  let entities = entities::evaluate(&ctx);
  let edge_scores = edges::evaluate(&ctx);
  let unresolved = unresolved::evaluate(&ctx);
  let domain = domains::evaluate(&ctx)?;
  let query = queries::evaluate(&ctx)?;
  let (binding_errors, findings) = ctx.finish();
  Ok(FixtureReport {
    name: name.to_owned(),
    repos: truth.project.repos.clone(),
    files: project.snapshot.files.len(),
    entities_indexed: project.snapshot.entities.len(),
    edges_indexed: project.snapshot.edges.len(),
    index_degraded: project.degraded.clone(),
    entities,
    edge_kinds: edge_scores.by_kind,
    edges: edge_scores.total,
    forbidden: edge_scores.forbidden,
    exhaustive_kinds: truth.project.exhaustive_edge_kinds.clone(),
    unresolved,
    unmatched_calls: domain.unmatched_calls,
    auth: domain.auth,
    endpoint_auth: domain.endpoint_auth,
    tests_of: query.tests_of,
    unused: query.unused,
    similar: query.similar,
    blast_radius: query.blast_radius,
    isolation: query.isolation,
    file_flags: domain.file_flags,
    calibration: edge_scores.calibration,
    binding_errors,
    findings,
  })
}
