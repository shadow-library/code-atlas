mod common;

use std::fs;

use atlas_core::{AnalysisInput, ContentHash, FileFacts, FileRef, LanguageAnalyzer, ParseStatus, RelPath, RepoName};
use atlas_lang_ts::TsAnalyzer;
use common::{fixtures_root, source_files, symbol_names};

const MUTATIONS: usize = 2_000;
const EXTENSIONS: [&str; 5] = ["ts", "tsx", "js", "d.ts", "spec.ts"];
const FRAGMENTS: [&str; 34] = [
  "{",
  "}",
  "(",
  ")",
  "[",
  "]",
  "<",
  ">",
  "`",
  "${",
  "'",
  "\"",
  "/*",
  "*/",
  "//",
  "@",
  "=>",
  "\n",
  ";",
  ",",
  "describe(",
  "it('x', () => {",
  "import ",
  "export default ",
  "class ",
  "function ",
  "async ",
  "\\",
  "é",
  "\u{0}",
  "require(",
  "import(",
  "#",
  "namespace ",
];

struct XorShift(u64);

impl XorShift {
  fn below(&mut self, bound: usize) -> usize {
    self.0 ^= self.0 << 13;
    self.0 ^= self.0 >> 7;
    self.0 ^= self.0 << 17;
    usize::try_from(self.0 % u64::try_from(bound.max(1)).unwrap()).unwrap()
  }
}

fn mutate(random: &mut XorShift, source: &str) -> String {
  let mut chars: Vec<char> = source.chars().collect();
  for _ in 0..=random.below(6) {
    let at = random.below(chars.len() + 1);
    match random.below(4) {
      0 => {
        let end = (at + random.below(40)).min(chars.len());
        chars.drain(at..end);
      }
      1 => {
        let fragment = FRAGMENTS[random.below(FRAGMENTS.len())];
        chars.splice(at..at, fragment.chars());
      }
      2 => {
        let end = (at + random.below(80)).min(chars.len());
        let copy: Vec<char> = chars[at..end].to_vec();
        let target = random.below(chars.len() + 1);
        chars.splice(target..target, copy);
      }
      _ => {
        if let (Some(_), Some(_)) = (chars.get(at), chars.get(at / 2)) {
          chars.swap(at, at / 2);
        }
      }
    }
  }
  chars.into_iter().collect()
}

fn analyze_checked(path: &str, source: &str) -> FileFacts {
  let file = FileRef::detect(RepoName::new("mutant").unwrap(), RelPath::new(path).unwrap(), ContentHash::of(source.as_bytes())).unwrap();
  let facts = TsAnalyzer::new()
    .analyze(&AnalysisInput { file: &file, source })
    .unwrap_or_else(|error| panic!("{path}: {error}"));
  facts.check_invariants(source.len()).unwrap_or_else(|error| panic!("{path}: {error}\n{source}"));
  assert!(!matches!(facts.parse_status, ParseStatus::Failed { .. }), "{path} fell back to failed facts:\n{source}");
  facts
}

fn on_a_2_mib_stack<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> T {
  std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(run).unwrap().join().unwrap()
}

fn has_errors(facts: &FileFacts) -> bool {
  matches!(facts.parse_status, ParseStatus::Partial { .. })
}

fn diagnostic_codes(facts: &FileFacts) -> Vec<&str> {
  facts.diagnostics.iter().map(|diagnostic| diagnostic.code.as_str()).collect()
}

#[test]
fn should_survive_seeded_mutations_of_the_fixture_sources_with_invariants_intact() {
  let corpus: Vec<String> = source_files(&fixtures_root()).iter().map(|path| fs::read_to_string(path).unwrap()).collect();
  assert!(corpus.len() > 50);
  let mut random = XorShift(0x5eed_a71a_5c0d_e001);
  for iteration in 0..MUTATIONS {
    let original = &corpus[random.below(corpus.len())];
    let source = mutate(&mut random, original);
    let extension = EXTENSIONS[random.below(EXTENSIONS.len())];
    analyze_checked(&format!("src/mutant{iteration}.{extension}"), &source);
  }
}

