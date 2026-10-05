mod common;

use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use atlas_core::RunId;
use atlas_lsp::{
  ClientOptions, LspError, LspPool, ManualClock, PoolMode, PoolOptions, RestartReason, ServerKind, ServerState, ServerSummary, SyncDemand, TypeScriptVersion, UnitFolder, UnitKey,
  UnitPhase, UnitScope, resident_kib,
};
use common::fake::{Workspace, initialize, methods, rust_ready, ts_ready, wait_dead, wait_until, with};
use serde_json::{Value, json};

fn manual_pool(clock: &Arc<ManualClock>) -> LspPool {
  LspPool::with_clock(
    PoolOptions {
      mode: PoolMode::OneShot,
      ..PoolOptions::warm()
    },
    clock.clone(),
  )
}

fn running(pool: &LspPool) -> Vec<(String, ServerKind)> {
  pool
    .status()
    .into_iter()
    .filter(|status| matches!(status.phase, UnitPhase::Running(_)))
    .map(|status| (status.key.project, status.key.kind))
    .collect()
}

fn summary(pool: &LspPool, project: &str) -> ServerSummary {
  pool
    .status()
    .into_iter()
    .find_map(|status| match status.phase {
      UnitPhase::Running(summary) if status.key.project == project => Some(summary),
      _ => None,
    })
    .unwrap()
}

#[test]
fn should_wait_for_rust_analyzer_quiescence_before_ready() {
  let workspace = Workspace::new();
  let steps = with(
    initialize(),
    [
      json!({"request": {"id": "p1", "method": "window/workDoneProgress/create", "params": {"token": "rustAnalyzer/Indexing"}}}),
      json!({"recv_reply": null}),
      json!({"notify": {"method": "$/progress", "params": {"token": "rustAnalyzer/Indexing", "value": {"kind": "begin", "title": "Indexing"}}}}),
      json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": false}}}),
      json!({"sleep_ms": 200}),
      json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": true}}}),
      json!({"sleep_ms": 200}),
      json!({"notify": {"method": "$/progress", "params": {"token": "rustAnalyzer/Indexing", "value": {"kind": "end"}}}}),
      json!({"notify": {"method": "$/progress", "params": {"token": "rust-analyzer/flycheck/0", "value": {"kind": "begin", "title": "cargo check"}}}}),
    ],
  );
  let unit = workspace.rust_unit("demo", workspace.script("ra", &steps));
  let pool = LspPool::new(PoolOptions::one_shot());
  let started = Instant::now();

  let lease = pool.get(&unit, Some(RunId(1))).unwrap();

  assert!(started.elapsed() >= Duration::from_millis(400), "ready after {:?}", started.elapsed());
  assert_eq!(lease.state(), ServerState::Ready);
  assert_eq!(lease.kind(), ServerKind::RustAnalyzer);
  assert_eq!(lease.server_info().map(|info| info.name.as_str()), Some("fake"));
  assert_eq!(lease.synced_generation(), Some(RunId(1)));
  assert_eq!(lease.start_cause(), None);
}

#[test]
fn should_be_ready_when_the_typescript_server_announces_its_version() {
  let workspace = Workspace::new();
  let unit = workspace.ts_unit("demo", workspace.script("ts", &ts_ready()));
  let pool = LspPool::new(PoolOptions::one_shot());

  let lease = pool.get(&unit, None).unwrap();

  assert_eq!(lease.state(), ServerState::Ready);
  assert_eq!(
    lease.typescript_version(),
    Some(TypeScriptVersion {
      version: "5.9.3".to_owned(),
      source: Some("bundled".to_owned())
    })
  );
  assert_eq!(lease.line_breaks(), atlas_lsp::LineBreaks::Ecma);
}

