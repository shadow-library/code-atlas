mod common;

use std::path::Path;

use atlas_engine::{Engine, FreshnessReport};
use common::edits::Tree;
use common::{SplitMix, acme_copy, digest, fact_diff, fresh, import_lineage, open};

const SEEDS: [u64; 3] = [0x00C0_DE47_1A55_0001, 0x5EED_0000_0000_0002, 0x5EED_0000_0000_0003];
const EDITS: usize = 60;
const REOPEN_EVERY: usize = 15;

fn reference_digest(project: &atlas_project::Project, home: &Path, incremental: &Engine) -> (Engine, atlas_core::ContentHash) {
  let reference = open(project, home);
  let report = fresh(&reference);
  assert!(report.header().run.is_some(), "the reference engine must index from scratch");
  import_lineage(incremental, &reference);
  let value = digest(&reference);
  (reference, value)
}

fn assert_equivalent(seed: u64, step: usize, description: &str, project: &atlas_project::Project, incremental: &Engine, scratch: &Path) {
  let home = scratch.join(format!("reference-{step}"));
  let (reference, expected) = reference_digest(project, &home, incremental);
  let actual = digest(incremental);
  if actual != expected {
    let diff = fact_diff(&incremental.db_path(), &reference.db_path());
    panic!("seed {seed:#x} step {step} ({description}): incremental digest {actual} != full digest {expected}\n{diff}");
  }
  drop(reference);
  std::fs::remove_dir_all(&home).unwrap();
}

fn seed_or(name: &str, fallback: u64) -> u64 {
  let Ok(raw) = std::env::var(name) else { return fallback };
  let text = raw.trim().replace('_', "");
  let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
    Some(hex) => u64::from_str_radix(hex, 16),
    None => text.parse(),
  };
  parsed.unwrap_or_else(|_| panic!("{name}={raw} is not a decimal or 0x-prefixed hex u64"))
}

fn env_or<T: std::str::FromStr>(name: &str, fallback: T) -> T {
  std::env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(fallback)
}

#[test]
fn should_match_a_full_index_after_every_randomized_edit_with_the_first_seed() {
  run_sequence(seed_or("ATLAS_EQUIVALENCE_SEED", SEEDS[0]), env_or("ATLAS_EQUIVALENCE_EDITS", EDITS));
}

#[test]
fn should_match_a_full_index_after_every_randomized_edit_with_the_second_seed() {
  run_sequence(seed_or("ATLAS_EQUIVALENCE_SEED_2", SEEDS[1]), env_or("ATLAS_EQUIVALENCE_EDITS", EDITS));
}

#[test]
fn should_match_a_full_index_after_every_randomized_edit_with_the_third_seed() {
  run_sequence(seed_or("ATLAS_EQUIVALENCE_SEED_3", SEEDS[2]), env_or("ATLAS_EQUIVALENCE_EDITS", EDITS));
}

fn run_sequence(seed: u64, edits: usize) {
  let scratch = tempfile::tempdir().unwrap();
  let tree_root = scratch.path().join("tree");
  let project = acme_copy(&tree_root);
  let tree = Tree { root: tree_root };
  let home = scratch.path().join("home");
  let mut engine = open(&project, &home);
  let first = fresh(&engine);
  assert!(matches!(first, FreshnessReport::Fresh(_)), "{first:?}");
  assert_equivalent(seed, 0, "initial index", &project, &engine, scratch.path());
  let mut rng = SplitMix(seed);
  for step in 1..=edits {
    let description = tree.apply(&mut rng, step);
    if step % REOPEN_EVERY == 0 {
      drop(engine);
      engine = open(&project, &home);
    }
    let report = fresh(&engine);
    assert!(report.header().run.is_some(), "step {step} ({description}) did not trigger an update: {report:?}");
    assert_equivalent(seed, step, &description, &project, &engine, scratch.path());
    let quiet = fresh(&engine);
    assert_eq!(quiet.header().run, None, "step {step} ({description}) left changes behind");
  }
}
