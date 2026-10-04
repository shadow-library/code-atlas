use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use atlas_engine::Engine;
use atlas_project::Project;

use super::workload::{Editable, RenameCandidate};
use crate::error::{BenchError, BenchResult};
use crate::report::ScenarioRecord;
use crate::source::{self, SourceFile, appended_function, bump_numbers, list_sources, replace_word};
use crate::stats::millis;

const BRANCH_SWITCH_PERCENT: usize = 30;
const MIN_MOVE_LINES: usize = 20;
const MAX_DELETE_SHARE_PERCENT: usize = 10;
const MIN_DELETE_FILES: usize = 3;
const IMMOBILE_NAMES: [&str; 6] = ["mod.rs", "lib.rs", "main.rs", "index.ts", "index.js", "server.ts"];

struct Update {
  latency_ms: f64,
  ran: bool,
  degraded: bool,
  files_updated: u64,
  phases: std::collections::BTreeMap<String, u64>,
  counts: std::collections::BTreeMap<String, u64>,
}

fn timed_update(engine: &Engine) -> BenchResult<Update> {
  let started = Instant::now();
  let report = engine.ensure_fresh()?;
  let latency_ms = millis(started.elapsed());
  let header = report.header();
  let (ran, files_updated) = (header.run.is_some(), header.files_updated);
  let last = engine.store().read(|tx| tx.runs(1))?.into_iter().next();
  let (phases, counts) = match (&last, ran) {
    (Some(run), true) => (run.report.phase_timings_ms.clone(), run.report.counts.clone()),
    _ => Default::default(),
  };
  Ok(Update {
    latency_ms,
    ran,
    degraded: !report.is_fresh(),
    files_updated,
    phases,
    counts,
  })
}

fn count(update: &Update, keys: [&str; 3]) -> u64 {
  keys.iter().map(|key| update.counts.get(*key).copied().unwrap_or(0)).sum()
}

fn describe(corpus: &str, scenario: &str, update: &Update, touched: usize, expected_files: u64) -> ScenarioRecord {
  if !update.ran {
    return ScenarioRecord::failed(corpus, scenario, "the freshness barrier did not trigger an index run");
  }
  let entities = count(update, ["entities_added", "entities_removed", "entities_changed"]);
  let edges = count(update, ["edges_added", "edges_removed", "edges_changed"]);
  let changed_files = count(update, ["files_added", "files_modified", "files_deleted"]);
  ScenarioRecord::ok(corpus, scenario)
    .metric("latency_ms", update.latency_ms)
    .info("files_touched", touched)
    .info("files_updated", update.files_updated)
    .info("files_changed_in_run", changed_files)
    .info("files_mismatch", changed_files != expected_files)
    .info("entities_changed", entities)
    .info("edges_changed", edges)
    .info("facts_changed", entities + edges)
    .info("renames_detected", update.counts.get("renames_detected").copied().unwrap_or(0))
    .info("owners_resolved", update.counts.get("owners_resolved").copied().unwrap_or(0))
    .info("degraded", update.degraded)
    .info("phases_ms", &update.phases)
}

fn spread<T>(items: &[T], count: usize) -> impl Iterator<Item = &T> {
  (0..count).map(move |index| &items[index * items.len() / count])
}

pub fn body_edits(corpus: &str, engine: &Engine, editable: &[Editable], counts: &[usize]) -> Vec<ScenarioRecord> {
  counts
    .iter()
    .map(|wanted| {
      let scenario = format!("incremental_{wanted}_files");
      if editable.len() < *wanted {
        return ScenarioRecord::skipped(corpus, &scenario, format!("only {} editable files in this corpus", editable.len()));
      }
      let outcome = (|| -> BenchResult<ScenarioRecord> {
        engine.ensure_fresh()?;
        for file in spread(editable, *wanted) {
          let text = source::read(&file.abs)?;
          if let Some(bumped) = bump_numbers(&text, 1) {
            source::write(&file.abs, &bumped)?;
          }
        }
        Ok(describe(corpus, &scenario, &timed_update(engine)?, *wanted, *wanted as u64))
      })();
      outcome.unwrap_or_else(|error| ScenarioRecord::failed(corpus, &scenario, error.to_string()))
    })
    .collect()
}

