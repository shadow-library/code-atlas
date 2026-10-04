mod cold;
mod concurrency;
mod incremental;
mod multi;
mod queries;
mod session;
mod workload;

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use atlas_core::ParseStatus;
use atlas_engine::{Engine, EngineOptions};
use atlas_project::{AtlasHome, Project};
use serde::{Deserialize, Serialize};

use crate::error::{BenchError, BenchResult};
use crate::report::ScenarioRecord;
use crate::source;
use crate::stats::millis;

const SIMILARITY_BLOB: &str = "similarity";
const MAX_PARSE_SAMPLES: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerMode {
  Cold,
  Session,
  Multi,
}

impl WorkerMode {
  pub fn name(self) -> &'static str {
    match self {
      Self::Cold => "cold",
      Self::Session => "session",
      Self::Multi => "multi",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
  pub barrier_iterations: usize,
  pub query_samples: usize,
  pub concurrency_seconds: f64,
  pub repeats: usize,
  pub concurrency_levels: Vec<usize>,
  pub edit_counts: Vec<usize>,
  pub edit_interval_ms: u64,
  #[serde(default)]
  pub scenarios: Vec<String>,
}

impl Settings {
  pub fn includes(&self, scenario: &str) -> bool {
    self.scenarios.is_empty() || self.scenarios.iter().any(|selected| selected == scenario)
  }

  pub fn for_suite() -> Self {
    Self {
      barrier_iterations: 50,
      query_samples: 30,
      concurrency_seconds: 5.0,
      repeats: 3,
      concurrency_levels: vec![1, 4, 8, 16],
      edit_counts: vec![1, 10, 100, 1000],
      edit_interval_ms: 100,
      scenarios: Vec::new(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerSpec {
  pub mode: WorkerMode,
  pub corpus: String,
  pub projects: Vec<PathBuf>,
  pub home: PathBuf,
  pub loc: u64,
  pub synthetic: bool,
  pub settings: Settings,
}

pub fn run_worker(spec: &WorkerSpec) -> BenchResult<Vec<ScenarioRecord>> {
  match spec.mode {
    WorkerMode::Cold => cold::run(spec),
    WorkerMode::Session => session::run(spec),
    WorkerMode::Multi => multi::run(spec),
  }
}

pub fn load_project(path: &Path) -> BenchResult<Project> {
  let text = source::read(path)?;
  serde_json::from_str(&text).map_err(|error| BenchError::json(path.display().to_string(), error))
}

pub fn open_engine(project: &Project, home: &Path) -> BenchResult<(Engine, f64)> {
  let started = Instant::now();
  let engine = Engine::open(project, &AtlasHome::rooted_at(home), EngineOptions::default())?;
  Ok((engine, millis(started.elapsed())))
}

pub fn server_readers() -> usize {
  let default = EngineOptions::default().store.readers;
  std::thread::available_parallelism().map_or(default, NonZeroUsize::get).max(default)
}

pub fn open_server(project: &Project, home: &Path) -> BenchResult<(Arc<Engine>, f64)> {
  let mut options = EngineOptions::default();
  options.store.readers = server_readers();
  let started = Instant::now();
  let engine = Engine::open(project, &AtlasHome::rooted_at(home), options)?;
  Ok((Arc::new(engine), millis(started.elapsed())))
}

pub fn db_bytes(engine: &Engine) -> u64 {
  let db = engine.db_path();
  ["", "-wal", "-shm"]
    .iter()
    .filter_map(|suffix| {
      let mut path = db.clone().into_os_string();
      path.push(suffix);
      std::fs::metadata(path).ok().map(|metadata| metadata.len())
    })
    .sum()
}

pub fn similarity_blob_bytes(engine: &Engine) -> BenchResult<u64> {
  let blob = engine.store().read(|tx| tx.blob(SIMILARITY_BLOB))?;
  Ok(blob.map(|blob| blob.bytes.len() as u64).unwrap_or(0))
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParseCounts {
  pub ok: u64,
  pub partial: u64,
  pub failed: u64,
  pub samples: Vec<String>,
}

impl ParseCounts {
  pub fn unclean(&self) -> u64 {
    self.partial + self.failed
  }
}

pub fn parse_counts(engine: &Engine, project: &Project) -> BenchResult<ParseCounts> {
  engine.store().read(|tx| -> BenchResult<ParseCounts> {
    let mut counts = ParseCounts::default();
    for repo in &project.repos {
      for record in tx.files_in_repo(&repo.name)? {
        if record.language.is_none() {
          continue;
        }
        let label = match &record.parse_status {
          ParseStatus::Ok => {
            counts.ok += 1;
            continue;
          }
          ParseStatus::Partial { .. } => {
            counts.partial += 1;
            "partial"
          }
          ParseStatus::Failed { .. } => {
            counts.failed += 1;
            "failed"
          }
        };
        if counts.samples.len() < MAX_PARSE_SAMPLES {
          counts.samples.push(format!("{label}: {}/{}", record.repo, record.path));
        }
      }
    }
    Ok(counts)
  })
}

pub fn guard(corpus: &str, scenario: &str, run: impl FnOnce() -> BenchResult<ScenarioRecord>) -> ScenarioRecord {
  run().unwrap_or_else(|error| ScenarioRecord::failed(corpus, scenario, error.to_string()))
}

pub fn guard_many(corpus: &str, scenario: &str, run: impl FnOnce() -> BenchResult<Vec<ScenarioRecord>>) -> Vec<ScenarioRecord> {
  run().unwrap_or_else(|error| vec![ScenarioRecord::failed(corpus, scenario, error.to_string())])
}
