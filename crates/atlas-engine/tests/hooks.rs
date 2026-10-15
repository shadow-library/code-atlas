mod common;

use std::collections::BTreeMap;
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use atlas_core::{ContentHash, Language, LanguageAnalyzer, ParseStatus, RelPath, RepoName};
use atlas_engine::{CargoProject, Engine, EngineError, EngineOptions, FreshnessReport, LanguageUnits, RustUnit, ScanKind, ScipUnit, TypeScriptUnit, UnitFolder};
use atlas_project::{Ecosystem, Project};
use atlas_store::EntityFilter;
use common::{SplitMix, acme_copy, digest, fresh, open, open_with, project, repo_config};

const WAIT_DEADLINE: Duration = Duration::from_secs(20);
const EVENT_QUIET: Duration = Duration::from_millis(300);
const RETOUCH_EVERY: Duration = Duration::from_secs(2);
const MONEY: &str = "acme-api/src/common/money.ts";
const SLUG: &str = "acme-api/src/common/slug.ts";

fn repo(name: &str) -> RepoName {
  RepoName::new(name).unwrap()
}

fn rel(path: &str) -> RelPath {
  RelPath::new(path).unwrap()
}

fn key(path: &str) -> (RepoName, RelPath) {
  let (name, rest) = path.split_once('/').unwrap();
  (repo(name), rel(rest))
}

fn append(tree: &Path, path: &str, text: &str) {
  let file = tree.join(path);
  let mut content = fs::read_to_string(&file).unwrap();
  content.push_str(text);
  fs::write(&file, content).unwrap();
}

fn probe(name: &str) -> String {
  format!("\nexport function {name}() {{\n  return 1;\n}}\n")
}

fn named(engine: &Engine, name: &str) -> usize {
  engine.store().read(|tx| tx.entities_by_name(name, &EntityFilter::default())).unwrap().len()
}

fn triggers(engine: &Engine) -> Vec<String> {
  let mut runs = engine.store().read(|tx| tx.runs(100)).unwrap();
  runs.reverse();
  runs.into_iter().map(|run| run.trigger).collect()
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
  let deadline = Instant::now() + WAIT_DEADLINE;
  while !condition() {
    assert!(Instant::now() < deadline, "timed out waiting for {what}");
    thread::sleep(Duration::from_millis(10));
  }
}

fn wait_until_retouching(what: &str, path: &Path, mut condition: impl FnMut() -> bool) {
  let mut touched = Instant::now();
  wait_until(what, || {
    if condition() {
      return true;
    }
    if touched.elapsed() >= RETOUCH_EVERY {
      fs::write(path, fs::read(path).unwrap()).unwrap();
      touched = Instant::now();
    }
    false
  });
}

fn wait_for_quiet_events(watch: &atlas_engine::WatchHandle) {
  let mut last = (watch.events_seen(), Instant::now());
  wait_until("the watcher to stop delivering events", || {
    let seen = watch.events_seen();
    if seen != last.0 {
      last = (seen, Instant::now());
    }
    last.1.elapsed() >= EVENT_QUIET
  });
}

fn indexed_acme(scratch: &Path) -> (PathBuf, Project, Engine) {
  let tree = scratch.join("tree");
  let project = acme_copy(&tree);
  let engine = open(&project, &scratch.join("home"));
  assert!(fresh(&engine).is_fresh());
  (tree, project, engine)
}

#[test]
fn should_reflect_written_files_in_the_next_query_without_another_barrier_scan() {
  let scratch = tempfile::tempdir().unwrap();
  let (tree, _, engine) = indexed_acme(scratch.path());
  let scans = engine.barrier_scans();
  let (written, report) = engine
    .write_then_refresh(&[key(MONEY)], || {
      append(&tree, MONEY, &probe("writtenProbe"));
      "applied"
    })
    .unwrap();
  assert_eq!(written, "applied");
  assert!(report.is_fresh(), "{report:?}");
  assert!(report.header().run.is_some());
  assert_eq!(report.header().files_updated, 1);
  assert_eq!(report.header().scan, ScanKind::StatScan);
  assert_eq!(named(&engine, "writtenProbe"), 1);
  assert_eq!(engine.barrier_scans(), scans);
  assert_eq!(triggers(&engine).last().map(String::as_str), Some("edit"));
  assert_eq!(fresh(&engine).header().run, None);
}

fn set_mtime(file: &Path, mtime: SystemTime) {
  fs::File::options().write(true).open(file).unwrap().set_modified(mtime).unwrap();
}

