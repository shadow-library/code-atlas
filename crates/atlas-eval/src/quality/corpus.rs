use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atlas_core::{ProjectName, RepoName};
use atlas_engine::{Engine, EngineOptions, FreshnessReport};
use atlas_project::{AtlasHome, Project, ProjectSettings, RepoConfig};
use atlas_query::QueryEngine;
use tempfile::TempDir;

use super::error::{QualityError, QualityResult};
use super::snapshot::Snapshot;
use super::truth::{Truth, load_truth};

#[derive(Debug, Clone)]
pub struct RepoSource {
  pub name: String,
  pub dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct FixtureGroup {
  pub name: String,
  pub truth: Truth,
  pub repos: Vec<RepoSource>,
}

pub struct IndexedProject {
  pub project: Project,
  pub engine: Arc<Engine>,
  pub query: QueryEngine,
  pub snapshot: Snapshot,
  pub degraded: Option<String>,
  pub tree: PathBuf,
  _scratch: TempDir,
}

pub struct Corpus {
  pub groups: Vec<FixtureGroup>,
  pub projects: BTreeMap<String, IndexedProject>,
}

pub fn copy_tree(from: &Path, to: &Path) -> QualityResult<()> {
  fs::create_dir_all(to).map_err(|error| QualityError::io(to, error))?;
  for entry in fs::read_dir(from).map_err(|error| QualityError::io(from, error))? {
    let entry = entry.map_err(|error| QualityError::io(from, error))?;
    let target = to.join(entry.file_name());
    let kind = entry.file_type().map_err(|error| QualityError::io(entry.path(), error))?;
    match kind.is_dir() {
      true => copy_tree(&entry.path(), &target)?,
      false => {
        fs::copy(entry.path(), &target).map_err(|error| QualityError::io(entry.path(), error))?;
      }
    }
  }
  Ok(())
}

pub fn discover(fixtures: &Path, only: &[String]) -> QualityResult<Vec<FixtureGroup>> {
  let mut locations: Vec<(String, PathBuf)> = vec![("acme".to_owned(), fixtures.join("acme")), ("globex".to_owned(), fixtures.join("globex"))];
  let adversarial = fixtures.join("adversarial");
  let mut cases: Vec<PathBuf> = fs::read_dir(&adversarial)
    .map_err(|error| QualityError::io(&adversarial, error))?
    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
    .filter(|path| path.join("expected.toml").is_file())
    .collect();
  cases.sort();
  for case in cases {
    let name = case.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    locations.push((format!("adversarial/{name}"), case));
  }
  let mut groups = Vec::new();
  for (name, dir) in locations {
    if !only.is_empty() && !only.iter().any(|filter| name.contains(filter.as_str())) {
      continue;
    }
    let truth = load_truth(&dir.join("expected.toml"))?;
    let repos = match name.starts_with("adversarial/") {
      true => truth
        .project
        .repos
        .iter()
        .map(|repo| RepoSource {
          name: repo.clone(),
          dir: dir.clone(),
        })
        .collect(),
      false => truth
        .project
        .repos
        .iter()
        .map(|repo| RepoSource {
          name: repo.clone(),
          dir: dir.join(repo),
        })
        .collect(),
    };
    groups.push(FixtureGroup { name, truth, repos });
  }
  Ok(groups)
}

impl IndexedProject {
  pub fn build(group: &FixtureGroup) -> QualityResult<Self> {
    let scratch = tempfile::tempdir().map_err(|error| QualityError::io(std::env::temp_dir(), error))?;
    let tree = scratch.path().join("tree");
    let mut repos = Vec::new();
    for source in &group.repos {
      let target = tree.join(&source.name);
      copy_tree(&source.dir, &target)?;
      repos.push(RepoConfig {
        name: RepoName::new(&source.name).map_err(|error| QualityError::truth(&group.name, error.to_string()))?,
        path: target,
        include: Vec::new(),
        exclude: Vec::new(),
        languages: None,
      });
    }
    let project = Project {
      name: ProjectName::new(&group.truth.project.name).map_err(|error| QualityError::truth(&group.name, error.to_string()))?,
      settings: ProjectSettings::default(),
      repos,
    };
    let engine = Arc::new(Engine::open(&project, &AtlasHome::rooted_at(scratch.path().join("home")), EngineOptions::default())?);
    let degraded = match engine.ensure_fresh()? {
      FreshnessReport::Fresh(_) => None,
      FreshnessReport::Degraded { reason, failed_files, .. } => Some(format!("{reason} ({} failed files)", failed_files.len())),
    };
    let snapshot = Snapshot::load(&engine, &project)?;
    let query = QueryEngine::new(Arc::clone(&engine));
    Ok(Self {
      project,
      engine,
      query,
      snapshot,
      degraded,
      tree,
      _scratch: scratch,
    })
  }
}

impl Corpus {
  pub fn index(groups: Vec<FixtureGroup>) -> QualityResult<Self> {
    let mut projects = BTreeMap::new();
    for group in &groups {
      tracing::info!(group = %group.name, "indexing fixture");
      projects.insert(group.truth.project.name.clone(), IndexedProject::build(group)?);
    }
    Ok(Self { groups, projects })
  }

  pub fn project(&self, name: &str) -> Option<&IndexedProject> {
    self.projects.get(name)
  }
}
