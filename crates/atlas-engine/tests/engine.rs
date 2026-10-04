mod common;

use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use atlas_core::{Certainty, Confidence, ContentHash, EdgeKind, Entity, EntityKind, Evidence, Freshness, Owner, Producer, RelPath, RepoName, Span};
use atlas_engine::{Engine, EngineOptions, FaultInjection, FreshnessReport, IndexMode, ScanKind, ScipLag};
use atlas_store::{EdgeFilter, EntityFilter, FreshnessPolicy, InferredQuery, NewInferredFact, RunStatus};
use common::edits::replace_word;
use common::{acme_copy, copy_dir, digest, fixtures_root, fresh, import_lineage, open, open_with, project, repo_config};

fn repo(name: &str) -> RepoName {
  RepoName::new(name).unwrap()
}

fn rel(path: &str) -> RelPath {
  RelPath::new(path).unwrap()
}

fn named(engine: &Engine, name: &str, repo_name: &str) -> Vec<Entity> {
  let found = engine.store().read(|tx| tx.entities_by_name(name, &EntityFilter::default())).unwrap();
  found
    .into_iter()
    .filter(|entity| entity.repo.as_ref().is_some_and(|repo| repo.as_str() == repo_name))
    .collect()
}

fn single(engine: &Engine, name: &str, repo_name: &str, kind: EntityKind) -> Entity {
  let mut found: Vec<Entity> = named(engine, name, repo_name).into_iter().filter(|entity| entity.kind == kind).collect();
  assert_eq!(found.len(), 1, "expected one {kind} named {name} in {repo_name}: {found:#?}");
  found.remove(0)
}

fn inferred(subject: atlas_core::EntityId, repo_name: &str, path: &str, hash: ContentHash) -> NewInferredFact {
  NewInferredFact {
    subjects: vec![subject],
    relation: "equivalent_to".to_owned(),
    payload: serde_json::json!({ "note": "test" }),
    certainty: Certainty::Inferred {
      confidence: Confidence::new(0.8).unwrap(),
      provider: "fake".to_owned(),
      model: "fake-model".to_owned(),
      rationale: "inserted by a test".to_owned(),
    },
    evidence: vec![Evidence {
      repo: repo(repo_name),
      path: rel(path),
      span: Span::from_zero_based_rows(0, 1, 0, 0, 0, 1),
      content_hash: hash,
      producer: Producer::new("test", 1),
      note: None,
    }],
    created_run: None,
    llm_call: None,
  }
}

fn file_hash(engine: &Engine, repo_name: &str, path: &str) -> ContentHash {
  engine.store().read(|tx| tx.file(&repo(repo_name), &rel(path))).unwrap().unwrap().content_hash
}

fn append(path: &Path, text: &str) {
  let mut content = fs::read_to_string(path).unwrap();
  content.push_str(text);
  fs::write(path, content).unwrap();
}

fn run_count(engine: &Engine) -> usize {
  engine.store().read(|tx| tx.runs(10_000)).unwrap().len()
}

