mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use atlas_core::{Availability, Certainty, EdgeKind, Owner, RelPath, RepoName};
use atlas_engine::{Engine, ScipIndexers, ScipLag, ScipPackageOutcome};
use atlas_scip::{CacheStatus, RustAnalyzerScip, ScipFacts, ScipIndexer, ScipOutcome, ScipRequest, ScipRun};
use common::{fresh, open, project, repo_config};
use protobuf::{EnumOrUnknown, Message};
use scip::types::{Document, Index, Metadata, Occurrence, PositionEncoding, SymbolRole, ToolInfo};

const SOURCE: &str = "export function load(n: number) {\n  return n;\n}\nexport function run() {\n  return load(1);\n}\n";

struct FakeScip {
  output: PathBuf,
  outcome: ScipOutcome,
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
      outcome: self.outcome.clone(),
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

fn index_bytes() -> Vec<u8> {
  let load = "scip-typescript npm demo 1.0.0 src/a.ts`load().";
  let run = "scip-typescript npm demo 1.0.0 src/a.ts`run().";
  let mut document = Document::new();
  document.relative_path = "src/a.ts".to_owned();
  document.position_encoding = EnumOrUnknown::new(PositionEncoding::UTF16CodeUnitOffsetFromLineStart);
  document.occurrences = vec![
    occurrence(load, vec![0, 16, 20], true),
    occurrence(run, vec![3, 16, 19], true),
    occurrence(load, vec![4, 9, 13], false),
  ];
  let mut tool = ToolInfo::new();
  tool.name = "scip-typescript".to_owned();
  let mut metadata = Metadata::new();
  metadata.tool_info = Some(tool).into();
  let mut index = Index::new();
  index.metadata = Some(metadata).into();
  index.documents = vec![document];
  index.write_to_bytes().unwrap()
}

#[test]
fn should_ingest_compiler_verified_edges_and_drop_them_when_the_file_changes() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("demo");
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(root.join("package.json"), "{ \"name\": \"demo\" }\n").unwrap();
  fs::write(root.join("src/a.ts"), SOURCE).unwrap();
  let project = project("demo", vec![repo_config("demo", &root)]);
  let engine = open(&project, &scratch.path().join("home"));
  assert_eq!(fresh(&engine).header().scip, ScipLag::NeverRun);
  let fake = FakeScip {
    output: scratch.path().join("index.scip"),
    outcome: ScipOutcome::Ok,
  };

  let report = engine.scip(&ScipIndexers { cargo: None, npm: Some(&fake) }).unwrap();

  assert_eq!(report.packages.len(), 1);
  let ScipPackageOutcome::Ingested {
    files, edges, skipped_changed, ..
  } = &report.packages[0].outcome
  else {
    panic!("{report:?}")
  };
  assert_eq!((*files, *edges, *skipped_changed), (1, 1, 0));
  let owner = Owner::Project {
    pass: ScipFacts::owner_pass(&RepoName::new("demo").unwrap(), &RelPath::new("src/a.ts").unwrap()),
  };
  let facts = engine.store().read(|tx| tx.owned_facts(&owner)).unwrap();
  assert_eq!(facts.edges.len(), 1);
  assert_eq!(facts.edges[0].kind, EdgeKind::Calls);
  assert_eq!(facts.edges[0].certainty, Certainty::CompilerVerified);
  assert_eq!(fresh(&engine).header().scip, ScipLag::Current);
  assert_eq!(engine.status().unwrap().scip.files_with_scip, 1);

  fs::write(root.join("src/a.ts"), format!("{SOURCE}export const later = 1;\n")).unwrap();
  let after = fresh(&engine);

  assert!(after.header().run.is_some());
  assert_eq!(after.header().scip, ScipLag::Behind { files: 1 });
  assert!(engine.store().read(|tx| tx.owned_facts(&owner)).unwrap().is_empty());
}

