mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use atlas_core::{AnalysisInput, Availability, Certainty, ContentHash, EdgeKind, EntityId, FileRef, KeyContext, LanguageAnalyzer, RelPath, RepoName};
use atlas_lang_rust::RustAnalyzer;
use atlas_scip::{CacheStatus, RustAnalyzerScip, ScipFileContext, ScipIndexer, ScipIngestContext, ScipInputFile, ScipOutcome, ScipRequest, ingest};
use common::rel;

const FIXTURE: &str = "../../fixtures/adversarial/trait-dispatch-rust";
const REPO: &str = "trait-dispatch-rust";

fn copy_dir(from: &Path, to: &Path) {
  fs::create_dir_all(to).unwrap();
  for entry in fs::read_dir(from).unwrap() {
    let entry = entry.unwrap();
    let target = to.join(entry.file_name());
    match entry.file_type().unwrap().is_dir() {
      true => copy_dir(&entry.path(), &target),
      false => {
        fs::copy(entry.path(), target).unwrap();
      }
    }
  }
}

fn item_path(display: &str) -> Vec<String> {
  let Some(inner) = display.strip_prefix('<') else {
    return display.split("::").map(str::to_owned).collect();
  };
  let (qualified, method) = inner.split_once(">::").unwrap();
  let (self_type, trait_name) = qualified.split_once(" as ").unwrap();
  vec![self_type.to_owned(), format!("<{trait_name}>"), method.to_owned()]
}

struct Analyzed {
  context: ScipIngestContext,
  request: ScipRequest,
  _work: tempfile::TempDir,
}

impl Analyzed {
  fn entity(&self, display: &str) -> EntityId {
    let wanted = item_path(display);
    self
      .context
      .files
      .values()
      .flat_map(|file| &file.symbols)
      .find(|symbol| symbol.key.item_path() == wanted)
      .unwrap_or_else(|| panic!("no symbol {display}"))
      .entity
  }
}

fn analyze_fixture() -> Analyzed {
  let work = tempfile::tempdir().unwrap();
  let fixture = work.path().join(REPO);
  copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE), &fixture);
  let source = fs::read_to_string(fixture.join("src/lib.rs")).unwrap();
  let repo = RepoName::new(REPO).unwrap();
  let file = FileRef::detect(repo.clone(), rel("src/lib.rs"), ContentHash::of(source.as_bytes())).unwrap();
  let facts = RustAnalyzer::new().analyze(&AnalysisInput { file: &file, source: &source }).unwrap();
  let keys = facts
    .symbol_keys(&KeyContext {
      package: REPO.to_owned(),
      module_path: vec![],
    })
    .unwrap();
  let context = ScipIngestContext::new(repo).with_file(rel("src/lib.rs"), ScipFileContext::from_facts(&facts, keys, source));
  let request = ScipRequest {
    root: fixture,
    cache_dir: work.path().join("cache"),
    files: vec![ScipInputFile {
      path: rel("src/lib.rs"),
      content_hash: file.content_hash,
    }],
  };
  Analyzed { context, request, _work: work }
}

fn expected() -> toml::Table {
  toml::from_str(&fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE).join("expected.toml")).unwrap()).unwrap()
}

fn endpoint(table: &toml::Table, side: &str, analyzed: &Analyzed) -> EntityId {
  analyzed.entity(table[side].as_table().unwrap()["symbol"].as_str().unwrap())
}

fn certainties(table: &toml::Table) -> Vec<&str> {
  table
    .get("certainty")
    .and_then(|value| value.as_array())
    .map(|values| values.iter().filter_map(|value| value.as_str()).collect())
    .unwrap_or_default()
}