#[test]
fn should_record_lineage_and_migrate_inferred_facts_when_a_function_is_renamed() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let old = single(&engine, "formatMoney", "acme-api", EntityKind::Function);
  let hash = file_hash(&engine, "acme-api", "src/common/money.ts");
  let fact = engine
    .store()
    .write(|tx| tx.insert_inferred_fact(&inferred(old.id, "acme-api", "src/common/money.ts", hash)))
    .unwrap();
  for path in ["src/common/money.ts", "src/orders/orders.service.ts", "src/reports/reports.service.ts"] {
    let file = tree.join("acme-api").join(path);
    fs::write(&file, replace_word(&fs::read_to_string(&file).unwrap(), "formatMoney", "formatCents")).unwrap();
  }

  let report = fresh(&engine);

  assert!(report.is_fresh(), "{report:?}");
  let new = single(&engine, "formatCents", "acme-api", EntityKind::Function);
  assert!(named(&engine, "formatMoney", "acme-api").is_empty());
  let lineage = engine
    .store()
    .read(|tx| {
      tx.edges_from(
        new.id,
        &EdgeFilter {
          kinds: vec![EdgeKind::RenamedFrom],
          ..EdgeFilter::default()
        },
      )
    })
    .unwrap();
  assert_eq!(lineage.len(), 1, "{lineage:#?}");
  assert_eq!(lineage[0].to, old.id);
  assert_eq!(lineage[0].certainty, Certainty::heuristic(Confidence::new(1.0).unwrap(), "identical-body"));
  assert_eq!(lineage[0].attrs["old_key"], serde_json::json!(old.key.to_string()));
  assert_eq!(lineage[0].attrs["run"], serde_json::json!(report.header().run.unwrap().0));
  let calls = |id| {
    engine
      .store()
      .read(|tx| {
        tx.edges_to(
          id,
          &EdgeFilter {
            kinds: vec![EdgeKind::Calls],
            ..EdgeFilter::default()
          },
        )
      })
      .unwrap()
  };
  let callers: Vec<String> = calls(new.id)
    .iter()
    .flat_map(|edge| edge.evidence.iter().map(|evidence| evidence.path.to_string()))
    .collect();
  assert!(callers.contains(&"src/orders/orders.service.ts".to_owned()), "{callers:?}");
  assert!(callers.contains(&"src/reports/reports.service.ts".to_owned()), "{callers:?}");
  assert!(calls(old.id).is_empty());
  let migrated = engine
    .store()
    .read(|tx| {
      tx.inferred_facts(&InferredQuery {
        subject: Some(new.id),
        relation: None,
        freshness: FreshnessPolicy::IncludeStale,
      })
    })
    .unwrap();
  assert_eq!(migrated.len(), 1);
  assert_eq!(migrated[0].relation, "equivalent_to");
  assert!(
    matches!(&migrated[0].freshness, Freshness::NeedsRevalidation { reason } if reason.contains(&old.key.to_string())),
    "{:?}",
    migrated[0].freshness
  );
  assert!(engine.store().read(|tx| tx.inferred_fact(fact)).unwrap().is_none());
  let current_only = engine
    .store()
    .read(|tx| {
      tx.inferred_facts(&InferredQuery {
        subject: Some(new.id),
        ..InferredQuery::default()
      })
    })
    .unwrap();
  assert!(current_only.is_empty());
}

#[test]
fn should_keep_projects_isolated_in_separate_databases() {
  let scratch = tempfile::tempdir().unwrap();
  let home = scratch.path().join("home");
  let acme = acme_copy(&scratch.path().join("acme"));
  let globex_repos = ["globex-api", "globex-worker"]
    .iter()
    .map(|name| {
      let target = scratch.path().join("globex").join(name);
      copy_dir(&fixtures_root().join("globex").join(name), &target);
      repo_config(name, &target)
    })
    .collect();
  let globex = project("globex", globex_repos);
  let acme_engine = open(&acme, &home);
  let globex_engine = open(&globex, &home);
  fresh(&acme_engine);
  fresh(&globex_engine);

  assert_ne!(acme_engine.db_path(), globex_engine.db_path());
  for (engine, prefix, other) in [(&acme_engine, "acme-", "globex-"), (&globex_engine, "globex-", "acme-")] {
    for kind in EntityKind::BUILTIN {
      let entities = engine.store().read(|tx| tx.entities_by_kind(kind, &EntityFilter::default())).unwrap();
      for entity in entities {
        let owner_repo = entity
          .repo
          .as_ref()
          .map(|repo| repo.to_string())
          .or_else(|| entity.attrs.get("repo").and_then(|repo| repo.as_str()).map(str::to_owned));
        if let Some(owner_repo) = owner_repo {
          assert!(
            owner_repo.starts_with(prefix) && !owner_repo.starts_with(other),
            "{} leaked into {prefix} store",
            entity.key
          );
        }
      }
    }
    for name in ["formatMoney", "UsersService"] {
      let found = engine.store().read(|tx| tx.entities_by_name(name, &EntityFilter::default())).unwrap();
      assert!(!found.is_empty(), "{name} missing from the {prefix} store");
      assert!(
        found.iter().all(|entity| entity.repo.as_ref().is_some_and(|repo| repo.as_str().starts_with(prefix))),
        "{found:#?}"
      );
    }
    let endpoints = engine.store().read(|tx| tx.entities_by_kind(&EntityKind::ApiEndpoint, &EntityFilter::default())).unwrap();
    let users_by_id: Vec<&Entity> = endpoints.iter().filter(|entity| entity.name == "GET /users/{id}").collect();
    assert!(
      !users_by_id.is_empty(),
      "GET /users/{{id}} missing from the {prefix} store: {:?}",
      endpoints.iter().map(|entity| &entity.name).collect::<Vec<_>>()
    );
    assert!(users_by_id.iter().all(|entity| entity.repo.as_ref().is_some_and(|repo| repo.as_str().starts_with(prefix))));
  }
}

