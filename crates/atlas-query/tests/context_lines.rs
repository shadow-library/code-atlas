mod common;

use atlas_query::{ContextAnswer, ContextItem, ContextRequest, LineRange, OmissionReason, RangeBasis, Snippet};

const VIEW: &str = "import { load } from './store';

export function render(id: string): string {
  return load(id).toUpperCase();
}

export function renderMany(ids: string[]): string[] {
  const out: string[] = [];
  for (const id of ids) {
    out.push(render(id));
  }
  return out;
}
";

const STORE: &str = "import { fetchRow } from './db';

export function load(id: string): string {
  return fetchRow(id);
}
";

const DB: &str = "export function fetchRow(id: string): string {
  return 'row ' + id;
}
";

const LABEL: &str = "export function formatLabel(input: string): string {
  let value = input;
  value = value.trim();
  value = value.replace(/\\s+/g, ' ');
  value = value.slice(0, 80);
  value = value.padEnd(4, '.');
  value = value.normalize();
  value = value.replace(/^-/, '');
  value = value.replace(/-$/, '');
  value = value.replace(/_/g, ' ');
  value = value.toUpperCase();
  value = value.concat('');
  value = value.substring(0);
  value = value.trimEnd();
  value = value.trimStart();
  value = value.at(0) === ' ' ? value.slice(1) : value;
  return value;
}
";

