use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use atlas_core::{Language, ProjectName, RepoName};
use atlas_detect::{DecoratorFamily, check_family_name};
use fd_lock::RwLock;
use serde::{Deserialize, Serialize};

use crate::binding::{Binding, Locator, Resolution};
use crate::config::{ConfigEntry, ConfigEnv, Keyed, McpFlags, PROJECT_KEYS, ProjectView, UserConfig, default_entries, inherited_settings, resolve_mcp, resolve_project};
use crate::document::merge_or_fresh;
use crate::error::ProjectError;
use crate::globs::PathGlobs;
use crate::home::AtlasHome;
use crate::marker::{MARKER_FILE, RepoFile};

const LOCK_FILE: &str = "projects.toml.lock";
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
  pub openai_base_url: Option<String>,
  pub api_key_env: Option<String>,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DetectSettings {
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub route_families: Vec<String>,
  #[serde(skip)]
  pub families: Vec<DecoratorFamily>,
}

impl DetectSettings {
  pub fn validate(&self) -> Result<(), String> {
    self
      .route_families
      .iter()
      .try_for_each(|name| check_family_name(name).map_err(|message| format!("detect.route_families: {message}")))
  }

  pub fn undeclared(&self) -> impl Iterator<Item = &str> {
    self
      .route_families
      .iter()
      .map(String::as_str)
      .filter(|name| !self.families.iter().any(|family| family.name() == *name))
  }

