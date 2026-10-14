use std::fs::{self, File};
use std::io::{self, ErrorKind, Read};
use std::path::{Path, PathBuf};

const GIT_ENTRY: &str = ".git";
const GITDIR_PREFIX: &str = "gitdir:";
const COMMONDIR_FILE: &str = "commondir";
const WORKTREES_DIR: &str = "worktrees";
const MAX_POINTER_BYTES: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutKind {
  Primary,
  LinkedWorktree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCheckout {
  pub root: PathBuf,
  pub common_dir: PathBuf,
  pub kind: CheckoutKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitLocation {
  Outside,
  Checkout(GitCheckout),
  Unresolved { root: PathBuf },
}

impl GitCheckout {
  pub fn find(directory: &Path) -> Option<Self> {
    match Self::locate(directory) {
      GitLocation::Checkout(checkout) => Some(checkout),
      GitLocation::Outside | GitLocation::Unresolved { .. } => None,
    }
  }

  pub fn locate(directory: &Path) -> GitLocation {
    let Ok(start) = fs::canonicalize(directory) else { return GitLocation::Outside };
    for candidate in start.ancestors() {
      let entry = candidate.join(GIT_ENTRY);
      let checkout = match fs::metadata(&entry) {
        Ok(metadata) if metadata.is_dir() => primary(candidate, &entry),
        Ok(metadata) if metadata.is_file() => from_pointer(candidate, &entry),
        Ok(_) => None,
        Err(error) if error.kind() == ErrorKind::NotFound => continue,
        Err(error) => {
          tracing::debug!(path = %entry.display(), error = %error, "cannot inspect the git entry");
          None
        }
      };
      return checkout.map_or_else(|| GitLocation::Unresolved { root: candidate.to_path_buf() }, GitLocation::Checkout);
    }
    GitLocation::Outside
  }

  pub fn main_root(&self) -> Option<&Path> {
    match self.common_dir.file_name() {
      Some(name) if name == GIT_ENTRY => self.common_dir.parent(),
      _ => None,
    }
  }

  pub fn is_other_checkout_of(&self, other: &GitCheckout) -> bool {
    self.common_dir == other.common_dir && self.root != other.root
  }
}

fn primary(root: &Path, git_dir: &Path) -> Option<GitCheckout> {
  Some(GitCheckout {
    root: root.to_path_buf(),
    common_dir: fs::canonicalize(git_dir).ok()?,
    kind: CheckoutKind::Primary,
  })
}

fn from_pointer(root: &Path, pointer: &Path) -> Option<GitCheckout> {
  let text = read_small(pointer).ok()??;
  let target = text.lines().next()?.strip_prefix(GITDIR_PREFIX)?.trim();
  if target.is_empty() {
    return None;
  }
  let git_dir = fs::canonicalize(root.join(target)).ok()?;
  let admin_parent = git_dir.parent().filter(|parent| parent.file_name().is_some_and(|name| name == WORKTREES_DIR));
  let common_dir = match read_small(&git_dir.join(COMMONDIR_FILE)).ok()? {
    Some(common) if common.trim().is_empty() => return None,
    Some(common) => fs::canonicalize(git_dir.join(common.trim())).ok()?,
    None => match admin_parent {
      Some(parent) => parent.parent()?.to_path_buf(),
      None => git_dir.clone(),
    },
  };
  let kind = match admin_parent == Some(common_dir.join(WORKTREES_DIR).as_path()) {
    true => CheckoutKind::LinkedWorktree,
    false => CheckoutKind::Primary,
  };
  Some(GitCheckout {
    root: root.to_path_buf(),
    common_dir,
    kind,
  })
}

fn read_small(path: &Path) -> io::Result<Option<String>> {
  let file = match File::open(path) {
    Ok(file) => file,
    Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
    Err(error) => return Err(error),
  };
  let mut text = String::new();
  file.take(MAX_POINTER_BYTES).read_to_string(&mut text)?;
  Ok(Some(text))
}

#[cfg(test)]
pub(crate) mod git_fixture {
  use std::fs;
  use std::path::{Path, PathBuf};
  use std::process::Command;

  pub(crate) fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
      .args(args)
      .current_dir(cwd)
      .env("GIT_CONFIG_GLOBAL", "/dev/null")
      .env("GIT_CONFIG_NOSYSTEM", "1")
      .env("GIT_AUTHOR_NAME", "atlas")
      .env("GIT_AUTHOR_EMAIL", "atlas@example.invalid")
      .env("GIT_COMMITTER_NAME", "atlas")
      .env("GIT_COMMITTER_EMAIL", "atlas@example.invalid")
      .output()
      .unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
  }

  pub(crate) fn repository(parent: &Path, name: &str) -> PathBuf {
    let main = parent.join(name);
    fs::create_dir_all(main.join("src")).unwrap();
    fs::write(main.join("src/lib.rs"), "pub fn answer() -> u8 {\n  42\n}\n").unwrap();
    git(&main, &["init", "--quiet", "--initial-branch=trunk"]);
    git(&main, &["add", "."]);
    git(&main, &["commit", "--quiet", "--message", "seed"]);
    main
  }
}

#[cfg(test)]
mod tests {
  use tempfile::TempDir;

  use super::git_fixture::git;
  use super::*;

  fn repository(root: &Path) -> PathBuf {
    super::git_fixture::repository(root, "main")
  }

  struct Scratch {
    _dir: TempDir,
    root: PathBuf,
  }

  impl Scratch {
    fn new() -> Self {
      let dir = tempfile::tempdir().unwrap();
      let root = fs::canonicalize(dir.path()).unwrap();
      Self { _dir: dir, root }
    }
  }

  #[test]
  fn should_find_the_primary_checkout_from_a_nested_directory() {
    let scratch = Scratch::new();
    let main = repository(&scratch.root);

    let checkout = GitCheckout::find(&main.join("src")).unwrap();

    assert_eq!(
      checkout,
      GitCheckout {
        root: main.clone(),
        common_dir: main.join(".git"),
        kind: CheckoutKind::Primary,
      }
    );
    assert_eq!(checkout.main_root(), Some(main.as_path()));
  }

  #[test]
  fn should_detect_a_worktree_inside_the_repository_and_name_its_main_checkout() {
    let scratch = Scratch::new();
    let main = repository(&scratch.root);
    git(&main, &["worktree", "add", "--quiet", "-b", "feature", ".worktrees/feature"]);
    let worktree = main.join(".worktrees/feature");

    let checkout = GitCheckout::find(&worktree.join("src")).unwrap();

    assert_eq!(
      checkout,
      GitCheckout {
        root: worktree,
        common_dir: main.join(".git"),
        kind: CheckoutKind::LinkedWorktree,
      }
    );
    assert_eq!(checkout.main_root(), Some(main.as_path()));
    assert!(checkout.is_other_checkout_of(&GitCheckout::find(&main).unwrap()));
    assert!(!checkout.is_other_checkout_of(&checkout.clone()));
  }

  #[test]
  fn should_detect_a_worktree_outside_the_repository() {
    let scratch = Scratch::new();
    let main = repository(&scratch.root);
    git(&main, &["worktree", "add", "--quiet", "-b", "outside", "../outside"]);

    let checkout = GitCheckout::find(&scratch.root.join("outside")).unwrap();

    assert_eq!(checkout.root, scratch.root.join("outside"));
    assert_eq!(checkout.kind, CheckoutKind::LinkedWorktree);
    assert_eq!(checkout.main_root(), Some(main.as_path()));
  }

  #[test]
  fn should_resolve_a_relative_gitdir_and_a_relative_commondir() {
    let scratch = Scratch::new();
    let common = scratch.root.join("repo/.git");
    let admin = common.join("worktrees/wt");
    fs::create_dir_all(&admin).unwrap();
    fs::write(admin.join(COMMONDIR_FILE), "../..\n").unwrap();
    let worktree = scratch.root.join("repo/.claude/worktrees/wt");
    fs::create_dir_all(worktree.join("deep")).unwrap();
    fs::write(worktree.join(GIT_ENTRY), "gitdir: ../../../.git/worktrees/wt\n").unwrap();

    let checkout = GitCheckout::find(&worktree.join("deep")).unwrap();

    assert_eq!(
      checkout,
      GitCheckout {
        root: worktree,
        common_dir: common,
        kind: CheckoutKind::LinkedWorktree,
      }
    );
    assert_eq!(checkout.main_root(), Some(scratch.root.join("repo").as_path()));
  }

  #[test]
  fn should_not_treat_a_malformed_or_dangling_git_file_as_a_checkout() {
    let scratch = Scratch::new();
    let worktree = scratch.root.join("wt");
    fs::create_dir_all(&worktree).unwrap();
    let cases = ["", "not a pointer\n", "gitdir:\n", "gitdir: ../missing/.git/worktrees/wt\n", "\u{0}\u{1}gitdir"];

    let found: Vec<(Option<GitCheckout>, GitLocation)> = cases
      .iter()
      .map(|text| {
        fs::write(worktree.join(GIT_ENTRY), text).unwrap();
        (GitCheckout::find(&worktree), GitCheckout::locate(&worktree))
      })
      .collect();

    assert_eq!(found, vec![(None, GitLocation::Unresolved { root: worktree.clone() }); cases.len()]);
  }

  #[test]
  fn should_fall_back_to_the_grandparent_of_a_worktree_admin_dir_without_a_commondir() {
    let scratch = Scratch::new();
    let main = repository(&scratch.root);
    git(&main, &["worktree", "add", "--quiet", "-b", "feature", ".worktrees/feature"]);
    fs::remove_file(main.join(".git/worktrees/feature").join(COMMONDIR_FILE)).unwrap();

    assert_eq!(
      GitCheckout::find(&main.join(".worktrees/feature")),
      Some(GitCheckout {
        root: main.join(".worktrees/feature"),
        common_dir: main.join(".git"),
        kind: CheckoutKind::LinkedWorktree,
      })
    );
  }

  #[test]
  fn should_not_treat_a_worktree_with_an_unreadable_commondir_as_a_worktree() {
    let scratch = Scratch::new();
    let admin = scratch.root.join("repo/.git/worktrees/wt");
    fs::create_dir_all(&admin).unwrap();
    let worktree = scratch.root.join("wt");
    fs::create_dir_all(&worktree).unwrap();
    fs::write(worktree.join(GIT_ENTRY), format!("gitdir: {}\n", admin.display())).unwrap();
    fs::write(admin.join(COMMONDIR_FILE), "../../../nowhere\n").unwrap();

    assert_eq!(GitCheckout::locate(&worktree), GitLocation::Unresolved { root: worktree.clone() });
  }

  #[test]
  fn should_treat_a_submodule_shaped_gitdir_as_its_own_primary_checkout() {
    let scratch = Scratch::new();
    let modules = scratch.root.join("super/.git/modules/vendor");
    fs::create_dir_all(&modules).unwrap();
    let submodule = scratch.root.join("super/vendor");
    fs::create_dir_all(&submodule).unwrap();
    fs::write(submodule.join(GIT_ENTRY), "gitdir: ../.git/modules/vendor\n").unwrap();
    let primary = GitCheckout {
      root: submodule.clone(),
      common_dir: modules.clone(),
      kind: CheckoutKind::Primary,
    };

    let without_commondir = GitCheckout::find(&submodule).unwrap();
    fs::write(modules.join(COMMONDIR_FILE), ".\n").unwrap();
    let with_commondir = GitCheckout::find(&submodule).unwrap();

    assert_eq!(without_commondir, primary);
    assert_eq!(with_commondir, primary);
    assert_eq!(primary.main_root(), None);
  }

  #[test]
  fn should_canonicalise_a_symlinked_worktree_path() {
    let scratch = Scratch::new();
    let main = repository(&scratch.root);
    git(&main, &["worktree", "add", "--quiet", "-b", "linked", ".worktrees/linked"]);
    let alias = scratch.root.join("alias");
    std::os::unix::fs::symlink(main.join(".worktrees/linked"), &alias).unwrap();

    let checkout = GitCheckout::find(&alias.join("src")).unwrap();

    assert_eq!(checkout.root, main.join(".worktrees/linked"));
    assert_eq!(checkout.common_dir, main.join(".git"));
    assert_eq!(checkout.kind, CheckoutKind::LinkedWorktree);
  }

  #[test]
  fn should_find_nothing_outside_any_repository() {
    let scratch = Scratch::new();
    let plain = scratch.root.join("plain/nested");
    fs::create_dir_all(&plain).unwrap();

    assert_eq!(GitCheckout::locate(&plain), GitLocation::Outside);
  }
}