#[test]
fn should_index_other_files_and_report_the_failure_when_an_analyzer_panics() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let options = EngineOptions {
    fault_injection: FaultInjection::panic_when(|repo, path| repo.as_str() == "acme-api" && path.as_str() == "src/users/users.service.ts"),
    ..EngineOptions::default()
  };
  let engine = open_with(&project, &scratch.path().join("home"), options);

  let report = fresh(&engine);

  let FreshnessReport::Degraded { failed_files, header, .. } = &report else {
    panic!("expected a degraded report: {report:?}")
  };
  assert_eq!(failed_files.len(), 1, "{failed_files:?}");
  assert_eq!(failed_files[0].repo, repo("acme-api"));
  assert_eq!(failed_files[0].path, Some(rel("src/users/users.service.ts")));
  assert!(failed_files[0].reason.contains("injected analyzer fault"), "{}", failed_files[0].reason);
  assert_eq!(header.files_failed_analysis, 1);
  let run = engine.store().read(|tx| tx.runs(1)).unwrap().remove(0);
  assert!(matches!(run.status, RunStatus::Partial { .. }), "{:?}", run.status);
  let controller = engine
    .store()
    .read(|tx| tx.entities_in_file(&repo("acme-api"), &rel("src/users/users.controller.ts"), &EntityFilter::default()))
    .unwrap();
  assert!(controller.iter().any(|entity| entity.name == "UsersController"));
  let failed_file = engine
    .store()
    .read(|tx| {
      tx.entity_by_key(&atlas_core::EntityKey::File {
        repo: repo("acme-api"),
        path: rel("src/users/users.service.ts"),
      })
    })
    .unwrap()
    .unwrap();
  assert_eq!(failed_file.attrs["parse_status"], serde_json::json!("failed"));
  let status = engine.status().unwrap();
  assert_eq!(status.files_by_parse_status.get("failed"), Some(&1));
  assert_eq!(status.failed_files.len(), 1);
  assert_eq!(status.failed_files[0].path, Some(rel("src/users/users.service.ts")));
  let quiet = fresh(&engine);
  assert_eq!(quiet.header().run, None);
  assert_eq!(quiet.header().files_failed_analysis, 1);
}

#[test]
fn should_coalesce_concurrent_barriers_into_one_run() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let engine = Arc::new(open(&project, &scratch.path().join("home")));
  fresh(&engine);
  let before = run_count(&engine);
  append(&tree.join("acme-sdk/src/money.ts"), "\nexport function coalesced() {\n  return 1;\n}\n");
  let barrier = Arc::new(Barrier::new(2));
  let workers: Vec<_> = (0..2)
    .map(|_| {
      let engine = Arc::clone(&engine);
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        engine.ensure_fresh().unwrap()
      })
    })
    .collect();
  let reports: Vec<FreshnessReport> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();

  assert_eq!(run_count(&engine), before + 1);
  assert_eq!(reports.iter().filter(|report| report.header().run.is_some()).count(), 1, "{reports:?}");
  assert!(reports.iter().all(FreshnessReport::is_fresh));
  assert_eq!(single(&engine, "coalesced", "acme-sdk", EntityKind::Function).name, "coalesced");
}