#[test]
fn should_time_out_waiting_for_readiness_with_an_actionable_message() {
  let workspace = Workspace::new();
  let steps = with(
    initialize(),
    [json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": false, "message": "Loading proc-macros"}}})],
  );
  let mut unit = workspace.rust_unit("demo", workspace.script("ra", &steps));
  unit.timeouts.ready = Duration::from_millis(300);
  let pool = LspPool::new(PoolOptions::one_shot());

  let error = pool.get(&unit, None).err().unwrap();

  assert_eq!(
    error.to_string(),
    "rust-analyzer was not ready after 0.3s (indexing: Loading proc-macros); check it with `code-atlas lsp check`, and on a large workspace raise lsp.ready_timeout_secs in the project config"
  );
  assert_eq!(pool.status(), vec![]);
}

#[test]
fn should_restart_a_crashed_server_on_next_use() {
  let workspace = Workspace::new();
  let crashing = with(rust_ready(), [json!({"recv": {"method": "fake/crash"}}), json!({"stderr": "boom\n"}), json!({"exit": 3})]);
  let unit = workspace.rust_unit("demo", workspace.script("ra", &crashing));
  let pool = LspPool::new(PoolOptions::one_shot());
  let lease = pool.get(&unit, None).unwrap();
  let first = lease.pid().unwrap();
  lease.notify("fake/crash", json!({})).unwrap();
  assert!(wait_until(|| matches!(lease.state(), ServerState::Exited { .. })));
  drop(lease);
  workspace.script("ra", &rust_ready());

  let restarted = pool.get(&unit, None).unwrap();

  assert_ne!(restarted.pid(), Some(first));
  assert_eq!(restarted.start_cause(), Some(&RestartReason::Crashed));
  assert_eq!(restarted.state(), ServerState::Ready);
  assert!(wait_dead(first));
}

#[test]
fn should_give_up_after_three_crashes_in_ten_minutes() {
  let workspace = Workspace::new();
  let command = workspace.counted("ra", &[json!({"stderr": "boom\n"}), json!({"exit": 1})]);
  let unit = workspace.rust_unit("demo", command);
  let clock = Arc::new(ManualClock::default());
  let pool = manual_pool(&clock);

  let failures: Vec<LspError> = (0..3).map(|_| pool.get(&unit, None).err().unwrap()).collect();
  let refused = pool.get(&unit, None).err().unwrap();

  assert!(failures.iter().all(|error| matches!(error, LspError::ServerExited { .. })), "{failures:?}");
  assert_eq!(workspace.spawns("ra"), 3);
  assert!(
    matches!(&refused, LspError::CrashLoop { server, failures: 3, retry_in, last_failure, .. }
      if server == "rust-analyzer" && *retry_in == Duration::from_secs(600) && last_failure.contains("boom")),
    "{refused}"
  );
  assert!(
    refused
      .to_string()
      .starts_with("rust-analyzer failed 3 times within 600s; not restarting it for another 600s; last failure: language server exited")
  );
  clock.advance(Duration::from_secs(300));
  assert!(matches!(pool.get(&unit, None), Err(LspError::CrashLoop { retry_in, .. }) if retry_in == Duration::from_secs(300)));
  assert_eq!(workspace.spawns("ra"), 3);

  clock.advance(Duration::from_secs(301));
  workspace.counted("ra", &rust_ready());
  let lease = pool.get(&unit, None).unwrap();

  assert_eq!(lease.state(), ServerState::Ready);
  assert_eq!(workspace.spawns("ra"), 4);
}

#[test]
fn should_forget_crashes_older_than_the_window() {
  let workspace = Workspace::new();
  let unit = workspace.rust_unit("demo", workspace.counted("ra", &[json!({"exit": 1})]));
  let clock = Arc::new(ManualClock::default());
  let pool = manual_pool(&clock);

  for _ in 0..2 {
    assert!(pool.get(&unit, None).is_err());
  }
  clock.advance(Duration::from_secs(601));
  let third = pool.get(&unit, None).err().unwrap();
  let fourth = pool.get(&unit, None).err().unwrap();

  assert!(matches!(third, LspError::ServerExited { .. }), "{third}");
  assert!(matches!(fourth, LspError::ServerExited { .. }), "{fourth}");
  assert_eq!(workspace.spawns("ra"), 4);
}

#[test]
fn should_start_a_unit_once_for_concurrent_callers() {
  let workspace = Workspace::new();
  let slow = vec![
    json!({"recv": {"method": "initialize"}}),
    json!({"sleep_ms": 300}),
    json!({"reply": {"method": "initialize", "result": {"capabilities": {}}}}),
    json!({"recv": {"method": "initialized"}}),
    json!({"notify": {"method": "$/typescriptVersion", "params": {"version": "5.9.3"}}}),
  ];
  let unit = workspace.ts_unit("demo", workspace.counted("ts", &slow));
  let pool = Arc::new(LspPool::new(PoolOptions::one_shot()));
  let barrier = Arc::new(Barrier::new(4));

  let pids: Vec<Option<u32>> = (0..4)
    .map(|_| {
      let (pool, barrier, unit) = (Arc::clone(&pool), Arc::clone(&barrier), unit.clone());
      thread::spawn(move || {
        barrier.wait();
        pool.get(&unit, None).unwrap().pid()
      })
    })
    .collect::<Vec<_>>()
    .into_iter()
    .map(|caller| caller.join().unwrap())
    .collect();

  assert_eq!(workspace.spawns("ts"), 1);
  assert!(pids[0].is_some());
  assert!(pids.iter().all(|pid| *pid == pids[0]), "{pids:?}");
}

#[test]
fn should_share_a_start_failure_with_concurrent_callers() {
  let workspace = Workspace::new();
  let failing = vec![json!({"recv": {"method": "initialize"}}), json!({"sleep_ms": 200}), json!({"exit": 4})];
  let unit = workspace.ts_unit("demo", workspace.counted("ts", &failing));
  let pool = Arc::new(LspPool::new(PoolOptions::one_shot()));
  let barrier = Arc::new(Barrier::new(3));

  let errors: Vec<String> = (0..3)
    .map(|_| {
      let (pool, barrier, unit) = (Arc::clone(&pool), Arc::clone(&barrier), unit.clone());
      thread::spawn(move || {
        barrier.wait();
        pool.get(&unit, None).err().unwrap().to_string()
      })
    })
    .collect::<Vec<_>>()
    .into_iter()
    .map(|caller| caller.join().unwrap())
    .collect();

  assert_eq!(workspace.spawns("ts"), 1);
  assert!(errors.iter().all(|error| error == "language server exited (the process exited with code 4)"), "{errors:?}");
}

#[test]
fn should_evict_the_least_recently_used_server_over_the_limit() {
  let workspace = Workspace::new();
  let command = workspace.script("ts", &ts_ready());
  let units: Vec<_> = ["a", "b", "c"].iter().map(|project| workspace.ts_unit(project, command.clone())).collect();
  let clock = Arc::new(ManualClock::default());
  let pool = manual_pool(&clock);
  let mut pids = Vec::new();
  for unit in &units[..2] {
    pids.push(pool.get(unit, None).unwrap().pid().unwrap());
    clock.advance(Duration::from_secs(1));
  }
  drop(pool.get(&units[0], None).unwrap());
  clock.advance(Duration::from_secs(1));

  let third = pool.get(&units[2], None).unwrap();

  assert_eq!(running(&pool), vec![("a".to_owned(), ServerKind::TypeScript), ("c".to_owned(), ServerKind::TypeScript)]);
  assert!(wait_dead(pids[1]), "the least recently used server b survived");
  assert_eq!(pool.get(&units[0], None).unwrap().pid(), Some(pids[0]));
  assert!(third.pid().is_some());
}

#[test]
fn should_shut_down_idle_servers() {
  let workspace = Workspace::new();
  let command = workspace.script("ts", &ts_ready());
  let clock = Arc::new(ManualClock::default());
  let pool = manual_pool(&clock);
  let idle = pool.get(&workspace.ts_unit("idle", command.clone()), None).unwrap().pid().unwrap();
  let busy = pool.get(&workspace.ts_unit("busy", command), None).unwrap();

  clock.advance(Duration::from_secs(540));
  assert_eq!(pool.reap_idle(), 0);
  assert_eq!(summary(&pool, "idle").idle, Duration::from_secs(540));
  assert!(summary(&pool, "busy").in_use);
  clock.advance(Duration::from_secs(61));
  let reaped = pool.reap_idle();

  assert_eq!(reaped, 1);
  assert_eq!(running(&pool), vec![("busy".to_owned(), ServerKind::TypeScript)]);
  assert!(wait_dead(idle));
  assert_eq!(busy.state(), ServerState::Ready);
}

#[test]
fn should_restart_a_unit_whose_configuration_changed() {
  let workspace = Workspace::new();
  let unit = workspace.ts_unit("demo", workspace.script("ts", &ts_ready()));
  let pool = LspPool::new(PoolOptions::one_shot());
  let first = pool.get(&unit, None).unwrap().pid().unwrap();
  let mut changed = unit.clone();
  changed.folders.push(atlas_lsp::UnitFolder::new("other", workspace.path("other")));

  let lease = pool.get(&changed, None).unwrap();

  assert_ne!(lease.pid(), Some(first));
  assert_eq!(lease.start_cause(), Some(&RestartReason::ConfigurationChanged));
  assert!(wait_dead(first));
}

#[test]
fn should_keep_a_server_whose_request_timed_out_after_a_notification() {
  let workspace = Workspace::new();
  let steps = with(
    rust_ready(),
    [
      json!({"recv": {"method": "fake/note"}}),
      json!({"recv": {"method": "fake/slow"}}),
      json!({"recv": {"method": "$/cancelRequest"}}),
    ],
  );
  let mut unit = workspace.rust_unit("demo", workspace.script("ra", &steps));
  unit.timeouts.first_request = Duration::ZERO;
  let pool = LspPool::new(PoolOptions::one_shot());
  let lease = pool.get(&unit, None).unwrap();
  let first = lease.pid();
  lease.notify("fake/note", json!({})).unwrap();

  let timed_out = lease.request_with_timeout::<Value>("fake/slow", json!({}), Duration::from_millis(100)).unwrap_err();
  drop(lease);
  let again = pool.get(&unit, None).unwrap();

  assert!(matches!(timed_out, LspError::Timeout { .. }));
  assert_eq!(again.sync_lost(), None);
  assert_eq!(again.pid(), first);
  assert_eq!(again.start_cause(), None);
}

#[test]
fn should_restart_a_server_whose_notification_could_not_be_queued() {
  let workspace = Workspace::new();
  let mut unit = workspace.rust_unit("demo", workspace.script("ra", &with(rust_ready(), [json!("hang")])));
  unit.timeouts.request = Duration::from_millis(200);
  let pool = LspPool::new(PoolOptions {
    client: ClientOptions {
      shutdown_timeout: Duration::from_millis(200),
      exit_grace: Duration::from_millis(200),
      ..ClientOptions::default()
    },
    ..PoolOptions::one_shot()
  });
  let lease = pool.get(&unit, None).unwrap();
  let first = lease.pid();
  let payload = "x".repeat(256 * 1024);

  let refused = (0..200).find_map(|_| lease.notify("fake/big", json!({"text": payload})).err()).unwrap();
  let lost = lease.sync_lost();
  drop(lease);
  workspace.script("ra", &rust_ready());
  let restarted = pool.get(&unit, None).unwrap();

  assert!(matches!(refused, LspError::Timeout { .. }), "{refused}");
  assert_eq!(lost.as_deref(), Some("fake/big could not be queued: fake/big timed out after 0.2s"));
  assert_ne!(restarted.pid(), first);
  assert_eq!(
    restarted.start_cause(),
    Some(&RestartReason::SyncLost("fake/big could not be queued: fake/big timed out after 0.2s".to_owned()))
  );
}

#[test]
fn should_report_a_busy_pool_when_every_slot_stays_in_use() {
  let workspace = Workspace::new();
  let command = workspace.script("ts", &ts_ready());
  let (first, second) = (workspace.ts_unit("a", command.clone()), workspace.ts_unit("b", command));
  let pool = LspPool::new(PoolOptions {
    max_servers: 1,
    busy_timeout: Duration::from_millis(300),
    ..PoolOptions::one_shot()
  });
  let held = pool.get(&first, None).unwrap();
  let started = Instant::now();

  let busy = pool.get(&second, None).err().unwrap();

  assert!(started.elapsed() >= Duration::from_millis(300));
  assert_eq!(
    busy.to_string(),
    "all 1 language server slots stayed in use for 0.3s; retry when a running operation finishes"
  );
  assert_eq!(running(&pool), vec![("a".to_owned(), ServerKind::TypeScript)]);
  assert_eq!(held.state(), ServerState::Ready);
}

#[test]
fn should_start_a_waiting_unit_once_a_lease_is_dropped() {
  let workspace = Workspace::new();
  let command = workspace.script("ts", &ts_ready());
  let (first, second) = (workspace.ts_unit("a", command.clone()), workspace.ts_unit("b", command));
  let pool = Arc::new(LspPool::new(PoolOptions {
    max_servers: 1,
    busy_timeout: Duration::from_secs(10),
    ..PoolOptions::one_shot()
  }));
  let held = pool.get(&first, None).unwrap();
  let held_pid = held.pid().unwrap();
  let waiter = {
    let pool = Arc::clone(&pool);
    thread::spawn(move || {
      let started = Instant::now();
      let pid = pool.get(&second, None).unwrap().pid();
      (pid, started.elapsed())
    })
  };
  thread::sleep(Duration::from_millis(300));
  assert_eq!(running(&pool), vec![("a".to_owned(), ServerKind::TypeScript)]);

  drop(held);
  let (pid, waited) = waiter.join().unwrap();

  assert!(pid.is_some());
  assert!(waited >= Duration::from_millis(300), "{waited:?}");
  assert!(wait_dead(held_pid));
  assert_eq!(running(&pool), vec![("b".to_owned(), ServerKind::TypeScript)]);
}

#[test]
fn should_keep_repo_scoped_units_of_one_project_apart() {
  let workspace = Workspace::new();
  let command = workspace.script("ts", &ts_ready());
  let scoped = |repo: &str| {
    let mut unit = workspace.ts_unit("shop", command.clone());
    unit.scope = UnitScope::Repo(repo.to_owned());
    unit.folders = vec![UnitFolder::new(repo, workspace.path(repo))];
    unit
  };
  let (api, web) = (scoped("api"), scoped("web"));
  let pool = LspPool::new(PoolOptions::one_shot());

  let api_pid = pool.get(&api, None).unwrap().pid();
  let web_pid = pool.get(&web, None).unwrap().pid();
  let api_again = pool.get(&api, None).unwrap();
  let web_again = pool.get(&web, None).unwrap();

  assert_ne!(api_pid, web_pid);
  assert_eq!((api_again.pid(), web_again.pid()), (api_pid, web_pid));
  assert_eq!((api_again.start_cause(), web_again.start_cause()), (None, None));
  assert_eq!(
    pool.status().into_iter().map(|status| status.key.scope).collect::<Vec<_>>(),
    vec![UnitScope::Repo("api".to_owned()), UnitScope::Repo("web".to_owned())]
  );
}

#[test]
fn should_count_the_whole_process_group_in_resident_memory() {
  let workspace = Workspace::new();
  let pid_file = workspace.path("sleeper.pid");
  let steps = with(rust_ready(), [json!({"sleeper": {"pid_file": pid_file}})]);
  let unit = workspace.rust_unit("demo", workspace.script("ra", &steps));
  let pool = LspPool::new(PoolOptions::one_shot());
  let lease = pool.get(&unit, None).unwrap();
  let pid = lease.pid().unwrap();
  assert!(wait_until(|| std::fs::read_to_string(&pid_file).is_ok_and(|text| !text.is_empty())));
  let sleeper: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
  let alone = |pid: u32| -> u64 {
    let output = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&output.stdout).trim().parse().unwrap()
  };

  let group = resident_kib(pid).unwrap();

  assert!(group >= alone(pid) + alone(sleeper), "group {group} KiB");
}

