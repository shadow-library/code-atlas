mod common;

use atlas_core::{AnalysisInput, KeyContext, LanguageAnalyzer};
use atlas_lang_rust::RustAnalyzer;
use common::file_ref;

const MUTATIONS: usize = 2_000;
const MAX_RANGE: usize = 64;
const CORPUS: [&str; 8] = [
  "fixtures/acme/acme-billing/src/store.rs",
  "fixtures/acme/acme-billing/src/handlers.rs",
  "fixtures/acme/acme-billing/tests/invoices.rs",
  "fixtures/adversarial/macro-heavy-rust/src/lib.rs",
  "fixtures/adversarial/trait-dispatch-rust/src/lib.rs",
  "fixtures/adversarial/broken-syntax/crate/src/lib.rs",
  "crates/atlas-core/src/args.rs",
  "crates/atlas-core/src/span.rs",
];
const MULTIBYTE_SAMPLE: &str = "/// Grüße — ✓ 😀\npub fn grüß(name: &str) -> String {\n  format!(\"héllo {name} 😀\")\n}\nconst EMOJI: &str = \"🦀\";\n";

fn deep_nesting_sample() -> String {
  format!(
    "fn deep() {{ let x = {}1{}; start(){}; m!({}{}); }}\n{}fn inner() {{}}{}\n",
    "(".repeat(400),
    ")".repeat(400),
    ".next()".repeat(400),
    "n!(".repeat(200),
    ")".repeat(200),
    "mod m { ".repeat(100),
    " }".repeat(100)
  )
}

struct XorShift(u64);

impl XorShift {
  fn next(&mut self) -> u64 {
    self.0 ^= self.0 << 13;
    self.0 ^= self.0 >> 7;
    self.0 ^= self.0 << 17;
    self.0
  }

  fn below(&mut self, bound: usize) -> usize {
    if bound == 0 {
      return 0;
    }
    (self.next() % bound as u64) as usize
  }
}

fn corpus() -> Vec<Vec<u8>> {
  let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
  CORPUS
    .iter()
    .map(|path| std::fs::read(format!("{root}/{path}")).unwrap())
    .chain([MULTIBYTE_SAMPLE.as_bytes().to_vec(), deep_nesting_sample().into_bytes()])
    .collect()
}

fn mutate(bytes: &mut Vec<u8>, rng: &mut XorShift) {
  for _ in 0..=rng.below(3) {
    let start = rng.below(bytes.len());
    let end = (start + 1 + rng.below(MAX_RANGE)).min(bytes.len());
    match rng.below(3) {
      0 => {
        bytes.drain(start..end);
      }
      1 => {
        let copy = bytes[start..end].to_vec();
        let at = rng.below(bytes.len() + 1);
        bytes.splice(at..at, copy);
      }
      _ => {
        for byte in &mut bytes[start..end] {
          *byte ^= (rng.next() & 0xff) as u8;
        }
      }
    }
  }
}

#[test]
fn should_never_panic_and_always_satisfy_invariants_on_mutated_sources() {
  let analyzer = RustAnalyzer::new();
  let corpus = corpus();
  let context = KeyContext {
    package: "fuzz".into(),
    module_path: vec!["module".into()],
  };
  let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
  let mut analyzed = 0;
  for iteration in 0..MUTATIONS {
    let mut bytes = corpus[iteration % corpus.len()].clone();
    mutate(&mut bytes, &mut rng);
    let source = String::from_utf8_lossy(&bytes).into_owned();
    let file = file_ref("src/fuzz.rs", &source);
    let Ok(facts) = analyzer.analyze(&AnalysisInput { file: &file, source: &source }) else {
      continue;
    };
    analyzed += 1;
    if let Err(error) = facts.check_invariants(source.len()) {
      panic!("mutation {iteration} broke an invariant: {error}\n{source}");
    }
    if let Err(error) = facts.symbol_keys(&context) {
      panic!("mutation {iteration} produced an invalid key: {error}\n{source}");
    }
  }
  assert_eq!(analyzed, MUTATIONS);
}
