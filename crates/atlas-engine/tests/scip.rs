mod common;

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use atlas_core::{Availability, Certainty, EdgeKind, Owner, RelPath, RepoName};
use atlas_engine::{ScipIndexers, ScipLag, ScipPackageOutcome};
use atlas_scip::{CacheStatus, ScipFacts, ScipIndexer, ScipOutcome, ScipRequest, ScipRun};
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
