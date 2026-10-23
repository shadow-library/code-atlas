use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::RepoName;
use serde::{Deserialize, Serialize};

use super::error::{AgentError, AgentResult, LinkedSpecError};
use super::tree_edit::SectionRef;

pub const TASK_FILE: &str = "task.toml";
pub const HIDDEN_DIR: &str = "hidden";
pub const DEFAULT_LINKED_REPO: &str = "linked";
pub const SESSION_REPO: &str = "tree";
const LINKED_PATCH_DOMAIN: &[u8] = b"\0linked.patch\0";

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
pub struct LinkedSpec {
  pub paths: Vec<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub patch: Option<String>,
  #[serde(default = "default_linked_repo")]
  pub repo: String,
}

fn default_linked_repo() -> String {
  DEFAULT_LINKED_REPO.to_owned()
}

impl LinkedSpec {
  pub fn contains(&self, path: &str) -> bool {
    self.paths.iter().any(|linked| is_at_or_below(path, linked))
  }

  pub fn include_globs(&self) -> Vec<String> {
    self.paths.iter().map(|path| format!("{}/**", escape_glob(path))).collect()
  }

  pub fn exclude_globs(&self) -> Vec<String> {
    self.paths.iter().map(|path| anchored_glob(path)).collect()
  }

  pub fn is_linked_ancestor(&self, dir: &str) -> bool {
    self.paths.iter().any(|path| path.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/')))
  }
}

pub fn anchored_glob(path: &str) -> String {
  format!("/{}", escape_glob(path))
}

fn escape_glob(path: &str) -> String {
  let mut glob = String::new();
  for c in path.chars() {
    if matches!(c, '\\' | '*' | '?' | '[' | ']' | '{' | '}' | ' ') {
      glob.push('\\');
    }
    glob.push(c);
  }
  glob
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameSpec {
  pub symbol: String,
  pub new_name: String,
}

impl RenameSpec {
  pub fn old_name(&self) -> &str {
    self.symbol.rsplit(['.', ':']).next().unwrap_or(&self.symbol)
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptLeak {
  Handed,
  Guided,
  Open,
}

impl PromptLeak {
  pub fn label(self) -> &'static str {
    match self {
      Self::Handed => "handed",
      Self::Guided => "guided",
      Self::Open => "open",
    }
  }
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
  pub base_patch: Option<String>,
  #[serde(default)]
  pub setup: Vec<String>,
  #[serde(default)]
  pub validation: ValidationSpec,
  #[serde(default)]
  pub ground_truth: GroundTruth,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub rename: Option<RenameSpec>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub prompt_leak: Option<PromptLeak>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub strip: Vec<String>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub drop_instruction_sections: Vec<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub atlas_config: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub linked: Option<LinkedSpec>,
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

  pub fn base_patch_file(&self) -> Option<PathBuf> {
    self.spec.base_patch.as_ref().map(|patch| self.dir.join(patch))
  }

  pub fn atlas_config_dir(&self) -> Option<PathBuf> {
    self.spec.atlas_config.as_ref().map(|dir| self.dir.join(dir))
  }

  pub fn linked_patch_file(&self) -> Option<PathBuf> {
    self.spec.linked.as_ref()?.patch.as_ref().map(|patch| self.dir.join(patch))
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
  if let Some(linked) = &spec.linked {
    validate_linked(&spec, linked).map_err(|source| AgentError::InvalidLinked { path: path.to_owned(), source })?;
  }
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
  let mut hasher = blake3::Hasher::new();
  hasher.update(text.as_bytes());
  if let Some(patch) = &spec.base_patch {
    let patch_path = dir.join(patch);
    if !patch_path.is_file() {
      return Err(AgentError::InvalidSpec {
        path,
        reason: format!("base patch `{patch}` does not exist"),
      });
    }
    hasher.update(&fs::read(&patch_path).map_err(|error| AgentError::io(&patch_path, error))?);
  }
  if let Some((linked, patch)) = spec.linked.as_ref().and_then(|linked| Some((linked, linked.patch.as_deref()?))) {
    let bytes = read_linked_patch(dir, &path, linked, patch)?;
    hasher.update(LINKED_PATCH_DOMAIN);
    hasher.update(&bytes);
  }
  Ok(LoadedTask {
    spec,
    dir: dir.to_owned(),
    spec_hash: hasher.finalize().to_hex()[..16].to_owned(),
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
  if let Some(patch) = spec.base_patch.as_deref().filter(|patch| !is_relative_inside(patch) || patch.starts_with("hidden/")) {
    return Err(format!("base patch `{patch}` must be a relative path inside the task directory, outside hidden/"));
  }
  if let Some(dir) = spec.atlas_config.as_deref().filter(|dir| !is_relative_inside(dir) || dir.starts_with("hidden/")) {
    return Err(format!("atlas_config `{dir}` must be a relative path inside the task directory, outside hidden/"));
  }
  if spec.setup.iter().any(|command| command.trim().is_empty()) {
    return Err("setup commands must not be empty".to_owned());
  }
  if let Some(rename) = &spec.rename {
    validate_rename(rename)?;
  }
  let sections = spec
    .drop_instruction_sections
    .iter()
    .map(|entry| SectionRef::parse(entry).filter(|section| is_relative_inside(section.file)).ok_or(entry));
  if let Some(bad) = sections.filter_map(Result::err).next() {
    return Err(format!("drop_instruction_sections entry `{bad}` must be `<relative file>#<heading>`"));
  }
  if let Some(bad) = spec.strip.iter().find(|path| path.split('/').any(|segment| segment.is_empty() || segment == ".")) {
    return Err(format!("strip path `{bad}` must name a file or directory below the tree root"));
  }
  let truth = &spec.ground_truth;
  let paths = truth
    .relevant_files
    .iter()
    .chain(&truth.must_change)
    .chain(&truth.must_consider)
    .chain(&spec.atlas_repos)
    .chain(&spec.strip);
  if let Some(bad) = paths.into_iter().find(|path| !is_relative_inside(path)) {
    return Err(format!("path `{bad}` must be relative to the tree root without `..`"));
  }
  Ok(())
}

fn validate_linked(spec: &TaskSpec, linked: &LinkedSpec) -> Result<(), LinkedSpecError> {
  if linked.paths.is_empty() {
    return Err(LinkedSpecError::NoPaths);
  }
  for path in &linked.paths {
    if path.split('/').all(|segment| segment.is_empty() || segment == ".") {
      return Err(LinkedSpecError::TreeRoot(path.clone()));
    }
    if path.starts_with('/') || path.split('/').any(|segment| segment.is_empty() || segment == "." || segment == "..") {
      return Err(LinkedSpecError::InvalidPath(path.clone()));
    }
  }
  let mut seen = BTreeSet::new();
  if let Some(duplicate) = linked.paths.iter().find(|path| !seen.insert(path.as_str())) {
    return Err(LinkedSpecError::Duplicate(duplicate.clone()));
  }
  for outer in &linked.paths {
    if let Some(inner) = linked.paths.iter().find(|inner| *inner != outer && is_at_or_below(inner, outer)) {
      return Err(LinkedSpecError::Nested {
        outer: outer.clone(),
        inner: inner.clone(),
      });
    }
    if let Some(strip) = spec.strip.iter().find(|strip| is_at_or_below(outer, strip)) {
      return Err(LinkedSpecError::InsideStrip {
        path: outer.clone(),
        strip: strip.clone(),
      });
    }
  }
  if !spec.atlas_repos.is_empty() {
    return Err(LinkedSpecError::WithAtlasRepos {
      atlas_repos: spec.atlas_repos.clone(),
      paths: linked.paths.clone(),
    });
  }
  for file in &spec.ground_truth.must_change {
    if let Some(path) = linked.paths.iter().find(|path| is_at_or_below(file, path)) {
      return Err(LinkedSpecError::MustChangeLinked {
        file: file.clone(),
        path: path.clone(),
      });
    }
  }
  if RepoName::new(linked.repo.as_str()).is_err() {
    return Err(LinkedSpecError::InvalidRepo(linked.repo.clone()));
  }
  if linked.repo == SESSION_REPO {
    return Err(LinkedSpecError::ReservedRepo(linked.repo.clone()));
  }
  match linked.patch.as_deref() {
    Some(patch) if !is_relative_inside(patch) || patch.starts_with("hidden/") => Err(LinkedSpecError::PatchLocation(patch.to_owned())),
    _ => Ok(()),
  }
}

fn read_linked_patch(dir: &Path, spec_path: &Path, linked: &LinkedSpec, patch: &str) -> AgentResult<Vec<u8>> {
  let file = dir.join(patch);
  let invalid = |source| AgentError::InvalidLinked {
    path: spec_path.to_owned(),
    source,
  };
  let bytes = match fs::read(&file) {
    Ok(bytes) => bytes,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(invalid(LinkedSpecError::PatchMissing(patch.to_owned()))),
    Err(error) => return Err(AgentError::io(&file, error)),
  };
  let touched = patch_paths(&String::from_utf8_lossy(&bytes));
  if touched.is_empty() {
    return Err(invalid(LinkedSpecError::PatchEmpty(patch.to_owned())));
  }
  if let Some(file) = touched.into_iter().find(|file| !linked.contains(file)) {
    return Err(invalid(LinkedSpecError::PatchOutsidePaths { patch: patch.to_owned(), file }));
  }
  Ok(bytes)
}

fn patch_paths(patch: &str) -> BTreeSet<String> {
  let mut paths = BTreeSet::new();
  let (mut old_left, mut new_left) = (0u32, 0u32);
  for line in patch.lines() {
    if old_left > 0 || new_left > 0 {
      match line.as_bytes().first() {
        Some(b'+') => new_left = new_left.saturating_sub(1),
        Some(b'-') => old_left = old_left.saturating_sub(1),
        Some(b'\\') => {}
        _ => {
          old_left = old_left.saturating_sub(1);
          new_left = new_left.saturating_sub(1);
        }
      }
      continue;
    }
    if let Some(header) = line.strip_prefix("@@ ") {
      (old_left, new_left) = hunk_lengths(header);
    } else if let Some(header) = line.strip_prefix("diff --git ") {
      if let Some((old, new)) = header.split_once(" b/") {
        paths.extend([old.trim_matches('"').trim_start_matches("a/"), new.trim_matches('"')].map(str::to_owned));
      }
    } else if let Some(side) = line.strip_prefix("--- ").or_else(|| line.strip_prefix("+++ ")) {
      let side = side.split('\t').next().unwrap_or(side).trim_matches('"');
      if side != "/dev/null" {
        paths.insert(side.strip_prefix("a/").or_else(|| side.strip_prefix("b/")).unwrap_or(side).to_owned());
      }
    } else if let Some(named) = ["rename from ", "rename to ", "copy from ", "copy to "].iter().find_map(|prefix| line.strip_prefix(prefix)) {
      paths.insert(named.trim_matches('"').to_owned());
    }
  }
  paths
}

fn hunk_lengths(header: &str) -> (u32, u32) {
  let length = |range: Option<&str>| {
    range.map_or(1, |range| match range.split_once(',') {
      Some((_, count)) => count.parse().unwrap_or(0),
      None => 1,
    })
  };
  let mut ranges = header.split_whitespace();
  let old = ranges.next().and_then(|range| range.strip_prefix('-'));
  let new = ranges.next().and_then(|range| range.strip_prefix('+'));
  (length(old), length(new))
}

fn is_at_or_below(path: &str, ancestor: &str) -> bool {
  path == ancestor || path.strip_prefix(ancestor).is_some_and(|rest| rest.starts_with('/'))
}

fn validate_rename(rename: &RenameSpec) -> Result<(), String> {
  let identifier = |text: &str| {
    text.chars().next().is_some_and(|first| first.is_alphabetic() || first == '_' || first == '$') && text.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$')
  };
  if !identifier(rename.old_name()) {
    return Err(format!("rename symbol `{}` must end in an identifier", rename.symbol));
  }
  if !identifier(&rename.new_name) {
    return Err(format!("rename new_name `{}` must be an identifier", rename.new_name));
  }
  if rename.old_name() == rename.new_name {
    return Err("rename new_name equals the old name".to_owned());
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
  fn should_parse_a_rename_section_and_derive_the_old_name_from_the_symbol() {
    let spec = parse(&format!("{SPEC}\n[rename]\nsymbol = \"atlas_query::TargetFilter\"\nnew_name = \"TargetConstraint\"\n")).unwrap();
    let rename = spec.rename.unwrap();
    assert_eq!((rename.old_name(), rename.new_name.as_str()), ("TargetFilter", "TargetConstraint"));
    let dotted = RenameSpec {
      symbol: "UsersService.findOne".to_owned(),
      new_name: "getById".to_owned(),
    };
    assert_eq!(dotted.old_name(), "findOne");
    assert_eq!(parse(SPEC).unwrap().rename, None);
  }

  #[test]
  fn should_reject_a_rename_that_is_not_an_identifier_or_changes_nothing() {
    for section in [
      "symbol = \"A.b\"\nnew_name = \"not valid\"",
      "symbol = \"A.b\"\nnew_name = \"b\"",
      "symbol = \"A.\"\nnew_name = \"c\"",
    ] {
      assert!(parse(&format!("{SPEC}\n[rename]\n{section}\n")).is_err(), "{section}");
    }
  }

  #[test]
  fn should_parse_the_declared_prompt_leak_class_and_leave_it_unset_by_default() {
    assert_eq!(parse(SPEC).unwrap().prompt_leak, None);
    for (text, class) in [("handed", PromptLeak::Handed), ("guided", PromptLeak::Guided), ("open", PromptLeak::Open)] {
      let spec = parse(&SPEC.replace("max_turns = 30", &format!("max_turns = 30\nprompt_leak = \"{text}\""))).unwrap();
      assert_eq!(spec.prompt_leak, Some(class));
      assert_eq!(class.label(), text);
    }
    assert!(parse(&SPEC.replace("max_turns = 30", "max_turns = 30\nprompt_leak = \"leaky\"")).is_err());
    assert!(!serde_json::to_string(&parse(SPEC).unwrap()).unwrap().contains("prompt_leak"));
  }

  #[test]
  fn should_parse_paths_to_strip_and_instruction_sections_to_drop_and_leave_both_out_when_unset() {
    let declared = "max_turns = 30\nstrip = [\"acme-e2e\", \"docs/postmortem.md\"]\ndrop_instruction_sections = [\"AGENTS.md#Code search\"]";
    let spec = parse(&SPEC.replace("max_turns = 30", declared)).unwrap();
    assert_eq!(spec.strip, vec!["acme-e2e", "docs/postmortem.md"]);
    assert_eq!(spec.drop_instruction_sections, vec!["AGENTS.md#Code search"]);
    let plain = serde_json::to_string(&parse(SPEC).unwrap()).unwrap();
    assert!(!plain.contains("strip") && !plain.contains("drop_instruction_sections"), "{plain}");
  }

  #[test]
  fn should_reject_strip_paths_and_section_files_that_leave_the_tree_and_sections_without_a_heading() {
    for field in [
      "strip = [\"../acme-e2e\"]",
      "strip = [\"/tmp/acme-e2e\"]",
      "strip = [\"\"]",
      "strip = [\".\"]",
      "strip = [\"docs/./notes\"]",
      "strip = [\"docs//notes\"]",
      "drop_instruction_sections = [\"AGENTS.md\"]",
      "drop_instruction_sections = [\"AGENTS.md#\"]",
      "drop_instruction_sections = [\"../AGENTS.md#Code search\"]",
    ] {
      assert!(parse(&SPEC.replace("max_turns = 30", &format!("max_turns = 30\n{field}"))).is_err(), "{field}");
    }
  }

  #[test]
  fn should_reject_unknown_fields() {
    assert!(parse(&format!("{SPEC}\nsurprise = 1\n")).is_err());
  }

  #[test]
  fn should_parse_setup_commands_and_a_base_patch() {
    let spec = parse(&SPEC.replace("max_turns = 30", "max_turns = 30\nbase_patch = \"base.patch\"\nsetup = [\"bun install --frozen-lockfile\"]")).unwrap();
    assert_eq!(spec.base_patch.as_deref(), Some("base.patch"));
    assert_eq!(spec.setup, vec!["bun install --frozen-lockfile"]);
    assert_eq!(parse(SPEC).unwrap().base_patch, None);
  }

  #[test]
  fn should_reject_escaping_or_hidden_base_patches_and_blank_setup_commands() {
    for patch in ["../base.patch", "/tmp/base.patch", "hidden/base.patch"] {
      assert!(
        parse(&SPEC.replace("max_turns = 30", &format!("max_turns = 30\nbase_patch = \"{patch}\""))).is_err(),
        "{patch}"
      );
    }
    assert!(parse(&SPEC.replace("max_turns = 30", "max_turns = 30\nsetup = [\" \"]")).is_err());
  }

  fn task_dir(spec: &str, patch: Option<&str>) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("t00-smoke");
    fs::create_dir_all(dir.join("hidden/acme-api")).unwrap();
    fs::write(dir.join("hidden/acme-api/check.mjs"), "").unwrap();
    fs::write(dir.join(TASK_FILE), spec).unwrap();
    if let Some(patch) = patch {
      fs::write(dir.join("base.patch"), patch).unwrap();
    }
    root
  }

  #[test]
  fn should_require_the_base_patch_file_and_hash_its_content() {
    let with_patch = SPEC.replace("max_turns = 30", "max_turns = 30\nbase_patch = \"base.patch\"");
    let missing = task_dir(&with_patch, None);
    assert!(load_task(&missing.path().join("t00-smoke")).is_err());
    let first = task_dir(&with_patch, Some("a"));
    let second = task_dir(&with_patch, Some("b"));
    let first = load_task(&first.path().join("t00-smoke")).unwrap();
    let second = load_task(&second.path().join("t00-smoke")).unwrap();
    assert_ne!(first.spec_hash, second.spec_hash);
    assert_eq!(first.base_patch_file(), Some(first.dir.join("base.patch")));
    let plain = task_dir(SPEC, None);
    assert_eq!(load_task(&plain.path().join("t00-smoke")).unwrap().spec_hash, blake3::hash(SPEC.as_bytes()).to_hex()[..16]);
  }

  const LINKED_SPEC: &str = r#"
id = "t00-smoke"
title = "Smoke"
repo = { fixture = "acme" }
base_commit = "9b1fc2d"
prompt = "do it"
timeout_minutes = 5
max_turns = 30
strip = ["acme-e2e"]
[linked]
paths = ["apps/acme-web", "packages/acme-ui"]
patch = "linked.patch"
repo = "web"
[ground_truth]
must_change = ["acme-api/src/users/user.entity.ts"]
must_consider = ["apps/acme-web/src/api/client.ts"]
"#;

  const LINKED_PATCH: &str = "diff --git a/apps/acme-web/src/api/client.ts b/apps/acme-web/src/api/client.ts
--- a/apps/acme-web/src/api/client.ts
+++ b/apps/acme-web/src/api/client.ts
@@ -1,2 +1,2 @@
-export const base = '/api';
+export const base = '/api/v2';
--- a/acme-api/src/not-a-header.ts
 export {};
diff --git a/packages/acme-ui/src/new.ts b/packages/acme-ui/src/new.ts
new file mode 100644
--- /dev/null
+++ b/packages/acme-ui/src/new.ts
@@ -0,0 +1 @@
+export const added = 1;
";

  fn linked_error(text: &str) -> LinkedSpecError {
    match parse(text) {
      Err(AgentError::InvalidLinked { source, .. }) => source,
      other => panic!("expected an invalid [linked] table, got {other:?}"),
    }
  }

  #[test]
  fn should_parse_a_linked_table_and_default_its_repo_name() {
    let spec = parse(LINKED_SPEC).unwrap();
    assert_eq!(
      spec.linked,
      Some(LinkedSpec {
        paths: vec!["apps/acme-web".to_owned(), "packages/acme-ui".to_owned()],
        patch: Some("linked.patch".to_owned()),
        repo: "web".to_owned(),
      })
    );
    let defaulted = parse(&LINKED_SPEC.replace("patch = \"linked.patch\"\nrepo = \"web\"\n", "")).unwrap().linked.unwrap();
    assert_eq!((defaulted.patch.as_deref(), defaulted.repo.as_str()), (None, DEFAULT_LINKED_REPO));
    assert_eq!(defaulted.include_globs(), vec!["apps/acme-web/**", "packages/acme-ui/**"]);
    assert!(defaulted.contains("apps/acme-web/src/main.ts") && defaulted.contains("apps/acme-web"));
    assert!(!defaulted.contains("apps/acme-web-old/src/main.ts"));
    assert!(!serde_json::to_string(&parse(SPEC).unwrap()).unwrap().contains("linked"));
  }

  #[test]
  fn should_reject_each_invalid_linked_table_with_its_typed_error() {
    let with_paths = |paths: &str| LINKED_SPEC.replace(r#"paths = ["apps/acme-web", "packages/acme-ui"]"#, &format!("paths = {paths}"));
    let cases = [
      (with_paths("[]"), LinkedSpecError::NoPaths),
      (with_paths(r#"["."]"#), LinkedSpecError::TreeRoot(".".to_owned())),
      (with_paths(r#"[""]"#), LinkedSpecError::TreeRoot(String::new())),
      (with_paths(r#"["/apps/acme-web"]"#), LinkedSpecError::InvalidPath("/apps/acme-web".to_owned())),
      (with_paths(r#"["apps/../acme-web"]"#), LinkedSpecError::InvalidPath("apps/../acme-web".to_owned())),
      (with_paths(r#"["apps//acme-web"]"#), LinkedSpecError::InvalidPath("apps//acme-web".to_owned())),
      (with_paths(r#"["apps/acme-web/"]"#), LinkedSpecError::InvalidPath("apps/acme-web/".to_owned())),
      (with_paths(r#"["apps/acme-web", "apps/acme-web"]"#), LinkedSpecError::Duplicate("apps/acme-web".to_owned())),
      (
        with_paths(r#"["apps/acme-web/src", "apps/acme-web"]"#),
        LinkedSpecError::Nested {
          outer: "apps/acme-web".to_owned(),
          inner: "apps/acme-web/src".to_owned(),
        },
      ),
      (
        with_paths(r#"["acme-e2e"]"#),
        LinkedSpecError::InsideStrip {
          path: "acme-e2e".to_owned(),
          strip: "acme-e2e".to_owned(),
        },
      ),
      (
        with_paths(r#"["acme-e2e/web"]"#),
        LinkedSpecError::InsideStrip {
          path: "acme-e2e/web".to_owned(),
          strip: "acme-e2e".to_owned(),
        },
      ),
      (
        LINKED_SPEC.replace("strip = [\"acme-e2e\"]", "atlas_repos = [\"acme-api\"]"),
        LinkedSpecError::WithAtlasRepos {
          atlas_repos: vec!["acme-api".to_owned()],
          paths: vec!["apps/acme-web".to_owned(), "packages/acme-ui".to_owned()],
        },
      ),
      (
        LINKED_SPEC.replace(
          "must_change = [\"acme-api/src/users/user.entity.ts\"]",
          "must_change = [\"packages/acme-ui/src/button.ts\"]",
        ),
        LinkedSpecError::MustChangeLinked {
          file: "packages/acme-ui/src/button.ts".to_owned(),
          path: "packages/acme-ui".to_owned(),
        },
      ),
      (
        LINKED_SPEC.replace("repo = \"web\"", "repo = \"Web App\""),
        LinkedSpecError::InvalidRepo("Web App".to_owned()),
      ),
      (LINKED_SPEC.replace("repo = \"web\"", "repo = \"tree\""), LinkedSpecError::ReservedRepo("tree".to_owned())),
      (
        LINKED_SPEC.replace("patch = \"linked.patch\"", "patch = \"../linked.patch\""),
        LinkedSpecError::PatchLocation("../linked.patch".to_owned()),
      ),
      (
        LINKED_SPEC.replace("patch = \"linked.patch\"", "patch = \"hidden/linked.patch\""),
        LinkedSpecError::PatchLocation("hidden/linked.patch".to_owned()),
      ),
    ];
    for (text, expected) in cases {
      assert_eq!(linked_error(&text), expected);
    }
    assert!(parse(&LINKED_SPEC.replace("repo = \"web\"", "repo = \"web\"\nsurprise = 1")).is_err());
  }

  #[test]
  fn should_name_both_tables_when_atlas_repos_meet_linked_paths() {
    let error = parse(&LINKED_SPEC.replace("strip = [\"acme-e2e\"]", "atlas_repos = [\"acme-api\"]")).unwrap_err();
    assert_eq!(
      error.to_string(),
      "invalid [linked] table in task spec task.toml: `atlas_repos` (acme-api) cannot be combined with [linked] (apps/acme-web, packages/acme-ui): with linked paths the session tree is one Code Atlas repo"
    );
  }

  fn linked_task_dir(patch: Option<&str>) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("t00-smoke");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(TASK_FILE), LINKED_SPEC).unwrap();
    if let Some(patch) = patch {
      fs::write(dir.join("linked.patch"), patch).unwrap();
    }
    root
  }

  fn load_linked(patch: Option<&str>) -> (tempfile::TempDir, AgentResult<LoadedTask>) {
    let root = linked_task_dir(patch);
    let loaded = load_task(&root.path().join("t00-smoke"));
    (root, loaded)
  }

  fn load_error(patch: Option<&str>) -> LinkedSpecError {
    match load_linked(patch).1 {
      Err(AgentError::InvalidLinked { source, .. }) => source,
      other => panic!("expected an invalid [linked] table, got {other:?}"),
    }
  }

  #[test]
  fn should_require_the_linked_patch_to_exist_touch_something_and_stay_under_the_linked_paths() {
    assert_eq!(load_error(None), LinkedSpecError::PatchMissing("linked.patch".to_owned()));
    assert_eq!(load_error(Some("not a patch\n")), LinkedSpecError::PatchEmpty("linked.patch".to_owned()));
    let outside = LINKED_PATCH.replace("packages/acme-ui/src/new.ts", "acme-api/src/new.ts");
    assert_eq!(
      load_error(Some(&outside)),
      LinkedSpecError::PatchOutsidePaths {
        patch: "linked.patch".to_owned(),
        file: "acme-api/src/new.ts".to_owned(),
      }
    );
    let renamed = "diff --git a/apps/acme-web/a.ts b/acme-api/a.ts\nsimilarity index 100%\nrename from apps/acme-web/a.ts\nrename to acme-api/a.ts\n";
    assert_eq!(
      load_error(Some(renamed)),
      LinkedSpecError::PatchOutsidePaths {
        patch: "linked.patch".to_owned(),
        file: "acme-api/a.ts".to_owned(),
      }
    );
    let (_root, loaded) = load_linked(Some(LINKED_PATCH));
    let loaded = loaded.unwrap();
    assert_eq!(loaded.linked_patch_file(), Some(loaded.dir.join("linked.patch")));
  }

  #[test]
  fn should_keep_an_unreadable_linked_patch_an_io_error_rather_than_a_missing_one() {
    let root = linked_task_dir(None);
    let patch = root.path().join("t00-smoke/linked.patch");
    fs::create_dir_all(&patch).unwrap();
    let loaded = load_task(&root.path().join("t00-smoke"));
    assert!(matches!(&loaded, Err(AgentError::Io { path, .. }) if *path == patch), "{loaded:?}");
  }

  #[test]
  fn should_read_both_sides_of_every_file_header_and_ignore_hunk_lines() {
    assert_eq!(
      patch_paths(LINKED_PATCH).into_iter().collect::<Vec<_>>(),
      vec!["apps/acme-web/src/api/client.ts", "packages/acme-ui/src/new.ts"]
    );
    let plain = "--- a/apps/acme-web/x.ts\t2026-01-01\n+++ b/apps/acme-web/y.ts\n@@ -1 +1 @@\n-a\n+b\n";
    assert_eq!(patch_paths(plain).into_iter().collect::<Vec<_>>(), vec!["apps/acme-web/x.ts", "apps/acme-web/y.ts"]);
  }

  #[test]
  fn should_hash_the_linked_patch_into_the_spec_hash() {
    let (_first_root, first) = load_linked(Some(LINKED_PATCH));
    let (_second_root, second) = load_linked(Some(&LINKED_PATCH.replace("added = 1", "added = 2")));
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_ne!(first.spec_hash, second.spec_hash);
    let mut hasher = blake3::Hasher::new();
    hasher.update(LINKED_SPEC.as_bytes());
    hasher.update(LINKED_PATCH_DOMAIN);
    hasher.update(LINKED_PATCH.as_bytes());
    assert_eq!(first.spec_hash, hasher.finalize().to_hex()[..16]);
  }

  #[test]
  fn should_strip_the_hidden_prefix_for_the_destination() {
    assert_eq!(hidden_destination("hidden/acme-api/test/a.spec.ts"), "acme-api/test/a.spec.ts");
  }
}
