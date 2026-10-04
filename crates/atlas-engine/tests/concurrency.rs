mod common;

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use atlas_core::{EntityKind, RunId};
use atlas_engine::{Engine, EngineOptions, FaultInjection, FreshnessReport, IndexMode};
use atlas_store::EntityFilter;
use common::{acme_copy, fresh, open, open_with};

const BURST: usize = 16;
const DEADLOCK_WATCHDOG: Duration = Duration::from_secs(120);

fn append(path: &Path, text: &str) {
  let mut content = fs::read_to_string(path).unwrap();
  content.push_str(text);
  fs::write(path, content).unwrap();
}

fn functions_named(engine: &Engine, name: &str) -> usize {
  engine
    .store()
    .read(|tx| tx.entities_by_name(name, &EntityFilter::default()))
    .unwrap()
    .into_iter()
    .filter(|entity| entity.kind == EntityKind::Function)
    .count()
}

fn run_count(engine: &Engine) -> usize {
  engine.store().read(|tx| tx.runs(10_000)).unwrap().len()
}

struct Finished {
  done: mpsc::Sender<()>,
}

impl Drop for Finished {
  fn drop(&mut self) {
    let _ = self.done.send(());
  }
}

fn marker(index: usize) -> String {
  format!("\nexport function arrivalMarker{index}() {{\n  return {index};\n}}\n")
}

#[test]
fn should_serve_a_burst_of_sixteen_barriers_with_at_most_two_scans() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let options = EngineOptions {
    fault_injection: FaultInjection::before_index_lock(|| thread::sleep(Duration::from_millis(300))),
    ..EngineOptions::default()
  };
  let engine = Arc::new(open_with(&project, &scratch.path().join("home"), options));
  fresh(&engine);
  let (scans, runs) = (engine.barrier_scans(), run_count(&engine));
  append(&tree.join("acme-sdk/src/money.ts"), &marker(0));
  let start = Arc::new(Barrier::new(BURST));

  let reports: Vec<FreshnessReport> = (0..BURST)
    .map(|_| {
      let (engine, start) = (Arc::clone(&engine), Arc::clone(&start));
      thread::spawn(move || {
        start.wait();
        engine.ensure_fresh().unwrap()
      })
    })
    .collect::<Vec<_>>()
    .into_iter()
    .map(|worker| worker.join().unwrap())
    .collect();

  assert_eq!(
    engine.barrier_scans() - scans,
    2,
    "the first arrival indexes, every later arrival shares one follow-up scan"
  );
  assert_eq!(run_count(&engine), runs + 1);
  assert_eq!(reports.iter().filter(|report| report.header().run.is_some()).count(), 1, "{reports:?}");
  assert!(reports.iter().all(FreshnessReport::is_fresh), "{reports:?}");
  assert_eq!(functions_named(&engine, "arrivalMarker0"), 1);
}

#[test]
fn should_always_show_an_edit_made_before_the_barrier_arrived_while_other_barriers_are_in_flight() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let engine = Arc::new(open(&project, &scratch.path().join("home")));
  fresh(&engine);
  let stop = Arc::new(AtomicBool::new(false));
  let background: Vec<_> = (0..6)
    .map(|_| {
      let (engine, stop) = (Arc::clone(&engine), Arc::clone(&stop));
      thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
          assert!(engine.ensure_fresh().unwrap().is_fresh());
        }
      })
    })
    .collect();
  let money = tree.join("acme-sdk/src/money.ts");

  for index in 1..=12 {
    append(&money, &marker(index));
    let report = engine.ensure_fresh().unwrap();
    assert!(report.is_fresh(), "{report:?}");
    assert_eq!(
      functions_named(&engine, &format!("arrivalMarker{index}")),
      1,
      "edit {index} must be visible to the barrier that followed it"
    );
  }

  stop.store(true, Ordering::SeqCst);
  for worker in background {
    worker.join().unwrap();
  }
}

