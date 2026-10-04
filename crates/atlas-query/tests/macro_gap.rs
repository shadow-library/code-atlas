mod common;

use atlas_query::{Caveat, ImpactRequest, Targeted, TraversalKind, TraverseRequest};

fn fixture() -> common::Fixture {
  common::fixture("adversarial", &["macro-heavy-rust"])
}

fn macro_caveat(caveats: &[Caveat]) -> Option<(usize, Vec<String>)> {
  caveats.iter().find_map(|caveat| match caveat {
    Caveat::MacroGeneratedRefs { total, sites, .. } => Some((*total, sites.iter().map(|site| site.path.clone()).collect())),
    _ => None,
  })
}

#[test]
fn should_warn_that_callees_of_a_function_calling_macro_generated_names_may_be_incomplete() {
  let fixture = fixture();

  let response = fixture.query.callees(&TraverseRequest::new("Point::manhattan")).unwrap();

  let Targeted::Found(answer) = &response.answer else { panic!("{:?}", response.answer) };
  assert_eq!(
    macro_caveat(&answer.caveats),
    Some((3, vec!["add".to_owned(), "self.x".to_owned(), "self.y".to_owned()])),
    "{:#?}",
    answer.caveats
  );
  let text = fixture.text(&response);
  assert!(
    text.contains("3 references in Point::manhattan to possibly macro-generated names were not resolved; its callees may be incomplete"),
    "{text}"
  );
}

#[test]
fn should_serialize_the_macro_caveat_in_json() {
  let fixture = fixture();

  let response = fixture.query.callees(&TraverseRequest::new("Point::manhattan")).unwrap();

  let json = serde_json::to_value(&response).unwrap();
  let caveat = json["answer"]["caveats"]
    .as_array()
    .unwrap()
    .iter()
    .find(|caveat| caveat["caveat"] == "macro_generated_refs")
    .unwrap();
  assert_eq!(caveat["total"], 3, "{caveat:#}");
  assert_eq!(caveat["sites"].as_array().unwrap().len(), 3, "{caveat:#}");
}

#[test]
fn should_warn_in_impact_about_the_targets_own_macro_generated_references() {
  let fixture = fixture();

  let response = fixture.query.impact(&ImpactRequest::new(["Point::manhattan"])).unwrap();

  let Targeted::Found(answer) = &response.answer else { panic!("{:?}", response.answer) };
  assert_eq!(macro_caveat(&answer.unresolved_risk).map(|found| found.0), Some(3), "{:#?}", answer.unresolved_risk);
  let text = fixture.text(&response);
  assert!(
    text.contains("3 references in Point::manhattan to possibly macro-generated names were not resolved; this concerns the target's own body and does not hide dependents"),
    "{text}"
  );
}

#[test]
fn should_not_warn_when_the_body_has_no_macro_generated_references() {
  let fixture = fixture();

  let response = fixture.query.traverse(TraversalKind::Callees, &TraverseRequest::new("helper")).unwrap();

  let Targeted::Found(answer) = &response.answer else { panic!("{:?}", response.answer) };
  assert_eq!(macro_caveat(&answer.caveats), None, "{:#?}", answer.caveats);
}

#[test]
fn should_not_attach_the_callee_caveat_to_callers() {
  let fixture = fixture();

  let response = fixture.query.callers(&TraverseRequest::new("Point::manhattan")).unwrap();

  let Targeted::Found(answer) = &response.answer else { panic!("{:?}", response.answer) };
  assert_eq!(macro_caveat(&answer.caveats), None, "{:#?}", answer.caveats);
}

#[test]
fn should_name_each_target_in_a_multi_target_impact() {
  let fixture = fixture();

  let response = fixture.query.impact(&ImpactRequest::new(["Point::manhattan", "plain"])).unwrap();

  let text = fixture.text(&response);
  assert!(text.contains("3 references in Point::manhattan to possibly macro-generated names"), "{text}");
  assert!(text.contains("1 reference in plain to possibly macro-generated names was not resolved"), "{text}");
}

#[test]
fn should_say_dependencies_not_callees_for_a_dependencies_query() {
  let fixture = fixture();

  let response = fixture.query.dependencies(&TraverseRequest::new("Point::manhattan")).unwrap();

  let text = fixture.text(&response);
  assert!(
    text.contains("in Point::manhattan to possibly macro-generated names were not resolved; its dependencies may be incomplete"),
    "{text}"
  );
  assert!(!text.contains("callees may be incomplete"), "{text}");
}
