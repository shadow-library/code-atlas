mod common;

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use atlas_core::{AnalysisInput, FileFacts, LanguageAnalyzer, ParseStatus};
use atlas_lang_rust::{ParseBudget, RustAnalyzer};
use common::{file_ref, symbol_names};

const HANG_INPUTS: usize = 14;
const LIMIT: Duration = Duration::from_millis(200);
const STALL: Duration = Duration::from_millis(50);
const CANCELLATION_SLACK: Duration = Duration::from_secs(1);

fn hang_inputs() -> Vec<(String, String)> {
  let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/hangs");
  let mut inputs: Vec<(String, String)> = fs::read_dir(dir)
    .unwrap()
    .map(|entry| entry.unwrap().path())
    .map(|path| (path.file_name().unwrap().to_string_lossy().into_owned(), fs::read_to_string(&path).unwrap()))
    .collect();
  inputs.sort();
  assert_eq!(inputs.len(), HANG_INPUTS);
  inputs
}

fn analyze_timed(analyzer: &RustAnalyzer, path: &str, source: &str) -> (FileFacts, Duration) {
  let file = file_ref(path, source);
  let started = Instant::now();
  let facts = analyzer.analyze(&AnalysisInput { file: &file, source }).unwrap();
  (facts, started.elapsed())
}

#[test]
fn should_fail_the_analysis_once_error_recovery_outlasts_the_limit() {
  let analyzer = RustAnalyzer::new().with_parse_budget(ParseBudget::fixed(LIMIT));
  let source = include_str!("hangs/error-recovery-1.rs.txt");

  let (facts, elapsed) = analyze_timed(&analyzer, "src/hang.rs", source);

  assert_eq!(
    facts,
    FileFacts::failed(file_ref("src/hang.rs", source), analyzer.producer(), "parse budget exceeded (200 ms, 352 bytes)")
  );
  assert!(elapsed >= LIMIT && elapsed < LIMIT + CANCELLATION_SLACK, "cancelled after {elapsed:?}");
}

#[test]
fn should_stop_every_known_error_recovery_loop_once_the_parse_stops_advancing() {
  let analyzer = RustAnalyzer::new().with_parse_budget(ParseBudget::default().with_stall_limit(STALL));
  for (name, source) in hang_inputs() {
    let (facts, elapsed) = analyze_timed(&analyzer, "src/hang.rs", &source);

    let reason = format!(
      "parse budget exceeded ({} ms, {} bytes)",
      ParseBudget::default().limit_for(source.len()).as_millis(),
      source.len()
    );
    assert_eq!(facts.parse_status, ParseStatus::Failed { reason }, "{name}");
    assert!(elapsed < STALL + CANCELLATION_SLACK, "{name} cancelled after {elapsed:?}");
  }
}

#[test]
fn should_stop_a_loop_after_a_mebibyte_of_valid_code_long_before_the_size_scaled_limit() {
  let valid: String = (0..30_000).map(|index| format!("pub fn f{index}(x: u32) -> u32 {{ x + {index} }}\n")).collect();
  let source = format!("{valid}{}", include_str!("hangs/error-recovery-1.rs.txt"));
  assert!(source.len() > 1024 * 1024);
  let budget = ParseBudget::default().with_stall_limit(Duration::from_millis(200));
  let analyzer = RustAnalyzer::new().with_parse_budget(budget);

  let (facts, elapsed) = analyze_timed(&analyzer, "src/big.rs", &source);

  assert_eq!(
    facts.parse_status,
    ParseStatus::Failed {
      reason: budget.failure_reason(source.len())
    }
  );
  assert!(budget.limit_for(source.len()) > Duration::from_secs(12));
  assert!(elapsed < Duration::from_secs(6), "cancelled after {elapsed:?}");
}

#[test]
fn should_parse_the_next_file_from_scratch_after_a_cancelled_parse() {
  let analyzer = RustAnalyzer::new().with_parse_budget(ParseBudget::fixed(LIMIT));
  let (cancelled, _) = analyze_timed(&analyzer, "src/hang.rs", include_str!("hangs/error-recovery-1.rs.txt"));
  assert!(cancelled.parse_status.is_budget_exceeded(), "{:?}", cancelled.parse_status);

  let (facts, _) = analyze_timed(&analyzer, "src/lib.rs", "pub fn after() {}\n");

  assert_eq!(facts.parse_status, ParseStatus::Ok);
  assert_eq!(symbol_names(&facts), ["after#function"]);
}

#[test]
fn should_fail_the_analysis_with_the_same_reason_once_extraction_outlasts_the_limit() {
  let impls: String = (0..3_000).map(|index| format!("impl T{index} for S {{}}\n")).collect();
  let source = format!("{}{impls}", "struct S;\n".repeat(3_000));
  let analyzer = RustAnalyzer::new().with_parse_budget(ParseBudget::fixed(LIMIT));

  let (facts, elapsed) = analyze_timed(&analyzer, "src/lib.rs", &source);

  let reason = format!("parse budget exceeded (200 ms, {} bytes)", source.len());
  assert_eq!(facts, FileFacts::failed(file_ref("src/lib.rs", &source), analyzer.producer(), reason));
  assert!(elapsed < LIMIT + CANCELLATION_SLACK, "cancelled after {elapsed:?}");
}
