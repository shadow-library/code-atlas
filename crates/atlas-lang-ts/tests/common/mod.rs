#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{AnalysisInput, ContentHash, FileFacts, FileRef, Language, LanguageAnalyzer, RelPath, RepoName};
use atlas_lang_ts::TsAnalyzer;

pub fn analyze(path: &str, source: &str) -> FileFacts {
  analyze_in("fixture", path, source)
}

pub fn analyze_in(repo: &str, path: &str, source: &str) -> FileFacts {
  let file = FileRef::detect(RepoName::new(repo).unwrap(), RelPath::new(path).unwrap(), ContentHash::of(source.as_bytes())).unwrap();
  let facts = TsAnalyzer.analyze(&AnalysisInput { file: &file, source }).unwrap();
  facts.check_invariants(source.len()).unwrap();
  facts
}

pub fn fixtures_root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

pub fn source_files(root: &Path) -> Vec<PathBuf> {
  let mut files = Vec::new();
  let mut pending = vec![root.to_path_buf()];
  while let Some(dir) = pending.pop() {
    let Ok(entries) = fs::read_dir(&dir) else { continue };
    for entry in entries.flatten() {
      let path = entry.path();
      let name = entry.file_name().to_string_lossy().into_owned();
      if path.is_dir() {
        if !matches!(name.as_str(), "node_modules" | "target" | ".git") {
          pending.push(path);
        }
      } else if Language::from_path(&name).is_some_and(|language| language != Language::Rust) {
        files.push(path);
      }
    }
  }
  files.sort();
  files
}

pub fn symbol_names(facts: &FileFacts) -> Vec<String> {
  facts.symbols.iter().map(|symbol| format!("{}#{}", symbol.item_path.join("."), symbol.kind)).collect()
}

pub fn owner_name(facts: &FileFacts, local: Option<atlas_core::LocalSymbolId>) -> String {
  match local.and_then(|local| facts.symbol(local)) {
    Some(symbol) => symbol.item_path.join("."),
    None => "<module>".to_owned(),
  }
}

pub fn refs(facts: &FileFacts) -> Vec<String> {
  facts
    .references
    .iter()
    .map(|reference| format!("{} -{:?}-> {}", owner_name(facts, reference.from), reference.kind, reference.path.join(".")))
    .collect()
}
