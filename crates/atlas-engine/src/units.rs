use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use atlas_core::{Language, LanguageFamily, RelPath, RepoName};
use atlas_project::{Ecosystem, Project};

use crate::engine::{Engine, lock_inner};
use crate::error::EngineResult;
use crate::scip::{UnitsByPackage, scip_units, unit_for_file};
use crate::state::{EngineState, RepoHealth};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LanguageUnits {
  pub rust: Option<RustUnit>,
  pub typescript: Option<TypeScriptUnit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustUnit {
  pub folders: Vec<UnitFolder>,
  pub linked_projects: Vec<CargoProject>,
  pub undiscovered: Vec<RepoName>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeScriptUnit {
  pub folders: Vec<UnitFolder>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitFolder {
  pub repo: RepoName,
  pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoProject {
  pub repo: RepoName,
  pub manifest: RelPath,
  pub path: PathBuf,
}

#[derive(Default)]
struct RepoLanguages {
  families: BTreeSet<LanguageFamily>,
  cargo_units: BTreeSet<RelPath>,
}

impl Engine {
  pub fn language_units(&self) -> EngineResult<LanguageUnits> {
    let mut inner = lock_inner(&self.inner);
    self.sync(&mut inner)?;
    Ok(language_units_of(&self.project(), &inner.state))
  }
}

fn languages_by_repo(state: &EngineState) -> BTreeMap<&RepoName, RepoLanguages> {
  let mut by_repo: BTreeMap<&RepoName, RepoLanguages> = BTreeMap::new();
  let mut units: BTreeMap<&RepoName, UnitsByPackage> = BTreeMap::new();
  for (repo, path) in state.files.keys() {
    let Some(family) = Language::from_path(path.as_str()).map(Language::family) else {
      continue;
    };
    let languages = by_repo.entry(repo).or_default();
    languages.families.insert(family);
    if family != LanguageFamily::Rust {
      continue;
    }
    let Some(graph) = state.repos.get(repo).and_then(|repo_state| repo_state.graph.as_ref()) else {
      continue;
    };
    let units = units.entry(repo).or_insert_with(|| scip_units(graph));
    if let Some((manifest, _, Ecosystem::Cargo)) = unit_for_file(graph, units, path) {
      languages.cargo_units.insert(manifest.clone());
    }
  }
  by_repo
}

fn language_units_of(project: &Project, state: &EngineState) -> LanguageUnits {
  let by_repo = languages_by_repo(state);
  let mut rust_folders = Vec::new();
  let mut linked_projects = Vec::new();
  let mut linked_paths = BTreeSet::new();
  let mut undiscovered = Vec::new();
  let mut ts_folders = Vec::new();
  for config in &project.repos {
    let Some(repo_state) = state.repos.get(&config.name) else { continue };
    if repo_state.health != RepoHealth::Present {
      continue;
    }
    let Some(languages) = by_repo.get(&config.name) else { continue };
    let folder = || UnitFolder {
      repo: config.name.clone(),
      root: repo_state.config.path.clone(),
    };
    if languages.families.contains(&LanguageFamily::Ts) {
      ts_folders.push(folder());
    }
    if !languages.families.contains(&LanguageFamily::Rust) {
      continue;
    }
    rust_folders.push(folder());
    if languages.cargo_units.is_empty() {
      undiscovered.push(config.name.clone());
    }
    for manifest in &languages.cargo_units {
      let path = repo_state.config.path.join(manifest.as_str());
      if linked_paths.insert(path.clone()) {
        linked_projects.push(CargoProject {
          repo: config.name.clone(),
          manifest: manifest.clone(),
          path,
        });
      }
    }
  }
  LanguageUnits {
    rust: (!rust_folders.is_empty()).then_some(RustUnit {
      folders: rust_folders,
      linked_projects,
      undiscovered,
    }),
    typescript: (!ts_folders.is_empty()).then_some(TypeScriptUnit { folders: ts_folders }),
  }
}