#[test]
fn should_wait_for_another_process_holding_the_index_lock_and_reuse_its_work() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let home = scratch.path().join("home");
  let first = open(&project, &home);
  fresh(&first);
  let second = Arc::new(open(&project, &home));
  assert_eq!(fresh(&second).header().run, None);
  append(&tree.join("acme-sdk/src/money.ts"), "\nexport function crossProcess() {\n  return 2;\n}\n");
  let mut lock_path = first.db_path().into_os_string();
  lock_path.push(".index.lock");
  let lock_file = fs::OpenOptions::new().read(true).write(true).open(&lock_path).unwrap();
  let mut lock = fd_lock::RwLock::new(lock_file);
  let guard = lock.write().unwrap();
  let (sender, receiver) = mpsc::channel();
  let waiting = {
    let second = Arc::clone(&second);
    thread::spawn(move || sender.send(second.ensure_fresh().unwrap()).unwrap())
  };
  thread::sleep(Duration::from_millis(400));
  assert!(receiver.try_recv().is_err(), "the second engine must wait for the index lock");
  drop(guard);
  let report = receiver.recv_timeout(Duration::from_secs(60)).unwrap();
  waiting.join().unwrap();

  assert!(report.header().run.is_some(), "{report:?}");
  let after = fresh(&first);
  assert_eq!(after.header().run, None, "the first engine must reuse the second engine's run: {after:?}");
  assert_eq!(after.header().last_run, report.header().run);
  assert_eq!(single(&first, "crossProcess", "acme-sdk", EntityKind::Function).name, "crossProcess");
  assert_eq!(first.similarity().len(), second.similarity().len());
}

fn wait_for(deadline: Duration, mut condition: impl FnMut() -> bool) -> bool {
  let started = Instant::now();
  while started.elapsed() < deadline {
    if condition() {
      return true;
    }
    thread::sleep(Duration::from_millis(50));
  }
  condition()
}

#[test]
fn should_refresh_after_the_watch_debounce_and_rescan_after_a_watcher_error() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let options = EngineOptions {
    watch_debounce: Duration::from_millis(100),
    ..EngineOptions::default()
  };
  let engine = Arc::new(open_with(&project, &scratch.path().join("home"), options));
  let handle = engine.watch().unwrap();
  assert_eq!(fresh(&engine).header().scan, ScanKind::StatScan);
  assert!(wait_for(Duration::from_secs(5), || fresh(&engine).header().scan == ScanKind::WatcherClean));

  append(&tree.join("acme-sdk/src/money.ts"), "\nexport function watched() {\n  return 3;\n}\n");

  assert!(
    wait_for(Duration::from_secs(15), || !named(&engine, "watched", "acme-sdk").is_empty()),
    "the watcher did not refresh the index"
  );
  assert!(wait_for(Duration::from_secs(5), || fresh(&engine).header().scan == ScanKind::WatcherClean));
  handle.inject_error("simulated overflow");
  assert_eq!(fresh(&engine).header().scan, ScanKind::StatScan);
  assert!(wait_for(Duration::from_secs(5), || fresh(&engine).header().scan == ScanKind::WatcherClean));
  drop(handle);
  assert_eq!(fresh(&engine).header().scan, ScanKind::StatScan);
}

#[test]
fn should_mark_inferred_facts_stale_when_their_evidence_changes() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let subject = single(&engine, "toCents", "acme-sdk", EntityKind::Function);
  let hash = file_hash(&engine, "acme-sdk", "src/money.ts");
  let fact = engine
    .store()
    .write(|tx| tx.insert_inferred_fact(&inferred(subject.id, "acme-sdk", "src/money.ts", hash)))
    .unwrap();
  append(&tree.join("acme-sdk/src/money.ts"), "\n// edited\n");

  let report = fresh(&engine);

  assert_eq!(report.header().stale_inferred_facts, 1);
  let default_view = engine.store().read(|tx| tx.inferred_facts(&InferredQuery::default())).unwrap();
  assert!(default_view.is_empty());
  let stored = engine.store().read(|tx| tx.inferred_fact(fact)).unwrap().unwrap();
  assert_eq!(
    stored.freshness,
    Freshness::Stale {
      reason: "acme-sdk:src/money.ts changed".to_owned()
    }
  );
  assert_eq!(engine.status().unwrap().inferred.stale, 1);
}

