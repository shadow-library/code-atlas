use std::fmt;
use std::path::Path;

use atlas_core::{RelPath, RustTarget};

use crate::discovery::DiscoveredFile;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoDir(Option<RelPath>);

impl RepoDir {
  pub fn root() -> Self {
    Self(None)
  }

  pub fn new(path: RelPath) -> Self {
    Self(Some(path))
  }

  pub fn of_file(file: &RelPath) -> Self {
    Self(file.parent())
  }

  pub fn as_rel_path(&self) -> Option<&RelPath> {
    self.0.as_ref()
  }

  pub fn is_root(&self) -> bool {
    self.0.is_none()
  }

  pub fn depth(&self) -> usize {
    self.0.as_ref().map_or(0, |path| path.segments().count())
  }

  pub fn as_str(&self) -> &str {
    self.0.as_ref().map_or("", RelPath::as_str)
  }

  pub fn contains(&self, file: &RelPath) -> bool {
    match &self.0 {
      None => true,
      Some(dir) => file.strip_dir_prefix(dir).is_some(),
    }
  }

  pub fn resolve(&self, relative: &str) -> Option<RepoDir> {
    let mut segments: Vec<&str> = self.as_str().split('/').filter(|segment| !segment.is_empty()).collect();
    if relative.starts_with('/') || relative.starts_with('\\') {
      return None;
    }
    for segment in relative.split(['/', '\\']) {
      match segment {
        "" | "." => {}
        ".." => {
          segments.pop()?;
        }
        other => segments.push(other),
      }
    }
    if segments.is_empty() {
      return Some(Self::root());
    }
    RelPath::new(&segments.join("/")).ok().map(Self::new)
  }

  pub fn file(&self, relative: &str) -> Option<RelPath> {
    self.resolve(relative)?.0
  }

  pub fn join_pattern(&self, pattern: &str) -> String {
    match self.as_str() {
      "" => pattern.to_owned(),
      dir => format!("{dir}/{pattern}"),
    }
  }
}

