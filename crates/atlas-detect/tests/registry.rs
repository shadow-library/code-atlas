mod common;

use std::collections::BTreeSet;

use atlas_core::{DetectInput, Detector};
use atlas_detect::DetectorRegistry;
use common::{analyze, fixture_files};

const FIXTURE_REPOS: [&str; 10] = [
  "acme/acme-api",
  "acme/acme-sdk",
  "acme/acme-web",
  "acme/acme-billing",
  "globex/globex-api",
  "globex/globex-worker",
  "adversarial/computed-routes-ts",
  "adversarial/dynamic-ts",
  "adversarial/broken-syntax",
  "adversarial/macro-heavy-rust",
];

#[test]
fn should_register_every_detector_with_a_unique_id_and_versioned_producer() {
  let registry = DetectorRegistry::default();
  assert_eq!(
    registry.ids(),
    [
      "http-endpoints-nest",
      "http-endpoints-express",
      "http-endpoints-axum",
      "http-clients",
      "auth-schemes",
      "config-env",
      "tests",
      "test-mocks",
      "events",
      "jobs",
      "db",
    ]
  );
  let producers: BTreeSet<String> = registry.producers().iter().map(ToString::to_string).collect();
  assert_eq!(producers.len(), 11);
  assert!(producers.contains("atlas-detect.http-endpoints-nest@1"));
}

#[test]
fn should_merge_detector_outputs_in_a_stable_order() {
  let analyzed = fixture_files("acme/acme-api")
    .into_iter()
    .find(|(path, _)| path == "src/users/users.controller.ts")
    .unwrap()
    .1;
  let registry = DetectorRegistry::default();
  let (first, diagnostics) = registry.run_all(&analyzed.input());
  let (second, _) = registry.run_all(&analyzed.input());
  assert_eq!(first, second);
  assert!(diagnostics.is_empty());
  let keys: Vec<String> = first.entities.iter().map(|entity| entity.key.to_string()).collect();
  let mut sorted_keys = keys.clone();
  sorted_keys.sort();
  sorted_keys.dedup();
  assert_eq!(keys.len(), sorted_keys.len());
  let kinds: Vec<&str> = first.edges.iter().map(|edge| edge.kind.as_str()).collect();
  let mut sorted_kinds = kinds.clone();
  sorted_kinds.sort();
  assert_eq!(kinds, sorted_kinds);
}

#[test]
fn should_count_files_and_facts_per_detector() {
  let registry = DetectorRegistry::default();
  for (_, analyzed) in fixture_files("acme/acme-api") {
    registry.run_all(&analyzed.input());
  }
  let stats = registry.stats();
  let nest = stats.iter().find(|stat| stat.detector == "http-endpoints-nest").unwrap();
  assert_eq!(nest.files_seen, 32);
  assert_eq!(nest.files_applied, 25);
  assert_eq!(nest.entities, 11);
  let express = stats.iter().find(|stat| stat.detector == "http-endpoints-express").unwrap();
  assert_eq!(express.files_applied, 0);
  assert_eq!(express.entities, 0);
}

#[test]
fn should_skip_files_a_detector_does_not_apply_to() {
  let analyzed = analyze("svc", "src/lib.rs", "pub fn f() {}\n");
  let registry = DetectorRegistry::default();
  assert!(registry.run_each(&analyzed.input()).is_empty());
  let nest = atlas_detect::NestEndpointDetector;
  assert!(!nest.applies_to(&analyzed.facts));
}

#[test]
fn should_never_panic_on_fixture_sources_or_truncated_prefixes_of_them() {
  let registry = DetectorRegistry::default();
  for repo in FIXTURE_REPOS {
    for (path, analyzed) in fixture_files(repo) {
      registry.run_all(&analyzed.input());
      let source = &analyzed.source;
      for cut in (1..source.len()).step_by(37).filter(|cut| source.is_char_boundary(*cut)) {
        let truncated = analyze("fuzz", &path, &source[..cut]);
        registry.run_all(&DetectInput {
          facts: &truncated.facts,
          source: &truncated.source,
          package: None,
        });
      }
    }
  }
}
