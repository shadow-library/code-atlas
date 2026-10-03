mod common;

use std::fs;

use atlas_core::{AnalysisInput, ContentHash, FileRef, LanguageAnalyzer, ParseStatus, RelPath, RepoName};
use atlas_lang_ts::TsAnalyzer;
use common::{fixtures_root, source_files};

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

fn analyze_checked(path: &str, source: &str) {
  let file = FileRef::detect(RepoName::new("mutant").unwrap(), RelPath::new(path).unwrap(), ContentHash::of(source.as_bytes())).unwrap();
  let facts = TsAnalyzer.analyze(&AnalysisInput { file: &file, source }).unwrap_or_else(|error| panic!("{path}: {error}"));
  facts.check_invariants(source.len()).unwrap_or_else(|error| panic!("{path}: {error}\n{source}"));
  assert!(!matches!(facts.parse_status, ParseStatus::Failed { .. }), "{path} fell back to failed facts:\n{source}");
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
