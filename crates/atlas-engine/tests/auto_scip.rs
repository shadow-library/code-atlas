mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use atlas_core::{Availability, Certainty, ContentHash, EdgeKind, Owner, RelPath, RepoName};
use atlas_engine::{AutoScipOptions, AutoScipOutcome, AutoScipSetting, Engine, EngineOptions, IndexMode, ScipLag};
use atlas_project::Project;
use atlas_scip::{CacheStatus, ScipFacts, ScipIndexer, ScipOutcome, ScipRequest, ScipRun};
use common::{fresh, open_with, project, repo_config};
use protobuf::{EnumOrUnknown, Message};
use scip::types::{Document, Index, Metadata, Occurrence, PositionEncoding, SymbolRole, ToolInfo};

const WAIT_DEADLINE: Duration = Duration::from_secs(30);
const EVENT_QUIET: Duration = Duration::from_millis(300);
const NO_RETRY_WINDOW: Duration = Duration::from_millis(2_500);
const A_SOURCE: &str = "export function load(n: number) {\n  return n;\n}\n";
const B_SOURCE: &str = "import { load } from './a';\nexport function run() {\n  return load(1);\n}\n";

#[derive(Default)]
struct Gate {
  released: Mutex<usize>,
  changed: Condvar,
}

impl Gate {
  fn wait_for(&self, call: usize, cancel: &AtomicBool) -> bool {
    let mut released = self.released.lock().unwrap();
    while *released < call {
      if cancel.load(Ordering::SeqCst) {
        return false;
      }
      released = self.changed.wait_timeout(released, Duration::from_millis(20)).unwrap().0;
    }
    true
  }

  fn release(&self) {
    *self.released.lock().unwrap() += 1;
    self.changed.notify_all();
  }
}

enum Behaviour {
  Index,
  Unavailable,
  Failing,
  Blocking(Arc<Gate>),
}

struct FakeScip {
  output: PathBuf,
  calls: Arc<AtomicUsize>,
  behaviour: Behaviour,
}

impl ScipIndexer for FakeScip {
  fn id(&self) -> &'static str {
    "fake-scip"
  }

  fn availability(&self) -> Availability {
    Availability::Available
  }

  fn index(&self, request: &ScipRequest) -> ScipRun {
    self.index_with_cancel(request, &AtomicBool::new(false))
  }

  fn index_with_cancel(&self, _request: &ScipRequest, cancel: &AtomicBool) -> ScipRun {
    let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
    let outcome = match &self.behaviour {
      Behaviour::Unavailable => ScipOutcome::Unavailable {
        reason: "npx is not on PATH".to_owned(),
      },
      Behaviour::Failing => ScipOutcome::Failed {
        reason: "fake-scip exited with status 101".to_owned(),
        stderr_tail: String::new(),
      },
      Behaviour::Index => ScipOutcome::Ok,
      Behaviour::Blocking(gate) => match gate.wait_for(call, cancel) {
        true => ScipOutcome::Ok,
        false => ScipOutcome::Failed {
          reason: "fake-scip was cancelled".to_owned(),
          stderr_tail: String::new(),
        },
      },
    };
    fs::write(&self.output, index_bytes()).unwrap();
    ScipRun {
      indexer: "fake-scip".to_owned(),
      version: Some("1".to_owned()),
      duration: Duration::ZERO,
      output_path: Some(self.output.clone()),
      outcome,
      cache: CacheStatus::Miss,
      degradations: Vec::new(),
      warnings: Vec::new(),
    }
  }
}

fn occurrence(symbol: &str, range: Vec<i32>, definition: bool) -> Occurrence {
  let mut occurrence = Occurrence::new();
  occurrence.symbol = symbol.to_owned();
  occurrence.range = range;
  if definition {
    occurrence.symbol_roles = SymbolRole::Definition as i32;
  }
  occurrence
}

fn document(path: &str, occurrences: Vec<Occurrence>) -> Document {
  let mut document = Document::new();
  document.relative_path = path.to_owned();
  document.position_encoding = EnumOrUnknown::new(PositionEncoding::UTF16CodeUnitOffsetFromLineStart);
  document.occurrences = occurrences;
  document
}

