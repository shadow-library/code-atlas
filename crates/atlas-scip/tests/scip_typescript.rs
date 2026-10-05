mod common;

use std::fs;
use std::path::Path;

use atlas_core::{AnalysisInput, Certainty, ContentHash, EdgeKind, EntityId, FileRef, KeyContext, LanguageAnalyzer, RepoName, ts_module_path};
use atlas_lang_ts::TsAnalyzer;
use atlas_scip::{Degradation, ScipFileContext, ScipIndexer, ScipIngestContext, ScipInputFile, ScipOutcome, ScipRequest, ScipTypescript, ingest};
use common::rel;

const FIXTURE: &str = "../../fixtures/adversarial/reexport-chain-ts";
const REPO: &str = "reexport-chain-ts";
const SOURCES: [&str; 6] = [
  "src/index.ts",
  "src/core/impl.ts",
  "src/core/index.ts",
  "src/app/checkout.ts",
  "src/other/total.ts",
  "src/lib/index.ts",
];

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

fn entity_in(context: &ScipIngestContext, path: &str, name: &str) -> EntityId {
  context.files[&rel(path)]
    .symbols
    .iter()
    .find(|symbol| symbol.name == name)
    .unwrap_or_else(|| panic!("no {name} in {path}"))
    .entity
}

#[test]
#[ignore = "needs network or a cached @sourcegraph/scip-typescript"]
fn should_resolve_the_alias_chain_reference_to_the_original_definition() {
  let indexer = ScipTypescript::new();
  let work = tempfile::tempdir().unwrap();
  let package = work.path().join(REPO);
  copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE), &package);
  let repo = RepoName::new(REPO).unwrap();
  let mut context = ScipIngestContext::new(repo.clone());
  let mut inputs = Vec::new();
  for path in SOURCES {
    let source = fs::read_to_string(package.join(path)).unwrap();
    let file = FileRef::detect(repo.clone(), rel(path), ContentHash::of(source.as_bytes())).unwrap();
    let facts = TsAnalyzer::new().analyze(&AnalysisInput { file: &file, source: &source }).unwrap();
    let module_path = ts_module_path(&rel(path), None).unwrap();
    let keys = facts
      .symbol_keys(&KeyContext {
        package: REPO.to_owned(),
        module_path,
      })
      .unwrap();
    context = context.with_file(rel(path), ScipFileContext::from_facts(&facts, keys, source));
    inputs.push(ScipInputFile {
      path: rel(path),
      content_hash: file.content_hash,
    });
  }

  let request = ScipRequest {
    root: package.clone(),
    cache_dir: work.path().join("cache"),
    files: inputs,
  };
  let run = indexer.index(&request);
  assert_eq!(run.outcome, ScipOutcome::Ok, "{run:?}");
  assert_eq!(run.degradations, vec![Degradation::InferredTsconfig, Degradation::MissingNodeModules]);
  assert!(!package.join("tsconfig.json").exists(), "the inferred tsconfig leaked into the package");
  let rerun = indexer.index(&request);
  assert_eq!(rerun.cache, atlas_scip::CacheStatus::Hit);
  assert_eq!(rerun.degradations, run.degradations);
  let scip = ingest(&fs::read(run.output_path.unwrap()).unwrap(), &context).unwrap();

  let checkout = entity_in(&context, "src/app/checkout.ts", "checkout");
  let edges = &scip.per_file[&(repo, rel("src/app/checkout.ts"))];
  let calls: Vec<_> = edges.iter().filter(|edge| edge.kind == EdgeKind::Calls && edge.from == checkout).collect();
  assert_eq!(calls.len(), 1, "{edges:?}");
  assert_eq!(calls[0].to, entity_in(&context, "src/core/impl.ts", "computeTotal"));
  assert_ne!(calls[0].to, entity_in(&context, "src/other/total.ts", "total"));
  assert_eq!(calls[0].certainty, Certainty::CompilerVerified);
}
