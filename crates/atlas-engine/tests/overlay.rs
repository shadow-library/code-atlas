mod common;

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use atlas_core::{Owner, ProjectName, RelPath, RepoName};
use atlas_engine::{BoundProject, Engine, EngineError, EngineOptions, bind_project, seed_overlay_with};
use atlas_project::{AtlasHome, Binding, ConfigEnv, Overlays, ProjectError, Registry, RepoSpec, SeedSource};
use atlas_store::StoreOptions;
use common::*;
use tempfile::TempDir;

const LIB_RS: &str = "pub mod pricing;\npub mod tax;\n";
const PRICING_RS: &str = "use crate::tax::vat;\n\npub fn total(net: u64) -> u64 {\n  net + vat(net)\n}\n";
const TAX_RS: &str = "pub fn vat(net: u64) -> u64 {\n  net / 5\n}\n";
const BILLING_RS: &str = "pub fn invoice(amount: u64) -> u64 {\n  amount\n}\n";

fn git(cwd: &Path, args: &[&str]) {
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

fn crate_dir(root: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
  let dir = root.join(name);
  fs::create_dir_all(dir.join("src")).unwrap();
  fs::write(dir.join("Cargo.toml"), format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n")).unwrap();
  for (path, text) in files {
    fs::write(dir.join(path), text).unwrap();
  }
  dir
}

struct Sandbox {
  _dir: TempDir,
  root: PathBuf,
  home: AtlasHome,
  main: PathBuf,
  billing: PathBuf,
}

impl Sandbox {
  fn new() -> Self {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let main = crate_dir(&root, "shop", &[("src/lib.rs", LIB_RS), ("src/pricing.rs", PRICING_RS), ("src/tax.rs", TAX_RS)]);
    fs::write(main.join(".gitignore"), ".worktrees/\n").unwrap();
    git(&main, &["init", "--quiet", "--initial-branch=trunk"]);
    git(&main, &["add", "."]);
    git(&main, &["commit", "--quiet", "--message", "seed"]);
    let billing = crate_dir(&root, "billing", &[("src/lib.rs", BILLING_RS)]);
    let home = AtlasHome::rooted_at(root.join("atlas"));
    let mut registry = Registry::load_with(&home, ConfigEnv::default()).unwrap();
    registry
      .create_project("acme", vec![RepoSpec::new(&main).named("shop"), RepoSpec::new(&billing).named("billing")])
      .unwrap();
    Self {
      _dir: dir,
      root,
      home,
      main,
      billing,
    }
  }

  fn registry(&self) -> Registry {
    Registry::load_with(&self.home, ConfigEnv::default()).unwrap()
  }

  fn base(&self) -> Engine {
    let project = self.registry().project("acme").unwrap().clone();
    Engine::open(&project, &self.home, EngineOptions::default()).unwrap()
  }

  fn worktree(&self, name: &str) -> PathBuf {
    let relative = format!(".worktrees/{name}");
    git(&self.main, &["worktree", "add", "--quiet", "-b", name, &relative]);
    fs::canonicalize(self.main.join(relative)).unwrap()
  }

  fn bind(&self, cwd: &Path) -> BoundProject {
    bind_project(&self.home, &self.registry(), cwd, None, &StoreOptions::default()).unwrap()
  }

  fn overlay(&self, cwd: &Path) -> (BoundProject, Engine) {
    let bound = self.bind(cwd);
    assert!(matches!(bound.binding, Binding::Overlay(_)), "{:?} {:?}", bound.binding, bound.warnings);
    let engine = Engine::open(&bound.project, &bound.engine_home, EngineOptions::default()).unwrap();
    (bound, engine)
  }

  fn reference(&self, worktree: &Path) -> Engine {
    let project = project("reference", vec![repo_config("shop", worktree), repo_config("billing", &self.billing)]);
    let engine = open(&project, &self.root.join(format!("reference-{}", worktree.file_name().unwrap().to_string_lossy())));
    assert!(fresh(&engine).is_fresh());
    engine
  }

  fn write_config(&self, text: &str) {
    fs::create_dir_all(self.home.config_dir()).unwrap();
    fs::write(self.home.config_path(), text).unwrap();
  }
}

fn names_in(engine: &Engine, path: &str) -> Vec<String> {
  let owner = Owner::File {
    repo: RepoName::new("shop").unwrap(),
    path: RelPath::new(path).unwrap(),
  };
  let mut names: Vec<String> = engine
    .store()
    .read(|tx| tx.owned_facts(&owner))
    .unwrap()
    .entities
    .into_iter()
    .map(|entity| entity.name)
    .collect();
  names.sort();
  names
}

fn files_analyzed(engine: &Engine) -> u64 {
  let runs = engine.store().read(|tx| tx.runs(1)).unwrap();
  runs[0].report.counts.get("files_analyzed").copied().unwrap_or(0)
}

#[test]
fn should_index_a_worktree_from_a_seeded_copy_exactly_as_a_full_index_of_it() {
  let sandbox = Sandbox::new();
  let base = sandbox.base();
  assert!(fresh(&base).is_fresh());
  let base_digest = digest(&base);
  let worktree = sandbox.worktree("feature");
  fs::write(
    worktree.join("src/pricing.rs"),
    format!("{PRICING_RS}\npub fn discount(net: u64) -> u64 {{\n  net / 10\n}}\n"),
  )
  .unwrap();

  let (bound, overlay) = sandbox.overlay(&worktree.join("src"));
  let report = fresh(&overlay);

  assert!(report.is_fresh(), "{report:?}");
  assert_eq!(
    bound.overlay.as_ref().and_then(|lease| lease.seeded.as_ref()).map(|seed| seed.source),
    Some(SeedSource::Base)
  );
  assert_eq!(bound.project.repo("shop").unwrap().path, worktree);
  assert_eq!(bound.project.repo("billing").unwrap().path, sandbox.billing);
  assert_eq!(bound.binding.edit_refusal(), None);
  assert_eq!(bound.binding.caveat(), None);
  assert_eq!(files_analyzed(&overlay), 1);
  assert!(names_in(&overlay, "src/pricing.rs").contains(&"discount".to_owned()));
  assert!(!names_in(&base, "src/pricing.rs").contains(&"discount".to_owned()));
  let reference = sandbox.reference(&worktree);
  assert_eq!(digest(&overlay), digest(&reference), "{}", fact_diff(&overlay.db_path(), &reference.db_path()));
  assert!(fresh(&base).is_fresh());
  assert_eq!(digest(&base), base_digest);
  assert_eq!(base.db_path(), sandbox.home.db_path(&ProjectName::new("acme").unwrap()));
  assert!(overlay.db_path().starts_with(sandbox.home.overlays_dir()));
}

#[test]
fn should_keep_two_overlays_of_one_project_independent() {
  let sandbox = Sandbox::new();
  assert!(fresh(&sandbox.base()).is_fresh());
  let left = sandbox.worktree("left");
  let right = sandbox.worktree("right");
  fs::write(left.join("src/tax.rs"), format!("{TAX_RS}\npub fn left_only() {{}}\n")).unwrap();
  fs::write(right.join("src/tax.rs"), format!("{TAX_RS}\npub fn right_only() {{}}\n")).unwrap();

  let (left_bound, left_engine) = sandbox.overlay(&left);
  let (right_bound, right_engine) = sandbox.overlay(&right);
  fresh(&left_engine);
  fresh(&right_engine);

  assert_ne!(left_bound.project.name, right_bound.project.name);
  assert_ne!(left_engine.db_path(), right_engine.db_path());
  assert_eq!(names_in(&left_engine, "src/tax.rs"), ["left_only", "src/tax.rs", "vat"]);
  assert_eq!(names_in(&right_engine, "src/tax.rs"), ["right_only", "src/tax.rs", "vat"]);
  assert_eq!(names_in(&sandbox.base(), "src/tax.rs"), ["src/tax.rs", "vat"]);
}

#[test]
fn should_seed_a_consistent_copy_while_the_base_project_keeps_indexing() {
  let sandbox = Sandbox::new();
  let base = Arc::new(sandbox.base());
  assert!(fresh(&base).is_fresh());
  let worktree = sandbox.worktree("feature");
  fs::write(worktree.join("src/pricing.rs"), format!("{PRICING_RS}\npub fn surcharge() {{}}\n")).unwrap();
  let stop = Arc::new(AtomicBool::new(false));
  let indexer = {
    let (base, stop, main) = (Arc::clone(&base), Arc::clone(&stop), sandbox.main.clone());
    thread::spawn(move || {
      let mut round = 0;
      while !stop.load(Ordering::SeqCst) {
        round += 1;
        fs::write(main.join("src/tax.rs"), format!("{TAX_RS}\npub fn round_{round}() {{}}\n")).unwrap();
        assert!(base.ensure_fresh().unwrap().is_fresh());
      }
      round
    })
  };

  let mut overlays = Vec::new();
  for name in ["one", "two", "three"] {
    let worktree = match name {
      "one" => worktree.clone(),
      other => sandbox.worktree(other),
    };
    let (_, engine) = sandbox.overlay(&worktree);
    assert!(fresh(&engine).is_fresh());
    overlays.push((worktree, engine));
  }
  stop.store(true, Ordering::SeqCst);
  let rounds = indexer.join().unwrap();

  assert!(rounds > 0);
  for (worktree, engine) in &overlays {
    let reference = sandbox.reference(worktree);
    assert_eq!(digest(engine), digest(&reference), "{}", fact_diff(&engine.db_path(), &reference.db_path()));
    assert_eq!(names_in(engine, "src/tax.rs"), ["src/tax.rs", "vat"]);
  }
}

#[test]
fn should_answer_from_the_main_checkout_and_refuse_edits_when_the_base_index_stays_locked() {
  let sandbox = Sandbox::new();
  assert!(fresh(&sandbox.base()).is_fresh());
  let worktree = sandbox.worktree("feature");
  let base_db = sandbox.home.db_path(&ProjectName::new("acme").unwrap());
  let held = File::options()
    .create(true)
    .truncate(false)
    .write(true)
    .open(format!("{}.index.lock", base_db.display()))
    .unwrap();
  held.lock().unwrap();
  let options = StoreOptions {
    lock_timeout: Duration::from_millis(200),
    ..StoreOptions::default()
  };

  let bound = bind_project(&sandbox.home, &sandbox.registry(), &worktree, None, &options).unwrap();

  assert!(matches!(bound.binding, Binding::Worktree(_)), "{:?}", bound.binding);
  assert!(bound.binding.edit_refusal().is_some());
  assert_eq!(bound.engine_home, sandbox.home);
  assert_eq!(bound.project.repo("shop").unwrap().path, sandbox.main);
  assert!(
    bound
      .warnings
      .iter()
      .any(|warning| warning.starts_with("could not prepare the worktree overlay acme@feature")),
    "{:?}",
    bound.warnings
  );
  let listed = Overlays::new(&sandbox.home).list().unwrap();
  assert!(listed.iter().all(|listing| !listing.db_path.exists()));
}

#[test]
fn should_leave_no_overlay_when_seeding_from_an_unreadable_base_fails() {
  let sandbox = Sandbox::new();
  let worktree = sandbox.worktree("feature");
  let base_db = sandbox.home.db_path(&ProjectName::new("acme").unwrap());
  fs::create_dir_all(base_db.parent().unwrap()).unwrap();
  fs::write(&base_db, vec![7u8; 8192]).unwrap();

  let bound = sandbox.bind(&worktree);

  assert!(bound.binding.edit_refusal().is_some(), "{:?}", bound.binding);
  let overlay_dir = sandbox.home.overlays_dir();
  let leftovers: Vec<String> = fs::read_dir(&overlay_dir)
    .unwrap()
    .flat_map(|entry| {
      let entry = entry.unwrap();
      match entry.file_type().unwrap().is_dir() {
        true => fs::read_dir(entry.path())
          .unwrap()
          .map(|inner| inner.unwrap().file_name().to_string_lossy().into_owned())
          .collect(),
        false => Vec::new(),
      }
    })
    .collect();
  assert_eq!(leftovers, Vec::<String>::new());
  assert_eq!(fs::read(&base_db).unwrap(), vec![7u8; 8192]);
}

#[test]
fn should_refuse_to_seed_without_room_for_two_copies_of_the_project_database() {
  let sandbox = Sandbox::new();
  assert!(fresh(&sandbox.base()).is_fresh());
  let base_db = sandbox.home.db_path(&ProjectName::new("acme").unwrap());
  let staging = sandbox.root.join("staging/atlas.db.seeding");
  fs::create_dir_all(staging.parent().unwrap()).unwrap();
  let size = fs::metadata(&base_db).unwrap().len();

  let refused = seed_overlay_with(&base_db, &staging, &[], &StoreOptions::default(), |_| Ok(2 * size - 1));
  let seeded = seed_overlay_with(&base_db, &staging, &[], &StoreOptions::default(), |_| Ok(u64::MAX));

  assert!(
    matches!(&refused, Err(EngineError::InsufficientSpace { needed, available, .. }) if *needed >= 2 * size && *available == 2 * size - 1),
    "{refused:?}"
  );
  assert_eq!(seeded.unwrap().source, SeedSource::Base);
}

#[test]
fn should_collect_overlays_of_removed_worktrees_from_project_commands() {
  let sandbox = Sandbox::new();
  assert!(fresh(&sandbox.base()).is_fresh());
  let worktree = sandbox.worktree("feature");
  let (bound, engine) = sandbox.overlay(&worktree);
  let overlay_db = engine.db_path();
  drop((engine, bound));
  git(&sandbox.main, &["worktree", "remove", "--force", ".worktrees/feature"]);
  let state = sandbox.home.worktrees_path();
  let mut document: serde_json::Value = serde_json::from_str(&fs::read_to_string(&state).unwrap()).unwrap();
  document["last_gc_ms"] = serde_json::json!(0);
  fs::write(&state, document.to_string()).unwrap();

  let in_main = sandbox.bind(&sandbox.main);

  assert_eq!(in_main.binding, Binding::Direct);
  assert!(!overlay_db.exists());
  assert_eq!(Overlays::new(&sandbox.home).list().unwrap(), []);
  assert!(sandbox.home.db_path(&ProjectName::new("acme").unwrap()).exists());
}

#[test]
fn should_start_an_overlay_empty_without_creating_data_for_a_base_that_was_never_indexed() {
  let sandbox = Sandbox::new();
  let worktree = sandbox.worktree("feature");

  let (bound, overlay) = sandbox.overlay(&worktree);
  let report = fresh(&overlay);

  assert!(report.is_fresh(), "{report:?}");
  assert_eq!(
    bound.overlay.as_ref().and_then(|lease| lease.seeded.as_ref()).map(|seed| seed.source),
    Some(SeedSource::Empty)
  );
  assert_eq!(names_in(&overlay, "src/tax.rs"), ["src/tax.rs", "vat"]);
  assert!(!sandbox.home.project_data_dir(&ProjectName::new("acme").unwrap()).exists());
}

#[test]
fn should_follow_the_configured_worktree_mode() {
  let sandbox = Sandbox::new();
  let worktree = sandbox.worktree("feature");

  sandbox.write_config("[worktrees]\nmode = \"main\"\n");
  let main = sandbox.bind(&worktree);
  sandbox.write_config("[worktrees]\nmode = \"off\"\n");
  let off = bind_project(&sandbox.home, &sandbox.registry(), &worktree, None, &StoreOptions::default());

  assert!(matches!(main.binding, Binding::Worktree(_)), "{:?}", main.binding);
  assert!(main.overlay.is_none());
  assert!(matches!(&off, Err(ProjectError::WorktreeIgnored { project, .. }) if project == "acme"), "{off:?}");
  assert_eq!(Overlays::new(&sandbox.home).list().unwrap(), []);
  let in_main = bind_project(&sandbox.home, &sandbox.registry(), &sandbox.main, None, &StoreOptions::default()).unwrap();
  assert_eq!(in_main.binding, Binding::Direct);
}
