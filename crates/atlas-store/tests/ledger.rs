mod common;

use std::collections::BTreeMap;

use atlas_core::{ContentHash, Language, ParseErrorFact, ParseStatus, Producer, RunId, TokenUsage};
use atlas_store::{CacheStatus, DerivedBlob, FileRecord, IndexRun, LlmCallOutcome, NewLlmCacheEntry, NewLlmCall, RunOutcome, RunReport, RunStatus, Store, StoreError};
use common::*;

fn file_record(path: &str, run: Option<RunId>) -> FileRecord {
  FileRecord {
    repo: repo(),
    path: rel(path),
    language: Some(Language::Rust),
    content_hash: ContentHash::of(path.as_bytes()),
    size: 1_234,
    mtime_ns: 1_700_000_000_123_456_789,
    inode: Some(u64::MAX - 7),
    parse_status: ParseStatus::Partial {
      error_count: 1,
      first_errors: vec![ParseErrorFact {
        span: span(4),
        message: "unexpected token".into(),
      }],
    },
    analyzer: Some(Producer::new("atlas-lang-rust", 3)),
    indexed_run: run,
  }
}

#[test]
fn should_track_the_index_run_lifecycle() {
  let store = Store::open_in_memory().unwrap();
  let report = RunReport {
    phase_timings_ms: BTreeMap::from([("parse".to_owned(), 12), ("resolve".to_owned(), 4)]),
    counts: BTreeMap::from([("files".to_owned(), 3)]),
    change_set: serde_json::json!({ "changed": ["src/lib.rs"] }),
  };

  let first = store.write(|tx| tx.begin_run("full")).unwrap();
  let running = read(&store, |tx| tx.run(first)).unwrap();
  store.write(|tx| tx.finish_run(first, RunOutcome::Ok, &report)).unwrap();
  let second = store.write(|tx| tx.begin_run("incremental")).unwrap();
  store
    .write(|tx| tx.finish_run(second, RunOutcome::Partial { reason: "2 files failed".into() }, &RunReport::default()))
    .unwrap();

  assert_eq!(running.status, RunStatus::Running);
  assert_eq!(running.finished_at_ms, None);
  let runs = read(&store, |tx| tx.runs(10));
  assert_eq!(runs.iter().map(|run| run.id).collect::<Vec<_>>(), vec![second, first]);
  let IndexRun {
    trigger,
    status,
    report: stored,
    finished_at_ms,
    ..
  } = runs[1].clone();
  assert_eq!((trigger.as_str(), status, stored), ("full", RunStatus::Ok, report));
  assert!(finished_at_ms.is_some());
  assert_eq!(runs[0].status, RunStatus::Partial { reason: "2 files failed".into() });
  assert_eq!(read(&store, |tx| tx.runs(1)).len(), 1);
}

#[test]
fn should_refuse_to_finish_or_resume_a_run_that_is_not_running() {
  let store = Store::open_in_memory().unwrap();
  let run = store.write(|tx| tx.begin_run("full")).unwrap();
  store
    .write(|tx| tx.finish_run(run, RunOutcome::Failed { error: "disk full".into() }, &RunReport::default()))
    .unwrap();

  let refinished = store.write(|tx| tx.finish_run(run, RunOutcome::Ok, &RunReport::default()));
  let resumed = store.write(|tx| tx.resume_run(run));
  let missing = store.write(|tx| tx.resume_run(RunId(99)));

  assert!(matches!(refinished, Err(StoreError::RunNotRunning { status: "failed", .. })));
  assert!(matches!(resumed, Err(StoreError::RunNotRunning { .. })));
  assert!(matches!(missing, Err(StoreError::RunNotFound(RunId(99)))));
  assert_eq!(read(&store, |tx| tx.run(run)).unwrap().status, RunStatus::Failed { error: "disk full".into() });
}

#[test]
fn should_refuse_to_begin_a_second_run_inside_one_write_transaction() {
  let store = Store::open_in_memory().unwrap();

  let (first, second, other) = store
    .write(|tx| {
      let first = tx.begin_run("full")?;
      let second = tx.begin_run("incremental");
      let other = tx.resume_run(RunId(first.0 + 100));
      tx.finish_run(first, RunOutcome::Ok, &RunReport::default())?;
      Ok::<_, StoreError>((first, second, other))
    })
    .unwrap();
  let next = store.write(|tx| tx.begin_run("incremental")).unwrap();

  assert!(matches!(second, Err(StoreError::RunAlreadyActive { active }) if active == first));
  assert!(matches!(other, Err(StoreError::RunAlreadyActive { active }) if active == first));
  assert_eq!(read(&store, |tx| tx.runs(10)).iter().map(|run| run.id).collect::<Vec<_>>(), vec![next, first]);
}

