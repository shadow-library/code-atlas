use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use atlas_query::QueryEngine;

use super::queries::QueryKind;
use super::workload::{Editable, Target};
use crate::error::{BenchError, BenchResult};
use crate::report::ScenarioRecord;
use crate::source::{self, bump_numbers};
use crate::stats::{Summary, median, millis, range};

const MIX: [QueryKind; 17] = [
  QueryKind::Symbol,
  QueryKind::Symbol,
  QueryKind::Symbol,
  QueryKind::Symbol,
  QueryKind::Callers,
  QueryKind::Callers,
  QueryKind::Callers,
  QueryKind::Callers,
  QueryKind::Search,
  QueryKind::Search,
  QueryKind::Search,
  QueryKind::Impact,
  QueryKind::Impact,
  QueryKind::Context,
  QueryKind::Context,
  QueryKind::Similar,
  QueryKind::Api,
];

struct LevelResult {
  latencies_ms: Vec<f64>,
  errors: u64,
  wall_s: f64,
  edits: u64,
}

fn worker(query: &QueryEngine, targets: &[Target], thread_index: usize, deadline: Instant) -> (Vec<f64>, u64) {
  let (mut latencies, mut errors) = (Vec::new(), 0);
  let mut iteration = 0usize;
  while Instant::now() < deadline {
    let kind = MIX[(iteration + thread_index) % MIX.len()];
    let target = &targets[(iteration * 7 + thread_index * 13) % targets.len()];
    let started = Instant::now();
    if kind.run(query, target).is_err() {
      errors += 1;
    }
    latencies.push(millis(started.elapsed()));
    iteration += 1;
  }
  (latencies, errors)
}

fn edit_stream(files: &[Editable], interval: Duration, stop: &AtomicBool) -> BenchResult<u64> {
  let mut edits = 0u64;
  while !stop.load(Ordering::Acquire) {
    let file = &files[edits as usize % files.len()];
    let text = source::read(&file.abs)?;
    if let Some(bumped) = bump_numbers(&text, 1) {
      source::write(&file.abs, &bumped)?;
      edits += 1;
    }
    thread::sleep(interval);
  }
  Ok(edits)
}

fn run_level(query: &QueryEngine, targets: &[Target], threads: usize, duration: Duration, edits: Option<(&[Editable], Duration)>) -> BenchResult<LevelResult> {
  let stop = AtomicBool::new(false);
  let started = Instant::now();
  let deadline = started + duration;
  thread::scope(|scope| {
    let editor = edits.map(|(files, interval)| {
      let stop = &stop;
      scope.spawn(move || edit_stream(files, interval, stop))
    });
    let workers: Vec<_> = (0..threads).map(|index| scope.spawn(move || worker(query, targets, index, deadline))).collect();
    let mut latencies_ms = Vec::new();
    let mut errors = 0;
    let mut panicked = false;
    for handle in workers {
      match handle.join() {
        Ok((latencies, failed)) => {
          latencies_ms.extend(latencies);
          errors += failed;
        }
        Err(_) => panicked = true,
      }
    }
    let wall_s = started.elapsed().as_secs_f64();
    stop.store(true, Ordering::Release);
    let edit_count = match editor {
      Some(handle) => handle.join().map_err(|_| BenchError::Worker {
        mode: "concurrency".to_owned(),
        detail: "edit stream panicked".to_owned(),
      })??,
      None => 0,
    };
    if panicked {
      return Err(BenchError::Worker {
        mode: "concurrency".to_owned(),
        detail: "a query thread panicked".to_owned(),
      });
    }
    Ok(LevelResult {
      latencies_ms,
      errors,
      wall_s,
      edits: edit_count,
    })
  })
}

#[allow(clippy::too_many_arguments)]
pub fn run(
  corpus: &str,
  scenario: &str,
  query: &QueryEngine,
  targets: &[Target],
  levels: &[usize],
  seconds: f64,
  repeats: usize,
  edits: Option<(&[Editable], Duration)>,
) -> BenchResult<ScenarioRecord> {
  let mut record = ScenarioRecord::ok(corpus, scenario).info("repeats", repeats).info("window_s", seconds);
  let mut spread = serde_json::Map::new();
  for threads in levels {
    let (mut qps, mut p50, mut p95) = (Vec::new(), Vec::new(), Vec::new());
    let (mut queries, mut errors, mut applied) = (0usize, 0u64, 0u64);
    for _ in 0..repeats {
      let result = run_level(query, targets, *threads, Duration::from_secs_f64(seconds), edits)?;
      let summary = Summary::of(&result.latencies_ms);
      qps.push(result.latencies_ms.len() as f64 / result.wall_s);
      p50.push(summary.p50);
      p95.push(summary.p95);
      queries += result.latencies_ms.len();
      errors += result.errors;
      applied += result.edits;
    }
    for (name, samples) in [("throughput_qps", &qps), ("p50_ms", &p50), ("p95_ms", &p95)] {
      let key = format!("n{threads}_{name}");
      let (low, high) = range(samples);
      spread.insert(key.clone(), serde_json::json!([low, high]));
      record = record.metric(&key, median(samples));
    }
    record = record
      .info(&format!("n{threads}_queries"), queries)
      .info(&format!("n{threads}_errors"), errors)
      .info(&format!("n{threads}_edits_applied"), applied);
  }
  Ok(record.info("spread", spread))
}
