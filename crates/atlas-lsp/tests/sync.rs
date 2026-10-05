mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use atlas_core::{ContentHash, RunId};
use atlas_lsp::{
  ChangeKind, ChangeList, FileChange, LspPool, PoolOptions, RestartReason, ServerState, ServerUnit, Settle, SkipReason, Skipped, SyncDemand, SyncLimits, SyncReport, uri_from_path,
};
use common::fake::{Workspace, methods, rust_ready, ts_ready, wait_until, with};
use serde_json::{Value, json};

fn pool(limits: SyncLimits) -> LspPool {
  LspPool::new(PoolOptions {
    sync: limits,
    ..PoolOptions::one_shot()
  })
}

fn limits() -> SyncLimits {
  SyncLimits {
    settle: Duration::from_millis(50),
    quiescence: Duration::from_secs(5),
    ..SyncLimits::default()
  }
}

fn changed(path: &Path) -> FileChange {
  FileChange::new(path, ChangeKind::Changed(ContentHash::of(&fs::read(path).unwrap())))
}

fn sync(pool: &LspPool, unit: &ServerUnit, generation: i64, changes: Vec<FileChange>, open: &[PathBuf]) -> SyncReport {
  pool
    .sync(unit, Some(RunId(generation)), SyncDemand::Best, |_| ChangeList::Known(changes.clone()), open)
    .unwrap()
    .report
}

fn flush(pool: &LspPool, unit: &ServerUnit, generation: i64) {
  let lease = pool.get(unit, Some(RunId(generation))).unwrap();
  assert!(lease.request::<Value>("fake/flush", json!({})).is_err());
}

fn uri(path: &Path) -> String {
  uri_from_path(path).unwrap()
}

fn recording_unit(workspace: &Workspace) -> ServerUnit {
  workspace.rust_unit("demo", workspace.script("ra", &with(rust_ready(), [workspace.record("ra")])))
}

#[test]
fn should_open_changed_files_as_overlays_up_to_the_limit() {
  let workspace = Workspace::new();
  let file = workspace.write("src/a.rs", "fn a() {}\n");
  let unit = recording_unit(&workspace);
  let pool = pool(limits());
  pool.get(&unit, Some(RunId(1))).unwrap();

  let opened = pool
    .sync(
      &unit,
      Some(RunId(2)),
      SyncDemand::Best,
      |since| {
        assert_eq!(since, Some(RunId(1)));
        ChangeList::Known(vec![changed(&file)])
      },
      &[],
    )
    .unwrap()
    .report;
  fs::write(&file, "fn a() { 1; }\n").unwrap();
  let edited = sync(&pool, &unit, 3, vec![changed(&file)], &[]);
  flush(&pool, &unit, 3);

  assert_eq!(opened.opened, vec![file.clone()]);
  assert_eq!(opened.watched, 1);
  assert_eq!(opened.settle, Settle::Quiet);
  assert_eq!(edited.changed, vec![file.clone()]);
  assert_eq!((edited.opened.len(), edited.closed.len(), edited.drifted.len()), (0, 0, 0));
  let recorded = workspace.recorded("ra");
  assert_eq!(
    methods(&recorded),
    vec![
      "textDocument/didOpen",
      "workspace/didChangeWatchedFiles",
      "textDocument/didChange",
      "workspace/didChangeWatchedFiles",
      "fake/flush"
    ]
  );
  assert_eq!(
    recorded[0]["params"],
    json!({"textDocument": {"uri": uri(&file), "languageId": "rust", "version": 1, "text": "fn a() {}\n"}})
  );
  assert_eq!(recorded[1]["params"], json!({"changes": [{"uri": uri(&file), "type": 2}]}));
  assert_eq!(
    recorded[2]["params"],
    json!({"textDocument": {"uri": uri(&file), "version": 2}, "contentChanges": [{"text": "fn a() { 1; }\n"}]})
  );
  assert_eq!(pool.get(&unit, Some(RunId(3))).unwrap().synced_generation(), Some(RunId(3)));
}