#[test]
fn should_report_unavailable_indexers_without_touching_the_index() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("demo");
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(root.join("package.json"), "{ \"name\": \"demo\" }\n").unwrap();
  fs::write(root.join("src/a.ts"), SOURCE).unwrap();
  let project = project("demo", vec![repo_config("demo", &root)]);
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let before = common::digest(&engine);
  let fake = FakeScip {
    output: scratch.path().join("index.scip"),
    outcome: ScipOutcome::Unavailable { reason: "no npx".to_owned() },
  };

  let report = engine.scip(&ScipIndexers { cargo: None, npm: Some(&fake) }).unwrap();

  assert_eq!(report.packages[0].outcome, ScipPackageOutcome::IndexerUnavailable { reason: "no npx".to_owned() });
  assert_eq!(common::digest(&engine), before);
  assert_eq!(fresh(&engine).header().scip, ScipLag::NeverRun);
}

const CALLER: &str = "pub fn caller() -> u32 {\n  b::callee()\n}\n";
const CALLEE: &str = "pub fn callee() -> u32 {\n  1\n}\n";
const STANDALONE: &str = "pub fn alone() -> u32 {\n  2\n}\n";

fn write(root: &Path, path: &str, content: &str) {
  let full = root.join(path);
  fs::create_dir_all(full.parent().unwrap()).unwrap();
  fs::write(full, content).unwrap();
}

