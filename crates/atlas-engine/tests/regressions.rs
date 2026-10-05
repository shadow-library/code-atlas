mod common;

use std::fs;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use atlas_core::{
  Availability, Certainty, Confidence, DetectInput, DetectedEdge, DetectedEntity, Detection, Detector, Diagnostic, EdgeKind, EntityKey, Evidence, FileFacts, NodeRef, Owner,
  Producer, RelPath, RepoName, Span,
};
use atlas_detect::DiagnosingDetector;
use atlas_engine::{Engine, EngineError, EngineOptions, FaultInjection, FreshnessReport, IndexMode, PassContext, PassError, PassOutput, ProjectPass, ScipIndexers, ScipLag};
use atlas_project::Project;
use atlas_scip::{CacheStatus, ScipFacts, ScipIndexer, ScipOutcome, ScipRequest, ScipRun};
use atlas_store::{EdgeFilter, EntityFilter, FreshnessPolicy, InferredQuery, NewInferredFact, ReadTx, RunStatus};
use common::edits::{Tree, replace_word};
use common::{ACME_REPOS, SplitMix, acme_copy, digest, fact_diff, fresh, import_lineage, open, open_with, project, repo_config};
use protobuf::{EnumOrUnknown, Message};
use scip::types::{Document, Index, Metadata, Occurrence, PositionEncoding, SymbolRole, ToolInfo};

fn mismatch_with(project: &Project, engine: &Engine, scratch: &Path, label: &str, options: EngineOptions) -> Option<String> {
  let home = scratch.join(format!("ref-{label}"));
  let reference = open_with(project, &home, options);
  fresh(&reference);
  import_lineage(engine, &reference);
  let expected = digest(&reference);
  let actual = digest(engine);
  let result = (actual != expected).then(|| fact_diff(&engine.db_path(), &reference.db_path()));
  drop(reference);
  let _ = fs::remove_dir_all(&home);
  result
}

fn mismatch(project: &Project, engine: &Engine, scratch: &Path, label: &str) -> Option<String> {
  mismatch_with(project, engine, scratch, label, EngineOptions::default())
}

fn named(engine: &Engine, name: &str) -> Vec<atlas_core::Entity> {
  engine.store().read(|tx| tx.entities_by_name(name, &EntityFilter::default())).unwrap()
}

#[test]
fn should_reload_when_another_process_indexes_between_the_scan_and_the_index_lock() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let home = scratch.path().join("home");
  let armed = Arc::new(AtomicBool::new(false));
  let (arrived_sender, arrived) = mpsc::channel::<()>();
  let (release, released) = mpsc::channel::<()>();
  let hook = {
    let armed = Arc::clone(&armed);
    let arrived_sender = Mutex::new(arrived_sender);
    let released = Mutex::new(released);
    move || {
      if armed.swap(false, Ordering::SeqCst) {
        arrived_sender.lock().unwrap().send(()).unwrap();
        released.lock().unwrap().recv().unwrap();
      }
    }
  };
  let options = EngineOptions {
    fault_injection: FaultInjection::before_index_lock(hook),
    ..EngineOptions::default()
  };
  let a = Arc::new(open_with(&project, &home, options));
  fresh(&a);
  let b = open(&project, &home);
  assert_eq!(fresh(&b).header().run, None);
  fs::write(tree.join("acme-api/src/common/fresh.ts"), "export function newThing() {\n  return 1;\n}\n").unwrap();
  armed.store(true, Ordering::SeqCst);
  let waiting = {
    let a = Arc::clone(&a);
    thread::spawn(move || a.ensure_fresh().unwrap())
  };
  arrived.recv_timeout(Duration::from_secs(60)).unwrap();
  for path in ["src/common/money.ts", "src/orders/orders.service.ts", "src/reports/reports.service.ts"] {
    let file = tree.join("acme-api").join(path);
    fs::write(&file, replace_word(&fs::read_to_string(&file).unwrap(), "formatMoney", "formatCents")).unwrap();
  }
  fs::write(
    tree.join("acme-api/src/common/slow.ts"),
    "import { newThing } from './fresh';\n\nexport function slowCaller() {\n  return newThing();\n}\n",
  )
  .unwrap();
  assert!(fresh(&b).header().run.is_some());
  release.send(()).unwrap();
  let report = waiting.join().unwrap();

  assert!(report.is_fresh(), "{report:?}");
  assert_eq!(report.header().run, None, "the waiting engine must reuse the other engine's run: {report:?}");
  let diff = mismatch(&project, &a, scratch.path(), "race");
  assert!(diff.is_none(), "stale facts after the race:\n{}", diff.unwrap_or_default());
  assert_eq!(fresh(&a).header().run, None);
}

struct CrossFileDetector;

impl Detector for CrossFileDetector {
  fn id(&self) -> &'static str {
    "test-cross-file"
  }

  fn producer(&self) -> Producer {
    Producer::new("test-cross-file", 1)
  }

  fn applies_to(&self, facts: &FileFacts) -> bool {
    matches!(facts.file.path.file_name(), "consumer.ts" | "producer.ts")
  }

  fn detect(&self, input: &DetectInput<'_>) -> Detection {
    let file = &input.facts.file;
    let target = EntityKey::EnvVar {
      repo: file.repo.clone(),
      name: "SYNTH_TARGET".to_owned(),
    };
    let span = Span::from_zero_based_rows(0, 1, 0, 0, 0, 1);
    let mut detection = Detection::default();
    match file.path.file_name() {
      "consumer.ts" if !input.facts.symbols.is_empty() => detection.edges.push(DetectedEdge {
        kind: EdgeKind::ReadsEnv,
        from: NodeRef::Local(input.facts.symbols[0].local),
        to: NodeRef::Key(target),
        certainty: Certainty::Syntactic,
        span,
        note: None,
        attrs: Default::default(),
      }),
      "producer.ts" if input.source.contains("SYNTH_TARGET") => detection.entities.push(DetectedEntity {
        key: target,
        name: "SYNTH_TARGET".to_owned(),
        certainty: Certainty::Syntactic,
        span,
        attrs: Default::default(),
      }),
      _ => {}
    }
    detection
  }
}

impl DiagnosingDetector for CrossFileDetector {
  fn detect_with_diagnostics(&self, input: &DetectInput<'_>) -> (Detection, Vec<Diagnostic>) {
    (self.detect(input), Vec::new())
  }
}

fn cross_file_options() -> EngineOptions {
  EngineOptions {
    extra_detectors: vec![Arc::new(CrossFileDetector)],
    ..EngineOptions::default()
  }
}

