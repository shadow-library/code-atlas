#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

use atlas_core::{ContentHash, ProjectName, RelPath, RepoName};
use atlas_edit::{ApplyOptions, EditScope, FileTarget};
use atlas_project::{Project, ProjectSettings, RepoConfig};
use tempfile::TempDir;

pub struct Workspace {
  _dir: Option<TempDir>,
  pub base: PathBuf,
  pub project: Project,
}

impl Workspace {
  pub fn new(repos: &[&str]) -> Self {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    Self::build(Some(dir), base, repos)
  }

  pub fn at(base: PathBuf, repos: &[&str]) -> Self {
    Self::build(None, fs::canonicalize(base).unwrap(), repos)
  }

  fn build(dir: Option<TempDir>, base: PathBuf, repos: &[&str]) -> Self {
    let repos = repos
      .iter()
      .map(|name| {
        let path = base.join(name);
        fs::create_dir_all(&path).unwrap();
        RepoConfig {
          name: RepoName::new(*name).unwrap(),
          path,
          include: Vec::new(),
          exclude: Vec::new(),
          languages: None,
        }
      })
      .collect();
    Self {
      _dir: dir,
      base,
      project: Project {
        name: ProjectName::new("demo").unwrap(),
        settings: ProjectSettings::default(),
        repos,
      },
    }
  }

  pub fn root(&self, repo: &str) -> PathBuf {
    self.project.repo(repo).unwrap().path.clone()
  }

  pub fn path(&self, repo: &str, relative: &str) -> PathBuf {
    self.root(repo).join(relative)
  }

  pub fn write(&self, repo: &str, relative: &str, content: impl AsRef<[u8]>) -> ContentHash {
    let path = self.path(repo, relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content.as_ref()).unwrap();
    ContentHash::of(content.as_ref())
  }

  pub fn read(&self, repo: &str, relative: &str) -> String {
    fs::read_to_string(self.path(repo, relative)).unwrap()
  }

  pub fn exists(&self, repo: &str, relative: &str) -> bool {
    fs::symlink_metadata(self.path(repo, relative)).is_ok()
  }

  pub fn scope(&self) -> EditScope {
    EditScope::new(&self.project).unwrap()
  }

  pub fn options(&self) -> ApplyOptions {
    ApplyOptions::new(self.base.join("project.db.edit.lock"))
  }

  pub fn outside(&self, relative: &str) -> PathBuf {
    let path = self.base.join("outside").join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    path
  }
}

pub fn target(repo: &str, path: &str) -> FileTarget {
  FileTarget::new(RepoName::new(repo).unwrap(), RelPath::new(path).unwrap())
}

pub fn hash(content: &str) -> ContentHash {
  ContentHash::of(content.as_bytes())
}

pub fn edited(content: &str, start: usize, end: usize, new_text: &str) -> String {
  format!("{}{new_text}{}", &content[..start], &content[end..])
}
