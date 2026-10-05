mod common;

use atlas_core::EntityKind;
use atlas_query::{Caveat, UnusedCandidate, UnusedClass, UnusedRequest};

const CARGO: &str = "[package]\nname = \"macros\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n";
const LIB: &str = r#"macro_rules! runner {
  ($name:ident) => {
    impl $name {
      pub fn go(&self) -> bool {
        self.prefilter()
      }
    }
  };
}

pub struct Alpha;

impl Alpha {
  fn prefilter(&self) -> bool {
    true
  }

  fn new() -> Self {
    Alpha
  }
}

runner!(Alpha);

fn orphan() -> u32 {
  1
}
"#;

fn find<'a>(candidates: &'a [UnusedCandidate], short: &str) -> &'a UnusedCandidate {
  candidates
    .iter()
    .find(|candidate| candidate.entity.short == short)
    .unwrap_or_else(|| panic!("{short} not reported: {candidates:#?}"))
}

#[test]
fn should_not_call_methods_invoked_from_a_macro_body_likely_unused() {
  let fixture = common::single_repo("macros", &[("Cargo.toml", CARGO), ("src/lib.rs", LIB)]);

  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 100,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;

  let all: Vec<UnusedCandidate> = answer.likely_unused.items.iter().chain(&answer.possibly_unused.items).cloned().collect();
  let prefilter = find(&all, "Alpha::prefilter");
  assert_eq!(prefilter.class, UnusedClass::PossiblyUnused);
  assert!(
    prefilter
      .evidence
      .iter()
      .any(|line| line.starts_with("called by name inside a macro_rules!/include! file (macros:src/lib.rs:5)")),
    "{prefilter:#?}"
  );
  let orphan = find(&all, "orphan");
  assert_eq!(orphan.class, UnusedClass::LikelyUnused, "{orphan:#?}");
  let constructor = find(&all, "Alpha::new");
  assert_eq!(constructor.class, UnusedClass::LikelyUnused, "a bare common name must not demote: {constructor:#?}");
}

const PACKAGE: &str = "{\n  \"name\": \"web\",\n  \"private\": true,\n  \"version\": \"1.0.0\"\n}\n";
const LOADER: &str = "export async function load(name: string) {\n  return import(name);\n}\n";
const HELPERS: &str = "export function formatLabel(value: string): string {\n  return value.trim();\n}\n";
const PAGE: &str = "export default function Home() {\n  return 'home';\n}\n";

#[test]
fn should_treat_dynamic_imports_and_framework_route_modules_as_reachability_risks() {
  let fixture = common::single_repo(
    "web",
    &[
      ("package.json", PACKAGE),
      ("src/loader.ts", LOADER),
      ("src/helpers.ts", HELPERS),
      ("src/pages/index.ts", PAGE),
    ],
  );

  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 100,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;

  let all: Vec<UnusedCandidate> = answer.likely_unused.items.iter().chain(&answer.possibly_unused.items).cloned().collect();
  let helper = find(&all, "formatLabel");
  assert_eq!(helper.class, UnusedClass::PossiblyUnused, "{helper:#?}");
  assert!(helper.evidence.iter().any(|line| line.contains("dynamic import()/require()")), "{helper:#?}");
  assert!(!all.iter().any(|candidate| candidate.entity.short == "Home"), "framework route modules are entrypoints");
  assert_eq!(answer.excluded_entrypoints.get("framework route module (default export)"), Some(&1));
}

const BARE_CARGO: &str = "[package]\nname = \"bare\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n";
const BARE_MAIN: &str = "#[macro_use]\nmod util;\nmod gen;\n\nfn main() {\n  gen::run_all();\n}\n";
const BARE_UTIL: &str =
  "fn validate(name: &str) -> bool {\n  !name.is_empty()\n}\n\nfn really_unused() -> u32 {\n  7\n}\n\nmacro_rules! checked {\n  ($name:expr) => {\n    validate($name)\n  };\n}\n";
const BARE_GEN: &str = "pub fn run_all() -> bool {\n  checked!(\"alpha\")\n}\n";

