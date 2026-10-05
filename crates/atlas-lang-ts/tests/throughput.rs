use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use atlas_core::{AnalysisInput, ContentHash, FileRef, LanguageAnalyzer, RelPath, RepoName};
use atlas_lang_ts::{MAX_SOURCE_BYTES, TsAnalyzer};

const MAX_FILES: usize = 3_000;
const DEFAULT_CORPUS: &str = "/opt/homebrew/lib/node_modules";

fn corpus_files(root: &Path) -> Vec<PathBuf> {
  let mut files = Vec::new();
  let mut pending = vec![root.to_path_buf()];
  while let Some(dir) = pending.pop() {
    let Ok(entries) = fs::read_dir(&dir) else { continue };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
      if path.is_symlink() {
        continue;
      }
      if path.is_dir() {
        pending.push(path);
      } else if path.extension().is_some_and(|extension| matches!(extension.to_str(), Some("ts" | "tsx" | "mts" | "cts"))) {
        files.push(path);
        if files.len() == MAX_FILES {
          return files;
        }
      }
    }
  }
  files
}

#[test]
#[ignore = "throughput measurement over a local corpus; set ATLAS_TS_CORPUS to point at a directory of .ts files"]
fn should_report_analysis_throughput_over_a_local_corpus() {
  let root = std::env::var("ATLAS_TS_CORPUS").unwrap_or_else(|_| DEFAULT_CORPUS.to_owned());
  let files = corpus_files(Path::new(&root));
  assert!(!files.is_empty(), "no .ts files under {root}");
  let repo = RepoName::new("corpus").unwrap();
  let (mut lines, mut bytes, mut analyzed, mut partial) = (0usize, 0usize, 0usize, 0usize);
  let mut elapsed = Duration::ZERO;
  for path in &files {
    let Ok(source) = fs::read_to_string(path) else { continue };
    if source.len() > MAX_SOURCE_BYTES {
      continue;
    }
    let relative = path.strip_prefix(&root).unwrap_or(path).to_string_lossy().replace('\\', "/");
    let Ok(relative) = RelPath::new(&relative) else { continue };
    let file = FileRef::detect(repo.clone(), relative, ContentHash::of(source.as_bytes())).unwrap();
    let started = Instant::now();
    let facts = TsAnalyzer::new().analyze(&AnalysisInput { file: &file, source: &source }).unwrap();
    elapsed += started.elapsed();
    facts.check_invariants(source.len()).unwrap();
    partial += usize::from(matches!(facts.parse_status, atlas_core::ParseStatus::Partial { .. }));
    lines += source.lines().count();
    bytes += source.len();
    analyzed += 1;
  }
  let seconds = elapsed.as_secs_f64().max(f64::EPSILON);
  println!(
    "analyzed {analyzed} files ({lines} LOC, {:.1} MiB, {partial} partial) in {seconds:.3}s: {:.0} LOC/s, {:.1} MiB/s",
    bytes as f64 / 1_048_576.0,
    lines as f64 / seconds,
    bytes as f64 / 1_048_576.0 / seconds
  );
}

fn generated_bundle(target_bytes: usize) -> String {
  let mut bundle = String::from("(function(){");
  let mut index = 0usize;
  while bundle.len() < target_bytes {
    bundle.push_str(&format!(
      "var v{index}=new M{index}();function f{index}(a,b){{var c=v{index}.m(a);return f{}(c,b)+g{index}(a)+v{index}.n.o(b);}}",
      index / 2
    ));
    index += 1;
  }
  bundle.push_str("})();");
  bundle
}

#[test]
#[ignore = "timing measurement over a generated minified bundle with one huge function scope"]
fn should_report_analysis_time_for_a_large_generated_bundle() {
  let source = generated_bundle(MAX_SOURCE_BYTES / 2);
  let file = FileRef::detect(
    RepoName::new("bundle").unwrap(),
    RelPath::new("dist/bundle.js").unwrap(),
    ContentHash::of(source.as_bytes()),
  )
  .unwrap();
  let started = Instant::now();
  let facts = TsAnalyzer::new().analyze(&AnalysisInput { file: &file, source: &source }).unwrap();
  let elapsed = started.elapsed();
  facts.check_invariants(source.len()).unwrap();
  println!(
    "bundle of {:.2} MiB: {} references in {:.3}s",
    source.len() as f64 / 1_048_576.0,
    facts.references.len(),
    elapsed.as_secs_f64()
  );
}