#[test]
fn should_re_resolve_a_file_whose_detector_key_target_another_file_emits() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let sdk = tree.join("acme-sdk/src");
  fs::write(sdk.join("consumer.ts"), "export function consume() {\n  return 1;\n}\n").unwrap();
  fs::write(sdk.join("producer.ts"), "export const marker = 'SYNTH_TARGET';\n").unwrap();
  let engine = open_with(&project, &scratch.path().join("home"), cross_file_options());
  fresh(&engine);
  let consume = named(&engine, "consume").remove(0);
  let reads = |engine: &Engine| {
    let filter = EdgeFilter {
      kinds: vec![EdgeKind::ReadsEnv],
      ..EdgeFilter::default()
    };
    engine.store().read(|tx| tx.edges_from(consume.id, &filter)).unwrap().len()
  };
  assert_eq!(reads(&engine), 1);

  for (step, (marker, expected)) in [("nothing", 0), ("SYNTH_TARGET", 1)].into_iter().enumerate() {
    fs::write(sdk.join("producer.ts"), format!("export const marker = '{marker}';\n")).unwrap();
    let report = fresh(&engine);

    assert!(report.header().run.is_some());
    let run = engine.store().read(|tx| tx.runs(1)).unwrap().remove(0);
    assert_eq!(run.report.counts["owners_resolved"], 2, "{:?}", run.report.counts);
    assert_eq!(run.report.counts["wanted_target_owners"], 1, "{:?}", run.report.counts);
    assert_eq!(reads(&engine), expected);
    let diff = mismatch_with(&project, &engine, scratch.path(), &format!("cross-{step}"), cross_file_options());
    assert!(diff.is_none(), "{}", diff.unwrap_or_default());
  }
}

#[test]
fn should_rebuild_a_similarity_index_that_was_not_persisted_before_the_engine_stopped() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let home = scratch.path().join("home");
  let engine = open(&project, &home);
  fresh(&engine);
  let removed = named(&engine, "formatMoney").remove(0).id;
  let slug = tree.join("acme-api/src/common/slug.ts");
  let mut text = fs::read_to_string(&slug).unwrap();
  text.push_str("\nexport function persistProbe(input: string) {\n  const parts = input.split('-');\n  return parts.map((part) => part.toUpperCase()).join('_');\n}\n");
  fs::write(&slug, text).unwrap();
  assert!(fresh(&engine).header().run.is_some());
  let money = tree.join("acme-api/src/common/money.ts");
  fs::write(&money, "export const currency = 'EUR';\n").unwrap();
  assert!(fresh(&engine).header().run.is_some());
  let added = named(&engine, "persistProbe").remove(0).id;
  assert!(engine.similarity().contains(added) && !engine.similarity().contains(removed));
  std::mem::forget(engine);

  let reopened = open(&project, &home);

  assert!(reopened.similarity().contains(added), "a symbol added by an unpersisted run is missing");
  assert!(!reopened.similarity().contains(removed), "a symbol deleted by an unpersisted run is still indexed");
  assert!(reopened.persist_similarity().unwrap());
  drop(reopened);
  let persisted = open(&project, &home);
  assert!(
    !persisted.persist_similarity().unwrap(),
    "closing the engine must persist the index for the current generation"
  );
  assert!(persisted.similarity().contains(added));
}

#[test]
fn should_report_a_project_configuration_change_until_the_engine_adopts_it() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let home = scratch.path().join("home");
  let mut fewer = project.clone();
  fewer.repos.retain(|repo| repo.name.as_str() != "acme-web");
  let old = open(&fewer, &home);
  fresh(&old);
  let grown = open(&project, &home);
  assert!(fresh(&grown).header().run.is_some());

  let report = fresh(&old);

  let FreshnessReport::Degraded { reason, header, .. } = &report else {
    panic!("{report:?}")
  };
  assert_eq!(reason, "project configuration changed; reopen the engine");
  assert_eq!(header.run, None);
  assert!(matches!(old.index(IndexMode::Incremental), Err(EngineError::ProjectConfigChanged)));
  old.reload_project(&project).unwrap();
  let adopted = fresh(&old);
  assert!(adopted.is_fresh() && adopted.header().run.is_none(), "{adopted:?}");
  assert_eq!(digest(&old), digest(&grown));
  let shrunk = open(&fewer, &home);
  assert!(fresh(&shrunk).header().run.is_some());
  assert!(shrunk.store().read(|tx| tx.files_in_repo(&RepoName::new("acme-web").unwrap())).unwrap().is_empty());
  assert!(mismatch(&fewer, &shrunk, scratch.path(), "shrunk").is_none());
  assert!(!fresh(&grown).is_fresh());
}

struct FakeScip {
  output: PathBuf,
}

impl ScipIndexer for FakeScip {
  fn id(&self) -> &'static str {
    "fake-scip"
  }

  fn availability(&self) -> Availability {
    Availability::Available
  }

  fn index(&self, _request: &ScipRequest) -> ScipRun {
    fs::write(&self.output, index_bytes()).unwrap();
    ScipRun {
      indexer: "fake-scip".to_owned(),
      version: Some("1".to_owned()),
      duration: Duration::ZERO,
      output_path: Some(self.output.clone()),
      outcome: ScipOutcome::Ok,
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
  let a_load = "scip-typescript npm demo 1.0.0 src/a/x.ts`load().";
  let c_load = "scip-typescript npm demo 1.0.0 src/c/x.ts`load().";
  let run = "scip-typescript npm demo 1.0.0 src/b.ts`run().";
  let mut tool = ToolInfo::new();
  tool.name = "scip-typescript".to_owned();
  let mut metadata = Metadata::new();
  metadata.tool_info = Some(tool).into();
  let mut index = Index::new();
  index.metadata = Some(metadata).into();
  index.documents = vec![
    document("src/a/x.ts", vec![occurrence(a_load, vec![0, 16, 20], true)]),
    document("src/c/x.ts", vec![occurrence(c_load, vec![0, 16, 20], true)]),
    document("src/b.ts", vec![occurrence(run, vec![1, 16, 19], true), occurrence(a_load, vec![2, 9, 13], false)]),
  ];
  index.write_to_bytes().unwrap()
}

#[test]
fn should_drop_compiler_verified_edges_when_resolution_changes() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("demo");
  fs::create_dir_all(root.join("src/a")).unwrap();
  fs::create_dir_all(root.join("src/c")).unwrap();
  fs::write(root.join("package.json"), "{ \"name\": \"demo\" }\n").unwrap();
  let tsconfig = |dir: &str| format!("{{ \"compilerOptions\": {{ \"baseUrl\": \".\", \"paths\": {{ \"@lib/*\": [\"src/{dir}/*\"] }} }} }}\n");
  fs::write(root.join("tsconfig.json"), tsconfig("a")).unwrap();
  let load = "export function load(n: number) {\n  return n;\n}\n";
  fs::write(root.join("src/a/x.ts"), load).unwrap();
  fs::write(root.join("src/c/x.ts"), load).unwrap();
  fs::write(root.join("src/b.ts"), "import { load } from '@lib/x';\nexport function run() {\n  return load(1);\n}\n").unwrap();
  let project = project("demo", vec![repo_config("demo", &root)]);
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let fake = FakeScip {
    output: scratch.path().join("index.scip"),
  };
  engine.scip(&ScipIndexers { cargo: None, npm: Some(&fake) }).unwrap();
  let owner = Owner::Project {
    pass: ScipFacts::owner_pass(&RepoName::new("demo").unwrap(), &RelPath::new("src/b.ts").unwrap()),
  };
  let before = engine.store().read(|tx| tx.owned_facts(&owner)).unwrap();
  assert!(before.edges.iter().any(|edge| edge.certainty == Certainty::CompilerVerified));

  fs::write(root.join("tsconfig.json"), tsconfig("c")).unwrap();
  let after = fresh(&engine);
  let kept = engine.store().read(|tx| tx.owned_facts(&owner)).unwrap();
  assert!(kept.edges.is_empty(), "a compiler-verified edge survived a tsconfig paths change");
  assert_eq!(after.header().scip, ScipLag::Behind { files: 3 });
}