pub fn rename(corpus: &str, engine: &Engine, project: &Project, candidate: Option<&RenameCandidate>) -> BenchResult<ScenarioRecord> {
  const SCENARIO: &str = "rename_symbol";
  let Some(candidate) = candidate else {
    return Ok(ScenarioRecord::skipped(corpus, SCENARIO, "no function name is referenced from two or more files"));
  };
  let renamed = if candidate.name.contains('_') {
    format!("{}_renamed", candidate.name)
  } else {
    format!("{}Renamed", candidate.name)
  };
  engine.ensure_fresh()?;
  let mut touched = 0;
  let mut replacements = 0;
  for file in list_sources(project)? {
    let text = source::read(&file.abs)?;
    let Some((updated, replaced)) = replace_word(&text, &candidate.name, &renamed) else {
      continue;
    };
    source::write(&file.abs, &updated)?;
    touched += 1;
    replacements += replaced;
  }
  let record = describe(corpus, SCENARIO, &timed_update(engine)?, touched, touched as u64);
  Ok(record.info("symbol", &candidate.name).info("replacements", replacements))
}

fn movable(files: &[SourceFile]) -> BenchResult<Option<&SourceFile>> {
  let start = files.len() / 2;
  for file in files[start..].iter().chain(files[..start].iter()) {
    let name = file.abs.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    if IMMOBILE_NAMES.contains(&name) {
      continue;
    }
    if source::read(&file.abs)?.lines().count() >= MIN_MOVE_LINES {
      return Ok(Some(file));
    }
  }
  Ok(None)
}

pub fn file_move(corpus: &str, engine: &Engine, project: &Project) -> BenchResult<ScenarioRecord> {
  const SCENARIO: &str = "file_move";
  let files = list_sources(project)?;
  let Some(file) = movable(&files)? else {
    return Ok(ScenarioRecord::skipped(corpus, SCENARIO, "no source file is large enough to move"));
  };
  engine.ensure_fresh()?;
  let parent = file.abs.parent().ok_or_else(|| BenchError::Invalid(format!("{} has no parent", file.abs.display())))?;
  let target_dir = parent.join("moved");
  fs::create_dir_all(&target_dir).map_err(|error| BenchError::io("create", &target_dir, error))?;
  let target = target_dir.join(file.abs.file_name().unwrap_or_default());
  fs::rename(&file.abs, &target).map_err(|error| BenchError::io("rename", &file.abs, error))?;
  Ok(describe(corpus, SCENARIO, &timed_update(engine)?, 1, 2).info("moved", &file.rel))
}

pub fn branch_switch(corpus: &str, engine: &Engine, editable: &[Editable]) -> BenchResult<ScenarioRecord> {
  const SCENARIO: &str = "branch_switch";
  let wanted = editable.len() * BRANCH_SWITCH_PERCENT / 100;
  if wanted == 0 {
    return Ok(ScenarioRecord::skipped(corpus, SCENARIO, "too few editable files to replace 30%"));
  }
  engine.ensure_fresh()?;
  let mut applied = 0;
  for (ordinal, file) in spread(editable, wanted).filter(|file| file.abs.exists()).enumerate() {
    applied += 1;
    let text = source::read(&file.abs)?;
    let mut updated = bump_numbers(&text, 2).unwrap_or(text);
    updated.push_str(&appended_function(file.language, ordinal));
    source::write(&file.abs, &updated)?;
  }
  Ok(describe(corpus, SCENARIO, &timed_update(engine)?, applied, applied as u64))
}