#[test]
fn should_index_a_listed_file_whose_writer_restored_its_size_and_mtime() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let money = tree.join(MONEY);
  let old = SystemTime::now() - Duration::from_secs(3600);
  set_mtime(&money, old);
  let engine = open(&project, &scratch.path().join("home"));
  assert!(fresh(&engine).is_fresh());
  let before = fs::metadata(&money).unwrap();

  let (_, report) = engine
    .write_then_refresh(&[key(MONEY)], || {
      let text = fs::read_to_string(&money).unwrap();
      fs::write(&money, text.replacen("formatMoney", "formatMonex", 1)).unwrap();
      set_mtime(&money, old);
    })
    .unwrap();
  let after = fs::metadata(&money).unwrap();
  assert_eq!((after.len(), after.modified().unwrap()), (before.len(), before.modified().unwrap()));
  assert!(report.is_fresh(), "{report:?}");
  assert_eq!(report.header().files_updated, 1);
  assert_eq!((named(&engine, "formatMonex"), named(&engine, "formatMoney")), (1, 0));
  assert_eq!(fresh(&engine).header().run, None);
}

#[test]
fn should_refuse_an_unknown_repo_without_running_the_writer() {
  let scratch = tempfile::tempdir().unwrap();
  let (_, _, engine) = indexed_acme(scratch.path());
  let ran = AtomicBool::new(false);
  let error = engine
    .write_then_refresh(&[(repo("elsewhere"), rel("src/a.ts"))], || ran.store(true, Ordering::SeqCst))
    .unwrap_err();
  assert!(matches!(&error, EngineError::UnknownRepo(name) if name.as_str() == "elsewhere"), "{error:?}");
  assert!(!ran.load(Ordering::SeqCst));
}

#[test]
fn should_index_what_a_failed_writer_left_on_disk() {
  let scratch = tempfile::tempdir().unwrap();
  let (tree, _, engine) = indexed_acme(scratch.path());
  let (written, report) = engine
    .write_then_refresh(&[key(MONEY), key(SLUG)], || -> Result<(), String> {
      append(&tree, MONEY, &probe("halfWritten"));
      Err("the second file could not be written".to_owned())
    })
    .unwrap();
  assert_eq!(written, Err("the second file could not be written".to_owned()));
  assert!(report.is_fresh(), "{report:?}");
  assert_eq!(report.header().files_updated, 1);
  assert_eq!(named(&engine, "halfWritten"), 1);

  let (written, report) = engine.write_then_refresh(&[key(SLUG)], || Err::<(), _>("nothing written")).unwrap();
  assert_eq!(written, Err("nothing written"));
  assert!(report.is_fresh(), "{report:?}");
  assert_eq!(report.header().run, None);
}

#[test]
fn should_refresh_and_release_the_locks_before_resuming_a_writer_panic() {
  let scratch = tempfile::tempdir().unwrap();
  let (tree, _, engine) = indexed_acme(scratch.path());
  let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
    engine.write_then_refresh(&[key(MONEY)], || {
      append(&tree, MONEY, &probe("beforePanic"));
      panic!("writer failed midway");
    })
  }));
  let payload = outcome.unwrap_err();
  assert_eq!(payload.downcast_ref::<&str>(), Some(&"writer failed midway"));
  assert_eq!(named(&engine, "beforePanic"), 1);
  assert_eq!(fresh(&engine).header().run, None);
  let (_, report) = engine.write_then_refresh(&[key(SLUG)], || append(&tree, SLUG, &probe("afterPanic"))).unwrap();
  assert_eq!(report.header().files_updated, 1);
  assert_eq!(named(&engine, "afterPanic"), 1);
}

struct BlockedWriter {
  entered: mpsc::Receiver<()>,
  release: mpsc::Sender<()>,
  writer: thread::JoinHandle<FreshnessReport>,
}

fn block_mid_write(engine: &Arc<Engine>, tree: &Path) -> BlockedWriter {
  let (entered_tx, entered) = mpsc::channel();
  let (release, released) = mpsc::channel::<()>();
  let writer = {
    let (engine, tree) = (Arc::clone(engine), tree.to_path_buf());
    thread::spawn(move || {
      let (_, report) = engine
        .write_then_refresh(&[key(MONEY), key(SLUG)], || {
          append(&tree, MONEY, &probe("firstHalf"));
          entered_tx.send(()).unwrap();
          released.recv().unwrap();
          append(&tree, SLUG, &probe("secondHalf"));
        })
        .unwrap();
      report
    })
  };
  BlockedWriter { entered, release, writer }
}

