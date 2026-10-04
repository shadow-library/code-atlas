use std::collections::BTreeMap;

use atlas_core::{ContentHash, Freshness, RelPath, RepoName, RunId};
use atlas_store::{InferredFactId, StoredInferredFact};

use crate::engine::{Job, JobOrder, SemanticEngine, SemanticReport, SkipReason};
use crate::error::SemanticResult;
use crate::task::SemanticTask;

#[derive(Debug, Clone, PartialEq)]
pub struct RevalidationReport {
  pub unchanged: Vec<InferredFactId>,
  pub skipped: Vec<(InferredFactId, SkipReason)>,
  pub rerun: SemanticReport,
}

impl SemanticEngine<'_> {
  pub fn revalidate(
    &self,
    run: Option<RunId>,
    facts: &[StoredInferredFact],
    current_hash: impl Fn(&RepoName, &RelPath) -> Option<ContentHash>,
    rebuild: impl Fn(&StoredInferredFact) -> Option<SemanticTask>,
  ) -> SemanticResult<RevalidationReport> {
    let (due, unchanged): (Vec<&StoredInferredFact>, Vec<&StoredInferredFact>) = facts
      .iter()
      .partition(|fact| !fact.freshness.is_current() || !Freshness::of_evidence(&fact.evidence, &current_hash).is_current());
    self.mark_changed_evidence_stale(&due, &current_hash)?;

    let mut jobs = Vec::new();
    let mut skipped = Vec::new();
    for fact in due {
      let Some(task) = rebuild(fact) else {
        skipped.push((fact.id, SkipReason::SourcesUnavailable));
        continue;
      };
      if !task.subjects().iter().all(|subject| fact.subjects.contains(subject)) {
        skipped.push((fact.id, SkipReason::RebuiltTaskMismatch));
        continue;
      }
      if !task.sources().iter().all(|source| current_hash(&source.repo, &source.path) == Some(source.file_hash)) {
        skipped.push((fact.id, SkipReason::EvidenceChanged));
        continue;
      }
      jobs.push(Job { task, replaces: Some(fact.id) });
    }
    let rerun = self.execute(run, jobs, JobOrder::ByKindAndSubjects)?;
    Ok(RevalidationReport {
      unchanged: unchanged.into_iter().map(|fact| fact.id).collect(),
      skipped,
      rerun,
    })
  }

  fn mark_changed_evidence_stale(&self, due: &[&StoredInferredFact], current_hash: &impl Fn(&RepoName, &RelPath) -> Option<ContentHash>) -> SemanticResult<()> {
    let mut changed: BTreeMap<(RepoName, RelPath), Option<ContentHash>> = BTreeMap::new();
    for evidence in due.iter().flat_map(|fact| &fact.evidence) {
      let now = current_hash(&evidence.repo, &evidence.path);
      if now != Some(evidence.content_hash) {
        changed.insert((evidence.repo.clone(), evidence.path.clone()), now);
      }
    }
    if changed.is_empty() {
      return Ok(());
    }
    self.store().write(|tx| -> SemanticResult<()> {
      for ((repo, path), hash) in &changed {
        match hash {
          Some(hash) => tx.mark_stale_where_evidence_changed(repo, path, hash)?,
          None => tx.mark_stale_where_evidence_removed(repo, path)?,
        };
      }
      Ok(())
    })
  }
}