fn git(dir: &Path, args: &[&str]) -> bool {
  std::process::Command::new("git")
    .args(args)
    .current_dir(dir)
    .env("GIT_AUTHOR_NAME", "atlas")
    .env("GIT_AUTHOR_EMAIL", "atlas@example.com")
    .env("GIT_COMMITTER_NAME", "atlas")
    .env("GIT_COMMITTER_EMAIL", "atlas@example.com")
    .output()
    .is_ok_and(|output| output.status.success())
}

#[test]
fn should_match_a_full_index_after_a_branch_switch() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let sdk = tree.join("acme-sdk");
  if !git(&sdk, &["init", "-q"]) {
    return;
  }
  assert!(git(&sdk, &["add", "-A"]) && git(&sdk, &["commit", "-qm", "base"]));
  let money = sdk.join("src/money.ts");
  fs::write(&money, replace_word(&fs::read_to_string(&money).unwrap(), "toCents", "toMinorUnits")).unwrap();
  fs::remove_file(sdk.join("src/types.ts")).unwrap();
  fs::write(sdk.join("src/extra.ts"), "export function extra() {\n  return 42;\n}\n").unwrap();
  assert!(git(&sdk, &["add", "-A"]) && git(&sdk, &["commit", "-qm", "feature"]));
  let home = scratch.path().join("home");
  let engine = open(&project, &home);
  fresh(&engine);
  assert!(git(&sdk, &["checkout", "-q", "HEAD~1"]));

  let report = fresh(&engine);

  assert!(report.header().run.is_some());
  let reference = open(&project, &scratch.path().join("reference"));
  fresh(&reference);
  import_lineage(&engine, &reference);
  assert_eq!(digest(&engine), digest(&reference), "{}", common::fact_diff(&engine.db_path(), &reference.db_path()));
  assert!(!named(&engine, "toCents", "acme-sdk").is_empty());
  assert!(named(&engine, "extra", "acme-sdk").is_empty());
}

#[test]
fn should_remove_facts_and_report_an_error_when_a_repo_directory_is_deleted() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  assert!(!named(&engine, "toCents", "acme-sdk").is_empty());
  fs::remove_dir_all(tree.join("acme-sdk")).unwrap();

  let report = fresh(&engine);

  let FreshnessReport::Degraded { failed_files, reason, .. } = &report else {
    panic!("{report:?}")
  };
  assert!(
    failed_files.iter().any(|failed| failed.repo == repo("acme-sdk") && failed.path.is_none()),
    "{failed_files:?} {reason}"
  );
  assert!(named(&engine, "toCents", "acme-sdk").is_empty());
  assert!(engine.store().read(|tx| tx.files_in_repo(&repo("acme-sdk"))).unwrap().is_empty());
  let again = fresh(&engine);
  assert!(
    matches!(again, FreshnessReport::Degraded { .. }),
    "a missing repo must keep the barrier degraded: {again:?}"
  );
  assert_eq!(again.header().run, None);
  assert_eq!(engine.status().unwrap().repos_missing.len(), 1);
}

#[test]
fn should_flag_generated_files_and_their_symbols() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  fs::write(tree.join("acme-sdk/src/api.generated.ts"), "export function generatedClient() {\n  return 1;\n}\n").unwrap();
  fs::write(tree.join("acme-sdk/src/schema.ts"), "// @generated by protoc\nexport const schemaVersion = 3;\n").unwrap();
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);

  for (path, symbol) in [("src/api.generated.ts", "generatedClient"), ("src/schema.ts", "schemaVersion")] {
    let file = engine
      .store()
      .read(|tx| {
        tx.entity_by_key(&atlas_core::EntityKey::File {
          repo: repo("acme-sdk"),
          path: rel(path),
        })
      })
      .unwrap()
      .unwrap();
    assert_eq!(file.attrs["generated"], serde_json::json!(true), "{path}");
    let entity = named(&engine, symbol, "acme-sdk").remove(0);
    assert_eq!(entity.attrs["generated"], serde_json::json!(true), "{symbol}");
  }
  let plain = engine
    .store()
    .read(|tx| {
      tx.entity_by_key(&atlas_core::EntityKey::File {
        repo: repo("acme-sdk"),
        path: rel("src/money.ts"),
      })
    })
    .unwrap()
    .unwrap();
  assert_eq!(plain.attrs["generated"], serde_json::json!(false));
  assert!(!single(&engine, "toCents", "acme-sdk", EntityKind::Function).attrs.contains_key("generated"));
}