#[test]
fn should_make_barriers_in_this_process_wait_for_a_write_in_progress() {
  let scratch = tempfile::tempdir().unwrap();
  let (tree, _, engine) = indexed_acme(scratch.path());
  let engine = Arc::new(engine);
  let blocked = block_mid_write(&engine, &tree);
  blocked.entered.recv().unwrap();
  let (done_tx, done) = mpsc::channel();
  let barrier = {
    let engine = Arc::clone(&engine);
    thread::spawn(move || {
      let report = engine.ensure_fresh().unwrap();
      done_tx.send(()).unwrap();
      report
    })
  };
  assert_eq!(done.recv_timeout(Duration::from_millis(300)), Err(mpsc::RecvTimeoutError::Timeout));
  blocked.release.send(()).unwrap();
  let written = blocked.writer.join().unwrap();
  let waited = barrier.join().unwrap();
  assert_eq!(written.header().files_updated, 2);
  assert!(waited.is_fresh(), "{waited:?}");
  assert_eq!(waited.header().run, None);
  assert_eq!((named(&engine, "firstHalf"), named(&engine, "secondHalf")), (1, 1));
}

#[test]
fn should_make_another_process_wait_and_never_index_a_half_applied_write() {
  let scratch = tempfile::tempdir().unwrap();
  let (tree, project, engine) = indexed_acme(scratch.path());
  let engine = Arc::new(engine);
  let other = Arc::new(open(&project, &scratch.path().join("home")));
  let before = triggers(&engine).len();
  let blocked = block_mid_write(&engine, &tree);
  blocked.entered.recv().unwrap();
  let barrier = {
    let other = Arc::clone(&other);
    thread::spawn(move || other.ensure_fresh().unwrap())
  };
  thread::sleep(Duration::from_millis(300));
  assert!(!barrier.is_finished(), "the other engine indexed while the write held the index lock");
  blocked.release.send(()).unwrap();
  let written = blocked.writer.join().unwrap();
  let waited = barrier.join().unwrap();
  assert_eq!(written.header().files_updated, 2);
  assert!(waited.is_fresh(), "{waited:?}");
  assert_eq!(waited.header().run, None);
  assert_eq!(triggers(&engine)[before..], ["edit".to_owned()]);
  assert_eq!((named(&other, "firstHalf"), named(&other, "secondHalf")), (1, 1));
}

#[test]
fn should_force_a_scan_after_write_then_refresh_even_with_a_clean_watcher() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let options = EngineOptions {
    watch_debounce: Duration::from_millis(50),
    ..EngineOptions::default()
  };
  let engine = Arc::new(open_with(&project, &scratch.path().join("home"), options));
  fresh(&engine);
  let watch = engine.watch().unwrap();
  wait_for_quiet_events(&watch);
  fresh(&engine);
  assert_eq!(fresh(&engine).header().scan, ScanKind::WatcherClean);
  let runs = triggers(&engine).len();

  let scans = engine.barrier_scans();
  let (_, report) = engine.write_then_refresh(&[key(MONEY)], || append(&tree, MONEY, &probe("watchedWrite"))).unwrap();
  assert_eq!(report.header().scan, ScanKind::StatScan);
  assert_eq!(report.header().files_updated, 1);
  assert_eq!(named(&engine, "watchedWrite"), 1);

  wait_until_retouching("the watcher to refresh after the written events", &tree.join(MONEY), || engine.barrier_scans() > scans);
  wait_for_quiet_events(&watch);
  assert_eq!(fresh(&engine).header().run, None);
  assert_eq!(triggers(&engine)[runs..], ["edit".to_owned()]);
}

fn write_file(root: &Path, path: &str, text: &str) {
  let file = root.join(path);
  fs::create_dir_all(file.parent().unwrap()).unwrap();
  fs::write(file, text).unwrap();
}

fn crate_manifest(name: &str) -> String {
  format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
}

const NPM_MANIFEST: &str = "{ \"name\": \"web\", \"version\": \"1.0.0\" }\n";