#[test]
fn should_store_and_list_file_records() {
  let store = Store::open_in_memory().unwrap();
  let run = store.write(|tx| tx.begin_run("full")).unwrap();
  let lib = file_record("src/lib.rs", Some(run));
  let main = file_record("src/main.rs", None);
  store
    .write(|tx| {
      tx.upsert_file(&main)?;
      tx.upsert_file(&lib)
    })
    .unwrap();

  let mut updated = lib.clone();
  updated.parse_status = ParseStatus::Ok;
  updated.analyzer = None;
  updated.inode = None;
  updated.language = None;
  store.write(|tx| tx.upsert_file(&updated)).unwrap();

  assert_eq!(read(&store, |tx| tx.file(&repo(), &rel("src/lib.rs"))), Some(updated.clone()));
  assert_eq!(read(&store, |tx| tx.files_in_repo(&repo())), vec![updated, main]);
  assert!(store.write(|tx| tx.delete_file(&repo(), &rel("src/main.rs"))).unwrap());
  assert!(!store.write(|tx| tx.delete_file(&repo(), &rel("src/main.rs"))).unwrap());
  assert_eq!(read(&store, |tx| tx.files_in_repo(&repo())).len(), 1);
}

#[test]
fn should_cache_llm_responses_and_ledger_calls() {
  let store = Store::open_in_memory().unwrap();
  let key = ContentHash::of(b"equivalence v1 evidence");
  let usage = TokenUsage {
    input: 900,
    output: 120,
    cache_read: 0,
    cache_write: 0,
  };
  let run = store.write(|tx| tx.begin_run("full")).unwrap();
  let call = NewLlmCall {
    run: Some(run),
    task_kind: "equivalence".into(),
    provider: "claude-cli".into(),
    model: "m".into(),
    usage,
    cost_estimate_usd: Some(0.0042),
    cache: CacheStatus::Miss,
    duration_ms: 1_800,
    outcome: LlmCallOutcome::Ok,
  };
  let failed = NewLlmCall {
    cache: CacheStatus::Hit,
    cost_estimate_usd: None,
    outcome: LlmCallOutcome::Failed { error: "timeout".into() },
    ..call.clone()
  };

  store
    .write(|tx| {
      tx.put_llm_cache(&NewLlmCacheEntry {
        key,
        provider: "claude-cli".into(),
        model: "m".into(),
        response: "{\"label\":\"equivalent\"}".into(),
        usage,
      })
    })
    .unwrap();
  let ids = store.write(|tx| Ok::<_, StoreError>((tx.record_llm_call(&call)?, tx.record_llm_call(&failed)?))).unwrap();

  let cached = read(&store, |tx| tx.llm_cache(&key)).unwrap();
  assert_eq!((cached.response.as_str(), cached.usage), ("{\"label\":\"equivalent\"}", usage));
  assert_eq!(read(&store, |tx| tx.llm_cache(&ContentHash::of(b"other"))), None);
  let calls = read(&store, |tx| tx.llm_calls(run));
  assert_eq!(calls.iter().map(|record| record.id).collect::<Vec<_>>(), vec![ids.0, ids.1]);
  assert_eq!(calls[0].call, call);
  assert_eq!(calls[1].call, failed);
}

#[test]
fn should_put_get_and_delete_derived_blobs() {
  let store = Store::open_in_memory().unwrap();
  let run = store
    .write(|tx| {
      let run = tx.begin_run("full")?;
      tx.put_blob("bm25", 1, &[1, 2, 3])?;
      tx.put_blob("bm25", 2, &[4, 5])?;
      Ok::<_, StoreError>(run)
    })
    .unwrap();

  assert_eq!(
    read(&store, |tx| tx.blob("bm25")),
    Some(DerivedBlob {
      name: "bm25".into(),
      version: 2,
      bytes: vec![4, 5],
      updated_run: Some(run),
    })
  );
  assert!(store.write(|tx| tx.delete_blob("bm25")).unwrap());
  assert_eq!(read(&store, |tx| tx.blob("bm25")), None);
  assert!(!store.write(|tx| tx.delete_blob("bm25")).unwrap());
}
