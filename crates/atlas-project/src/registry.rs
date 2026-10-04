use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use atlas_core::{Language, ProjectName, RepoName};
use fd_lock::RwLock;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ProjectSettings {
  pub llm: LlmSettings,
  pub scip: ScipSettings,
  pub max_file_bytes: u64,
}

impl Default for ProjectSettings {
  fn default() -> Self {
    Self {
      llm: LlmSettings::default(),
      scip: ScipSettings::default(),
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
    self.mutate(|projects| {
      find_project_mut(projects, project)?.settings = settings;
      Ok(())
    })
  }

  pub fn resolve_project(&self, cwd: &Path, explicit: Option<&str>) -> Result<&Project, ProjectError> {
    if let Some(name) = explicit {
      return self.project(name);
    }
    let canonical = fs::canonicalize(cwd).map_err(|source| ProjectError::io("canonicalize", cwd, source))?;
    if let Some((marker, name)) = find_marker(&canonical)? {
      return self.project(&name).map_err(|_| ProjectError::MarkerProjectNotFound { path: marker, project: name });
    }
    let mut candidates: Vec<&Project> = self
      .projects
      .iter()
      .filter(|project| project.repos.iter().any(|repo| canonical.starts_with(&repo.path)))
      .collect();
    candidates.sort_by(|a, b| a.name.cmp(&b.name));
    match candidates.as_slice() {
      [only] => Ok(only),
      [] => Err(ProjectError::NotInProject { cwd: canonical }),
      several => Err(ProjectError::Ambiguous {
        cwd: canonical,
        candidates: several.iter().map(|project| project.name.to_string()).collect(),
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
