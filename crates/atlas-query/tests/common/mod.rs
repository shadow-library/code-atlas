#![allow(dead_code)]

pub mod truth;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atlas_core::{ProjectName, RepoName};
use atlas_engine::{Engine, EngineOptions};
use atlas_project::{AtlasHome, Project, ProjectSettings, RepoConfig};
use atlas_query::{QueryEngine, RenderText, Response, render_text};
use tempfile::TempDir;

pub const ACME_REPOS: [&str; 4] = ["acme-api", "acme-sdk", "acme-web", "acme-billing"];
pub const GLOBEX_REPOS: [&str; 2] = ["globex-api", "globex-worker"];

pub struct Fixture {
  pub scratch: TempDir,
  pub project: Project,
  pub query: QueryEngine,
}

impl Fixture {
  pub fn repo_root(&self, repo: &str) -> PathBuf {
    self.scratch.path().join("tree").join(repo)
  }

  pub fn text<T: RenderText>(&self, response: &Response<T>) -> String {
    normalise(&render_text(response))
  }
}

pub fn fixtures_root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

pub fn copy_dir(from: &Path, to: &Path) {
  fs::create_dir_all(to).unwrap();
  for entry in fs::read_dir(from).unwrap() {
    let entry = entry.unwrap();
    let target = to.join(entry.file_name());
    if entry.file_type().unwrap().is_dir() {
      copy_dir(&entry.path(), &target);
    } else {
      fs::copy(entry.path(), &target).unwrap();
    }
  }
}

pub fn engine_options() -> EngineOptions {
  EngineOptions::default()
}

pub fn fixture(group: &str, repos: &[&str]) -> Fixture {
  fixture_with(group, repos, engine_options())
}

pub fn fixture_with(group: &str, repos: &[&str], options: EngineOptions) -> Fixture {
  let scratch = tempfile::tempdir().unwrap();
  let configs = repos
    .iter()
    .map(|name| {
      let target = scratch.path().join("tree").join(name);
      copy_dir(&fixtures_root().join(group).join(name), &target);
      RepoConfig {
        name: RepoName::new(*name).unwrap(),
        path: target,
        include: Vec::new(),
        exclude: Vec::new(),
        languages: None,
      }
    })
    .collect();
  let project = Project {
    name: ProjectName::new(group).unwrap(),
    settings: ProjectSettings::default(),
    repos: configs,
  };
  let engine = Engine::open(&project, &AtlasHome::rooted_at(scratch.path().join("home")), options).unwrap();
  let query = QueryEngine::new(Arc::new(engine));
  Fixture { scratch, project, query }
}

pub fn single_repo(name: &str, files: &[(&str, &str)]) -> Fixture {
  single_repo_with(name, files, engine_options())
}

pub fn single_repo_with(name: &str, files: &[(&str, &str)], options: EngineOptions) -> Fixture {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("tree").join(name);
  for (path, content) in files {
    let target = root.join(path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, content).unwrap();
  }
  let project = Project {
    name: ProjectName::new(name).unwrap(),
    settings: ProjectSettings::default(),
    repos: vec![RepoConfig {
      name: RepoName::new(name).unwrap(),
      path: root,
      include: Vec::new(),
      exclude: Vec::new(),
      languages: None,
    }],
  };
  let engine = Engine::open(&project, &AtlasHome::rooted_at(scratch.path().join("home")), options).unwrap();
  let query = QueryEngine::new(Arc::new(engine));
  Fixture { scratch, project, query }
}

pub fn acme() -> Fixture {
  fixture("acme", &ACME_REPOS)
}

pub fn globex() -> Fixture {
  fixture("globex", &GLOBEX_REPOS)
}

pub fn normalise(text: &str) -> String {
  let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
  if let Some(first) = lines.first_mut() {
    *first = strip_duration(first);
  }
  let mut joined = lines.join("\n");
  joined.push('\n');
  joined
}

fn strip_duration(line: &str) -> String {
  let Some(end) = line.find("ms)") else { return line.to_owned() };
  let start = line[..end].rfind(", ").map_or(end, |index| index + 2);
  format!("{}<t>{}", &line[..start], &line[end + 2..])
}