fn multi_sequence(seed: u64, steps: usize, per_step: usize) {
  let scratch = tempfile::tempdir().unwrap();
  let tree_root = scratch.path().join("tree");
  let project = acme_copy(&tree_root);
  let tree = Tree { root: tree_root };
  let home = scratch.path().join("home");
  let engine = open(&project, &home);
  fresh(&engine);
  let mut rng = SplitMix(seed);
  for step in 1..=steps {
    let mut descriptions = Vec::new();
    for index in 0..per_step {
      descriptions.push(tree.apply(&mut rng, step * 100 + index));
    }
    let report = fresh(&engine);
    assert!(report.header().run.is_some());
    if let Some(diff) = mismatch(&project, &engine, scratch.path(), &format!("{step}")) {
      panic!("seed {seed:#x} step {step} edits {descriptions:#?}\n{diff}");
    }
  }
}

#[test]
fn should_match_a_full_index_after_batches_of_four_edits() {
  multi_sequence(0xA11C_E001, 20, 4);
}

#[test]
fn should_match_a_full_index_after_batches_of_six_edits() {
  multi_sequence(0xA11C_E002, 20, 6);
}

#[test]
fn should_match_a_full_index_after_batches_of_ten_edits() {
  multi_sequence(0xA11C_E003, 15, 10);
}

#[test]
fn should_match_a_full_index_after_batches_of_five_edits() {
  multi_sequence(0xA11C_E004, 15, 5);
}

type Edit = Box<dyn Fn(&Path)>;

fn edit(path: &'static str, from: &'static str, to: &'static str) -> Edit {
  Box::new(move |root: &Path| {
    let file = root.join(path);
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.contains(from), "{path} lacks {from:?}");
    fs::write(&file, text.replacen(from, to, 1)).unwrap();
  })
}

#[test]
fn should_match_a_full_index_after_targeted_edit_kinds() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let cases: Vec<(&str, Edit)> = vec![
    ("orphan rust module (remove pub mod money)", edit("acme-billing/src/lib.rs", "pub mod money;\n", "")),
    (
      "restore rust module",
      edit("acme-billing/src/lib.rs", "pub mod handlers;\n", "pub mod handlers;\npub mod money;\n"),
    ),
    (
      "rename re-exported export in source only",
      edit("acme-sdk/src/money.ts", "export function toCents(", "export function toCentz("),
    ),
    (
      "restore re-exported export",
      edit("acme-sdk/src/money.ts", "export function toCentz(", "export function toCents("),
    ),
    ("drop barrel export *", edit("acme-sdk/src/index.ts", "export * from './types';\n", "")),
    (
      "restore barrel export *",
      edit(
        "acme-sdk/src/index.ts",
        "export * from './client';\n",
        "export * from './client';\nexport * from './types';\n",
      ),
    ),
    ("rename npm package", edit("acme-sdk/package.json", "\"name\": \"@acme/sdk\"", "\"name\": \"@acme/sdk2\"")),
    ("restore npm package", edit("acme-sdk/package.json", "\"name\": \"@acme/sdk2\"", "\"name\": \"@acme/sdk\"")),
    (
      "move file across repos",
      Box::new(|root: &Path| {
        fs::rename(root.join("acme-api/src/common/slug.ts"), root.join("acme-web/src/utils/slug.ts")).unwrap();
      }),
    ),
    (
      "add cargo bin target file",
      Box::new(|root: &Path| {
        fs::create_dir_all(root.join("acme-billing/src/bin")).unwrap();
        fs::write(
          root.join("acme-billing/src/bin/tool.rs"),
          "use acme_billing::money::to_cents;\n\nfn main() {\n  let _ = to_cents(\"1\");\n}\n",
        )
        .unwrap();
      }),
    ),
    (
      "empty file",
      Box::new(|root: &Path| {
        fs::write(root.join("acme-web/src/utils/empty.ts"), "").unwrap();
      }),
    ),
    (
      "binary file",
      Box::new(|root: &Path| {
        fs::write(root.join("acme-web/src/utils/blob.ts"), [0xff_u8, 0xfe, 0x00, 0x41]).unwrap();
      }),
    ),
    (
      "binary file fixed",
      Box::new(|root: &Path| fs::write(root.join("acme-web/src/utils/blob.ts"), "export const blob = 1;\n").unwrap()),
    ),
    ("generated header flip", edit("acme-web/src/utils/currency.ts", "", "// @generated by probe\n")),
    (
      "tsconfig extends",
      Box::new(|root: &Path| {
        fs::write(
          root.join("acme-api/tsconfig.base.json"),
          "{ \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@lib/*\": [\"src/common/*\"] } } }\n",
        )
        .unwrap();
        let config = root.join("acme-api/tsconfig.json");
        let text = fs::read_to_string(&config).unwrap();
        fs::write(&config, text.replacen("{", "{\n  \"extends\": \"./tsconfig.base.json\",", 1)).unwrap();
        let orders = root.join("acme-api/src/orders/orders.service.ts");
        let text = fs::read_to_string(&orders).unwrap();
        fs::write(&orders, format!("import {{ slugify as viaAlias }} from '@lib/slug';\n{text}")).unwrap();
      }),
    ),
    (
      "edit only the extended tsconfig",
      Box::new(|root: &Path| {
        fs::write(
          root.join("acme-api/tsconfig.base.json"),
          "{ \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@lib/*\": [\"src/users/*\"] } } }\n",
        )
        .unwrap();
      }),
    ),
    (
      "delete a whole directory",
      Box::new(|root: &Path| {
        fs::remove_dir_all(root.join("acme-api/src/reports")).unwrap();
      }),
    ),
  ];
  let mut failures = Vec::new();
  for (index, (label, apply)) in cases.iter().enumerate() {
    apply(&tree);
    let report = fresh(&engine);
    eprintln!("{label}: run={:?} fresh={}", report.header().run, report.is_fresh());
    if report.header().run.is_none() {
      failures.push(format!("{label}: NO RUN TRIGGERED"));
    }
    if let Some(diff) = mismatch(&project, &engine, scratch.path(), &format!("t{index}")) {
      failures.push(format!("{label}:\n{diff}"));
    }
  }
  assert!(failures.is_empty(), "{}", failures.join("\n=====\n"));
}

