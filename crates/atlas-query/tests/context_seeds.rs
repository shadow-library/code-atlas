mod common;

use atlas_query::{Caveat, ContextAnswer, ContextRequest};

fn challenge_tests() -> Vec<(String, String)> {
  (0..12)
    .map(|n| {
      (
        format!("tests/challenge{n}.test.ts"),
        format!("test('challenge header quoting realm case {n}', () => {{\n  expect(String({n})).toBeDefined();\n}});\n"),
      )
    })
    .collect()
}

fn answer(fixture: &common::Fixture, request: &ContextRequest) -> ContextAnswer {
  fixture.query.context(request).unwrap().answer
}

fn omitted_labels(answer: &ContextAnswer) -> Vec<String> {
  answer.omitted.items.iter().map(|item| item.label.clone()).collect()
}

fn notes(answer: &ContextAnswer) -> Vec<String> {
  answer
    .caveats
    .iter()
    .filter_map(|caveat| match caveat {
      Caveat::Note { message } => Some(message.clone()),
      _ => None,
    })
    .collect()
}

#[test]
fn should_seed_production_code_when_test_hits_outnumber_the_search_slots() {
  let tests = challenge_tests();
  let mut files: Vec<(&str, &str)> = vec![
    ("package.json", r#"{ "name": "web" }"#),
    (
      "src/auth.ts",
      "export function challenge(realm: string): string {\n  return `Basic realm=\"${realm}\"`;\n}\n\nexport function quoteHeader(value: string): string {\n  return value.replace(/\"/g, '\\\\\"');\n}\n",
    ),
  ];
  files.extend(tests.iter().map(|(path, text)| (path.as_str(), text.as_str())));
  let fixture = common::single_repo("web", &files);

  let answer = answer(&fixture, &ContextRequest::new("quote the challenge header realm"));

  let keys: Vec<&str> = answer.must_read.iter().map(|item| item.entity.key.as_str()).collect();
  assert_eq!(keys, ["ts:web:web:src/auth::quoteHeader#function", "ts:web:web:src/auth::challenge#function"]);
}

fn serve_files(count: usize) -> Vec<(String, String)> {
  (0..count)
    .map(|n| {
      (
        format!("src/m{n}.ts"),
        format!("export function serveStatic(root: string): string {{\n  return root + '{n}';\n}}\n"),
      )
    })
    .collect()
}

fn serve_fixture(count: usize) -> common::Fixture {
  let generated = serve_files(count);
  let mut files: Vec<(&str, &str)> = vec![("package.json", r#"{ "name": "web" }"#)];
  files.extend(generated.iter().map(|(path, text)| (path.as_str(), text.as_str())));
  common::single_repo("web", &files)
}

#[test]
fn should_list_the_unused_candidates_of_an_identifier_with_more_than_four_matches() {
  let fixture = serve_fixture(6);
  let mut request = ContextRequest::new("make `serveStatic` negotiate compression");
  request.explain = true;

  let answer = answer(&fixture, &request);

  let seeded = answer
    .explain
    .as_ref()
    .unwrap()
    .seeds
    .iter()
    .filter(|seed| seed.contains("named in task as serveStatic (ambiguous)"))
    .count();
  assert_eq!(seeded, 4);
  let labels = omitted_labels(&answer);
  assert_eq!(labels.len(), 3);
  assert_eq!(labels[0], "serveStatic candidate serveStatic function web:src/m4.ts:1");
  assert_eq!(labels[1], "serveStatic candidate serveStatic function web:src/m5.ts:1");
  assert_eq!(labels[2], "serveStatic: 6 candidates, 4 used");
}

#[test]
fn should_rank_ambiguous_candidates_by_how_well_their_file_matches_the_task() {
  let mut generated = serve_files(5);
  generated.push((
    "src/precompressed.ts".to_owned(),
    "export function serveStatic(root: string): string {\n  return 'precompressed brotli gzip negotiation ' + root;\n}\n".to_owned(),
  ));
  let mut files: Vec<(&str, &str)> = vec![("package.json", r#"{ "name": "web" }"#)];
  files.extend(generated.iter().map(|(path, text)| (path.as_str(), text.as_str())));
  let fixture = common::single_repo("web", &files);
  let mut request = ContextRequest::new("`serveStatic` precompressed brotli gzip negotiation");
  request.explain = true;

  let answer = answer(&fixture, &request);

  let mut seeded: Vec<String> = answer
    .explain
    .as_ref()
    .unwrap()
    .seeds
    .iter()
    .filter(|seed| seed.contains("named in task as serveStatic (ambiguous)"))
    .map(|seed| seed.split(' ').next().unwrap().to_owned())
    .collect();
  seeded.sort();
  assert_eq!(
    seeded,
    [
      "ts:web:web:src/m0::serveStatic#function",
      "ts:web:web:src/m1::serveStatic#function",
      "ts:web:web:src/m2::serveStatic#function",
      "ts:web:web:src/precompressed::serveStatic#function",
    ]
  );
}

#[test]
fn should_seed_every_match_of_an_ambiguous_explicit_seed_and_name_them() {
  let fixture = serve_fixture(2);
  let mut request = ContextRequest::new("negotiate compression");
  request.seeds = vec!["serveStatic".to_owned()];

  let answer = answer(&fixture, &request);

  let mut keys: Vec<&str> = answer.must_read.iter().map(|item| item.entity.key.as_str()).collect();
  keys.sort();
  assert_eq!(keys, ["ts:web:web:src/m0::serveStatic#function", "ts:web:web:src/m1::serveStatic#function"]);
  assert_eq!(
    notes(&answer),
    ["seed serveStatic matches 2 entities; seeded 2: serveStatic function web:src/m0.ts:1; serveStatic function web:src/m1.ts:1"]
  );
}

#[test]
fn should_still_report_an_explicit_seed_that_matches_nothing() {
  let fixture = serve_fixture(2);
  let mut request = ContextRequest::new("negotiate compression");
  request.seeds = vec!["noSuchThing".to_owned()];

  let answer = answer(&fixture, &request);

  let unknown: Vec<&str> = answer
    .omitted
    .items
    .iter()
    .filter(|item| item.label.starts_with("noSuchThing"))
    .map(|item| item.label.as_str())
    .collect();
  assert_eq!(unknown.len(), 1);
  assert_eq!(unknown[0], "noSuchThing: no entity matches noSuchThing; try a search for noSuchThing");
}

fn auth_with_helper(helper: bool) -> common::Fixture {
  let tests = challenge_tests();
  let mut files: Vec<(&str, &str)> = vec![
    ("package.json", r#"{ "name": "web" }"#),
    ("src/auth.ts", "export function realm(value: string): string {\n  return value;\n}\n"),
  ];
  if helper {
    files.push(("tests/support/challenge.ts", "export function challengeHeaderQuoting(): string {\n  return 'quoting';\n}\n"));
  }
  files.extend(tests.iter().map(|(path, text)| (path.as_str(), text.as_str())));
  common::single_repo("web", &files)
}

#[test]
fn should_fall_back_to_the_top_production_hit_when_every_production_hit_is_weak() {
  for helper in [false, true] {
    let fixture = auth_with_helper(helper);
    let mut request = ContextRequest::new("challenge header quoting realm");
    request.explain = true;

    let answer = answer(&fixture, &request);

    let keys: Vec<&str> = answer.must_read.iter().map(|item| item.entity.key.as_str()).collect();
    assert_eq!(keys, ["ts:web:web:src/auth::realm#function"], "helper={helper} {:#?}", answer.explain);
  }
}

#[test]
fn should_keep_seed_diagnostics_when_more_than_forty_items_are_omitted() {
  let generated = serve_files(6);
  let methods: String = (0..60)
    .map(|n| format!("  handle{n}(value: string): string {{\n    return value + '{n}';\n  }}\n"))
    .collect();
  let others: Vec<(String, String)> = (0..6)
    .map(|c| (format!("src/serve/router{c}.ts"), format!("export class ServeRouter{c} {{\n{methods}}}\n")))
    .collect();
  let mut files: Vec<(&str, &str)> = vec![("package.json", r#"{ "name": "web" }"#)];
  files.extend(generated.iter().chain(&others).map(|(path, text)| (path.as_str(), text.as_str())));
  let fixture = common::single_repo("web", &files);
  let mut request = ContextRequest::new(
    "make `serveStatic` and the serve router negotiate compression through `ServeRouter0` `ServeRouter1` `ServeRouter2` `ServeRouter3` `ServeRouter4` `ServeRouter5`",
  );
  request.budget = 100;
  request.seeds = vec!["noSuchThing".to_owned()];

  let answer = answer(&fixture, &request);

  assert!(answer.omitted.total > 40, "{}", answer.omitted.total);
  let labels = omitted_labels(&answer);
  assert_eq!(labels.len(), 40);
  assert!(labels.contains(&"serveStatic: 6 candidates, 4 used".to_owned()));
  assert!(labels.iter().any(|label| label.starts_with("noSuchThing")));
}