#[test]
fn should_announce_changes_beyond_the_overlay_limit_as_watched_files() {
  let workspace = Workspace::new();
  let files: Vec<PathBuf> = ["a", "b", "c"].iter().map(|name| workspace.write(&format!("src/{name}.rs"), "fn x() {}\n")).collect();
  let unit = recording_unit(&workspace);
  let pool = pool(SyncLimits { overlay_files: 2, ..limits() });
  sync(&pool, &unit, 1, vec![], std::slice::from_ref(&files[0]));
  for file in &files {
    fs::write(file, "fn y() {}\n").unwrap();
  }

  let report = sync(&pool, &unit, 2, files.iter().map(|file| changed(file)).collect(), &[]);
  flush(&pool, &unit, 2);

  assert_eq!(report.changed, vec![files[0].clone()]);
  assert_eq!(report.opened, Vec::<PathBuf>::new());
  assert_eq!(report.watched, 3);
  let recorded = workspace.recorded("ra");
  assert_eq!(
    methods(&recorded),
    vec!["textDocument/didOpen", "textDocument/didChange", "workspace/didChangeWatchedFiles", "fake/flush"]
  );
  assert_eq!(
    recorded[2]["params"]["changes"],
    json!(files.iter().map(|file| json!({"uri": uri(file), "type": 2})).collect::<Vec<_>>())
  );
}

#[test]
fn should_close_an_open_document_that_exceeds_the_byte_budget() {
  let workspace = Workspace::new();
  let large = workspace.write("src/a.rs", "fn a() {}\n");
  let unit = recording_unit(&workspace);
  let pool = pool(SyncLimits { overlay_bytes: 16, ..limits() });
  sync(&pool, &unit, 1, vec![], std::slice::from_ref(&large));
  fs::write(&large, "fn a() { let value = 1 + 2 + 3 + 4; }\n").unwrap();
  let small = workspace.write("src/b.rs", "fn b(){}");

  let report = sync(
    &pool,
    &unit,
    2,
    vec![changed(&large), FileChange::new(&small, ChangeKind::Created(ContentHash::of(b"fn b(){}")))],
    &[],
  );
  flush(&pool, &unit, 2);

  assert_eq!(report.closed, vec![large.clone()]);
  assert_eq!(report.opened, vec![small.clone()]);
  assert_eq!(
    report.skipped,
    vec![Skipped {
      path: large.clone(),
      reason: SkipReason::OverBudget { bytes: 38 }
    }]
  );
  let recorded = workspace.recorded("ra");
  assert_eq!(
    methods(&recorded),
    vec![
      "textDocument/didOpen",
      "textDocument/didClose",
      "textDocument/didOpen",
      "workspace/didChangeWatchedFiles",
      "fake/flush"
    ]
  );
  assert_eq!(recorded[3]["params"]["changes"], json!([{"uri": uri(&large), "type": 2}, {"uri": uri(&small), "type": 1}]));
}

#[test]
fn should_restart_the_unit_when_a_change_set_exceeds_the_threshold() {
  let workspace = Workspace::new();
  let files: Vec<PathBuf> = ["a", "b", "c"].iter().map(|name| workspace.write(&format!("src/{name}.rs"), "fn x() {}\n")).collect();
  let unit = recording_unit(&workspace);
  let pool = pool(SyncLimits { watched_files: 2, ..limits() });
  let first = pool.get(&unit, Some(RunId(1))).unwrap().pid();

  let synced = pool
    .sync(
      &unit,
      Some(RunId(2)),
      SyncDemand::Best,
      |_| ChangeList::Known(files.iter().map(|file| changed(file)).collect()),
      &files[..1],
    )
    .unwrap();

  assert_eq!(synced.report.restarted, Some(RestartReason::TooManyChanges { count: 3, limit: 2 }));
  assert_eq!(synced.report.opened, vec![files[0].clone()]);
  assert_eq!(synced.report.watched, 0);
  assert_ne!(synced.lease.pid(), first);
  assert_eq!(synced.lease.synced_generation(), Some(RunId(2)));
  assert_eq!(synced.lease.start_cause(), Some(&RestartReason::TooManyChanges { count: 3, limit: 2 }));
  assert!(synced.lease.request::<Value>("fake/flush", json!({})).is_err());
  assert_eq!(methods(&workspace.recorded("ra")), vec!["textDocument/didOpen", "fake/flush"]);
}