#[test]
fn should_index_a_symlinked_repo_root_like_its_target() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let real = acme_copy(&tree);
  let link = scratch.path().join("link-api");
  std::os::unix::fs::symlink(tree.join("acme-api"), &link).unwrap();
  let mut linked = real.clone();
  for repo in &mut linked.repos {
    if repo.name.as_str() == "acme-api" {
      repo.path = link.clone();
    }
  }
  let via_link = open(&linked, &scratch.path().join("home-link"));
  fresh(&via_link);
  let direct = open(&real, &scratch.path().join("home-real"));
  fresh(&direct);
  let count = |engine: &Engine| engine.store().read(|tx| tx.files_in_repo(&RepoName::new("acme-api").unwrap())).unwrap().len();
  assert_eq!(count(&via_link), count(&direct));
  assert_eq!(digest(&via_link), digest(&direct));
  let file = tree.join("acme-api/src/common/slug.ts");
  let mut text = fs::read_to_string(&file).unwrap();
  text.push_str("\nexport function viaLink() {\n  return 1;\n}\n");
  fs::write(&file, text).unwrap();
  let report = fresh(&via_link);
  assert!(report.header().run.is_some());
}

#[test]
fn should_not_migrate_inferred_facts_between_unrelated_tiny_functions() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let file = tree.join("acme-api/src/users/email.ts");
  let mut text = fs::read_to_string(&file).unwrap();
  text.push_str("\nexport function isAdmin(user: { admin: boolean }) {\n  return user.admin;\n}\n");
  fs::write(&file, &text).unwrap();
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let old = engine.store().read(|tx| tx.entities_by_name("isAdmin", &atlas_store::EntityFilter::default())).unwrap();
  assert_eq!(old.len(), 1);
  let hash = engine
    .store()
    .read(|tx| tx.file(&RepoName::new("acme-api").unwrap(), &RelPath::new("src/users/email.ts").unwrap()))
    .unwrap()
    .unwrap()
    .content_hash;
  engine
    .store()
    .write(|tx| {
      tx.insert_inferred_fact(&NewInferredFact {
        subjects: vec![old[0].id],
        relation: "security_check".to_owned(),
        payload: serde_json::json!({ "note": "authorization gate" }),
        certainty: Certainty::Inferred {
          confidence: Confidence::new(0.9).unwrap(),
          provider: "fake".to_owned(),
          model: "fake".to_owned(),
          rationale: "probe".to_owned(),
        },
        evidence: vec![Evidence {
          repo: RepoName::new("acme-api").unwrap(),
          path: RelPath::new("src/users/email.ts").unwrap(),
          span: Span::from_zero_based_rows(0, 1, 0, 0, 0, 1),
          content_hash: hash,
          producer: Producer::new("test", 1),
          note: None,
        }],
        created_run: None,
        llm_call: None,
      })
    })
    .unwrap();
  let replaced = text.replace(
    "export function isAdmin(user: { admin: boolean }) {\n  return user.admin;\n}\n",
    "export function isArchived(order: { archived: boolean }) {\n  return order.archived;\n}\n",
  );
  fs::write(&file, replaced).unwrap();
  fresh(&engine);
  let new = engine.store().read(|tx| tx.entities_by_name("isArchived", &atlas_store::EntityFilter::default())).unwrap();
  let migrated = engine
    .store()
    .read(|tx| {
      tx.inferred_facts(&InferredQuery {
        subject: Some(new[0].id),
        relation: None,
        freshness: FreshnessPolicy::IncludeStale,
      })
    })
    .unwrap();
  assert!(migrated.is_empty(), "isAdmin's security fact must not migrate onto isArchived: {migrated:#?}");
}

const WAIT_DEADLINE: Duration = Duration::from_secs(30);
const EVENT_QUIET: Duration = Duration::from_millis(300);

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
  let deadline = Instant::now() + WAIT_DEADLINE;
  while !condition() {
    assert!(Instant::now() < deadline, "timed out waiting for {what}");
    thread::sleep(Duration::from_millis(10));
  }
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

struct GatePass {
  armed: Arc<AtomicBool>,
  entered: Arc<AtomicBool>,
  release: Arc<AtomicBool>,
}

impl ProjectPass for GatePass {
  fn name(&self) -> &'static str {
    "gate-probe"
  }

  fn run(&self, _tx: &ReadTx<'_>, _context: &PassContext<'_>) -> Result<PassOutput, PassError> {
    if self.armed.load(Ordering::SeqCst) {
      self.entered.store(true, Ordering::SeqCst);
      wait_until("the test to release the pass", || self.release.load(Ordering::SeqCst));
    }
    Ok(PassOutput::default())
  }
}

#[test]
fn should_not_report_watcher_clean_while_events_are_queued() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let armed = Arc::new(AtomicBool::new(false));
  let entered = Arc::new(AtomicBool::new(false));
  let release = Arc::new(AtomicBool::new(false));
  let options = EngineOptions {
    passes: vec![Arc::new(GatePass {
      armed: Arc::clone(&armed),
      entered: Arc::clone(&entered),
      release: Arc::clone(&release),
    })],
    watch_debounce: Duration::from_millis(50),
    ..EngineOptions::default()
  };
  let engine = Arc::new(open_with(&project, &scratch.path().join("home"), options));
  fresh(&engine);
  let watch = engine.watch().unwrap();
  let mut stale = Vec::new();
  for attempt in 0..8 {
    wait_for_quiet_events(&watch);
    fresh(&engine);
    entered.store(false, Ordering::SeqCst);
    release.store(false, Ordering::SeqCst);
    armed.store(true, Ordering::SeqCst);
    let x = tree.join("acme-api/src/common/money.ts");
    let mut text = fs::read_to_string(&x).unwrap();
    text.push_str(&format!("\nexport const touchX{attempt} = {attempt};\n"));
    fs::write(&x, text).unwrap();
    wait_until("the watcher run to enter the gated pass", || entered.load(Ordering::SeqCst));
    let before_y = watch.events_seen();
    let y = tree.join("acme-api/src/common/slug.ts");
    let mut text = fs::read_to_string(&y).unwrap();
    text.push_str(&format!("\nexport function watchProbe{attempt}() {{\n  return {attempt};\n}}\n"));
    fs::write(&y, text).unwrap();
    wait_until("the watcher to deliver the second edit while the run is in flight", || watch.events_seen() > before_y);
    wait_for_quiet_events(&watch);
    armed.store(false, Ordering::SeqCst);
    release.store(true, Ordering::SeqCst);
    fresh(&engine);
    let found = engine
      .store()
      .read(|tx| tx.entities_by_name(&format!("watchProbe{attempt}"), &atlas_store::EntityFilter::default()))
      .unwrap();
    if found.is_empty() {
      stale.push(attempt);
    }
  }
  assert!(stale.is_empty(), "stale watcher-clean answers on attempts {stale:?}");
}

