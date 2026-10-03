mod common;

use std::fs;
use std::path::Path;

use common::{analyze_in, fixtures_root, source_files};

const REPOS: [&str; 11] = [
  "acme/acme-api",
  "acme/acme-sdk",
  "acme/acme-web",
  "globex/globex-api",
  "adversarial/shadowing-ts",
  "adversarial/reexport-chain-ts",
  "adversarial/dynamic-ts",
  "adversarial/computed-routes-ts",
  "adversarial/generated-code",
  "adversarial/broken-syntax",
  "adversarial/similar-not-equivalent",
];

fn snapshot_repo(relative: &str) {
  let repo_root = fixtures_root().join(relative);
  let repo = Path::new(relative).file_name().unwrap().to_string_lossy().into_owned();
  let files = source_files(&repo_root.join(if relative.starts_with("acme/") { "src" } else { "" }));
  assert!(!files.is_empty(), "no sources under {relative}");
  for file in files {
    let path = file.strip_prefix(&repo_root).unwrap().to_string_lossy().replace('\\', "/");
    let source = fs::read_to_string(&file).unwrap();
    let facts = analyze_in(&repo, &path, &source);
    let name = format!("{repo}__{}", path.replace('/', "__"));
    insta::with_settings!({ snapshot_path => "snapshots", prepend_module_to_snapshot => false, omit_expression => true }, {
      insta::assert_yaml_snapshot!(name, facts);
    });
  }
}

#[test]
fn should_snapshot_the_acme_api_sources() {
  snapshot_repo(REPOS[0]);
}

#[test]
fn should_snapshot_the_acme_sdk_sources() {
  snapshot_repo(REPOS[1]);
}

#[test]
fn should_snapshot_the_acme_web_sources() {
  snapshot_repo(REPOS[2]);
}

#[test]
fn should_snapshot_the_globex_api_sources() {
  snapshot_repo(REPOS[3]);
}

#[test]
fn should_snapshot_the_adversarial_ts_cases() {
  for relative in &REPOS[4..] {
    snapshot_repo(relative);
  }
}
