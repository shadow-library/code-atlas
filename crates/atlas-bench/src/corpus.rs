use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use atlas_project::AtlasHome;

use crate::error::{BenchError, BenchResult};

const CORPORA_DIR: &str = "bench-corpora";
const COPY_EXCLUDES: [&str; 6] = ["target", "node_modules", ".git", ".worktrees", "scratch-pad", "graphify-out"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedCorpus {
  pub name: &'static str,
  pub url: &'static str,
  pub commit: &'static str,
}

pub const RIPGREP: PinnedCorpus = PinnedCorpus {
  name: "ripgrep",
  url: "https://github.com/BurntSushi/ripgrep",
  commit: "3fce3b5bb0236da2df6d99672afb8a719642eca7",
};

pub const HONO: PinnedCorpus = PinnedCorpus {
  name: "hono",
  url: "https://github.com/honojs/hono",
  commit: "f23b146afcec63606144cde50b5fbd360dd60238",
};

pub const PINNED: [PinnedCorpus; 2] = [RIPGREP, HONO];

pub fn corpora_dir(override_dir: Option<&Path>) -> BenchResult<PathBuf> {
  if let Some(dir) = override_dir {
    return Ok(dir.to_path_buf());
  }
  if let Some(root) = std::env::var_os("CODE_ATLAS_HOME").filter(|value| !value.is_empty()) {
    return Ok(PathBuf::from(root).join(CORPORA_DIR));
  }
  Ok(AtlasHome::resolve()?.data_dir().join(CORPORA_DIR))
}

fn git(dir: &Path, args: &[&str], corpus: &str) -> BenchResult<String> {
  let output = Command::new("git")
    .current_dir(dir)
    .args(args)
    .output()
    .map_err(|source| BenchError::io("spawn", Path::new("git"), source))?;
  if !output.status.success() {
    return Err(BenchError::Fetch {
      corpus: corpus.to_owned(),
      detail: format!("git {} exited with {}: {}", args.join(" "), output.status, String::from_utf8_lossy(&output.stderr).trim()),
    });
  }
  Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub fn checked_out_commit(dir: &Path) -> Option<String> {
  dir.join(".git").exists().then(|| git(dir, &["rev-parse", "HEAD"], "probe").ok()).flatten()
}

pub fn fetch(corpus: &PinnedCorpus, base: &Path) -> BenchResult<PathBuf> {
  let target = base.join(corpus.name);
  if checked_out_commit(&target).as_deref() == Some(corpus.commit) {
    return Ok(target);
  }
  let staging = base.join(format!("{}.partial", corpus.name));
  for stale in [&staging, &target] {
    if stale.exists() {
      fs::remove_dir_all(stale).map_err(|source| BenchError::io("remove", stale, source))?;
    }
  }
  fs::create_dir_all(&staging).map_err(|source| BenchError::io("create", &staging, source))?;
  let outcome = (|| {
    git(&staging, &["init", "--quiet"], corpus.name)?;
    git(&staging, &["remote", "add", "origin", corpus.url], corpus.name)?;
    git(&staging, &["fetch", "--quiet", "--depth", "1", "--no-tags", "origin", corpus.commit], corpus.name)?;
    git(&staging, &["checkout", "--quiet", "FETCH_HEAD"], corpus.name)
  })();
  if let Err(error) = outcome {
    let _ = fs::remove_dir_all(&staging);
    return Err(error);
  }
  fs::rename(&staging, &target).map_err(|source| BenchError::io("rename", &staging, source))?;
  Ok(target)
}

pub fn copy_tree(from: &Path, to: &Path) -> BenchResult<()> {
  fs::create_dir_all(to).map_err(|source| BenchError::io("create", to, source))?;
  for entry in fs::read_dir(from).map_err(|source| BenchError::io("read", from, source))? {
    let entry = entry.map_err(|source| BenchError::io("read", from, source))?;
    let name = entry.file_name();
    if name.to_str().is_some_and(|name| COPY_EXCLUDES.contains(&name)) {
      continue;
    }
    let target = to.join(&name);
    let kind = entry.file_type().map_err(|source| BenchError::io("stat", &entry.path(), source))?;
    if kind.is_dir() {
      copy_tree(&entry.path(), &target)?;
    } else if kind.is_file() {
      fs::copy(entry.path(), &target).map_err(|source| BenchError::io("copy", &entry.path(), source))?;
    }
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_pin_full_length_commits() {
    for corpus in PINNED {
      assert_eq!(corpus.commit.len(), 40, "{}", corpus.name);
      assert!(corpus.commit.chars().all(|c| c.is_ascii_hexdigit()));
    }
  }

  #[test]
  fn should_copy_trees_without_excluded_directories() {
    let scratch = tempfile::tempdir().unwrap();
    let from = scratch.path().join("from");
    fs::create_dir_all(from.join("src")).unwrap();
    fs::create_dir_all(from.join("target/debug")).unwrap();
    fs::create_dir_all(from.join(".git")).unwrap();
    fs::write(from.join("src/a.rs"), "fn a() {}").unwrap();
    fs::write(from.join("target/debug/x"), "x").unwrap();
    fs::write(from.join(".git/HEAD"), "ref").unwrap();
    let to = scratch.path().join("to");
    copy_tree(&from, &to).unwrap();
    assert!(to.join("src/a.rs").exists());
    assert!(!to.join("target").exists());
    assert!(!to.join(".git").exists());
  }

  #[test]
  fn should_prefer_an_explicit_corpora_directory() {
    assert_eq!(corpora_dir(Some(Path::new("/x/y"))).unwrap(), PathBuf::from("/x/y"));
  }
}