#[test]
fn should_purge_a_removed_repo_even_after_a_version_bump() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let home = scratch.path().join("home");
  let engine = open(&project, &home);
  fresh(&engine);
  engine
    .store()
    .write(|tx| -> Result<(), atlas_store::StoreError> {
      let blob = tx.blob("engine:meta")?.unwrap();
      let mut meta: serde_json::Value = serde_json::from_slice(&blob.bytes).unwrap();
      meta["facts_version"] = serde_json::json!(0);
      tx.put_blob("engine:meta", blob.version, &serde_json::to_vec(&meta).unwrap())
    })
    .unwrap();
  drop(engine);
  let mut fewer = project.clone();
  fewer.repos.retain(|repo| repo.name.as_str() != "acme-web");
  let upgraded = open(&fewer, &home);
  let report = fresh(&upgraded);
  let web_files = upgraded.store().read(|tx| tx.files_in_repo(&RepoName::new("acme-web").unwrap())).unwrap().len();
  let web_entities = upgraded
    .store()
    .read(|tx| tx.entities_by_name("formatCurrency", &atlas_store::EntityFilter::default()))
    .unwrap()
    .into_iter()
    .filter(|entity| entity.repo.as_ref().is_some_and(|repo| repo.as_str() == "acme-web"))
    .count();
  assert_eq!(web_entities, 0);
  assert!(report.header().run.is_some());
  let diff = mismatch(&fewer, &upgraded, scratch.path(), "bump");
  assert!(
    web_files == 0 && diff.is_none(),
    "removed repo acme-web still has {web_files} file records; diff:\n{}",
    diff.unwrap_or_default()
  );
}

struct PanicPass {
  armed: Arc<AtomicBool>,
}

impl ProjectPass for PanicPass {
  fn name(&self) -> &'static str {
    "panic-probe"
  }

  fn run(&self, _tx: &ReadTx<'_>, _context: &PassContext<'_>) -> Result<PassOutput, PassError> {
    if self.armed.load(Ordering::SeqCst) {
      panic!("simulated pass bug");
    }
    Ok(PassOutput::default())
  }
}

#[test]
fn should_keep_refreshing_when_a_project_pass_panics_under_the_watcher() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let armed = Arc::new(AtomicBool::new(false));
  let options = EngineOptions {
    passes: vec![Arc::new(PanicPass { armed: Arc::clone(&armed) })],
    watch_debounce: Duration::from_millis(50),
    ..EngineOptions::default()
  };
  let engine = Arc::new(open_with(&project, &scratch.path().join("home"), options));
  fresh(&engine);
  let watch = engine.watch().unwrap();
  thread::sleep(Duration::from_millis(300));
  fresh(&engine);
  armed.store(true, Ordering::SeqCst);
  let x = tree.join("acme-api/src/common/money.ts");
  let mut text = fs::read_to_string(&x).unwrap();
  text.push_str("\nexport const touchX = 1;\n");
  fs::write(&x, text).unwrap();
  thread::sleep(Duration::from_millis(1000));
  armed.store(false, Ordering::SeqCst);
  fresh(&engine);
  let y = tree.join("acme-api/src/common/slug.ts");
  let mut text = fs::read_to_string(&y).unwrap();
  text.push_str("\nexport function deadWatcherProbe() {\n  return 1;\n}\n");
  fs::write(&y, text).unwrap();
  thread::sleep(Duration::from_millis(500));
  let report = fresh(&engine);
  let found = engine
    .store()
    .read(|tx| tx.entities_by_name("deadWatcherProbe", &atlas_store::EntityFilter::default()))
    .unwrap();
  assert!(!found.is_empty(), "barrier trusted a stalled watcher ({:?}, {:?})", report.header().scan, watch.state());
}

const CHILD_HELPER: &str = "should_abort_mid_persist_when_run_as_a_crash_child";

#[test]
#[ignore = "helper: runs only as a child process of a crash regression test"]
fn should_abort_mid_persist_when_run_as_a_crash_child() {
  let (Ok(tree), Ok(home)) = (std::env::var("ATLAS_CRASH_CHILD_TREE"), std::env::var("ATLAS_CRASH_CHILD_HOME")) else {
    return;
  };
  let tree = PathBuf::from(tree);
  let options = EngineOptions {
    batch_max_owners: 1,
    ..EngineOptions::default()
  };
  let project = project("acme", ["acme-api", "acme-web"].iter().map(|name| repo_config(name, &tree.join(name))).collect());
  let engine = open_with(&project, Path::new(&home), options);
  let db = engine.db_path();
  thread::spawn(move || {
    let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    loop {
      let count: i64 = conn.query_row("SELECT COUNT(*) FROM files WHERE repo = 'acme-web'", [], |row| row.get(0)).unwrap_or(0);
      if count >= 5 {
        std::process::abort();
      }
    }
  });
  fresh(&engine);
}

#[test]
fn should_purge_a_repo_whose_first_index_crashed_after_it_was_removed() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  acme_copy(&tree);
  let bulk = tree.join("acme-web/src/bulk");
  fs::create_dir_all(&bulk).unwrap();
  for index in 0..300 {
    fs::write(bulk.join(format!("bulk{index}.ts")), format!("export function bulk{index}() {{\n  return {index};\n}}\n")).unwrap();
  }
  let home = scratch.path().join("home");
  let api_only = project("acme", vec![repo_config("acme-api", &tree.join("acme-api"))]);
  fresh(&open(&api_only, &home));
  let status = std::process::Command::new(std::env::current_exe().unwrap())
    .args(["--exact", CHILD_HELPER, "--ignored", "--test-threads=1"])
    .env("ATLAS_CRASH_CHILD_TREE", &tree)
    .env("ATLAS_CRASH_CHILD_HOME", &home)
    .status()
    .unwrap();
  assert!(!status.success(), "the child must abort in the middle of its first index");

  let reopened = open(&api_only, &home);
  let crashed_files = reopened.store().read(|tx| tx.files_in_repo(&RepoName::new("acme-web").unwrap())).unwrap().len();
  assert!(crashed_files >= 5, "the crash must leave acme-web rows behind, found {crashed_files}");
  assert!(fresh(&reopened).header().run.is_some());

  assert!(reopened.store().read(|tx| tx.files_in_repo(&RepoName::new("acme-web").unwrap())).unwrap().is_empty());
  let diff = mismatch(&api_only, &reopened, scratch.path(), "crash-purge");
  assert!(diff.is_none(), "{}", diff.unwrap_or_default());
}

