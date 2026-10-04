use std::fs;
use std::path::Path;

use atlas_core::Entity;
use atlas_engine::{Engine, EngineOptions};
use atlas_project::{AtlasHome, Project, ProjectSettings, Registry};
use atlas_store::EntityFilter;

const PROJECT_LIMIT: usize = 2048;

fn padded_module(function: &str, size: usize) -> String {
  let mut text = format!("export function {function}(): number {{\n  return 1;\n}}\n");
  while text.len() < size {
    text.push_str("// padding padding padding padding padding padding padding padding\n");
  }
  text
}

fn write(path: &Path, content: &str) {
  fs::create_dir_all(path.parent().unwrap()).unwrap();
  fs::write(path, content).unwrap();
}

fn registry_text(work: &Path) -> String {
  format!(
    r#"[[project]]
name = "limits"

[project.settings]
max_file_bytes = {PROJECT_LIMIT}

[[project.repo]]
name = "roomy"
path = "{roomy}"
max_file_bytes = 65536

[[project.repo]]
name = "plain"
path = "{plain}"

[[project.repo]]
name = "tight"
path = "{tight}"
max_file_bytes = 512
"#,
    roomy = work.join("roomy").display(),
    plain = work.join("plain").display(),
    tight = work.join("tight").display(),
  )
}

fn setup(root: &Path) -> (AtlasHome, Project) {
  let work = fs::canonicalize(root).unwrap().join("work");
  write(&work.join("roomy/src/big.ts"), &padded_module("roomyHelper", 4 * PROJECT_LIMIT));
  write(&work.join("plain/src/big.ts"), &padded_module("plainHelper", 4 * PROJECT_LIMIT));
  write(&work.join("plain/src/small.ts"), &padded_module("plainSmallHelper", 100));
  write(&work.join("tight/src/medium.ts"), &padded_module("tightHelper", 1000));
  let home = AtlasHome::rooted_at(root.join("home"));
  write(&home.registry_path(), &registry_text(&work));
  let project = Registry::load(&home).unwrap().project("limits").unwrap().clone();
  (home, project)
}

fn named(engine: &Engine, name: &str, repo: &str) -> Vec<Entity> {
  let found = engine.store().read(|tx| tx.entities_by_name(name, &EntityFilter::default())).unwrap();
  found.into_iter().filter(|entity| entity.repo.as_ref().is_some_and(|owner| owner.as_str() == repo)).collect()
}

#[test]
fn should_apply_a_repo_file_size_limit_over_the_project_default_when_indexing() {
  let root = tempfile::tempdir().unwrap();
  let (home, project) = setup(root.path());
  let engine = Engine::open(&project, &home, EngineOptions::default()).unwrap();
  engine.ensure_fresh().unwrap();

  assert_eq!(named(&engine, "roomyHelper", "roomy").len(), 1, "a repo limit above the project default must admit the file");
  assert_eq!(named(&engine, "plainHelper", "plain").len(), 0, "repos without a limit keep the project default");
  assert_eq!(named(&engine, "plainSmallHelper", "plain").len(), 1);
  assert_eq!(named(&engine, "tightHelper", "tight").len(), 0, "a repo limit below the project default must skip the file");
}

#[test]
fn should_keep_applying_the_repo_limit_on_incremental_updates() {
  let root = tempfile::tempdir().unwrap();
  let (home, project) = setup(root.path());
  let engine = Engine::open(&project, &home, EngineOptions::default()).unwrap();
  engine.ensure_fresh().unwrap();

  let work = fs::canonicalize(root.path()).unwrap().join("work");
  write(&work.join("roomy/src/big.ts"), &padded_module("roomyRenamed", 5 * PROJECT_LIMIT));
  write(&work.join("roomy/src/extra.ts"), &padded_module("roomyExtra", 3 * PROJECT_LIMIT));
  write(&work.join("tight/src/medium.ts"), &padded_module("tightRenamed", 1100));
  engine.ensure_fresh().unwrap();

  assert_eq!(named(&engine, "roomyRenamed", "roomy").len(), 1);
  assert_eq!(named(&engine, "roomyHelper", "roomy").len(), 0);
  assert_eq!(named(&engine, "roomyExtra", "roomy").len(), 1);
  assert_eq!(named(&engine, "tightRenamed", "tight").len(), 0);
}

#[test]
fn should_persist_repo_limits_only_on_the_repos_that_set_them() {
  let root = tempfile::tempdir().unwrap();
  let (home, _) = setup(root.path());
  let mut registry = Registry::load(&home).unwrap();
  let settings = ProjectSettings {
    max_file_bytes: 4096,
    ..ProjectSettings::default()
  };
  registry.set_settings("limits", settings).unwrap();

  let reloaded = Registry::load(&home).unwrap();
  let project = reloaded.project("limits").unwrap();
  assert_eq!(project.settings.max_file_bytes, 4096);
  let written = fs::read_to_string(home.registry_path()).unwrap();
  assert_eq!(written.matches("max_file_bytes = 65536").count(), 1, "{written}");
  assert_eq!(written.matches("max_file_bytes = 512").count(), 1, "{written}");
  assert_eq!(written.matches("max_file_bytes").count(), 3, "{written}");
}
