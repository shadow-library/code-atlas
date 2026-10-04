use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::{AgentError, AgentResult};

pub const TASK_FILE: &str = "task.toml";
pub const HIDDEN_DIR: &str = "hidden";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawRepoSource", into = "RawRepoSource")]
pub enum RepoSource {
  SelfRepo,
  Git { url: String, commit: String },
  Fixture { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum RawRepoSource {
  Keyword(String),
  Git { git: String, commit: String },
  Fixture { fixture: String },
}

impl TryFrom<RawRepoSource> for RepoSource {
  type Error = String;

  fn try_from(raw: RawRepoSource) -> Result<Self, Self::Error> {
    match raw {
      RawRepoSource::Keyword(keyword) if keyword == "self" => Ok(Self::SelfRepo),
      RawRepoSource::Keyword(other) => Err(format!("repo must be \"self\", {{ git, commit }} or {{ fixture }}, got \"{other}\"")),
      RawRepoSource::Git { git, commit } => Ok(Self::Git { url: git, commit }),
      RawRepoSource::Fixture { fixture } => Ok(Self::Fixture { name: fixture }),
    }
  }
}

impl From<RepoSource> for RawRepoSource {
  fn from(source: RepoSource) -> Self {
    match source {
      RepoSource::SelfRepo => Self::Keyword("self".to_owned()),
      RepoSource::Git { url, commit } => Self::Git { git: url, commit },
      RepoSource::Fixture { name } => Self::Fixture { fixture: name },
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationSpec {
  #[serde(default)]
  pub hidden_tests: Vec<String>,
  #[serde(default)]
  pub commands: Vec<String>,
  #[serde(default)]
  pub regression: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundTruth {
  #[serde(default)]
  pub relevant_files: Vec<String>,
  #[serde(default)]
  pub must_change: Vec<String>,
  #[serde(default)]
  pub must_consider: Vec<String>,
  #[serde(default)]
  pub reuse_targets: Vec<String>,
  #[serde(default)]
  pub forbidden_duplicates: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
  pub id: String,
  pub title: String,
  pub repo: RepoSource,
  #[serde(default)]
  pub base_commit: Option<String>,
  pub prompt: String,
  #[serde(default)]
  pub category: Vec<String>,
  pub timeout_minutes: u64,
  pub max_turns: u32,
  #[serde(default)]
  pub atlas_repos: Vec<String>,
  #[serde(default)]
  pub validation: ValidationSpec,
  #[serde(default)]
  pub ground_truth: GroundTruth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedTask {
  pub spec: TaskSpec,
  pub dir: PathBuf,
  pub spec_hash: String,
}

impl LoadedTask {
  pub fn hidden_file(&self, entry: &str) -> PathBuf {
    self.dir.join(entry)
  }
}

pub fn hidden_destination(entry: &str) -> &str {
  entry.strip_prefix(HIDDEN_DIR).and_then(|rest| rest.strip_prefix('/')).unwrap_or(entry)
}

pub fn parse_task(text: &str, path: &Path) -> AgentResult<TaskSpec> {
  let spec: TaskSpec = toml::from_str(text).map_err(|error| AgentError::InvalidSpec {
    path: path.to_owned(),
    reason: error.to_string(),
  })?;
  validate(&spec).map_err(|reason| AgentError::InvalidSpec { path: path.to_owned(), reason })?;
  Ok(spec)
}

pub fn load_task(dir: &Path) -> AgentResult<LoadedTask> {
  let path = dir.join(TASK_FILE);
  let text = fs::read_to_string(&path).map_err(|error| AgentError::io(&path, error))?;
  let spec = parse_task(&text, &path)?;
  let dir_name = dir.file_name().and_then(|name| name.to_str()).unwrap_or_default();
  if dir_name != spec.id {
    return Err(AgentError::InvalidSpec {
      path,
      reason: format!("id `{}` must equal the directory name `{dir_name}`", spec.id),
    });
  }
  for entry in &spec.validation.hidden_tests {
    let hidden = dir.join(entry);
    if !hidden.is_file() {
      return Err(AgentError::InvalidSpec {
        path,
        reason: format!("hidden test `{entry}` does not exist"),
      });
    }
  }
  Ok(LoadedTask {
    spec,
    dir: dir.to_owned(),
    spec_hash: blake3::hash(text.as_bytes()).to_hex()[..16].to_owned(),
  })
}

pub fn load_tasks(tasks_dir: &Path, only: &[String]) -> AgentResult<Vec<LoadedTask>> {
  let entries = fs::read_dir(tasks_dir).map_err(|error| AgentError::io(tasks_dir, error))?;
  let mut dirs: Vec<PathBuf> = entries
    .filter_map(Result::ok)
    .map(|entry| entry.path())
    .filter(|path| path.join(TASK_FILE).is_file())
    .collect();
  dirs.sort();
  let mut tasks = dirs.iter().map(|dir| load_task(dir)).collect::<AgentResult<Vec<_>>>()?;
  if let Some(unknown) = only.iter().find(|id| !tasks.iter().any(|task| &task.spec.id == *id)) {
    return Err(AgentError::UnknownTask(unknown.clone()));
  }
  if !only.is_empty() {
    tasks.retain(|task| only.contains(&task.spec.id));
  }
  if tasks.is_empty() {
    return Err(AgentError::NoTasks(tasks_dir.to_owned()));
  }
  Ok(tasks)
}

fn validate(spec: &TaskSpec) -> Result<(), String> {
  if !is_slug(&spec.id) {
    return Err(format!("id `{}` must be lowercase letters, digits and dashes", spec.id));
  }
  if spec.prompt.trim().is_empty() {
    return Err("prompt is empty".to_owned());
  }
  if spec.timeout_minutes == 0 || spec.max_turns == 0 {
    return Err("timeout_minutes and max_turns must be positive".to_owned());
  }
  validate_repo(spec)?;
  for entry in &spec.validation.hidden_tests {
    if !entry.starts_with("hidden/") || hidden_destination(entry).is_empty() {
      return Err(format!("hidden test `{entry}` must live under hidden/"));
    }
    if !is_relative_inside(entry) {
      return Err(format!("hidden test `{entry}` must be a relative path without `..`"));
    }
  }
  let truth = &spec.ground_truth;
  let paths = truth.relevant_files.iter().chain(&truth.must_change).chain(&truth.must_consider).chain(&spec.atlas_repos);
  if let Some(bad) = paths.into_iter().find(|path| !is_relative_inside(path)) {
    return Err(format!("path `{bad}` must be relative to the tree root without `..`"));
  }
  Ok(())
}

fn validate_repo(spec: &TaskSpec) -> Result<(), String> {
  match (&spec.repo, &spec.base_commit) {
    (RepoSource::SelfRepo | RepoSource::Fixture { .. }, None) => Err("base_commit is required for self and fixture repos".to_owned()),
    (RepoSource::SelfRepo | RepoSource::Fixture { .. }, Some(commit)) if !is_commit(commit) => Err(format!("base_commit `{commit}` is not a hex commit id")),
    (RepoSource::Fixture { name }, _) if !is_slug(name) => Err(format!("fixture `{name}` must be a plain directory name")),
    (RepoSource::Git { commit, .. }, _) if !is_commit(commit) => Err(format!("commit `{commit}` is not a hex commit id")),
    (RepoSource::Git { commit, .. }, Some(base)) if base != commit => Err(format!("base_commit `{base}` differs from the git source commit `{commit}`")),
    _ => Ok(()),
  }
}

fn is_slug(value: &str) -> bool {
  !value.is_empty() && value.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_commit(value: &str) -> bool {
  (7..=64).contains(&value.len()) && value.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_relative_inside(path: &str) -> bool {
  !path.is_empty() && !path.starts_with('/') && path.split('/').all(|segment| segment != "..")
}

#[cfg(test)]
mod tests {
  use super::*;

  const SPEC: &str = r#"
id = "t00-smoke"
title = "Smoke"
repo = { fixture = "acme" }
base_commit = "9b1fc2d"
prompt = "do it"
category = ["smoke"]
timeout_minutes = 5
max_turns = 30
atlas_repos = ["acme-api"]
[validation]
hidden_tests = ["hidden/acme-api/check.mjs"]
commands = ["node acme-api/check.mjs"]
regression = ["true"]
[ground_truth]
relevant_files = ["acme-api/src/users/user.entity.ts"]
must_change = ["acme-api/src/users/user.entity.ts"]
reuse_targets = ["UsersService.findOne"]
"#;

  fn parse(text: &str) -> AgentResult<TaskSpec> {
    parse_task(text, Path::new("task.toml"))
  }

  #[test]
  fn should_parse_a_fixture_task() {
    let spec = parse(SPEC).unwrap();
    assert_eq!(spec.repo, RepoSource::Fixture { name: "acme".to_owned() });
    assert_eq!(spec.validation.hidden_tests, vec!["hidden/acme-api/check.mjs"]);
    assert_eq!(spec.ground_truth.reuse_targets, vec!["UsersService.findOne"]);
    assert_eq!(spec.atlas_repos, vec!["acme-api"]);
  }

  #[test]
  fn should_parse_self_and_git_sources() {
    let self_spec = parse(&SPEC.replace(r#"repo = { fixture = "acme" }"#, r#"repo = "self""#)).unwrap();
    assert_eq!(self_spec.repo, RepoSource::SelfRepo);
    let git = SPEC
      .replace(r#"repo = { fixture = "acme" }"#, r#"repo = { git = "https://example.test/r.git", commit = "abcdef1" }"#)
      .replace(r#"base_commit = "9b1fc2d""#, "");
    assert_eq!(
      parse(&git).unwrap().repo,
      RepoSource::Git {
        url: "https://example.test/r.git".to_owned(),
        commit: "abcdef1".to_owned()
      }
    );
  }

  #[test]
  fn should_reject_unknown_repo_keywords_and_missing_base_commits() {
    assert!(parse(&SPEC.replace(r#"repo = { fixture = "acme" }"#, r#"repo = "other""#)).is_err());
    assert!(parse(&SPEC.replace(r#"base_commit = "9b1fc2d""#, "")).is_err());
  }

  #[test]
  fn should_reject_hidden_tests_outside_hidden_and_escaping_paths() {
    assert!(parse(&SPEC.replace("hidden/acme-api/check.mjs", "acme-api/check.mjs")).is_err());
    assert!(parse(&SPEC.replace("hidden/acme-api/check.mjs", "hidden/../x.mjs")).is_err());
    assert!(parse(&SPEC.replace(r#"relevant_files = ["acme-api"#, r#"relevant_files = ["../acme-api"#)).is_err());
  }

  #[test]
  fn should_reject_unknown_fields() {
    assert!(parse(&format!("{SPEC}\nsurprise = 1\n")).is_err());
  }

  #[test]
  fn should_strip_the_hidden_prefix_for_the_destination() {
    assert_eq!(hidden_destination("hidden/acme-api/test/a.spec.ts"), "acme-api/test/a.spec.ts");
  }
}