fn index_bytes() -> Vec<u8> {
  let load = "scip-typescript npm demo 1.0.0 src/a.ts`load().";
  let run = "scip-typescript npm demo 1.0.0 src/b.ts`run().";
  let mut tool = ToolInfo::new();
  tool.name = "scip-typescript".to_owned();
  let mut metadata = Metadata::new();
  metadata.tool_info = Some(tool).into();
  let mut index = Index::new();
  index.metadata = Some(metadata).into();
  index.documents = vec![
    document("src/a.ts", vec![occurrence(load, vec![0, 16, 20], true)]),
    document("src/b.ts", vec![occurrence(run, vec![1, 16, 19], true), occurrence(load, vec![2, 9, 13], false)]),
  ];
  index.write_to_bytes().unwrap()
}

struct Fixture {
  _scratch: tempfile::TempDir,
  root: PathBuf,
  engine: Arc<Engine>,
  calls: Arc<AtomicUsize>,
}

fn demo_project(root: &Path, enabled: bool) -> Project {
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(root.join("package.json"), "{ \"name\": \"demo\" }\n").unwrap();
  fs::write(root.join("src/a.ts"), A_SOURCE).unwrap();
  fs::write(root.join("src/b.ts"), B_SOURCE).unwrap();
  let mut project = project("demo", vec![repo_config("demo", root)]);
  project.settings.scip.enabled = enabled;
  project.settings.scip.debounce_secs = 1;
  project
}

fn fixture(enabled: bool, behaviour: Behaviour, retry: Duration) -> Fixture {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("demo");
  let project = demo_project(&root, enabled);
  let calls = Arc::new(AtomicUsize::new(0));
  let fake = FakeScip {
    output: scratch.path().join("index.scip"),
    calls: Arc::clone(&calls),
    behaviour,
  };
  let options = EngineOptions {
    watch_debounce: Duration::from_millis(50),
    auto_scip: AutoScipOptions {
      cargo: None,
      npm: Some(Arc::new(fake)),
      unavailable_retry: retry,
      failure_retry: retry,
    },
    ..EngineOptions::default()
  };
  let engine = Arc::new(open_with(&project, &scratch.path().join("home"), options));
  assert!(fresh(&engine).is_fresh());
  Fixture {
    root,
    engine,
    calls,
    _scratch: scratch,
  }
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
  let deadline = Instant::now() + WAIT_DEADLINE;
  while !condition() {
    assert!(Instant::now() < deadline, "timed out waiting for {what}");
    thread::sleep(Duration::from_millis(20));
  }
}

fn scip_owner(path: &str) -> Owner {
  Owner::Project {
    pass: ScipFacts::owner_pass(&RepoName::new("demo").unwrap(), &RelPath::new(path).unwrap()),
  }
}

fn file_owner_facts(engine: &Engine) -> BTreeMap<String, String> {
  common::dump(&engine.db_path())
    .into_iter()
    .filter(|(key, _)| key.split(' ').nth(1) == Some("file"))
    .collect()
}

fn indexed_hash(engine: &Engine, path: &str) -> Option<ContentHash> {
  engine
    .store()
    .read(|tx| tx.file(&RepoName::new("demo").unwrap(), &RelPath::new(path).unwrap()))
    .unwrap()
    .map(|file| file.content_hash)
}

