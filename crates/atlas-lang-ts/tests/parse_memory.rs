// Alone in its own test binary: cargo runs test binaries one at a time, so no other memory-heavy test runs beside the watched child.
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use atlas_core::{AnalysisInput, ContentHash, FileRef, LanguageAnalyzer, ParseStatus, RelPath, RepoName};
use atlas_lang_ts::{MAX_SOURCE_BYTES, ParseBudget, TsAnalyzer};

const CHILD_TEST: &str = "should_report_one_unterminated_run_when_spawned_as_a_watched_child";
const CASE_VAR: &str = "ATLAS_PARSE_MEMORY_CASE";
const REPORT_PREFIX: &str = "report:";
const RUN_PREFIX: &str = "const x = sql";
const RESIDENT_BASE_BYTES: u64 = 256 * 1024 * 1024;
const RESIDENT_BYTES_PER_SOURCE_BYTE: u64 = 1024;
const WATCH_INTERVAL: Duration = Duration::from_millis(10);
const WALL_BACKSTOP_FACTOR: u32 = 10;

fn run_units(case: &str) -> Option<usize> {
  match case {
    "33k" => Some(33_000),
    "60k" => Some(60_000),
    "2mib" => Some((MAX_SOURCE_BYTES - RUN_PREFIX.len()) / "<a>".len()),
    _ => None,
  }
}

fn unterminated_run(units: usize) -> String {
  format!("{RUN_PREFIX}{}", "<a>".repeat(units))
}

#[derive(Debug, PartialEq, Eq)]
struct Report {
  status: String,
  symbols: Vec<String>,
  references: usize,
}

enum ChildRun {
  Unwatchable,
  OverMemory {
    resident_bytes: u64,
  },
  OverTime {
    elapsed: Duration,
  },
  ExitedUnsampled {
    stderr: String,
  },
  Exited {
    report: Option<Report>,
    peak_resident_bytes: u64,
    stderr: String,
  },
}

enum Sample {
  Resident(u64),
  NotListed,
  PsUnavailable,
}

fn sample_resident(pid: u32) -> Sample {
  let Ok(output) = Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).stderr(Stdio::null()).output() else {
    return Sample::PsUnavailable;
  };
  match String::from_utf8_lossy(&output.stdout).trim().parse::<u64>() {
    Ok(kib) => Sample::Resident(kib * 1024),
    Err(_) => Sample::NotListed,
  }
}

fn parse_report(stdout: &str) -> Option<Report> {
  let line = stdout.lines().find_map(|line| line.split_once(REPORT_PREFIX))?.1;
  let [status, symbols, references] = line.split('\t').collect::<Vec<_>>()[..] else {
    return None;
  };
  Some(Report {
    status: status.to_owned(),
    symbols: symbols.split(',').filter(|name| !name.is_empty()).map(str::to_owned).collect(),
    references: references.parse().ok()?,
  })
}

fn run_watched(case: &str, resident_limit_bytes: u64, wall_limit: Duration) -> ChildRun {
  let started = Instant::now();
  let mut child = Command::new(std::env::current_exe().unwrap())
    .args([CHILD_TEST, "--exact", "--ignored", "--nocapture", "--test-threads=1"])
    .env(CASE_VAR, case)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
  let mut peak_resident_bytes: Option<u64> = None;
  while child.try_wait().unwrap().is_none() {
    let elapsed = started.elapsed();
    let stopped = match sample_resident(child.id()) {
      Sample::PsUnavailable => Some(ChildRun::Unwatchable),
      Sample::Resident(bytes) if bytes > resident_limit_bytes => Some(ChildRun::OverMemory { resident_bytes: bytes }),
      Sample::Resident(bytes) => {
        peak_resident_bytes = Some(peak_resident_bytes.unwrap_or(0).max(bytes));
        None
      }
      Sample::NotListed => None,
    }
    .or_else(|| (elapsed > wall_limit).then_some(ChildRun::OverTime { elapsed }));
    if let Some(stopped) = stopped {
      child.kill().unwrap();
      child.wait().unwrap();
      return stopped;
    }
    thread::sleep(WATCH_INTERVAL);
  }
  let output = child.wait_with_output().unwrap();
  let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
  match peak_resident_bytes {
    None => ChildRun::ExitedUnsampled { stderr },
    Some(peak_resident_bytes) => ChildRun::Exited {
      report: parse_report(&String::from_utf8_lossy(&output.stdout)),
      peak_resident_bytes,
      stderr,
    },
  }
}

