mod common;

use std::fs;

use atlas_core::{ArgSummary, EntityKind, LocalSymbolId, TestKind};
use common::{analyze, fixtures_root, refs, symbol_names};

fn tests_of(facts: &atlas_core::FileFacts) -> Vec<(String, String, TestKind, Option<LocalSymbolId>)> {
  facts
    .symbols
    .iter()
    .filter_map(|symbol| {
      let test = symbol.test.as_ref()?;
      Some((symbol.item_path.join(" > "), test.framework.clone(), test.kind, symbol.parent))
    })
    .collect()
}

#[test]
fn should_build_suite_paths_and_attach_references_to_tests_and_suites() {
  let source = fs::read_to_string(fixtures_root().join("acme/acme-api/src/users/users.service.spec.ts")).unwrap();
  let facts = analyze("src/users/users.service.spec.ts", &source);
  let suite = Some(LocalSymbolId(0));
  let find_one = Some(LocalSymbolId(1));
  assert_eq!(
    tests_of(&facts),
    [
      ("UsersService".into(), "jest".into(), TestKind::Suite, None),
      ("UsersService > findOne".into(), "jest".into(), TestKind::Suite, suite),
      (
        "UsersService > findOne > should return the user when it exists".into(),
        "jest".into(),
        TestKind::Case,
        find_one
      ),
      (
        "UsersService > findOne > should throw NotFoundException when the user is missing".into(),
        "jest".into(),
        TestKind::Case,
        find_one
      ),
      ("UsersService > normalizeEmail".into(), "jest".into(), TestKind::Suite, suite),
      (
        "UsersService > normalizeEmail > should lowercase and trim".into(),
        "jest".into(),
        TestKind::Case,
        Some(LocalSymbolId(4))
      ),
    ]
  );
  assert_eq!(facts.symbols[0].kind, EntityKind::TestSuite);
  assert_eq!(facts.symbols[2].kind, EntityKind::Test);
  assert_eq!(facts.symbols[2].signature.as_deref(), Some("it('should return the user when it exists')"));
  assert_eq!(
    facts.symbols[2].test.as_ref().and_then(|test| test.display_name.as_deref()),
    Some("should return the user when it exists")
  );
  let lines = refs(&facts);
  assert!(lines.contains(&"UsersService -Call-> afterEach".to_owned()));
  assert!(lines.contains(&"UsersService -MethodCall-> jest.resetAllMocks".to_owned()));
  assert!(lines.contains(&"UsersService -Instantiation-> UsersService".to_owned()));
  assert!(lines.contains(&"UsersService.normalizeEmail.should lowercase and trim -Call-> normalizeEmail".to_owned()));
  assert!(!lines.iter().any(|line| line.ends_with("-> describe") || line.ends_with("-> it")));
}

#[test]
fn should_take_the_framework_from_imports() {
  let source = fs::read_to_string(fixtures_root().join("acme/acme-sdk/src/money.test.ts")).unwrap();
  let facts = analyze("src/money.test.ts", &source);
  assert_eq!(
    tests_of(&facts),
    [
      ("toCents".into(), "vitest".into(), TestKind::Suite, None),
      (
        "toCents > should convert whole and fractional amounts".into(),
        "vitest".into(),
        TestKind::Case,
        Some(LocalSymbolId(0))
      ),
      (
        "toCents > should keep the sign of negative amounts".into(),
        "vitest".into(),
        TestKind::Case,
        Some(LocalSymbolId(0))
      ),
    ]
  );
  let mocha = analyze(
    "test/a.js",
    "const { describe, it } = require('mocha');\nimport 'mocha';\ndescribe('m', () => { it('x', () => {}); });\n",
  );
  assert!(tests_of(&mocha).iter().all(|(_, framework, ..)| framework == "mocha"));
  let bun = analyze("a.test.ts", "import { test } from 'bun:test';\ntest('x', () => {});\n");
  assert_eq!(tests_of(&bun)[0].1, "bun");
  let node = analyze("a.test.ts", "import test from 'node:test';\ntest('x', () => {});\n");
  assert_eq!(tests_of(&node)[0].1, "node");
  let unknown = analyze("tests/a.ts", "describe('m', () => {});\n");
  assert_eq!(tests_of(&unknown)[0].1, "unknown");
  let vitest_globals = analyze("src/a.spec.ts", "it('x', () => { vi.fn(); });\n");
  assert_eq!(tests_of(&vitest_globals)[0].1, "vitest");
}

#[test]
fn should_recognise_modifier_and_table_variants() {
  let source = r#"
describe.only('focused', () => {
  it.skip('skipped', () => {});
  test.each([[1, 2]])('adds %i', (a, b) => {});
  it.concurrent.each`a | b`('table', () => {});
  xit('legacy skip', () => {});
  it.todo('later');
  test(`template ${name}`, () => {});
  it(dynamicName, () => {});
  it('', () => {});
});
"#;
  let facts = analyze("src/variants.spec.ts", source);
  assert_eq!(
    symbol_names(&facts),
    [
      "focused#test_suite",
      "focused.skipped#test",
      "focused.adds %i#test",
      "focused.table#test",
      "focused.legacy skip#test",
      "focused.later#test",
      "focused.template ${name}#test",
    ]
  );
  let attributes: Vec<Vec<(String, Vec<ArgSummary>)>> = facts
    .symbols
    .iter()
    .map(|symbol| symbol.attributes.iter().map(|attribute| (attribute.path.join("."), attribute.args.clone())).collect())
    .collect();
  assert_eq!(attributes[0], [("only".to_owned(), vec![])]);
  assert_eq!(attributes[1], [("skip".to_owned(), vec![])]);
  assert_eq!(
    attributes[2],
    [(
      "each".to_owned(),
      vec![ArgSummary::Array(vec![ArgSummary::Array(vec![
        ArgSummary::Number("1".into()),
        ArgSummary::Number("2".into())
      ])])]
    )]
  );
  assert_eq!(attributes[3].iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(), ["concurrent", "each"]);
  assert_eq!(attributes[4], [("skip".to_owned(), vec![])]);
  assert_eq!(attributes[5], [("todo".to_owned(), vec![])]);
  assert!(facts.symbols[5].body.is_none());
  assert!(facts.symbols[2].body.is_some());
}

#[test]
fn should_not_treat_shadowed_or_foreign_describe_as_tests() {
  let foreign = analyze("src/otel.spec.ts", "import { context } from '@opentelemetry/api';\ncontext('x', () => {});\n");
  assert!(foreign.symbols.is_empty());
  let shadowed = analyze("src/local.spec.ts", "function run(describe: Fn) { describe('x', () => {}); }\n");
  assert_eq!(symbol_names(&shadowed), ["run#function"]);
}

#[test]
fn should_attach_hooks_to_the_enclosing_suite() {
  let source = "describe('outer', () => {\n  beforeEach(() => { seed(); });\n  describe('inner', () => {\n    afterAll(() => cleanup());\n  });\n});\nbeforeAll(() => boot());\n";
  let facts = analyze("src/hooks.test.ts", source);
  assert_eq!(
    refs(&facts),
    [
      "outer -Call-> beforeEach",
      "outer -Call-> seed",
      "outer.inner -Call-> afterAll",
      "outer.inner -Call-> cleanup",
      "<module> -Call-> beforeAll",
      "<module> -Call-> boot",
    ]
  );
}
