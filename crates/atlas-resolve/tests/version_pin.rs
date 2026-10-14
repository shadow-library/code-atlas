mod common;

use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::ContentHash;
use atlas_project::is_manifest_name;
use atlas_resolve::{RESOLVER_VERSION, manifest_edges};
use common::{Project, fixtures_root};
use serde::Serialize;

const PINNED: (u32, &str, &str) = (
  7,
  "a8b5ed46f5494c01cc0264ba1f68e08a2886b3e974186bf78c2e77af9b905cd2",
  "6c7d2284af361a2c31f7e61fe81894441f2cd7b77a2530ecb02b7dff214393c2",
);

fn walk(dir: &Path) -> Vec<PathBuf> {
  let mut files = Vec::new();
  let mut stack = vec![dir.to_path_buf()];
  while let Some(dir) = stack.pop() {
    for entry in fs::read_dir(&dir).unwrap().flatten() {
      match entry.file_type().unwrap().is_dir() {
        true => stack.push(entry.path()),
        false => files.push(entry.path()),
      }
    }
  }
  files.sort();
  files
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

fn corpus_root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pin-corpus")
}

fn named_dirs(dir: &Path) -> Vec<(String, PathBuf)> {
  walk_dirs(dir)
    .into_iter()
    .map(|dir| (dir.file_name().unwrap().to_string_lossy().into_owned(), dir))
    .collect()
}

fn load(repos: &[(String, PathBuf)]) -> Project {
  Project::load(&repos.iter().map(|(name, dir)| (name.as_str(), dir.clone())).collect::<Vec<_>>())
}

fn pinned_projects() -> Vec<Project> {
  let root = fixtures_root();
  let mut projects = Vec::new();
  for group in walk_dirs(&root) {
    let repos = named_dirs(&group);
    match group.file_name().is_some_and(|name| name == "adversarial") {
      true => projects.extend(repos.into_iter().map(|repo| load(&[repo]))),
      false => projects.push(load(&repos)),
    }
  }
  projects.push(load(&named_dirs(&corpus_root())));
  projects
}

fn sorted_json<T: Serialize>(items: &[T]) -> Vec<String> {
  let mut lines: Vec<String> = items.iter().map(|item| serde_json::to_string(item).unwrap()).collect();
  lines.sort();
  lines
}

fn inputs_digest(projects: &[Project]) -> String {
  let mut bytes = Vec::new();
  for source in projects.iter().flat_map(|project| &project.repos) {
    for file in walk(&source.root) {
      let name = file.file_name().unwrap().to_string_lossy();
      if !is_manifest_name(&name) {
        continue;
      }
      bytes.extend(source.name.as_str().as_bytes());
      bytes.push(0);
      bytes.extend(file.strip_prefix(&source.root).unwrap().to_string_lossy().replace('\\', "/").as_bytes());
      bytes.push(0);
      bytes.extend(ContentHash::of(&fs::read(&file).unwrap()).as_bytes());
    }
    for (facts, context) in &source.files {
      bytes.extend(serde_json::to_vec(facts).unwrap());
      bytes.extend(serde_json::to_vec(context).unwrap());
      bytes.push(b'\n');
    }
  }
  ContentHash::of(&bytes).to_string()
}

fn output_digest(projects: &[Project]) -> String {
  let mut bytes = Vec::new();
  for project in projects {
    for ((repo, path), resolved) in &project.results {
      let canonical = serde_json::json!({
        "file": [repo, path],
        "module_path_override": resolved.module_path_override,
        "package_override": resolved.package_override,
        "reachability": format!("{:?}", resolved.reachability),
        "edges": sorted_json(&resolved.edges),
        "unresolved": sorted_json(&resolved.unresolved),
        "structural": sorted_json(&resolved.structural),
      });
      bytes.extend(serde_json::to_vec(&canonical).unwrap());
      bytes.push(b'\n');
    }
    for source in &project.repos {
      let hash_of = |path: &atlas_core::RelPath| fs::read(source.root.join(path.as_str())).ok().map(|bytes| ContentHash::of(&bytes));
      let mut manifests: Vec<String> = manifest_edges(&project.table, &source.name, hash_of)
        .into_iter()
        .map(|manifest| serde_json::json!([source.name, manifest.manifest, manifest.package.to_string(), sorted_json(&manifest.edges)]).to_string())
        .collect();
      manifests.sort();
      bytes.extend(manifests.join("\n").as_bytes());
      bytes.push(b'\n');
    }
  }
  ContentHash::of(&bytes).to_string()
}

fn assert_pinned(constant: &str, pinned: (u32, &str, &str), actual: (u32, String, String)) {
  let (version, inputs, output) = &actual;
  if (*version, inputs.as_str(), output.as_str()) == pinned {
    return;
  }
  let advice = match (inputs == pinned.1, *version == pinned.0) {
    (false, _) => format!("the resolved sources, manifests or analyzer facts changed, so re-pin; if the resolver's code changed as well, also bump {constant}"),
    (true, true) => format!("the output over unchanged inputs changed: bump {constant} so the engine re-resolves every file, then re-pin"),
    (true, false) => format!("{constant} changed: re-pin"),
  };
  panic!("{advice}\nnew pin: ({version}, \"{inputs}\", \"{output}\")");
}

#[test]
fn should_change_the_resolver_output_over_the_fixtures_only_with_a_version_bump() {
  let projects = pinned_projects();
  assert_pinned(
    "atlas_resolve::RESOLVER_VERSION",
    PINNED,
    (RESOLVER_VERSION, inputs_digest(&projects), output_digest(&projects)),
  );
}