fn directory_candidates(project: &Project, files: &[SourceFile]) -> Vec<(PathBuf, usize)> {
  let mut counts: std::collections::BTreeMap<PathBuf, usize> = Default::default();
  for file in files {
    let Some(repo) = project.repos.iter().find(|repo| file.abs.starts_with(&repo.path)) else {
      continue;
    };
    let mut directory = file.abs.parent();
    while let Some(current) = directory.filter(|dir| *dir != repo.path.as_path() && dir.starts_with(&repo.path)) {
      *counts.entry(current.to_path_buf()).or_default() += 1;
      directory = current.parent();
    }
  }
  counts.into_iter().collect()
}

pub fn choose_directory(project: &Project, files: &[SourceFile]) -> Option<(PathBuf, usize)> {
  let ceiling = files.len() * MAX_DELETE_SHARE_PERCENT / 100;
  directory_candidates(project, files)
    .into_iter()
    .filter(|(_, count)| *count >= MIN_DELETE_FILES && *count <= ceiling)
    .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
}

pub fn delete_directory(corpus: &str, engine: &Engine, project: &Project) -> BenchResult<ScenarioRecord> {
  const SCENARIO: &str = "delete_directory";
  let files = list_sources(project)?;
  let Some((directory, count)) = choose_directory(project, &files) else {
    return Ok(ScenarioRecord::skipped(corpus, SCENARIO, "no directory holds between 3 source files and 10% of the corpus"));
  };
  engine.ensure_fresh()?;
  fs::remove_dir_all(&directory).map_err(|error| BenchError::io("remove", &directory, error))?;
  Ok(describe(corpus, SCENARIO, &timed_update(engine)?, count, count as u64).info("directory", relative(project, &directory)))
}

fn relative(project: &Project, directory: &Path) -> String {
  project
    .repos
    .iter()
    .find_map(|repo| directory.strip_prefix(&repo.path).ok().map(|rest| format!("{}/{}", repo.name, rest.display())))
    .unwrap_or_else(|| directory.display().to_string())
}

#[cfg(test)]
mod tests {
  use super::*;
  use atlas_core::{Language, ProjectName, RepoName};
  use atlas_project::{ProjectSettings, RepoConfig};

  fn project(root: &Path) -> Project {
    Project {
      name: ProjectName::new("p").unwrap(),
      settings: ProjectSettings::default(),
      repos: vec![RepoConfig {
        name: RepoName::new("r").unwrap(),
        path: root.to_path_buf(),
        include: Vec::new(),
        exclude: Vec::new(),
        languages: None,
      }],
    }
  }

  fn file(root: &Path, rel: &str) -> SourceFile {
    SourceFile {
      abs: root.join(rel),
      rel: format!("r/{rel}"),
      language: Language::Rust,
    }
  }

  #[test]
  fn should_spread_picks_evenly() {
    let items: Vec<u32> = (0..10).collect();
    assert_eq!(spread(&items, 5).copied().collect::<Vec<_>>(), vec![0, 2, 4, 6, 8]);
  }

  #[test]
  fn should_choose_the_largest_directory_within_the_share_ceiling() {
    let root = Path::new("/w/r");
    let mut files: Vec<SourceFile> = (0..80).map(|index| file(root, &format!("misc/f{index}.rs"))).collect();
    files.extend((0..6).map(|index| file(root, &format!("a/b/f{index}.rs"))));
    files.extend((0..3).map(|index| file(root, &format!("c/f{index}.rs"))));
    let (directory, count) = choose_directory(&project(root), &files).unwrap();
    assert_eq!((directory, count), (root.join("a"), 6));
  }

  #[test]
  fn should_find_no_directory_in_a_tiny_corpus() {
    let root = Path::new("/w/r");
    let files: Vec<SourceFile> = (0..4).map(|index| file(root, &format!("a/f{index}.rs"))).collect();
    assert_eq!(choose_directory(&project(root), &files), None);
  }
}
