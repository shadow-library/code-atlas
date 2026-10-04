use std::time::Instant;

use atlas_engine::{IndexMode, RunOutcomeKind};

use super::{WorkerSpec, db_bytes, load_project, open_engine, parse_counts, similarity_blob_bytes};
use crate::error::{BenchError, BenchResult};
use crate::report::ScenarioRecord;
use crate::stats::millis;

const SCENARIO: &str = "cold_index";

pub fn run(spec: &WorkerSpec) -> BenchResult<Vec<ScenarioRecord>> {
  let project = load_project(&spec.projects[0])?;
  let (engine, open_ms) = open_engine(&project, &spec.home)?;
  let summary = engine.index(IndexMode::Full)?;
  let index_ms = millis(summary.duration);
  let persisted = Instant::now();
  engine.persist_similarity()?;
  let persist_ms = millis(persisted.elapsed());
  let parse = parse_counts(&engine, &project)?;
  let outcome = match &summary.outcome {
    RunOutcomeKind::Ok => "ok".to_owned(),
    RunOutcomeKind::Partial { reason } => format!("partial: {reason}"),
  };
  if spec.synthetic && parse.unclean() > 0 {
    return Err(BenchError::ParseFailures {
      count: parse.unclean(),
      sample: parse.samples.join("; "),
    });
  }
  if spec.synthetic && outcome != "ok" {
    return Err(BenchError::Worker {
      mode: "cold".to_owned(),
      detail: outcome,
    });
  }
  let files = summary.counts.get("files_analyzed").copied().unwrap_or(0);
  let seconds = (index_ms / 1000.0).max(f64::EPSILON);
  let mut record = ScenarioRecord::ok(&spec.corpus, SCENARIO)
    .metric("open_ms", open_ms)
    .metric("index_ms", index_ms)
    .metric("persist_similarity_ms", persist_ms)
    .metric("files_per_s", files as f64 / seconds)
    .metric("loc_per_s", spec.loc as f64 / seconds)
    .metric("db_bytes", db_bytes(&engine) as f64)
    .metric("similarity_blob_bytes", similarity_blob_bytes(&engine)? as f64);
  for (phase, ms) in &summary.phase_timings_ms {
    record = record.metric(&format!("phase_{phase}_ms"), *ms as f64);
  }
  Ok(vec![
    record
      .info("files_analyzed", files)
      .info("loc", spec.loc)
      .info("run_outcome", outcome)
      .info("counts", &summary.counts)
      .info("parse_ok", parse.ok)
      .info("parse_partial", parse.partial)
      .info("parse_failed", parse.failed)
      .info("parse_samples", &parse.samples),
  ])
}