const EDIT_CHILD_HELPER: &str = "should_abort_mid_edit_refresh_when_run_as_a_crash_child";
const EDIT_BULK_FILES: usize = 300;

fn acme_web_rows(db: &Path) -> i64 {
  let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
  conn.query_row("SELECT COUNT(*) FROM files WHERE repo = 'acme-web'", [], |row| row.get(0)).unwrap_or(0)
}

#[test]
#[ignore = "helper: runs only as a child process of a crash regression test"]
fn should_abort_mid_edit_refresh_when_run_as_a_crash_child() {
  let (Ok(tree), Ok(home)) = (std::env::var("ATLAS_EDIT_CRASH_CHILD_TREE"), std::env::var("ATLAS_EDIT_CRASH_CHILD_HOME")) else {
    return;
  };
  let tree = PathBuf::from(tree);
  let options = EngineOptions {
    batch_max_owners: 1,
    ..EngineOptions::default()
  };
  let project = project("acme", ACME_REPOS.iter().map(|name| repo_config(name, &tree.join(name))).collect());
  let engine = open_with(&project, Path::new(&home), options);
  let db = engine.db_path();
  let baseline = acme_web_rows(&db);
  thread::spawn(move || {
    loop {
      if acme_web_rows(&db) >= baseline + 5 {
        std::process::abort();
      }
    }
  });
  let bulk = tree.join("acme-web/src/bulk");
  let _ = engine.write_then_refresh(&[], || {
    fs::create_dir_all(&bulk).unwrap();
    for index in 0..EDIT_BULK_FILES {
      fs::write(bulk.join(format!("bulk{index}.ts")), format!("export function bulk{index}() {{\n  return {index};\n}}\n")).unwrap();
    }
  });
}

#[test]
fn should_fail_an_interrupted_edit_run_and_reindex_its_writes_after_reopening() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let home = scratch.path().join("home");
  fresh(&open(&project, &home));
  let status = std::process::Command::new(std::env::current_exe().unwrap())
    .args(["--exact", EDIT_CHILD_HELPER, "--ignored", "--test-threads=1"])
    .env("ATLAS_EDIT_CRASH_CHILD_TREE", &tree)
    .env("ATLAS_EDIT_CRASH_CHILD_HOME", &home)
    .status()
    .unwrap();
  assert!(!status.success(), "the child must abort in the middle of its edit refresh");

  let reopened = open(&project, &home);
  let runs = reopened.store().read(|tx| tx.runs(10)).unwrap();
  let edit = runs.iter().find(|run| run.trigger == "edit").expect("the child must have begun an edit run");
  assert_eq!(edit.status, RunStatus::Failed { error: "interrupted".to_owned() });
  assert!(fresh(&reopened).header().run.is_some());
  assert_eq!(named(&reopened, &format!("bulk{}", EDIT_BULK_FILES - 1)).len(), 1);
  let diff = mismatch(&project, &reopened, scratch.path(), "edit-crash");
  assert!(diff.is_none(), "{}", diff.unwrap_or_default());
}

const LOAD_RENAMED: &str = "export function other(n: number) {\n  return n * 2;\n}\nexport function load(n: number) {\n  return n;\n}\n";

struct RacingScip {
  output: PathBuf,
  during: Box<dyn Fn() + Send + Sync>,
}

impl ScipIndexer for RacingScip {
  fn id(&self) -> &'static str {
    "racing-scip"
  }

  fn availability(&self) -> Availability {
    Availability::Available
  }

  fn index(&self, _request: &ScipRequest) -> ScipRun {
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
    fs::write(&self.output, index.write_to_bytes().unwrap()).unwrap();
    (self.during)();
    ScipRun {
      indexer: "racing-scip".to_owned(),
      version: Some("1".to_owned()),
      duration: Duration::ZERO,
      output_path: Some(self.output.clone()),
      outcome: ScipOutcome::Ok,
      cache: CacheStatus::Miss,
      degradations: Vec::new(),
      warnings: Vec::new(),
    }
  }
}

#[test]
fn should_not_map_an_index_onto_files_another_process_reindexed_meanwhile() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("demo");
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(root.join("package.json"), "{ \"name\": \"demo\" }\n").unwrap();
  fs::write(root.join("src/a.ts"), "export function load(n: number) {\n  return n;\n}\n").unwrap();
  fs::write(root.join("src/b.ts"), "import { load } from './a';\nexport function run() {\n  return load(1);\n}\n").unwrap();
  let project = project("demo", vec![repo_config("demo", &root)]);
  let home = scratch.path().join("home");
  let scip_engine = open(&project, &home);
  fresh(&scip_engine);
  let other = Arc::new(open(&project, &home));
  fresh(&other);
  let during = {
    let other = Arc::clone(&other);
    let root = root.clone();
    move || {
      fs::write(root.join("src/a.ts"), LOAD_RENAMED).unwrap();
      assert!(other.ensure_fresh().unwrap().header().run.is_some());
    }
  };
  let indexer = RacingScip {
    output: scratch.path().join("racing.scip"),
    during: Box::new(during),
  };

  let report = scip_engine.scip(&ScipIndexers { cargo: None, npm: Some(&indexer) }).unwrap();

  let IngestCounts { files, skipped } = ingested(&report);
  assert_eq!((files, skipped), (1, 1), "{report:?}");
  let owner = |path: &str| Owner::Project {
    pass: ScipFacts::owner_pass(&RepoName::new("demo").unwrap(), &RelPath::new(path).unwrap()),
  };
  assert!(scip_engine.store().read(|tx| tx.owned_facts(&owner("src/a.ts"))).unwrap().is_empty());
  let other_fn = named(&scip_engine, "other").remove(0);
  let b_edges = scip_engine.store().read(|tx| tx.owned_facts(&owner("src/b.ts"))).unwrap().edges;
  assert!(
    b_edges.iter().all(|edge| edge.to != other_fn.id),
    "an old index range was mapped onto the new file: {b_edges:#?}"
  );
  assert_eq!(fresh(&scip_engine).header().scip, ScipLag::Behind { files: 1 });
}