#[test]
fn should_restart_the_unit_when_the_changes_are_unknown() {
  let workspace = Workspace::new();
  let unit = recording_unit(&workspace);
  let pool = pool(limits());
  let first = pool.get(&unit, Some(RunId(1))).unwrap().pid();

  let synced = pool.sync(&unit, Some(RunId(9)), SyncDemand::Best, |_| ChangeList::Unknown, &[]).unwrap();

  assert_eq!(synced.report.restarted, Some(RestartReason::UnknownChanges));
  assert_ne!(synced.lease.pid(), first);
  assert_eq!(synced.lease.synced_generation(), Some(RunId(9)));
}

#[test]
fn should_resynchronise_a_server_that_died_after_accepting_a_notification() {
  let workspace = Workspace::new();
  let file = workspace.write("src/a.rs", "fn a() {}\n");
  let crashing = with(rust_ready(), [json!({"recv": {"method": "textDocument/didOpen"}}), json!({"exit": 3})]);
  let unit = workspace.rust_unit("demo", workspace.script("ra", &crashing));
  let pool = pool(limits());
  let synced = pool.sync(&unit, Some(RunId(1)), SyncDemand::Best, |_| unreachable!(), std::slice::from_ref(&file)).unwrap();
  assert_eq!(synced.report.opened, vec![file.clone()]);
  assert!(wait_until(|| matches!(synced.lease.state(), ServerState::Exited { .. })));
  drop(synced);
  workspace.script("ra", &with(rust_ready(), [workspace.record("ra")]));

  let resynced = pool.sync(&unit, Some(RunId(1)), SyncDemand::Best, |_| unreachable!(), std::slice::from_ref(&file)).unwrap();

  assert_eq!(resynced.lease.start_cause(), Some(&RestartReason::Crashed));
  assert_eq!(resynced.report.opened, vec![file.clone()]);
  assert!(resynced.lease.request::<Value>("fake/flush", json!({})).is_err());
  let recorded = workspace.recorded("ra");
  assert_eq!(methods(&recorded), vec!["textDocument/didOpen", "fake/flush"]);
  assert_eq!(recorded[0]["params"]["textDocument"]["version"], json!(1));
}

#[test]
fn should_report_drift_missing_unreadable_and_foreign_files() {
  let workspace = Workspace::new();
  let drifted = workspace.write("src/a.rs", "fn a() { 2 }\n");
  let missing = workspace.root.join("src/gone.rs");
  let binary = workspace.root.join("src/bin.rs");
  fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
  let manifest = workspace.write("Cargo.toml", "[package]\n");
  let outside = workspace.path("elsewhere.rs");
  fs::write(&outside, "fn x() {}\n").unwrap();
  let unit = recording_unit(&workspace);
  let pool = pool(limits());
  pool.get(&unit, Some(RunId(0))).unwrap();

  let report = sync(
    &pool,
    &unit,
    1,
    vec![
      FileChange::new(&drifted, ChangeKind::Changed(ContentHash::of(b"fn a() { 1 }\n"))),
      FileChange::new(&missing, ChangeKind::Created(ContentHash::of(b"fn gone() {}\n"))),
      FileChange::new(&binary, ChangeKind::Changed(ContentHash::of(&[0xff, 0xfe, 0x00]))),
      FileChange::new(&manifest, ChangeKind::Changed(ContentHash::of(b"[package]\n"))),
      FileChange::new(&outside, ChangeKind::Changed(ContentHash::of(b"fn x() {}\n"))),
    ],
    &[],
  );
  flush(&pool, &unit, 1);

  assert_eq!(report.opened, vec![drifted.clone()]);
  assert_eq!(report.drifted, vec![drifted.clone(), missing.clone()]);
  assert_eq!(
    report.skipped,
    vec![
      Skipped {
        path: binary.clone(),
        reason: SkipReason::NotUtf8
      },
      Skipped {
        path: missing.clone(),
        reason: SkipReason::Missing
      },
    ]
  );
  assert_eq!(report.watched, 4);
  let recorded = workspace.recorded("ra");
  assert_eq!(
    recorded[1]["params"]["changes"],
    json!([
      {"uri": uri(&manifest), "type": 2},
      {"uri": uri(&drifted), "type": 2},
      {"uri": uri(&binary), "type": 2},
      {"uri": uri(&missing), "type": 3},
    ])
  );
}

