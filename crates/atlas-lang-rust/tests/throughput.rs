mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use atlas_core::{AnalysisInput, LanguageAnalyzer};
use atlas_lang_rust::{DEFAULT_MAX_SOURCE_BYTES, RustAnalyzer};
use common::file_ref;

const MAX_FILES: usize = 3_000;

fn rust_files(root: &Path, limit: usize, found: &mut Vec<PathBuf>) {
  let mut pending = vec![root.to_path_buf()];
  while let Some(directory) = pending.pop() {
    let Ok(entries) = std::fs::read_dir(&directory) else { continue };
    let mut entries: Vec<PathBuf> = entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect();
    entries.sort();
    for path in entries {
      if found.len() >= limit {
        return;
      }
      if path.is_dir() && path.file_name().is_some_and(|name| name != "target" && name != ".git") {
        pending.push(path);
      } else if path.extension().is_some_and(|extension| extension == "rs") {
        found.push(path);
      }
    }
  }
}

#[test]
#[ignore = "throughput measurement; run with --release -- --ignored --nocapture"]
fn should_report_analysis_throughput_over_real_rust_sources() {
  let mut files = Vec::new();
  rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates"), MAX_FILES, &mut files);
  if let Some(home) = std::env::var_os("HOME") {
    rust_files(&Path::new(&home).join(".cargo/registry/src"), MAX_FILES, &mut files);
  }
  let analyzer = RustAnalyzer::new();
  let (mut lines, mut bytes, mut analyzed, mut partial) = (0usize, 0usize, 0usize, 0usize);
  let mut elapsed = Duration::ZERO;
  for path in &files {
    let Ok(source) = std::fs::read_to_string(path) else { continue };
    if source.len() > DEFAULT_MAX_SOURCE_BYTES {
      continue;
    }
    let file = file_ref("src/lib.rs", &source);
    let started = Instant::now();
    let facts = analyzer.analyze(&AnalysisInput { file: &file, source: &source }).unwrap();
    elapsed += started.elapsed();
    facts.check_invariants(source.len()).unwrap();
    analyzed += 1;
    lines += source.lines().count();
    bytes += source.len();
    partial += usize::from(!matches!(facts.parse_status, atlas_core::ParseStatus::Ok));
  }
  let seconds = elapsed.as_secs_f64();
  println!(
    "analyzed {analyzed} files ({partial} partial), {lines} LOC, {bytes} bytes in {seconds:.3}s: {:.0} LOC/s, {:.1} MiB/s",
    lines as f64 / seconds,
    bytes as f64 / seconds / 1_048_576.0
  );
  assert!(analyzed > 0);
}
