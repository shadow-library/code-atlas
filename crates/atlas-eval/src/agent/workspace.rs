use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::error::{AgentError, AgentResult};
use super::linked_snapshot::LinkedSnapshot;
use super::process::{ProcessExit, run_captured, tail};
use super::spec::{HIDDEN_DIR, LinkedSpec, LoadedTask, RepoSource, anchored_glob, hidden_destination};
use super::tree_edit::{FinalLink, drop_sections, remove_paths, tree_path};

const GIT_TIMEOUT: Duration = Duration::from_secs(600);
const STRIPPED_ROOT_DIRS: [&str; 3] = ["eval", "scratch-pad", HIDDEN_DIR];
const REPOSITORY_ONLY_DIRS: [&str; 1] = ["docs/plan"];
const GROUND_TRUTH_FILES: [&str; 2] = ["expected.toml", "expected.schema.md"];
const SCRATCH_PREFIX: &str = "ae-";
const COPY_EXCLUDED_DIRS: [&str; 3] = [".git", "target", "node_modules"];
const LOCAL_EXCLUDES: &str = "target/\nnode_modules/\n.code-atlas/\n";
const BASE_TAG: &str = "eval-base";
const FIXED_DATE: &str = "2000-01-01T00:00:00Z";
pub const LINKED_HOLD: &str = "linked-hold";
pub const LINKED_VIEW: &str = "linked-view";
pub const LINKED_CONFLICT: &str = "linked-conflict";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TreeSource {
  GitArchive { repo: PathBuf, commit: String, subdir: Option<String> },
  Directory { path: PathBuf },
}