struct IngestCounts {
  files: u64,
  skipped: u64,
}

fn ingested(report: &atlas_engine::ScipReport) -> IngestCounts {
  match &report.packages[0].outcome {
    atlas_engine::ScipPackageOutcome::Ingested { files, skipped_changed, .. } => IngestCounts {
      files: *files,
      skipped: *skipped_changed,
    },
    other => panic!("expected an ingested package, got {other:?}"),
  }
}

struct HookedScip {
  inner: FakeScip,
  during: Box<dyn Fn() + Send + Sync>,
}

impl ScipIndexer for HookedScip {
  fn id(&self) -> &'static str {
    "hooked-scip"
  }

  fn availability(&self) -> Availability {
    Availability::Available
  }

  fn index(&self, request: &ScipRequest) -> ScipRun {
    let run = self.inner.index(request);
    (self.during)();
    run
  }
}

fn paths_tsconfig(dir: &str) -> String {
  format!("{{ \"compilerOptions\": {{ \"baseUrl\": \".\", \"paths\": {{ \"@lib/*\": [\"src/{dir}/*\"] }} }} }}\n")
}

fn paths_tree(scratch: &Path) -> (PathBuf, Project) {
  let root = scratch.join("demo");
  fs::create_dir_all(root.join("src/a")).unwrap();
  fs::create_dir_all(root.join("src/c")).unwrap();
  fs::write(root.join("package.json"), "{ \"name\": \"demo\" }\n").unwrap();
  fs::write(root.join("tsconfig.json"), paths_tsconfig("a")).unwrap();
  let load = "export function load(n: number) {\n  return n;\n}\n";
  fs::write(root.join("src/a/x.ts"), load).unwrap();
  fs::write(root.join("src/c/x.ts"), load).unwrap();
  fs::write(root.join("src/b.ts"), "import { load } from '@lib/x';\nexport function run() {\n  return load(1);\n}\n").unwrap();
  let project = project("demo", vec![repo_config("demo", &root)]);
  (root, project)
}

fn assert_package_left_lagging(engine: &Engine, report: &atlas_engine::ScipReport) {
  let IngestCounts { files, skipped } = ingested(report);
  assert_eq!((files, skipped), (0, 3), "{report:?}");
  let owner = Owner::Project {
    pass: ScipFacts::owner_pass(&RepoName::new("demo").unwrap(), &RelPath::new("src/b.ts").unwrap()),
  };
  let kept = engine.store().read(|tx| tx.owned_facts(&owner)).unwrap();
  assert!(
    kept.edges.is_empty(),
    "a compiler-verified edge was written after a tsconfig change during indexing: {:#?}",
    kept.edges
  );
  assert_eq!(fresh(engine).header().scip, ScipLag::Behind { files: 3 });
}

#[test]
fn should_not_ingest_a_package_whose_tsconfig_changed_while_the_indexer_ran() {
  let scratch = tempfile::tempdir().unwrap();
  let (root, project) = paths_tree(scratch.path());
  let engine = Arc::new(open(&project, &scratch.path().join("home")));
  fresh(&engine);
  let during = {
    let engine = Arc::clone(&engine);
    move || {
      fs::write(root.join("tsconfig.json"), paths_tsconfig("c")).unwrap();
      assert!(engine.ensure_fresh().unwrap().header().run.is_some());
    }
  };
  let indexer = HookedScip {
    inner: FakeScip {
      output: scratch.path().join("index.scip"),
    },
    during: Box::new(during),
  };

  let report = engine.scip(&ScipIndexers { cargo: None, npm: Some(&indexer) }).unwrap();

  assert_package_left_lagging(&engine, &report);
}

#[test]
fn should_not_ingest_a_package_whose_tsconfig_another_process_changed_while_the_indexer_ran() {
  let scratch = tempfile::tempdir().unwrap();
  let (root, project) = paths_tree(scratch.path());
  let home = scratch.path().join("home");
  let scip_engine = open(&project, &home);
  fresh(&scip_engine);
  let other = Arc::new(open(&project, &home));
  fresh(&other);
  let during = {
    let other = Arc::clone(&other);
    move || {
      fs::write(root.join("tsconfig.json"), paths_tsconfig("c")).unwrap();
      assert!(other.ensure_fresh().unwrap().header().run.is_some());
    }
  };
  let indexer = HookedScip {
    inner: FakeScip {
      output: scratch.path().join("index.scip"),
    },
    during: Box::new(during),
  };

  let report = scip_engine.scip(&ScipIndexers { cargo: None, npm: Some(&indexer) }).unwrap();

  assert_package_left_lagging(&scip_engine, &report);
}

#[test]
fn should_report_scip_current_after_a_lagging_file_is_deleted() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("demo");
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(root.join("package.json"), "{ \"name\": \"demo\" }\n").unwrap();
  fs::write(root.join("src/a.ts"), "export function load(n: number) {\n  return n;\n}\n").unwrap();
  fs::write(root.join("src/b.ts"), "import { load } from './a';\nexport function run() {\n  return load(1);\n}\n").unwrap();
  let project = project("demo", vec![repo_config("demo", &root)]);
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let indexer = RacingScip {
    output: scratch.path().join("racing.scip"),
    during: Box::new(|| {}),
  };
  engine.scip(&ScipIndexers { cargo: None, npm: Some(&indexer) }).unwrap();
  assert_eq!(fresh(&engine).header().scip, ScipLag::Current);

  fs::write(root.join("src/b.ts"), "import { load } from './a';\nexport function run() {\n  return load(2);\n}\n").unwrap();
  assert_eq!(fresh(&engine).header().scip, ScipLag::Behind { files: 1 });
  fs::remove_file(root.join("src/b.ts")).unwrap();
  assert_eq!(fresh(&engine).header().scip, ScipLag::Behind { files: 1 });

  engine.scip(&ScipIndexers { cargo: None, npm: Some(&indexer) }).unwrap();
  assert_eq!(fresh(&engine).header().scip, ScipLag::Current);
}

#[test]
fn should_prune_lagging_entries_without_a_file_record_when_ingesting() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("demo");
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(root.join("package.json"), "{ \"name\": \"demo\" }\n").unwrap();
  fs::write(root.join("src/a.ts"), "export function load(n: number) {\n  return n;\n}\n").unwrap();
  fs::write(root.join("src/b.ts"), "import { load } from './a';\nexport function run() {\n  return load(1);\n}\n").unwrap();
  let project = project("demo", vec![repo_config("demo", &root)]);
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let indexer = RacingScip {
    output: scratch.path().join("racing.scip"),
    during: Box::new(|| {}),
  };
  engine.scip(&ScipIndexers { cargo: None, npm: Some(&indexer) }).unwrap();
  engine
    .store()
    .write(|tx| -> Result<(), atlas_store::StoreError> {
      tx.put_blob("engine:scip", 1, br#"{"last_run_ms":1,"lagging":["demo:src/gone.ts"]}"#)?;
      Ok(())
    })
    .unwrap();
  drop(engine);
  let reopened = open(&project, &scratch.path().join("home"));
  assert_eq!(fresh(&reopened).header().scip, ScipLag::Behind { files: 1 });

  reopened.scip(&ScipIndexers { cargo: None, npm: Some(&indexer) }).unwrap();

  assert_eq!(fresh(&reopened).header().scip, ScipLag::Current);
}

