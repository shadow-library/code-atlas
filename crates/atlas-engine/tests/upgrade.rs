mod common;

use std::io::{Read, Write};
use std::path::Path;

use atlas_core::{EdgeKind, Owner, RelPath, RepoName};
use atlas_engine::{Engine, FreshnessReport};
use atlas_project::Project;
use atlas_store::StoreError;
use common::{ACME_REPOS, acme_copy, digest, fresh, open};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use serde_json::{Value, json};

const OLD_VERSION: u32 = 1;
const TAMPERED_REPO: &str = "acme-web";
const TAMPERED_PATH: &str = "src/hooks/useMe.ts";

fn artifact_name(repo: &str, path: &str) -> String {
  format!("artifact:{repo}:{path}")
}

fn read_artifact(bytes: &[u8]) -> Value {
  let mut json = Vec::new();
  ZlibDecoder::new(bytes).read_to_end(&mut json).unwrap();
  serde_json::from_slice(&json).unwrap()
}

fn write_artifact(artifact: &Value) -> Vec<u8> {
  let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
  encoder.write_all(&serde_json::to_vec(artifact).unwrap()).unwrap();
  encoder.finish().unwrap()
}

fn code_files(engine: &Engine) -> Vec<(String, String)> {
  engine
    .store()
    .read(|tx| -> Result<Vec<(String, String)>, StoreError> {
      let mut files = Vec::new();
      for repo in ACME_REPOS {
        let records = tx.files_in_repo(&RepoName::new(repo).unwrap())?;
        files.extend(
          records
            .into_iter()
            .filter(|record| record.language.is_some())
            .map(|record| (repo.to_owned(), record.path.to_string())),
        );
      }
      Ok(files)
    })
    .unwrap()
}

fn rewrite_artifacts(engine: &Engine, files: &[(String, String)], edit: impl Fn(&mut Value)) {
  engine
    .store()
    .write(|tx| -> Result<(), StoreError> {
      for (repo, path) in files {
        let name = artifact_name(repo, path);
        let blob = tx.blob(&name)?.unwrap();
        let mut artifact = read_artifact(&blob.bytes);
        edit(&mut artifact);
        tx.put_blob(&name, blob.version, &write_artifact(&artifact))?;
      }
      Ok(())
    })
    .unwrap();
}

fn set_resolver_version(engine: &Engine, version: u32) {
  engine
    .store()
    .write(|tx| -> Result<(), StoreError> {
      let blob = tx.blob("engine:meta")?.unwrap();
      let mut meta: Value = serde_json::from_slice(&blob.bytes).unwrap();
      meta["resolver_version"] = json!(version);
      tx.put_blob("engine:meta", blob.version, &serde_json::to_vec(&meta).unwrap())
    })
    .unwrap();
}

fn stamp_old_toolchain(artifact: &mut Value) {
  artifact["facts"]["analyzer"]["version"] = json!(OLD_VERSION);
  for producer in artifact["detectors"].as_array_mut().unwrap() {
    producer["version"] = json!(OLD_VERSION);
  }
  for output in artifact["detections"].as_array_mut().unwrap() {
    output["producer"]["version"] = json!(OLD_VERSION);
  }
}

fn tampered_owner() -> Owner {
  Owner::File {
    repo: RepoName::new(TAMPERED_REPO).unwrap(),
    path: RelPath::new(TAMPERED_PATH).unwrap(),
  }
}

fn simulate_old_facts(engine: &Engine) {
  let owner = tampered_owner();
  let mut facts = engine.store().read(|tx| tx.owned_facts(&owner)).unwrap();
  let before = facts.edges.len();
  facts.edges.retain(|edge| edge.kind != EdgeKind::Calls);
  assert!(facts.edges.len() < before, "{TAMPERED_PATH} must have resolved calls to drop");
  engine.store().write(|tx| tx.replace_owner(&owner, &facts)).unwrap();
  let file = [(TAMPERED_REPO.to_owned(), TAMPERED_PATH.to_owned())];
  rewrite_artifacts(engine, &file, |artifact| artifact["facts"]["references"] = json!([]));
}