#[test]
fn should_keep_serving_barriers_reads_and_explicit_runs_under_an_edit_stream_without_deadlock() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let engine = Arc::new(open(&project, &scratch.path().join("home")));
  fresh(&engine);
  let stop = Arc::new(AtomicBool::new(false));
  let served = Arc::new(AtomicU64::new(0));
  let edits = 40;
  let (done, finished) = mpsc::channel::<()>();

  let mut workers = Vec::new();
  {
    let (stop, done, money) = (Arc::clone(&stop), done.clone(), tree.join("acme-sdk/src/money.ts"));
    workers.push(thread::spawn(move || {
      let _finished = Finished { done };
      for index in 0..edits {
        append(&money, &format!("\nexport const streamed{index} = {index};\n"));
        thread::sleep(Duration::from_millis(25));
      }
      stop.store(true, Ordering::SeqCst);
    }));
  }
  for _ in 0..8 {
    let (engine, stop, served, done) = (Arc::clone(&engine), Arc::clone(&stop), Arc::clone(&served), done.clone());
    workers.push(thread::spawn(move || {
      let _finished = Finished { done };
      while !stop.load(Ordering::SeqCst) {
        let report = engine.ensure_fresh().unwrap();
        assert!(report.is_fresh(), "{report:?}");
        assert!(functions_named(&engine, "toCents") >= 1);
        served.fetch_add(1, Ordering::SeqCst);
      }
    }));
  }
  {
    let (engine, stop, done) = (Arc::clone(&engine), Arc::clone(&stop), done.clone());
    workers.push(thread::spawn(move || {
      let _finished = Finished { done };
      while !stop.load(Ordering::SeqCst) {
        engine.index(IndexMode::Incremental).unwrap();
        engine.status().unwrap();
        thread::sleep(Duration::from_millis(40));
      }
    }));
  }
  drop(done);

  let started = Instant::now();
  for _ in 0..workers.len() {
    let remaining = DEADLOCK_WATCHDOG.saturating_sub(started.elapsed());
    finished
      .recv_timeout(remaining)
      .expect("a worker did not finish within the watchdog: the barrier deadlocked under the edit stream");
  }
  for worker in workers {
    worker.join().unwrap();
  }

  assert!(served.load(Ordering::SeqCst) > 0);
  assert!(fresh(&engine).is_fresh());
  let constants = engine
    .store()
    .read(|tx| tx.entities_by_name(&format!("streamed{}", edits - 1), &EntityFilter::default()))
    .unwrap();
  assert_eq!(constants.len(), 1, "the last streamed edit must be indexed");
}

#[test]
fn should_keep_a_read_snapshot_on_its_generation_while_a_barrier_commits_a_newer_one() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let engine = Arc::new(open(&project, &scratch.path().join("home")));
  let observed = fresh(&engine).header().run.unwrap();
  let (opened, snapshot_open) = mpsc::channel();
  let (committed, commit_seen) = mpsc::channel::<()>();

  let reader = {
    let engine = Arc::clone(&engine);
    thread::spawn(move || {
      engine
        .store()
        .read(|tx| -> atlas_store::StoreResult<_> {
          let latest = |tx: &atlas_store::ReadTx<'_>| -> atlas_store::StoreResult<(RunId, usize)> {
            let runs = tx.runs(1)?;
            let markers = tx.entities_by_name("snapshotMarker", &EntityFilter::default())?.len();
            Ok((runs[0].id, markers))
          };
          let before = latest(tx)?;
          opened.send(()).unwrap();
          commit_seen.recv().unwrap();
          Ok((before, latest(tx)?))
        })
        .unwrap()
    })
  };
  snapshot_open.recv().unwrap();
  append(&tree.join("acme-sdk/src/money.ts"), "\nexport function snapshotMarker() {\n  return 1;\n}\n");
  let newer = fresh(&engine).header().run.unwrap();
  committed.send(()).unwrap();
  let (before, after) = reader.join().unwrap();

  assert!(newer > observed);
  assert_eq!(before, (observed, 0));
  assert_eq!(after, (observed, 0), "a snapshot must not see a generation committed after it opened");
  assert_eq!(functions_named(&engine, "snapshotMarker"), 1);
}
