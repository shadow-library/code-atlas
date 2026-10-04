mod common;

use atlas_query::{SearchAnswer, SearchHitView, SearchRequest};

fn names(hits: &[SearchHitView]) -> Vec<String> {
  hits.iter().map(|hit| format!("{}@{}", hit.entity.short, hit.entity.repo())).collect()
}

fn ranked(answer: &SearchAnswer) -> Vec<String> {
  [&answer.exact_name, &answer.strong, &answer.related].into_iter().flat_map(|group| names(group)).collect()
}

fn search(fixture: &common::Fixture, query: &str) -> SearchAnswer {
  fixture.query.search(&SearchRequest::new(query)).unwrap().answer
}

#[test]
fn should_not_list_prefix_or_partial_token_matches_as_exact_for_user_lookup() {
  let fixture = common::acme();

  let answer = search(&fixture, "user lookup");

  assert_eq!(names(&answer.exact_name), Vec::<String>::new(), "{answer:#?}");
  assert!(ranked(&answer).contains(&"useUser@acme-web".to_owned()), "{answer:#?}");
}

#[test]
fn should_list_a_camel_case_name_as_exact_for_the_same_phrase_with_or_without_separators() {
  let fixture = common::acme();

  for query in ["formatMoney", "format money", "format_money", "FORMAT MONEY"] {
    let answer = search(&fixture, query);

    assert_eq!(names(&answer.exact_name), ["formatMoney@acme-api"], "{query}: {answer:#?}");
  }
}

#[test]
fn should_not_list_a_longer_name_as_exact_for_a_single_token_query() {
  let fixture = common::acme();

  let answer = search(&fixture, "money");

  assert_eq!(names(&answer.exact_name), ["money@acme-billing"], "{answer:#?}");
  assert!(names(&answer.strong).contains(&"formatMoney@acme-api".to_owned()), "{answer:#?}");
}

#[test]
fn should_require_the_identifier_tokens_to_equal_the_query_tokens_for_users_service() {
  let fixture = common::acme();

  let answer = search(&fixture, "users service");

  let exact = names(&answer.exact_name);
  assert!(exact.contains(&"UsersService@acme-api".to_owned()), "{exact:?}");
  assert!(!exact.contains(&"UsersService.users@acme-api".to_owned()), "{exact:?}");
  assert!(!exact.contains(&"UsersModule@acme-api".to_owned()), "{exact:?}");
}

#[test]
fn should_rank_names_covering_more_distinct_query_tokens_first() {
  let fixture = common::acme();

  let answer = search(&fixture, "format money");

  let order = ranked(&answer);
  let position = |name: &str| {
    order
      .iter()
      .position(|candidate| candidate == name)
      .unwrap_or_else(|| panic!("{name} missing from {order:?}"))
  };
  assert_eq!(order[0], "formatMoney@acme-api", "{order:?}");
  assert!(position("formatMoney@acme-api") < position("parseMoney@acme-web"), "{order:?}");
}