#[test]
fn should_keep_waiting_for_a_slow_server_in_warm_mode_instead_of_respawning_it() {
  let workspace = Workspace::new();
  let steps = with(
    initialize(),
    [
      json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": false, "message": "Loading"}}}),
      json!({"sleep_ms": 700}),
      json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": true}}}),
    ],
  );
  let mut unit = workspace.rust_unit("demo", workspace.counted("ra", &steps));
  unit.timeouts.ready = Duration::from_millis(300);
  let pool = LspPool::new(PoolOptions {
    mode: PoolMode::Warm {
      reap_every: Duration::from_secs(3600),
    },
    ..PoolOptions::warm()
  });

  let first = pool.get(&unit, None).err().unwrap();
  let warming = pool.status();
  let lease = wait_for_lease(&pool, &unit);

  assert!(matches!(first, LspError::NotReady { .. }), "{first}");
  assert!(
    matches!(&warming[0].phase, UnitPhase::Warming(summary) if summary.state == ServerState::Indexing { message: Some("Loading".to_owned()) }),
    "{warming:?}"
  );
  assert_eq!(lease.state(), ServerState::Ready);
  assert_eq!(workspace.spawns("ra"), 1);
  assert!(matches!(&pool.status()[0].phase, UnitPhase::Running(_)));
}