fn chain() -> common::Fixture {
  common::single_repo(
    "web",
    &[("package.json", r#"{ "name": "web" }"#), ("src/view.ts", VIEW), ("src/store.ts", STORE), ("src/db.ts", DB)],
  )
}

fn seeded(seed: &str) -> ContextRequest {
  ContextRequest {
    seeds: vec![seed.to_owned()],
    ..ContextRequest::new("rework this")
  }
}

fn answer(fixture: &common::Fixture, request: &ContextRequest) -> ContextAnswer {
  fixture.query.context(request).unwrap().answer
}

fn code_items(answer: &ContextAnswer) -> Vec<&ContextItem> {
  answer.must_read.iter().chain(&answer.skim).collect()
}

fn code_names(answer: &ContextAnswer) -> Vec<&str> {
  let mut names: Vec<&str> = code_items(answer).iter().map(|item| item.entity.short.as_str()).collect();
  names.sort();
  names
}

fn symbol(start: u32, end: u32) -> Option<LineRange> {
  Some(LineRange {
    start,
    end,
    basis: RangeBasis::Symbol,
  })
}

#[test]
fn should_state_the_symbol_span_of_every_code_item() {
  let fixture = chain();

  let response = fixture.query.context(&ContextRequest { depth: 1, ..seeded("render") }).unwrap();
  let text = fixture.text(&response);

  let ranges: Vec<(&str, Option<LineRange>)> = code_items(&response.answer).iter().map(|item| (item.entity.short.as_str(), item.lines)).collect();
  assert_eq!(ranges, [("render", symbol(3, 5)), ("load", symbol(3, 5))]);
  assert!(text.contains("render function web:src/view.ts:3-5  why: explicit seed"), "{text}");
}

#[test]
fn should_point_a_caller_at_the_line_that_calls_the_seed() {
  let fixture = chain();

  let response = fixture.query.context(&seeded("render")).unwrap();
  let text = fixture.text(&response);

  let caller = response.answer.impact.iter().find(|item| item.entity.short == "renderMany").expect("caller in IMPACT");
  assert_eq!(
    caller.lines,
    Some(LineRange {
      start: 10,
      end: 10,
      basis: RangeBasis::Site,
    })
  );
  assert!(text.contains("renderMany function web:src/view.ts:10 (site)  why: calls render [resolved]"), "{text}");
}

#[test]
fn should_expand_three_hops_by_default_and_fewer_when_asked() {
  let fixture = chain();

  let default = answer(&fixture, &seeded("render"));
  let one_hop = answer(&fixture, &ContextRequest { depth: 1, ..seeded("render") });

  assert_eq!(code_names(&default), ["fetchRow", "load", "render"]);
  assert_eq!(code_names(&one_hop), ["load", "render"]);
}

#[test]
fn should_refuse_a_depth_above_three() {
  let fixture = chain();

  let error = fixture.query.context(&ContextRequest { depth: 4, ..seeded("render") }).unwrap_err();

  assert_eq!(error.to_string(), "invalid query: context depth 4 is above the maximum of 3");
}

#[test]
fn should_excerpt_the_lines_matching_the_task_inside_an_outlined_edit_target() {
  let fixture = common::single_repo("web", &[("package.json", r#"{ "name": "web" }"#), ("src/label.ts", LABEL)]);

  let response = fixture.query.context(&ContextRequest::new("make formatLabel stop calling toUpperCase")).unwrap();
  let text = fixture.text(&response);

  let item = response
    .answer
    .must_read
    .iter()
    .find(|item| item.entity.short == "formatLabel")
    .expect("edit target in MUST READ");
  assert!(item.read_directly && item.body.is_none(), "{item:#?}");
  assert_eq!(item.lines, symbol(1, 18));
  assert_eq!(
    item.excerpt,
    Some(Snippet::Text {
      start_line: 10,
      lines: vec![
        "  value = value.replace(/_/g, ' ');".to_owned(),
        "  value = value.toUpperCase();".to_owned(),
        "  value = value.concat('');".to_owned(),
        "  value = value.substring(0);".to_owned(),
        "  value = value.trimEnd();".to_owned(),
      ],
      omitted_lines: 0,
    })
  );
  assert!(text.contains("    11|   value = value.toUpperCase();"), "{text}");
}

#[test]
fn should_drop_an_excerpt_rather_than_an_item_when_the_budget_is_tight() {
  let fixture = common::single_repo("web", &[("package.json", r#"{ "name": "web" }"#), ("src/label.ts", LABEL)]);
  let request = ContextRequest {
    budget: 260,
    ..ContextRequest::new("make formatLabel stop calling toUpperCase")
  };

  let answer = answer(&fixture, &request);

  let shown: Vec<(&str, bool)> = code_items(&answer).iter().map(|item| (item.entity.short.as_str(), item.excerpt.is_some())).collect();
  assert_eq!(shown, [("formatLabel", false)]);
}

#[test]
fn should_leave_a_known_symbol_out_of_the_code_sections_and_say_so() {
  let fixture = chain();

  let answer = answer(
    &fixture,
    &ContextRequest {
      known: vec!["render".to_owned()],
      ..ContextRequest::new("rework this")
    },
  );

  assert_eq!(code_names(&answer), ["fetchRow", "load"]);
  let held: Vec<(&str, OmissionReason)> = answer.omitted.items.iter().map(|item| (item.label.as_str(), item.reason)).collect();
  assert_eq!(held, [("render function web:src/view.ts:3 as render (1 pack item)", OmissionReason::Held)]);
}

#[test]
fn should_seed_the_symbols_of_a_known_file_without_listing_them() {
  let fixture = chain();

  let answer = answer(
    &fixture,
    &ContextRequest {
      known: vec!["src/view.ts".to_owned()],
      ..ContextRequest::new("rework this")
    },
  );

  assert_eq!(code_names(&answer), ["fetchRow", "load"]);
  let labels: Vec<&str> = answer.omitted.items.iter().map(|item| item.label.as_str()).collect();
  assert_eq!(labels, ["src/view.ts file web:src/view.ts as src/view.ts (2 pack items)"]);
}

#[test]
fn should_report_a_known_item_that_matches_nothing() {
  let fixture = chain();

  let answer = answer(
    &fixture,
    &ContextRequest {
      known: vec!["missingThing".to_owned()],
      ..seeded("render")
    },
  );

  let unknown: Vec<(&str, OmissionReason)> = answer
    .omitted
    .items
    .iter()
    .filter(|item| item.reason == OmissionReason::UnknownKnown)
    .map(|item| (item.label.as_str(), item.reason))
    .collect();
  assert_eq!(
    unknown,
    [(
      "known missingThing: no entity matches missingThing; try a search for missingThing",
      OmissionReason::UnknownKnown
    )]
  );
}

#[test]
fn should_list_the_candidates_of_an_ambiguous_known_item_without_seeding_them() {
  let fixture = common::single_repo(
    "web",
    &[
      ("package.json", r#"{ "name": "web" }"#),
      ("src/view.ts", VIEW),
      ("src/store.ts", STORE),
      ("src/db.ts", DB),
      ("src/a.ts", "export function helper(): number {\n  return 1;\n}\n"),
      ("src/b.ts", "export function helper(): number {\n  return 2;\n}\n"),
    ],
  );

  let answer = answer(
    &fixture,
    &ContextRequest {
      known: vec!["helper".to_owned()],
      depth: 1,
      ..seeded("render")
    },
  );

  assert_eq!(code_names(&answer), ["load", "render"]);
  let sections = [
    &answer.must_read,
    &answer.skim,
    &answer.patterns,
    &answer.reuse,
    &answer.impact,
    &answer.tests,
    &answer.uncertain,
  ];
  assert!(sections.iter().flat_map(|items| items.iter()).all(|item| item.entity.short != "helper"), "{answer:#?}");
  let rows: Vec<(&str, OmissionReason)> = answer.omitted.items.iter().map(|item| (item.label.as_str(), item.reason)).collect();
  assert_eq!(
    rows,
    [
      ("known helper: 2 candidates, none held; pass a key or a path", OmissionReason::AmbiguousKnown),
      ("known helper: candidate helper function web:src/a.ts:1", OmissionReason::AmbiguousKnown),
      ("known helper: candidate helper function web:src/b.ts:1", OmissionReason::AmbiguousKnown),
    ]
  );
}

#[test]
fn should_keep_the_tests_of_held_code() {
  let fixture = common::single_repo(
    "web",
    &[
      ("package.json", r#"{ "name": "web" }"#),
      ("src/view.ts", VIEW),
      ("src/store.ts", STORE),
      ("src/db.ts", DB),
      (
        "tests/store.test.ts",
        "import { load } from '../src/store';\n\ntest('loads a row', () => {\n  expect(load('1')).toBe('row 1');\n});\n",
      ),
    ],
  );

  let answer = answer(
    &fixture,
    &ContextRequest {
      known: vec!["src/store.ts".to_owned()],
      ..ContextRequest::new("rework this")
    },
  );

  assert!(code_items(&answer).iter().all(|item| item.entity.short != "load"), "{answer:#?}");
  let tests: Vec<&str> = answer.tests.iter().map(|item| item.entity.short.as_str()).collect();
  assert_eq!(tests, ["loads a row"]);
}
