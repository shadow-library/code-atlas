#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{AnalysisInput, ContentHash, DetectInput, Detection, Diagnostic, FileFacts, FileRef, Language, LanguageAnalyzer, LanguageFamily, NodeRef, RelPath, RepoName};
use atlas_detect::{DetectorRegistry, DiagnosingDetector};
use atlas_lang_rust::RustAnalyzer;
use atlas_lang_ts::TsAnalyzer;

pub struct Analyzed {
  pub facts: FileFacts,
  pub source: String,
}

impl Analyzed {
  pub fn input(&self) -> DetectInput<'_> {
    DetectInput {
      facts: &self.facts,
      source: &self.source,
      package: None,
    }
  }
}

pub fn fixtures_root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

pub fn analyze(repo: &str, path: &str, source: &str) -> Analyzed {
  let file = FileRef::detect(RepoName::new(repo).unwrap(), RelPath::new(path).unwrap(), ContentHash::of(source.as_bytes())).unwrap();
  let input = AnalysisInput { file: &file, source };
  let facts = if file.language.family() == LanguageFamily::Rust {
    RustAnalyzer::new().analyze(&input)
  } else {
    TsAnalyzer.analyze(&input)
  }
  .unwrap();
  Analyzed { facts, source: source.to_owned() }
}

pub fn fixture_files(repo_dir: &str) -> Vec<(String, Analyzed)> {
  let root = fixtures_root().join(repo_dir);
  let repo = Path::new(repo_dir).file_name().unwrap().to_string_lossy().into_owned();
  let mut files = Vec::new();
  collect(&root, &mut files);
  files.sort();
  files
    .into_iter()
    .map(|file| {
      let path = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
      let source = fs::read_to_string(&file).unwrap();
      let analyzed = analyze(&repo, &path, &source);
      (path, analyzed)
    })
    .collect()
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
  for entry in fs::read_dir(dir).unwrap().flatten() {
    let path = entry.path();
    if path.is_dir() {
      if !path.ends_with("node_modules") && !path.ends_with("target") {
        collect(&path, files);
      }
    } else if Language::from_path(&path.to_string_lossy()).is_some() {
      files.push(path);
    }
  }
}

pub fn node(facts: &FileFacts, node: &NodeRef) -> String {
  match node {
    NodeRef::Local(local) => {
      let symbol = facts.symbol(*local).unwrap();
      let separator = if facts.file.language.family() == LanguageFamily::Rust { "::" } else { "." };
      symbol.module_suffix.iter().chain(&symbol.item_path).cloned().collect::<Vec<_>>().join(separator)
    }
    NodeRef::Key(key) => key.to_string(),
    NodeRef::Unresolved { path, .. } => format!("?{}", path.join("::")),
  }
}

pub fn render(facts: &FileFacts, detection: &Detection) -> Vec<String> {
  let mut lines: Vec<String> = detection.entities.iter().map(|entity| format!("{} {}", entity.key, entity.certainty.tag())).collect();
  lines.extend(
    detection
      .edges
      .iter()
      .map(|edge| format!("{} {} -> {} {}", edge.kind, node(facts, &edge.from), node(facts, &edge.to), edge.certainty.tag())),
  );
  lines
}

pub fn run(detector: &dyn DiagnosingDetector, analyzed: &Analyzed) -> (Detection, Vec<Diagnostic>) {
  if !detector.applies_to(&analyzed.facts) {
    return (Detection::default(), Vec::new());
  }
  detector.detect_with_diagnostics(&analyzed.input())
}

pub fn render_source(detector: &dyn DiagnosingDetector, repo: &str, path: &str, source: &str) -> Vec<String> {
  let analyzed = analyze(repo, path, source);
  let (detection, _) = run(detector, &analyzed);
  sorted(render(&analyzed.facts, &detection))
}

pub fn render_repo(detector: &dyn DiagnosingDetector, repo_dir: &str) -> Vec<String> {
  let mut lines = Vec::new();
  for (_, analyzed) in fixture_files(repo_dir) {
    let (detection, _) = run(detector, &analyzed);
    lines.extend(render(&analyzed.facts, &detection));
  }
  sorted(lines)
}

pub fn render_file(detector: &dyn DiagnosingDetector, repo_dir: &str, path: &str) -> Vec<String> {
  let repo = Path::new(repo_dir).file_name().unwrap().to_string_lossy().into_owned();
  let source = fs::read_to_string(fixtures_root().join(repo_dir).join(path)).unwrap();
  render_source(detector, &repo, path, &source)
}

pub fn registry_render(repo: &str, path: &str, source: &str) -> Vec<String> {
  let analyzed = analyze(repo, path, source);
  let (detection, _) = DetectorRegistry::default().run_all(&analyzed.input());
  render(&analyzed.facts, &detection)
}

pub fn sorted(mut lines: Vec<String>) -> Vec<String> {
  lines.sort();
  lines.dedup();
  lines
}

pub fn lines(expected: &[&str]) -> Vec<String> {
  sorted(expected.iter().map(|line| (*line).to_owned()).collect())
}