#[test]
#[ignore = "spawned by should_analyse_unterminated_type_argument_runs_within_the_budget_and_bounded_memory"]
fn should_report_one_unterminated_run_when_spawned_as_a_watched_child() {
  let Some(units) = std::env::var(CASE_VAR).ok().and_then(|case| run_units(&case)) else {
    return;
  };
  let source = unterminated_run(units);
  let line = thread::Builder::new()
    .stack_size(2 * 1024 * 1024)
    .spawn(move || {
      let file = FileRef::detect(RepoName::new("fixture").unwrap(), RelPath::new("src/run.ts").unwrap(), ContentHash::of(source.as_bytes())).unwrap();
      let facts = TsAnalyzer::new().analyze(&AnalysisInput { file: &file, source: &source }).unwrap();
      let status = match &facts.parse_status {
        ParseStatus::Ok => "ok".to_owned(),
        ParseStatus::Partial { error_count, .. } => format!("partial {error_count}"),
        ParseStatus::Failed { reason } => format!("failed {reason}"),
      };
      let symbols: Vec<&str> = facts.symbols.iter().map(|symbol| symbol.name.as_str()).collect();
      format!("{REPORT_PREFIX}{status}\t{}\t{}", symbols.join(","), facts.references.len())
    })
    .unwrap()
    .join()
    .unwrap();
  println!("{line}");
}

// The analyzer charges its budget in CPU time and fails the file once it is spent, so an `ok` status proves the analysis finished within
// the budget whatever the machine load; the wall limit only stops a parse that never reaches a progress callback.
#[test]
fn should_analyse_unterminated_type_argument_runs_within_the_budget_and_bounded_memory() {
  for case in ["33k", "60k", "2mib"] {
    let units = run_units(case).unwrap();
    let source_bytes = unterminated_run(units).len();
    let resident_limit_bytes = RESIDENT_BASE_BYTES + RESIDENT_BYTES_PER_SOURCE_BYTE * u64::try_from(source_bytes).unwrap();
    let wall_limit = ParseBudget::default().limit_for(source_bytes) * WALL_BACKSTOP_FACTOR;

    let run = run_watched(case, resident_limit_bytes, wall_limit);

    let (report, peak_resident_bytes) = match run {
      ChildRun::Exited {
        report: Some(report),
        peak_resident_bytes,
        ..
      } => (report, peak_resident_bytes),
      ChildRun::Exited { report: None, stderr, .. } => panic!("{case}: the child reported nothing\n{stderr}"),
      ChildRun::ExitedUnsampled { stderr } => panic!("{case}: the child exited before ps sampled its resident memory\n{stderr}"),
      ChildRun::Unwatchable => panic!("{case}: ps could not be run, so the child's resident memory cannot be watched"),
      ChildRun::OverMemory { resident_bytes } => panic!("{case}: killed at {} MiB, over the {} MiB limit", resident_bytes >> 20, resident_limit_bytes >> 20),
      ChildRun::OverTime { elapsed } => panic!("{case}: killed after {elapsed:?}, over the {wall_limit:?} limit"),
    };
    assert_eq!(
      report,
      Report {
        status: "ok".to_owned(),
        symbols: vec!["x".to_owned()],
        references: units + 1,
      },
      "{case}"
    );
    assert!(peak_resident_bytes <= resident_limit_bytes, "{case}: peak {} MiB", peak_resident_bytes >> 20);
  }
}
