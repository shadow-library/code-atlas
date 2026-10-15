use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{Diagnostic, EdgeKind, EntityKey, Freshness, Owner, ParseStatus, UnresolvedReason};
use atlas_detect::{DecoratorFamily, conflicting_families};
use atlas_scip::ScipFacts;
use atlas_store::{FreshnessPolicy, InferredQuery, OwnedFacts};

use crate::engine::{Engine, lock_inner};
use crate::error::EngineResult;
use crate::passes::{PASS_LINEAGE, PASS_REPOS, PASS_STRUCTURE};
use crate::report::{
  BudgetRetryStatus, FailedFile, IndexStatus, InferredCounts, LlmSpend, RouteFamilyState, RouteFamilyStatus, ScanKind, ScipCoverage, ServerMatches, SkippedSummary,
};

const STATUS_RUNS: usize = 10;
const SPEND_RUNS: usize = 1_000;

fn reason_label(reason: &UnresolvedReason) -> String {
  match reason {
    UnresolvedReason::NotFound => "not_found".to_owned(),
    UnresolvedReason::Ambiguous { .. } => "ambiguous".to_owned(),
    UnresolvedReason::Dynamic => "dynamic".to_owned(),
    UnresolvedReason::External { .. } => "external".to_owned(),
    UnresolvedReason::Unsupported { detail } => format!("unsupported:{detail}"),
    _ => "other".to_owned(),
  }
}

fn parse_label(status: &ParseStatus) -> &'static str {
  match status {
    ParseStatus::Ok => "ok",
    ParseStatus::Partial { .. } => "partial",
    ParseStatus::Failed { .. } => "failed",
  }
}

#[derive(Default)]
struct FactTally {
  unresolved_by_reason: BTreeMap<String, u64>,
  edge_certainty: BTreeMap<String, BTreeMap<String, u64>>,
}

impl FactTally {
  fn add(&mut self, facts: &OwnedFacts) {
    for reference in &facts.unresolved {
      *self.unresolved_by_reason.entry(reason_label(&reference.reason)).or_default() += 1;
    }
    for edge in &facts.edges {
      let classes = self.edge_certainty.entry(edge.kind.as_str().to_owned()).or_default();
      *classes.entry(edge.certainty.class().as_str().to_owned()).or_default() += 1;
    }
  }
}

#[derive(Default)]
struct RouteTally {
  controllers: BTreeMap<String, BTreeSet<(String, String)>>,
  routes: BTreeMap<String, u64>,
  client_calls: BTreeMap<String, u64>,
  definition_conflicts: BTreeMap<String, u64>,
}

impl RouteTally {
  fn add(&mut self, owner: &str, facts: &OwnedFacts) {
    for entity in facts.entities.iter().filter(|entity| matches!(entity.key, EntityKey::Endpoint { .. })) {
      let Some(framework) = entity.attrs.get("framework").and_then(serde_json::Value::as_str) else {
        continue;
      };
      *self.routes.entry(framework.to_owned()).or_default() += 1;
      if let Some(controller) = entity.attrs.get("controller").and_then(serde_json::Value::as_str) {
        self.controllers.entry(framework.to_owned()).or_default().insert((owner.to_owned(), controller.to_owned()));
      }
    }
    for diagnostic in facts
      .entities
      .iter()
      .filter_map(|entity| entity.attrs.get("detector_diagnostics"))
      .filter_map(serde_json::Value::as_array)
      .flatten()
    {
      let Ok(diagnostic) = serde_json::from_value::<Diagnostic>(diagnostic["diagnostic"].clone()) else {
        continue;
      };
      for family in conflicting_families(&diagnostic) {
        *self.definition_conflicts.entry(family.to_owned()).or_default() += 1;
      }
    }
    for edge in facts.edges.iter().filter(|edge| edge.kind == EdgeKind::MakesRequest) {
      let Some(library) = edge.attrs.get("library").and_then(serde_json::Value::as_str) else {
        continue;
      };
      *self.client_calls.entry(library.to_owned()).or_default() += edge.evidence.len() as u64;
    }
  }

  fn matched(&self, family: &DecoratorFamily) -> RouteFamilyState {
    let name = family.name();
    RouteFamilyState::Matched {
      server: family.has_routes().then(|| ServerMatches {
        controllers: self.controllers.get(name).map_or(0, |controllers| controllers.len() as u64),
        routes: self.routes.get(name).copied().unwrap_or(0),
      }),
      client_calls: family.has_client().then(|| self.client_calls.get(name).copied().unwrap_or(0)),
      definition_conflicts: self.definition_conflicts.get(name).copied().unwrap_or(0),
    }
  }
}

impl Engine {
  fn route_family_status(&self, tally: &RouteTally) -> Vec<RouteFamilyStatus> {
    let detect = &self.project().settings.detect;
    detect
      .route_families
      .iter()
      .map(|name| RouteFamilyStatus {
        name: name.clone(),
        state: match detect.families.iter().find(|family| family.name() == name) {
          Some(family) => tally.matched(family),
          None => RouteFamilyState::Undeclared {
            path: self.home.detectors_dir().join(format!("{name}.toml")).display().to_string(),
          },
        },
      })
      .collect()
  }

