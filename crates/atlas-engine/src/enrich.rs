use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use atlas_core::{ContentHash, EntityId, RelPath, RepoName, RunId};
use atlas_semantic::{
  RevalidationReport, SemanticBudget, SemanticConfig, SemanticEngine, SemanticReport, SemanticTask, SimilarityPair, SourceRef, SymbolSnippet, TaskProviders,
  providers_from_settings,
};
use atlas_store::{FreshnessPolicy, InferredQuery, RunOutcome, RunReport, StoredInferredFact};

use crate::engine::Engine;
use crate::error::EngineResult;

const STRUCTURAL_THRESHOLD: f64 = 0.6;

#[derive(Debug, Clone, PartialEq)]
pub struct EnrichReport {
  pub run: RunId,
  pub candidates: u64,
  pub candidates_without_source: u64,
  pub similarity: SemanticReport,
  pub revalidation: RevalidationReport,
}

impl Engine {
  pub fn enrich(&self, budget: SemanticBudget) -> EngineResult<EnrichReport> {
    let providers: Arc<TaskProviders> = match &self.options.llm_providers {
      Some(providers) => Arc::clone(providers),
      None => Arc::new(providers_from_settings(&self.project().settings.llm)),
    };
    let run = self.store.write(|tx| -> EngineResult<RunId> { Ok(tx.begin_run("enrich")?) })?;
    let pairs = self.similarity_candidates(budget.max_calls_per_run as usize);
    let candidates = pairs.len() as u64;
    let tasks: Vec<SemanticTask> = pairs
      .into_iter()
      .filter_map(|(left, right)| SimilarityPair::new(self.snippet(left)?, self.snippet(right)?).ok())
      .map(SemanticTask::ClassifySimilarityPair)
      .collect();
    let candidates_without_source = candidates - tasks.len() as u64;
    let config = SemanticConfig {
      budget,
      ..SemanticConfig::default()
    };
    let semantic = SemanticEngine::with_task_providers(&providers, &self.store, config);
    let similarity = semantic.run(Some(run), tasks)?;
    let due = self.store.read(|tx| {
      tx.inferred_facts(&InferredQuery {
        subject: None,
        relation: None,
        freshness: FreshnessPolicy::IncludeStale,
      })
    })?;
    let due: Vec<StoredInferredFact> = due.into_iter().filter(|fact| !fact.freshness.is_current()).collect();
    let current_hashes = self.current_hashes(&due)?;
    let revalidation = semantic.revalidate(
      Some(run),
      &due,
      |repo, path| current_hashes.get(&(repo.clone(), path.clone())).copied(),
      |fact| self.rebuild_similarity_task(fact),
    )?;
    let counts = BTreeMap::from([
      ("candidates".to_owned(), candidates),
      ("provider_calls".to_owned(), u64::from(similarity.provider_calls + revalidation.rerun.provider_calls)),
      ("cache_hits".to_owned(), u64::from(similarity.cache_hits + revalidation.rerun.cache_hits)),
      ("revalidation_due".to_owned(), due.len() as u64),
    ]);
    let report = RunReport {
      phase_timings_ms: BTreeMap::new(),
      counts,
      change_set: serde_json::Value::Null,
    };
    self.store.write(|tx| -> EngineResult<()> { Ok(tx.finish_run(run, RunOutcome::Ok, &report)?) })?;
    tracing::info!(run = run.0, candidates, status = ?similarity.status, "semantic enrichment finished");
    Ok(EnrichReport {
      run,
      candidates,
      candidates_without_source,
      similarity,
      revalidation,
    })
  }

  fn similarity_candidates(&self, limit: usize) -> Vec<(EntityId, EntityId)> {
    let index = self.similarity();
    let mut pairs = Vec::new();
    for cluster in index.clusters(STRUCTURAL_THRESHOLD).clusters {
      for (position, left) in cluster.members.iter().enumerate() {
        for right in &cluster.members[position + 1..] {
          if pairs.len() >= limit {
            return pairs;
          }
          pairs.push((*left, *right));
        }
      }
    }
    pairs
  }

  fn snippet(&self, entity: EntityId) -> Option<SymbolSnippet> {
    let found = self.store.read(|tx| tx.entity(entity)).ok()??;
    let (repo, path, span) = (found.repo.clone()?, found.path.clone()?, found.span?);
    let (source, file_hash) = self.read_source(&repo, &path).ok()?;
    let snippet = span.text(&source)?.to_owned();
    Some(SymbolSnippet {
      entity,
      key: found.key.to_string(),
      signature: found.signature.clone(),
      snippet,
      source: SourceRef { repo, path, span, file_hash },
      callers: None,
      test_names: Vec::new(),
    })
  }

  fn current_hashes(&self, facts: &[StoredInferredFact]) -> EngineResult<BTreeMap<(RepoName, RelPath), ContentHash>> {
    let files: BTreeSet<(RepoName, RelPath)> = facts
      .iter()
      .flat_map(|fact| fact.evidence.iter().map(|evidence| (evidence.repo.clone(), evidence.path.clone())))
      .collect();
    self.store.read(|tx| -> EngineResult<_> {
      let mut hashes = BTreeMap::new();
      for (repo, path) in files {
        if let Some(record) = tx.file(&repo, &path)? {
          hashes.insert((repo, path), record.content_hash);
        }
      }
      Ok(hashes)
    })
  }

  fn rebuild_similarity_task(&self, fact: &StoredInferredFact) -> Option<SemanticTask> {
    let [left, right] = fact.subjects.as_slice() else { return None };
    let pair = SimilarityPair::new(self.snippet(*left)?, self.snippet(*right)?).ok()?;
    Some(SemanticTask::ClassifySimilarityPair(pair))
  }
}