#[test]
fn should_verify_the_trait_dispatch_fixture_edges_with_real_rust_analyzer() {
  let indexer = RustAnalyzerScip::new();
  if let Availability::Unavailable { reason } = indexer.availability() {
    eprintln!("SKIPPED should_verify_the_trait_dispatch_fixture_edges_with_real_rust_analyzer: rust-analyzer unavailable ({reason})");
    return;
  }
  let analyzed = analyze_fixture();
  let run = indexer.index(&analyzed.request);
  assert_eq!(run.outcome, ScipOutcome::Ok, "{run:?}");
  assert_eq!(run.cache, CacheStatus::Miss);
  assert!(run.version.as_deref().is_some_and(|version| version.starts_with("rust-analyzer")));
  let bytes = fs::read(run.output_path.as_ref().unwrap()).unwrap();
  let root = &analyzed.request.root;
  assert!(!root.join("target").exists(), "rust-analyzer wrote target/ into the repo");
  assert!(!root.join("Cargo.lock").exists(), "Cargo.lock was left behind in the repo");
  assert!(analyzed.request.cache_dir.join("rust-target").is_dir());

  let scip = ingest(&bytes, &analyzed.context).unwrap();
  let repo = RepoName::new(REPO).unwrap();
  let edges = &scip.per_file[&(repo, RelPath::new("src/lib.rs").unwrap())];
  assert!(edges.iter().all(|edge| edge.certainty == Certainty::CompilerVerified));
  assert!(edges.iter().all(|edge| !edge.evidence.is_empty()));
  assert!(scip.warnings.is_empty(), "{:?}", scip.warnings);
  assert_eq!(scip.stats.documents, 1);
  assert_eq!(scip.stats.references_unmapped, 5, "unexpected analyzer/scip disagreement: {:?}", scip.stats);
  assert_eq!(scip.stats.relationships_mapped, 2);

  let expected = expected();
  let mut asserted = 0;
  for edge in expected["edge"].as_array().unwrap().iter().map(|edge| edge.as_table().unwrap()) {
    let kind = edge["kind"].as_str().unwrap();
    let authoritative = certainties(edge).contains(&"resolved");
    if !authoritative || !["calls", "implements"].contains(&kind) {
      continue;
    }
    let (from, to) = (endpoint(edge, "from", &analyzed), endpoint(edge, "to", &analyzed));
    let kind: EdgeKind = kind.parse().unwrap();
    assert!(
      edges.iter().any(|candidate| candidate.kind == kind && candidate.from == from && candidate.to == to),
      "missing {kind} {edge:?}"
    );
    asserted += 1;
  }
  assert_eq!(asserted, 8);

  for forbidden in expected["forbidden_edge"].as_array().unwrap().iter().map(|edge| edge.as_table().unwrap()) {
    let (from, to) = (endpoint(forbidden, "from", &analyzed), endpoint(forbidden, "to", &analyzed));
    assert!(
      !edges.iter().any(|candidate| candidate.from == from && candidate.to == to),
      "forbidden edge present: {forbidden:?}"
    );
  }

  let implements: BTreeMap<EntityId, EntityId> = edges.iter().filter(|edge| edge.kind == EdgeKind::Implements).map(|edge| (edge.from, edge.to)).collect();
  assert_eq!(implements.len(), 2);
  assert_eq!(implements[&analyzed.entity("MemStore")], analyzed.entity("Store"));
  assert_eq!(implements[&analyzed.entity("DiskStore")], analyzed.entity("Store"));

  let rerun = indexer.index(&analyzed.request);
  assert_eq!(rerun.outcome, ScipOutcome::Ok);
  assert_eq!(rerun.cache, CacheStatus::Hit);
  assert_eq!(rerun.output_path, run.output_path);
  assert_eq!(rerun.degradations, run.degradations);
  assert!(rerun.duration < Duration::from_secs(1));
}

#[test]
fn should_report_unavailable_when_the_binary_does_not_exist() {
  let indexer = RustAnalyzerScip::new().with_binary("/nonexistent/rust-analyzer".into());
  assert!(matches!(indexer.availability(), Availability::Unavailable { .. }));
  let work = tempfile::tempdir().unwrap();
  let run = indexer.index(&ScipRequest {
    root: work.path().to_path_buf(),
    cache_dir: work.path().join("cache"),
    files: vec![],
  });
  assert!(matches!(run.outcome, ScipOutcome::Unavailable { .. }), "{run:?}");
  assert_eq!(run.output_path, None);
}

#[test]
fn should_fail_with_stderr_when_the_indexer_exits_non_zero() {
  let work = tempfile::tempdir().unwrap();
  let script = work.path().join("fake-ra");
  fs::write(
    &script,
    "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'fake-ra 0.0.1'; exit 0; fi\necho 'boom: no cargo workspace' >&2\nexit 7\n",
  )
  .unwrap();
  fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
  let indexer = RustAnalyzerScip::new().with_binary(script);
  let run = indexer.index(&ScipRequest {
    root: work.path().to_path_buf(),
    cache_dir: work.path().join("cache"),
    files: vec![],
  });
  let ScipOutcome::Failed { reason, stderr_tail } = &run.outcome else {
    panic!("expected failure, got {run:?}")
  };
  assert!(reason.contains("status 7"), "{reason}");
  assert!(stderr_tail.contains("boom: no cargo workspace"));
  assert_eq!(run.version.as_deref(), Some("fake-ra 0.0.1"));
  assert_eq!(fs::read_dir(work.path().join("cache")).unwrap().count(), 0);
}