impl fmt::Display for RepoDir {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(if self.is_root() { "." } else { self.as_str() })
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestWarning {
  pub file: RelPath,
  pub error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ecosystem {
  Cargo,
  Npm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencySection {
  Normal,
  Dev,
  Build,
  Peer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CargoDependencySource {
  Registry,
  Path { raw: String, resolved: Option<RepoDir> },
  Git { url: String },
  UnresolvedWorkspace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoDependency {
  pub local_name: String,
  pub package_name: String,
  pub section: DependencySection,
  pub source: CargoDependencySource,
  pub version: Option<String>,
  pub optional: bool,
  pub target_cfg: Option<String>,
}

impl CargoDependency {
  pub fn extern_crate_name(&self) -> String {
    self.local_name.replace('-', "_")
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustTargetInfo {
  pub target: RustTarget,
  pub name: String,
  pub root_file: RelPath,
}

impl RustTargetInfo {
  pub fn module_dir(&self) -> RepoDir {
    RepoDir::of_file(&self.root_file)
  }
}

pub fn default_rust_module_path(target: &RustTargetInfo, file: &RelPath) -> Option<Vec<String>> {
  if *file == target.root_file {
    return Some(Vec::new());
  }
  if target.target == RustTarget::BuildScript {
    return None;
  }
  let module_dir = target.module_dir();
  let within = match module_dir.as_rel_path() {
    Some(dir) => file.strip_dir_prefix(dir)?,
    None => file.as_str(),
  };
  let stem = within.strip_suffix(".rs")?;
  let mut segments: Vec<String> = stem.split('/').map(str::to_owned).collect();
  if segments.last().is_some_and(|last| last == "mod") {
    segments.pop();
  }
  Some(segments)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoPackage {
  pub name: String,
  pub version: Option<String>,
  pub targets: Vec<RustTargetInfo>,
  pub dependencies: Vec<CargoDependency>,
  pub workspace_root: Option<RepoDir>,
}

impl CargoPackage {
  pub fn lib(&self) -> Option<&RustTargetInfo> {
    self.targets.iter().find(|target| target.target == RustTarget::Lib)
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NpmSpec {
  File(String),
  Link(String),
  Workspace(String),
  Semver(String),
  Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpmDependency {
  pub name: String,
  pub section: DependencySection,
  pub spec: NpmSpec,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NpmExportTarget {
  Path(String),
  Blocked,
  Conditions(Vec<(String, NpmExportTarget)>),
  Fallbacks(Vec<NpmExportTarget>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum NpmExports {
  Root(NpmExportTarget),
  Subpaths(Vec<(String, NpmExportTarget)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NpmPackage {
  pub name: Option<String>,
  pub version: Option<String>,
  pub main: Option<String>,
  pub module: Option<String>,
  pub types: Option<String>,
  pub exports: Option<NpmExports>,
  pub dependencies: Vec<NpmDependency>,
  pub workspace_root: Option<RepoDir>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PackageKind {
  Cargo(CargoPackage),
  Npm(NpmPackage),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackageInfo {
  pub manifest: RelPath,
  pub root: RepoDir,
  pub kind: PackageKind,
}

impl PackageInfo {
  pub fn ecosystem(&self) -> Ecosystem {
    match self.kind {
      PackageKind::Cargo(_) => Ecosystem::Cargo,
      PackageKind::Npm(_) => Ecosystem::Npm,
    }
  }

  pub fn name(&self) -> Option<&str> {
    match &self.kind {
      PackageKind::Cargo(cargo) => Some(&cargo.name),
      PackageKind::Npm(npm) => npm.name.as_deref(),
    }
  }

  pub fn as_cargo(&self) -> Option<&CargoPackage> {
    match &self.kind {
      PackageKind::Cargo(cargo) => Some(cargo),
      PackageKind::Npm(_) => None,
    }
  }

  pub fn as_npm(&self) -> Option<&NpmPackage> {
    match &self.kind {
      PackageKind::Npm(npm) => Some(npm),
      PackageKind::Cargo(_) => None,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceInfo {
  pub manifest: RelPath,
  pub root: RepoDir,
  pub ecosystem: Ecosystem,
  pub is_virtual: bool,
  pub member_patterns: Vec<String>,
  pub exclude_patterns: Vec<String>,
  pub members: Vec<RepoDir>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsPathMapping {
  pub pattern: String,
  pub targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsConfigInfo {
  pub path: RelPath,
  pub dir: RepoDir,
  pub extends: Vec<std::path::PathBuf>,
  pub base_url: Option<RepoDir>,
  pub paths_base: Option<RepoDir>,
  pub paths: Vec<TsPathMapping>,
  pub root_dir: Option<RepoDir>,
  pub include: Vec<String>,
}

impl TsConfigInfo {
  pub fn is_primary(&self) -> bool {
    matches!(self.path.file_name(), "tsconfig.json" | "jsconfig.json")
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustFileTarget {
  pub target: RustTarget,
  pub module_path: Vec<String>,
}

pub(crate) struct LoadContext<'a> {
  pub root: &'a Path,
  pub files: &'a [DiscoveredFile],
  pub warnings: Vec<ManifestWarning>,
}

impl LoadContext<'_> {
  pub fn warn(&mut self, file: &RelPath, error: impl fmt::Display) {
    self.warnings.push(ManifestWarning {
      file: file.clone(),
      error: error.to_string(),
    });
  }

  pub fn read(&mut self, file: &RelPath) -> Option<String> {
    match std::fs::read_to_string(self.root.join(file.as_str())) {
      Ok(text) => Some(text),
      Err(error) => {
        self.warn(file, format_args!("cannot read: {error}"));
        None
      }
    }
  }

  pub fn files_named<'s>(&'s self, name: &'s str) -> impl Iterator<Item = &'s RelPath> {
    self.files.iter().map(|file| &file.path).filter(move |path| path.file_name() == name)
  }
}
