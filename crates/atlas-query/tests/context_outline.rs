mod common;

use atlas_query::{ContextRequest, OmissionReason, Snippet};

#[test]
fn should_outline_named_seeds_and_keep_full_bodies_for_search_hits() {
  let fixture = common::acme();

  let request = ContextRequest {
    seeds: vec!["UsersService".to_owned()],
    ..ContextRequest::new("add a DELETE /orders/:id endpoint with api-key auth")
  };
  let response = fixture.query.context(&request).unwrap();
  let text = fixture.text(&response);
  let answer = response.answer;

  let seed = answer.must_read.iter().find(|item| item.entity.short == "UsersService").expect("seed in MUST READ");
  assert!(seed.read_directly && seed.body.is_none(), "{seed:#?}");
  assert!(
    text.contains("UsersService class acme-api:src/users/users.service.ts:14-37 (named in the task: read the file directly)"),
    "{text}"
  );
  let guard = answer.must_read.iter().find(|item| item.entity.short == "ApiKeyGuard").expect("search hit in MUST READ");
  assert!(!guard.read_directly && matches!(guard.body, Some(Snippet::Text { .. })), "{guard:#?}");
}

#[test]
fn should_say_when_a_child_is_covered_by_an_outline() {
  let fixture = common::acme();

  let request = ContextRequest {
    seeds: vec!["UsersService".to_owned()],
    ..ContextRequest::new("add a DELETE /orders/:id endpoint with api-key auth")
  };
  let response = fixture.query.context(&request).unwrap();
  let text = fixture.text(&response);

  assert_eq!(response.answer.omitted_by_reason.get(&OmissionReason::ShownInOutline), Some(&5));
  assert!(text.contains("; 5 shown in the outline of an enclosing MUST READ item"), "{text}");
}

#[test]
fn should_keep_the_body_of_a_small_named_seed() {
  let fixture = common::acme();

  let request = ContextRequest {
    seeds: vec!["UsersService.remove".to_owned()],
    ..ContextRequest::new("add a DELETE /orders/:id endpoint with api-key auth")
  };
  let answer = fixture.query.context(&request).unwrap().answer;

  let seed = answer.must_read.iter().find(|item| item.entity.short == "UsersService.remove").expect("seed in MUST READ");
  assert!(!seed.read_directly && matches!(seed.body, Some(Snippet::Text { .. })), "{seed:#?}");
}
