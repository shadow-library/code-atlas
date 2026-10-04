mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use atlas_core::{ProjectName, RepoName};
use atlas_engine::Engine;
use atlas_project::{AtlasHome, Project, ProjectSettings, RepoConfig};
use atlas_query::{ContextRequest, ImpactRequest, QueryEngine, SymbolRequest, Targeted, TraverseRequest};

const SAMPLES: usize = 20;
const TARGETS: [&str; 5] = ["Graph::walk", "ReadTx::traverse", "QueryEngine::impact", "Engine::ensure_fresh", "SimilarityIndex::search"];

fn p95(mut samples: Vec<Duration>) -> Duration {
  samples.sort();
  samples[(samples.len() * 95 / 100).min(samples.len() - 1)]
}

fn measure(name: &str, barrier: Duration, mut run: impl FnMut(usize)) -> Duration {
  let samples: Vec<Duration> = (0..SAMPLES)
    .map(|index| {
      let started = Instant::now();
      run(index);
      started.elapsed()
    })
    .collect();
  let total = p95(samples);
  let excluding = total.saturating_sub(barrier);
  println!("{name}: p95 {total:?} including the barrier, ~{excluding:?} excluding it");
  excluding
}

#[test]
#[ignore = "performance probe over this workspace; run with --release -- --ignored --nocapture"]
fn should_meet_query_latency_targets_on_this_repository() {
  let scratch = tempfile::tempdir().unwrap();
  let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
  let project = Project {
    name: ProjectName::new("atlas").unwrap(),
    settings: ProjectSettings::default(),
    repos: vec![RepoConfig {
      name: RepoName::new("code-atlas").unwrap(),
      path: root,
      include: vec!["crates/**".to_owned()],
      exclude: Vec::new(),
      languages: None,
    }],
  };
  let engine = Arc::new(Engine::open(&project, &AtlasHome::rooted_at(scratch.path()), common::engine_options()).unwrap());
  engine.ensure_fresh().unwrap();
  let query = QueryEngine::new(Arc::clone(&engine));
  for target in TARGETS {
    assert!(
      matches!(query.symbol(&SymbolRequest::new(target)).unwrap().answer, Targeted::Found(_)),
      "{target} must resolve"
    );
  }
  let files = engine.status().unwrap().files_by_language.values().sum::<u64>();
  let barrier = p95(
    (0..SAMPLES)
      .map(|_| {
        let started = Instant::now();
        engine.ensure_fresh().unwrap();
        started.elapsed()
      })
      .collect(),
  );

  let symbol = measure("symbol", barrier, |index| {
    query.symbol(&SymbolRequest::new(TARGETS[index % TARGETS.len()])).unwrap();
  });
  let callers = measure("callers", barrier, |index| {
    query.callers(&TraverseRequest::new(TARGETS[index % TARGETS.len()])).unwrap();
  });
  let impact = measure("impact depth 3", barrier, |index| {
    query.impact(&ImpactRequest::new([TARGETS[index % TARGETS.len()]])).unwrap();
  });
  let context = measure("context 6k", barrier, |_| {
    query
      .context(&ContextRequest::new("add a traversal cap to the impact query in the store graph walker"))
      .unwrap();
  });

  println!("{files} files indexed; clean barrier p95 {barrier:?}");
  assert!(symbol < Duration::from_millis(20), "symbol p95 {symbol:?}");
  assert!(callers < Duration::from_millis(20), "callers p95 {callers:?}");
  assert!(impact < Duration::from_millis(100), "impact p95 {impact:?}");
  assert!(context < Duration::from_millis(300), "context p95 {context:?}");
}
