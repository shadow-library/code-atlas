use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use atlas_core::{Language, ProjectName, RepoName};
use fd_lock::RwLock;
use serde::{Deserialize, Serialize};

use crate::binding::{Binding, Locator, Resolution};
use crate::error::ProjectError;
use crate::globs::PathGlobs;
use crate::home::AtlasHome;

const LOCK_FILE: &str = "projects.toml.lock";
const MARKER_FILE: &str = ".code-atlas.toml";
const MAX_REPO_NAME_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoConfig {
  pub name: RepoName,
  pub path: PathBuf,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub include: Vec<String>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub exclude: Vec<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub languages: Option<Vec<Language>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LlmSettings {
  pub provider: Option<String>,
  pub model: Option<String>,
  pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScipSettings {
  pub enabled: bool,
  pub debounce_secs: u64,
}

impl Default for ScipSettings {
  fn default() -> Self {
    Self {
      enabled: false,
      debounce_secs: 30,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LspUnit {
  Project,
  Repo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LspSettings {
  pub enabled: bool,
  pub download: bool,
  pub typescript_command: Option<Vec<String>>,
  pub unit: LspUnit,
  pub max_servers: usize,
  pub idle_secs: u64,
  pub ready_timeout_secs: u64,
  pub check_timeout_secs: u64,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub exclude: Vec<String>,
}

impl Default for LspSettings {
  fn default() -> Self {
    Self {
      enabled: true,
      download: true,
      typescript_command: None,
      unit: LspUnit::Project,
      max_servers: 2,
      idle_secs: 600,
      ready_timeout_secs: 300,
      check_timeout_secs: 120,
      exclude: Vec::new(),
    }
  }
}

fn check_exclude_glob(glob: &str) -> Result<(), String> {
  if glob.trim().is_empty() {
    return Err("lsp.exclude must not contain an empty glob".to_owned());
  }
  if glob.starts_with('/') {
    return Err(format!("lsp.exclude globs are relative to the repo root; {glob:?} is absolute"));
  }
  let (mut class, mut braces, mut escaped) = (false, 0_i32, false);
  for character in glob.chars() {
    match (escaped, character) {
      (true, _) => escaped = false,
      (false, '\\') => escaped = true,
      (false, '[') => class = true,
      (false, ']') => class = false,
      (false, '{') => braces += 1,
      (false, '}') => braces -= 1,
      _ => {}
    }
  }
  match (class, braces) {
    (false, 0) => Ok(()),
    _ => Err(format!("lsp.exclude: {glob:?} has an unclosed or unbalanced [ ] or {{ }}")),
  }
}

impl LspSettings {
  pub fn validate(&self) -> Result<(), String> {
    if self.max_servers < 1 {
      return Err("lsp.max_servers must be at least 1".to_owned());
    }
    for glob in &self.exclude {
      check_exclude_glob(glob)?;
    }
    if let Err(error) = crate::PathGlobs::new(&self.exclude) {
      return Err(format!("lsp.exclude: {error}"));
    }
    let Some(command) = &self.typescript_command else {
      return Ok(());
    };
    let Some(program) = command.first() else {
      return Err("lsp.typescript_command must not be empty".to_owned());
    };
    if !Path::new(program).is_absolute() {
      return Err(format!("lsp.typescript_command must start with an absolute path, got {program:?}"));
    }
    Ok(())
  }

  fn is_default(&self) -> bool {
    *self == Self::default()
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ProjectSettings {
  pub llm: LlmSettings,
  pub scip: ScipSettings,
  #[serde(skip_serializing_if = "LspSettings::is_default")]
  pub lsp: LspSettings,
  pub max_file_bytes: u64,
}

impl ProjectSettings {
  pub fn validate(&self) -> Result<(), String> {
    self.lsp.validate()
  }
}

impl Default for ProjectSettings {
  fn default() -> Self {
    Self {
      llm: LlmSettings::default(),
      scip: ScipSettings::default(),
      lsp: LspSettings::default(),
      max_file_bytes: 1024 * 1024,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
  pub name: ProjectName,
  #[serde(default)]
  pub settings: ProjectSettings,
  #[serde(default, rename = "repo")]
  pub repos: Vec<RepoConfig>,
}

impl Project {
  pub fn repo(&self, name: &str) -> Option<&RepoConfig> {
    self.repos.iter().find(|repo| repo.name.as_str() == name)
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSpec {
  pub path: PathBuf,
  pub name: Option<String>,
  pub include: Vec<String>,
  pub exclude: Vec<String>,
  pub languages: Option<Vec<Language>>,
}

impl RepoSpec {
  pub fn new(path: impl Into<PathBuf>) -> Self {
    Self {
      path: path.into(),
      name: None,
      include: Vec::new(),
      exclude: Vec::new(),
      languages: None,
    }
  }

  pub fn named(mut self, name: impl Into<String>) -> Self {
    self.name = Some(name.into());
    self
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoFilters {
  pub include: Vec<String>,
  pub exclude: Vec<String>,
  pub languages: Option<Vec<Language>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
  #[serde(default, rename = "project")]
  projects: Vec<Project>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarkerFile {
  project: String,
}

#[derive(Debug)]
pub struct Registry {
  registry_path: PathBuf,
  projects: Vec<Project>,
}

impl Registry {
  pub fn load(home: &AtlasHome) -> Result<Self, ProjectError> {
    let registry_path = home.registry_path();
    let projects = read_projects(&registry_path)?;
    Ok(Self { registry_path, projects })
  }

  pub fn list(&self) -> &[Project] {
    &self.projects
  }

  pub fn project(&self, name: &str) -> Result<&Project, ProjectError> {
    self
      .projects
      .iter()
      .find(|project| project.name.as_str() == name)
      .ok_or_else(|| ProjectError::ProjectNotFound(name.to_owned()))
  }

  pub fn create_project(&mut self, name: &str, repos: Vec<RepoSpec>) -> Result<&Project, ProjectError> {
    let name = ProjectName::new(name)?;
    self.mutate(|projects| {
      if projects.iter().any(|project| project.name == name) {
        return Err(ProjectError::ProjectExists(name.to_string()));
      }
      let mut project = Project {
        name: name.clone(),
        settings: ProjectSettings::default(),
        repos: Vec::new(),
      };
      for spec in repos {
        let repo = build_repo(&project, spec)?;
        project.repos.push(repo);
      }
      projects.push(project);
      Ok(())
    })?;
    self.project(name.as_str())
  }

  pub fn delete_project(&mut self, name: &str) -> Result<Project, ProjectError> {
    self.mutate(|projects| {
      let index = projects
        .iter()
        .position(|project| project.name.as_str() == name)
        .ok_or_else(|| ProjectError::ProjectNotFound(name.to_owned()))?;
      Ok(projects.remove(index))
    })
  }

  pub fn add_repo(&mut self, project: &str, spec: RepoSpec) -> Result<RepoConfig, ProjectError> {
    self.mutate(|projects| {
      let target = find_project_mut(projects, project)?;
      let repo = build_repo(target, spec)?;
      target.repos.push(repo.clone());
      Ok(repo)
    })
  }

  pub fn remove_repo(&mut self, project: &str, repo: &str) -> Result<RepoConfig, ProjectError> {
    self.mutate(|projects| {
      let target = find_project_mut(projects, project)?;
      let index = repo_index(target, repo)?;
      Ok(target.repos.remove(index))
    })
  }

  pub fn rename_repo(&mut self, project: &str, from: &str, to: &str) -> Result<(), ProjectError> {
    let to = RepoName::new(to)?;
    self.mutate(|projects| {
      let target = find_project_mut(projects, project)?;
      let index = repo_index(target, from)?;
      if target.repos.iter().any(|repo| repo.name == to) {
        return Err(ProjectError::DuplicateRepo {
          project: project.to_owned(),
          repo: to.to_string(),
        });
      }
      target.repos[index].name = to;
      Ok(())
    })
  }

  pub fn set_repo_filters(&mut self, project: &str, repo: &str, update: impl FnOnce(&RepoFilters) -> RepoFilters) -> Result<(RepoConfig, RepoConfig), ProjectError> {
    self.mutate(|projects| {
      let target = find_project_mut(projects, project)?;
      let index = repo_index(target, repo)?;
      let before = target.repos[index].clone();
      let filters = update(&RepoFilters {
        include: before.include.clone(),
        exclude: before.exclude.clone(),
        languages: before.languages.clone(),
      });
      PathGlobs::new(&filters.include)?;
      PathGlobs::new(&filters.exclude)?;
      let after = RepoConfig {
        include: filters.include,
        exclude: filters.exclude,
        languages: filters.languages,
        ..before.clone()
      };
      target.repos[index] = after.clone();
      Ok((before, after))
    })
  }

  pub fn set_settings(&mut self, project: &str, settings: ProjectSettings) -> Result<(), ProjectError> {
    settings.validate().map_err(|message| ProjectError::InvalidSettings {
      project: project.to_owned(),
      message,
    })?;
    self.mutate(|projects| {
      find_project_mut(projects, project)?.settings = settings;
      Ok(())
    })
  }

  pub fn resolve_project(&self, cwd: &Path, explicit: Option<&str>) -> Result<&Project, ProjectError> {
    self.locate(cwd, explicit).map(|(project, _)| project)
  }

  pub fn resolve(&self, cwd: &Path, explicit: Option<&str>) -> Result<Resolution, ProjectError> {
    let (project, binding) = self.locate(cwd, explicit)?;
    Ok(Resolution {
      project: project.clone(),
      binding,
    })
  }

  fn locate(&self, cwd: &Path, explicit: Option<&str>) -> Result<(&Project, Binding), ProjectError> {
    if let Some(name) = explicit {
      let project = self.project(name)?;
      let binding = fs::canonicalize(cwd).ok().map(Locator::new).and_then(|locator| locator.bind(project));
      return Ok((project, binding.unwrap_or(Binding::Direct)));
    }
    let canonical = fs::canonicalize(cwd).map_err(|source| ProjectError::io("canonicalize", cwd, source))?;
    let locator = Locator::new(canonical);
    if let Some((marker, name)) = find_marker(&locator.cwd)? {
      let project = self.project(&name).map_err(|_| ProjectError::MarkerProjectNotFound { path: marker, project: name })?;
      return Ok((project, locator.bind(project).unwrap_or(Binding::Direct)));
    }
    let bound: Vec<(&Project, Binding)> = self.projects.iter().filter_map(|project| locator.bind(project).map(|binding| (project, binding))).collect();
    let any_direct = bound.iter().any(|(_, binding)| *binding == Binding::Direct);
    let mut candidates: Vec<(&Project, Binding)> = bound.into_iter().filter(|(_, binding)| !any_direct || *binding == Binding::Direct).collect();
    candidates.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    match candidates.len() {
      1 => Ok(candidates.remove(0)),
      0 => Err(ProjectError::NotInProject { cwd: locator.cwd }),
      _ => Err(ProjectError::Ambiguous {
        cwd: locator.cwd,
        candidates: candidates.iter().map(|(project, _)| project.name.to_string()).collect(),
      }),
    }
  }

  fn mutate<T>(&mut self, apply: impl FnOnce(&mut Vec<Project>) -> Result<T, ProjectError>) -> Result<T, ProjectError> {
    let directory = self.registry_path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(directory).map_err(|source| ProjectError::io("create directory", directory, source))?;
    let lock_path = directory.join(LOCK_FILE);
    let lock_file = OpenOptions::new()
      .create(true)
      .truncate(false)
      .write(true)
      .open(&lock_path)
      .map_err(|source| ProjectError::io("open lock", &lock_path, source))?;
    let mut lock = RwLock::new(lock_file);
    let _guard = lock.write().map_err(|source| ProjectError::io("acquire lock", &lock_path, source))?;
    let mut projects = read_projects(&self.registry_path)?;
    let outcome = apply(&mut projects)?;
    write_projects(&self.registry_path, &projects)?;
    self.projects = projects;
    Ok(outcome)
  }
}

fn find_project_mut<'a>(projects: &'a mut [Project], name: &str) -> Result<&'a mut Project, ProjectError> {
  projects
    .iter_mut()
    .find(|project| project.name.as_str() == name)
    .ok_or_else(|| ProjectError::ProjectNotFound(name.to_owned()))
}

fn repo_index(project: &Project, repo: &str) -> Result<usize, ProjectError> {
  project
    .repos
    .iter()
    .position(|candidate| candidate.name.as_str() == repo)
    .ok_or_else(|| ProjectError::RepoNotFound {
      project: project.name.to_string(),
      repo: repo.to_owned(),
    })
}

fn build_repo(project: &Project, spec: RepoSpec) -> Result<RepoConfig, ProjectError> {
  let path = fs::canonicalize(&spec.path).map_err(|source| match source.kind() {
    io::ErrorKind::NotFound => ProjectError::RepoPathMissing(spec.path.clone()),
    _ => ProjectError::io("canonicalize", &spec.path, source),
  })?;
  if !path.is_dir() {
    return Err(ProjectError::RepoPathNotDirectory(path));
  }
  if path.to_str().is_none() {
    return Err(ProjectError::RepoPathNotUtf8(path));
  }
  let name = match &spec.name {
    Some(explicit) => RepoName::new(explicit.as_str())?,
    None => derive_repo_name(&path)?,
  };
  if project.repos.iter().any(|repo| repo.name == name) {
    return Err(ProjectError::DuplicateRepo {
      project: project.name.to_string(),
      repo: name.to_string(),
    });
  }
  if let Some(existing) = project.repos.iter().find(|repo| repo.path == path) {
    return Err(ProjectError::DuplicatePath {
      project: project.name.to_string(),
      repo: existing.name.to_string(),
      path,
    });
  }
  PathGlobs::new(&spec.include)?;
  PathGlobs::new(&spec.exclude)?;
  Ok(RepoConfig {
    name,
    path,
    include: spec.include,
    exclude: spec.exclude,
    languages: spec.languages,
  })
}

fn derive_repo_name(path: &Path) -> Result<RepoName, ProjectError> {
  let raw = path
    .file_name()
    .and_then(|name| name.to_str())
    .ok_or_else(|| ProjectError::NameUnderivable(path.to_owned()))?;
  let slug = slugify(raw).ok_or_else(|| ProjectError::NameUnderivable(path.to_owned()))?;
  Ok(RepoName::new(slug)?)
}

pub fn slugify(raw: &str) -> Option<String> {
  let mut slug = String::with_capacity(raw.len());
  for c in raw.chars().flat_map(char::to_lowercase) {
    let mapped = if c.is_ascii_alphanumeric() || matches!(c, '_' | '.') { c } else { '-' };
    if mapped == '-' && slug.ends_with('-') {
      continue;
    }
    slug.push(mapped);
  }
  let trimmed = slug.trim_start_matches(|c: char| !c.is_ascii_alphanumeric()).trim_end_matches('-');
  let truncated: String = trimmed.chars().take(MAX_REPO_NAME_LEN).collect();
  if truncated.is_empty() { None } else { Some(truncated) }
}

fn find_marker(canonical_cwd: &Path) -> Result<Option<(PathBuf, String)>, ProjectError> {
  for directory in canonical_cwd.ancestors() {
    let marker = directory.join(MARKER_FILE);
    if !marker.is_file() {
      continue;
    }
    let text = fs::read_to_string(&marker).map_err(|source| ProjectError::io("read", &marker, source))?;
    let parsed: MarkerFile = toml::from_str(&text).map_err(|error| ProjectError::InvalidMarker {
      path: marker.clone(),
      message: error.to_string(),
    })?;
    return Ok(Some((marker, parsed.project)));
  }
  Ok(None)
}

fn read_projects(path: &Path) -> Result<Vec<Project>, ProjectError> {
  let text = match fs::read_to_string(path) {
    Ok(text) => text,
    Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(source) => return Err(ProjectError::io("read", path, source)),
  };
  let file: RegistryFile = toml::from_str(&text).map_err(|error| ProjectError::RegistryParse {
    path: path.to_owned(),
    message: error.to_string(),
  })?;
  validate_loaded(path, &file.projects)?;
  Ok(file.projects)
}

fn validate_loaded(path: &Path, projects: &[Project]) -> Result<(), ProjectError> {
  let invalid = |message: String| ProjectError::RegistryParse { path: path.to_owned(), message };
  for (index, project) in projects.iter().enumerate() {
    if let Err(message) = project.settings.validate() {
      return Err(invalid(format!("project {:?}: {message}", project.name.as_str())));
    }
    if projects[..index].iter().any(|earlier| earlier.name == project.name) {
      return Err(invalid(format!("project {:?} is defined twice", project.name.as_str())));
    }
    for (position, repo) in project.repos.iter().enumerate() {
      let earlier = &project.repos[..position];
      if earlier.iter().any(|other| other.name == repo.name) {
        return Err(invalid(format!("repo {:?} is defined twice in project {:?}", repo.name.as_str(), project.name.as_str())));
      }
      if earlier.iter().any(|other| other.path == repo.path) {
        return Err(invalid(format!("path {} is registered twice in project {:?}", repo.path.display(), project.name.as_str())));
      }
    }
  }
  Ok(())
}

fn write_projects(path: &Path, projects: &[Project]) -> Result<(), ProjectError> {
  let directory = path.parent().unwrap_or(Path::new("."));
  let file = RegistryFileRef { projects };
  let text = toml::to_string_pretty(&file).map_err(|error| ProjectError::RegistrySerialize(error.to_string()))?;
  let mut temp = tempfile::NamedTempFile::new_in(directory).map_err(|source| ProjectError::io("create temp file", directory, source))?;
  temp.write_all(text.as_bytes()).map_err(|source| ProjectError::io("write", temp.path(), source))?;
  temp.as_file().sync_all().map_err(|source| ProjectError::io("sync", temp.path(), source))?;
  temp.persist(path).map_err(|error| ProjectError::io("rename into place", path, error.error))?;
  sync_directory(directory);
  Ok(())
}

fn sync_directory(directory: &Path) {
  if let Ok(handle) = File::open(directory) {
    let _ = handle.sync_all();
  }
}

#[derive(Serialize)]
struct RegistryFileRef<'a> {
  #[serde(rename = "project")]
  projects: &'a [Project],
}

#[cfg(test)]
mod tests {
  use std::sync::mpsc;
  use std::time::Duration;

  use tempfile::TempDir;

  use super::*;
  use crate::binding::{CheckoutRole, UnresolvedWorktree, WorktreeOf};
  use crate::worktree::git_fixture;

  struct Sandbox {
    _dir: TempDir,
    home: AtlasHome,
    root: PathBuf,
  }

  impl Sandbox {
    fn new() -> Self {
      let dir = tempfile::tempdir().unwrap();
      let root = fs::canonicalize(dir.path()).unwrap();
      let home = AtlasHome::rooted_at(root.join("atlas"));
      Self { _dir: dir, home, root }
    }

    fn repo_dir(&self, name: &str) -> PathBuf {
      let path = self.root.join("src").join(name);
      fs::create_dir_all(&path).unwrap();
      path
    }

    fn registry(&self) -> Registry {
      Registry::load(&self.home).unwrap()
    }
  }

  #[test]
  fn should_load_an_empty_registry_when_no_file_exists() {
    let sandbox = Sandbox::new();
    assert!(sandbox.registry().list().is_empty());
  }

  #[test]
  fn should_create_and_persist_a_project_with_repos() {
    let sandbox = Sandbox::new();
    let web = sandbox.repo_dir("acme-web");
    let api = sandbox.repo_dir("acme-api");
    let mut registry = sandbox.registry();
    let project = registry.create_project("acme", vec![RepoSpec::new(&web), RepoSpec::new(&api).named("backend")]).unwrap();
    assert_eq!(project.repos.iter().map(|repo| repo.name.as_str()).collect::<Vec<_>>(), ["acme-web", "backend"]);

    let reloaded = sandbox.registry();
    let project = reloaded.project("acme").unwrap();
    assert_eq!(project.repos[0].path, web);
    assert_eq!(project.settings, ProjectSettings::default());
  }

  #[test]
  fn should_reject_a_duplicate_project_name() {
    let sandbox = Sandbox::new();
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![]).unwrap();
    assert!(matches!(registry.create_project("acme", vec![]), Err(ProjectError::ProjectExists(name)) if name == "acme"));
  }

  #[test]
  fn should_reject_invalid_project_names() {
    let sandbox = Sandbox::new();
    assert!(matches!(sandbox.registry().create_project("has space", vec![]), Err(ProjectError::Name(_))));
  }

  #[test]
  fn should_delete_a_project() {
    let sandbox = Sandbox::new();
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![]).unwrap();
    registry.create_project("globex", vec![]).unwrap();
    registry.delete_project("acme").unwrap();
    assert_eq!(sandbox.registry().list().iter().map(|project| project.name.as_str()).collect::<Vec<_>>(), ["globex"]);
    assert!(matches!(registry.delete_project("acme"), Err(ProjectError::ProjectNotFound(_))));
  }

  #[test]
  fn should_derive_and_slugify_repo_names_from_the_directory() {
    let sandbox = Sandbox::new();
    let odd = sandbox.repo_dir("My Cool_Repo (v2)");
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![]).unwrap();
    let repo = registry.add_repo("acme", RepoSpec::new(&odd)).unwrap();
    assert_eq!(repo.name.as_str(), "my-cool_repo-v2");
  }

  #[test]
  fn should_slugify_names() {
    assert_eq!(slugify("My Cool_Repo (v2)").as_deref(), Some("my-cool_repo-v2"));
    assert_eq!(slugify("__hidden").as_deref(), Some("hidden"));
    assert_eq!(slugify("a//b").as_deref(), Some("a-b"));
    assert_eq!(slugify("---"), None);
    assert_eq!(slugify("日本"), None);
    assert_eq!(slugify(&"x".repeat(300)).unwrap().len(), 128);
  }

  #[test]
  fn should_reject_duplicate_repo_names_within_a_project() {
    let sandbox = Sandbox::new();
    let one = sandbox.repo_dir("one");
    let two = sandbox.repo_dir("two");
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![RepoSpec::new(&one).named("svc")]).unwrap();
    let error = registry.add_repo("acme", RepoSpec::new(&two).named("svc")).unwrap_err();
    assert!(matches!(error, ProjectError::DuplicateRepo { repo, .. } if repo == "svc"));
  }

  #[test]
  fn should_reject_a_derived_name_that_collides() {
    let sandbox = Sandbox::new();
    let first = sandbox.repo_dir("a/app");
    let second = sandbox.repo_dir("b/app");
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![RepoSpec::new(&first)]).unwrap();
    assert!(matches!(registry.add_repo("acme", RepoSpec::new(&second)), Err(ProjectError::DuplicateRepo { .. })));
  }

  #[test]
  fn should_reject_a_missing_repo_path_with_a_clear_message() {
    let sandbox = Sandbox::new();
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![]).unwrap();
    let missing = sandbox.root.join("nope");
    let error = registry.add_repo("acme", RepoSpec::new(&missing)).unwrap_err();
    assert!(matches!(error, ProjectError::RepoPathMissing(_)));
    assert_eq!(error.to_string(), format!("repo path {} does not exist", missing.display()));
  }

  #[test]
  fn should_reject_a_file_as_repo_path() {
    let sandbox = Sandbox::new();
    let file = sandbox.root.join("file.txt");
    fs::write(&file, "x").unwrap();
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![]).unwrap();
    assert!(matches!(registry.add_repo("acme", RepoSpec::new(&file)), Err(ProjectError::RepoPathNotDirectory(_))));
  }

  #[test]
  fn should_reject_the_same_path_twice_in_one_project_even_through_a_symlink() {
    let sandbox = Sandbox::new();
    let real = sandbox.repo_dir("real");
    let link = sandbox.root.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![RepoSpec::new(&real).named("one")]).unwrap();
    let error = registry.add_repo("acme", RepoSpec::new(&link).named("two")).unwrap_err();
    assert!(matches!(error, ProjectError::DuplicatePath { repo, .. } if repo == "one"));
  }

  #[test]
  fn should_allow_the_same_path_in_two_projects() {
    let sandbox = Sandbox::new();
    let shared = sandbox.repo_dir("shared");
    let mut registry = sandbox.registry();
    registry.create_project("one", vec![RepoSpec::new(&shared)]).unwrap();
    registry.create_project("two", vec![RepoSpec::new(&shared)]).unwrap();
    assert_eq!(sandbox.registry().list().len(), 2);
  }

  #[test]
  fn should_reject_invalid_globs() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("one");
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![]).unwrap();
    let mut spec = RepoSpec::new(&dir);
    spec.exclude = vec!["{a".to_owned()];
    assert!(matches!(registry.add_repo("acme", spec), Err(ProjectError::InvalidGlob { .. })));
  }

  #[test]
  fn should_replace_repo_filters_in_place_and_return_before_and_after() {
    let sandbox = Sandbox::new();
    let one = sandbox.repo_dir("one");
    let two = sandbox.repo_dir("two");
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![RepoSpec::new(&one), RepoSpec::new(&two)]).unwrap();

    let (before, after) = registry
      .set_repo_filters("acme", "one", |current| RepoFilters {
        exclude: vec!["dist/".to_owned()],
        ..current.clone()
      })
      .unwrap();

    assert_eq!((before.exclude, after.exclude), (Vec::<String>::new(), vec!["dist/".to_owned()]));
    assert_eq!(after.path, before.path);
    let reloaded = sandbox.registry();
    let names: Vec<&str> = reloaded.project("acme").unwrap().repos.iter().map(|repo| repo.name.as_str()).collect();
    assert_eq!(names, ["one", "two"]);
    assert_eq!(reloaded.project("acme").unwrap().repos[0].exclude, ["dist/"]);
  }

  #[test]
  fn should_leave_the_registry_file_byte_identical_when_a_filter_glob_is_invalid() {
    let sandbox = Sandbox::new();
    let one = sandbox.repo_dir("one");
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![RepoSpec::new(&one)]).unwrap();
    let before = fs::read(sandbox.home.registry_path()).unwrap();

    let result = registry.set_repo_filters("acme", "one", |current| RepoFilters {
      exclude: vec!["{a".to_owned()],
      ..current.clone()
    });

    assert!(matches!(result, Err(ProjectError::InvalidGlob { .. })));
    assert_eq!(fs::read(sandbox.home.registry_path()).unwrap(), before);
  }

  #[test]
  fn should_report_an_unknown_repo_when_setting_filters() {
    let sandbox = Sandbox::new();
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![]).unwrap();

    let result = registry.set_repo_filters("acme", "ghost", |current| current.clone());

    assert!(matches!(result, Err(ProjectError::RepoNotFound { repo, .. }) if repo == "ghost"));
  }

  #[test]
  fn should_remove_and_rename_repos() {
    let sandbox = Sandbox::new();
    let one = sandbox.repo_dir("one");
    let two = sandbox.repo_dir("two");
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![RepoSpec::new(&one), RepoSpec::new(&two)]).unwrap();
    registry.rename_repo("acme", "one", "primary").unwrap();
    registry.remove_repo("acme", "two").unwrap();
    let reloaded = sandbox.registry();
    let names: Vec<_> = reloaded.project("acme").unwrap().repos.iter().map(|repo| repo.name.as_str()).collect();
    assert_eq!(names, ["primary"]);
    assert!(matches!(registry.remove_repo("acme", "two"), Err(ProjectError::RepoNotFound { .. })));
    assert!(matches!(registry.rename_repo("acme", "ghost", "x"), Err(ProjectError::RepoNotFound { .. })));
  }

  #[test]
  fn should_reject_renaming_onto_an_existing_repo() {
    let sandbox = Sandbox::new();
    let one = sandbox.repo_dir("one");
    let two = sandbox.repo_dir("two");
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![RepoSpec::new(&one), RepoSpec::new(&two)]).unwrap();
    assert!(matches!(registry.rename_repo("acme", "one", "two"), Err(ProjectError::DuplicateRepo { .. })));
  }

  #[test]
  fn should_round_trip_settings_and_filters() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("one");
    let mut registry = sandbox.registry();
    let mut spec = RepoSpec::new(&dir);
    spec.include = vec!["src/**".to_owned()];
    spec.exclude = vec!["*.gen.ts".to_owned()];
    spec.languages = Some(vec![Language::Rust, Language::TypeScript]);
    registry.create_project("acme", vec![spec]).unwrap();
    let settings = ProjectSettings {
      llm: LlmSettings {
        provider: Some("anthropic".to_owned()),
        model: Some("m".to_owned()),
        enabled: true,
      },
      scip: ScipSettings { enabled: false, debounce_secs: 5 },
      lsp: LspSettings {
        download: false,
        typescript_command: Some(vec!["/opt/ts server/bin/tls".to_owned(), "--flag".to_owned()]),
        unit: LspUnit::Repo,
        max_servers: 3,
        exclude: vec!["fixtures/**".to_owned()],
        ..LspSettings::default()
      },
      max_file_bytes: 42,
    };
    registry.set_settings("acme", settings.clone()).unwrap();
    let reloaded = sandbox.registry();
    let project = reloaded.project("acme").unwrap();
    assert_eq!(project.settings, settings);
    assert_eq!(project.repos[0].include, ["src/**"]);
    assert_eq!(project.repos[0].exclude, ["*.gen.ts"]);
    assert_eq!(project.repos[0].languages, Some(vec![Language::Rust, Language::TypeScript]));
  }

  #[test]
  fn should_default_lsp_settings_for_a_registry_written_before_they_existed() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.home.config_dir()).unwrap();
    let old = "[[project]]\nname = \"acme\"\n\n[project.settings]\nmax_file_bytes = 42\n\n[project.settings.llm]\nenabled = false\n\n[project.settings.scip]\nenabled = true\ndebounce_secs = 7\n";
    fs::write(sandbox.home.registry_path(), old).unwrap();
    let mut registry = Registry::load(&sandbox.home).unwrap();
    let settings = registry.project("acme").unwrap().settings.clone();

    assert_eq!(settings.lsp, LspSettings::default());
    assert_eq!(
      settings.lsp,
      LspSettings {
        enabled: true,
        download: true,
        typescript_command: None,
        unit: LspUnit::Project,
        max_servers: 2,
        idle_secs: 600,
        ready_timeout_secs: 300,
        check_timeout_secs: 120,
        exclude: Vec::new(),
      }
    );
    registry.set_settings("acme", settings).unwrap();
    let rewritten = fs::read_to_string(sandbox.home.registry_path()).unwrap();
    assert!(!rewritten.contains("lsp"), "{rewritten}");
    registry.set_settings("acme", registry.project("acme").unwrap().settings.clone()).unwrap();
    assert_eq!(fs::read_to_string(sandbox.home.registry_path()).unwrap(), rewritten);
  }

  #[test]
  fn should_serialise_default_lsp_settings_identically_to_the_previous_format() {
    let sandbox = Sandbox::new();
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![]).unwrap();
    let written = fs::read_to_string(sandbox.home.registry_path()).unwrap();

    assert_eq!(
      written,
      "[[project]]\nname = \"acme\"\nrepo = []\n\n[project.settings]\nmax_file_bytes = 1048576\n\n[project.settings.llm]\nenabled = false\n\n[project.settings.scip]\nenabled = false\ndebounce_secs = 30\n"
    );
  }

  #[test]
  fn should_write_and_reload_non_default_lsp_settings_in_snake_case() {
    let sandbox = Sandbox::new();
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![]).unwrap();
    let mut settings = ProjectSettings::default();
    settings.lsp.unit = LspUnit::Repo;
    settings.lsp.enabled = false;
    registry.set_settings("acme", settings.clone()).unwrap();
    let written = fs::read_to_string(sandbox.home.registry_path()).unwrap();

    assert!(written.contains("[project.settings.lsp]"), "{written}");
    assert!(written.contains("unit = \"repo\""), "{written}");
    assert_eq!(sandbox.registry().project("acme").unwrap().settings, settings);
  }

  #[test]
  fn should_reject_invalid_lsp_settings_on_update_and_on_load() {
    let sandbox = Sandbox::new();
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![]).unwrap();
    let before = fs::read(sandbox.home.registry_path()).unwrap();
    let invalid = [
      (
        LspSettings {
          max_servers: 0,
          ..LspSettings::default()
        },
        "lsp.max_servers must be at least 1",
      ),
      (
        LspSettings {
          typescript_command: Some(vec![]),
          ..LspSettings::default()
        },
        "lsp.typescript_command must not be empty",
      ),
      (
        LspSettings {
          typescript_command: Some(vec!["tls".to_owned()]),
          ..LspSettings::default()
        },
        "lsp.typescript_command must start with an absolute path, got \"tls\"",
      ),
      (
        LspSettings {
          typescript_command: Some(vec!["./bin/tls".to_owned()]),
          ..LspSettings::default()
        },
        "lsp.typescript_command must start with an absolute path, got \"./bin/tls\"",
      ),
    ];
    for (lsp, message) in invalid {
      let settings = ProjectSettings {
        lsp,
        ..ProjectSettings::default()
      };
      let error = registry.set_settings("acme", settings).unwrap_err();
      assert_eq!(error.to_string(), format!("invalid settings for project \"acme\": {message}"));
    }
    assert_eq!(fs::read(sandbox.home.registry_path()).unwrap(), before);

    fs::write(sandbox.home.registry_path(), "[[project]]\nname = \"acme\"\n[project.settings.lsp]\nmax_servers = 0\n").unwrap();
    assert!(matches!(Registry::load(&sandbox.home), Err(ProjectError::RegistryParse { message, .. }) if message.contains("lsp.max_servers must be at least 1")));
    fs::write(sandbox.home.registry_path(), "[[project]]\nname = \"acme\"\n[project.settings.lsp]\nbogus = 1\n").unwrap();
    assert!(matches!(Registry::load(&sandbox.home), Err(ProjectError::RegistryParse { .. })));
    fs::write(sandbox.home.registry_path(), "[[project]]\nname = \"acme\"\n[project.settings.lsp]\nunit = \"galaxy\"\n").unwrap();
    assert!(matches!(Registry::load(&sandbox.home), Err(ProjectError::RegistryParse { .. })));
  }

  #[test]
  fn should_reject_unknown_keys_deterministically() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.home.config_dir()).unwrap();
    for body in [
      "[[project]]\nname = \"a\"\nbogus = 1\n",
      "[[project]]\nname = \"a\"\n[project.settings]\nbogus = 1\n",
      "bogus = 1\n",
    ] {
      fs::write(sandbox.home.registry_path(), body).unwrap();
      assert!(matches!(Registry::load(&sandbox.home), Err(ProjectError::RegistryParse { .. })), "{body}");
    }
  }

  #[test]
  fn should_reject_duplicates_in_a_hand_edited_file() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.home.config_dir()).unwrap();
    fs::write(sandbox.home.registry_path(), "[[project]]\nname = \"a\"\n[[project]]\nname = \"a\"\n").unwrap();
    assert!(matches!(Registry::load(&sandbox.home), Err(ProjectError::RegistryParse { .. })));
  }

  #[test]
  fn should_leave_the_file_untouched_when_a_mutation_fails() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("one");
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![RepoSpec::new(&dir)]).unwrap();
    let before = fs::read(sandbox.home.registry_path()).unwrap();
    assert!(registry.add_repo("acme", RepoSpec::new(sandbox.root.join("missing"))).is_err());
    assert!(registry.create_project("acme", vec![]).is_err());
    assert_eq!(fs::read(sandbox.home.registry_path()).unwrap(), before);
  }

  #[test]
  fn should_write_atomically_without_leaving_temp_files() {
    let sandbox = Sandbox::new();
    let mut registry = sandbox.registry();
    for name in ["a", "b", "c"] {
      registry.create_project(name, vec![]).unwrap();
    }
    let mut entries: Vec<String> = fs::read_dir(sandbox.home.config_dir())
      .unwrap()
      .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
      .collect();
    entries.sort();
    assert_eq!(entries, ["projects.toml", "projects.toml.lock"]);
  }

  #[test]
  fn should_not_lose_updates_from_concurrent_writers() {
    let sandbox = Sandbox::new();
    let handles: Vec<_> = (0..8)
      .map(|index| {
        let home = sandbox.home.clone();
        std::thread::spawn(move || {
          let mut registry = Registry::load(&home).unwrap();
          registry.create_project(&format!("project-{index}"), vec![]).unwrap();
        })
      })
      .collect();
    for handle in handles {
      handle.join().unwrap();
    }
    let mut names: Vec<_> = sandbox.registry().list().iter().map(|project| project.name.to_string()).collect();
    names.sort();
    assert_eq!(names, (0..8).map(|index| format!("project-{index}")).collect::<Vec<_>>());
  }

  #[test]
  fn should_block_writers_while_the_lock_is_held() {
    let sandbox = Sandbox::new();
    sandbox.registry().create_project("seed", vec![]).unwrap();
    let lock_file = OpenOptions::new().write(true).open(sandbox.home.config_dir().join(LOCK_FILE)).unwrap();
    let mut lock = RwLock::new(lock_file);
    let guard = lock.write().unwrap();

    let (sender, receiver) = mpsc::channel();
    let home = sandbox.home.clone();
    let writer = std::thread::spawn(move || {
      Registry::load(&home).unwrap().create_project("late", vec![]).unwrap();
      sender.send(()).unwrap();
    });
    assert!(receiver.recv_timeout(Duration::from_millis(300)).is_err());
    drop(guard);
    receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    writer.join().unwrap();
    assert_eq!(sandbox.registry().list().len(), 2);
  }

  #[test]
  fn should_pick_up_changes_made_by_another_process_before_mutating() {
    let sandbox = Sandbox::new();
    let mut stale = sandbox.registry();
    sandbox.registry().create_project("other", vec![]).unwrap();
    stale.create_project("mine", vec![]).unwrap();
    assert_eq!(sandbox.registry().list().len(), 2);
  }

  fn registry_with(sandbox: &Sandbox, projects: &[(&str, &[&Path])]) -> Registry {
    let mut registry = sandbox.registry();
    for (name, repos) in projects {
      let specs = repos.iter().enumerate().map(|(index, path)| RepoSpec::new(*path).named(format!("repo-{index}"))).collect();
      registry.create_project(name, specs).unwrap();
    }
    registry
  }

  #[test]
  fn should_prefer_an_explicit_project_name() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("one");
    let registry = registry_with(&sandbox, &[("acme", &[&dir]), ("globex", &[])]);
    assert_eq!(registry.resolve_project(&dir, Some("globex")).unwrap().name.as_str(), "globex");
    assert!(matches!(registry.resolve_project(&dir, Some("ghost")), Err(ProjectError::ProjectNotFound(_))));
  }

  #[test]
  fn should_resolve_from_the_nearest_marker_file() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("one");
    let registry = registry_with(&sandbox, &[("acme", &[&dir]), ("globex", &[&dir])]);
    let outer = sandbox.root.join("work");
    let inner = outer.join("deep/er");
    fs::create_dir_all(&inner).unwrap();
    fs::write(outer.join(MARKER_FILE), "project = \"acme\"\n").unwrap();
    assert_eq!(registry.resolve_project(&inner, None).unwrap().name.as_str(), "acme");
    fs::write(outer.join("deep").join(MARKER_FILE), "project = \"globex\"\n").unwrap();
    assert_eq!(registry.resolve_project(&inner, None).unwrap().name.as_str(), "globex");
  }

  #[test]
  fn should_reject_a_marker_naming_an_unknown_project() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("one");
    let registry = registry_with(&sandbox, &[("acme", &[&dir])]);
    fs::write(dir.join(MARKER_FILE), "project = \"ghost\"\n").unwrap();
    assert!(matches!(registry.resolve_project(&dir, None), Err(ProjectError::MarkerProjectNotFound { project, .. }) if project == "ghost"));
  }

  #[test]
  fn should_reject_a_malformed_marker() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("one");
    let registry = registry_with(&sandbox, &[("acme", &[&dir])]);
    fs::write(dir.join(MARKER_FILE), "projekt = \"acme\"\n").unwrap();
    assert!(matches!(registry.resolve_project(&dir, None), Err(ProjectError::InvalidMarker { .. })));
  }

  #[test]
  fn should_resolve_the_single_project_containing_the_cwd() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("one");
    let other = sandbox.repo_dir("two");
    let nested = dir.join("src/deep");
    fs::create_dir_all(&nested).unwrap();
    let registry = registry_with(&sandbox, &[("acme", &[&dir]), ("globex", &[&other])]);
    assert_eq!(registry.resolve_project(&nested, None).unwrap().name.as_str(), "acme");
  }

  #[test]
  fn should_resolve_once_when_one_project_has_several_matching_repos() {
    let sandbox = Sandbox::new();
    let outer = sandbox.repo_dir("outer");
    let inner = outer.join("inner");
    fs::create_dir_all(&inner).unwrap();
    let registry = registry_with(&sandbox, &[("acme", &[&outer, &inner])]);
    assert_eq!(registry.resolve_project(&inner, None).unwrap().name.as_str(), "acme");
  }

  #[test]
  fn should_report_ambiguity_with_sorted_candidates() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("one");
    let registry = registry_with(&sandbox, &[("zeta", &[&dir]), ("alpha", &[&dir])]);
    let error = registry.resolve_project(&dir, None).unwrap_err();
    assert!(matches!(&error, ProjectError::Ambiguous { candidates, .. } if candidates == &["alpha", "zeta"]));
    assert!(error.to_string().contains("alpha, zeta"));
  }

  #[test]
  fn should_report_not_in_project_with_a_hint() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("one");
    let elsewhere = sandbox.root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let registry = registry_with(&sandbox, &[("acme", &[&dir])]);
    let error = registry.resolve_project(&elsewhere, None).unwrap_err();
    assert!(matches!(error, ProjectError::NotInProject { .. }));
    assert!(error.to_string().contains("--project"));
  }

  #[test]
  fn should_not_treat_a_sibling_with_a_shared_name_prefix_as_inside_the_repo() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("app");
    let sibling = sandbox.repo_dir("app-extra");
    let registry = registry_with(&sandbox, &[("acme", &[&dir])]);
    assert!(matches!(registry.resolve_project(&sibling, None), Err(ProjectError::NotInProject { .. })));
  }

  #[test]
  fn should_canonicalise_a_symlinked_cwd() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("real");
    let nested = dir.join("sub");
    fs::create_dir_all(&nested).unwrap();
    let link = sandbox.root.join("alias");
    std::os::unix::fs::symlink(&dir, &link).unwrap();
    let registry = registry_with(&sandbox, &[("acme", &[&dir])]);
    assert_eq!(registry.resolve_project(&link.join("sub"), None).unwrap().name.as_str(), "acme");
  }

  fn worktree_of(project: &str, repo: &str, worktree: &Path, main: &Path) -> Binding {
    worktree_in(project, repo, worktree, main, CheckoutRole::MainCheckout)
  }

  fn worktree_in(project: &str, repo: &str, worktree: &Path, indexed: &Path, indexed_role: CheckoutRole) -> Binding {
    Binding::Worktree(WorktreeOf {
      project: ProjectName::new(project).unwrap(),
      repo: RepoName::new(repo).unwrap(),
      worktree: worktree.to_path_buf(),
      indexed_checkout: indexed.to_path_buf(),
      indexed_role,
    })
  }

  #[test]
  fn should_bind_the_main_checkout_directly() {
    let sandbox = Sandbox::new();
    let main = git_fixture::repository(&sandbox.root, "main");
    let registry = registry_with(&sandbox, &[("acme", &[&main])]);

    let resolution = registry.resolve(&main.join("src"), None).unwrap();

    assert_eq!((resolution.project.name.as_str(), resolution.binding), ("acme", Binding::Direct));
  }

  #[test]
  fn should_bind_a_worktree_inside_a_registered_repo_as_a_worktree_of_it() {
    let sandbox = Sandbox::new();
    let main = git_fixture::repository(&sandbox.root, "main");
    git_fixture::git(&main, &["worktree", "add", "--quiet", "-b", "feature", ".claude/worktrees/feature"]);
    let worktree = main.join(".claude/worktrees/feature");
    let registry = registry_with(&sandbox, &[("acme", &[&main])]);

    let resolution = registry.resolve(&worktree.join("src"), None).unwrap();

    assert_eq!(resolution.project.name.as_str(), "acme");
    assert_eq!(resolution.binding, worktree_of("acme", "repo-0", &worktree, &main));
    assert_eq!(registry.resolve_project(&worktree, None).unwrap().name.as_str(), "acme");
  }

  #[test]
  fn should_bind_a_worktree_outside_every_registered_path_through_its_main_repository() {
    let sandbox = Sandbox::new();
    let main = git_fixture::repository(&sandbox.root, "main");
    git_fixture::git(&main, &["worktree", "add", "--quiet", "-b", "outside", "../outside"]);
    let worktree = sandbox.root.join("outside");
    let registry = registry_with(&sandbox, &[("acme", &[&main])]);

    assert_eq!(registry.resolve(&worktree, None).unwrap().binding, worktree_of("acme", "repo-0", &worktree, &main));
  }

  #[test]
  fn should_bind_a_worktree_even_when_the_project_is_explicit_or_pinned_by_a_marker() {
    let sandbox = Sandbox::new();
    let main = git_fixture::repository(&sandbox.root, "main");
    git_fixture::git(&main, &["worktree", "add", "--quiet", "-b", "feature", ".worktrees/feature"]);
    let worktree = main.join(".worktrees/feature");
    let registry = registry_with(&sandbox, &[("acme", &[&main]), ("globex", &[])]);
    let expected = worktree_of("acme", "repo-0", &worktree, &main);

    let explicit = registry.resolve(&worktree, Some("acme")).unwrap().binding;
    let unrelated = registry.resolve(&worktree, Some("globex")).unwrap().binding;
    fs::write(worktree.join(MARKER_FILE), "project = \"acme\"\n").unwrap();
    let pinned = registry.resolve(&worktree.join("src"), None).unwrap().binding;

    assert_eq!(explicit, expected);
    assert_eq!(unrelated, Binding::Direct);
    assert_eq!(pinned, expected);
  }

  #[test]
  fn should_name_the_registered_repo_whose_counterpart_contains_the_cwd() {
    let sandbox = Sandbox::new();
    let main = git_fixture::repository(&sandbox.root, "main");
    fs::create_dir_all(main.join("packages/api")).unwrap();
    fs::create_dir_all(main.join("packages/web")).unwrap();
    fs::write(main.join("packages/web/index.ts"), "export const web = 1;\n").unwrap();
    fs::write(main.join("packages/api/index.ts"), "export const api = 1;\n").unwrap();
    git_fixture::git(&main, &["add", "."]);
    git_fixture::git(&main, &["commit", "--quiet", "--message", "packages"]);
    git_fixture::git(&main, &["worktree", "add", "--quiet", "-b", "feature", ".worktrees/feature"]);
    let worktree = main.join(".worktrees/feature");
    let mut registry = sandbox.registry();
    let specs = vec![RepoSpec::new(main.join("packages/api")).named("api"), RepoSpec::new(main.join("packages/web")).named("web")];
    registry.create_project("acme", specs).unwrap();

    let in_web = registry.resolve(&worktree.join("packages/web"), None).unwrap().binding;
    let at_root = registry.resolve(&worktree, None).unwrap().binding;

    assert_eq!(in_web, worktree_of("acme", "web", &worktree, &main));
    assert_eq!(at_root, worktree_of("acme", "api", &worktree, &main));
  }

  #[test]
  fn should_prefer_a_project_that_registers_the_worktree_itself() {
    let sandbox = Sandbox::new();
    let main = git_fixture::repository(&sandbox.root, "main");
    git_fixture::git(&main, &["worktree", "add", "--quiet", "-b", "feature", ".worktrees/feature"]);
    let worktree = main.join(".worktrees/feature");
    let registry = registry_with(&sandbox, &[("acme", &[&main]), ("acme-feature", &[&worktree])]);

    let in_worktree = registry.resolve(&worktree.join("src"), None).unwrap();
    let in_main = registry.resolve(&main.join("src"), None).unwrap();

    assert_eq!((in_worktree.project.name.as_str(), in_worktree.binding), ("acme-feature", Binding::Direct));
    assert_eq!((in_main.project.name.as_str(), in_main.binding), ("acme", Binding::Direct));
  }

  #[test]
  fn should_report_ambiguity_between_projects_that_both_index_the_main_checkout_of_a_worktree() {
    let sandbox = Sandbox::new();
    let main = git_fixture::repository(&sandbox.root, "main");
    git_fixture::git(&main, &["worktree", "add", "--quiet", "-b", "feature", "../feature"]);
    let registry = registry_with(&sandbox, &[("zeta", &[&main]), ("alpha", &[&main])]);

    let error = registry.resolve(&sandbox.root.join("feature"), None).unwrap_err();

    assert!(matches!(&error, ProjectError::Ambiguous { candidates, .. } if candidates == &["alpha", "zeta"]), "{error}");
  }

  #[test]
  fn should_keep_binding_a_submodule_directly_to_the_project_containing_it() {
    let sandbox = Sandbox::new();
    let main = git_fixture::repository(&sandbox.root, "main");
    let submodule = main.join("vendor");
    fs::create_dir_all(&submodule).unwrap();
    fs::create_dir_all(main.join(".git/modules/vendor")).unwrap();
    fs::write(submodule.join(".git"), "gitdir: ../.git/modules/vendor\n").unwrap();
    let registry = registry_with(&sandbox, &[("acme", &[&main])]);

    assert_eq!(registry.resolve(&submodule, None).unwrap().binding, Binding::Direct);
  }

  #[test]
  fn should_bind_a_worktree_of_a_repo_inside_a_registered_plain_parent_folder() {
    let sandbox = Sandbox::new();
    let workspace = sandbox.root.join("workspace");
    let app = git_fixture::repository(&workspace, "app");
    git_fixture::git(&app, &["worktree", "add", "--quiet", "-b", "feature", ".worktrees/feature"]);
    let worktree = app.join(".worktrees/feature");
    let registry = registry_with(&sandbox, &[("acme", &[&workspace])]);

    let in_worktree = registry.resolve(&worktree.join("src"), None).unwrap().binding;
    let in_main = registry.resolve(&app.join("src"), None).unwrap().binding;

    assert_eq!(in_worktree, worktree_of("acme", "repo-0", &worktree, &app));
    assert_eq!(in_main, Binding::Direct);
  }

  #[test]
  fn should_bind_a_worktree_of_an_independent_repo_nested_in_a_registered_repo() {
    let sandbox = Sandbox::new();
    let outer = git_fixture::repository(&sandbox.root, "outer");
    let nested = git_fixture::repository(&outer.join("vendor"), "lib");
    git_fixture::git(&nested, &["worktree", "add", "--quiet", "-b", "feature", ".worktrees/feature"]);
    let worktree = nested.join(".worktrees/feature");
    let registry = registry_with(&sandbox, &[("acme", &[&outer])]);

    assert_eq!(registry.resolve(&worktree, None).unwrap().binding, worktree_of("acme", "repo-0", &worktree, &nested));
    assert_eq!(registry.resolve(&nested, None).unwrap().binding, Binding::Direct);
  }

  #[test]
  fn should_report_ambiguity_between_a_parent_folder_project_and_a_project_registering_the_repo() {
    let sandbox = Sandbox::new();
    let workspace = sandbox.root.join("workspace");
    let app = git_fixture::repository(&workspace, "app");
    git_fixture::git(&app, &["worktree", "add", "--quiet", "-b", "feature", ".worktrees/feature"]);
    let registry = registry_with(&sandbox, &[("zeta", &[&workspace]), ("alpha", &[&app])]);

    let error = registry.resolve(&app.join(".worktrees/feature"), None).unwrap_err();

    assert!(matches!(&error, ProjectError::Ambiguous { candidates, .. } if candidates == &["alpha", "zeta"]), "{error}");
  }

  #[test]
  fn should_bind_directly_where_the_worktree_root_itself_is_registered_under_a_parent_folder_project() {
    let sandbox = Sandbox::new();
    let workspace = sandbox.root.join("workspace");
    let app = git_fixture::repository(&workspace, "app");
    git_fixture::git(&app, &["worktree", "add", "--quiet", "-b", "feature", ".worktrees/feature"]);
    let worktree = app.join(".worktrees/feature");
    let registry = registry_with(&sandbox, &[("acme", &[&workspace, &worktree])]);

    assert_eq!(registry.resolve(&worktree.join("src"), None).unwrap().binding, Binding::Direct);
  }

  #[test]
  fn should_fail_closed_on_an_unresolvable_git_file_below_a_registered_repo() {
    let sandbox = Sandbox::new();
    let main = git_fixture::repository(&sandbox.root, "main");
    git_fixture::git(&main, &["worktree", "add", "--quiet", "-b", "feature", ".worktrees/feature"]);
    let worktree = main.join(".worktrees/feature");
    fs::remove_dir_all(main.join(".git/worktrees/feature")).unwrap();
    let registry = registry_with(&sandbox, &[("acme", &[&main])]);

    let binding = registry.resolve(&worktree.join("src"), None).unwrap().binding;

    assert_eq!(
      binding,
      Binding::UnresolvedWorktree(UnresolvedWorktree {
        project: ProjectName::new("acme").unwrap(),
        repo: RepoName::new("repo-0").unwrap(),
        root: worktree.clone(),
        registered: main.clone(),
      })
    );
    assert_eq!(registry.resolve(&main, None).unwrap().binding, Binding::Direct);
  }

  #[test]
  fn should_bind_a_worktree_whose_commondir_file_is_missing() {
    let sandbox = Sandbox::new();
    let main = git_fixture::repository(&sandbox.root, "main");
    git_fixture::git(&main, &["worktree", "add", "--quiet", "-b", "feature", ".worktrees/feature"]);
    fs::remove_file(main.join(".git/worktrees/feature/commondir")).unwrap();
    let worktree = main.join(".worktrees/feature");
    let registry = registry_with(&sandbox, &[("acme", &[&main])]);

    assert_eq!(registry.resolve(&worktree, None).unwrap().binding, worktree_of("acme", "repo-0", &worktree, &main));
  }

  #[test]
  fn should_call_a_registered_linked_worktree_the_indexed_checkout() {
    let sandbox = Sandbox::new();
    let source = git_fixture::repository(&sandbox.root, "source");
    let bare = sandbox.root.join("repo.git");
    git_fixture::git(&sandbox.root, &["clone", "--quiet", "--bare", source.to_str().unwrap(), bare.to_str().unwrap()]);
    git_fixture::git(&bare, &["worktree", "add", "--quiet", sandbox.root.join("main").to_str().unwrap(), "trunk"]);
    git_fixture::git(&bare, &["worktree", "add", "--quiet", "-b", "feature", sandbox.root.join("feature").to_str().unwrap()]);
    let (main, feature) = (sandbox.root.join("main"), sandbox.root.join("feature"));
    let registry = registry_with(&sandbox, &[("acme", &[&main])]);

    let binding = registry.resolve(&feature, None).unwrap().binding;

    assert_eq!(binding, worktree_in("acme", "repo-0", &feature, &main, CheckoutRole::OtherCheckout));
    assert!(
      binding
        .caveat()
        .unwrap()
        .starts_with(&format!("worktree caveat: answers describe the indexed checkout at {}, not", main.display())),
      "{binding:?}"
    );
    assert!(
      binding
        .edit_refusal()
        .unwrap()
        .contains(&format!(": project acme indexes the checkout at {} (repo repo-0)", main.display())),
      "{binding:?}"
    );
  }

  #[test]
  fn should_render_the_unresolved_worktree_caveat_and_refusal() {
    let binding = Binding::UnresolvedWorktree(UnresolvedWorktree {
      project: ProjectName::new("acme").unwrap(),
      repo: RepoName::new("api").unwrap(),
      root: PathBuf::from("/src/api/.worktrees/x"),
      registered: PathBuf::from("/src/api"),
    });

    assert_eq!(
      binding.caveat().unwrap(),
      "worktree caveat: /src/api/.worktrees/x has a .git entry that does not resolve to a git checkout (a broken worktree?); answers describe the checkout registered at /src/api (repo api of project acme), not /src/api/.worktrees/x; edits are refused here"
    );
    assert_eq!(
      binding.edit_refusal().unwrap(),
      "refusing to edit from /src/api/.worktrees/x: its .git entry does not resolve to a git checkout, so the worktree could not be resolved and the edit could write into the checkout registered at /src/api (repo api of project acme) instead; nothing was written. Repair it with `git worktree repair`, or run the edit from /src/api"
    );
    assert_eq!((Binding::Direct.caveat(), Binding::Direct.edit_refusal()), (None, None));
  }

  #[test]
  fn should_render_the_worktree_caveat_and_refusal_with_both_paths() {
    let binding = WorktreeOf {
      project: ProjectName::new("acme").unwrap(),
      repo: RepoName::new("api").unwrap(),
      worktree: PathBuf::from("/src/api/.worktrees/x"),
      indexed_checkout: PathBuf::from("/src/api"),
      indexed_role: CheckoutRole::MainCheckout,
    };

    assert_eq!(
      binding.caveat(),
      "worktree caveat: answers describe the main checkout at /src/api, not the git worktree at /src/api/.worktrees/x (repo api of project acme); edits are refused here"
    );
    assert_eq!(
      binding.edit_refusal(),
      "refusing to edit from the git worktree at /src/api/.worktrees/x: project acme indexes the main checkout at /src/api (repo api), so the edit would write there, not into this worktree; nothing was written. Edit the worktree's files directly, or run the edit from /src/api"
    );
  }

  #[test]
  fn should_resolve_through_the_tmp_symlink_where_one_exists() {
    if !Path::new("/tmp").is_symlink() {
      return;
    }
    let scratch = tempfile::Builder::new().prefix("atlas-project-").tempdir_in("/tmp").unwrap();
    let repo_dir = scratch.path().join("repo");
    fs::create_dir_all(repo_dir.join("sub")).unwrap();
    assert!(repo_dir.starts_with("/tmp"));
    let sandbox = Sandbox::new();
    let registry = registry_with(&sandbox, &[("acme", &[&repo_dir])]);
    assert!(registry.project("acme").unwrap().repos[0].path.starts_with("/private/tmp"));
    assert_eq!(registry.resolve_project(&repo_dir.join("sub"), None).unwrap().name.as_str(), "acme");
  }
}
