mod common;

use std::fs;

use atlas_core::{ContentHash, Owner, RelPath, RepoName};
use common::*;

fn money_owner() -> Owner {
  Owner::File {
    repo: RepoName::new("acme-sdk").unwrap(),
    path: RelPath::new("src/money.ts").unwrap(),
  }
}

#[test]
fn should_rewrite_no_edge_of_a_file_whose_body_edit_keeps_every_span() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let before = engine.store().read(|tx| tx.owned_facts(&money_owner())).unwrap();
  let file = scratch.path().join("tree/acme-sdk/src/money.ts");
  let original = fs::read_to_string(&file).unwrap();
  let edited = original.replacen("* 100 +", "* 101 +", 1);
  assert_ne!(edited, original);
  fs::write(&file, &edited).unwrap();

  let report = fresh(&engine);

  assert!(report.is_fresh(), "{report:?}");
  let run = report.header().run.unwrap();
  let deltas = engine.store().read(|tx| tx.run_deltas(run)).unwrap();
  assert!(deltas.iter().all(|delta| delta.owner != money_owner()), "{deltas:#?}");
  let after = engine.store().read(|tx| tx.owned_facts(&money_owner())).unwrap();
  let located: Vec<ContentHash> = after
    .edges
    .iter()
    .flat_map(|edge| &edge.evidence)
    .filter(|evidence| evidence.path.as_str() == "src/money.ts")
    .map(|evidence| evidence.content_hash)
    .collect();
  assert_eq!(after.edges.len(), before.edges.len());
  assert!(!located.is_empty());
  assert!(located.iter().all(|hash| *hash == ContentHash::of(edited.as_bytes())));
  let reference = open(&project, &scratch.path().join("reference"));
  fresh(&reference);
  import_lineage(&engine, &reference);
  assert_eq!(digest(&engine), digest(&reference), "{}", fact_diff(&engine.db_path(), &reference.db_path()));
}

fn write_module(root: &std::path::Path, index: usize) {
  let body = match index {
    0 => "export function f0(): number {\n  return 0;\n}\n".to_owned(),
    _ => format!(
      "import {{ f{previous} }} from './m{previous}';\n\nexport function f{index}(): number {{\n  return f{previous}() + missing{index}();\n}}\n",
      previous = index - 1
    ),
  };
  fs::write(root.join(format!("src/m{index}.ts")), body).unwrap();
}

fn modules_repo(root: &std::path::Path, count: usize) -> atlas_project::Project {
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(root.join("package.json"), "{ \"name\": \"modules\", \"version\": \"1.0.0\" }\n").unwrap();
  for index in 0..count {
    write_module(root, index);
  }
  project("modules", vec![repo_config("modules", root)])
}

#[test]
fn should_build_the_same_facts_with_a_bulk_load_as_with_incremental_batches() {
  let scratch = tempfile::tempdir().unwrap();
  let bulk_project = modules_repo(&scratch.path().join("bulk"), 320);
  let bulk = open(&bulk_project, &scratch.path().join("bulk-home"));
  let stepped_project = modules_repo(&scratch.path().join("stepped"), 120);
  let stepped = open(&stepped_project, &scratch.path().join("stepped-home"));

  fresh(&bulk);
  fresh(&stepped);
  for index in 120..320 {
    write_module(&scratch.path().join("stepped"), index);
  }
  fresh(&stepped);

  let bulk_run = bulk.store().read(|tx| tx.runs(1)).unwrap().remove(0);
  let stepped_run = stepped.store().read(|tx| tx.runs(1)).unwrap().remove(0);
  assert_eq!(bulk_run.report.counts.get("indexes_deferred"), Some(&1));
  assert_eq!(stepped_run.report.counts.get("indexes_deferred"), None);
  assert_eq!(bulk_run.report.counts["files_analyzed"], 320);
  assert_eq!(stepped_run.report.counts["files_analyzed"], 200);
  assert_eq!(digest(&bulk), digest(&stepped), "{}", fact_diff(&bulk.db_path(), &stepped.db_path()));
}

fn fingerprint_before_schema_v2(project: &atlas_project::Project) -> String {
  let mut repos: Vec<&atlas_project::RepoConfig> = project.repos.iter().collect();
  repos.sort_by(|left, right| left.name.cmp(&right.name));
  let identity = serde_json::json!({
    "name": project.name.as_str(),
    "repos": repos,
    "max_file_bytes": project.settings.max_file_bytes,
  });
  blake3::hash(identity.to_string().as_bytes()).to_hex().to_string()
}

