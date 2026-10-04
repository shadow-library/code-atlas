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

#[test]
fn should_describe_a_low_entropy_target_as_too_repetitive() {
  let source = format!("export function bump(a: number): void {{\n{}}}\n", "  a++;\n".repeat(10));
  let fixture = common::single_repo("tiny", &[("package.json", PACKAGE), ("src/bump.ts", &source)]);

  let text = fixture.text(&fixture.query.similar(&SimilarRequest::new("bump")).unwrap());

  assert!(text.contains("too repetitive for structural comparison (32 tokens, few distinct tokens)"), "{text}");
  assert!(!text.contains("LowEntropy") && !text.contains("too small"), "{text}");
}