#[test]
fn should_refresh_open_documents_that_changed_outside_the_change_set() {
  let workspace = Workspace::new();
  let kept = workspace.write("src/a.rs", "fn a() {}\n");
  let removed = workspace.write("src/b.rs", "fn b() {}\n");
  let unit = recording_unit(&workspace);
  let pool = pool(limits());
  sync(&pool, &unit, 1, vec![], &[kept.clone(), removed.clone()]);
  fs::write(&kept, "fn a() { changed() }\n").unwrap();
  fs::remove_file(&removed).unwrap();

  let report = sync(&pool, &unit, 2, vec![], &[]);
  flush(&pool, &unit, 2);

  assert_eq!(report.changed, vec![kept.clone()]);
  assert_eq!(report.closed, vec![removed.clone()]);
  assert_eq!(report.drifted, vec![kept.clone(), removed.clone()]);
  let recorded = workspace.recorded("ra");
  assert_eq!(
    methods(&recorded),
    vec![
      "textDocument/didOpen",
      "textDocument/didOpen",
      "textDocument/didChange",
      "textDocument/didClose",
      "workspace/didChangeWatchedFiles",
      "fake/flush"
    ]
  );
  assert_eq!(recorded[2]["params"]["textDocument"]["version"], json!(3));
  assert_eq!(recorded[4]["params"]["changes"], json!([{"uri": uri(&removed), "type": 3}]));
}

#[test]
fn should_close_the_least_recently_used_documents_over_the_open_limit() {
  let workspace = Workspace::new();
  let files: Vec<PathBuf> = ["a", "b", "c"].iter().map(|name| workspace.write(&format!("src/{name}.rs"), "fn x() {}\n")).collect();
  let unit = recording_unit(&workspace);
  let pool = pool(SyncLimits { open_documents: 2, ..limits() });

  for (generation, file) in files.iter().enumerate() {
    sync(&pool, &unit, generation as i64, vec![], std::slice::from_ref(file));
  }
  let lease = pool.get(&unit, Some(RunId(2))).unwrap();

  assert_eq!(lease.open_document(&files[0]), None);
  assert_eq!(lease.open_document(&files[2]).map(|doc| doc.version), Some(3));
  assert_eq!(lease.close_documents(&files[1..2]).unwrap(), vec![files[1].clone()]);
  assert!(lease.request::<Value>("fake/flush", json!({})).is_err());
  assert_eq!(
    methods(&workspace.recorded("ra")),
    vec![
      "textDocument/didOpen",
      "textDocument/didOpen",
      "textDocument/didOpen",
      "textDocument/didClose",
      "textDocument/didClose",
      "fake/flush"
    ]
  );
}

