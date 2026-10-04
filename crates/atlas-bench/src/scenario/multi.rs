use std::collections::BTreeSet;
use std::time::Instant;

use atlas_core::EntityKind;
use atlas_engine::{Engine, IndexMode};
use atlas_project::Project;
use atlas_store::EntityFilter;

use super::{WorkerSpec, db_bytes, load_project, open_engine, parse_counts};
use crate::error::{BenchError, BenchResult};
use crate::report::ScenarioRecord;
use crate::stats::millis;

const SCENARIO: &str = "multi_project";

struct Footprint {
  files: u64,
  foreign_files: u64,
  foreign_entities: u64,
}

fn footprint(engine: &Engine, project: &Project, others: &[Project]) -> BenchResult<Footprint> {
  let own: BTreeSet<&str> = project.repos.iter().map(|repo| repo.name.as_str()).collect();
  engine.store().read(|tx| -> BenchResult<Footprint> {
    let mut files = 0;
    for repo in &project.repos {
      files += tx.files_in_repo(&repo.name)?.len() as u64;
    }
    let mut foreign_files = 0;
    for repo in others.iter().flat_map(|other| other.repos.iter()) {
      foreign_files += tx.files_in_repo(&repo.name)?.len() as u64;
    }
    let foreign_entities = tx
      .entities_by_kind(&EntityKind::File, &EntityFilter::default())?
      .iter()
      .filter(|entity| entity.repo.as_ref().is_some_and(|repo| !own.contains(repo.as_str())))
      .count() as u64;
    Ok(Footprint {
      files,
      foreign_files,
      foreign_entities,
    })
  })
}

pub fn run(spec: &WorkerSpec) -> BenchResult<Vec<ScenarioRecord>> {
  let projects: Vec<Project> = spec.projects.iter().map(|path| load_project(path)).collect::<BenchResult<_>>()?;
  let started = Instant::now();
  let mut record = ScenarioRecord::ok(&spec.corpus, SCENARIO);
  let mut db_paths = BTreeSet::new();
  for (index, project) in projects.iter().enumerate() {
    let (engine, _) = open_engine(project, &spec.home)?;
    let indexed = Instant::now();
    engine.index(IndexMode::Full)?;
    record = record.metric(&format!("project_{index}_index_ms"), millis(indexed.elapsed()));
    let parse = parse_counts(&engine, project)?;
    if parse.unclean() > 0 {
      return Err(BenchError::ParseFailures {
        count: parse.unclean(),
        sample: parse.samples.join("; "),
      });
    }
    db_paths.insert(engine.db_path());
    record = record.metric(&format!("project_{index}_db_bytes"), db_bytes(&engine) as f64);
  }
  let total_ms = millis(started.elapsed());
  let mut violations = 0u64;
  let mut files = Vec::new();
  for (index, project) in projects.iter().enumerate() {
    let others: Vec<Project> = projects.iter().enumerate().filter(|(other, _)| *other != index).map(|(_, other)| other.clone()).collect();
    let (engine, _) = open_engine(project, &spec.home)?;
    let found = footprint(&engine, project, &others)?;
    violations += found.foreign_files + found.foreign_entities;
    files.push(found.files);
  }
  if db_paths.len() != projects.len() {
    violations += 1;
  }
  Ok(vec![
    record
      .metric("total_ms", total_ms)
      .info("projects", projects.len())
      .info("files_per_project", files)
      .info("cross_talk_violations", violations)
      .info("distinct_databases", db_paths.len()),
  ])
}
