use atlas_engine::{ScipIndexers, ScipPackageOutcome};
use atlas_scip::{RustAnalyzerScip, ScipTypescript};

use crate::context::Context;
use crate::error::CliResult;
use crate::output::{Outcome, emit_line};

pub fn run(ctx: &Context) -> CliResult<Outcome> {
  let project = ctx.resolve()?;
  let engine = ctx.open_engine(&project)?;
  let freshness = engine.ensure_fresh()?;
  let cargo = RustAnalyzerScip::new();
  let npm = ScipTypescript::new();
  let report = engine.scip(&ScipIndexers {
    cargo: Some(&cargo),
    npm: Some(&npm),
  })?;
  emit_line(format!("scip run {}: {} packages", report.run, report.packages.len()))?;
  if !project.settings.scip.enabled {
    emit_line(format!(
      "automatic SCIP is off for this project; enable it with `catl project config {} --scip on`",
      project.name
    ))?;
  }
  let mut degraded = !freshness.is_fresh();
  for package in &report.packages {
    let indexer = package.indexer.as_deref().unwrap_or("none");
    let detail = match &package.outcome {
      ScipPackageOutcome::Ingested {
        files, edges, skipped_changed, ..
      } => {
        format!("ingested {files} files, {edges} edges ({skipped_changed} skipped because they changed)")
      }
      ScipPackageOutcome::NoIndexer => "no indexer for this ecosystem".to_owned(),
      ScipPackageOutcome::IndexerUnavailable { reason } => {
        degraded = true;
        format!("indexer unavailable: {reason}")
      }
      ScipPackageOutcome::IndexerFailed { reason } => {
        degraded = true;
        format!("indexer failed: {reason}")
      }
    };
    emit_line(format!("  {}:{} [{indexer}] {detail}", package.repo, package.manifest))?;
  }
  Ok(Outcome::degraded_if(degraded))
}
