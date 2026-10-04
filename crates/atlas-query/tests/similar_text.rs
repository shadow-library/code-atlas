mod common;

use atlas_query::SimilarRequest;

const PACKAGE: &str = "{\n  \"name\": \"tiny\",\n  \"private\": true\n}\n";
const SOURCE: &str = "export function one(): number {\n  return 1;\n}\n";

#[test]
fn should_describe_a_too_small_target_in_words_not_debug_output() {
  let fixture = common::single_repo("tiny", &[("package.json", PACKAGE), ("src/one.ts", SOURCE)]);

  let response = fixture.query.similar(&SimilarRequest::new("one")).unwrap();
  let text = fixture.text(&response);

  assert!(text.contains("no structural fingerprint for the target: too small for structural comparison ("), "{text}");
  assert!(!text.contains("TooSmall"), "{text}");
}
