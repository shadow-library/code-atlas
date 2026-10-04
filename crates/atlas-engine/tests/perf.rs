mod common;

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use atlas_engine::{Engine, RunSummary};
use common::{fresh, open, project, repo_config};

fn copy_tree(from: &Path, to: &Path) {
  fs::create_dir_all(to).unwrap();
  for entry in fs::read_dir(from).unwrap() {
    let entry = entry.unwrap();
    let name = entry.file_name();
    if matches!(name.to_str(), Some("target" | "node_modules" | ".git")) {
      continue;
    }
    let target = to.join(&name);
    match entry.file_type().unwrap().is_dir() {
      true => copy_tree(&entry.path(), &target),
      false => {
        fs::copy(entry.path(), &target).unwrap();
      }
    }
  }
}

fn median(mut samples: Vec<Duration>) -> Duration {
  samples.sort();
  samples[samples.len() / 2]
}

fn db_bytes(engine: &Engine) -> u64 {
  let db = engine.db_path();
  ["", "-wal", "-shm"]
    .iter()
    .filter_map(|suffix| {
      let mut path = db.clone().into_os_string();
      path.push(suffix);
      fs::metadata(path).ok().map(|metadata| metadata.len())
    })
    .sum()
}

fn last_run(engine: &Engine) -> atlas_store::IndexRun {
  engine.store().read(|tx| tx.runs(1)).unwrap().remove(0)
}

fn describe(label: &str, summary: Option<&RunSummary>, run: &atlas_store::IndexRun, elapsed: Duration) {
  eprintln!("PERF {label}: {elapsed:?}");
  eprintln!("PERF {label} phases_ms: {:?}", run.report.phase_timings_ms);
  eprintln!("PERF {label} counts: {:?}", run.report.counts);
  if let Some(summary) = summary {
    eprintln!("PERF {label} files_updated: {}", summary.files_updated);
  }
}

#[test]
#[ignore = "performance measurement on this workspace; run with --release -- --ignored --nocapture"]
fn should_measure_indexing_performance_on_this_workspace() {
  let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
  let scratch = tempfile::tempdir().unwrap();
  let crates = scratch.path().join("crates");
  let fixtures = scratch.path().join("fixtures");
  copy_tree(&workspace.join("crates"), &crates);
  copy_tree(&workspace.join("fixtures"), &fixtures);
  let project = project("atlas", vec![repo_config("crates", &crates), repo_config("fixtures", &fixtures)]);
  let home = scratch.path().join("home");

  let started = Instant::now();
  let engine = open(&project, &home);
  let opened = started.elapsed();
  let report = fresh(&engine);
  let cold = started.elapsed();
  assert!(report.header().run.is_some());
  let run = last_run(&engine);
  describe("cold", None, &run, cold);
  let files = run.report.counts["files_analyzed"];
  eprintln!(
    "PERF cold open_ms={} files={files} files_per_s={:.0}",
    opened.as_millis(),
    files as f64 / cold.as_secs_f64()
  );
  eprintln!("PERF db_bytes={}", db_bytes(&engine));

  let samples: Vec<Duration> = (0..25)
    .map(|_| {
      let started = Instant::now();
      let report = fresh(&engine);
      assert_eq!(report.header().run, None);
      started.elapsed()
    })
    .collect();
  eprintln!("PERF no_change_barrier median={:?} max={:?}", median(samples.clone()), samples.iter().max().unwrap());

  drop(engine);
  let started = Instant::now();
  let engine = open(&project, &home);
  let warm_open = started.elapsed();
  let report = fresh(&engine);
  eprintln!(
    "PERF warm open={warm_open:?} first_barrier={:?} run={:?}",
    started.elapsed() - warm_open,
    report.header().run
  );

  let targets = [
    crates.join("atlas-store/src/files.rs"),
    crates.join("atlas-core/src/span.rs"),
    fixtures.join("acme/acme-api/src/users/users.service.ts"),
    fixtures.join("acme/acme-web/src/utils/currency.ts"),
  ];
  for (round, target) in targets.iter().cycle().take(12).enumerate() {
    let mut text = fs::read_to_string(target).unwrap();
    let addition = match target.extension().and_then(|ext| ext.to_str()) {
      Some("rs") => format!("\npub fn perf_probe_{round}() -> u32 {{\n  {round}\n}}\n"),
      _ => format!("\nexport function perfProbe{round}() {{\n  return {round};\n}}\n"),
    };
    text.push_str(&addition);
    fs::write(target, text).unwrap();
    let started = Instant::now();
    let report = fresh(&engine);
    let elapsed = started.elapsed();
    assert!(report.header().run.is_some());
    let run = last_run(&engine);
    eprintln!(
      "PERF single_file_edit {} elapsed={elapsed:?} owners_resolved={} phases_ms={:?}",
      target.strip_prefix(scratch.path()).unwrap().display(),
      run.report.counts["owners_resolved"],
      run.report.phase_timings_ms
    );
  }
  eprintln!("PERF db_bytes_after_edits={}", db_bytes(&engine));
  let status = engine.status().unwrap();
  eprintln!("PERF files_by_language={:?} parse_status={:?}", status.files_by_language, status.files_by_parse_status);
}
