mod common;

use atlas_query::{UnusedCandidate, UnusedClass, UnusedRequest};

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