#[test]
fn should_report_status_and_run_ledger() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);

  let status = engine.status().unwrap();

  assert_eq!(status.runs.len(), 1);
  let run = &status.runs[0];
  assert_eq!(run.trigger, "barrier");
  assert_eq!(run.status, RunStatus::Ok);
  for phase in ["plan", "analyze", "symbols", "resolve", "persist", "passes", "similarity"] {
    assert!(run.report.phase_timings_ms.contains_key(phase), "missing phase {phase}");
  }
  let files: u64 = status.files_by_language.values().sum();
  assert_eq!(files, run.report.counts["files_analyzed"]);
  assert_eq!(status.files_by_parse_status.get("failed"), None);
  assert!(
    status.files_by_language.contains_key("rust") && status.files_by_language.contains_key("typescript"),
    "{:?}",
    status.files_by_language
  );
  assert!(status.unresolved_by_reason.contains_key("external"), "{:?}", status.unresolved_by_reason);
  assert!(status.edge_certainty["contains"].contains_key("syntactic"));
  assert!(status.edge_certainty["calls"].contains_key("resolved"));
  assert_eq!(status.scip.lag, Some(ScipLag::NeverRun));
  assert_eq!(status.scip.files_with_scip, 0);
  assert_eq!(status.inferred, atlas_engine::InferredCounts::default());
  assert_eq!(status.llm.calls, 0);
  let applications = engine.store().read(|tx| tx.entities_by_kind(&EntityKind::Application, &EntityFilter::default())).unwrap();
  let names: Vec<&str> = applications.iter().map(|app| app.name.as_str()).collect();
  assert!(names.contains(&"acme-billing") && names.contains(&"acme-api"), "{names:?}");
}

#[test]
fn should_rebuild_the_same_facts_from_a_full_reindex() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let before = digest(&engine);

  let summary = engine.index(IndexMode::Full).unwrap();

  assert_eq!(summary.counts["files_analyzed"], engine.status().unwrap().files_by_language.values().sum::<u64>());
  assert_eq!(digest(&engine), before);
  let lineage = engine
    .store()
    .read(|tx| {
      tx.owned_facts(&Owner::Project {
        pass: atlas_engine::PASS_LINEAGE.to_owned(),
      })
    })
    .unwrap();
  assert!(lineage.is_empty());
}

#[test]
fn should_skip_enrichment_without_affecting_freshness_when_the_llm_is_unavailable() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let providers = atlas_semantic::TaskProviders::from_config(&atlas_semantic::ProviderConfig::None { reason: "offline".to_owned() });
  let options = EngineOptions {
    llm_providers: Some(Arc::new(providers)),
    ..EngineOptions::default()
  };
  let engine = open_with(&project, &scratch.path().join("home"), options);
  fresh(&engine);

  let report = engine.enrich(atlas_semantic::SemanticBudget::default()).unwrap();

  assert!(report.candidates > 0, "{report:?}");
  assert!(
    matches!(report.similarity.status, atlas_semantic::SemanticRunStatus::ProviderUnavailable { .. }),
    "{:?}",
    report.similarity.status
  );
  assert!(
    report
      .similarity
      .tasks
      .iter()
      .all(|task| matches!(task.outcome, atlas_semantic::TaskOutcome::Skipped { .. }))
  );
  assert_eq!(report.similarity.provider_calls, 0);
  let after = fresh(&engine);
  assert!(after.is_fresh());
  assert_eq!(after.header().run, None);
  assert_eq!(engine.status().unwrap().inferred, atlas_engine::InferredCounts::default());
}