#[test]
fn should_wait_for_rust_analyzer_to_settle_after_watched_file_changes() {
  let workspace = Workspace::new();
  let manifest = workspace.write("Cargo.toml", "[package]\n");
  let steps = with(
    rust_ready(),
    [
      json!({"recv": {"method": "workspace/didChangeWatchedFiles"}}),
      json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": false}}}),
      json!({"sleep_ms": 200}),
      json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": true}}}),
      json!({"recv": {"method": "workspace/didChangeWatchedFiles"}}),
      json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": false, "message": "Fetching"}}}),
    ],
  );
  let unit = workspace.rust_unit("demo", workspace.script("ra", &steps));
  let pool = pool(SyncLimits {
    quiescence: Duration::from_millis(300),
    settle: Duration::from_secs(5),
    ..limits()
  });
  pool.get(&unit, Some(RunId(1))).unwrap();
  let started = Instant::now();

  let settled = sync(&pool, &unit, 2, vec![changed(&manifest)], &[]);
  let waited = started.elapsed();
  let busy = sync(&pool, &unit, 3, vec![changed(&manifest)], &[]);

  assert_eq!(settled.settle, Settle::Ready);
  assert!(waited >= Duration::from_millis(200), "settled after {waited:?}");
  assert_eq!(
    busy.settle,
    Settle::Busy {
      status: "indexing: Fetching".to_owned()
    }
  );
}

#[test]
fn should_mark_typescript_changes_outside_overlays_as_unconfirmed() {
  let workspace = Workspace::new();
  let source = workspace.write("src/a.tsx", "export const a = <div />;\n");
  let config = workspace.write("tsconfig.json", "{}\n");
  let unit = workspace.ts_unit("demo", workspace.script("ts", &with(ts_ready(), [workspace.record("ts")])));
  let pool = pool(limits());
  pool.get(&unit, Some(RunId(1))).unwrap();

  let overlaid = sync(&pool, &unit, 2, vec![changed(&source)], &[]);
  let announced = sync(&pool, &unit, 3, vec![changed(&config)], &[]);
  flush(&pool, &unit, 3);

  assert_eq!(overlaid.opened, vec![source.clone()]);
  assert_eq!(overlaid.settle, Settle::NotNeeded);
  assert_eq!(announced.watched, 1);
  assert_eq!(announced.settle, Settle::Unconfirmed { paths: vec![config.clone()] });
  let recorded = workspace.recorded("ts");
  assert_eq!(recorded[0]["params"]["textDocument"]["languageId"], json!("typescriptreact"));
  assert_eq!(
    methods(&recorded),
    vec!["textDocument/didOpen", "workspace/didChangeWatchedFiles", "workspace/didChangeWatchedFiles", "fake/flush"]
  );
}

fn ts_recording_unit(workspace: &Workspace) -> ServerUnit {
  workspace.ts_unit("demo", workspace.script("ts", &with(ts_ready(), [workspace.record("ts")])))
}

fn created(path: &Path) -> FileChange {
  FileChange::new(path, ChangeKind::Created(ContentHash::of(&fs::read(path).unwrap())))
}

#[test]
fn should_remember_unconfirmed_typescript_changes_until_they_are_overlaid() {
  let workspace = Workspace::new();
  let money = workspace.write("src/money.ts", "export const toCents = (n: number) => n * 100;\n");
  let unit = ts_recording_unit(&workspace);
  let pool = pool(SyncLimits { overlay_files: 0, ..limits() });
  sync(&pool, &unit, 1, vec![], std::slice::from_ref(&money));
  let extra = workspace.write("src/extra.ts", "import { toCents } from './money';\nexport const e = toCents(9);\n");
  let readme = workspace.write("README.md", "# shop\n");

  let created_report = sync(&pool, &unit, 2, vec![created(&extra), changed(&readme)], std::slice::from_ref(&money));
  let quiet_report = sync(&pool, &unit, 3, vec![], std::slice::from_ref(&money));
  let opened_report = sync(&pool, &unit, 4, vec![], &[money.clone(), extra.clone()]);

  assert_eq!(created_report.opened, Vec::<PathBuf>::new());
  assert_eq!(created_report.watched, 2);
  assert_eq!(created_report.settle, Settle::Unconfirmed { paths: vec![extra.clone()] });
  assert_eq!(quiet_report.settle, Settle::Unconfirmed { paths: vec![extra.clone()] });
  assert_eq!(opened_report.opened, vec![extra.clone()]);
  assert_eq!(opened_report.settle, Settle::NotNeeded);
}

