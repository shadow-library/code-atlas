#![allow(dead_code)]

use atlas_core::{AnalysisInput, ContentHash, FileFacts, FileRef, LanguageAnalyzer, RefKind, ReferenceFact, RelPath, RepoName, SymbolFact};
use atlas_lang_rust::RustAnalyzer;

pub fn file_ref(path: &str, source: &str) -> FileRef {
  FileRef::detect(RepoName::new("fixture").unwrap(), RelPath::new(path).unwrap(), ContentHash::of(source.as_bytes())).unwrap()
}

pub fn analyze_at(path: &str, source: &str) -> FileFacts {
  let file = file_ref(path, source);
  let facts = RustAnalyzer::new().analyze(&AnalysisInput { file: &file, source }).unwrap();
  facts.check_invariants(source.len()).unwrap();
  facts
}

pub fn analyze(source: &str) -> FileFacts {
  analyze_at("src/lib.rs", source)
}

pub fn strings(values: &[&str]) -> Vec<String> {
  values.iter().map(|value| (*value).to_owned()).collect()
}

pub fn symbol<'a>(facts: &'a FileFacts, module_suffix: &[&str], item_path: &[&str]) -> &'a SymbolFact {
  facts
    .symbols
    .iter()
    .find(|symbol| symbol.module_suffix == module_suffix && symbol.item_path == item_path)
    .unwrap_or_else(|| {
      panic!(
        "no symbol {module_suffix:?} {item_path:?} in {:#?}",
        facts.symbols.iter().map(|s| (&s.module_suffix, &s.item_path)).collect::<Vec<_>>()
      )
    })
}

pub fn item(facts: &FileFacts, item_path: &[&str]) -> SymbolFact {
  symbol(facts, &[], item_path).clone()
}

pub fn symbol_names(facts: &FileFacts) -> Vec<String> {
  facts
    .symbols
    .iter()
    .map(|symbol| {
      let mut path = symbol.module_suffix.clone();
      path.extend(symbol.item_path.iter().cloned());
      format!("{}#{}", path.join("::"), symbol.kind)
    })
    .collect()
}

pub fn from_name(facts: &FileFacts, reference: &ReferenceFact) -> String {
  match reference.from.and_then(|local| facts.symbol(local)) {
    Some(symbol) => symbol.name.clone(),
    None => "<file>".to_owned(),
  }
}

pub fn refs_of(facts: &FileFacts, kind: RefKind) -> Vec<(String, Vec<String>)> {
  facts
    .references
    .iter()
    .filter(|reference| reference.kind == kind)
    .map(|reference| (from_name(facts, reference), reference.path.clone()))
    .collect()
}

pub fn paths_of(facts: &FileFacts, kind: RefKind) -> Vec<String> {
  facts
    .references
    .iter()
    .filter(|reference| reference.kind == kind)
    .map(|reference| reference.path.join("::"))
    .collect()
}

pub fn reference<'a>(facts: &'a FileFacts, kind: RefKind, path: &[&str]) -> &'a ReferenceFact {
  facts
    .references
    .iter()
    .find(|reference| reference.kind == kind && reference.path == path)
    .unwrap_or_else(|| panic!("no {kind:?} reference {path:?} in {:#?}", facts.references))
}
