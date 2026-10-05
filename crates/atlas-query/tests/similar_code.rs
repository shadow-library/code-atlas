mod common;

use atlas_core::Language;
use atlas_query::{CodeQuery, DraftOrigin, DraftSimilarAnswer, QueryError, SimilarRequest, SimilaritySignal};

const RENAMED_MONEY: &str = "export function renderPrice(totalInCents: number): string {
    const prefix = totalInCents < 0 ? '-' : '';
    const absolute = Math.abs(totalInCents);
    const dollars = Math.floor(absolute / 100);
    const remainder = String(absolute % 100).padStart(2, '0');

    return `${prefix}$${dollars}.${remainder}`;
}
";

const REORDERED_MONEY: &str = "function price(value: number) {
  const magnitude = Math.abs(value);
  const sign = value < 0 ? '-' : '';
  const fraction = String(magnitude % 100).padStart(2, '0');
  const whole = Math.floor(magnitude / 100);
  return sign + '$' + whole + '.' + fraction;
}
";

const MONEY_STATEMENTS: &str = "const sign = amount < 0 ? '-' : '';
const magnitude = Math.abs(amount);
const units = Math.floor(magnitude / 100);
const subunits = String(magnitude % 100).padStart(2, '0');
return `${sign}$${units}.${subunits}`;
";