fn polyglot(root: &Path) -> Project {
  let mono = root.join("mono");
  write_file(&mono, "rust/Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
  write_file(&mono, "rust/crates/a/Cargo.toml", &crate_manifest("a"));
  write_file(&mono, "rust/crates/a/src/lib.rs", "pub fn alpha() -> u8 {\n  1\n}\n");
  write_file(&mono, "rust/crates/b/Cargo.toml", &crate_manifest("b"));
  write_file(&mono, "rust/crates/b/src/lib.rs", "pub fn beta() -> u8 {\n  2\n}\n");
  write_file(&mono, "tools/gen/Cargo.toml", &crate_manifest("gen"));
  write_file(&mono, "tools/gen/src/main.rs", "fn main() {}\n");
  write_file(&mono, "web/package.json", NPM_MANIFEST);
  write_file(&mono, "web/src/app.ts", "export function app(): number {\n  return 1;\n}\n");
  let docs = root.join("docs");
  write_file(&docs, "README.md", "# docs\n");
  let lib = root.join("lib");
  write_file(&lib, "Cargo.toml", &format!("{}\n[workspace]\n", crate_manifest("lib")));
  write_file(&lib, "src/lib.rs", "pub struct Lib;\n");
  let loose = root.join("loose");
  write_file(&loose, "scripts/loose.rs", "fn loose() {}\n");
  let site = root.join("site");
  write_file(&site, "index.ts", "export const site = 1;\n");
  project(
    "polyglot",
    ["mono", "docs", "lib", "loose", "site"].iter().map(|name| repo_config(name, &root.join(name))).collect(),
  )
}

fn folder(root: &Path, name: &str) -> UnitFolder {
  UnitFolder {
    repo: repo(name),
    root: root.join(name),
  }
}

fn cargo(root: &Path, name: &str, manifest: &str) -> CargoProject {
  CargoProject {
    repo: repo(name),
    manifest: rel(manifest),
    path: root.join(name).join(manifest),
  }
}

#[test]
fn should_list_one_rust_and_one_typescript_unit_with_workspaces_folded() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("tree");
  let project = polyglot(&root);
  let engine = open(&project, &scratch.path().join("home"));
  assert_eq!(engine.language_units().unwrap(), LanguageUnits::default());
  fresh(&engine);

  let expected = LanguageUnits {
    rust: Some(RustUnit {
      folders: vec![folder(&root, "mono"), folder(&root, "lib"), folder(&root, "loose")],
      linked_projects: vec![
        cargo(&root, "mono", "rust/Cargo.toml"),
        cargo(&root, "mono", "tools/gen/Cargo.toml"),
        cargo(&root, "lib", "Cargo.toml"),
      ],
      undiscovered: vec![repo("loose")],
    }),
    typescript: Some(TypeScriptUnit {
      folders: vec![folder(&root, "mono"), folder(&root, "site")],
    }),
  };
  assert_eq!(engine.language_units().unwrap(), expected);

  fs::remove_dir_all(root.join("site")).unwrap();
  assert!(!fresh(&engine).is_fresh());
  let units = engine.language_units().unwrap();
  assert_eq!(
    units.typescript,
    Some(TypeScriptUnit {
      folders: vec![folder(&root, "mono")]
    })
  );
  assert_eq!(units.rust, expected.rust);
}

fn hashed(root: &Path, repo: &str, paths: &[&str]) -> BTreeMap<RelPath, ContentHash> {
  paths
    .iter()
    .map(|path| (rel(path), ContentHash::of(&fs::read(root.join(repo).join(path)).unwrap())))
    .collect()
}

#[test]
fn should_plan_scip_units_per_workspace_standalone_crate_and_npm_package() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("tree");
  let project = polyglot(&root);
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let unit = |name: &str, manifest: &str, ecosystem: Ecosystem, dir: Option<&str>, paths: &[&str]| ScipUnit {
    repo: repo(name),
    manifest: rel(manifest),
    ecosystem,
    root: dir.map_or_else(|| root.join(name), |dir| root.join(name).join(dir)),
    package_dir: dir.map(rel),
    files: hashed(&root, name, paths),
  };
  let plan = engine.scip_plan().unwrap();
  assert_eq!(
    plan,
    vec![
      unit("lib", "Cargo.toml", Ecosystem::Cargo, None, &["src/lib.rs"]),
      unit(
        "mono",
        "rust/Cargo.toml",
        Ecosystem::Cargo,
        Some("rust"),
        &["rust/crates/a/src/lib.rs", "rust/crates/b/src/lib.rs"]
      ),
      unit("mono", "tools/gen/Cargo.toml", Ecosystem::Cargo, Some("tools/gen"), &["tools/gen/src/main.rs"]),
      unit("mono", "web/package.json", Ecosystem::Npm, Some("web"), &["web/src/app.ts"]),
    ]
  );
  let inputs: Vec<String> = plan[1].input_files().into_iter().map(|file| file.path.to_string()).collect();
  assert_eq!(inputs, ["crates/a/src/lib.rs", "crates/b/src/lib.rs"]);
}

fn symbol_names(engine: &Engine, language: Language, text: &str) -> Vec<String> {
  let facts = engine.analyze_text(language, text).unwrap();
  assert_eq!(facts.parse_status, ParseStatus::Ok);
  facts.symbols.into_iter().map(|symbol| symbol.name).collect()
}

