use std::sync::Arc;
use std::time::{Duration, Instant};

use atlas_engine::Engine;
use atlas_project::Project;
use atlas_query::QueryEngine;

use super::workload::Workload;
use super::{WorkerSpec, concurrency, guard, guard_many, incremental, load_project, open_server, queries, server_readers};
use crate::error::BenchResult;
use crate::report::ScenarioRecord;
use crate::source::list_sources;
use crate::stats::{Summary, millis};

fn warm_startup(corpus: &str, engine: &Engine, open_ms: f64) -> BenchResult<ScenarioRecord> {
  let started = Instant::now();
  let report = engine.ensure_fresh()?;
  Ok(
    ScenarioRecord::ok(corpus, "warm_startup")
      .metric("open_ms", open_ms)
      .info("readers", server_readers())
      .metric("first_barrier_ms", millis(started.elapsed()))
      .info("first_barrier_ran_index", report.header().run.is_some())
      .info("first_barrier_fresh", report.is_fresh()),
  )
}

fn no_change_barrier(corpus: &str, engine: &Engine, iterations: usize) -> BenchResult<ScenarioRecord> {
  let mut timings = Vec::with_capacity(iterations);
  let mut runs = 0u64;
  for _ in 0..iterations {
    let started = Instant::now();
    let report = engine.ensure_fresh()?;
    timings.push(millis(started.elapsed()));
    runs += u64::from(report.header().run.is_some());
  }
  let summary = Summary::of(&timings);
  Ok(
    ScenarioRecord::ok(corpus, "no_change_barrier")
      .metric("barrier_p50_ms", summary.p50)
      .metric("barrier_p95_ms", summary.p95)
      .metric("barrier_max_ms", summary.max)
      .info("iterations", summary.count)
      .info("unexpected_index_runs", runs),
  )
}

fn workload_scenarios(spec: &WorkerSpec, project: &Project, engine: &Arc<Engine>, workload: &Workload) -> Vec<ScenarioRecord> {
  let corpus = spec.corpus.as_str();
  let settings = &spec.settings;
  let query = Arc::new(QueryEngine::new(engine.clone()));
  let mut records = Vec::new();
  let mut scenario = |name: &str, run: &mut dyn FnMut() -> Vec<ScenarioRecord>| {
    if settings.includes(name) {
      records.extend(run());
    }
  };
  scenario("query_latency", &mut || {
    let unbarriered = Arc::new(QueryEngine::without_barrier(engine.clone()));
    vec![guard(corpus, "query_latency", || {
      queries::latency(corpus, &query, &unbarriered, &workload.targets, settings.query_samples)
    })]
  });
  scenario("concurrency", &mut || {
    vec![guard(corpus, "concurrency", || {
      concurrency::run(
        corpus,
        "concurrency",
        &query,
        &workload.targets,
        &settings.concurrency_levels,
        settings.concurrency_seconds,
        settings.repeats,
        None,
      )
    })]
  });
  scenario("incremental", &mut || {
    guard_many(corpus, "incremental", || {
      Ok(incremental::body_edits(corpus, engine, &workload.editable, &settings.edit_counts))
    })
  });
  scenario("rename_symbol", &mut || {
    vec![guard(corpus, "rename_symbol", || incremental::rename(corpus, engine, project, workload.rename.as_ref()))]
  });
  scenario("file_move", &mut || vec![guard(corpus, "file_move", || incremental::file_move(corpus, engine, project))]);
  scenario("branch_switch", &mut || {
    vec![guard(corpus, "branch_switch", || incremental::branch_switch(corpus, engine, &workload.editable))]
  });
  scenario("delete_directory", &mut || {
    vec![guard(corpus, "delete_directory", || incremental::delete_directory(corpus, engine, project))]
  });
  scenario("concurrency_with_edits", &mut || {
    vec![guard(corpus, "concurrency_with_edits", || {
      if workload.editable.is_empty() {
        return Ok(ScenarioRecord::skipped(corpus, "concurrency_with_edits", "no editable files"));
      }
      let interval = Duration::from_millis(settings.edit_interval_ms);
      let pool: Vec<_> = workload.editable.iter().filter(|file| file.abs.exists()).take(20).cloned().collect();
      if pool.is_empty() {
        return Ok(ScenarioRecord::skipped(corpus, "concurrency_with_edits", "no editable files remain"));
      }
      engine.ensure_fresh()?;
      concurrency::run(
        corpus,
        "concurrency_with_edits",
        &query,
        &workload.targets,
        &settings.concurrency_levels,
        settings.concurrency_seconds,
        settings.repeats,
        Some((&pool, interval)),
      )
    })]
  });
  records
}

pub fn run(spec: &WorkerSpec) -> BenchResult<Vec<ScenarioRecord>> {
  let corpus = spec.corpus.as_str();
  let project = load_project(&spec.projects[0])?;
  let (engine, open_ms) = open_server(&project, &spec.home)?;
  let mut records = vec![guard(corpus, "warm_startup", || warm_startup(corpus, &engine, open_ms))];
  if spec.settings.includes("no_change_barrier") {
    records.push(guard(corpus, "no_change_barrier", || no_change_barrier(corpus, &engine, spec.settings.barrier_iterations)));
  }
  let prepared = list_sources(&project).and_then(|sources| Workload::prepare(&engine, &sources, spec.settings.query_samples));
  match prepared {
    Ok(Some(workload)) => {
      records.extend(workload_scenarios(spec, &project, &engine, &workload));
      records.push(
        ScenarioRecord::ok(corpus, "workload")
          .info("targets", workload.targets.len())
          .info("editable_files", workload.editable.len()),
      );
    }
    Ok(None) => records.push(ScenarioRecord::failed(corpus, "workload", "the index holds no function entities")),
    Err(error) => records.push(ScenarioRecord::failed(corpus, "workload", error.to_string())),
  }
  Ok(records)
}