  pub fn status(&self) -> EngineResult<IndexStatus> {
    let (scip_meta, header, dangling, missing, skipped, budget_retries) = {
      let inner = lock_inner(&self.inner);
      let state = &inner.state;
      let dangling = state.files.values().map(|file| file.dangling).sum::<u64>();
      let missing: Vec<FailedFile> = state
        .repos
        .iter()
        .filter_map(|(name, repo)| match &repo.health {
          crate::state::RepoHealth::Present => None,
          crate::state::RepoHealth::Missing { reason } => Some(FailedFile {
            repo: name.clone(),
            path: None,
            reason: format!("repository is unavailable: {reason}"),
          }),
        })
        .collect();
      (
        state.scip.clone(),
        self.header(state, None, 0, std::time::Duration::ZERO, ScanKind::StatScan),
        dangling,
        missing,
        SkippedSummary::of(state.repos.values().flat_map(|repo| &repo.skipped)),
        state
          .budget_retries
          .iter()
          .map(|((repo, path), retry)| BudgetRetryStatus {
            repo: repo.clone(),
            path: path.clone(),
            attempts: retry.attempts,
            retry_at_ms: retry.retry_at_ms,
          })
          .collect(),
      )
    };
    let passes: Vec<String> = [PASS_REPOS, PASS_STRUCTURE, atlas_link::PASS_NAME, PASS_LINEAGE]
      .into_iter()
      .map(str::to_owned)
      .chain(self.options.passes.iter().map(|pass| pass.name().to_owned()))
      .collect();
    let project = self.project();
    self.store.read(|tx| -> EngineResult<IndexStatus> {
      let mut files_by_language: BTreeMap<String, u64> = BTreeMap::new();
      let mut files_by_parse_status: BTreeMap<String, u64> = BTreeMap::new();
      let mut failed_files = Vec::new();
      let mut tally = FactTally::default();
      let mut routes = RouteTally::default();
      let mut scip = ScipCoverage {
        last_run_ms: scip_meta.last_run_ms,
        lag: Some(header.scip),
        auto: self.auto_scip_status(tx)?,
        ..ScipCoverage::default()
      };
      for repo in &project.repos {
        for record in tx.files_in_repo(&repo.name)? {
          let owner = Owner::File {
            repo: record.repo.clone(),
            path: record.path.clone(),
          };
          let facts = tx.owned_facts(&owner)?;
          tally.add(&facts);
          routes.add(&format!("{}:{}", record.repo, record.path), &facts);
          let Some(language) = record.language else { continue };
          *files_by_language.entry(language.as_str().to_owned()).or_default() += 1;
          *files_by_parse_status.entry(parse_label(&record.parse_status).to_owned()).or_default() += 1;
          if let ParseStatus::Failed { reason } = &record.parse_status {
            failed_files.push(FailedFile {
              repo: record.repo.clone(),
              path: Some(record.path.clone()),
              reason: reason.clone(),
            });
          }
          scip.files_total += 1;
          let scip_facts = tx.owned_facts(&Owner::Project {
            pass: ScipFacts::owner_pass(&record.repo, &record.path),
          })?;
          if !scip_facts.is_empty() {
            scip.files_with_scip += 1;
            tally.add(&scip_facts);
          }
        }
      }
      for pass in &passes {
        tally.add(&tx.owned_facts(&Owner::Project { pass: pass.clone() })?);
      }
      let mut inferred = InferredCounts::default();
      for fact in tx.inferred_facts(&InferredQuery {
        subject: None,
        relation: None,
        freshness: FreshnessPolicy::IncludeStale,
      })? {
        match fact.freshness {
          Freshness::Current => inferred.current += 1,
          Freshness::Stale { .. } => inferred.stale += 1,
          Freshness::NeedsRevalidation { .. } => inferred.needs_revalidation += 1,
        }
      }
      let mut llm = LlmSpend::default();
      for run in tx.runs(SPEND_RUNS)? {
        for call in tx.llm_calls(run.id)? {
          llm.calls += 1;
          llm.input_tokens += call.call.usage.input;
          llm.output_tokens += call.call.usage.output;
          llm.cost_estimate_usd += call.call.cost_estimate_usd.unwrap_or(0.0);
        }
      }
      Ok(IndexStatus {
        runs: tx.runs(STATUS_RUNS)?,
        files_by_language,
        files_by_parse_status,
        failed_files,
        unresolved_by_reason: tally.unresolved_by_reason,
        edge_certainty: tally.edge_certainty,
        scip,
        inferred,
        llm,
        dangling_detector_edges_dropped: dangling,
        repos_missing: missing,
        skipped,
        budget_retries,
        route_families: self.route_family_status(&routes),
      })
    })
  }
}
