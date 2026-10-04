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

pub fn latency(corpus: &str, query: &Arc<QueryEngine>, targets: &[Target], samples: usize) -> BenchResult<ScenarioRecord> {
  let mut record = ScenarioRecord::ok(corpus, "query_latency");
  for kind in QueryKind::ALL {
    let chosen = &targets[..targets.len().min(kind.sample_limit(samples))];
    if !kind.is_heavy() {
      kind.run(query, &chosen[0]).ok();
    }
    let (mut timings, mut found, mut errors, mut timed_out) = (Vec::new(), 0u64, 0u64, false);
    for target in chosen {
      match run_timed(kind, query, target) {
        Timed::TimedOut => {
          timed_out = true;
          break;
        }
        Timed::Done(elapsed, outcome) => {
          timings.push(elapsed);
          match outcome {
            Ok(true) => found += 1,
            Ok(false) => {}
            Err(_) => errors += 1,
          }
        }
      }
      if timings.iter().sum::<f64>() > KIND_BUDGET_MS {
        break;
      }
    }
    let name = kind.name();
    record = record
      .info(&format!("{name}_samples"), timings.len())
      .info(&format!("{name}_found"), found)
      .info(&format!("{name}_errors"), errors);
    if timed_out {
      record = record.info(&format!("{name}_timed_out_after_s"), HEAVY_TIMEOUT.as_secs());
    }
    if timings.is_empty() {
      continue;
    }
    let summary = Summary::of(&timings);
    record = record.metric(&format!("{name}_p50_ms"), summary.p50).metric(&format!("{name}_max_ms"), summary.max);
    if summary.count >= MIN_PERCENTILE_SAMPLES {
      record = record.metric(&format!("{name}_p95_ms"), summary.p95);
    }
  }
  Ok(record)
}