#[test]
fn should_count_bare_calls_of_common_names_inside_macro_bodies() {
  let fixture = common::single_repo(
    "bare",
    &[("Cargo.toml", BARE_CARGO), ("src/main.rs", BARE_MAIN), ("src/util.rs", BARE_UTIL), ("src/gen.rs", BARE_GEN)],
  );

  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 100,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;

  let all: Vec<UnusedCandidate> = answer.likely_unused.items.iter().chain(&answer.possibly_unused.items).cloned().collect();
  let validate = find(&all, "validate");
  assert_eq!(validate.class, UnusedClass::PossiblyUnused, "{validate:#?}");
  assert!(
    validate
      .evidence
      .iter()
      .any(|line| line.starts_with("called by name inside a macro_rules!/include! file (bare:src/util.rs:11)")),
    "{validate:#?}"
  );
  assert_eq!(find(&all, "really_unused").class, UnusedClass::LikelyUnused);
}

const SHAPE: &str = "export interface Shape {\n  area(): number;\n}\n\nexport class Square implements Shape {\n  area(): number {\n    return 1;\n  }\n}\n";
const SHAPE_TEST: &str = "import { Shape, Square } from './shape';\n\ndescribe('shape', () => {\n  it('measures', () => {\n    const shape: Shape = new Square();\n    expect(shape.area()).toBe(1);\n  });\n});\n";
const SHARED: &str = "export function slug(value: string): string {\n  return value.trim();\n}\n\nexport function onlyTested(value: string): string {\n  return value.trim();\n}\n";
const SHARED_USER: &str = "import { slug } from './shared';\n\nexport function title(value: string): string {\n  return slug(value);\n}\n";
const SHARED_TEST: &str = "import { onlyTested, slug } from './shared';\nimport { title } from './user';\n\ndescribe('shared', () => {\n  it('works', () => {\n    expect(slug(' a ')).toBe('a');\n    expect(onlyTested(' a ')).toBe('a');\n    expect(title(' a ')).toBe('a');\n  });\n});\n";

fn all_shorts(answer: &atlas_query::UnusedAnswer) -> Vec<(String, UnusedClass)> {
  let mut shorts: Vec<(String, UnusedClass)> = answer
    .likely_unused
    .items
    .iter()
    .chain(&answer.possibly_unused.items)
    .chain(&answer.test_only.items)
    .map(|candidate| (candidate.entity.short.clone(), candidate.class))
    .collect();
  shorts.sort();
  shorts
}

#[test]
fn should_class_symbols_used_only_by_tests_as_test_only_and_keep_override_liveness() {
  let fixture = common::single_repo(
    "shapes",
    &[
      ("package.json", PACKAGE),
      ("src/shape.ts", SHAPE),
      ("src/shape.test.ts", SHAPE_TEST),
      ("src/shared.ts", SHARED),
      ("src/user.ts", SHARED_USER),
      ("src/shared.test.ts", SHARED_TEST),
    ],
  );

  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 100,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;

  assert_eq!(
    all_shorts(&answer),
    [
      ("Square".to_owned(), UnusedClass::TestOnly),
      ("onlyTested".to_owned(), UnusedClass::TestOnly),
      ("title".to_owned(), UnusedClass::TestOnly),
    ],
    "{answer:#?}"
  );
  assert_eq!(answer.excluded_entrypoints, Default::default(), "{answer:#?}");
  let only_tested = find(&answer.test_only.items, "onlyTested");
  assert!(only_tested.evidence[0].starts_with("used only by tests: "), "{only_tested:#?}");
  assert!(only_tested.evidence[0].contains("src/shared.test.ts"), "{only_tested:#?}");
}

const RUST_CARGO: &str = "[package]\nname = \"gated\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n";
const RUST_LIB: &str = "fn helper() -> u32 {\n  1\n}\n\nfn shared() -> u32 {\n  2\n}\n\npub fn run() -> u32 {\n  shared()\n}\n\n#[cfg(test)]\nmod tests {\n  use super::*;\n\n  #[test]\n  fn should_use_both() {\n    assert_eq!(helper() + shared(), 3);\n  }\n}\n";