#[test]
fn should_analyze_unsaved_buffers_without_writing_to_the_store() {
  let scratch = tempfile::tempdir().unwrap();
  let (_, _, engine) = indexed_acme(scratch.path());
  let before = (digest(&engine), triggers(&engine));
  assert_eq!(
    symbol_names(&engine, Language::Rust, "pub fn total(cents: i64) -> i64 {\n  cents + 1\n}\n\npub struct Ledger;\n"),
    ["total", "Ledger"]
  );
  assert_eq!(
    symbol_names(
      &engine,
      Language::TypeScript,
      "export function formatCents(cents: number): string {\n  return `${cents}`;\n}\n"
    ),
    ["formatCents"]
  );
  assert_eq!(symbol_names(&engine, Language::Tsx, "export const Badge = () => <span>ok</span>;\n"), ["Badge"]);
  let facts = engine.analyze_text(Language::JavaScript, "export function plain() {}\n").unwrap();
  assert_eq!(
    (facts.file.repo.as_str(), facts.file.path.as_str(), facts.file.language),
    ("unsaved", "buffer.js", Language::JavaScript)
  );
  assert_eq!(facts.file.content_hash, ContentHash::of(b"export function plain() {}\n"));
  assert_eq!((digest(&engine), triggers(&engine)), before);
}

#[test]
fn should_return_failed_facts_for_a_buffer_over_the_file_size_limit() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("tree");
  let mut project = polyglot(&root);
  project.settings.max_file_bytes = 16;
  let engine = open(&project, &scratch.path().join("home"));
  let facts = engine.analyze_text(Language::Rust, "pub fn too_long_for_the_limit() {}\n").unwrap();
  assert_eq!(
    facts.parse_status,
    ParseStatus::Failed {
      reason: "buffer of 35 bytes exceeds max_file_bytes 16".to_owned()
    }
  );
  assert!(facts.symbols.is_empty());
}

fn garbage(rng: &mut SplitMix, length: usize) -> String {
  const ALPHABET: &[char] = &[
    '{', '}', '(', ')', '<', '>', '[', ']', '"', '\'', '`', '/', '*', '\\', '#', '!', ';', ':', '=', '\n', '\0', 'é', '\u{fffd}', 'x', '1',
  ];
  (0..length).map(|_| ALPHABET[rng.below(ALPHABET.len())]).collect()
}

#[test]
fn should_never_panic_on_garbage_buffers() {
  let scratch = tempfile::tempdir().unwrap();
  let (_, _, engine) = indexed_acme(scratch.path());
  let mut rng = SplitMix(0xB4_6A5B_A6E0);
  let depth = 20_000;
  let mut buffers = vec![
    String::new(),
    "\u{feff}\0\0\u{fffd}".to_owned(),
    format!("fn f() {{ {}1{} }}", "(".repeat(depth), ")".repeat(depth)),
    format!("const a = {}1{};", "[".repeat(depth), "]".repeat(depth)),
    "}".repeat(depth),
  ];
  buffers.extend((0..40).map(|index| garbage(&mut rng, 64 + index * 97)));
  let languages = [Language::Rust, Language::TypeScript, Language::Tsx, Language::JavaScript, Language::Jsx];
  let worker = {
    let engine = Arc::new(engine);
    thread::Builder::new()
      .stack_size(1024 * 1024)
      .spawn(move || {
        let mut analyzed = 0usize;
        for buffer in &buffers {
          for language in languages {
            let facts = engine.analyze_text(language, buffer).unwrap();
            assert_eq!(facts.file.language, language);
            assert_eq!(facts.check_invariants(buffer.len()), Ok(()));
            if !matches!(facts.parse_status, ParseStatus::Failed { .. }) {
              analyzed += 1;
            }
          }
        }
        analyzed
      })
      .unwrap()
  };
  let analyzed = worker.join().unwrap();
  assert_eq!(analyzed, 45 * 5);
}

#[test]
fn should_use_the_toolchain_analyzer_producer_for_buffer_facts() {
  let scratch = tempfile::tempdir().unwrap();
  let (_, _, engine) = indexed_acme(scratch.path());
  let facts = engine.analyze_text(Language::Rust, "fn a() {}\n").unwrap();
  assert_eq!(facts.analyzer, atlas_lang_rust::RustAnalyzer::new().producer());
  let facts = engine.analyze_text(Language::TypeScript, "function a() {}\n").unwrap();
  assert_eq!(facts.analyzer, atlas_lang_ts::TsAnalyzer::new().producer());
}