fn index_edit(engine: &Engine) {
  engine.index(IndexMode::Incremental).unwrap();
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

fn append(path: &Path, text: &str) {
  let mut content = fs::read_to_string(path).unwrap();
  content.push_str(text);
  fs::write(path, content).unwrap();
}

#[test]
fn should_clear_the_scip_lag_after_the_debounce_without_an_explicit_run() {
  let fixture = fixture(true, Behaviour::Index, Duration::from_secs(3600));
  assert_eq!(fresh(&fixture.engine).header().scip, ScipLag::NeverRun);
  let _watch = fixture.engine.watch().unwrap();

  wait_until("the first automatic run", || fresh(&fixture.engine).header().scip == ScipLag::Current);
  append(&fixture.root.join("src/b.ts"), "export const later = 1;\n");
  index_edit(&fixture.engine);
  wait_until("the second automatic run to start", || fixture.calls.load(Ordering::SeqCst) == 2);
  wait_until("the lag to clear", || fresh(&fixture.engine).header().scip == ScipLag::Current);

  assert_eq!(fixture.calls.load(Ordering::SeqCst), 2);
  let edges = fixture.engine.store().read(|tx| tx.owned_facts(&scip_owner("src/b.ts"))).unwrap().edges;
  assert_eq!(edges.len(), 1);
  assert_eq!(edges[0].kind, EdgeKind::Calls);
  assert_eq!(edges[0].certainty.clone(), Certainty::CompilerVerified);
  let auto = fixture.engine.status().unwrap().scip.auto;
  assert_eq!(auto.setting, AutoScipSetting::On { debounce_secs: 1 });
  let last = auto.last_run.unwrap();
  assert_eq!((last.files_ingested, last.edges, last.outcome), (2, 1, AutoScipOutcome::Completed { packages_failed: 0 }));
  assert!(auto.backoff.is_empty());
  let runs = fixture.engine.store().read(|tx| tx.runs(50)).unwrap();
  assert_eq!(runs.iter().filter(|run| run.trigger == "scip-auto").count(), 2);
}

#[test]
fn should_not_run_automatically_when_scip_is_disabled_but_still_run_explicitly() {
  let fixture = fixture(false, Behaviour::Index, Duration::from_secs(3600));
  let watch = fixture.engine.watch().unwrap();

  thread::sleep(NO_RETRY_WINDOW);

  assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
  assert_eq!(fresh(&fixture.engine).header().scip, ScipLag::NeverRun);
  let status = fixture.engine.status().unwrap().scip.auto;
  assert_eq!((status.setting, status.last_run), (AutoScipSetting::Off, None));
  drop(watch);
  let explicit = FakeScip {
    output: fixture.root.parent().unwrap().join("explicit.scip"),
    calls: Arc::clone(&fixture.calls),
    behaviour: Behaviour::Index,
  };
  fixture
    .engine
    .scip(&atlas_engine::ScipIndexers {
      cargo: None,
      npm: Some(&explicit),
    })
    .unwrap();
  assert_eq!(fresh(&fixture.engine).header().scip, ScipLag::Current);
}

#[test]
fn should_back_off_when_the_indexer_is_unavailable_without_degrading_barriers() {
  let fixture = fixture(true, Behaviour::Unavailable, Duration::from_secs(3600));
  let watch = fixture.engine.watch().unwrap();

  wait_until("the first attempt", || fixture.calls.load(Ordering::SeqCst) == 1);
  wait_until("the backoff to be recorded", || !fixture.engine.status().unwrap().scip.auto.backoff.is_empty());
  let before = indexed_hash(&fixture.engine, "src/b.ts");
  append(&fixture.root.join("src/b.ts"), "export const later = 1;\n");
  wait_until("the edit to be indexed", || {
    fresh(&fixture.engine).is_fresh() && indexed_hash(&fixture.engine, "src/b.ts") != before
  });
  wait_for_quiet_events(&watch);
  thread::sleep(NO_RETRY_WINDOW);

  assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
  let report = fresh(&fixture.engine);
  assert!(report.is_fresh(), "{report:?}");
  assert_eq!(report.header().scip, ScipLag::NeverRun);
  let auto = fixture.engine.status().unwrap().scip.auto;
  assert_eq!(auto.backoff.len(), 1);
  assert_eq!((auto.backoff[0].indexer.as_str(), auto.backoff[0].reason.as_str()), ("fake-scip", "npx is not on PATH"));
  assert_eq!(auto.last_run.unwrap().outcome, AutoScipOutcome::Completed { packages_failed: 1 });
}

#[test]
fn should_retry_an_unavailable_indexer_after_the_backoff_expires() {
  let fixture = fixture(true, Behaviour::Unavailable, Duration::from_secs(2));
  let _watch = fixture.engine.watch().unwrap();

  wait_until("a retry after the backoff", || fixture.calls.load(Ordering::SeqCst) == 2);

  assert!(fresh(&fixture.engine).is_fresh());
}

#[test]
fn should_answer_barriers_while_a_run_indexes_and_leave_files_edited_meanwhile_lagging() {
  let gate = Arc::new(Gate::default());
  let fixture = fixture(true, Behaviour::Blocking(Arc::clone(&gate)), Duration::from_secs(3600));
  let watch = fixture.engine.watch().unwrap();
  wait_until("the automatic run to start", || fixture.calls.load(Ordering::SeqCst) == 1);
  assert!(fixture.engine.status().unwrap().scip.auto.in_progress);

  let before = indexed_hash(&fixture.engine, "src/b.ts");
  append(&fixture.root.join("src/b.ts"), "export const later = 1;\n");
  let mut barrier = Duration::ZERO;
  wait_until("the edit to be indexed", || {
    let started = Instant::now();
    fixture.engine.ensure_fresh().unwrap();
    if indexed_hash(&fixture.engine, "src/b.ts") == before {
      index_edit(&fixture.engine);
    }
    barrier = barrier.max(started.elapsed());
    indexed_hash(&fixture.engine, "src/b.ts") != before
  });
  let file_owners = file_owner_facts(&fixture.engine);
  gate.release();
  wait_until("the first run to finish", || fixture.engine.status().unwrap().scip.auto.last_run.is_some());

  assert!(barrier < Duration::from_secs(5), "barrier took {barrier:?}");
  let last = fixture.engine.status().unwrap().scip.auto.last_run.unwrap();
  assert_eq!(last.files_ingested, 1);
  assert_eq!(fresh(&fixture.engine).header().scip, ScipLag::Behind { files: 1 });
  assert!(fixture.engine.store().read(|tx| tx.owned_facts(&scip_owner("src/b.ts"))).unwrap().is_empty());
  assert_eq!(file_owner_facts(&fixture.engine), file_owners);
  gate.release();
  wait_until("the second run to clear the lag", || fresh(&fixture.engine).header().scip == ScipLag::Current);
  assert_eq!(fixture.calls.load(Ordering::SeqCst), 2);
  drop(watch);
  assert!(!fixture.engine.status().unwrap().scip.auto.in_progress);
}

#[test]
fn should_skip_a_failing_package_until_its_inputs_change() {
  let fixture = fixture(true, Behaviour::Failing, Duration::from_secs(3600));
  let watch = fixture.engine.watch().unwrap();
  wait_until("the failing attempt to be recorded", || fixture.engine.status().unwrap().scip.auto.last_run.is_some());
  let auto = fixture.engine.status().unwrap().scip.auto;
  assert_eq!(auto.backoff.len(), 1);
  assert_eq!(
    (auto.backoff[0].package.as_deref(), auto.backoff[0].reason.as_str()),
    (Some("demo:package.json"), "fake-scip exited with status 101")
  );

  fs::write(fixture.root.join("notes.rs"), "pub fn note() {}\n").unwrap();
  index_edit(&fixture.engine);
  assert!(indexed_hash(&fixture.engine, "notes.rs").is_some());
  wait_for_quiet_events(&watch);
  thread::sleep(NO_RETRY_WINDOW);
  assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);

  append(&fixture.root.join("src/b.ts"), "export const later = 1;\n");
  index_edit(&fixture.engine);
  wait_until("a retry after the package changed", || fixture.calls.load(Ordering::SeqCst) == 2);
  assert!(fresh(&fixture.engine).is_fresh());
}