impl TreeSource {
  pub fn reproducible(&self) -> bool {
    matches!(self, Self::GitArchive { .. })
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StripProfile {
  ThisRepository,
  Fixture,
  External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StripPlan {
  pub profile: StripProfile,
  pub hidden_paths: Vec<String>,
}

impl StripPlan {
  pub fn for_task(task: &LoadedTask, overridden: bool) -> Self {
    let profile = match (&task.spec.repo, overridden) {
      (_, true) | (RepoSource::SelfRepo, false) => StripProfile::ThisRepository,
      (RepoSource::Fixture { .. }, false) => StripProfile::Fixture,
      (RepoSource::Git { .. }, false) => StripProfile::External,
    };
    let hidden_paths = task.spec.validation.hidden_tests.iter().map(|entry| hidden_destination(entry).to_owned()).collect();
    Self { profile, hidden_paths }
  }
}

pub fn scratch_dir(tree_root: &Path, batch_dir: &Path, key: &str) -> PathBuf {
  let digest = blake3::hash(format!("{}\0{key}", batch_dir.display()).as_bytes()).to_hex();
  tree_root.join(format!("{SCRATCH_PREFIX}{}", &digest[..16]))
}

pub fn check_isolated_location(dir: &Path, forbidden_roots: &[&Path]) -> AgentResult<()> {
  let canonical = dir.canonicalize().map_err(|error| AgentError::io(dir, error))?;
  for root in forbidden_roots {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if canonical.starts_with(&root) {
      return Err(AgentError::InvalidArgument(format!("{} must not live under {}", canonical.display(), root.display())));
    }
  }
  if let Some(repo) = canonical.ancestors().find(|ancestor| ancestor.join(".git").exists()) {
    return Err(AgentError::InvalidArgument(format!(
      "{} has a git repository ancestor at {}",
      canonical.display(),
      repo.display()
    )));
  }
  Ok(())
}

pub fn resolve_source(task: &LoadedTask, repo_root: &Path, override_dir: Option<&Path>, cache_dir: &Path) -> AgentResult<TreeSource> {
  if let Some(path) = override_dir {
    return Ok(TreeSource::Directory { path: path.to_owned() });
  }
  let base_commit = || {
    task.spec.base_commit.clone().ok_or_else(|| AgentError::InvalidSpec {
      path: task.dir.clone(),
      reason: "base_commit is missing".to_owned(),
    })
  };
  match &task.spec.repo {
    RepoSource::SelfRepo => Ok(TreeSource::GitArchive {
      repo: repo_root.to_owned(),
      commit: base_commit()?,
      subdir: None,
    }),
    RepoSource::Fixture { name } => Ok(TreeSource::GitArchive {
      repo: repo_root.to_owned(),
      commit: base_commit()?,
      subdir: Some(format!("fixtures/{name}")),
    }),
    RepoSource::Git { url, commit } => Ok(TreeSource::GitArchive {
      repo: ensure_clone(url, commit, cache_dir)?,
      commit: commit.clone(),
      subdir: None,
    }),
  }
}

fn ensure_clone(url: &str, commit: &str, cache_dir: &Path) -> AgentResult<PathBuf> {
  let clone = cache_dir.join("repos").join(&blake3::hash(url.as_bytes()).to_hex()[..16]);
  if !clone.join("HEAD").is_file() {
    fs::create_dir_all(cache_dir.join("repos")).map_err(|error| AgentError::io(cache_dir, error))?;
    git(cache_dir, &["clone", "--quiet", "--bare", url, &clone.display().to_string()])?;
  }
  let wanted = format!("{commit}^{{commit}}");
  if git(&clone, &["cat-file", "-e", &wanted]).is_err() {
    git(&clone, &["fetch", "--quiet", "origin", commit])?;
  }
  Ok(clone)
}

pub fn materialize(source: &TreeSource, tree: &Path, plan: &StripPlan) -> AgentResult<()> {
  fs::create_dir_all(tree).map_err(|error| AgentError::io(tree, error))?;
  match source {
    TreeSource::GitArchive { repo, commit, subdir } => archive(repo, commit, subdir.as_deref(), tree)?,
    TreeSource::Directory { path } => copy_dir(path, tree)?,
  }
  strip(tree, plan)
}

pub fn materialize_task(task: &LoadedTask, source: &TreeSource, tree: &Path, plan: &StripPlan) -> AgentResult<()> {
  materialize(source, tree, plan)?;
  if let Some(patch) = task.base_patch_file() {
    apply_patch(tree, &patch)?;
  }
  remove_paths(tree, &task.spec.strip)?;
  drop_sections(tree, &task.spec.drop_instruction_sections)
}

pub fn apply_linked_patch(task: &LoadedTask, tree: &Path) -> AgentResult<()> {
  match task.linked_patch_file() {
    Some(patch) => apply_patch(tree, &patch),
    None => Ok(()),
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedLayout<'a> {
  pub spec: &'a LinkedSpec,
  pub hold: PathBuf,
  pub view: PathBuf,
  pub conflict: PathBuf,
}

impl<'a> LinkedLayout<'a> {
  pub fn of(task: &'a LoadedTask, scratch: &Path) -> Option<Self> {
    let spec = task.spec.linked.as_ref()?;
    Some(Self {
      spec,
      hold: scratch.join(LINKED_HOLD),
      view: scratch.join(LINKED_VIEW),
      conflict: scratch.join(LINKED_CONFLICT),
    })
  }

  pub fn materialize_view(&self, task: &LoadedTask, source: &TreeSource, plan: &StripPlan) -> AgentResult<()> {
    materialize_task(task, source, &self.view, plan)?;
    apply_linked_patch(task, &self.view)
  }

  pub fn hide(&self, tree: &Path) -> AgentResult<()> {
    for path in &self.spec.paths {
      let from = tree_path(tree, path, FinalLink::Refused)?;
      match fs::symlink_metadata(&from) {
        Ok(kind) if kind.is_dir() => {}
        Ok(_) => return Err(AgentError::InvalidArgument(format!("linked path `{path}` is not a directory of the tree"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
          return Err(AgentError::InvalidArgument(format!("linked path `{path}` does not exist in the tree")));
        }
        Err(error) => return Err(AgentError::io(&from, error)),
      }
      move_path(&from, &self.hold.join(path))?;
      remove_empty_ancestors(tree, path)?;
    }
    Ok(())
  }

  pub fn restore(&self, tree: &Path) -> AgentResult<Restored> {
    let mut restored = Restored::default();
    for path in &self.spec.paths {
      if let Some(occupied) = occupied_prefix(tree, path)? {
        move_path(&tree.join(&occupied), &self.conflict.join(&occupied))?;
        restored.conflicts.push(occupied);
      }
      let held = self.hold.join(path);
      match fs::symlink_metadata(&held) {
        Ok(kind) if kind.is_dir() => move_path(&held, &tree.join(path))?,
        Ok(_) => restored.missing.push(path.clone()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => restored.missing.push(path.clone()),
        Err(error) => return Err(AgentError::io(&held, error)),
      }
    }
    Ok(restored)
  }

  pub fn snapshot(&self) -> LinkedSnapshot {
    LinkedSnapshot::take(&[(LINKED_HOLD, &self.hold), (LINKED_VIEW, &self.view)])
  }

  pub fn view_excludes(&self) -> AgentResult<Vec<String>> {
    let mut dirs: BTreeSet<String> = BTreeSet::from([String::new()]);
    for path in &self.spec.paths {
      let mut ancestor = Path::new(path).parent();
      while let Some(dir) = ancestor.and_then(Path::to_str).filter(|dir| !dir.is_empty()) {
        dirs.insert(dir.to_owned());
        ancestor = Path::new(dir).parent();
      }
    }
    let mut excludes = Vec::new();
    for dir in &dirs {
      let absolute = self.view.join(dir);
      let entries = fs::read_dir(&absolute).map_err(|error| AgentError::io(&absolute, error))?;
      let mut names = Vec::new();
      for entry in entries {
        let entry = entry.map_err(|error| AgentError::io(&absolute, error))?;
        let is_dir = entry.file_type().map_err(|error| AgentError::io(entry.path(), error))?.is_dir();
        if let Some(name) = entry.file_name().to_str().filter(|_| is_dir) {
          names.push(if dir.is_empty() { name.to_owned() } else { format!("{dir}/{name}") });
        }
      }
      names.sort();
      excludes.extend(
        names
          .into_iter()
          .filter(|relative| !self.spec.paths.contains(relative) && !self.spec.is_linked_ancestor(relative))
          .map(|relative| anchored_glob(&relative)),
      );
    }
    Ok(excludes)
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Restored {
  pub conflicts: Vec<String>,
  pub missing: Vec<String>,
}

fn occupied_prefix(tree: &Path, path: &str) -> AgentResult<Option<String>> {
  let segments: Vec<&str> = path.split('/').collect();
  for depth in 1..=segments.len() {
    let prefix = segments[..depth].join("/");
    let target = tree.join(&prefix);
    match fs::symlink_metadata(&target) {
      Ok(kind) if kind.is_dir() && depth < segments.len() => {}
      Ok(_) => return Ok(Some(prefix)),
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
      Err(error) => return Err(AgentError::io(&target, error)),
    }
  }
  Ok(None)
}

fn move_path(from: &Path, to: &Path) -> AgentResult<()> {
  if let Some(parent) = to.parent() {
    fs::create_dir_all(parent).map_err(|error| AgentError::io(parent, error))?;
  }
  fs::rename(from, to).map_err(|error| AgentError::io(from, error))
}

fn remove_empty_ancestors(tree: &Path, path: &str) -> AgentResult<()> {
  let mut current = Path::new(path).parent();
  while let Some(relative) = current.filter(|relative| !relative.as_os_str().is_empty()) {
    let dir = tree.join(relative);
    let mut entries = fs::read_dir(&dir).map_err(|error| AgentError::io(&dir, error))?;
    if entries.next().is_some() {
      return Ok(());
    }
    fs::remove_dir(&dir).map_err(|error| AgentError::io(&dir, error))?;
    current = relative.parent();
  }
  Ok(())
}

pub fn materialize_session_tree(task: &LoadedTask, source: &TreeSource, tree: &Path, plan: &StripPlan) -> AgentResult<()> {
  materialize_task(task, source, tree, plan)?;
  match &task.spec.linked {
    Some(linked) => remove_paths(tree, &linked.paths),
    None => Ok(()),
  }
}

pub fn local_source_path(source: &RepoSource, cache_dir: &Path) -> Option<PathBuf> {
  let RepoSource::Git { url, .. } = source else {
    return None;
  };
  let location = url.strip_prefix("file://").unwrap_or(url);
  let host_before_path = location.split('/').next().is_some_and(|first| first.contains(':'));
  if location.contains("://") || host_before_path {
    return None;
  }
  // `ensure_clone` runs `git clone` in the cache directory, so that is what a relative source resolves against.
  let path = cache_dir.join(location);
  Some(path.canonicalize().unwrap_or_else(|_| std::path::absolute(&path).unwrap_or(path)))
}

pub fn apply_patch(tree: &Path, patch: &Path) -> AgentResult<()> {
  let patch = patch.canonicalize().map_err(|error| AgentError::io(patch, error))?;
  let tree = tree.canonicalize().map_err(|error| AgentError::io(tree, error))?;
  let mut command = git_command(&tree);
  command.args(["apply", "--whitespace=nowarn"]).arg(&patch);
  if let Some(parent) = tree.parent() {
    // Outside a repository `git apply` patches the cwd; the ceiling keeps it from adopting an ancestor repository.
    command.env("GIT_CEILING_DIRECTORIES", parent);
  }
  let output = run_captured(command, None, GIT_TIMEOUT);
  match output.exit {
    ProcessExit::Exited { code: Some(0) } => Ok(()),
    exit => Err(AgentError::Command {
      command: format!("git apply {}", patch.display()),
      detail: format!("{exit:?}: {}", tail(output.stderr.trim(), 2000)),
    }),
  }
}

fn remove_file(path: &Path) -> AgentResult<()> {
  match fs::remove_file(path) {
    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(AgentError::io(path, error)),
    _ => Ok(()),
  }
}

fn archive(repo: &Path, commit: &str, subdir: Option<&str>, tree: &Path) -> AgentResult<()> {
  let treeish = subdir.map_or_else(|| commit.to_owned(), |subdir| format!("{commit}:{subdir}"));
  let rendered = format!("git -C {} archive {treeish} | tar -x -C {}", repo.display(), tree.display());
  let failed = |detail: String| AgentError::Command {
    command: rendered.clone(),
    detail,
  };
  let mut producer = git_command(repo)
    .args(["archive", "--format=tar", &treeish])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|error| failed(error.to_string()))?;
  let stdout = producer.stdout.take().ok_or_else(|| failed("git archive has no stdout".to_owned()))?;
  let extracted = Command::new("tar")
    .arg("-x")
    .arg("-C")
    .arg(tree)
    .stdin(stdout)
    .output()
    .map_err(|error| failed(error.to_string()))?;
  let produced = producer.wait_with_output().map_err(|error| failed(error.to_string()))?;
  if !produced.status.success() {
    return Err(failed(String::from_utf8_lossy(&produced.stderr).trim().to_owned()));
  }
  if !extracted.status.success() {
    return Err(failed(String::from_utf8_lossy(&extracted.stderr).trim().to_owned()));
  }
  Ok(())
}

pub(super) fn copy_dir(from: &Path, to: &Path) -> AgentResult<()> {
  let entries = fs::read_dir(from).map_err(|error| AgentError::io(from, error))?;
  for entry in entries {
    let entry = entry.map_err(|error| AgentError::io(from, error))?;
    let name = entry.file_name();
    let source = entry.path();
    let target = to.join(&name);
    let kind = entry.file_type().map_err(|error| AgentError::io(&source, error))?;
    if kind.is_dir() {
      if COPY_EXCLUDED_DIRS.iter().any(|excluded| name == *excluded) {
        continue;
      }
      fs::create_dir_all(&target).map_err(|error| AgentError::io(&target, error))?;
      copy_dir(&source, &target)?;
    } else if kind.is_file() {
      fs::copy(&source, &target).map_err(|error| AgentError::io(&source, error))?;
    }
  }
  Ok(())
}

pub fn strip(tree: &Path, plan: &StripPlan) -> AgentResult<()> {
  for dir in STRIPPED_ROOT_DIRS {
    remove_dir(&tree.join(dir))?;
  }
  for hidden in &plan.hidden_paths {
    remove_file(&tree.join(hidden))?;
  }
  match plan.profile {
    StripProfile::ThisRepository => {
      for dir in REPOSITORY_ONLY_DIRS {
        remove_dir(&tree.join(dir))?;
      }
      Ok(())
    }
    StripProfile::Fixture => remove_ground_truth(tree),
    StripProfile::External => Ok(()),
  }
}

fn remove_ground_truth(dir: &Path) -> AgentResult<()> {
  let entries = fs::read_dir(dir).map_err(|error| AgentError::io(dir, error))?;
  for entry in entries {
    let entry = entry.map_err(|error| AgentError::io(dir, error))?;
    let path = entry.path();
    let kind = entry.file_type().map_err(|error| AgentError::io(&path, error))?;
    if kind.is_dir() && entry.file_name() != ".git" {
      remove_ground_truth(&path)?;
    } else if kind.is_file() && GROUND_TRUTH_FILES.iter().any(|name| entry.file_name() == *name) {
      remove_file(&path)?;
    }
  }
  Ok(())
}

fn remove_dir(path: &Path) -> AgentResult<()> {
  match fs::remove_dir_all(path) {
    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(AgentError::io(path, error)),
    _ => Ok(()),
  }
}

pub fn init_base(tree: &Path) -> AgentResult<String> {
  git(tree, &["init", "--quiet"])?;
  let exclude = tree.join(".git").join("info").join("exclude");
  if let Some(parent) = exclude.parent() {
    fs::create_dir_all(parent).map_err(|error| AgentError::io(parent, error))?;
  }
  fs::write(&exclude, LOCAL_EXCLUDES).map_err(|error| AgentError::io(&exclude, error))?;
  git(tree, &["add", "--all"])?;
  git(tree, &["commit", "--quiet", "--allow-empty", "--no-verify", "-m", BASE_TAG])?;
  git(tree, &["tag", BASE_TAG])?;
  Ok(git(tree, &["rev-parse", "HEAD"])?.trim().to_owned())
}

pub fn diff_against(tree: &Path, base: &str, excluded: &[String]) -> AgentResult<String> {
  git(tree, &["add", "--all"])?;
  let pathspecs: Vec<String> = excluded.iter().map(|path| format!(":(top,exclude,literal){path}")).collect();
  let mut args = vec!["diff", "--cached", "--no-color", "--no-ext-diff", "--find-renames", base, "--", "."];
  args.extend(pathspecs.iter().map(String::as_str));
  git(tree, &args)
}

pub fn base_file(tree: &Path, base: &str, path: &str) -> Option<String> {
  git(tree, &["show", &format!("{base}:{path}")]).ok()
}

pub fn copy_hidden(task: &LoadedTask, tree: &Path) -> AgentResult<Vec<String>> {
  let mut copied = Vec::new();
  for entry in &task.spec.validation.hidden_tests {
    let destination = tree.join(hidden_destination(entry));
    if let Some(parent) = destination.parent() {
      fs::create_dir_all(parent).map_err(|error| AgentError::io(parent, error))?;
    }
    let source = task.hidden_file(entry);
    fs::copy(&source, &destination).map_err(|error| AgentError::io(&source, error))?;
    copied.push(hidden_destination(entry).to_owned());
  }
  Ok(copied)
}

pub fn head_commit(repo: &Path) -> Option<String> {
  git(repo, &["rev-parse", "HEAD"]).ok().map(|sha| sha.trim().to_owned())
}

pub(super) fn git_command(cwd: &Path) -> Command {
  let mut command = Command::new("git");
  command
    .current_dir(cwd)
    .args([
      "-c",
      "core.hooksPath=/dev/null",
      "-c",
      "commit.gpgsign=false",
      "-c",
      "tag.gpgsign=false",
      "-c",
      "core.quotePath=false",
    ])
    .args(["-c", "user.name=atlas-eval", "-c", "user.email=atlas-eval@localhost"])
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .env("GIT_AUTHOR_DATE", FIXED_DATE)
    .env("GIT_COMMITTER_DATE", FIXED_DATE)
    .env_remove("GIT_DIR")
    .env_remove("GIT_WORK_TREE")
    .env_remove("GIT_INDEX_FILE");
  command
}

fn git(cwd: &Path, args: &[&str]) -> AgentResult<String> {
  let mut command = git_command(cwd);
  command.args(args);
  let output = run_captured(command, None, GIT_TIMEOUT);
  match output.exit {
    ProcessExit::Exited { code: Some(0) } => Ok(output.stdout),
    exit => Err(AgentError::Command {
      command: format!("git {}", args.join(" ")),
      detail: format!("{exit:?}: {}", tail(output.stderr.trim(), 2000)),
    }),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
  }

  fn source_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    write(&repo.path().join("src/lib.rs"), "pub fn a() {}\n");
    write(&repo.path().join("eval/tasks/t/task.toml"), "secret");
    write(&repo.path().join("scratch-pad/notes.md"), "secret");
    write(&repo.path().join("crates/x/hidden/keep.rs"), "pub fn legit() {}\n");
    write(&repo.path().join("hidden/answer.rs"), "secret");
    write(&repo.path().join("acme-api/test/deleted-at.check.mjs"), "secret");
    write(&repo.path().join("docs/plan/implementation-plan.md"), "secret");
    write(&repo.path().join("docs/guide.md"), "keep\n");
    write(&repo.path().join("fixtures/acme/expected.schema.md"), "secret");
    write(&repo.path().join("fixtures/acme/acme-api/src/main.ts"), "main();\n");
    write(&repo.path().join("fixtures/acme/expected.toml"), "[project]\n");
    write(&repo.path().join("fixtures/acme/acme-web/expected.toml"), "[project]\n");
    git(repo.path(), &["init", "--quiet"]).unwrap();
    git(repo.path(), &["add", "--all"]).unwrap();
    git(repo.path(), &["commit", "--quiet", "-m", "init"]).unwrap();
    repo
  }

  fn files_under(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
      for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap() == ".git" {
          continue;
        }
        if path.is_dir() {
          stack.push(path);
        } else {
          found.push(path.strip_prefix(root).unwrap().display().to_string());
        }
      }
    }
    found.sort();
    found
  }

  fn plan(profile: StripProfile) -> StripPlan {
    StripPlan {
      profile,
      hidden_paths: vec!["acme-api/test/deleted-at.check.mjs".to_owned()],
    }
  }

  #[test]
  fn should_materialize_a_stripped_snapshot_without_history() {
    let repo = source_repo();
    let commit = head_commit(repo.path()).unwrap();
    let out = tempfile::tempdir().unwrap();
    let tree = out.path().join("tree");
    materialize(
      &TreeSource::GitArchive {
        repo: repo.path().to_owned(),
        commit: commit.clone(),
        subdir: None,
      },
      &tree,
      &plan(StripProfile::ThisRepository),
    )
    .unwrap();
    assert_eq!(
      files_under(&tree),
      vec![
        "crates/x/hidden/keep.rs",
        "docs/guide.md",
        "fixtures/acme/acme-api/src/main.ts",
        "fixtures/acme/acme-web/expected.toml",
        "fixtures/acme/expected.schema.md",
        "fixtures/acme/expected.toml",
        "src/lib.rs"
      ]
    );
    let base = init_base(&tree).unwrap();
    assert_ne!(base, commit);
    assert_eq!(git(&tree, &["rev-list", "--count", "HEAD"]).unwrap().trim(), "1");
  }

  #[test]
  fn should_materialize_a_fixture_subdirectory_at_the_tree_root() {
    let repo = source_repo();
    let commit = head_commit(repo.path()).unwrap();
    let out = tempfile::tempdir().unwrap();
    let tree = out.path().join("tree");
    let source = TreeSource::GitArchive {
      repo: repo.path().to_owned(),
      commit,
      subdir: Some("fixtures/acme".to_owned()),
    };
    materialize(&source, &tree, &plan(StripProfile::Fixture)).unwrap();
    assert_eq!(files_under(&tree), vec!["acme-api/src/main.ts"]);
  }

  #[test]
  fn should_copy_an_override_directory_without_git_or_build_output() {
    let repo = source_repo();
    write(&repo.path().join("target/debug/junk"), "x");
    let out = tempfile::tempdir().unwrap();
    let tree = out.path().join("tree");
    materialize(&TreeSource::Directory { path: repo.path().to_owned() }, &tree, &plan(StripProfile::External)).unwrap();
    assert_eq!(
      files_under(&tree),
      vec![
        "crates/x/hidden/keep.rs",
        "docs/guide.md",
        "docs/plan/implementation-plan.md",
        "fixtures/acme/acme-api/src/main.ts",
        "fixtures/acme/acme-web/expected.toml",
        "fixtures/acme/expected.schema.md",
        "fixtures/acme/expected.toml",
        "src/lib.rs"
      ]
    );
  }

  #[test]
  fn should_apply_a_base_patch_to_an_archived_tree_before_the_base_commit() {
    let repo = source_repo();
    let commit = head_commit(repo.path()).unwrap();
    let out = tempfile::tempdir().unwrap();
    let tree = out.path().join("tree");
    let source = TreeSource::GitArchive {
      repo: repo.path().to_owned(),
      commit,
      subdir: None,
    };
    materialize(&source, &tree, &plan(StripProfile::ThisRepository)).unwrap();
    let patch = out.path().join("base.patch");
    fs::write(&patch, "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-pub fn a() {}\n+pub fn a() -> u8 { 1 }\n").unwrap();
    apply_patch(&tree, &patch).unwrap();
    let base = init_base(&tree).unwrap();
    assert_eq!(base_file(&tree, &base, "src/lib.rs").as_deref(), Some("pub fn a() -> u8 { 1 }\n"));
    assert_eq!(diff_against(&tree, &base, &[]).unwrap(), "");
  }

  fn task_with(extra: &str, patch: Option<&str>) -> (tempfile::TempDir, LoadedTask) {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("t97-edited-tree");
    let spec =
      format!("id = \"t97-edited-tree\"\ntitle = \"Edited tree\"\nrepo = \"self\"\nbase_commit = \"abcdef1\"\nprompt = \"do it\"\ntimeout_minutes = 5\nmax_turns = 30\n{extra}\n");
    write(&dir.join("task.toml"), &spec);
    if let Some(patch) = patch {
      write(&dir.join("base.patch"), patch);
    }
    let task = super::super::spec::load_task(&dir).unwrap();
    (root, task)
  }

  #[test]
  fn should_strip_declared_paths_and_drop_declared_sections_after_the_base_patch() {
    let repo = source_repo();
    write(
      &repo.path().join("AGENTS.md"),
      "# Agents\n\n## Search\n\nUse the index daemon.\n\n## Style\n\nTwo spaces.\n",
    );
    write(&repo.path().join("suite/checkout.spec.ts"), "// the cause is in src/lib.rs\n");
    let patch = "--- a/suite/checkout.spec.ts\n+++ b/suite/checkout.spec.ts\n@@ -1 +1 @@\n-// the cause is in src/lib.rs\n+// planted\n";
    let (_root, task) = task_with(
      "base_patch = \"base.patch\"\nstrip = [\"suite\", \"docs/guide.md\"]\ndrop_instruction_sections = [\"AGENTS.md#Search\"]",
      Some(patch),
    );
    let out = tempfile::tempdir().unwrap();
    let tree = out.path().join("tree");
    let source = TreeSource::Directory { path: repo.path().to_owned() };

    materialize_task(&task, &source, &tree, &plan(StripProfile::External)).unwrap();

    let files = files_under(&tree);
    assert!(!files.iter().any(|file| file.starts_with("suite/") || file == "docs/guide.md"), "{files:?}");
    assert!(
      files.contains(&"src/lib.rs".to_owned()) && files.contains(&"docs/plan/implementation-plan.md".to_owned()),
      "{files:?}"
    );
    assert_eq!(fs::read_to_string(tree.join("AGENTS.md")).unwrap(), "# Agents\n\n## Style\n\nTwo spaces.\n");
  }

  fn linked_task(paths: &str) -> (tempfile::TempDir, LoadedTask) {
    task_with(&format!("[linked]\npaths = {paths}\nrepo = \"web\""), None)
  }

  fn linked_tree(root: &Path) -> PathBuf {
    let tree = root.join("tree");
    write(&tree.join("package.json"), "{\"workspaces\": [\"apps/*\"]}\n");
    write(&tree.join("apps/acme-api/src/main.ts"), "main();\n");
    write(&tree.join("apps/acme-web/src/client.ts"), "export const base = '/api';\n");
    write(&tree.join("packages/acme-ui/src/button.ts"), "export const button = 1;\n");
    tree
  }

  #[test]
  fn should_hide_linked_paths_from_the_base_commit_and_restore_them_after_the_session() {
    let (_task_root, task) = linked_task(r#"["apps/acme-web", "packages/acme-ui"]"#);
    let scratch = tempfile::tempdir().unwrap();
    let tree = linked_tree(scratch.path());
    let linked = LinkedLayout::of(&task, scratch.path()).unwrap();

    linked.hide(&tree).unwrap();
    let base = init_base(&tree).unwrap();

    assert_eq!(files_under(&tree), vec!["apps/acme-api/src/main.ts", "package.json"]);
    assert!(!tree.join("packages").exists(), "an ancestor emptied by the hide must go too");
    assert_eq!(git(&tree, &["status", "--porcelain", "--ignored"]).unwrap(), "");
    assert_eq!(fs::read_to_string(tree.join(".git/info/exclude")).unwrap(), "target/\nnode_modules/\n.code-atlas/\n");
    assert_eq!(
      files_under(&scratch.path().join(LINKED_HOLD)),
      vec!["apps/acme-web/src/client.ts", "packages/acme-ui/src/button.ts"]
    );

    assert_eq!(linked.restore(&tree).unwrap(), Restored::default());
    assert_eq!(
      files_under(&tree),
      vec!["apps/acme-api/src/main.ts", "apps/acme-web/src/client.ts", "package.json", "packages/acme-ui/src/button.ts"]
    );
    assert!(!scratch.path().join(LINKED_CONFLICT).exists());
    assert_eq!(diff_against(&tree, &base, &linked.spec.paths).unwrap(), "");
  }

  #[test]
  fn should_move_what_the_session_created_at_a_linked_path_aside_before_restoring() {
    let (_task_root, task) = linked_task(r#"["apps/acme-web", "packages/acme-ui"]"#);
    let scratch = tempfile::tempdir().unwrap();
    let tree = linked_tree(scratch.path());
    let linked = LinkedLayout::of(&task, scratch.path()).unwrap();
    linked.hide(&tree).unwrap();
    let base = init_base(&tree).unwrap();
    write(&tree.join("apps/acme-web/src/client.ts"), "export const base = '/guessed';\n");
    write(&tree.join("apps/acme-api/src/main.ts"), "main(1);\n");
    write(&tree.join("packages"), "not a directory\n");

    let diff = diff_against(&tree, &base, &linked.spec.paths).unwrap();
    let restored = linked.restore(&tree).unwrap();

    assert_eq!(
      super::super::diff::Diff::parse(&diff).changed_files().into_iter().collect::<Vec<_>>(),
      vec!["apps/acme-api/src/main.ts", "packages"]
    );
    assert_eq!(
      restored,
      Restored {
        conflicts: vec!["apps/acme-web".to_owned(), "packages".to_owned()],
        missing: Vec::new(),
      }
    );
    assert_eq!(fs::read_to_string(tree.join("apps/acme-web/src/client.ts")).unwrap(), "export const base = '/api';\n");
    assert_eq!(fs::read_to_string(tree.join("packages/acme-ui/src/button.ts")).unwrap(), "export const button = 1;\n");
    assert_eq!(files_under(&scratch.path().join(LINKED_CONFLICT)), vec!["apps/acme-web/src/client.ts", "packages"]);
  }

  #[cfg(unix)]
  #[test]
  fn should_never_restore_through_a_symbolic_link_the_session_planted_on_the_way() {
    let (_task_root, task) = linked_task(r#"["packages/acme-ui"]"#);
    let scratch = tempfile::tempdir().unwrap();
    let tree = linked_tree(scratch.path());
    let outside = tempfile::tempdir().unwrap();
    let linked = LinkedLayout::of(&task, scratch.path()).unwrap();
    linked.hide(&tree).unwrap();
    std::os::unix::fs::symlink(outside.path(), tree.join("packages")).unwrap();

    assert_eq!(linked.restore(&tree).unwrap().conflicts, vec!["packages"]);

    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    assert!(fs::symlink_metadata(tree.join("packages")).unwrap().is_dir());
    assert!(
      fs::symlink_metadata(scratch.path().join(LINKED_CONFLICT).join("packages"))
        .unwrap()
        .file_type()
        .is_symlink()
    );
  }

  #[test]
  fn should_report_a_linked_path_whose_held_copy_the_session_moved_away_instead_of_failing() {
    let (_task_root, task) = linked_task(r#"["apps/acme-web", "packages/acme-ui"]"#);
    let scratch = tempfile::tempdir().unwrap();
    let tree = linked_tree(scratch.path());
    let linked = LinkedLayout::of(&task, scratch.path()).unwrap();
    linked.hide(&tree).unwrap();
    fs::rename(linked.hold.join("apps/acme-web"), scratch.path().join("elsewhere")).unwrap();

    assert_eq!(
      linked.restore(&tree).unwrap(),
      Restored {
        conflicts: Vec::new(),
        missing: vec!["apps/acme-web".to_owned()],
      }
    );
    assert_eq!(files_under(&tree), vec!["apps/acme-api/src/main.ts", "package.json", "packages/acme-ui/src/button.ts"]);
  }

  #[test]
  fn should_exclude_every_view_directory_beside_the_linked_paths_and_their_ancestors() {
    let (_task_root, task) = linked_task(r#"["apps/acme-web", "packages/ui/acme-kit"]"#);
    let scratch = tempfile::tempdir().unwrap();
    let linked = LinkedLayout::of(&task, scratch.path()).unwrap();
    for file in [
      "package.json",
      "tsconfig.base.json",
      "apps/package.json",
      "apps/acme-api/package.json",
      "apps/acme-web/package.json",
      "apps/acme-web/src/a.ts",
      "apps/acme web[2]/package.json",
      "packages/acme-shared/package.json",
      "packages/ui/acme-kit/package.json",
      "packages/ui/acme-theme/package.json",
      "tools/gen/package.json",
    ] {
      write(&linked.view.join(file), "{}\n");
    }

    let excludes = linked.view_excludes().unwrap();

    assert_eq!(
      excludes,
      vec!["/tools", "/apps/acme\\ web\\[2\\]", "/apps/acme-api", "/packages/acme-shared", "/packages/ui/acme-theme"]
    );
    let globs = atlas_project::PathGlobs::new(&excludes).unwrap();
    assert!(globs.matches("apps/acme web[2]", true));
    assert!(!globs.matches("apps/acme-web", true));
    assert!(!globs.matches("packages/ui", true));
  }

  #[test]
  fn should_refuse_to_hide_a_linked_path_the_tree_does_not_have() {
    let (_task_root, task) = linked_task(r#"["apps/acme-admin"]"#);
    let scratch = tempfile::tempdir().unwrap();
    let tree = linked_tree(scratch.path());
    let hidden = LinkedLayout::of(&task, scratch.path()).unwrap().hide(&tree);
    assert!(
      matches!(&hidden, Err(AgentError::InvalidArgument(reason)) if reason == "linked path `apps/acme-admin` does not exist in the tree"),
      "{hidden:?}"
    );
  }

  #[test]
  fn should_build_the_view_from_the_same_source_with_the_shipped_patch_and_the_root_manifest() {
    let repo = source_repo();
    write(&repo.path().join("package.json"), "{}\n");
    write(&repo.path().join("apps/acme-web/src/client.ts"), "export const base = '/api';\n");
    let (_task_root, task) = task_with(
      "[linked]\npaths = [\"apps/acme-web\"]\npatch = \"base.patch\"",
      Some("--- a/apps/acme-web/src/client.ts\n+++ b/apps/acme-web/src/client.ts\n@@ -1 +1 @@\n-export const base = '/api';\n+export const base = '/api/v2';\n"),
    );
    let scratch = tempfile::tempdir().unwrap();
    let linked = LinkedLayout::of(&task, scratch.path()).unwrap();

    linked
      .materialize_view(&task, &TreeSource::Directory { path: repo.path().to_owned() }, &plan(StripProfile::External))
      .unwrap();

    assert_eq!(linked.view, scratch.path().join(LINKED_VIEW));
    assert_eq!(
      fs::read_to_string(linked.view.join("apps/acme-web/src/client.ts")).unwrap(),
      "export const base = '/api/v2';\n"
    );
    assert_eq!(fs::read_to_string(linked.view.join("package.json")).unwrap(), "{}\n");
    assert!(!linked.view.join(".git").exists());
  }

  #[test]
  fn should_fail_to_build_a_tree_whose_declared_strip_path_or_section_is_missing() {
    let repo = source_repo();
    let source = TreeSource::Directory { path: repo.path().to_owned() };
    for extra in ["strip = [\"suite\"]", "drop_instruction_sections = [\"docs/guide.md#Search\"]"] {
      let (_root, task) = task_with(extra, None);
      let out = tempfile::tempdir().unwrap();
      let built = materialize_task(&task, &source, &out.path().join("tree"), &plan(StripProfile::External));
      assert!(matches!(built, Err(AgentError::InvalidArgument(_))), "{extra}: {built:?}");
    }
  }

  #[test]
  fn should_name_the_local_directory_of_a_git_source_and_nothing_for_a_remote_or_another_kind() {
    let local = tempfile::tempdir().unwrap();
    let canonical = local.path().canonicalize().unwrap();
    let git = |url: String| RepoSource::Git {
      url,
      commit: "abcdef1".to_owned(),
    };
    let cache = canonical.join("home/runs/.cache");
    fs::create_dir_all(&cache).unwrap();
    fs::create_dir_all(canonical.join("storefront")).unwrap();
    assert_eq!(local_source_path(&git(local.path().display().to_string()), &cache), Some(canonical.clone()));
    assert_eq!(local_source_path(&git(format!("file://{}", local.path().display())), &cache), Some(canonical.clone()));
    assert_eq!(
      local_source_path(&git("/nonexistent/storefront".to_owned()), &cache),
      Some(PathBuf::from("/nonexistent/storefront"))
    );
    assert_eq!(local_source_path(&git("../../../storefront".to_owned()), &cache), Some(canonical.join("storefront")));
    assert_eq!(local_source_path(&git("absent/storefront".to_owned()), &cache), Some(cache.join("absent/storefront")));
    assert_eq!(local_source_path(&git("https://example.test/r.git".to_owned()), &cache), None);
    assert_eq!(local_source_path(&git("ssh://git@example.test/team/r.git".to_owned()), &cache), None);
    assert_eq!(local_source_path(&git("git@example.test:team/r.git".to_owned()), &cache), None);
    assert_eq!(local_source_path(&RepoSource::SelfRepo, &cache), None);
  }

  #[test]
  fn should_fail_when_a_base_patch_does_not_apply() {
    let repo = source_repo();
    let out = tempfile::tempdir().unwrap();
    let tree = out.path().join("tree");
    materialize(&TreeSource::Directory { path: repo.path().to_owned() }, &tree, &plan(StripProfile::ThisRepository)).unwrap();
    let patch = out.path().join("base.patch");
    fs::write(&patch, "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-pub fn missing() {}\n+pub fn b() {}\n").unwrap();
    assert!(matches!(apply_patch(&tree, &patch), Err(AgentError::Command { .. })));
    assert_eq!(fs::read_to_string(tree.join("src/lib.rs")).unwrap(), "pub fn a() {}\n");
  }

  #[test]
  fn should_produce_identical_base_commits_for_identical_trees() {
    let repo = source_repo();
    let commit = head_commit(repo.path()).unwrap();
    let source = TreeSource::GitArchive {
      repo: repo.path().to_owned(),
      commit,
      subdir: None,
    };
    let shas: Vec<String> = (0..2)
      .map(|_| {
        let out = tempfile::tempdir().unwrap();
        let tree = out.path().join("tree");
        materialize(&source, &tree, &plan(StripProfile::ThisRepository)).unwrap();
        init_base(&tree).unwrap()
      })
      .collect();
    assert_eq!(shas[0], shas[1]);
  }

  #[test]
  fn should_diff_working_changes_and_committed_changes_against_the_base() {
    let repo = source_repo();
    let out = tempfile::tempdir().unwrap();
    let tree = out.path().join("tree");
    materialize(&TreeSource::Directory { path: repo.path().to_owned() }, &tree, &plan(StripProfile::ThisRepository)).unwrap();
    let base = init_base(&tree).unwrap();
    write(&tree.join("src/lib.rs"), "pub fn a() {}\npub fn b() {}\n");
    git(&tree, &["commit", "--quiet", "-am", "agent commit"]).unwrap();
    write(&tree.join("src/new.rs"), "fn c() {}\n");
    write(&tree.join("target/out.o"), "binary");
    let diff = super::super::diff::Diff::parse(&diff_against(&tree, &base, &[]).unwrap());
    assert_eq!(diff.changed_files().into_iter().collect::<Vec<_>>(), vec!["src/lib.rs", "src/new.rs"]);
    assert_eq!(base_file(&tree, &base, "src/lib.rs").as_deref(), Some("pub fn a() {}\n"));
  }

  #[test]
  fn should_place_trees_at_neutral_paths_outside_any_repository() {
    let repo = source_repo();
    let scratch_root = tempfile::tempdir().unwrap();
    let batch = repo.path().join("eval/runs/2026-10-04-seed1");
    let baseline = scratch_dir(scratch_root.path(), &batch, "t00-smoke.baseline.r0");
    let atlas = scratch_dir(scratch_root.path(), &batch, "t00-smoke.atlas.r0");
    assert_ne!(baseline, atlas);
    for dir in [&baseline, &atlas] {
      let rendered = dir.join("tree").display().to_string();
      assert!(!rendered.contains("atlas") && !rendered.contains("baseline") && !rendered.contains("eval/"), "{rendered}");
      fs::create_dir_all(dir.join("tree")).unwrap();
      check_isolated_location(&dir.join("tree"), &[repo.path(), &batch]).unwrap();
      assert!(dir.join("tree").ancestors().all(|ancestor| !ancestor.join(".git").exists()));
    }
  }

  #[test]
  fn should_reject_trees_inside_a_repository_or_the_eval_home() {
    let repo = source_repo();
    let inside = repo.path().join("eval/runs/b/ae-x/tree");
    fs::create_dir_all(&inside).unwrap();
    assert!(check_isolated_location(&inside, &[]).is_err());
    let plain = tempfile::tempdir().unwrap();
    let nested = plain.path().join("home/runs/tree");
    fs::create_dir_all(&nested).unwrap();
    assert!(check_isolated_location(&nested, &[&plain.path().join("home")]).is_err());
  }
}