#[test]
fn should_forget_unconfirmed_changes_when_the_server_restarts() {
  let workspace = Workspace::new();
  let extra = workspace.write("src/extra.ts", "export const e = 1;\n");
  let unit = ts_recording_unit(&workspace);
  let pool = pool(SyncLimits { overlay_files: 0, ..limits() });
  pool.get(&unit, Some(RunId(1))).unwrap();
  assert_eq!(sync(&pool, &unit, 2, vec![created(&extra)], &[]).settle, Settle::Unconfirmed { paths: vec![extra.clone()] });

  let restarted = pool.sync(&unit, Some(RunId(3)), SyncDemand::Best, |_| ChangeList::Unknown, &[]).unwrap().report;

  assert_eq!(restarted.restarted, Some(RestartReason::UnknownChanges));
  assert_eq!(restarted.settle, Settle::NotNeeded);
}

#[test]
fn should_report_drift_in_a_requested_document() {
  let workspace = Workspace::new();
  let target = workspace.write("src/a.rs", "fn renamed() {}\n");
  let unit = recording_unit(&workspace);
  let pool = pool(limits());
  pool.get(&unit, Some(RunId(1))).unwrap();

  let report = sync(
    &pool,
    &unit,
    2,
    vec![FileChange::new(&target, ChangeKind::Changed(ContentHash::of(b"fn original() {}\n")))],
    std::slice::from_ref(&target),
  );

  assert_eq!(report.opened, vec![target.clone()]);
  assert_eq!(report.drifted, vec![target.clone()]);
}

#[test]
fn should_overlay_every_changed_typescript_file_for_an_exact_sync() {
  let workspace = Workspace::new();
  let unit = ts_recording_unit(&workspace);
  let pool = pool(SyncLimits { overlay_files: 0, ..limits() });
  pool.get(&unit, Some(RunId(1))).unwrap();
  let files: Vec<PathBuf> = ["a", "b"].iter().map(|name| workspace.write(&format!("src/{name}.ts"), "export {};\n")).collect();

  let report = pool
    .sync(
      &unit,
      Some(RunId(2)),
      SyncDemand::Exact,
      |_| ChangeList::Known(files.iter().map(|file| created(file)).collect()),
      &[],
    )
    .unwrap()
    .report;

  assert_eq!(report.opened, files);
  assert_eq!(report.settle, Settle::NotNeeded);
  assert_eq!(report.restarted, None);
}

#[test]
fn should_restart_typescript_when_an_exact_sync_cannot_be_confirmed() {
  let workspace = Workspace::new();
  let config = workspace.write("tsconfig.json", "{}\n");
  let unit = ts_recording_unit(&workspace);
  let pool = pool(limits());
  let first = pool.get(&unit, Some(RunId(1))).unwrap().pid();

  let synced = pool
    .sync(&unit, Some(RunId(2)), SyncDemand::Exact, |_| ChangeList::Known(vec![changed(&config)]), &[])
    .unwrap();

  assert_eq!(synced.report.restarted, Some(RestartReason::Unconfirmed { count: 1 }));
  assert_eq!(synced.report.settle, Settle::NotNeeded);
  assert_ne!(synced.lease.pid(), first);
  assert_eq!(synced.lease.synced_generation(), Some(RunId(2)));
}