const PARAPHRASED_TO_CENTS: &str = "pub fn parse_amount(input: &str) -> i64 {
  let cleaned = input.trim();
  let (is_negative, digits) = match cleaned.strip_prefix('-') {
    Some(tail) => (true, tail),
    None => (false, cleaned),
  };
  let (units, decimals) = digits.split_once('.').unwrap_or((digits, \"\"));
  let decimals = format!(\"{decimals:0<2}\");
  let total = units.parse::<i64>().unwrap_or(0) * 100 + decimals[..2].parse::<i64>().unwrap_or(0);
  if is_negative {
    -total
  } else {
    total
  }
}
";

const UNRELATED: &str = "export function zzqqOrderEvents(queue: Array<{ at: number; tag: string }>): Map<string, number[]> {
  const grouped = new Map<string, number[]>();
  for (const entry of queue) {
    const bucket = grouped.get(entry.tag) ?? [];
    bucket.push(entry.at);
    grouped.set(entry.tag, bucket);
  }
  for (const [tag, stamps] of grouped) {
    stamps.sort((left, right) => left - right);
    if (stamps.length > 3) {
      grouped.set(tag, stamps.slice(-3));
    }
  }
  return grouped;
}
";

fn ask(fixture: &common::Fixture, language: Language, text: &str) -> DraftSimilarAnswer {
  fixture.query.similar_to_code(&SimilarRequest::from_code(language, text)).unwrap().answer
}

fn structural(answer: &DraftSimilarAnswer, draft: usize, short: &str) -> Option<f64> {
  answer.drafts[draft]
    .members
    .iter()
    .find(|member| member.entity.short == short)?
    .signals
    .iter()
    .find_map(|signal| match signal {
      SimilaritySignal::Structural { jaccard } => Some(*jaccard),
      _ => None,
    })
}

fn rust_function(source: &str, name: &str) -> String {
  let start = source.find(&format!("pub fn {name}")).unwrap();
  let end = start + source[start..].find("\n}\n").unwrap() + 3;
  source[start..end].to_owned()
}

fn invalid(result: Result<impl std::fmt::Debug, QueryError>) -> String {
  match result {
    Err(QueryError::Invalid(message)) => message,
    other => panic!("expected an invalid-query error, got {other:?}"),
  }
}

#[test]
fn should_find_an_indexed_typescript_function_at_1_0_from_its_own_text() {
  let fixture = common::acme();
  let text = std::fs::read_to_string(fixture.repo_root("acme-api").join("src/common/money.ts")).unwrap();

  let answer = ask(&fixture, Language::TypeScript, &text);

  assert_eq!(answer.drafts.len(), 1);
  assert_eq!(answer.drafts[0].draft.name, "formatMoney");
  assert_eq!(answer.drafts[0].members[0].entity.short, "formatMoney");
  assert_eq!(structural(&answer, 0, "formatMoney"), Some(1.0));
}

#[test]
fn should_find_an_indexed_rust_function_at_1_0_from_its_own_text() {
  let fixture = common::acme();
  let source = std::fs::read_to_string(fixture.repo_root("acme-billing").join("src/money.rs")).unwrap();

  let answer = ask(&fixture, Language::Rust, &rust_function(&source, "to_cents"));

  assert_eq!(answer.drafts[0].draft.name, "to_cents");
  assert_eq!(answer.drafts[0].members[0].entity.short, "to_cents");
  assert_eq!(structural(&answer, 0, "to_cents"), Some(1.0));
}

#[test]
fn should_score_an_indexed_function_at_1_0_against_its_own_text_in_a_single_repo_index() {
  let package = "{\n  \"name\": \"tiny\",\n  \"private\": true\n}\n";
  let source = "export function tally(values: number[]): number {\n  let total = 0;\n  for (const value of values) {\n    if (value > 0) {\n      total += value * 2;\n    } else {\n      total -= 1;\n    }\n  }\n  return total;\n}\n";
  let fixture = common::single_repo("tiny", &[("package.json", package), ("src/tally.ts", source)]);

  let answer = ask(&fixture, Language::TypeScript, source);

  assert_eq!(answer.drafts[0].members.len(), 1);
  assert_eq!(answer.drafts[0].members[0].entity.short, "tally");
  assert_eq!(structural(&answer, 0, "tally"), Some(1.0));
}

#[test]
fn should_find_formatmoney_from_a_draft_with_other_names_and_whitespace_and_state_how_to_import_it() {
  let fixture = common::acme();

  let response = fixture.query.similar_to_code(&SimilarRequest::from_code(Language::TypeScript, RENAMED_MONEY)).unwrap();
  let draft = &response.answer.drafts[0];

  assert_eq!(draft.draft.name, "renderPrice");
  assert_eq!(draft.draft.origin, DraftOrigin::Declared);
  assert_eq!(draft.members[0].entity.short, "formatMoney");
  assert_eq!(draft.members[0].signals[0], SimilaritySignal::Structural { jaccard: 1.0 });
  assert_eq!(draft.members[0].importability.as_deref(), Some("exported from acme-api; behaviour unverified"));
  assert_eq!(draft.members[0].verdict, "structurally identical, behaviour unverified");
  let text = fixture.text(&response);
  assert!(text.contains("similar to draft fn renderPrice (typescript, 8 lines)"), "{text}");
  assert!(text.contains("importability: exported from acme-api; behaviour unverified"), "{text}");
}

#[test]
fn should_rank_formatmoney_first_when_the_draft_reorders_statements_and_builds_the_string_differently() {
  let fixture = common::acme();

  let answer = ask(&fixture, Language::TypeScript, REORDERED_MONEY);

  let top = &answer.drafts[0].members[0];
  assert_eq!(top.entity.short, "formatMoney");
  assert_eq!(structural(&answer, 0, "formatMoney"), Some(0.576));
  assert_eq!(top.verdict, "same shape, behaviour unverified");
  assert_eq!(top.signals[0], SimilaritySignal::Structural { jaccard: 0.576 });
}

#[test]
fn should_find_the_rust_duplicate_from_a_paraphrased_rust_draft() {
  let fixture = common::acme();

  let response = fixture.query.similar_to_code(&SimilarRequest::from_code(Language::Rust, PARAPHRASED_TO_CENTS)).unwrap();
  let draft = &response.answer.drafts[0];

  assert_eq!(draft.draft.name, "parse_amount");
  assert_eq!(draft.members[0].entity.short, "to_cents");
  assert_eq!(draft.members[0].signals[0], SimilaritySignal::Structural { jaccard: 1.0 });
  assert_eq!(draft.members[0].importability.as_deref(), Some("pub in acme-billing; behaviour unverified"));
  assert!(fixture.text(&response).contains("similar to draft fn parse_amount (rust, 15 lines)"));
}

#[test]
fn should_compare_bare_statements_as_a_function_body() {
  let fixture = common::acme();

  let response = fixture.query.similar_to_code(&SimilarRequest::from_code(Language::TypeScript, MONEY_STATEMENTS)).unwrap();
  let draft = &response.answer.drafts[0];

  assert_eq!(draft.draft.origin, DraftOrigin::WrappedFragment);
  assert_eq!(draft.draft.lines, 5);
  assert_eq!(draft.members.len(), 1);
  assert_eq!(draft.members[0].entity.short, "formatMoney");
  assert_eq!(draft.members[0].signals, vec![SimilaritySignal::Structural { jaccard: 1.0 }]);
  let text = fixture.text(&response);
  assert!(text.contains("similar to draft fragment (typescript, 5 lines)"), "{text}");
  assert!(
    text.contains("the draft declares no function; its statements were compared as the body of a function, by structure only"),
    "{text}"
  );
}

#[test]
fn should_report_no_structural_match_for_unrelated_code() {
  let fixture = common::acme();

  let answer = ask(&fixture, Language::TypeScript, UNRELATED);

  assert_eq!(answer.drafts[0].draft.name, "zzqqOrderEvents");
  assert!(answer.drafts[0].shape_only.is_empty());
  assert!(
    answer.drafts[0].members.iter().all(|member| structural_of(member).is_none()),
    "{:?}",
    answer.drafts[0].members
  );
}

fn structural_of(member: &atlas_query::ClusterMember) -> Option<f64> {
  member.signals.iter().find_map(|signal| match signal {
    SimilaritySignal::Structural { jaccard } => Some(*jaccard),
    _ => None,
  })
}

#[test]
fn should_say_what_was_searched_when_nothing_is_similar() {
  let fixture = common::acme();
  let body = UNRELATED.split_once("{\n").unwrap().1.trim_end().trim_end_matches('}');

  let response = fixture.query.similar_to_code(&SimilarRequest::from_code(Language::TypeScript, body)).unwrap();
  let text = fixture.text(&response);

  assert!(response.answer.drafts[0].members.is_empty());
  assert_eq!(response.answer.drafts[0].searched.languages, vec!["tsx", "typescript"]);
  assert!(text.contains("no structural neighbours found"), "{text}");
  assert!(
    text.contains("searched tsx, typescript symbols in acme-api, acme-sdk, acme-web, acme-billing (index of 222 symbols; structural comparison needs bodies of at least 24 tokens); structural threshold 0.50"),
    "{text}"
  );
}

#[test]
fn should_group_results_per_function_when_the_draft_has_two() {
  let fixture = common::acme();

  let answer = ask(&fixture, Language::TypeScript, &format!("{RENAMED_MONEY}\n{UNRELATED}"));

  let names: Vec<&str> = answer.drafts.iter().map(|draft| draft.draft.name.as_str()).collect();
  assert_eq!(names, vec!["renderPrice", "zzqqOrderEvents"]);
  assert_eq!(structural(&answer, 0, "formatMoney"), Some(1.0));
  assert!(answer.drafts[1].members.is_empty());
}

#[test]
fn should_query_each_method_of_a_drafted_class_but_not_closures_inside_them() {
  let fixture = common::acme();
  let draft = "class Prices {\n  format(value: number): string {\n    const run = () => value;\n    return String(run());\n  }\n\n  parse(text: string): number {\n    return Number(text);\n  }\n}\n";

  let answer = ask(&fixture, Language::TypeScript, draft);

  let drafts: Vec<(&str, &atlas_core::EntityKind)> = answer.drafts.iter().map(|draft| (draft.draft.name.as_str(), &draft.draft.kind)).collect();
  assert_eq!(drafts, vec![("format", &atlas_core::EntityKind::Method), ("parse", &atlas_core::EntityKind::Method)]);
}

#[test]
fn should_cap_the_functions_queried_and_say_so() {
  let fixture = common::acme();
  let draft: String = (0..9).map(|n| format!("function draft{n}() {{ return {n}; }}\n")).collect();

  let answer = ask(&fixture, Language::TypeScript, &draft);

  assert_eq!(answer.drafts.len(), 8);
  assert_eq!(answer.drafts[7].draft.name, "draft7");
  assert!(
    answer
      .caveats
      .iter()
      .any(|caveat| format!("{caveat:?}").contains("the draft declares 9 functions; only the first 8 were compared")),
    "{:?}",
    answer.caveats
  );
}

#[test]
fn should_flag_a_draft_with_syntax_errors_and_still_compare_what_parses() {
  let fixture = common::acme();
  let draft = format!("{RENAMED_MONEY}\nfunction broken( {{ return ;\n");

  let answer = ask(&fixture, Language::TypeScript, &draft);

  assert_eq!(answer.drafts[0].draft.name, "renderPrice");
  assert_eq!(structural(&answer, 0, "formatMoney"), Some(1.0));
  assert!(
    answer.caveats.iter().any(|caveat| format!("{caveat:?}").contains("the draft has syntax errors (")),
    "{:?}",
    answer.caveats
  );
}

#[test]
fn should_compare_nothing_for_a_comment_only_draft() {
  let fixture = common::acme();

  let answer = ask(&fixture, Language::TypeScript, "// just a note\n");

  assert!(answer.drafts.is_empty());
  assert_eq!(
    format!("{:?}", answer.caveats),
    "[Note { message: \"no function, method or statement body was found in the draft, so nothing was compared\" }]"
  );
}

#[test]
fn should_reject_a_request_with_both_a_target_and_code() {
  let fixture = common::acme();
  let request = SimilarRequest {
    code: Some(CodeQuery {
      language: Language::TypeScript,
      text: RENAMED_MONEY.to_owned(),
    }),
    ..SimilarRequest::new("formatMoney")
  };

  assert_eq!(invalid(fixture.query.similar_to_code(&request)), "give either a target or code to compare, not both");
  assert_eq!(invalid(fixture.query.similar(&request)), "give either a target or code to compare, not both");
}

#[test]
fn should_reject_a_request_with_neither_a_target_nor_code() {
  let fixture = common::acme();

  assert_eq!(invalid(fixture.query.similar(&SimilarRequest::new("  "))), "give a target symbol or code to compare");
  assert_eq!(invalid(fixture.query.similar_to_code(&SimilarRequest::new(""))), "give a target symbol or code to compare");
}

#[test]
fn should_send_each_kind_of_request_to_its_own_entry_point() {
  let fixture = common::acme();

  assert_eq!(
    invalid(fixture.query.similar(&SimilarRequest::from_code(Language::TypeScript, RENAMED_MONEY))),
    "code drafts are answered by `similar_to_code`"
  );
  assert_eq!(
    invalid(fixture.query.similar_to_code(&SimilarRequest::new("formatMoney"))),
    "no code given; use `similar` to compare an indexed symbol"
  );
}

#[test]
fn should_reject_code_over_the_size_cap_instead_of_truncating_it() {
  let fixture = common::acme();
  let text = format!("// {}\n{RENAMED_MONEY}", "x".repeat(64 * 1024));

  let message = invalid(fixture.query.similar_to_code(&SimilarRequest::from_code(Language::TypeScript, text.clone())));

  assert_eq!(
    message,
    format!(
      "the code to compare is {} bytes, over the 65536 byte limit; pass the function or fragment, not the whole file",
      text.len()
    )
  );
}

#[test]
fn should_reject_blank_code() {
  let fixture = common::acme();

  assert_eq!(
    invalid(fixture.query.similar_to_code(&SimilarRequest::from_code(Language::Rust, " \n\t"))),
    "the code to compare is empty"
  );
}

#[test]
fn should_answer_the_same_request_identically_twice() {
  let fixture = common::acme();
  let request = SimilarRequest::from_code(Language::TypeScript, REORDERED_MONEY);

  let first = fixture.query.similar_to_code(&request).unwrap();
  let second = fixture.query.similar_to_code(&request).unwrap();

  assert_eq!(first.answer, second.answer);
}

#[test]
fn should_not_write_to_the_index_when_comparing_a_draft() {
  let fixture = common::acme();
  let request = SimilarRequest::from_code(Language::TypeScript, UNRELATED);
  let first = fixture.query.similar_to_code(&request).unwrap();
  let runs = fixture.query.engine().store().read(|tx| tx.runs(50)).unwrap().len();

  let second = fixture.query.similar_to_code(&SimilarRequest::from_code(Language::TypeScript, RENAMED_MONEY)).unwrap();

  assert_eq!(fixture.query.engine().similarity().len(), 222);
  assert_eq!(second.freshness.run, first.freshness.run);
  assert_eq!(fixture.query.engine().store().read(|tx| tx.runs(50)).unwrap().len(), runs);
}

const RUST_STATEMENTS: &str = "let cleaned = input.trim();
let (is_negative, digits) = match cleaned.strip_prefix('-') {
  Some(tail) => (true, tail),
  None => (false, cleaned),
};
let (units, decimals) = digits.split_once('.').unwrap_or((digits, \"\"));
let decimals = format!(\"{decimals:0<2}\");
let total = units.parse::<i64>().unwrap_or(0) * 100 + decimals[..2].parse::<i64>().unwrap_or(0);
if is_negative {
  -total
} else {
  total
}";

const PACKAGE: &str = "{\n  \"name\": \"tiny\",\n  \"private\": true\n}\n";
const TALLY_BODY: &str =
  "{\n  let total = 0;\n  for (const value of values) {\n    if (value > 0) {\n      total += value * 2;\n    } else {\n      total -= 1;\n    }\n  }\n  return total;\n}\n";

fn names(members: &[atlas_query::ClusterMember]) -> Vec<&str> {
  members.iter().map(|member| member.entity.short.as_str()).collect()
}

#[test]
fn should_compare_rust_statements_through_the_wrapper() {
  let fixture = common::acme();

  let answer = ask(&fixture, Language::Rust, RUST_STATEMENTS);

  assert_eq!(answer.drafts.len(), 1);
  assert_eq!(answer.drafts[0].draft.origin, DraftOrigin::WrappedFragment);
  assert_eq!(answer.drafts[0].draft.name, "(fragment)");
  assert_eq!(names(&answer.drafts[0].members), vec!["to_cents"]);
  assert_eq!(structural(&answer, 0, "to_cents"), Some(1.0));
}

#[test]
fn should_offer_no_rust_candidates_for_a_typescript_draft_and_no_typescript_ones_for_a_rust_draft() {
  let fixture = common::acme();

  let typescript = ask(&fixture, Language::TypeScript, RENAMED_MONEY);
  let rust = ask(&fixture, Language::Rust, PARAPHRASED_TO_CENTS);

  for draft in typescript.drafts.iter().chain(&rust.drafts) {
    let expected = if draft.draft.name == "renderPrice" { "ts" } else { "rust" };
    assert!(draft.members.iter().chain(&draft.related).all(|member| member.language == expected), "{draft:?}");
  }
  assert_eq!(names(&typescript.drafts[0].related), vec!["toCents", "parseMoney"]);
  assert_eq!(names(&rust.drafts[0].members), vec!["to_cents"]);
}

#[test]
fn should_also_compare_the_statements_that_sit_outside_a_declared_helper() {
  let fixture = common::acme();
  let draft = format!("const clamp = (n: number) => Math.min(n, 100);\n\n{MONEY_STATEMENTS}");

  let response = fixture.query.similar_to_code(&SimilarRequest::from_code(Language::TypeScript, draft)).unwrap();
  let drafts = &response.answer.drafts;

  let heads: Vec<(&str, DraftOrigin)> = drafts.iter().map(|draft| (draft.draft.name.as_str(), draft.draft.origin)).collect();
  assert_eq!(heads, vec![("clamp", DraftOrigin::Declared), ("(fragment)", DraftOrigin::WrappedFragment)]);
  assert_eq!(names(&drafts[1].members), vec!["formatMoney"]);
  assert_eq!(drafts[1].members[0].signals, vec![SimilaritySignal::Structural { jaccard: 1.0 }]);
  assert!(
    fixture
      .text(&response)
      .contains("statements outside the declared functions were also compared as the body of a function, by structure only")
  );
}

#[test]
fn should_not_add_a_fragment_for_a_declared_function_with_only_imports_comments_and_exports_around_it() {
  let fixture = common::acme();
  let draft = format!("import {{ x }} from './x';\n// the price\n{RENAMED_MONEY}");

  let answer = ask(&fixture, Language::TypeScript, &draft);

  assert_eq!(answer.drafts.len(), 1);
  assert_eq!(answer.drafts[0].draft.name, "renderPrice");
}

#[test]
fn should_emit_only_the_nothing_compared_caveat_when_wrapping_yields_no_source() {
  let fixture = common::acme();

  let answer = ask(&fixture, Language::TypeScript, "interface Price { cents: number }\n");

  assert!(answer.drafts.is_empty(), "{answer:?}");
  assert_eq!(
    format!("{:?}", answer.caveats),
    "[Note { message: \"no function, method or statement body was found in the draft, so nothing was compared\" }]"
  );
}

fn tiny(files: &[(&str, &str)]) -> common::Fixture {
  let mut all = vec![("package.json", PACKAGE)];
  all.extend_from_slice(files);
  common::single_repo("tiny", &all)
}

fn formatters() -> String {
  [
    ("formatItemOne", "x"),
    ("formatItemTwo", "y"),
    ("formatItemThree", "z"),
    ("formatItemFour", "w"),
    ("formatItemFive", "v"),
    ("formatItemSix", "u"),
    ("formatItemSeven", "t"),
  ]
  .iter()
  .map(|(name, arg)| format!("export function {name}({arg}: number): string {{\n  return String({arg});\n}}\n\n"))
  .chain(["export function item(a: number): string {\n  return String(a);\n}\n".to_owned()])
  .collect()
}

#[test]
fn should_cap_name_only_rows_at_five_in_a_separate_low_precision_block_and_drop_those_far_below_the_best() {
  let fixture = tiny(&[("src/format.ts", &formatters())]);

  let response = fixture
    .query
    .similar_to_code(&SimilarRequest::from_code(
      Language::TypeScript,
      "function formatItem(q: number): string {\n  return String(q);\n}\n",
    ))
    .unwrap();
  let draft = &response.answer.drafts[0];
  let text = fixture.text(&response);

  assert!(draft.members.is_empty());
  assert_eq!(draft.related.len(), 5, "{:?}", names(&draft.related));
  assert!(!names(&draft.related).contains(&"item"), "{:?}", names(&draft.related));
  assert!(text.contains("related by name/signature (low precision) (5):"), "{text}");
  assert!(text.contains("candidates (0)"), "{text}");
}

#[test]
fn should_order_name_only_rows_by_raw_lexical_score() {
  let source = "export function formatItemLabel(a: number): string {\n  return String(a);\n}\n\nexport function formatItem(a: number): string {\n  return String(a);\n}\n\nexport function formatItemLabelWithLongSuffixName(a: number): string {\n  return String(a);\n}\n";
  let fixture = tiny(&[("src/format.ts", source)]);

  let answer = fixture
    .query
    .similar_to_code(&SimilarRequest::from_code(
      Language::TypeScript,
      "function formatItem(q: number): string {\n  return String(q);\n}\n",
    ))
    .unwrap()
    .answer;
  let scores: Vec<f64> = answer.drafts[0]
    .related
    .iter()
    .map(|member| match &member.signals[0] {
      SimilaritySignal::Lexical { score, .. } => *score,
      other => panic!("{other:?}"),
    })
    .collect();

  assert_eq!(names(&answer.drafts[0].related)[0], "formatItem");
  assert!(scores.windows(2).all(|pair| pair[0] >= pair[1]), "{scores:?}");
  assert!(scores[0] > scores[2], "{scores:?}");
}

#[test]
fn should_put_structural_matches_ahead_of_name_only_matches() {
  let source = format!("export function tally(values: number[]): number {TALLY_BODY}\nexport function sumUpEverything(a: number): string {{\n  return String(a);\n}}\n");
  let fixture = tiny(&[("src/tally.ts", &source)]);

  let answer = fixture
    .query
    .similar_to_code(&SimilarRequest::from_code(
      Language::TypeScript,
      format!("function sumUp(values: number[]): number {TALLY_BODY}"),
    ))
    .unwrap()
    .answer;

  assert_eq!(names(&answer.drafts[0].members), vec!["tally"]);
  assert!(!names(&answer.drafts[0].related).contains(&"tally"));
}

#[test]
fn should_exclude_test_file_candidates_and_count_them() {
  let tally = format!("export function tally(values: number[]): number {TALLY_BODY}");
  let helper = format!(
    "export function sumUpHelper(values: number[]): number {TALLY_BODY}\nexport function sumUpFixture(a: number): string {{\n  return String(a);\n}}\n\nexport function fixtureUp(a: number): string {{\n  return String(a);\n}}\n"
  );
  let fixture = tiny(&[("src/tally.ts", &tally), ("src/tally.spec.ts", &helper)]);

  let response = fixture
    .query
    .similar_to_code(&SimilarRequest::from_code(
      Language::TypeScript,
      format!("function sumUp(values: number[]): number {TALLY_BODY}"),
    ))
    .unwrap();
  let draft = &response.answer.drafts[0];

  assert_eq!(names(&draft.members), vec!["tally"]);
  assert!(draft.related.is_empty(), "{:?}", names(&draft.related));
  assert!(format!("{:?}", draft.caveats).contains("2 test helpers excluded"), "{:?}", draft.caveats);
  assert!(fixture.text(&response).contains("2 test helpers excluded"));
}

#[test]
fn should_apply_the_repo_filter_to_code_drafts() {
  let fixture = common::acme();
  let request = SimilarRequest {
    filter: atlas_query::TargetFilter {
      repo: Some("acme-sdk".to_owned()),
      ..Default::default()
    },
    ..SimilarRequest::from_code(Language::TypeScript, RENAMED_MONEY)
  };

  let answer = fixture.query.similar_to_code(&request).unwrap().answer;

  assert!(answer.drafts[0].members.is_empty());
  assert_eq!(names(&answer.drafts[0].related), vec!["toCents"]);
  assert!(
    answer.drafts[0].related.iter().all(|member| member.entity.key.contains("acme-sdk")),
    "{:?}",
    answer.drafts[0].related
  );
  assert_eq!(answer.drafts[0].searched.repos, vec!["acme-sdk"]);
}

#[test]
fn should_reject_filters_that_do_not_apply_to_code_drafts() {
  let fixture = common::acme();
  let kinds = SimilarRequest {
    filter: atlas_query::TargetFilter {
      kinds: vec![atlas_core::EntityKind::Class],
      ..Default::default()
    },
    ..SimilarRequest::from_code(Language::TypeScript, RENAMED_MONEY)
  };
  let repo = SimilarRequest {
    filter: atlas_query::TargetFilter {
      repo: Some("nope".to_owned()),
      ..Default::default()
    },
    ..SimilarRequest::from_code(Language::TypeScript, RENAMED_MONEY)
  };

  assert_eq!(
    invalid(fixture.query.similar_to_code(&kinds)),
    "the kinds filter does not apply to code drafts; only the repo filter does"
  );
  assert_eq!(
    invalid(fixture.query.similar_to_code(&repo)),
    "unknown repo \"nope\"; this project has acme-api, acme-sdk, acme-web, acme-billing"
  );
}

#[test]
fn should_say_one_line_not_one_lines() {
  let fixture = common::acme();

  let text = fixture.text(
    &fixture
      .query
      .similar_to_code(&SimilarRequest::from_code(Language::TypeScript, "function a() { return 1; }"))
      .unwrap(),
  );

  assert!(text.contains("similar to draft fn a (typescript, 1 line)"), "{text}");
}

#[test]
fn should_still_return_a_same_language_target_when_the_other_language_dominates_the_index() {
  let rust: String = (0..300)
    .map(|n| format!("pub fn format_item_total_{n}(value: i64) -> String {{\n  value.to_string()\n}}\n\n"))
    .collect();
  let typescript = "export function itemTotal(value) {\n  return String(value);\n}\n";
  let fixture = tiny(&[("src/lib.rs", &rust), ("src/price.js", typescript)]);

  let answer = fixture
    .query
    .similar_to_code(&SimilarRequest::from_code(Language::JavaScript, "function formatItemTotal(q) {\n  return String(q);\n}\n"))
    .unwrap()
    .answer;

  assert_eq!(fixture.query.engine().similarity().len(), 301);
  assert_eq!(names(&answer.drafts[0].related), vec!["itemTotal"]);
}

#[test]
fn should_not_add_a_fragment_group_for_a_lone_constant_next_to_a_declared_function() {
  let fixture = common::acme();

  let response = fixture
    .query
    .similar_to_code(&SimilarRequest::from_code(Language::TypeScript, format!("const LIMIT = 100;\n{RENAMED_MONEY}")))
    .unwrap();

  let heads: Vec<&str> = response.answer.drafts.iter().map(|draft| draft.draft.name.as_str()).collect();
  assert_eq!(heads, vec!["renderPrice"]);
  assert!(!fixture.text(&response).contains("fragment"));
}

#[test]
fn should_report_the_statement_residue_own_line_count_for_a_fragment_beside_a_function() {
  let fixture = common::acme();
  let draft = format!("{RENAMED_MONEY}\n{MONEY_STATEMENTS}");

  let response = fixture.query.similar_to_code(&SimilarRequest::from_code(Language::TypeScript, draft)).unwrap();

  assert_eq!(response.answer.drafts.len(), 2);
  assert_eq!(response.answer.drafts[1].draft.lines, 5);
  assert_eq!(names(&response.answer.drafts[1].members), vec!["formatMoney"]);
  assert!(fixture.text(&response).contains("similar to draft fragment (typescript, 5 lines)"));
}

#[test]
fn should_not_flag_syntax_errors_for_a_rust_helper_followed_by_statements() {
  let fixture = common::acme();
  let draft = format!("fn helper(value: i64) -> i64 {{\n  value + 1\n}}\n\n{RUST_STATEMENTS}");

  let answer = ask(&fixture, Language::Rust, &draft);

  let heads: Vec<&str> = answer.drafts.iter().map(|draft| draft.draft.name.as_str()).collect();
  assert_eq!(heads, vec!["helper", "(fragment)"]);
  assert!(!format!("{:?}", answer.caveats).contains("syntax errors"), "{:?}", answer.caveats);
  assert_eq!(names(&answer.drafts[1].members), vec!["to_cents"]);
}