#[test]
fn should_stop_indexing_when_a_binary_with_another_store_schema_adopted_the_project() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let adopted = engine.store().read(|tx| tx.blob("engine:project")).unwrap().unwrap();
  let legacy = fingerprint_before_schema_v2(&project);
  engine.store().write(|tx| tx.put_blob("engine:project", adopted.version, legacy.as_bytes())).unwrap();
  let digest_before = digest(&engine);
  fs::write(scratch.path().join("tree/acme-sdk/src/added.ts"), "export const added = 1;\n").unwrap();

  let report = fresh(&engine);

  assert_ne!(String::from_utf8(adopted.bytes).unwrap(), legacy);
  assert!(
    matches!(&report, atlas_engine::FreshnessReport::Degraded { reason, .. } if reason == "project configuration changed; reopen the engine"),
    "{report:?}"
  );
  assert_eq!(digest(&engine), digest_before);
}

const BULK_MODULES: usize = 300;
const BULK_SEED: u64 = 0xB01C_5EED_0000_0007;

struct Modules {
  root: std::path::PathBuf,
  modules: Vec<Option<(usize, u32)>>,
}

impl Modules {
  fn create(root: &std::path::Path) -> (Self, atlas_project::Project) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("package.json"), "{ \"name\": \"modules\", \"version\": \"1.0.0\" }\n").unwrap();
    let mut modules = Self {
      root: root.to_path_buf(),
      modules: Vec::new(),
    };
    for index in 0..BULK_MODULES {
      modules.modules.push(Some((index.saturating_sub(1), 1)));
      modules.write(index);
    }
    (modules, project("modules", vec![repo_config("modules", root)]))
  }

  fn file(&self, index: usize) -> std::path::PathBuf {
    self.root.join(format!("src/m{index}.ts"))
  }

  fn write(&self, index: usize) {
    let Some((target, constant)) = self.modules[index] else {
      let _ = fs::remove_file(self.file(index));
      return;
    };
    let body = match target == index {
      true => format!("export function f{index}(): number {{\n  return {constant};\n}}\n"),
      false => format!("import {{ f{target} }} from './m{target}';\n\nexport function f{index}(): number {{\n  return f{target}() + {constant} + missing{index}();\n}}\n"),
    };
    fs::write(self.file(index), body).unwrap();
  }

  fn edit(&mut self, rng: &mut SplitMix, count: usize) -> String {
    let mut touched = Vec::new();
    for _ in 0..count {
      let index = rng.below(self.modules.len() + 1);
      if index == self.modules.len() {
        self.modules.push(Some((rng.below(index), 1)));
      } else {
        self.modules[index] = match (rng.below(8), self.modules[index]) {
          (0, _) => None,
          (1..=4, Some((target, constant))) => Some((target, constant + 1)),
          (_, _) => Some((rng.below(index + 1), 7)),
        };
      }
      self.write(index);
      touched.push(index);
    }
    format!("{} edits, first {:?}", touched.len(), &touched[..touched.len().min(5)])
  }
}

#[test]
fn should_match_a_bulk_loaded_full_index_after_random_edits_and_bulk_batches() {
  let scratch = tempfile::tempdir().unwrap();
  let (mut modules, project) = Modules::create(&scratch.path().join("tree"));
  let engine = open(&project, &scratch.path().join("home"));
  let mut rng = SplitMix(BULK_SEED);
  for (step, count) in [0, 1, 5, 280, 3, 40, 290, 1].into_iter().enumerate() {
    let description = modules.edit(&mut rng, count);
    fresh(&engine);
    let reference = open(&project, &scratch.path().join(format!("reference-{step}")));
    fresh(&reference);
    let deferred = reference.store().read(|tx| tx.runs(1)).unwrap().remove(0).report.counts.get("indexes_deferred").copied();
    import_lineage(&engine, &reference);
    assert_eq!(deferred, Some(1), "step {step}: the reference must bulk-load");
    assert_eq!(
      digest(&engine),
      digest(&reference),
      "step {step} ({description})\n{}",
      fact_diff(&engine.db_path(), &reference.db_path())
    );
  }
}

const CRASH_CHILD: &str = "should_abort_a_bulk_load_while_its_indexes_are_deferred_when_run_as_a_child";

fn secondary_index_count(db: &std::path::Path) -> i64 {
  let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
  conn
    .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL", [], |row| row.get(0))
    .unwrap()
}

#[test]
#[ignore = "helper: runs only as a child process of a crash regression test"]
fn should_abort_a_bulk_load_while_its_indexes_are_deferred_when_run_as_a_child() {
  let (Ok(tree), Ok(home)) = (std::env::var("ATLAS_BULK_CHILD_TREE"), std::env::var("ATLAS_BULK_CHILD_HOME")) else {
    return;
  };
  let project = project("modules", vec![repo_config("modules", std::path::Path::new(&tree))]);
  let options = atlas_engine::EngineOptions {
    batch_max_owners: 1,
    ..atlas_engine::EngineOptions::default()
  };
  let engine = open_with(&project, std::path::Path::new(&home), options);
  let db = engine.db_path();
  std::thread::spawn(move || {
    let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    loop {
      let count: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0)).unwrap_or(0);
      if count >= 50 {
        std::process::abort();
      }
    }
  });
  fresh(&engine);
}