#[test]
fn should_time_out_and_leave_no_cache_entry_or_work_directory() {
  let work = tempfile::tempdir().unwrap();
  let script = work.path().join("slow-ra");
  fs::write(&script, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'slow-ra 0.0.1'; exit 0; fi\nsleep 60\n").unwrap();
  fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
  let indexer = RustAnalyzerScip::new().with_binary(script).with_timeout(Duration::from_millis(400));
  let run = indexer.index(&ScipRequest {
    root: work.path().to_path_buf(),
    cache_dir: work.path().join("cache"),
    files: vec![],
  });
  assert_eq!(run.outcome, ScipOutcome::TimedOut);
  assert!(run.duration < Duration::from_secs(10));
  assert_eq!(fs::read_dir(work.path().join("cache")).unwrap().count(), 0);
}

fn fake_binary(dir: &Path, body: &str) -> std::path::PathBuf {
  let script = dir.join("fake-ra");
  fs::write(
    &script,
    format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'fake-ra 0.0.1'; exit 0; fi\n{body}\n"),
  )
  .unwrap();
  fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
  script
}

#[test]
fn should_delete_a_lockfile_the_indexer_created_and_warn() {
  let work = tempfile::tempdir().unwrap();
  let root = work.path().join("repo");
  fs::create_dir_all(&root).unwrap();
  let binary = fake_binary(work.path(), "echo locked > \"$2/Cargo.lock\"\nprintf index > \"$4\"");
  let run = RustAnalyzerScip::new().with_binary(binary).index(&ScipRequest {
    root: root.clone(),
    cache_dir: work.path().join("cache"),
    files: vec![],
  });
  assert_eq!(run.outcome, ScipOutcome::Ok, "{run:?}");
  assert!(!root.join("Cargo.lock").exists());
  assert_eq!(run.warnings.len(), 1);
  assert!(run.warnings[0].contains("Cargo.lock") && run.warnings[0].contains("restored"), "{:?}", run.warnings);
}

#[test]
fn should_restore_the_original_bytes_of_a_lockfile_the_indexer_rewrote() {
  let work = tempfile::tempdir().unwrap();
  let root = work.path().join("repo");
  fs::create_dir_all(&root).unwrap();
  fs::write(root.join("Cargo.lock"), "original").unwrap();
  let binary = fake_binary(work.path(), "echo rewritten > \"$2/Cargo.lock\"\nprintf index > \"$4\"");
  let run = RustAnalyzerScip::new().with_binary(binary).index(&ScipRequest {
    root: root.clone(),
    cache_dir: work.path().join("cache"),
    files: vec![],
  });
  assert_eq!(run.outcome, ScipOutcome::Ok, "{run:?}");
  assert_eq!(fs::read_to_string(root.join("Cargo.lock")).unwrap(), "original");
  assert_eq!(run.warnings.len(), 1);
}

#[test]
fn should_restore_the_lockfile_even_when_the_indexer_times_out() {
  let work = tempfile::tempdir().unwrap();
  let root = work.path().join("repo");
  fs::create_dir_all(&root).unwrap();
  let binary = fake_binary(work.path(), "echo locked > \"$2/Cargo.lock\"\nsleep 60");
  let indexer = RustAnalyzerScip::new().with_binary(binary).with_timeout(Duration::from_millis(400));
  let run = indexer.index(&ScipRequest {
    root: root.clone(),
    cache_dir: work.path().join("cache"),
    files: vec![],
  });
  assert_eq!(run.outcome, ScipOutcome::TimedOut);
  assert!(!root.join("Cargo.lock").exists());
  assert_eq!(run.warnings.len(), 1);
}

#[test]
fn should_rebuild_when_a_manifest_changes_and_reuse_recorded_degradations_otherwise() {
  let work = tempfile::tempdir().unwrap();
  let root = work.path().join("repo");
  fs::create_dir_all(&root).unwrap();
  fs::write(root.join("Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
  let binary = fake_binary(work.path(), "printf index > \"$4\"");
  let indexer = RustAnalyzerScip::new().with_binary(binary);
  let request = ScipRequest {
    root: root.clone(),
    cache_dir: work.path().join("cache"),
    files: vec![],
  };
  let first = indexer.index(&request);
  assert_eq!(indexer.index(&request).cache, CacheStatus::Hit);
  fs::write(root.join("Cargo.toml"), "[package]\nname = \"b\"\n").unwrap();
  let second = indexer.index(&request);
  assert_eq!(second.cache, CacheStatus::Miss);
  assert_ne!(first.output_path, second.output_path);
}