struct PanickingPass;

impl atlas_engine::ProjectPass for PanickingPass {
  fn name(&self) -> &'static str {
    "panicking"
  }

  fn run(&self, _tx: &atlas_store::ReadTx<'_>, _context: &atlas_engine::PassContext<'_>) -> Result<atlas_engine::PassOutput, atlas_engine::PassError> {
    panic!("pass bug");
  }
}

#[test]
fn should_turn_a_panicking_project_pass_into_a_partial_degraded_run() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let options = EngineOptions {
    passes: vec![Arc::new(PanickingPass)],
    ..EngineOptions::default()
  };
  let engine = open_with(&project, &scratch.path().join("home"), options);

  let report = fresh(&engine);

  let FreshnessReport::Degraded { reason, .. } = &report else { panic!("{report:?}") };
  assert!(reason.contains("project pass panicking failed: panicked: pass bug"), "{reason}");
  let run = engine.store().read(|tx| tx.runs(1)).unwrap().remove(0);
  assert!(matches!(&run.status, RunStatus::Partial { reason } if reason.contains("panicking")), "{:?}", run.status);
  assert!(!named(&engine, "toCents", "acme-sdk").is_empty());
}

#[test]
fn should_recover_a_consistent_index_after_an_interrupted_run() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let home = scratch.path().join("home");
  let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
  let options = EngineOptions {
    fault_injection: FaultInjection::crash_after_persist(Arc::clone(&armed)),
    ..EngineOptions::default()
  };
  let crashing = open_with(&project, &home, options);
  fresh(&crashing);
  let unrelated = crashing.store().write(|tx| tx.begin_run("scip")).unwrap();
  let money = tree.join("acme-api/src/common/money.ts");
  fs::write(&money, replace_word(&fs::read_to_string(&money).unwrap(), "formatMoney", "formatAmount")).unwrap();
  armed.store(true, std::sync::atomic::Ordering::SeqCst);
  let crashed = fresh(&crashing);
  assert!(
    matches!(&crashed, FreshnessReport::Degraded { reason, .. } if reason.contains("injected crash")),
    "{crashed:?}"
  );
  std::mem::forget(crashing);

  let recovered = open(&project, &home);
  let report = fresh(&recovered);

  assert!(report.header().run.is_some(), "an interrupted run must trigger recovery: {report:?}");
  let runs = recovered.store().read(|tx| tx.runs(10)).unwrap();
  assert!(runs.iter().any(|run| run.status == RunStatus::Failed { error: "interrupted".to_owned() }), "{runs:#?}");
  assert_eq!(runs.iter().find(|run| run.id == unrelated).unwrap().status, RunStatus::Running);
  let reference = open(&project, &scratch.path().join("reference"));
  fresh(&reference);
  import_lineage(&recovered, &reference);
  assert_eq!(digest(&recovered), digest(&reference), "{}", common::fact_diff(&recovered.db_path(), &reference.db_path()));
  let amount = single(&recovered, "formatAmount", "acme-api", EntityKind::Function);
  assert!(recovered.similarity().contains(amount.id), "the similarity index must be rebuilt after an interrupted run");
}

#[test]
fn should_reanalyze_files_whose_stored_artifact_is_outdated() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let home = scratch.path().join("home");
  let engine = open(&project, &home);
  fresh(&engine);
  let before = digest(&engine);
  engine.store().write(|tx| tx.put_blob("artifact:acme-sdk:src/money.ts", 999, b"not an artifact")).unwrap();
  drop(engine);

  let reopened = open(&project, &home);
  let report = fresh(&reopened);

  assert!(report.is_fresh(), "{report:?}");
  let run = reopened.store().read(|tx| tx.runs(1)).unwrap().remove(0);
  assert_eq!(run.report.counts["files_analyzed"], 1);
  assert_eq!(digest(&reopened), before);
  assert_eq!(fresh(&reopened).header().run, None);
}