fn cargo_workspace(root: &Path) {
  write(root, "Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]\nexclude = [\"c\"]\nresolver = \"2\"\n");
  write(
    root,
    "a/Cargo.toml",
    "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nb = { path = \"../b\" }\n",
  );
  write(root, "a/src/lib.rs", CALLER);
  write(root, "b/Cargo.toml", "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
  write(root, "b/src/lib.rs", CALLEE);
  write(root, "c/Cargo.toml", "[package]\nname = \"c\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
  write(root, "c/src/lib.rs", STANDALONE);
}

struct RecordingCargoScip {
  output: PathBuf,
  requests: Mutex<Vec<(PathBuf, Vec<String>)>>,
}

fn rust_index_bytes() -> Vec<u8> {
  let caller = "rust-analyzer cargo a 0.1.0 caller().";
  let callee = "rust-analyzer cargo b 0.1.0 callee().";
  let utf8 = |path: &str, occurrences: Vec<Occurrence>| {
    let mut document = Document::new();
    document.relative_path = path.to_owned();
    document.position_encoding = EnumOrUnknown::new(PositionEncoding::UTF8CodeUnitOffsetFromLineStart);
    document.occurrences = occurrences;
    document
  };
  let mut tool = ToolInfo::new();
  tool.name = "rust-analyzer".to_owned();
  let mut metadata = Metadata::new();
  metadata.tool_info = Some(tool).into();
  let mut index = Index::new();
  index.metadata = Some(metadata).into();
  index.documents = vec![
    utf8("a/src/lib.rs", vec![occurrence(caller, vec![0, 7, 13], true), occurrence(callee, vec![1, 5, 11], false)]),
    utf8("b/src/lib.rs", vec![occurrence(callee, vec![0, 7, 13], true)]),
  ];
  index.write_to_bytes().unwrap()
}

impl ScipIndexer for RecordingCargoScip {
  fn id(&self) -> &'static str {
    "recording-cargo-scip"
  }

  fn availability(&self) -> Availability {
    Availability::Available
  }

  fn index(&self, request: &ScipRequest) -> ScipRun {
    let mut files: Vec<String> = request.files.iter().map(|file| file.path.to_string()).collect();
    files.sort();
    self.requests.lock().unwrap().push((request.root.clone(), files));
    let bytes = match request.files.iter().any(|file| file.path.as_str() == "a/src/lib.rs") {
      true => rust_index_bytes(),
      false => Index::new().write_to_bytes().unwrap(),
    };
    fs::write(&self.output, bytes).unwrap();
    ScipRun {
      indexer: "recording-cargo-scip".to_owned(),
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

fn symbol_id(engine: &Engine, path: &str, name: &str) -> atlas_core::EntityId {
  let owner = Owner::File {
    repo: RepoName::new("ws").unwrap(),
    path: RelPath::new(path).unwrap(),
  };
  let facts = engine.store().read(|tx| tx.owned_facts(&owner)).unwrap();
  facts.entities.iter().find(|entity| entity.name == name).unwrap_or_else(|| panic!("no {name} in {path}")).id
}

fn scip_calls(engine: &Engine, path: &str) -> Vec<atlas_core::Edge> {
  let owner = Owner::Project {
    pass: ScipFacts::owner_pass(&RepoName::new("ws").unwrap(), &RelPath::new(path).unwrap()),
  };
  engine
    .store()
    .read(|tx| tx.owned_facts(&owner))
    .unwrap()
    .edges
    .into_iter()
    .filter(|edge| edge.kind == EdgeKind::Calls)
    .collect()
}

#[test]
fn should_index_a_cargo_workspace_once_at_its_root_and_standalone_crates_on_their_own() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("ws");
  cargo_workspace(&root);
  let project = project("ws", vec![repo_config("ws", &root)]);
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let indexer = RecordingCargoScip {
    output: scratch.path().join("index.scip"),
    requests: Mutex::new(Vec::new()),
  };

  let report = engine.scip(&ScipIndexers { cargo: Some(&indexer), npm: None }).unwrap();

  let manifests: Vec<&str> = report.packages.iter().map(|package| package.manifest.as_str()).collect();
  assert_eq!(manifests, vec!["Cargo.toml", "c/Cargo.toml"]);
  let requests = indexer.requests.lock().unwrap().clone();
  assert_eq!(
    requests,
    vec![
      (root.clone(), vec!["a/src/lib.rs".to_owned(), "b/src/lib.rs".to_owned()]),
      (root.join("c"), vec!["src/lib.rs".to_owned()]),
    ]
  );
  let calls = scip_calls(&engine, "a/src/lib.rs");
  assert_eq!(calls.len(), 1, "{calls:#?}");
  assert_eq!(calls[0].from, symbol_id(&engine, "a/src/lib.rs", "caller"));
  assert_eq!(calls[0].to, symbol_id(&engine, "b/src/lib.rs", "callee"));
  assert_eq!(calls[0].certainty, Certainty::CompilerVerified);
  assert_eq!(fresh(&engine).header().scip, ScipLag::Current);
}

#[test]
fn should_verify_a_cross_crate_call_in_a_cargo_workspace_with_real_rust_analyzer() {
  let indexer = RustAnalyzerScip::new();
  if let Availability::Unavailable { reason } = indexer.availability() {
    eprintln!("SKIPPED should_verify_a_cross_crate_call_in_a_cargo_workspace_with_real_rust_analyzer: rust-analyzer unavailable ({reason})");
    return;
  }
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("ws");
  cargo_workspace(&root);
  fs::remove_dir_all(root.join("c")).unwrap();
  write(&root, "Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]\nresolver = \"2\"\n");
  let project = project("ws", vec![repo_config("ws", &root)]);
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);

  let report = engine.scip(&ScipIndexers { cargo: Some(&indexer), npm: None }).unwrap();

  assert_eq!(report.packages.len(), 1, "{report:?}");
  assert!(matches!(report.packages[0].outcome, ScipPackageOutcome::Ingested { files: 2, .. }), "{report:?}");
  let calls = scip_calls(&engine, "a/src/lib.rs");
  let callee = symbol_id(&engine, "b/src/lib.rs", "callee");
  let cross = calls
    .iter()
    .find(|edge| edge.to == callee)
    .unwrap_or_else(|| panic!("no verified call into crate b: {calls:#?}"));
  assert_eq!(cross.from, symbol_id(&engine, "a/src/lib.rs", "caller"));
  assert_eq!(cross.certainty, Certainty::CompilerVerified);
  assert!(!root.join("target").exists());
}

struct BytesScip {
  output: PathBuf,
  bytes: Vec<u8>,
}

impl ScipIndexer for BytesScip {
  fn id(&self) -> &'static str {
    "bytes-scip"
  }

  fn availability(&self) -> Availability {
    Availability::Available
  }

  fn index(&self, _request: &ScipRequest) -> ScipRun {
    fs::write(&self.output, &self.bytes).unwrap();
    ScipRun {
      indexer: "bytes-scip".to_owned(),
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

fn ts_document(path: &str, occurrences: Vec<Occurrence>) -> Document {
  let mut document = Document::new();
  document.relative_path = path.to_owned();
  document.position_encoding = EnumOrUnknown::new(PositionEncoding::UTF16CodeUnitOffsetFromLineStart);
  document.occurrences = occurrences;
  document
}

fn dynamic_call_index_bytes() -> Vec<u8> {
  let load = "scip-typescript npm demo 1.0.0 src/y.ts`load().";
  let run = "scip-typescript npm demo 1.0.0 src/x.ts`run().";
  let mut tool = ToolInfo::new();
  tool.name = "scip-typescript".to_owned();
  let mut metadata = Metadata::new();
  metadata.tool_info = Some(tool).into();
  let mut index = Index::new();
  index.metadata = Some(metadata).into();
  index.documents = vec![
    ts_document("src/y.ts", vec![occurrence(load, vec![0, 16, 20], true)]),
    ts_document("src/x.ts", vec![occurrence(run, vec![0, 16, 19], true), occurrence(load, vec![1, 16, 23], false)]),
  ];
  index.write_to_bytes().unwrap()
}

#[test]
fn should_drop_scip_edges_into_an_entity_that_vanished_from_another_file() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("demo");
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(root.join("package.json"), "{ \"name\": \"demo\" }\n").unwrap();
  fs::write(root.join("src/y.ts"), "export function load(n: number) {\n  return n;\n}\n").unwrap();
  fs::write(root.join("src/x.ts"), "export function run(target: any) {\n  return target.compute(1);\n}\n").unwrap();
  let project = project("demo", vec![repo_config("demo", &root)]);
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  let demo = RepoName::new("demo").unwrap();
  let x = RelPath::new("src/x.ts").unwrap();
  let load = engine
    .store()
    .read(|tx| tx.entities_by_name("load", &atlas_store::EntityFilter::default()))
    .unwrap()
    .into_iter()
    .find(|entity| entity.path.as_ref().is_some_and(|path| path.as_str() == "src/y.ts"))
    .unwrap()
    .id;
  let x_owner = Owner::File {
    repo: demo.clone(),
    path: x.clone(),
  };
  let resolved = engine.store().read(|tx| tx.owned_facts(&x_owner)).unwrap();
  assert!(resolved.edges.iter().all(|edge| edge.to != load), "the resolver must not link the dynamic call itself");
  let indexer = BytesScip {
    output: scratch.path().join("index.scip"),
    bytes: dynamic_call_index_bytes(),
  };
  engine.scip(&ScipIndexers { cargo: None, npm: Some(&indexer) }).unwrap();
  let scip_owner = Owner::Project {
    pass: ScipFacts::owner_pass(&demo, &x),
  };
  let verified = engine.store().read(|tx| tx.owned_facts(&scip_owner)).unwrap();
  assert_eq!(
    verified.edges.iter().map(|edge| (edge.to, edge.certainty.clone())).collect::<Vec<_>>(),
    vec![(load, Certainty::CompilerVerified)]
  );

  fs::write(root.join("src/y.ts"), "export function fetchValue(n: number) {\n  return n;\n}\n").unwrap();
  let after = fresh(&engine);

  assert!(after.header().run.is_some());
  assert!(engine.store().read(|tx| tx.owned_facts(&scip_owner)).unwrap().is_empty());
  assert_eq!(engine.store().read(|tx| tx.edge_count_to(load)).unwrap(), 0);
  assert_eq!(after.header().scip, ScipLag::Behind { files: 2 });
  let reference = open(&project, &scratch.path().join("reference"));
  fresh(&reference);
  assert_eq!(common::digest(&engine), common::digest(&reference));
}
