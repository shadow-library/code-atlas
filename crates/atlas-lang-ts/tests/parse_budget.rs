use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use atlas_core::{AnalysisInput, ContentHash, FileFacts, FileRef, LanguageAnalyzer, ParseStatus, RelPath, RepoName};
use atlas_lang_ts::{ParseBudget, TsAnalyzer};

const HANG_INPUTS: usize = 10;
const LIMIT: Duration = Duration::from_millis(200);
const STALL: Duration = Duration::from_millis(50);
const CANCELLATION_SLACK: Duration = Duration::from_secs(1);

fn file_ref(path: &str, source: &str) -> FileRef {
  FileRef::detect(RepoName::new("fixture").unwrap(), RelPath::new(path).unwrap(), ContentHash::of(source.as_bytes())).unwrap()
}

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

fn analyze_timed(analyzer: &TsAnalyzer, path: &str, source: &str) -> (FileFacts, Duration) {
  let file = file_ref(path, source);
  let started = Instant::now();
  let facts = analyzer.analyze(&AnalysisInput { file: &file, source }).unwrap();
  (facts, started.elapsed())
}

#[test]
fn should_fail_the_analysis_once_the_parse_outlasts_the_limit_and_parse_the_next_file_from_scratch() {
  let elements = (0..1_000).map(|index| index.to_string()).collect::<Vec<_>>().join(", ");
  let source = format!("export const values = [{elements}];\n");
  let exhausted = TsAnalyzer::new().with_parse_budget(ParseBudget::fixed(Duration::ZERO));

  let (cancelled, _) = analyze_timed(&exhausted, "src/values.ts", &source);
  let (next, _) = analyze_timed(&TsAnalyzer::new(), "src/values.ts", &source);

  assert_eq!(source.len(), 4_914);
  assert_eq!(
    cancelled,
    FileFacts::failed(file_ref("src/values.ts", &source), exhausted.producer(), "parse budget exceeded (0 ms, 4914 bytes)")
  );
  assert_eq!(next.parse_status, ParseStatus::Ok);
  assert_eq!(next.symbols.iter().map(|symbol| symbol.name.as_str()).collect::<Vec<_>>(), ["values"]);
}

#[test]
fn should_fail_the_analysis_once_tsx_error_recovery_outlasts_the_limit() {
  let analyzer = TsAnalyzer::new().with_parse_budget(ParseBudget::fixed(LIMIT));
  let source = include_str!("hangs/tsx-error-recovery-1.tsx.txt");

  let (facts, elapsed) = analyze_timed(&analyzer, "src/hang.tsx", source);

  assert_eq!(
    facts,
    FileFacts::failed(file_ref("src/hang.tsx", source), analyzer.producer(), "parse budget exceeded (200 ms, 349 bytes)")
  );
  assert!(elapsed >= LIMIT && elapsed < LIMIT + CANCELLATION_SLACK, "cancelled after {elapsed:?}");
}

#[test]
fn should_stop_every_known_tsx_error_recovery_loop_once_the_parse_stops_advancing() {
  let analyzer = TsAnalyzer::new().with_parse_budget(ParseBudget::default().with_stall_limit(STALL));
  for (name, source) in hang_inputs() {
    let (facts, elapsed) = analyze_timed(&analyzer, "src/hang.tsx", &source);

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
fn should_fail_the_analysis_with_the_same_reason_once_extraction_outlasts_the_limit() {
  let source = format!("{}x{}", "namespace a {".repeat(2_000), "}".repeat(2_000));
  let analyzer = TsAnalyzer::new().with_parse_budget(ParseBudget::fixed(LIMIT));

  let (facts, elapsed) = analyze_timed(&analyzer, "src/deep.ts", &source);

  assert_eq!(
    facts,
    FileFacts::failed(file_ref("src/deep.ts", &source), analyzer.producer(), "parse budget exceeded (200 ms, 28001 bytes)")
  );
  assert!(elapsed < LIMIT + CANCELLATION_SLACK, "cancelled after {elapsed:?}");
}
