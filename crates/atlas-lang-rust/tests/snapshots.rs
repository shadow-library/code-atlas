mod common;

use atlas_core::FileFacts;
use common::analyze_at;

const FINGERPRINT_PREVIEW: usize = 8;

fn facts_for(workspace_path: &str, repo_path: &str) -> FileFacts {
  let absolute = format!("{}/../../{workspace_path}", env!("CARGO_MANIFEST_DIR"));
  let source = std::fs::read_to_string(&absolute).unwrap_or_else(|error| panic!("cannot read {absolute}: {error}"));
  let mut facts = analyze_at(repo_path, &source);
  for body in facts.symbols.iter_mut().filter_map(|symbol| symbol.body.as_mut()) {
    body.normalized_tokens.truncate(FINGERPRINT_PREVIEW);
  }
  facts
}

fn assert_snapshots(root: &str, repo_paths: &[&str]) {
  for repo_path in repo_paths {
    let facts = facts_for(&format!("{root}/{repo_path}"), repo_path);
    let name = format!("{root}/{repo_path}").replace(['/', '.'], "_");
    insta::assert_yaml_snapshot!(name, facts);
  }
}

#[test]
fn should_match_snapshots_for_the_acme_billing_crate() {
  assert_snapshots(
    "fixtures/acme/acme-billing",
    &[
      "src/acme_client.rs",
      "src/error.rs",
      "src/events.rs",
      "src/handlers.rs",
      "src/lib.rs",
      "src/main.rs",
      "src/money.rs",
      "src/store.rs",
      "tests/invoices.rs",
    ],
  );
}

#[test]
fn should_match_snapshots_for_adversarial_rust_fixtures() {
  assert_snapshots("fixtures/adversarial/trait-dispatch-rust", &["src/lib.rs"]);
  assert_snapshots("fixtures/adversarial/macro-heavy-rust", &["src/lib.rs"]);
  assert_snapshots(
    "fixtures/adversarial/duplicate-names-rust",
    &["src/lib.rs", "src/a.rs", "src/b.rs", "src/ambiguous.rs", "src/consumer.rs"],
  );
  assert_snapshots("fixtures/adversarial/broken-syntax", &["crate/src/lib.rs"]);
  assert_snapshots("fixtures/adversarial/generated-code", &["rust/build.rs", "rust/src/lib.rs"]);
}

#[test]
fn should_match_snapshots_for_atlas_core_sources() {
  assert_snapshots("crates/atlas-core", &["src/error.rs", "src/lang.rs", "src/span.rs"]);
}