  fn is_default(&self) -> bool {
    self.route_families.is_empty() && self.families.is_empty()
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
  #[serde(skip_serializing_if = "DetectSettings::is_default")]
  pub detect: DetectSettings,
}

impl LlmSettings {
  pub fn validate(&self) -> Result<(), String> {
    if let Some(name) = &self.api_key_env
      && !is_variable_name(name)
    {
      return Err(
        "llm.api_key_env must name an environment variable (letters, digits and underscores, not starting with a digit); the value is not shown in case it is a key".to_owned(),
      );
    }
    let Some(url) = &self.openai_base_url else {
      return Ok(());
    };
    let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) else {
      return Err("llm.openai_base_url must start with http:// or https://; the value is not shown in case it holds a credential".to_owned());
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    match authority.contains('@') {
      true => Err("llm.openai_base_url must not carry credentials (user:password@); name the key variable with llm.api_key_env instead".to_owned()),
      false => Ok(()),
    }
  }
}

fn is_variable_name(name: &str) -> bool {
  let mut characters = name.chars();
  characters.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_') && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

impl ProjectSettings {
  pub fn validate(&self) -> Result<(), String> {
    self.llm.validate()?;
    self.lsp.validate()?;
    self.detect.validate()
  }
}

impl Default for ProjectSettings {
  fn default() -> Self {
    Self {
      llm: LlmSettings::default(),
      scip: ScipSettings::default(),
      lsp: LspSettings::default(),
      max_file_bytes: 1024 * 1024,
      detect: DetectSettings::default(),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsUpdate {
  pub values: ProjectSettings,
  pub explicit: Vec<String>,
  pub reset: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredProject {
  pub(crate) project: Project,
  pub(crate) pinned: BTreeSet<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
  #[serde(default, rename = "project")]
  projects: Vec<ProjectRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectRecord {
  name: ProjectName,
  #[serde(default)]
  settings: SettingsRecord,
  #[serde(default, rename = "repo")]
  repos: Vec<RepoConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct SettingsRecord {
  llm: LlmSettings,
  scip: ScipSettings,
  #[serde(skip_serializing_if = "LspSettings::is_default")]
  lsp: LspSettings,
  max_file_bytes: u64,
  #[serde(skip_serializing_if = "DetectSettings::is_default")]
  detect: DetectSettings,
  #[serde(skip_serializing_if = "BTreeSet::is_empty")]
  pinned: BTreeSet<String>,
}

impl Default for SettingsRecord {
  fn default() -> Self {
    Self::new(ProjectSettings::default(), BTreeSet::new())
  }
}

impl SettingsRecord {
  fn new(settings: ProjectSettings, pinned: BTreeSet<String>) -> Self {
    Self {
      llm: settings.llm,
      scip: settings.scip,
      lsp: settings.lsp,
      max_file_bytes: settings.max_file_bytes,
      detect: settings.detect,
      pinned,
    }
  }
}

impl From<ProjectRecord> for StoredProject {
  fn from(record: ProjectRecord) -> Self {
    let SettingsRecord {
      llm,
      scip,
      lsp,
      max_file_bytes,
      detect,
      pinned,
    } = record.settings;
    Self {
      project: Project {
        name: record.name,
        settings: ProjectSettings {
          llm,
          scip,
          lsp,
          max_file_bytes,
          detect,
        },
        repos: record.repos,
      },
      pinned,
    }
  }
}

impl From<&StoredProject> for ProjectRecord {
  fn from(stored: &StoredProject) -> Self {
    Self {
      name: stored.project.name.clone(),
      settings: SettingsRecord::new(stored.project.settings.clone(), stored.pinned.clone()),
      repos: stored.project.repos.clone(),
    }
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigReport {
  pub entries: Vec<ConfigEntry>,
  pub warnings: Vec<String>,
}

#[derive(Debug)]
pub struct ResolutionTrace {
  pub steps: Vec<String>,
  pub outcome: Result<Resolution, ProjectError>,
}

#[derive(Debug)]
struct Explained {
  entries: Vec<ConfigEntry>,
  warnings: Vec<String>,
}

#[derive(Debug)]
pub struct Registry {
  registry_path: PathBuf,
  config: UserConfig,
  env: ConfigEnv,
  stored: Vec<StoredProject>,
  projects: Vec<Project>,
  explained: Vec<Explained>,
}

impl Registry {
  pub fn load(home: &AtlasHome) -> Result<Self, ProjectError> {
    Self::load_with(home, ConfigEnv::from_process())
  }

  pub fn load_with(home: &AtlasHome, env: ConfigEnv) -> Result<Self, ProjectError> {
    let config = UserConfig::load(home)?;
    let registry_path = home.registry_path();
    let stored = read_projects(&registry_path, read_text(&registry_path)?.as_deref())?;
    let mut registry = Self {
      registry_path,
      config,
      env,
      stored: Vec::new(),
      projects: Vec::new(),
      explained: Vec::new(),
    };
    registry.adopt(stored);
    let enabled = registry
      .projects
      .iter()
      .flat_map(|project| project.settings.detect.route_families.iter().map(String::as_str));
    registry.config.families().check_enabled(enabled)?;
    Ok(registry)
  }

  pub fn list(&self) -> &[Project] {
    &self.projects
  }

  pub fn project(&self, name: &str) -> Result<&Project, ProjectError> {
    self.position(name).map(|index| &self.projects[index])
  }

  pub fn user_config(&self) -> &UserConfig {
    &self.config
  }

  pub fn env(&self) -> &ConfigEnv {
    &self.env
  }

  pub fn stored_settings(&self, name: &str) -> Result<&ProjectSettings, ProjectError> {
    self.position(name).map(|index| &self.stored[index].project.settings)
  }

  pub fn warnings(&self, project: Option<&str>) -> Vec<String> {
    let mut warnings = self.config.warnings().to_vec();
    warnings.extend(self.env.warnings());
    if let Some(index) = project.and_then(|name| self.position(name).ok()) {
      warnings.extend(self.explained[index].warnings.iter().cloned());
    }
    warnings
  }

  pub fn explain(&self, project: Option<&str>) -> Result<ConfigReport, ProjectError> {
    let (mut entries, mut warnings) = match project {
      Some(name) => {
        let explained = &self.explained[self.position(name)?];
        (explained.entries.clone(), self.warnings(Some(name)))
      }
      None => {
        let (entries, warnings) = default_entries(&self.config, &self.env);
        (entries, self.warnings(None).into_iter().chain(warnings).collect())
      }
    };
    let mcp = resolve_mcp(&self.config, &self.env, McpFlags::default());
    entries.extend(mcp.entries);
    entries.extend(self.config.reserved_entries());
    warnings.extend(mcp.warnings);
    warnings.extend(mcp.errors);
    Ok(ConfigReport { entries, warnings })
  }

  pub fn create_project(&mut self, name: &str, repos: Vec<RepoSpec>) -> Result<&Project, ProjectError> {
    let name = ProjectName::new(name)?;
    self.mutate(|projects| {
      if projects.iter().any(|stored| stored.project.name == name) {
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
      projects.push(StoredProject { project, pinned: BTreeSet::new() });
      Ok(())
    })?;
    self.project(name.as_str())
  }

  pub fn delete_project(&mut self, name: &str) -> Result<Project, ProjectError> {
    self.mutate(|projects| {
      let index = projects
        .iter()
        .position(|stored| stored.project.name.as_str() == name)
        .ok_or_else(|| ProjectError::ProjectNotFound(name.to_owned()))?;
      Ok(projects.remove(index).project)
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
    self.update_settings(
      project,
      SettingsUpdate {
        values: settings,
        explicit: Vec::new(),
        reset: false,
      },
    )
  }

  pub fn update_settings(&mut self, project: &str, update: SettingsUpdate) -> Result<(), ProjectError> {
    let invalid = |message: String| ProjectError::InvalidSettings {
      project: project.to_owned(),
      message,
    };
    update.values.validate().map_err(invalid)?;
    if let Some(unknown) = update.explicit.iter().find(|key| !PROJECT_KEYS.iter().any(|spec| spec.name == key.as_str())) {
      return Err(invalid(format!("unknown setting {unknown}")));
    }
    let built_in = ProjectSettings::default();
    let inherited = inherited_settings(&self.config);
    self.mutate(|projects| {
      let target = projects
        .iter_mut()
        .find(|stored| stored.project.name.as_str() == project)
        .ok_or_else(|| ProjectError::ProjectNotFound(project.to_owned()))?;
      if update.reset {
        target.pinned.clear();
      }
      for key in &update.explicit {
        let value = update.values.get(key);
        match value.is_some() && value == built_in.get(key) && inherited.get(key) != value {
          true => target.pinned.insert(key.clone()),
          false => target.pinned.remove(key),
        };
      }
      target.pinned.retain(|key| update.values.get(key) == built_in.get(key));
      target.project.settings = update.values;
      Ok(())
    })
  }

  pub fn resolve_project(&self, cwd: &Path, explicit: Option<&str>) -> Result<&Project, ProjectError> {
    self.locate(cwd, explicit, &mut Lookup::default()).map(|(project, _)| project)
  }

  pub fn resolve(&self, cwd: &Path, explicit: Option<&str>) -> Result<Resolution, ProjectError> {
    let mut lookup = Lookup::default();
    let (project, binding) = self.locate(cwd, explicit, &mut lookup)?;
    Ok(Resolution {
      project: project.clone(),
      binding,
      warnings: lookup.warnings,
    })
  }

  pub fn trace(&self, cwd: &Path, explicit: Option<&str>) -> ResolutionTrace {
    let mut lookup = Lookup::default();
    let outcome = self.locate(cwd, explicit, &mut lookup).map(|(project, binding)| Resolution {
      project: project.clone(),
      binding,
      warnings: lookup.warnings.clone(),
    });
    let mut steps = lookup.steps;
    match &outcome {
      Ok(resolution) => steps.push(format!("resolved project {}{}", resolution.project.name, binding_note(&resolution.binding))),
      Err(error) => steps.push(format!("no project: {error}")),
    }
    ResolutionTrace { steps, outcome }
  }

  pub(crate) fn project_on_paths(&self, base: &str, name: ProjectName, paths: &[(RepoName, PathBuf)]) -> Result<(Project, Vec<String>), ProjectError> {
    let mut stored = self.stored[self.position(base)?].clone();
    stored.project.name = name;
    for repo in &mut stored.project.repos {
      if let Some((_, path)) = paths.iter().find(|(name, _)| *name == repo.name) {
        repo.path = path.clone();
      }
    }
    let view = resolve_project(&stored, &self.registry_path, &self.config, &self.env);
    Ok((view.project, view.warnings))
  }

  fn is_repo_root(&self, directory: &Path) -> bool {
    self.stored.iter().flat_map(|stored| &stored.project.repos).any(|repo| repo.path == directory)
  }

  fn find_marker(&self, canonical_cwd: &Path, lookup: &mut Lookup) -> Result<Option<(PathBuf, String)>, ProjectError> {
    for directory in canonical_cwd.ancestors() {
      let marker = directory.join(MARKER_FILE);
      let Some(file) = RepoFile::read(&marker)? else { continue };
      if !self.is_repo_root(directory) {
        lookup.warn_outside_repo_root(&marker, &file);
      }
      match file.project {
        Some(project) => {
          lookup.steps.push(format!("{} pins project {project}", marker.display()));
          return Ok(Some((marker, project)));
        }
        None => lookup.steps.push(format!("{} has no `project` key, so it pins nothing", marker.display())),
      }
    }
    lookup.steps.push("no .code-atlas.toml pins a project; matching registered repo paths".to_owned());
    Ok(None)
  }

  fn locate(&self, cwd: &Path, explicit: Option<&str>, lookup: &mut Lookup) -> Result<(&Project, Binding), ProjectError> {
    if let Some(name) = explicit {
      lookup.steps.push(format!("--project {name} given explicitly"));
      let project = self.project(name)?;
      let binding = fs::canonicalize(cwd).ok().map(Locator::new).and_then(|locator| locator.bind(project));
      return Ok((project, binding.unwrap_or(Binding::Direct)));
    }
    let canonical = fs::canonicalize(cwd).map_err(|source| ProjectError::io("canonicalize", cwd, source))?;
    lookup.steps.push(format!("working directory {}", canonical.display()));
    let locator = Locator::new(canonical);
    if let Some((marker, name)) = self.find_marker(&locator.cwd, lookup)? {
      let project = self.project(&name).map_err(|_| ProjectError::MarkerProjectNotFound { path: marker, project: name })?;
      return Ok((project, locator.bind(project).unwrap_or(Binding::Direct)));
    }
    let bound: Vec<(&Project, Binding)> = self.projects.iter().filter_map(|project| locator.bind(project).map(|binding| (project, binding))).collect();
    for (project, binding) in &bound {
      lookup.steps.push(format!("project {} contains it{}", project.name, binding_note(binding)));
    }
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

  fn position(&self, name: &str) -> Result<usize, ProjectError> {
    self
      .projects
      .iter()
      .position(|project| project.name.as_str() == name)
      .ok_or_else(|| ProjectError::ProjectNotFound(name.to_owned()))
  }

  fn adopt(&mut self, stored: Vec<StoredProject>) {
    let views: Vec<ProjectView> = stored
      .iter()
      .map(|project| resolve_project(project, &self.registry_path, &self.config, &self.env))
      .collect();
    self.projects = views.iter().map(|view| view.project.clone()).collect();
    self.explained = views
      .into_iter()
      .map(|view| Explained {
        entries: view.entries,
        warnings: view.warnings,
      })
      .collect();
    self.stored = stored;
  }

  fn mutate<T>(&mut self, apply: impl FnOnce(&mut Vec<StoredProject>) -> Result<T, ProjectError>) -> Result<T, ProjectError> {
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
    let text = read_text(&self.registry_path)?;
    let before = read_projects(&self.registry_path, text.as_deref())?;
    let mut projects = before.clone();
    let outcome = apply(&mut projects)?;
    let rendered = render_projects(&self.registry_path, text.as_deref(), &before, &projects)?;
    write_projects(&self.registry_path, &rendered)?;
    self.adopt(projects);
    Ok(outcome)
  }
}

fn binding_note(binding: &Binding) -> String {
  match binding {
    Binding::Direct => String::new(),
    Binding::Worktree(worktree) => format!(" through the git worktree at {} (repo {})", worktree.worktree.display(), worktree.repo),
    Binding::UnresolvedWorktree(unresolved) => format!(" through an unresolvable .git entry at {} (repo {})", unresolved.root.display(), unresolved.repo),
    Binding::Overlay(overlay) => format!(" through the worktree overlay {} at {}", overlay.label, overlay.worktree.display()),
  }
}

fn find_project_mut<'a>(projects: &'a mut [StoredProject], name: &str) -> Result<&'a mut Project, ProjectError> {
  projects
    .iter_mut()
    .map(|stored| &mut stored.project)
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

#[derive(Default)]
struct Lookup {
  steps: Vec<String>,
  warnings: Vec<String>,
}

impl Lookup {
  fn warn_outside_repo_root(&mut self, marker: &Path, file: &RepoFile) {
    let mut warnings: Vec<String> = file.warnings.iter().map(|warning| format!("{}: {warning}", marker.display())).collect();
    let configures = !file.include.is_empty() || !file.exclude.is_empty() || file.languages.is_some() || !file.route_families.is_empty() || !file.settings.is_empty();
    if configures {
      warnings.push(format!(
        "{}: only `project` is read here; settings and [index] keys apply only in the .code-atlas.toml at a registered repo root",
        marker.display()
      ));
    }
    self.steps.extend(warnings.iter().map(|warning| format!("warning: {warning}")));
    self.warnings.extend(warnings);
  }
}

fn read_text(path: &Path) -> Result<Option<String>, ProjectError> {
  match fs::read_to_string(path) {
    Ok(text) => Ok(Some(text)),
    Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
    Err(source) => Err(ProjectError::io("read", path, source)),
  }
}

fn read_projects(path: &Path, text: Option<&str>) -> Result<Vec<StoredProject>, ProjectError> {
  let Some(text) = text else { return Ok(Vec::new()) };
  let file: RegistryFile = toml::from_str(text).map_err(|error| ProjectError::RegistryParse {
    path: path.to_owned(),
    message: atlas_detect::toml_error_summary(text, &error),
  })?;
  let projects: Vec<StoredProject> = file.projects.into_iter().map(StoredProject::from).collect();
  validate_loaded(path, &projects)?;
  Ok(projects)
}

fn validate_loaded(path: &Path, projects: &[StoredProject]) -> Result<(), ProjectError> {
  let invalid = |message: String| ProjectError::RegistryParse { path: path.to_owned(), message };
  for (index, stored) in projects.iter().enumerate() {
    let project = &stored.project;
    if let Err(message) = project.settings.validate() {
      return Err(invalid(format!("project {:?}: {message}", project.name.as_str())));
    }
    if let Some(unknown) = stored.pinned.iter().find(|key| !PROJECT_KEYS.iter().any(|spec| spec.name == key.as_str())) {
      return Err(invalid(format!("project {:?}: pinned names unknown setting {unknown:?}", project.name.as_str())));
    }
    if projects[..index].iter().any(|earlier| earlier.project.name == project.name) {
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

fn render_projects(path: &Path, previous: Option<&str>, before: &[StoredProject], after: &[StoredProject]) -> Result<String, ProjectError> {
  let file = RegistryFileRef {
    projects: after.iter().map(ProjectRecord::from).collect(),
  };
  let fresh = toml::to_string_pretty(&file).map_err(|error| ProjectError::RegistrySerialize(error.to_string()))?;
  let Some(previous) = previous.filter(|text| !text.trim().is_empty()) else {
    return Ok(fresh);
  };
  let unchanged = |index: usize| before.get(index).is_some_and(|old| after.iter().any(|new| new == old));
  let reproduces = |merged: &str| read_projects(path, Some(merged)).is_ok_and(|parsed| parsed == after);
  Ok(merge_or_fresh(previous, &fresh, unchanged, reproduces))
}

fn write_projects(path: &Path, text: &str) -> Result<(), ProjectError> {
  let directory = path.parent().unwrap_or(Path::new("."));
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
struct RegistryFileRef {
  #[serde(rename = "project")]
  projects: Vec<ProjectRecord>,
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
      Registry::load_with(&self.home, ConfigEnv::default()).unwrap()
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
        ..LlmSettings::default()
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
      detect: DetectSettings::default(),
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
  fn should_ignore_unknown_marker_keys_but_reject_broken_syntax() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("one");
    let registry = registry_with(&sandbox, &[("acme", &[&dir]), ("globex", &[])]);
    fs::write(dir.join(MARKER_FILE), "projekt = \"globex\"\n[[detect.routes]]\npack = \"x\"\n").unwrap();
    assert_eq!(registry.resolve_project(&dir, None).unwrap().name.as_str(), "acme");
    fs::write(dir.join(MARKER_FILE), "project = \n").unwrap();
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

  fn write_config(sandbox: &Sandbox, text: &str) {
    fs::create_dir_all(sandbox.home.config_dir()).unwrap();
    fs::write(sandbox.home.config_path(), text).unwrap();
  }

  fn origin_of(registry: &Registry, project: &str, key: &str) -> Vec<String> {
    let report = registry.explain(Some(project)).unwrap();
    let entry = report.entries.iter().find(|entry| entry.key == key).unwrap_or_else(|| panic!("no entry {key}"));
    entry.origins.iter().map(ToString::to_string).collect()
  }

  #[test]
  fn should_resolve_every_project_exactly_as_stored_without_config_toml_or_repo_files() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("one");
    let mut spec = RepoSpec::new(&dir);
    spec.exclude = vec!["dist/".to_owned(), "dist/".to_owned()];
    spec.languages = Some(vec![Language::Rust]);
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![spec]).unwrap();
    let mut settings = ProjectSettings::default();
    settings.scip.enabled = true;
    registry.set_settings("acme", settings.clone()).unwrap();

    let reloaded = sandbox.registry();
    let project = reloaded.project("acme").unwrap();

    assert_eq!(project.settings, settings);
    assert_eq!(project.repos[0].exclude, ["dist/", "dist/"]);
    assert_eq!(project.repos[0].languages, Some(vec![Language::Rust]));
    assert!(reloaded.warnings(Some("acme")).is_empty());
  }

  #[test]
  fn should_layer_config_defaults_under_sparse_project_settings_and_the_repo_layer_above_them() {
    let sandbox = Sandbox::new();
    let one = sandbox.repo_dir("one");
    let mut registry = sandbox.registry();
    let mut spec = RepoSpec::new(&one);
    spec.exclude = vec!["dist/".to_owned()];
    registry.create_project("acme", vec![spec]).unwrap();
    let mut settings = ProjectSettings::default();
    settings.lsp.max_servers = 5;
    settings.max_file_bytes = 4096;
    registry.set_settings("acme", settings).unwrap();
    write_config(
      &sandbox,
      "version = 1\n[defaults]\nmax_file_bytes = 2048\nexclude = [\"**/*.gen.ts\", \"dist/\"]\n[defaults.lsp]\nmax_servers = 3\nenabled = false\n[defaults.scip]\nenabled = true\n",
    );
    fs::write(one.join(MARKER_FILE), "project = \"acme\"\nmax_file_bytes = 8192\n[index]\nexclude = [\"legacy/**\"]\n").unwrap();
    let env = ConfigEnv::from_pairs([("CODE_ATLAS_SCIP_ENABLED", "false"), ("CODE_ATLAS_MAX_FILE_BYTES", "1")]);

    let registry = Registry::load_with(&sandbox.home, env).unwrap();
    let project = registry.project("acme").unwrap();

    assert_eq!(
      (
        project.settings.lsp.enabled,
        project.settings.lsp.max_servers,
        project.settings.scip.enabled,
        project.settings.max_file_bytes
      ),
      (false, 5, false, 8192)
    );
    assert_eq!(project.repos[0].exclude, ["dist/", "**/*.gen.ts", "legacy/**"]);
    assert_eq!(origin_of(&registry, "acme", "lsp.enabled"), ["config.toml"]);
    assert_eq!(origin_of(&registry, "acme", "lsp.max_servers"), ["projects.toml"]);
    assert_eq!(origin_of(&registry, "acme", "scip.enabled"), ["env:CODE_ATLAS_SCIP_ENABLED"]);
    assert_eq!(origin_of(&registry, "acme", "max_file_bytes"), ["one/.code-atlas.toml"]);
    assert_eq!(origin_of(&registry, "acme", "lsp.idle_secs"), ["built-in"]);
    assert_eq!(origin_of(&registry, "acme", "repo.one.exclude"), ["projects.toml", "config.toml", "one/.code-atlas.toml"]);
    assert_eq!(
      registry.warnings(Some("acme")),
      ["env:CODE_ATLAS_MAX_FILE_BYTES: max_file_bytes ignored: it changes indexed facts, so only config.toml, projects.toml or a repository's .code-atlas.toml may set it"]
    );
  }

  #[test]
  fn should_let_a_global_default_reach_a_project_that_stored_the_old_built_in_defaults() {
    let sandbox = Sandbox::new();
    sandbox.registry().create_project("acme", vec![]).unwrap();
    let stored = fs::read_to_string(sandbox.home.registry_path()).unwrap();
    assert!(stored.contains("max_file_bytes = 1048576") && stored.contains("debounce_secs = 30"), "{stored}");
    write_config(&sandbox, "[defaults]\nmax_file_bytes = 2048\n[defaults.scip]\ndebounce_secs = 5\n");

    let registry = sandbox.registry();

    assert_eq!(registry.project("acme").unwrap().settings.max_file_bytes, 2048);
    assert_eq!(registry.project("acme").unwrap().settings.scip.debounce_secs, 5);
    assert_eq!(origin_of(&registry, "acme", "scip.debounce_secs"), ["config.toml"]);
  }

  #[test]
  fn should_pin_an_explicit_built_in_value_only_when_a_global_default_would_override_it() {
    let sandbox = Sandbox::new();
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![]).unwrap();
    let pristine = fs::read_to_string(sandbox.home.registry_path()).unwrap();
    let explicit_scip_off = |registry: &mut Registry| {
      let values = registry.stored_settings("acme").unwrap().clone();
      let update = SettingsUpdate {
        values,
        explicit: vec!["scip.enabled".to_owned()],
        reset: false,
      };
      registry.update_settings("acme", update).unwrap();
    };

    explicit_scip_off(&mut registry);
    assert_eq!(fs::read_to_string(sandbox.home.registry_path()).unwrap(), pristine);

    write_config(&sandbox, "[defaults.scip]\nenabled = true\n");
    let mut registry = sandbox.registry();
    assert!(registry.project("acme").unwrap().settings.scip.enabled);
    explicit_scip_off(&mut registry);

    let written = fs::read_to_string(sandbox.home.registry_path()).unwrap();
    assert!(written.contains("pinned = [\"scip.enabled\"]"), "{written}");
    let reloaded = sandbox.registry();
    assert!(!reloaded.project("acme").unwrap().settings.scip.enabled);
    assert_eq!(origin_of(&reloaded, "acme", "scip.enabled"), ["projects.toml"]);

    let mut reloaded = reloaded;
    let reset = SettingsUpdate {
      values: ProjectSettings::default(),
      explicit: Vec::new(),
      reset: true,
    };
    reloaded.update_settings("acme", reset).unwrap();
    assert_eq!(fs::read_to_string(sandbox.home.registry_path()).unwrap(), pristine);
  }

  #[test]
  fn should_reject_an_unknown_pinned_setting() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.home.config_dir()).unwrap();
    fs::write(sandbox.home.registry_path(), "[[project]]\nname = \"a\"\n[project.settings]\npinned = [\"scip.enabld\"]\n").unwrap();
    let error = Registry::load_with(&sandbox.home, ConfigEnv::default()).unwrap_err();
    assert!(error.to_string().ends_with("project \"a\": pinned names unknown setting \"scip.enabld\""), "{error}");
  }

  #[test]
  fn should_keep_comments_and_untouched_projects_when_writing_the_registry() {
    let sandbox = Sandbox::new();
    let one = sandbox.repo_dir("one");
    fs::create_dir_all(sandbox.home.config_dir()).unwrap();
    let hand_written = format!(
      "# Code Atlas projects, edited by hand\n\n[[project]]\nname = \"acme\" # the main one\n\n[project.settings]\nmax_file_bytes = 1048576 # keep small\n\n[[project.repo]]\nname = \"one\"\npath = \"{}\" # checkout\n\n[[project]]\nname = \"sparse\"\n# no settings on purpose\n",
      one.display()
    );
    fs::write(sandbox.home.registry_path(), &hand_written).unwrap();
    let mut registry = sandbox.registry();

    let mut settings = registry.stored_settings("acme").unwrap().clone();
    settings.max_file_bytes = 42;
    registry.set_settings("acme", settings).unwrap();
    registry.rename_repo("acme", "one", "primary").unwrap();
    registry.create_project("globex", vec![]).unwrap();

    let written = fs::read_to_string(sandbox.home.registry_path()).unwrap();
    for kept in [
      "# Code Atlas projects, edited by hand\n",
      "name = \"acme\" # the main one\n",
      "max_file_bytes = 42 # keep small\n",
      "name = \"primary\"\n",
      "\" # checkout\n",
      "[[project]]\nname = \"sparse\"\n\n[[project]]\nname = \"globex\"\n",
      "# no settings on purpose\n",
    ] {
      assert!(written.contains(kept), "missing {kept:?} in\n{written}");
    }
    let reloaded = sandbox.registry();
    let names: Vec<&str> = reloaded.list().iter().map(|project| project.name.as_str()).collect();
    assert_eq!(names, ["acme", "sparse", "globex"]);
    assert_eq!(reloaded.project("acme").unwrap().settings.max_file_bytes, 42);
  }

  #[test]
  fn should_keep_crlf_line_endings_when_writing_the_registry() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.home.config_dir()).unwrap();
    fs::write(sandbox.home.registry_path(), "# windows\r\n[[project]]\r\nname = \"acme\"\r\nrepo = []\r\n").unwrap();

    sandbox.registry().create_project("globex", vec![]).unwrap();

    let written = fs::read_to_string(sandbox.home.registry_path()).unwrap();
    assert!(written.starts_with("# windows\r\n[[project]]\r\nname = \"acme\"\r\n"), "{written:?}");
    assert_eq!(written.matches('\n').count(), written.matches("\r\n").count(), "{written:?}");
    assert_eq!(sandbox.registry().list().len(), 2);
  }

  #[test]
  fn should_ignore_a_repo_layer_value_that_two_repos_disagree_on() {
    let sandbox = Sandbox::new();
    let one = sandbox.repo_dir("one");
    let two = sandbox.repo_dir("two");
    let registry = registry_with(&sandbox, &[("acme", &[&one, &two])]);
    drop(registry);
    fs::write(one.join(MARKER_FILE), "max_file_bytes = 10\n").unwrap();
    fs::write(two.join(MARKER_FILE), "max_file_bytes = 20\n[lsp]\ntypescript_command = [\"/tmp/evil\"]\n").unwrap();

    let registry = sandbox.registry();

    assert_eq!(registry.project("acme").unwrap().settings.max_file_bytes, 1024 * 1024);
    assert_eq!(registry.project("acme").unwrap().settings.lsp.typescript_command, None);
    assert_eq!(
      registry.warnings(Some("acme")),
      [
        "max_file_bytes differs between repo-0/.code-atlas.toml (10) and repo-1/.code-atlas.toml (20); both ignored",
        "repo-1/.code-atlas.toml: lsp.typescript_command ignored: a repository file may not name an executable",
      ]
    );
  }

  const ACME_FAMILY: &str = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n";

  fn write_family(sandbox: &Sandbox, name: &str, text: &str) {
    let dir = sandbox.home.detectors_dir();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.toml")), text).unwrap();
  }

  #[test]
  fn should_enable_route_families_from_the_user_and_repo_layers_but_never_the_environment() {
    let sandbox = Sandbox::new();
    let one = sandbox.repo_dir("one");
    let two = sandbox.repo_dir("two");
    drop(registry_with(&sandbox, &[("acme", &[&one, &two])]));
    write_family(&sandbox, "acme-http", ACME_FAMILY);
    write_family(&sandbox, "acme-rpc", ACME_FAMILY);
    write_config(&sandbox, "[defaults.detect]\nroute_families = [\"acme-rpc\"]\n");
    fs::write(two.join(MARKER_FILE), "[detect]\nroute_families = [\"acme-http\", \"acme-htp\"]\n").unwrap();
    let env = ConfigEnv::from_pairs([("CODE_ATLAS_DETECT_ROUTE_FAMILIES", "[\"acme-env\"]")]);

    let registry = Registry::load_with(&sandbox.home, env).unwrap();
    let detect = &registry.project("acme").unwrap().settings.detect;

    assert_eq!(detect.route_families, ["acme-htp", "acme-http", "acme-rpc"]);
    assert_eq!(detect.families.iter().map(DecoratorFamily::name).collect::<Vec<_>>(), ["acme-http", "acme-rpc"]);
    assert_eq!(detect.undeclared().collect::<Vec<_>>(), ["acme-htp"]);
    assert_eq!(origin_of(&registry, "acme", "detect.route_families"), ["config.toml", "repo-1/.code-atlas.toml"]);
    assert_eq!(
      registry.warnings(Some("acme")),
      [
        "env:CODE_ATLAS_DETECT_ROUTE_FAMILIES: detect.route_families ignored: it changes indexed facts, so only config.toml, projects.toml or a repository's .code-atlas.toml may set it".to_owned(),
        format!("route family acme-htp ignored: {} does not exist", sandbox.home.detectors_dir().join("acme-htp.toml").display()),
      ]
    );
    assert!(!fs::read_to_string(sandbox.home.registry_path()).unwrap().contains("detect"));
  }

  #[test]
  fn should_store_project_route_families_in_the_registry_and_reread_them() {
    let sandbox = Sandbox::new();
    write_family(&sandbox, "acme-http", ACME_FAMILY);
    let mut registry = sandbox.registry();
    registry.create_project("acme", vec![]).unwrap();
    let mut settings = ProjectSettings::default();
    settings.detect.route_families = vec!["acme-http".to_owned()];
    registry.set_settings("acme", settings).unwrap();

    let stored = fs::read_to_string(sandbox.home.registry_path()).unwrap();
    let reloaded = sandbox.registry();
    let detect = &reloaded.project("acme").unwrap().settings.detect;

    assert!(stored.contains("[project.settings.detect]\nroute_families = [\"acme-http\"]"), "{stored}");
    assert_eq!(detect.families.iter().map(DecoratorFamily::name).collect::<Vec<_>>(), ["acme-http"]);
    assert_eq!(origin_of(&reloaded, "acme", "detect.route_families"), ["projects.toml"]);
    assert!(reloaded.warnings(Some("acme")).is_empty());
  }

  #[test]
  fn should_refuse_to_load_only_when_a_project_enables_an_invalid_family() {
    let sandbox = Sandbox::new();
    let one = sandbox.repo_dir("one");
    drop(registry_with(&sandbox, &[("acme", &[&one])]));
    write_family(&sandbox, "acme-http", "packages = [\"@acme/http\"]\n");
    write_family(&sandbox, "Acme", ACME_FAMILY);

    let unused = Registry::load_with(&sandbox.home, ConfigEnv::default()).unwrap();
    assert_eq!(
      unused.warnings(Some("acme")),
      [
        "detectors/Acme.toml ignored: file name: a route family name is a lowercase letter followed by lowercase letters, digits, - or _ (at most 64 characters), got \"Acme\"",
        "detectors/acme-http.toml ignored: controller: missing; name the controller decorators",
      ]
    );
    fs::write(one.join(MARKER_FILE), "[detect]\nroute_families = [\"acme-http\"]\n").unwrap();

    let error = Registry::load_with(&sandbox.home, ConfigEnv::default()).unwrap_err();

    assert_eq!(
      error.to_string(),
      format!(
        "invalid config file {}: controller: missing; name the controller decorators",
        sandbox.home.detectors_dir().join("acme-http.toml").display()
      )
    );
  }

  #[test]
  fn should_trace_how_the_project_was_chosen() {
    let sandbox = Sandbox::new();
    let dir = sandbox.repo_dir("one");
    let nested = dir.join("src");
    fs::create_dir_all(&nested).unwrap();
    let registry = registry_with(&sandbox, &[("acme", &[&dir])]);
    fs::write(nested.join(MARKER_FILE), "[index]\nexclude = [\"x\"]\n").unwrap();

    let trace = registry.trace(&nested, None);

    assert_eq!(
      trace.steps,
      [
        format!("working directory {}", nested.display()),
        format!(
          "warning: {}: only `project` is read here; settings and [index] keys apply only in the .code-atlas.toml at a registered repo root",
          nested.join(MARKER_FILE).display()
        ),
        format!("{} has no `project` key, so it pins nothing", nested.join(MARKER_FILE).display()),
        "no .code-atlas.toml pins a project; matching registered repo paths".to_owned(),
        "project acme contains it".to_owned(),
        "resolved project acme".to_owned(),
      ]
    );
    assert_eq!(registry.resolve(&nested, None).unwrap().warnings.len(), 1);
    fs::write(dir.join(MARKER_FILE), "project = \"acme\"\nfuture = 1\n").unwrap();
    assert!(registry.resolve(&dir, None).unwrap().warnings.is_empty());
    let pinned = registry.trace(&nested, Some("acme"));
    assert_eq!(pinned.steps, ["--project acme given explicitly", "resolved project acme"]);
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