fn wait_for_lease(pool: &LspPool, unit: &atlas_lsp::ServerUnit) -> atlas_lsp::Lease {
  for _ in 0..20 {
    match pool.get(unit, None) {
      Ok(lease) => return lease,
      Err(LspError::NotReady { .. }) => continue,
      Err(error) => panic!("{error}"),
    }
  }
  panic!("the server never became ready");
}

#[test]
fn should_give_up_on_a_slow_server_in_one_shot_mode() {
  let workspace = Workspace::new();
  let steps = with(
    initialize(),
    [json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": false}}})],
  );
  let mut unit = workspace.rust_unit("demo", workspace.counted("ra", &steps));
  unit.timeouts.ready = Duration::from_millis(200);
  let pool = LspPool::new(PoolOptions::one_shot());

  assert!(matches!(pool.get(&unit, None), Err(LspError::NotReady { .. })));
  assert!(matches!(pool.get(&unit, None), Err(LspError::NotReady { .. })));

  assert_eq!(workspace.spawns("ra"), 2);
  assert_eq!(pool.status(), vec![]);
}

#[test]
fn should_cancel_a_start_in_flight_on_shutdown() {
  let workspace = Workspace::new();
  let script = workspace.script("ra", &[json!({"recv": {"method": "initialize"}}), json!("hang")]).args.remove(0);
  let pid_file = workspace.path("leader.pid");
  let line = format!(
    "echo $$ > '{}'; exec '{}' '{}'",
    pid_file.display(),
    atlas_test_lsp::fake_lsp_path().display(),
    std::path::Path::new(&script).display()
  );
  let unit = workspace.rust_unit("demo", atlas_lsp::ServerCommand::new("sh").arg("-c").arg(line));
  let pool = Arc::new(LspPool::new(PoolOptions {
    client: ClientOptions {
      shutdown_timeout: Duration::from_millis(200),
      exit_grace: Duration::from_millis(200),
      ..ClientOptions::default()
    },
    ..PoolOptions::one_shot()
  }));
  let starter = {
    let (pool, unit) = (Arc::clone(&pool), unit.clone());
    thread::spawn(move || pool.get(&unit, None).err())
  };
  assert!(wait_until(|| std::fs::read_to_string(&pid_file).is_ok_and(|text| !text.trim().is_empty())));
  let leader: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
  thread::sleep(Duration::from_millis(100));
  let started = Instant::now();

  pool.shutdown();
  let error = starter.join().unwrap();

  assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
  assert!(matches!(error, Some(LspError::PoolShutDown)), "{error:?}");
  assert!(wait_dead(leader));
}

#[test]
fn should_give_every_concurrent_caller_the_restart_cause() {
  let workspace = Workspace::new();
  let crashing = with(rust_ready(), [json!({"recv": {"method": "fake/crash"}}), json!({"exit": 3})]);
  let unit = workspace.rust_unit("demo", workspace.script("ra", &crashing));
  let pool = Arc::new(LspPool::new(PoolOptions::one_shot()));
  let lease = pool.get(&unit, None).unwrap();
  lease.notify("fake/crash", json!({})).unwrap();
  assert!(wait_until(|| matches!(lease.state(), ServerState::Exited { .. })));
  drop(lease);
  workspace.script("ra", &rust_ready());
  let barrier = Arc::new(Barrier::new(3));

  let causes: Vec<Option<RestartReason>> = (0..3)
    .map(|_| {
      let (pool, barrier, unit) = (Arc::clone(&pool), Arc::clone(&barrier), unit.clone());
      thread::spawn(move || {
        barrier.wait();
        pool.get(&unit, None).unwrap().start_cause().cloned()
      })
    })
    .collect::<Vec<_>>()
    .into_iter()
    .map(|caller| caller.join().unwrap())
    .collect();

  assert_eq!(causes, vec![Some(RestartReason::Crashed); 3]);
}

#[test]
fn should_refuse_new_servers_after_shutdown_and_stop_running_ones() {
  let workspace = Workspace::new();
  let unit = workspace.ts_unit("demo", workspace.script("ts", &ts_ready()));
  let pool = LspPool::new(PoolOptions::warm());
  let pid = pool.get(&unit, None).unwrap().pid().unwrap();

  pool.shutdown();

  assert!(wait_dead(pid));
  assert!(matches!(pool.get(&unit, None), Err(LspError::PoolShutDown)));
  assert_eq!(pool.status(), vec![]);
}

#[test]
fn should_report_status_with_versions_and_resident_memory() {
  let workspace = Workspace::new();
  let file = workspace.write("src/a.ts", "export const a = 1;\n");
  let steps = with(ts_ready(), [workspace.record("ts")]);
  let unit = workspace.ts_unit("demo", workspace.script("ts", &steps));
  let pool = LspPool::new(PoolOptions::one_shot());
  let synced = pool.sync(&unit, None, SyncDemand::Best, |_| unreachable!(), std::slice::from_ref(&file)).unwrap();

  let status = pool.status();

  assert_eq!(status.len(), 1);
  assert_eq!(
    status[0].key,
    UnitKey {
      project: "demo".to_owned(),
      kind: ServerKind::TypeScript,
      scope: UnitScope::Project
    }
  );
  let UnitPhase::Running(summary) = &status[0].phase else {
    panic!("{status:?}");
  };
  assert_eq!(summary.state, ServerState::Ready);
  assert_eq!(summary.open_documents, 1);
  assert!(summary.in_use);
  assert_eq!(summary.server.as_ref().and_then(|info| info.version.as_deref()), Some("0.1"));
  assert_eq!(summary.typescript.as_ref().map(|version| version.version.as_str()), Some("5.9.3"));
  assert!(resident_kib(synced.lease.pid().unwrap()).is_some_and(|kib| kib > 0));
  synced.lease.request::<Value>("fake/flush", json!({})).unwrap_err();
  assert_eq!(methods(&workspace.recorded("ts")), vec!["textDocument/didOpen", "fake/flush"]);
}

#[test]
fn should_report_a_cargo_lock_rewritten_since_the_server_started() {
  let workspace = Workspace::new();
  let lockfile = workspace.write("Cargo.lock", "version = 3\n");
  workspace.write("other/Cargo.lock", "version = 3\n");
  let unit = workspace.rust_unit("demo", workspace.script("ra", &rust_ready()));
  let pool = LspPool::new(PoolOptions::one_shot());
  let lease = pool.get(&unit, None).unwrap();
  assert_eq!(lease.lockfile_changes(), Vec::<std::path::PathBuf>::new());

  std::fs::write(&lockfile, "version = 4\n").unwrap();

  assert_eq!(lease.lockfile_changes(), vec![lockfile]);
}

#[test]
fn should_reap_idle_servers_from_the_warm_pool_thread() {
  let workspace = Workspace::new();
  let unit = workspace.ts_unit("demo", workspace.script("ts", &ts_ready()));
  let pool = LspPool::new(PoolOptions {
    mode: PoolMode::Warm {
      reap_every: Duration::from_millis(50),
    },
    idle_timeout: Duration::from_millis(100),
    ..PoolOptions::warm()
  });
  let pid = pool.get(&unit, None).unwrap().pid().unwrap();

  assert!(wait_until(|| pool.status().is_empty()));
  assert!(wait_dead(pid));
}

fn never_ready(workspace: &Workspace, project: &str) -> atlas_lsp::ServerUnit {
  let steps = with(
    initialize(),
    [json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": false, "message": "Loading"}}})],
  );
  let mut unit = workspace.rust_unit(project, workspace.counted(project, &steps));
  unit.timeouts.ready = Duration::from_millis(200);
  unit
}

fn warm_pool(clock: &Arc<ManualClock>, restart: atlas_lsp::RestartPolicy, max_servers: usize) -> LspPool {
  LspPool::with_clock(
    PoolOptions {
      mode: PoolMode::Warm {
        reap_every: Duration::from_secs(3600),
      },
      max_servers,
      restart,
      ..PoolOptions::warm()
    },
    clock.clone(),
  )
}

fn warming_pid(pool: &LspPool) -> u32 {
  pool
    .status()
    .into_iter()
    .find_map(|status| match status.phase {
      UnitPhase::Warming(summary) => summary.pid,
      _ => None,
    })
    .unwrap()
}

#[test]
fn should_evict_an_idle_warming_server_for_another_unit() {
  let workspace = Workspace::new();
  let slow = never_ready(&workspace, "slow");
  let other = workspace.ts_unit("other", workspace.script("ts", &ts_ready()));
  let clock = Arc::new(ManualClock::default());
  let pool = warm_pool(&clock, atlas_lsp::RestartPolicy::default(), 1);
  assert!(matches!(pool.get(&slow, None), Err(LspError::NotReady { .. })));
  let slow_pid = warming_pid(&pool);

  let lease = pool.get(&other, None).unwrap();

  assert_eq!(lease.state(), ServerState::Ready);
  assert_eq!(running(&pool), vec![("other".to_owned(), ServerKind::TypeScript)]);
  assert!(wait_dead(slow_pid));
}

#[test]
fn should_recycle_a_server_that_stays_warming_and_count_it_as_a_failure() {
  let workspace = Workspace::new();
  let slow = never_ready(&workspace, "slow");
  let clock = Arc::new(ManualClock::default());
  let pool = warm_pool(&clock, atlas_lsp::RestartPolicy::default(), 2);
  assert!(matches!(pool.get(&slow, None), Err(LspError::NotReady { .. })));
  let first = warming_pid(&pool);
  clock.advance(Duration::from_millis(500));
  assert!(matches!(pool.get(&slow, None), Err(LspError::NotReady { .. })));
  assert_eq!((workspace.spawns("slow"), warming_pid(&pool)), (1, first));

  clock.advance(Duration::from_millis(200));
  let recycled = pool.get(&slow, None).err().unwrap();

  assert!(matches!(recycled, LspError::NotReady { .. }), "{recycled}");
  assert_eq!(workspace.spawns("slow"), 2);
  assert_ne!(warming_pid(&pool), first);
  assert!(wait_dead(first));
}

#[test]
fn should_back_off_a_unit_that_never_becomes_ready() {
  let workspace = Workspace::new();
  let slow = never_ready(&workspace, "slow");
  let clock = Arc::new(ManualClock::default());
  let policy = atlas_lsp::RestartPolicy {
    max_failures: 1,
    ..atlas_lsp::RestartPolicy::default()
  };
  let pool = warm_pool(&clock, policy, 2);
  assert!(matches!(pool.get(&slow, None), Err(LspError::NotReady { .. })));
  clock.advance(Duration::from_millis(700));

  let refused = pool.get(&slow, None).err().unwrap();

  assert!(
    matches!(&refused, LspError::CrashLoop { failures: 1, last_failure, .. } if last_failure == "the server was still not ready after 0.7s (indexing: Loading)"),
    "{refused}"
  );
  assert_eq!(workspace.spawns("slow"), 1);
  assert!(matches!(pool.get(&slow, None), Err(LspError::CrashLoop { .. })));
}
