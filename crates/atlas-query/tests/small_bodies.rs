mod common;

use atlas_query::{ContextRequest, SearchRequest, SimilarRequest, SimilaritySignal, Targeted};

const PACKAGE: &str = "{\n  \"name\": \"small\",\n  \"private\": true\n}\n";
const BODY: &str = "(xs: number[]): number {\n  let total = 0;\n  for (const x of xs) { if (x > 0) total += x * 2; }\n  return total;\n}\n";

fn fixture() -> common::Fixture {
  let source: String = ["sumItems", "sumValues", "quxAlpha", "plaidBeta"]
    .iter()
    .map(|name| format!("export function {name}{BODY}\n"))
    .collect();
  common::single_repo("small", &[("package.json", PACKAGE), ("src/a.ts", &source)])
}

#[test]
fn should_count_a_small_match_in_similar_only_when_a_name_corroborates_it() {
  let fixture = fixture();
  let Targeted::Found(answer) = fixture.query.similar(&SimilarRequest::new("sumItems")).unwrap().answer else {
    panic!("target not found")
  };

  let members: Vec<&str> = answer.members.iter().map(|member| member.entity.short.as_str()).collect();
  assert!(members.contains(&"sumValues"), "{members:?}");
  assert!(!members.contains(&"quxAlpha") && !members.contains(&"plaidBeta"), "{members:?}");
  let corroborated = answer.members.iter().find(|member| member.entity.short == "sumValues").unwrap();
  assert!(corroborated.signals.iter().any(|signal| matches!(signal, SimilaritySignal::SmallStructural { .. })));
  assert!(!corroborated.signals.iter().any(|signal| matches!(signal, SimilaritySignal::Structural { .. })));
  assert!(
    !corroborated.verdict.contains("same shape") && !corroborated.verdict.contains("structurally identical"),
    "{}",
    corroborated.verdict
  );
  assert_eq!(names_of_shape_only(&answer), vec!["plaidBeta", "quxAlpha"]);
  assert!(answer.shape_only.iter().all(|hit| hit.note == "shape-only (small body, likely boilerplate)"));
}

fn names_of_shape_only(answer: &atlas_query::SimilarAnswer) -> Vec<&str> {
  let mut found: Vec<&str> = answer.shape_only.iter().map(|hit| hit.entity.short.as_str()).collect();
  found.sort_unstable();
  found
}

#[test]
fn should_keep_uncorroborated_small_matches_out_of_similar_members_and_consolidation() {
  let fixture = fixture();
  let response = fixture.query.similar(&SimilarRequest::new("quxAlpha")).unwrap();
  let text = fixture.text(&response);
  let Targeted::Found(answer) = response.answer else { panic!("target not found") };

  assert!(
    answer
      .members
      .iter()
      .all(|member| !matches!(member.entity.short.as_str(), "plaidBeta" | "sumItems" | "sumValues")),
    "{text}"
  );
  assert_eq!(names_of_shape_only(&answer), vec!["plaidBeta", "sumItems", "sumValues"]);
  assert!(answer.shared_home.is_empty() && answer.coupling_risks.is_empty(), "{text}");
  assert!(text.contains("shape-only, low weight (3)"), "{text}");
  assert!(!text.contains("same shape") && !text.contains("structurally identical"), "{text}");
}

#[test]
fn should_say_when_the_small_pair_floor_overrides_the_requested_threshold() {
  let fixture = fixture();
  let response = fixture.query.similar(&SimilarRequest::new("sumItems")).unwrap();
  let text = fixture.text(&response);
  assert!(text.contains("held to a minimum score of 0.85, overriding the requested threshold 0.50"), "{text}");
}

#[test]
fn should_drop_uncorroborated_small_matches_from_search_structural_hits() {
  let fixture = fixture();
  let answer = fixture.query.search(&SearchRequest::new("quxAlpha")).unwrap().answer;
  assert!(answer.exact_name.iter().any(|hit| hit.entity.short == "quxAlpha"));
  assert!(answer.structurally_similar.is_empty(), "{:?}", answer.structurally_similar);
}

#[test]
fn should_keep_name_corroborated_small_matches_in_search_structural_hits() {
  let fixture = fixture();
  let answer = fixture
    .query
    .search(&SearchRequest {
      limit: 1,
      ..SearchRequest::new("sumItems")
    })
    .unwrap()
    .answer;
  let hits: Vec<&str> = answer.structurally_similar.iter().map(|hit| hit.entity.short.as_str()).collect();
  assert!(hits.contains(&"sumValues"), "{hits:?}");
  assert!(!hits.contains(&"quxAlpha") && !hits.contains(&"plaidBeta"), "{hits:?}");
  assert!(answer.structurally_similar.iter().all(|hit| hit.small_body));
}

#[test]
fn should_not_offer_uncorroborated_small_bodies_as_reuse_in_context() {
  let fixture = fixture();
  let answer = fixture
    .query
    .context(&ContextRequest {
      seeds: vec!["quxAlpha".to_owned()],
      ..ContextRequest::new("change quxAlpha")
    })
    .unwrap()
    .answer;
  let reuse: Vec<&str> = answer.reuse.iter().map(|item| item.entity.short.as_str()).collect();
  assert!(reuse.is_empty(), "{reuse:?}");
}

#[test]
fn should_offer_name_corroborated_small_bodies_as_reuse_in_context_without_claiming_the_same_shape() {
  let fixture = fixture();
  let answer = fixture
    .query
    .context(&ContextRequest {
      seeds: vec!["sumItems".to_owned()],
      ..ContextRequest::new("change sumItems")
    })
    .unwrap()
    .answer;
  let item = answer
    .reuse
    .iter()
    .find(|item| item.entity.short == "sumValues")
    .unwrap_or_else(|| panic!("{:?}", answer.reuse));
  let why = item.why.clone();
  assert!(why.contains("small body"), "{why}");
  assert!(!why.contains("same shape"), "{why}");
  assert!(answer.reuse.iter().all(|item| !matches!(item.entity.short.as_str(), "quxAlpha" | "plaidBeta")));
}