fn run_counts(engine: &Engine, report: &FreshnessReport) -> (u64, u64) {
  let run = report.header().run.expect("the upgrade must trigger a run");
  let indexed = engine.store().read(|tx| tx.run(run)).unwrap().unwrap();
  let count = |key: &str| indexed.report.counts.get(key).copied().unwrap_or(0);
  (count("files_analyzed"), count("owners_resolved"))
}

fn reopen(engine: Engine, project: &Project, home: &Path) -> Engine {
  drop(engine);
  open(project, home)
}

#[test]
fn should_re_resolve_and_re_analyse_facts_an_older_binary_wrote_and_match_a_full_index() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let home = scratch.path().join("home");
  let engine = open(&project, &home);
  fresh(&engine);
  let full = digest(&engine);
  let files = code_files(&engine);
  let file_count = files.len() as u64;
  simulate_old_facts(&engine);

  let engine = reopen(engine, &project, &home);
  let unchanged = fresh(&engine);

  assert_eq!(unchanged.header().run, None, "current producer versions must not trigger a run");
  assert_ne!(digest(&engine), full, "the simulated old facts must differ from a full index");

  set_resolver_version(&engine, OLD_VERSION);
  let engine = reopen(engine, &project, &home);
  let re_resolved = fresh(&engine);

  assert_eq!(run_counts(&engine, &re_resolved), (0, file_count));
  assert_ne!(digest(&engine), full, "re-resolving alone must not repair facts an older analyzer produced");

  set_resolver_version(&engine, OLD_VERSION);
  rewrite_artifacts(&engine, &files, stamp_old_toolchain);
  let engine = reopen(engine, &project, &home);
  let upgraded = fresh(&engine);

  assert!(upgraded.is_fresh(), "{upgraded:?}");
  assert_eq!(run_counts(&engine, &upgraded), (file_count, file_count));
  assert_eq!(digest(&engine), full);
  assert_eq!(fresh(&engine).header().run, None);
}

fn set_pass_version(engine: &Engine, pass: &str, version: u32) {
  engine
    .store()
    .write(|tx| -> Result<(), StoreError> {
      let blob = tx.blob("engine:meta")?.unwrap();
      let mut meta: Value = serde_json::from_slice(&blob.bytes).unwrap();
      meta["pass_versions"][pass] = json!(version);
      tx.put_blob("engine:meta", blob.version, &serde_json::to_vec(&meta).unwrap())
    })
    .unwrap();
}

#[test]
fn should_rerun_only_the_project_passes_when_a_pass_version_changed() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let home = scratch.path().join("home");
  let engine = open(&project, &home);
  fresh(&engine);
  let full = digest(&engine);
  let link = Owner::Project {
    pass: atlas_link::PASS_NAME.to_owned(),
  };
  let linked = engine.store().read(|tx| tx.owned_facts(&link)).unwrap();
  assert!(!linked.edges.is_empty());
  engine.store().write(|tx| tx.replace_owner(&link, &atlas_store::OwnedFacts::default())).unwrap();

  let engine = reopen(engine, &project, &home);
  assert_eq!(fresh(&engine).header().run, None, "current pass versions must not trigger a run");

  set_pass_version(&engine, atlas_link::PASS_NAME, atlas_link::LINK_VERSION - 1);
  let engine = reopen(engine, &project, &home);
  let rerun = fresh(&engine);

  assert_eq!(run_counts(&engine, &rerun), (0, 0));
  assert_eq!(engine.store().read(|tx| tx.owned_facts(&link)).unwrap(), linked);
  assert_eq!(digest(&engine), full);
  assert_eq!(fresh(&engine).header().run, None);
}