#[test]
fn should_class_a_rust_helper_called_only_from_a_cfg_test_module_as_test_only() {
  let fixture = common::single_repo("gated", &[("Cargo.toml", RUST_CARGO), ("src/lib.rs", RUST_LIB)]);

  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 100,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;

  let reported: Vec<&str> = answer.test_only.items.iter().map(|candidate| candidate.entity.short.as_str()).collect();
  assert_eq!(reported, ["helper"], "{answer:#?}");
  assert!(
    !answer
      .likely_unused
      .items
      .iter()
      .chain(&answer.possibly_unused.items)
      .any(|candidate| candidate.entity.short == "helper")
  );
}

#[test]
fn should_note_that_config_keys_and_env_vars_are_not_checked_for_use() {
  let fixture = common::single_repo("web", &[("package.json", PACKAGE), ("src/helpers.ts", HELPERS)]);
  let expected = Caveat::Note {
    message: "config keys and env vars are not checked for use: their definitions (.env, config files) are not indexed".to_owned(),
  };

  for kind in [EntityKind::ConfigKey, EntityKind::EnvVar] {
    let answer = fixture
      .query
      .unused(&UnusedRequest {
        kinds: vec![kind.clone()],
        ..UnusedRequest::default()
      })
      .unwrap()
      .answer;

    assert_eq!(answer.caveats, std::slice::from_ref(&expected), "{kind:?}");
    assert_eq!(answer.likely_unused.total + answer.possibly_unused.total + answer.test_only.total, 0, "{kind:?}");
  }
  let functions = fixture
    .query
    .unused(&UnusedRequest {
      kinds: vec![EntityKind::Function],
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;
  assert!(!functions.caveats.contains(&expected), "{:?}", functions.caveats);
}

#[test]
fn should_class_the_acme_test_only_symbols_once_and_outside_the_unused_listings() {
  let fixture = common::acme();

  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 500,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;

  let test_only: Vec<&str> = answer.test_only.items.iter().map(|candidate| candidate.entity.short.as_str()).collect();
  assert_eq!(test_only, ["normalizeEmail", "InMemoryInvoiceStore", "AcmeClient.getOrder"]);
  assert_eq!(answer.test_only.total, 3);
  let normalize = find(&answer.test_only.items, "normalizeEmail");
  assert_eq!(normalize.class, UnusedClass::TestOnly);
  assert_eq!(
    normalize.evidence,
    ["used only by tests: UsersService.normalizeEmail.should lowercase and trim at acme-api:src/users/users.service.spec.ts:25"]
  );
  let store = find(&answer.test_only.items, "InMemoryInvoiceStore");
  assert!(
    store
      .evidence
      .iter()
      .any(|line| line.starts_with("its trait methods are called from production code through the trait")),
    "{store:#?}"
  );
  let client = find(&answer.test_only.items, "AcmeClient.getOrder");
  assert!(client.evidence.iter().any(|line| line.starts_with("public API of a library package")), "{client:#?}");
  for name in test_only {
    let elsewhere = answer
      .likely_unused
      .items
      .iter()
      .chain(&answer.possibly_unused.items)
      .any(|candidate| candidate.entity.short == name);
    assert!(!elsewhere, "{name} is listed twice");
  }
}

const GUARD_LIB: &str = "use std::sync::{Mutex, MutexGuard};\n\npub struct Budget {\n  used: u32,\n}\n\nimpl Budget {\n  pub fn try_reserve(&mut self, amount: u32) -> bool {\n    self.used += amount;\n    true\n  }\n}\n\nfn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {\n  value.lock().unwrap()\n}\n\npub fn reserve(budget: &Mutex<Budget>) -> bool {\n  lock(budget).try_reserve(2)\n}\n\n#[cfg(test)]\nmod checks {\n  use super::*;\n\n  #[test]\n  fn should_reserve() {\n    let mut budget = Budget { used: 0 };\n    assert!(budget.try_reserve(1));\n  }\n}\n";
const GUARD_CARGO: &str = "[package]\nname = \"guard\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n";

fn unused_answer(fixture: &common::Fixture) -> atlas_query::UnusedAnswer {
  fixture
    .query
    .unused(&UnusedRequest {
      limit: 100,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer
}

fn test_only_shorts(answer: &atlas_query::UnusedAnswer) -> Vec<&str> {
  answer.test_only.items.iter().map(|candidate| candidate.entity.short.as_str()).collect()
}

#[test]
fn should_not_class_a_rust_method_test_only_when_production_calls_it_on_a_call_result() {
  let fixture = common::single_repo("guard", &[("Cargo.toml", GUARD_CARGO), ("src/lib.rs", GUARD_LIB)]);

  let answer = unused_answer(&fixture);

  assert_eq!(test_only_shorts(&answer), Vec::<&str>::new(), "{answer:#?}");
}

const CONTEXT_TS: &str = "export class Context {\n  set(key: string, value: number): void {\n    console.log(key, value);\n  }\n}\n";
const TAGGER_TS: &str = "export const tagger = () => {\n  return async function tag(ctx, next) {\n    ctx.set('tag', 1);\n    await next();\n  };\n};\n";
const CONTEXT_TEST_TS: &str =
  "import { Context } from './context';\n\ndescribe('context', () => {\n  it('sets', () => {\n    const c: Context = new Context();\n    c.set('a', 1);\n  });\n});\n";

#[test]
fn should_not_class_a_ts_method_test_only_when_production_calls_it_on_an_untyped_parameter() {
  let fixture = common::single_repo(
    "hono-like",
    &[
      ("package.json", PACKAGE),
      ("src/context.ts", CONTEXT_TS),
      ("src/tagger.ts", TAGGER_TS),
      ("src/context.test.ts", CONTEXT_TEST_TS),
    ],
  );

  let answer = unused_answer(&fixture);

  assert!(!test_only_shorts(&answer).contains(&"Context.set"), "{answer:#?}");
}

const WORKSPACE: &str = r#"{ "name": "mono", "private": true, "workspaces": ["packages/*"] }"#;
const LIB_PACKAGE: &str = r#"{ "name": "@m/a", "version": "1.0.0", "main": "src/index.ts" }"#;
const OTHER_PACKAGE: &str = r#"{ "name": "@m/b", "version": "1.0.0", "main": "src/index.ts" }"#;
const HELPER_TS: &str = "export function helper(value: string): string {\n  return value.trim();\n}\n";
const HELPER_TEST_TS: &str = "import { helper } from './helper';\n\ndescribe('helper', () => {\n  it('trims', () => {\n    expect(helper(' a ')).toBe('a');\n  });\n});\n";
const BARREL_TS: &str = "export { helper } from './helper';\n";

#[test]
fn should_keep_a_barrel_reexported_function_called_only_by_a_spec_as_test_only_with_the_entry_point_hedge() {
  let fixture = common::single_repo(
    "lib",
    &[
      ("package.json", r#"{ "name": "lib", "version": "1.0.0", "main": "src/index.ts" }"#),
      ("src/helper.ts", HELPER_TS),
      ("src/index.ts", BARREL_TS),
      ("src/helper.test.ts", HELPER_TEST_TS),
    ],
  );

  let answer = unused_answer(&fixture);

  assert_eq!(test_only_shorts(&answer), ["helper"], "{answer:#?}");
  let helper = find(&answer.test_only.items, "helper");
  assert!(helper.evidence.iter().any(|line| line.starts_with("exported from its package entry point")), "{helper:#?}");
}

fn two_packages(other_source: &str) -> common::Fixture {
  common::single_repo(
    "mono",
    &[
      ("package.json", WORKSPACE),
      ("packages/a/package.json", LIB_PACKAGE),
      ("packages/a/src/helper.ts", HELPER_TS),
      ("packages/a/src/helper.test.ts", HELPER_TEST_TS),
      ("packages/b/package.json", OTHER_PACKAGE),
      ("packages/b/src/other.ts", other_source),
    ],
  )
}

#[test]
fn should_keep_test_only_when_an_unrelated_package_calls_the_same_name_on_an_unknown_receiver() {
  let fixture = two_packages("export function run(thing) {\n  return thing.helper('x');\n}\n");

  let answer = unused_answer(&fixture);

  assert_eq!(test_only_shorts(&answer), ["helper"], "{answer:#?}");
}

#[test]
fn should_keep_test_only_when_an_unrelated_package_makes_a_computed_call() {
  let fixture = two_packages("export function run(thing, name) {\n  return thing[name]();\n}\n");

  let answer = unused_answer(&fixture);

  assert_eq!(test_only_shorts(&answer), ["helper"], "{answer:#?}");
}

const STORE_TS: &str = "export class Store {\n  save(value: string): string {\n    return value;\n  }\n}\n";
const STORE_TEST_TS: &str =
  "import { Store } from './store';\n\ndescribe('store', () => {\n  it('saves', () => {\n    const store = new Store();\n    expect(store.save('a')).toBe('a');\n  });\n});\n";
const COMPUTED_TS: &str = "import { Store } from './store';\n\nexport function run(store: Store, name: string) {\n  return store[name]('a');\n}\n";

#[test]
fn should_drop_a_test_only_method_when_a_file_importing_its_class_makes_a_computed_call() {
  let fixture = common::single_repo(
    "computed",
    &[
      ("package.json", PACKAGE),
      ("src/store.ts", STORE_TS),
      ("src/store.test.ts", STORE_TEST_TS),
      ("src/computed.ts", COMPUTED_TS),
    ],
  );

  let answer = unused_answer(&fixture);

  assert!(!test_only_shorts(&answer).contains(&"Store.save"), "{answer:#?}");
}

#[test]
fn should_keep_a_test_only_method_when_only_a_test_file_makes_a_computed_call() {
  let fixture = common::single_repo(
    "computed-test",
    &[
      ("package.json", PACKAGE),
      ("src/store.ts", STORE_TS),
      (
        "src/store.test.ts",
        "import { Store } from './store';\n\ndescribe('store', () => {\n  it('saves', () => {\n    const store = new Store();\n    store['save']('a');\n    expect(store.save('b')).toBe('b');\n  });\n});\n",
      ),
    ],
  );

  let answer = unused_answer(&fixture);

  assert!(test_only_shorts(&answer).contains(&"Store.save"), "{answer:#?}");
}

const GOLD_CARGO: &str = "[package]\nname = \"gold\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n";
const GOLDEN_LIB: &str = "pub fn real() -> u32 {\n  1\n}\n\n#[cfg(test)]\nmod golden;\n\n#[cfg(test)]\nmod checks {\n  #[test]\n  fn should_use_golden() {\n    assert_eq!(super::golden::fixture(), 2);\n  }\n}\n";
const GOLDEN_MODULE: &str = "pub fn fixture() -> u32 {\n  2\n}\n";

#[test]
fn should_treat_symbols_of_any_cfg_test_module_as_test_code() {
  let fixture = common::single_repo("gold", &[("Cargo.toml", GOLD_CARGO), ("src/lib.rs", GOLDEN_LIB), ("src/golden.rs", GOLDEN_MODULE)]);

  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 100,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;

  assert_eq!(answer.test_only.total, 0, "{answer:#?}");
  assert!(
    !answer
      .likely_unused
      .items
      .iter()
      .chain(&answer.possibly_unused.items)
      .any(|candidate| candidate.entity.short == "fixture"),
    "{answer:#?}"
  );
}

#[test]
fn should_not_class_a_method_test_only_when_production_calls_it_outside_any_package_manifest() {
  let fixture = common::single_repo(
    "loose",
    &[("src/context.ts", CONTEXT_TS), ("src/tagger.ts", TAGGER_TS), ("src/context.test.ts", CONTEXT_TEST_TS)],
  );

  let answer = unused_answer(&fixture);

  assert!(!test_only_shorts(&answer).contains(&"Context.set"), "{answer:#?}");
}

#[test]
fn should_treat_a_test_path_file_without_test_entities_as_test_code() {
  let fixture = common::single_repo(
    "pathed",
    &[
      ("package.json", PACKAGE),
      ("src/helper.ts", HELPER_TS),
      (
        "runtime-tests/deno/helper.test.ts",
        "import { helper } from '../../src/helper';\n\nDeno.test('trims', () => {\n  console.log(helper(' a '));\n});\n",
      ),
    ],
  );

  let answer = unused_answer(&fixture);

  assert_eq!(test_only_shorts(&answer), ["helper"], "{answer:#?}");
}