#[test]
fn should_survive_pathologically_deep_nesting_on_a_small_stack() {
  let depth = 20_000;
  let cases = [
    format!("const a = {}1{};", "[".repeat(depth), "]".repeat(depth)),
    format!("const b = {}1;", "x + ".repeat(depth)),
    format!("const c = q{};", ".m()".repeat(depth)),
    format!("function d() {{ {} }}", "{".repeat(depth)),
    format!("const e = {}0;", "() => ".repeat(depth / 4)),
    format!("call({}x{});", "f(".repeat(depth), ")".repeat(depth)),
    format!("type T = {}string{};", "Array<".repeat(depth / 4), ">".repeat(depth / 4)),
    format!("const g = {}1{};", "(".repeat(depth), ")".repeat(depth)),
    format!("let h = <div>{}</div>;", "<a>".repeat(depth / 4)),
    "\u{feff}#!/usr/bin/env node\n\0\0\u{fffd}export {".to_owned(),
    format!("{}const i = 1;", "declare ".repeat(depth)),
    format!("const j = sql<{}string{}>`x`; const k = ;", "Array<".repeat(depth / 4), ">".repeat(depth / 4)),
    format!("const l = {}T{};", "a<".repeat(depth), ">`x`".repeat(depth)),
  ];
  let worker = std::thread::Builder::new()
    .stack_size(1024 * 1024)
    .spawn(move || {
      for (index, source) in cases.iter().enumerate() {
        let extension = if index == 8 { "tsx" } else { "ts" };
        analyze_checked(&format!("src/deep{index}.{extension}"), source);
      }
    })
    .unwrap();
  worker.join().unwrap();
}

#[test]
fn should_survive_runs_that_tree_sitter_merges_recursively_on_a_2_mib_stack() {
  let depth = 30_000;
  let cases = [
    ("src/angles.ts", format!("const x = sql{}1;\n", "<a>".repeat(depth))),
    ("src/tags.ts", format!("const x = sql{}`x`;\n", "<a>".repeat(depth))),
    ("src/calls.tsx", format!("const x = sql{}1;\n", "<a>(".repeat(depth))),
  ];

  let outcomes = on_a_2_mib_stack(move || cases.map(|(path, source)| analyze_checked(path, &source)));

  let summaries: Vec<(Vec<String>, bool)> = outcomes.iter().map(|facts| (symbol_names(facts), has_errors(facts))).collect();
  assert_eq!(
    summaries,
    [(vec!["x#constant".to_owned()], false), (vec!["x#constant".to_owned()], false), (Vec::new(), true)]
  );
}

#[test]
fn should_survive_a_zero_width_identifier_at_the_bottom_of_deep_nesting_on_a_2_mib_stack() {
  let depth = 20_000;
  let cases = [
    format!("const a = {}x +{};\n", "f(".repeat(depth), ")".repeat(depth)),
    format!("const a = {}x +{};\n", "[".repeat(depth), "]".repeat(depth)),
    format!("const a = {}x +{};\n", "(".repeat(depth), ")".repeat(depth)),
    format!("const a = {}x +{};\n", "() => (".repeat(depth), ")".repeat(depth)),
    format!("const a = {}{};\n", "{b: ".repeat(depth), "}".repeat(depth)),
  ];

  let outcomes = on_a_2_mib_stack(move || cases.map(|source| analyze_checked("src/missing.ts", &source)));

  let summaries: Vec<(Vec<String>, bool)> = outcomes.iter().map(|facts| (symbol_names(facts), has_errors(facts))).collect();
  let constant = (vec!["a#constant".to_owned()], true);
  assert_eq!(
    summaries,
    [constant.clone(), constant.clone(), constant.clone(), (vec!["a#function".to_owned()], true), constant]
  );
}

#[test]
fn should_stop_declaring_symbols_nested_beyond_the_depth_limit() {
  let depth = 1_000;
  let namespaces = format!("{}export const x = 1;{}", "namespace n {".repeat(depth), "}".repeat(depth));
  let suites = format!(
    "import {{ describe, it }} from 'vitest';\n{}it('t', () => {{}});{}",
    "describe('s', () => {".repeat(depth),
    "});".repeat(depth)
  );

  let (namespaces, suites) = on_a_2_mib_stack(move || (analyze_checked("src/deep.ts", &namespaces), analyze_checked("src/deep.test.ts", &suites)));

  assert_eq!(namespaces.symbols.len(), 64);
  assert_eq!(namespaces.symbols.last().map(|symbol| symbol.item_path.len()), Some(64));
  assert_eq!(diagnostic_codes(&namespaces), ["ts.depth_limit"]);
  assert_eq!(suites.symbols.len(), 64);
  assert_eq!(suites.symbols.last().map(|symbol| symbol.item_path.len()), Some(64));
  assert_eq!(diagnostic_codes(&suites), ["ts.depth_limit"]);
}
