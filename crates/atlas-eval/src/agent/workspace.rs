use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::error::{AgentError, AgentResult};
use super::process::{ProcessExit, run_captured, tail};
use super::spec::{HIDDEN_DIR, LoadedTask, RepoSource, hidden_destination};

const GIT_TIMEOUT: Duration = Duration::from_secs(600);
const STRIPPED_ROOT_DIRS: [&str; 3] = ["eval", "scratch-pad", HIDDEN_DIR];
const REPOSITORY_ONLY_DIRS: [&str; 1] = ["docs/plan"];
const GROUND_TRUTH_FILES: [&str; 2] = ["expected.toml", "expected.schema.md"];
const SCRATCH_PREFIX: &str = "ae-";
const COPY_EXCLUDED_DIRS: [&str; 3] = [".git", "target", "node_modules"];
const LOCAL_EXCLUDES: &str = "target/\nnode_modules/\n.code-atlas/\n";
const BASE_TAG: &str = "eval-base";
const FIXED_DATE: &str = "2000-01-01T00:00:00Z";

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

fn copy_dir(from: &Path, to: &Path) -> AgentResult<()> {
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

pub fn diff_against(tree: &Path, base: &str) -> AgentResult<String> {
  git(tree, &["add", "--all"])?;
  git(tree, &["diff", "--cached", "--no-color", "--no-ext-diff", "--find-renames", base])
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

fn git_command(cwd: &Path) -> Command {
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
    assert_eq!(diff_against(&tree, &base).unwrap(), "");
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
    let diff = super::super::diff::Diff::parse(&diff_against(&tree, &base).unwrap());
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
