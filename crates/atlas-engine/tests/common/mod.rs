#![allow(dead_code)]

pub mod edits;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{ContentHash, Owner, ProjectName, RepoName};
use atlas_engine::{Engine, EngineOptions, FreshnessReport, PASS_LINEAGE};
use atlas_project::{AtlasHome, Project, ProjectSettings, RepoConfig};

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

pub fn repo_config(name: &str, path: &Path) -> RepoConfig {
  RepoConfig {
    name: RepoName::new(name).unwrap(),
    path: path.to_path_buf(),
    include: Vec::new(),
    exclude: Vec::new(),
    languages: None,
  }
}

pub fn project(name: &str, repos: Vec<RepoConfig>) -> Project {
  Project {
    name: ProjectName::new(name).unwrap(),
    settings: ProjectSettings::default(),
    repos,
  }
}

pub const ACME_REPOS: [&str; 4] = ["acme-api", "acme-sdk", "acme-web", "acme-billing"];

pub fn acme_copy(root: &Path) -> Project {
  let repos = ACME_REPOS
    .iter()
    .map(|name| {
      let target = root.join(name);
      copy_dir(&fixtures_root().join("acme").join(name), &target);
      repo_config(name, &target)
    })
    .collect();
  project("acme", repos)
}

pub fn open(project: &Project, home: &Path) -> Engine {
  Engine::open(project, &AtlasHome::rooted_at(home), EngineOptions::default()).unwrap()
}

pub fn open_with(project: &Project, home: &Path, options: EngineOptions) -> Engine {
  Engine::open(project, &AtlasHome::rooted_at(home), options).unwrap()
}

pub fn fresh(engine: &Engine) -> FreshnessReport {
  engine.ensure_fresh().unwrap()
}

pub fn digest(engine: &Engine) -> ContentHash {
  engine.store().read(|tx| tx.fact_set_digest()).unwrap()
}

pub fn import_lineage(from: &Engine, into: &Engine) {
  let owner = Owner::Project { pass: PASS_LINEAGE.to_owned() };
  let facts = from.store().read(|tx| tx.owned_facts(&owner)).unwrap();
  into.store().write(|tx| tx.replace_owner(&owner, &facts)).unwrap();
}

pub fn dump(db: &Path) -> BTreeMap<String, String> {
  let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
  let mut facts = BTreeMap::new();
  let queries = [
    ("entity", "SELECT o.key, eo.entity_id, eo.data FROM entity_owners eo JOIN owners o ON o.id = eo.owner_id"),
    ("edge", "SELECT o.key, eo.edge_id, eo.data FROM edge_owners eo JOIN owners o ON o.id = eo.owner_id"),
    ("unresolved", "SELECT o.key, u.ordinal, u.data FROM unresolved_refs u JOIN owners o ON o.id = u.owner_id"),
  ];
  for (tag, sql) in queries {
    let mut statement = conn.prepare(sql).unwrap();
    let rows = statement
      .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?)))
      .unwrap();
    for row in rows {
      let (owner, id, data) = row.unwrap();
      let key = match tag {
        "unresolved" => format!("{tag} {owner} {data}"),
        _ => format!("{tag} {owner} {id:016x}"),
      };
      facts.insert(key, data);
    }
  }
  facts
}

pub fn fact_diff(incremental: &Path, full: &Path) -> String {
  let left = dump(incremental);
  let right = dump(full);
  let mut lines = Vec::new();
  for (key, value) in &left {
    match right.get(key) {
      None => lines.push(format!("- only incremental: {key}\n    {value}")),
      Some(other) if other != value => lines.push(format!("~ differs: {key}\n    incremental: {value}\n    full:        {other}")),
      Some(_) => {}
    }
  }
  for (key, value) in &right {
    if !left.contains_key(key) {
      lines.push(format!("+ only full: {key}\n    {value}"));
    }
  }
  lines.truncate(60);
  lines.join("\n")
}

pub struct SplitMix(pub u64);

impl SplitMix {
  pub fn next(&mut self) -> u64 {
    self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = self.0;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
  }

  pub fn below(&mut self, bound: usize) -> usize {
    (self.next() % bound as u64) as usize
  }

  pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
    if items.is_empty() {
      return None;
    }
    Some(&items[self.below(items.len())])
  }
}

pub fn walk_files(root: &Path) -> Vec<PathBuf> {
  let mut files = Vec::new();
  let mut stack = vec![root.to_path_buf()];
  while let Some(dir) = stack.pop() {
    let Ok(entries) = fs::read_dir(&dir) else { continue };
    for entry in entries.flatten() {
      let path = entry.path();
      if path.is_dir() {
        stack.push(path);
      } else {
        files.push(path);
      }
    }
  }
  files.sort();
  files
}