#[test]
fn should_retry_a_failing_package_after_the_failure_backoff_expires() {
  let fixture = fixture(true, Behaviour::Failing, Duration::from_secs(2));
  let _watch = fixture.engine.watch().unwrap();

  wait_until("a retry after the backoff", || fixture.calls.load(Ordering::SeqCst) == 2);
}

#[test]
fn should_record_a_run_cancelled_during_its_last_package_as_partial() {
  let gate = Arc::new(Gate::default());
  let fixture = fixture(true, Behaviour::Blocking(gate), Duration::from_secs(3600));
  let watch = fixture.engine.watch().unwrap();
  wait_until("the automatic run to start", || fixture.calls.load(Ordering::SeqCst) == 1);

  drop(watch);

  let runs = fixture.engine.store().read(|tx| tx.runs(50)).unwrap();
  let run = runs.iter().find(|run| run.trigger == "scip-auto").unwrap();
  assert_eq!(
    run.status,
    atlas_store::RunStatus::Partial {
      reason: "cancelled before every package was indexed".to_owned()
    }
  );
  assert_eq!(fixture.engine.status().unwrap().scip.auto.last_run.unwrap().outcome, AutoScipOutcome::Cancelled);
  assert_eq!(fresh(&fixture.engine).header().scip, ScipLag::NeverRun);
}
