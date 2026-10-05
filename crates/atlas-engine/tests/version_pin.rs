mod common;

use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{ContentHash, EdgeKind, Owner};
use atlas_engine::{PASS_STRUCTURE, STRUCTURE_VERSION};
use atlas_project::Project;
use common::{copy_dir, digest, fixtures_root, fresh, open, project, repo_config};

const PINNED_STRUCTURE: (u32, &str, &str) = (
  1,
  "7461d7a375ce2f76f0c56ce263195bb6c73b4ab8e64bdf1b70f7df25aa3cc2bf",
  "ec3f51b2a78d7e98e15aa5e2c7e63822eeeec8282aac7b8cc0ed688a426f1776",
);
const PINNED_LINK: (u32, &str, &str) = (
  2,
  "85068879e6d39604a2f62c5e8febd41b3b674878a3d645e2f135f3067e74a201",
  "1d2146cb0db58075b6fade03c2ac72939bb4a26db99289dbc907291cbe72fb22",
);

#[derive(Default)]
struct PassDigests {
  inputs: Vec<u8>,
  output: Vec<u8>,
}

fn walk_dirs(dir: &Path) -> Vec<PathBuf> {
  let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
    .unwrap()
    .flatten()
    .filter(|entry| entry.file_type().unwrap().is_dir())
    .map(|entry| entry.path())
    .collect();
  dirs.sort();
  dirs
}

fn fixture_projects(tree: &Path) -> Vec<Project> {
  let mut projects = Vec::new();
  for group in walk_dirs(&fixtures_root()) {
    let group_name = group.file_name().unwrap().to_string_lossy().into_owned();
    let repos: Vec<(String, PathBuf)> = walk_dirs(&group)
      .into_iter()
      .map(|source| {
        let name = source.file_name().unwrap().to_string_lossy().into_owned();
        let target = tree.join(&group_name).join(&name);
        copy_dir(&source, &target);
        (name, target)
      })
      .collect();
    match group_name == "adversarial" {
      true => projects.extend(repos.iter().map(|(name, path)| project(name, vec![repo_config(name, path)]))),
      false => projects.push(project(&group_name, repos.iter().map(|(name, path)| repo_config(name, path)).collect())),
    }
  }
  projects
}

fn link_corpus_project(tree: &Path) -> Project {
  let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pin-corpus/link");
  let repos = walk_dirs(&root)
    .into_iter()
    .map(|source| {
      let name = source.file_name().unwrap().to_string_lossy().into_owned();
      let target = tree.join("link-corpus").join(&name);
      copy_dir(&source, &target);
      repo_config(&name, &target)
    })
    .collect();
  project("link-corpus", repos)
}

fn canonical_owner(facts: &atlas_store::OwnedFacts) -> Vec<u8> {
  let mut lines: Vec<String> = facts.entities.iter().map(|entity| serde_json::to_string(entity).unwrap()).collect();
  lines.extend(facts.edges.iter().map(|edge| serde_json::to_string(edge).unwrap()));
  lines.sort();
  lines.join("\n").into_bytes()
}

fn pass_digests(scratch: &Path) -> (PassDigests, PassDigests) {
  let mut structure = PassDigests::default();
  let mut link = PassDigests::default();
  let tree = scratch.join("tree");
  let mut projects: Vec<(Project, bool)> = fixture_projects(&tree).into_iter().map(|project| (project, true)).collect();
  projects.push((link_corpus_project(&tree), false));
  for (index, (project, with_structure)) in projects.iter().enumerate() {
    let engine = open(project, &scratch.join(format!("home-{index}")));
    assert!(fresh(&engine).is_fresh());
    let mut passes = vec![(&mut link, atlas_link::PASS_NAME)];
    if *with_structure {
      passes.push((&mut structure, PASS_STRUCTURE));
    }
    for (digests, pass) in passes {
      let owner = Owner::Project { pass: pass.to_owned() };
      let facts = engine.store().read(|tx| tx.owned_facts(&owner)).unwrap();
      digests.output.extend(project.name.as_str().as_bytes());
      digests.output.extend(canonical_owner(&facts));
      engine.store().write(|tx| tx.remove_owner(&owner)).unwrap();
      digests.inputs.extend(digest(&engine).as_bytes());
    }
  }
  (structure, link)
}

fn assert_pinned(constant: &str, pinned: (u32, &str, &str), version: u32, digests: &PassDigests) {
  let inputs = ContentHash::of(&digests.inputs).to_string();
  let output = ContentHash::of(&digests.output).to_string();
  if (version, inputs.as_str(), output.as_str()) == pinned {
    return;
  }
  let advice = match (inputs == pinned.1, version == pinned.0) {
    (false, _) => format!("the facts this pass reads changed (fixtures or an upstream producer), so re-pin; if the pass's code changed as well, also bump {constant}"),
    (true, true) => format!("the pass output over unchanged inputs changed: bump {constant} so the engine re-runs the passes on stored indexes, then re-pin"),
    (true, false) => format!("{constant} changed: re-pin"),
  };
  panic!("{advice}\nnew pin: ({version}, \"{inputs}\", \"{output}\")");
}

#[test]
fn should_change_the_structure_and_link_pass_output_over_the_fixtures_only_with_a_version_bump() {
  let scratch = tempfile::tempdir().unwrap();
  let (structure, link) = pass_digests(scratch.path());
  assert_pinned("atlas_engine::STRUCTURE_VERSION", PINNED_STRUCTURE, STRUCTURE_VERSION, &structure);
  assert_pinned("atlas_link::LINK_VERSION", PINNED_LINK, atlas_link::LINK_VERSION, &link);
}

#[test]
fn should_derive_auth_usage_in_the_link_pin_corpus_from_production_code_only() {
  let scratch = tempfile::tempdir().unwrap();
  let engine = open(&link_corpus_project(scratch.path()), &scratch.path().join("home"));
  assert!(fresh(&engine).is_fresh());
  let owner = Owner::Project {
    pass: atlas_link::PASS_NAME.to_owned(),
  };
  let users = engine
    .store()
    .read(|tx| -> Result<Vec<String>, atlas_store::StoreError> {
      let mut names = Vec::new();
      for edge in tx.owned_facts(&owner)?.edges.iter().filter(|edge| edge.kind == EdgeKind::UsesAuth) {
        names.extend(tx.entity(edge.from)?.map(|entity| entity.name));
      }
      names.sort();
      Ok(names)
    })
    .unwrap();
  assert_eq!(users, vec!["loadMe".to_owned(), "load_order".to_owned()]);
}
