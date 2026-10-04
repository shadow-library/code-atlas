mod common;

use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{ContentHash, Owner};
use atlas_engine::{PASS_STRUCTURE, STRUCTURE_VERSION};
use atlas_project::Project;
use common::{copy_dir, digest, fixtures_root, fresh, open, project, repo_config};

const PINNED_STRUCTURE: (u32, &str, &str) = (
  1,
  "ccb59dc4905a89a4b37a014a0742818b0333cdf087b3c3c6648766ddcfd501b5",
  "154ab9895486141503ebfd5361e5195b68da2f17c9654e379a55a93cba4757a2",
);
const PINNED_LINK: (u32, &str, &str) = (
  2,
  "2dc0fa76d4849a9f179c5ceb4fd8413d5dc24527107d49d372018fafa853e94d",
  "476e5950ca7f5615d7ccaf30c41dd5edd90b56001c87e75696cf627804351ac3",
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

fn canonical_owner(facts: &atlas_store::OwnedFacts) -> Vec<u8> {
  let mut lines: Vec<String> = facts.entities.iter().map(|entity| serde_json::to_string(entity).unwrap()).collect();
  lines.extend(facts.edges.iter().map(|edge| serde_json::to_string(edge).unwrap()));
  lines.sort();
  lines.join("\n").into_bytes()
}

fn pass_digests(scratch: &Path) -> (PassDigests, PassDigests) {
  let mut structure = PassDigests::default();
  let mut link = PassDigests::default();
  for (index, project) in fixture_projects(&scratch.join("tree")).iter().enumerate() {
    let engine = open(project, &scratch.join(format!("home-{index}")));
    assert!(fresh(&engine).is_fresh());
    for (digests, pass) in [(&mut link, atlas_link::PASS_NAME), (&mut structure, PASS_STRUCTURE)] {
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