#[test]
fn should_refuse_an_exact_sync_while_rust_analyzer_is_still_busy() {
  let workspace = Workspace::new();
  let manifest = workspace.write("Cargo.toml", "[package]\n");
  let source = workspace.write("src/a.rs", "fn a() {}\n");
  let steps = with(
    rust_ready(),
    [
      json!({"recv_all": ["textDocument/didOpen", "workspace/didChangeWatchedFiles"]}),
      json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": false, "message": "Fetching"}}}),
    ],
  );
  let unit = workspace.rust_unit("demo", workspace.script("ra", &steps));
  let pool = pool(SyncLimits {
    overlay_files: 0,
    quiescence: Duration::from_millis(300),
    ..limits()
  });
  pool.get(&unit, Some(RunId(1))).unwrap();

  let error = pool
    .sync(
      &unit,
      Some(RunId(2)),
      SyncDemand::Exact,
      |_| ChangeList::Known(vec![changed(&manifest), changed(&source)]),
      &[],
    )
    .err()
    .unwrap();

  assert_eq!(
    error.to_string(),
    "rust-analyzer could not be brought in sync with the files on disk for an exact operation: the server is still indexing: Fetching; retry once the server settles"
  );
}

#[test]
fn should_count_only_handled_files_against_the_overlay_limit() {
  let workspace = Workspace::new();
  let source = workspace.write("src/a.rs", "fn a() {}\n");
  let manifest = workspace.write("Cargo.toml", "[package]\n");
  let readme = workspace.write("README.md", "# a\n");
  let unit = recording_unit(&workspace);
  let pool = pool(SyncLimits { overlay_files: 1, ..limits() });
  pool.get(&unit, Some(RunId(1))).unwrap();

  let report = sync(&pool, &unit, 2, vec![changed(&source), changed(&manifest), changed(&readme)], &[]);

  assert_eq!(report.opened, vec![source]);
  assert_eq!(report.watched, 3);
}

#[test]
fn should_not_move_a_server_back_to_an_older_generation() {
  let workspace = Workspace::new();
  let source = workspace.write("src/a.rs", "fn a() {}\n");
  let unit = recording_unit(&workspace);
  let pool = pool(limits());
  pool.get(&unit, Some(RunId(1))).unwrap();
  sync(&pool, &unit, 3, vec![changed(&source)], &[]);

  let older = pool
    .sync(&unit, Some(RunId(2)), SyncDemand::Best, |since| panic!("asked for changes since {since:?}"), &[])
    .unwrap();

  assert_eq!(older.lease.synced_generation(), Some(RunId(3)));
  assert_eq!(older.report.opened, Vec::<PathBuf>::new());
}

#[test]
fn should_skip_the_settle_window_when_every_change_is_an_overlay() {
  let workspace = Workspace::new();
  let source = workspace.write("src/a.rs", "fn a() {}\n");
  let unit = recording_unit(&workspace);
  let pool = pool(SyncLimits {
    settle: Duration::from_secs(5),
    ..limits()
  });
  pool.get(&unit, Some(RunId(1))).unwrap();
  let started = Instant::now();

  let report = sync(&pool, &unit, 2, vec![changed(&source)], &[]);

  assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
  assert_eq!(report.opened, vec![source]);
  assert_eq!(report.settle, Settle::Quiet);
}

#[test]
fn should_confirm_a_typescript_deletion_by_opening_and_closing_the_document() {
  let workspace = Workspace::new();
  let unit = ts_recording_unit(&workspace);
  let pool = pool(limits());
  pool.get(&unit, Some(RunId(1))).unwrap();
  let gone = workspace.root.join("src/gone.ts");

  let report = sync(&pool, &unit, 2, vec![FileChange::new(&gone, ChangeKind::Deleted)], &[]);
  flush(&pool, &unit, 2);

  assert_eq!(report.settle, Settle::NotNeeded);
  assert_eq!((report.opened.len(), report.closed.len()), (0, 0));
  let recorded = workspace.recorded("ts");
  assert_eq!(
    methods(&recorded),
    vec!["textDocument/didOpen", "textDocument/didClose", "workspace/didChangeWatchedFiles", "fake/flush"]
  );
  assert_eq!(
    recorded[0]["params"],
    json!({"textDocument": {"uri": uri(&gone), "languageId": "typescript", "version": 1, "text": ""}})
  );
}
