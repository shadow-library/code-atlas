use std::collections::BTreeSet;
use std::path::Path;

use toml::Value;

use super::env::ConfigEnv;
use super::families::FamilyCatalog;
use super::keys::{Keyed, PROJECT_KEYS, find_spec};
use super::layers::{ConfigEntry, Layered, Origin};
use super::user::UserConfig;
use crate::marker::{MARKER_FILE, RepoFile};
use crate::registry::{DetectSettings, Project, ProjectSettings, RepoConfig, StoredProject};

const ROUTE_FAMILIES_KEY: &str = "detect.route_families";

pub(crate) struct ProjectView {
  pub(crate) project: Project,
  pub(crate) entries: Vec<ConfigEntry>,
  pub(crate) warnings: Vec<String>,
}

fn explicit_settings(stored: &StoredProject) -> Vec<(String, Value)> {
  let built_in = ProjectSettings::default();
  PROJECT_KEYS
    .iter()
    .filter_map(|spec| {
      let value = stored.project.settings.get(spec.name)?;
      let explicit = built_in.get(spec.name).as_ref() != Some(&value) || stored.pinned.contains(spec.name);
      explicit.then(|| (spec.name.to_owned(), value))
    })
    .collect()
}

fn user_defaults(config: &UserConfig) -> Layered<ProjectSettings> {
  let mut settings = Layered::<ProjectSettings>::default();
  for (key, value) in &config.defaults {
    settings.apply(&config.origin(), key, value);
  }
  settings
}

pub(crate) fn inherited_settings(config: &UserConfig) -> ProjectSettings {
  user_defaults(config).value
}

pub(crate) fn default_entries(config: &UserConfig, env: &ConfigEnv) -> (Vec<ConfigEntry>, Vec<String>) {
  let mut settings = user_defaults(config);
  for entry in env.entries::<ProjectSettings>() {
    settings.apply(&entry.origin, entry.key, &entry.value);
  }
  let mut entries = settings.entries();
  entries.push(config.default_exclude_entry());
  (entries, settings.warnings)
}

pub(crate) fn resolve_project(stored: &StoredProject, registry_path: &Path, config: &UserConfig, env: &ConfigEnv) -> ProjectView {
  let mut warnings = Vec::new();
  let files: Vec<Option<RepoFile>> = stored.project.repos.iter().map(|repo| read_repo_file(repo, &mut warnings)).collect();
  let registry = Origin::Registry { path: registry_path.to_owned() };
  let mut settings = user_defaults(config);
  for (key, value) in explicit_settings(stored) {
    settings.apply(&registry, &key, &value);
  }
  for (origin, key, value) in repo_settings(&stored.project.repos, &files, &mut warnings) {
    settings.apply(&origin, &key, &value);
  }
  for entry in env.entries::<ProjectSettings>() {
    settings.apply(&entry.origin, entry.key, &entry.value);
  }
  let mut entries = settings.entries();
  if let Some(entry) = entries.iter_mut().find(|entry| entry.key == ROUTE_FAMILIES_KEY) {
    let enabling = enabling_repos(&stored.project.repos, &files);
    *entry = enable_route_families(&mut settings.value.detect, entry, &enabling, config.families(), &mut warnings);
  }
  let mut repos = Vec::with_capacity(files.len());
  for (repo, file) in stored.project.repos.iter().zip(&files) {
    let (effective, repo_entries) = effective_repo(repo, file.as_ref(), config, &registry);
    repos.push(effective);
    entries.extend(repo_entries);
  }
  warnings.extend(settings.warnings);
  ProjectView {
    project: Project {
      name: stored.project.name.clone(),
      settings: settings.value,
      repos,
    },
    entries,
    warnings,
  }
}

fn enabling_repos<'f>(repos: &[RepoConfig], files: &'f [Option<RepoFile>]) -> Vec<(Origin, &'f [String])> {
  repos
    .iter()
    .zip(files)
    .filter_map(|(repo, file)| Some((repo_origin(repo), file.as_ref()?.route_families.as_slice())))
    .filter(|(_, names)| !names.is_empty())
    .collect()
}