#[test]
fn should_skip_a_file_re_resolved_during_the_first_ever_scip_run() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("demo");
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(root.join("package.json"), "{ \"name\": \"demo\" }\n").unwrap();
  fs::write(root.join("src/a.ts"), "export function load(n: number) {\n  return n;\n}\n").unwrap();
  fs::write(root.join("src/b.ts"), "import { load } from './a';\nexport function run() {\n  return load(1);\n}\n").unwrap();
  let project = project("demo", vec![repo_config("demo", &root)]);
  let engine = Arc::new(open(&project, &scratch.path().join("home")));
  fresh(&engine);
  let during = {
    let engine = Arc::clone(&engine);
    let root = root.clone();
    move || {
      fs::write(root.join("src/a.ts"), "export function loadAll(n: number) {\n  return n;\n}\n").unwrap();
      assert!(engine.ensure_fresh().unwrap().header().run.is_some());
    }
  };
  let indexer = RacingScip {
    output: scratch.path().join("racing.scip"),
    during: Box::new(during),
  };

  let report = engine.scip(&ScipIndexers { cargo: None, npm: Some(&indexer) }).unwrap();

  let IngestCounts { files, skipped } = ingested(&report);
  assert_eq!((files, skipped), (0, 2), "{report:?}");
  assert_eq!(fresh(&engine).header().scip, ScipLag::Behind { files: 2 });
}

struct HookedRustScip {
  output: PathBuf,
  during: Box<dyn Fn() + Send + Sync>,
}

impl ScipIndexer for HookedRustScip {
  fn id(&self) -> &'static str {
    "hooked-rust-scip"
  }

  fn availability(&self) -> Availability {
    Availability::Available
  }

  fn index(&self, _request: &ScipRequest) -> ScipRun {
    let load = "rust-analyzer cargo demo 0.1.0 load().";
    let run = "rust-analyzer cargo demo 0.1.0 run().";
    let mut tool = ToolInfo::new();
    tool.name = "rust-analyzer".to_owned();
    let mut metadata = Metadata::new();
    metadata.tool_info = Some(tool).into();
    let mut index = Index::new();
    index.metadata = Some(metadata).into();
    let mut lib = document(
      "src/lib.rs",
      vec![
        occurrence(load, vec![0, 7, 11], true),
        occurrence(run, vec![4, 7, 10], true),
        occurrence(load, vec![5, 2, 6], false),
      ],
    );
    lib.position_encoding = EnumOrUnknown::new(PositionEncoding::UTF8CodeUnitOffsetFromLineStart);
    index.documents = vec![lib];
    fs::write(&self.output, index.write_to_bytes().unwrap()).unwrap();
    (self.during)();
    ScipRun {
      indexer: "hooked-rust-scip".to_owned(),
      version: Some("1".to_owned()),
      duration: Duration::ZERO,
      output_path: Some(self.output.clone()),
      outcome: ScipOutcome::Ok,
      cache: CacheStatus::Miss,
      degradations: Vec::new(),
      warnings: Vec::new(),
    }
  }
}

#[test]
fn should_not_ingest_while_another_process_crashed_mid_run_during_the_indexer() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("demo");
  fs::create_dir_all(root.join("src")).unwrap();
  let manifest = |dependencies: &str| format!("[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dependencies}");
  fs::write(root.join("Cargo.toml"), manifest("")).unwrap();
  fs::write(root.join("src/lib.rs"), "pub fn load(n: i32) -> i32 {\n  n\n}\n\npub fn run() -> i32 {\n  load(1)\n}\n").unwrap();
  let project = project("demo", vec![repo_config("demo", &root)]);
  let home = scratch.path().join("home");
  let engine = open(&project, &home);
  fresh(&engine);
  let armed = Arc::new(AtomicBool::new(false));
  let other = Arc::new(open_with(
    &project,
    &home,
    EngineOptions {
      fault_injection: FaultInjection::crash_after_persist(Arc::clone(&armed)),
      ..EngineOptions::default()
    },
  ));
  fresh(&other);
  let during = {
    let other = Arc::clone(&other);
    let armed = Arc::clone(&armed);
    let root = root.clone();
    move || {
      fs::write(root.join("Cargo.toml"), manifest("serde = \"1\"\n")).unwrap();
      armed.store(true, Ordering::SeqCst);
      other.ensure_fresh().unwrap();
      armed.store(false, Ordering::SeqCst);
    }
  };
  let indexer = HookedRustScip {
    output: scratch.path().join("index.scip"),
    during: Box::new(during),
  };

  engine.scip(&ScipIndexers { cargo: Some(&indexer), npm: None }).unwrap();

  let owner = Owner::Project {
    pass: ScipFacts::owner_pass(&RepoName::new("demo").unwrap(), &RelPath::new("src/lib.rs").unwrap()),
  };
  let edges = engine.store().read(|tx| tx.owned_facts(&owner)).unwrap().edges;
  assert!(edges.is_empty(), "stale edges after ingest: {edges:#?}");
  fresh(&engine);
  assert!(engine.store().read(|tx| tx.owned_facts(&owner)).unwrap().edges.is_empty());
}

#[test]
fn should_re_resolve_dependents_directly_when_a_repo_is_removed() {
  for removed in ["acme-sdk", "acme-web", "acme-api"] {
    let scratch = tempfile::tempdir().unwrap();
    let tree = scratch.path().join("tree");
    let project = acme_copy(&tree);
    let home = scratch.path().join("home");
    let engine = open(&project, &home);
    fresh(&engine);
    drop(engine);
    let mut fewer = project.clone();
    fewer.repos.retain(|repo| repo.name.as_str() != removed);
    let reopened = open(&fewer, &home);

    let summary = reopened.index(IndexMode::Incremental).unwrap();

    assert_eq!(
      summary.counts.get("safety_net_owners"),
      None,
      "removing {removed} relied on the safety net: {:?}",
      summary.counts
    );
    assert!(
      mismatch(&fewer, &reopened, scratch.path(), removed).is_none(),
      "removing {removed} diverged from a full index"
    );
  }
}