#[test]
fn should_restore_indexes_a_crashed_bulk_load_left_deferred_in_an_engine_that_was_already_open() {
  let scratch = tempfile::tempdir().unwrap();
  let (_, project) = Modules::create(&scratch.path().join("tree"));
  let home = scratch.path().join("home");
  let long_lived = open(&project, &home);
  let db = long_lived.db_path();
  let full = secondary_index_count(&db);

  let status = std::process::Command::new(std::env::current_exe().unwrap())
    .args(["--exact", CRASH_CHILD, "--ignored", "--test-threads=1"])
    .env("ATLAS_BULK_CHILD_TREE", scratch.path().join("tree"))
    .env("ATLAS_BULK_CHILD_HOME", &home)
    .status()
    .unwrap();
  let after_crash = secondary_index_count(&db);
  let report = fresh(&long_lived);

  assert!(!status.success(), "the child must abort in the middle of its bulk load");
  assert_eq!(after_crash, full - 7);
  assert!(report.is_fresh(), "{report:?}");
  assert_eq!(secondary_index_count(&db), full);
  let run = long_lived.store().read(|tx| tx.runs(1)).unwrap().remove(0);
  assert_eq!(run.report.counts.get("indexes_restored"), Some(&7));
  let reference = open(&project, &scratch.path().join("reference"));
  fresh(&reference);
  import_lineage(&long_lived, &reference);
  assert_eq!(digest(&long_lived), digest(&reference), "{}", fact_diff(&long_lived.db_path(), &reference.db_path()));
}

fn downgrade_to_schema_v1(engine: &atlas_engine::Engine) {
  let conn = rusqlite::Connection::open(engine.db_path()).unwrap();
  let owners: Vec<(i64, String)> = conn
    .prepare("SELECT id, key FROM owners ORDER BY id")
    .unwrap()
    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap();
  for (owner_id, key) in owners {
    let owner: Owner = serde_json::from_str(&key).unwrap();
    let facts = engine.store().read(|tx| tx.owned_facts(&owner)).unwrap();
    for entity in &facts.entities {
      let data = serde_json::to_string(entity).unwrap();
      conn
        .execute(
          "UPDATE entity_owners SET data = ?3 WHERE owner_id = ?1 AND entity_id = ?2",
          rusqlite::params![owner_id, entity.id.as_i64(), data],
        )
        .unwrap();
    }
    for edge in &facts.edges {
      let data = serde_json::to_string(edge).unwrap();
      conn
        .execute(
          "UPDATE edge_owners SET data = ?3 WHERE owner_id = ?1 AND edge_id = ?2",
          rusqlite::params![owner_id, edge.id.as_i64(), data],
        )
        .unwrap();
    }
    for (ordinal, reference) in facts.unresolved.iter().enumerate() {
      let data = serde_json::to_string(reference).unwrap();
      conn
        .execute(
          "UPDATE unresolved_refs SET data = ?3 WHERE owner_id = ?1 AND ordinal = ?2",
          rusqlite::params![owner_id, ordinal as i64, data],
        )
        .unwrap();
    }
  }
  conn.execute_batch("ALTER TABLE owners DROP COLUMN content_hash; PRAGMA user_version = 1;").unwrap();
}

#[test]
fn should_stop_an_engine_of_the_previous_schema_from_writing_after_a_migration_and_before_a_current_engine_adopts() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let home = scratch.path().join("home");
  let previous = open(&project, &home);
  fresh(&previous);
  downgrade_to_schema_v1(&previous);
  drop(atlas_store::Store::open(previous.db_path()).unwrap());
  let digest_after_migration = digest(&previous);
  let runs_after_migration = previous.store().read(|tx| tx.runs(1)).unwrap()[0].id;
  let money = scratch.path().join("tree/acme-sdk/src/money.ts");
  fs::write(&money, fs::read_to_string(&money).unwrap() + "\nexport const migrated = 1;\n").unwrap();

  let report = fresh(&previous);

  assert!(
    matches!(&report, atlas_engine::FreshnessReport::Degraded { reason, .. } if reason == "project configuration changed; reopen the engine"),
    "{report:?}"
  );
  assert_eq!(digest(&previous), digest_after_migration);
  assert_eq!(previous.store().read(|tx| tx.runs(1)).unwrap()[0].id, runs_after_migration);
  let current = open(&project, &home);
  let report = fresh(&current);
  let run = current.store().read(|tx| tx.runs(1)).unwrap().remove(0);
  assert!(report.is_fresh(), "{report:?}");
  assert_eq!(run.report.counts["files_analyzed"], 1);
  assert_eq!(run.report.counts.get("files_deleted"), Some(&0));
  let reference = open(&project, &scratch.path().join("reference"));
  fresh(&reference);
  import_lineage(&current, &reference);
  assert_eq!(digest(&current), digest(&reference), "{}", fact_diff(&current.db_path(), &reference.db_path()));
}