fn enable_route_families(detect: &mut DetectSettings, entry: &ConfigEntry, enabling: &[(Origin, &[String])], catalog: &FamilyCatalog, warnings: &mut Vec<String>) -> ConfigEntry {
  let mut origins: Vec<Origin> = match detect.route_families.is_empty() {
    true => Vec::new(),
    false => entry.origins.clone(),
  };
  let mut names: Vec<String> = detect.route_families.clone();
  for (origin, repo_names) in enabling {
    names.extend(repo_names.iter().cloned());
    origins.push(origin.clone());
  }
  names.sort();
  names.dedup();
  if origins.is_empty() {
    origins.push(Origin::BuiltIn);
  }
  detect.families = names.iter().filter_map(|name| catalog.get(name)).cloned().collect();
  detect.route_families = names;
  for name in detect.undeclared() {
    warnings.push(format!("route family {name} ignored: {} does not exist", catalog.path_of(name).display()));
  }
  ConfigEntry {
    key: entry.key.clone(),
    value: Some(Value::Array(detect.route_families.iter().cloned().map(Value::String).collect())),
    origins,
  }
}

fn repo_origin(repo: &RepoConfig) -> Origin {
  Origin::Repo {
    repo: repo.name.clone(),
    path: repo.path.join(MARKER_FILE),
  }
}

fn read_repo_file(repo: &RepoConfig, warnings: &mut Vec<String>) -> Option<RepoFile> {
  let origin = repo_origin(repo);
  match RepoFile::read(&repo.path.join(MARKER_FILE)) {
    Ok(file) => {
      let file = file?;
      warnings.extend(file.warnings.iter().map(|warning| format!("{origin}: {warning}")));
      Some(file)
    }
    Err(error) => {
      warnings.push(format!("{origin} ignored: {error}"));
      None
    }
  }
}

fn repo_settings(repos: &[RepoConfig], files: &[Option<RepoFile>], warnings: &mut Vec<String>) -> Vec<(Origin, String, Value)> {
  let mut chosen: Vec<(Origin, String, Value)> = Vec::new();
  let mut conflicting = BTreeSet::new();
  for (repo, file) in repos.iter().zip(files) {
    let Some(file) = file else { continue };
    let origin = repo_origin(repo);
    for (key, value) in &file.settings {
      let admitted = find_spec(key).is_some_and(|spec| spec.class.admits(&origin).is_ok());
      match chosen.iter().find(|(_, chosen_key, _)| chosen_key == key) {
        Some((first, _, kept)) if admitted && kept != value => {
          warnings.push(format!("{key} differs between {first} ({kept}) and {origin} ({value}); both ignored"));
          conflicting.insert(key.clone());
        }
        Some(_) if admitted => {}
        _ => chosen.push((origin.clone(), key.clone(), value.clone())),
      }
    }
  }
  chosen.retain(|(_, key, _)| !conflicting.contains(key));
  chosen
}

fn effective_repo(repo: &RepoConfig, file: Option<&RepoFile>, config: &UserConfig, registry: &Origin) -> (RepoConfig, Vec<ConfigEntry>) {
  let origin = repo_origin(repo);
  let unset = RepoFile::default();
  let file = file.unwrap_or(&unset);
  let (include, include_origins) = merged(&[(registry, &repo.include), (&origin, &file.include)]);
  let (exclude, exclude_origins) = merged(&[(registry, &repo.exclude), (&config.origin(), &config.default_exclude), (&origin, &file.exclude)]);
  let (languages, languages_origin) = match (&file.languages, &repo.languages) {
    (Some(languages), _) => (Some(languages.clone()), origin),
    (None, Some(languages)) => (Some(languages.clone()), registry.clone()),
    (None, None) => (None, Origin::BuiltIn),
  };
  let entry = |field: &str, value: Option<Value>, origins: Vec<Origin>| ConfigEntry {
    key: format!("repo.{}.{field}", repo.name),
    value,
    origins,
  };
  let entries = vec![
    entry("path", Some(Value::String(repo.path.to_string_lossy().into_owned())), vec![registry.clone()]),
    entry("include", Some(globs_value(&include)), include_origins),
    entry("exclude", Some(globs_value(&exclude)), exclude_origins),
    entry(
      "languages",
      languages.as_ref().and_then(|languages| Value::try_from(languages).ok()),
      vec![languages_origin],
    ),
  ];
  let effective = RepoConfig {
    include,
    exclude,
    languages,
    ..repo.clone()
  };
  (effective, entries)
}

fn merged(layers: &[(&Origin, &Vec<String>)]) -> (Vec<String>, Vec<Origin>) {
  let mut globs: Vec<String> = Vec::new();
  let mut origins = Vec::new();
  for (origin, values) in layers {
    let added: Vec<String> = values.iter().filter(|glob| !globs.contains(glob)).cloned().collect();
    if added.is_empty() {
      continue;
    }
    globs.extend(added);
    origins.push((*origin).clone());
  }
  if origins.is_empty() {
    origins.push(Origin::BuiltIn);
  }
  (globs, origins)
}

fn globs_value(globs: &[String]) -> Value {
  Value::Array(globs.iter().cloned().map(Value::String).collect())
}
