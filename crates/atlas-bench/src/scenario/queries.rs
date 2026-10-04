use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use atlas_query::{ApiRequest, ContextRequest, ImpactRequest, QueryEngine, QueryResult, SearchRequest, SimilarRequest, SymbolRequest, Targeted, TraverseRequest, UnusedRequest};

use super::workload::Target;
use crate::error::BenchResult;
use crate::report::ScenarioRecord;
use crate::stats::{MIN_PERCENTILE_SAMPLES, Summary, millis};

const IMPACT_DEPTH: u32 = 3;
const CONTEXT_BUDGET: usize = 6_000;
const HEAVY_SAMPLES: usize = 3;
const HEAVY_TIMEOUT: Duration = Duration::from_secs(60);
const KIND_BUDGET_MS: f64 = 30_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
  Symbol,
  Callers,
  Impact,
  Search,
  Similar,
  Context,
  Api,
  Unused,
}

impl QueryKind {
  pub const ALL: [QueryKind; 8] = [
    Self::Symbol,
    Self::Callers,
    Self::Impact,
    Self::Search,
    Self::Similar,
    Self::Context,
    Self::Api,
    Self::Unused,
  ];

  pub fn name(self) -> &'static str {
    match self {
      Self::Symbol => "symbol",
      Self::Callers => "callers",
      Self::Impact => "impact",
      Self::Search => "search",
      Self::Similar => "similar",
      Self::Context => "context",
      Self::Api => "api",
      Self::Unused => "unused",
    }
  }

  fn is_heavy(self) -> bool {
    matches!(self, Self::Api | Self::Unused)
  }

  fn sample_limit(self, requested: usize) -> usize {
    match self.is_heavy() {
      true => requested.min(HEAVY_SAMPLES),
      false => requested,
    }
  }

  pub fn run(self, query: &QueryEngine, target: &Target) -> QueryResult<bool> {
    match self {
      Self::Symbol => Ok(is_found(&query.symbol(&SymbolRequest::new(&target.key))?.answer)),
      Self::Callers => Ok(is_found(&query.callers(&TraverseRequest::new(&target.key))?.answer)),
      Self::Impact => {
        let mut request = ImpactRequest::new([&target.key]);
        request.depth = IMPACT_DEPTH;
        Ok(is_found(&query.impact(&request)?.answer))
      }
      Self::Search => {
        query.search(&SearchRequest::new(&target.name))?;
        Ok(true)
      }
      Self::Similar => Ok(is_found(&query.similar(&SimilarRequest::new(&target.key))?.answer)),
      Self::Context => {
        let mut request = ContextRequest::new(format!("change {}", target.name));
        request.budget = CONTEXT_BUDGET;
        request.seeds = vec![target.key.clone()];
        query.context(&request)?;
        Ok(true)
      }
      Self::Api => {
        query.api(&ApiRequest {
          repo: Some(target.repo.clone()),
          ..ApiRequest::default()
        })?;
        Ok(true)
      }
      Self::Unused => {
        query.unused(&UnusedRequest {
          repo: Some(target.repo.clone()),
          ..UnusedRequest::default()
        })?;
        Ok(true)
      }
    }
  }
}

fn is_found<T>(outcome: &Targeted<T>) -> bool {
  matches!(outcome, Targeted::Found(_))
}

enum Timed {
  Done(f64, QueryResult<bool>),
  TimedOut,
}

fn run_timed(kind: QueryKind, query: &Arc<QueryEngine>, target: &Target) -> Timed {
  if !kind.is_heavy() {
    let started = Instant::now();
    let outcome = kind.run(query, target);
    return Timed::Done(millis(started.elapsed()), outcome);
  }
  let (sender, receiver) = mpsc::channel();
  let (query, target) = (Arc::clone(query), target.clone());
  let started = Instant::now();
  thread::spawn(move || {
    let outcome = kind.run(&query, &target);
    sender.send((millis(started.elapsed()), outcome)).ok();
  });
  match receiver.recv_timeout(HEAVY_TIMEOUT) {
    Ok((elapsed, outcome)) => Timed::Done(elapsed, outcome),
    Err(_) => Timed::TimedOut,
  }
}

struct Measured {
  timings: Vec<f64>,
  found: u64,
  errors: u64,
  timed_out: bool,
}

fn measure(kind: QueryKind, query: &Arc<QueryEngine>, chosen: &[Target]) -> Measured {
  if !kind.is_heavy() {
    kind.run(query, &chosen[0]).ok();
  }
  let mut measured = Measured {
    timings: Vec::new(),
    found: 0,
    errors: 0,
    timed_out: false,
  };
  for target in chosen {
    match run_timed(kind, query, target) {
      Timed::TimedOut => {
        measured.timed_out = true;
        break;
      }
      Timed::Done(elapsed, outcome) => {
        measured.timings.push(elapsed);
        match outcome {
          Ok(true) => measured.found += 1,
          Ok(false) => {}
          Err(_) => measured.errors += 1,
        }
      }
    }
    if measured.timings.iter().sum::<f64>() > KIND_BUDGET_MS {
      break;
    }
  }
  measured
}

fn record_measured(mut record: ScenarioRecord, prefix: &str, measured: &Measured) -> ScenarioRecord {
  record = record
    .info(&format!("{prefix}_samples"), measured.timings.len())
    .info(&format!("{prefix}_found"), measured.found)
    .info(&format!("{prefix}_errors"), measured.errors);
  if measured.timed_out {
    record = record.info(&format!("{prefix}_timed_out_after_s"), HEAVY_TIMEOUT.as_secs());
  }
  if measured.timings.is_empty() {
    return record;
  }
  let summary = Summary::of(&measured.timings);
  record = record.metric(&format!("{prefix}_p50_ms"), summary.p50).metric(&format!("{prefix}_max_ms"), summary.max);
  if summary.count >= MIN_PERCENTILE_SAMPLES {
    record = record.metric(&format!("{prefix}_p95_ms"), summary.p95);
  }
  record
}

pub fn latency(corpus: &str, barriered: &Arc<QueryEngine>, unbarriered: &Arc<QueryEngine>, targets: &[Target], samples: usize) -> BenchResult<ScenarioRecord> {
  let mut record = ScenarioRecord::ok(corpus, "query_latency").info("nobarrier", "the same queries with the freshness barrier skipped");
  for kind in QueryKind::ALL {
    let chosen = &targets[..targets.len().min(kind.sample_limit(samples))];
    let name = kind.name();
    record = record_measured(record, name, &measure(kind, barriered, chosen));
    record = record_measured(record, &format!("{name}_nobarrier"), &measure(kind, unbarriered, chosen));
  }
  Ok(record)
}
