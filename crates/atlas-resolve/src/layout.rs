use std::collections::{BTreeMap, HashMap, HashSet};

use atlas_core::{RelPath, RepoName};
use atlas_project::{CargoDependency, CargoDependencySource, CargoPackage, DependencySection, NpmPackage, PackageGraph, PackageInfo, RepoDir, TsConfigInfo};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct PackageKey {
  pub repo: RepoName,
  pub manifest: RelPath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExternCrate {
  InProject(PackageKey),
  Ambiguous(Vec<PackageKey>),
  External(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DependencyUse {
  Any,
  Runtime,
  Test,
  BuildScript,
}

impl DependencyUse {
  fn admits(self, section: &DependencySection) -> bool {
    match self {
      Self::Any => true,
      Self::Runtime => matches!(section, DependencySection::Normal | DependencySection::Peer),
      Self::Test => matches!(section, DependencySection::Normal | DependencySection::Peer | DependencySection::Dev),
      Self::BuildScript => matches!(section, DependencySection::Build),
    }
  }
}

#[derive(Debug, Default)]
pub(crate) struct Layout {
  repos: BTreeMap<RepoName, PackageGraph>,
  npm_by_name: HashMap<String, Vec<PackageKey>>,
  cargo_by_name: HashMap<String, Vec<PackageKey>>,
}

impl Layout {
  pub fn set_repo(&mut self, repo: RepoName, graph: PackageGraph) {
    self.repos.insert(repo, graph);
    self.reindex();
  }

  pub fn remove_repo(&mut self, repo: &RepoName) -> bool {
    let removed = self.repos.remove(repo).is_some();
    self.reindex();
    removed
  }

  fn reindex(&mut self) {
    self.npm_by_name.clear();
    self.cargo_by_name.clear();
    for (repo, graph) in &self.repos {
      for package in &graph.packages {
        let key = PackageKey {
          repo: repo.clone(),
          manifest: package.manifest.clone(),
        };
        match (package.as_npm(), package.as_cargo()) {
          (Some(npm), _) => {
            if let Some(name) = &npm.name {
              self.npm_by_name.entry(name.clone()).or_default().push(key);
            }
          }
          (_, Some(cargo)) => self.cargo_by_name.entry(cargo.name.clone()).or_default().push(key),
          _ => {}
        }
      }
    }
  }

  pub fn package(&self, key: &PackageKey) -> Option<&PackageInfo> {
    self.repos.get(&key.repo)?.packages.iter().find(|package| package.manifest == key.manifest)
  }

  pub fn cargo(&self, key: &PackageKey) -> Option<&CargoPackage> {
    self.package(key)?.as_cargo()
  }

  pub fn npm(&self, key: &PackageKey) -> Option<&NpmPackage> {
    self.package(key)?.as_npm()
  }

  pub fn package_for_file(&self, repo: &RepoName, path: &RelPath) -> Option<PackageKey> {
    let package = self.repos.get(repo)?.package_for_file(path)?;
    Some(PackageKey {
      repo: repo.clone(),
      manifest: package.manifest.clone(),
    })
  }

  pub fn tsconfig_for_file(&self, repo: &RepoName, path: &RelPath) -> Option<&TsConfigInfo> {
    self.repos.get(repo)?.tsconfig_for_file(path)
  }

  pub fn npm_packages_named(&self, name: &str) -> &[PackageKey] {
    self.npm_by_name.get(name).map_or(&[], Vec::as_slice)
  }

  pub fn cargo_packages(&self) -> impl Iterator<Item = PackageKey> + '_ {
    self.repos.iter().flat_map(|(repo, graph)| {
      graph.packages.iter().filter(|package| package.as_cargo().is_some()).map(move |package| PackageKey {
        repo: repo.clone(),
        manifest: package.manifest.clone(),
      })
    })
  }

  pub fn graph(&self, repo: &RepoName) -> Option<&PackageGraph> {
    self.repos.get(repo)
  }

  pub fn extern_crate(&self, from: &PackageKey, name: &str) -> Option<ExternCrate> {
    let cargo = self.cargo(from)?;
    if let Some(dependency) = cargo.dependencies.iter().find(|dependency| dependency.extern_crate_name() == name) {
      let mut targets = self.dependency_targets(from, dependency);
      return Some(match targets.len() {
        0 => ExternCrate::External(dependency.package_name.clone()),
        1 => ExternCrate::InProject(targets.remove(0)),
        _ => ExternCrate::Ambiguous(targets),
      });
    }
    cargo.lib().filter(|lib| lib.name.replace('-', "_") == name)?;
    Some(ExternCrate::InProject(from.clone()))
  }

  pub fn dependency_packages(&self, from: &PackageKey) -> Vec<PackageKey> {
    self.dependencies_in(from, |_| true)
  }

  pub fn linked_packages(&self, from: &PackageKey, usage: DependencyUse) -> HashSet<PackageKey> {
    let mut linked = HashSet::from([from.clone()]);
    let mut pending = self.dependencies_in(from, |section| usage.admits(section));
    while let Some(package) = pending.pop() {
      if linked.insert(package.clone()) {
        pending.extend(self.dependencies_in(&package, |section| DependencyUse::Runtime.admits(section)));
      }
    }
    linked
  }

  fn dependencies_in(&self, from: &PackageKey, admit: impl Fn(&DependencySection) -> bool) -> Vec<PackageKey> {
    let Some(package) = self.package(from) else { return Vec::new() };
    match (package.as_cargo(), package.as_npm()) {
      (Some(cargo), _) => cargo
        .dependencies
        .iter()
        .filter(|dependency| admit(&dependency.section))
        .filter_map(|dependency| self.in_project_dependency(from, dependency))
        .collect(),
      (_, Some(npm)) => npm
        .dependencies
        .iter()
        .filter(|dependency| admit(&dependency.section))
        .filter_map(|dependency| self.npm_package(&from.repo, &dependency.name))
        .collect(),
      _ => Vec::new(),
    }
  }

  pub fn npm_package(&self, preferred_repo: &RepoName, name: &str) -> Option<PackageKey> {
    let candidates = self.npm_packages_named(name);
    if let Some(local) = candidates.iter().find(|key| key.repo == *preferred_repo) {
      return Some(local.clone());
    }
    match candidates {
      [single] => Some(single.clone()),
      _ => None,
    }
  }

  pub fn in_project_dependency(&self, from: &PackageKey, dependency: &CargoDependency) -> Option<PackageKey> {
    let mut targets = self.dependency_targets(from, dependency);
    match targets.len() {
      1 => Some(targets.remove(0)),
      _ => None,
    }
  }

  fn dependency_targets(&self, from: &PackageKey, dependency: &CargoDependency) -> Vec<PackageKey> {
    let with_lib = |key: &PackageKey| self.cargo(key).is_some_and(|cargo| cargo.lib().is_some());
    if let CargoDependencySource::Path { resolved: Some(dir), .. } = &dependency.source {
      return self.cargo_package_at(&from.repo, dir).filter(|key| with_lib(key)).into_iter().collect();
    }
    self
      .cargo_by_name
      .get(&dependency.package_name)
      .map(|candidates| candidates.iter().filter(|key| key.repo != from.repo && with_lib(key)).cloned().collect())
      .unwrap_or_default()
  }

  fn cargo_package_at(&self, repo: &RepoName, dir: &RepoDir) -> Option<PackageKey> {
    let graph = self.repos.get(repo)?;
    let package = graph.packages.iter().find(|package| package.as_cargo().is_some() && package.root == *dir)?;
    Some(PackageKey {
      repo: repo.clone(),
      manifest: package.manifest.clone(),
    })
  }
}
