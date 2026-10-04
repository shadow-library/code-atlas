use crate::compare::{Comparison, ComparisonEntry, Verdict};
use crate::report::{Outcome, PerfReport, ScenarioRecord};

const MB: f64 = 1024.0 * 1024.0;
const TOP_PHASES: usize = 6;
const QUERY_KINDS: [&str; 8] = ["symbol", "callers", "impact", "search", "similar", "context", "api", "unused"];
const INCREMENTAL_SCENARIOS: [&str; 8] = [
  "incremental_1_files",
  "incremental_10_files",
  "incremental_100_files",
  "incremental_1000_files",
  "rename_symbol",
  "file_move",
  "branch_switch",
  "delete_directory",
];

fn find<'a>(report: &'a PerfReport, corpus: &str, scenario: &str) -> Option<&'a ScenarioRecord> {
  report.records.iter().find(|record| record.corpus == corpus && record.scenario == scenario)
}

fn metric(record: Option<&ScenarioRecord>, name: &str) -> Option<f64> {
  record.filter(|record| record.is_ok()).and_then(|record| record.metrics.get(name).copied())
}

fn cell(value: Option<f64>, render: impl Fn(f64) -> String) -> String {
  value.map(render).unwrap_or_else(|| "-".to_owned())
}

fn ms(value: f64) -> String {
  if value >= 10_000.0 {
    format!("{:.1} s", value / 1000.0)
  } else if value >= 100.0 {
    format!("{value:.0} ms")
  } else {
    format!("{value:.1} ms")
  }
}

fn mb(value: f64) -> String {
  format!("{:.1} MB", value / MB)
}

fn int(value: f64) -> String {
  format!("{value:.0}")
}

fn info_u64(record: Option<&ScenarioRecord>, name: &str) -> Option<f64> {
  record.and_then(|record| record.info.get(name)).and_then(serde_json::Value::as_f64)
}

fn spread_cell(record: Option<&ScenarioRecord>, name: &str, render: impl Fn(f64) -> String) -> String {
  let bounds = record
    .and_then(|record| record.info.get("spread"))
    .and_then(|spread| spread.get(name))
    .and_then(serde_json::Value::as_array);
  match bounds.map(Vec::as_slice) {
    Some([low, high]) => format!("{}-{}", render(low.as_f64().unwrap_or(0.0)), render(high.as_f64().unwrap_or(0.0))),
    _ => "-".to_owned(),
  }
}

fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
  let mut out = String::new();
  out.push_str(&format!("| {} |\n", headers.join(" | ")));
  out.push_str(&format!("|{}|\n", headers.iter().map(|_| "---").collect::<Vec<_>>().join("|")));
  for row in rows {
    out.push_str(&format!("| {} |\n", row.join(" | ")));
  }
  out.push('\n');
  out
}

fn corpus_names(report: &PerfReport) -> Vec<&str> {
  report.corpora.iter().map(|corpus| corpus.name.as_str()).collect()
}

fn environment(report: &PerfReport) -> String {
  let env = &report.environment;
  format!(
    "- Date: {}\n- Suite: `{}`\n- Platform: {} / {}, {} logical CPUs, rayon threads: {}\n- Build profile: {}\n- Resource usage from: `{}` on a child process per scenario group\n- Total harness time: {:.0} s\n\n",
    report.date, report.suite, env.os, env.arch, env.logical_cpus, env.rayon_threads, env.build_profile, env.usage_source, report.total_seconds
  )
}

fn corpora(report: &PerfReport) -> String {
  let rows: Vec<Vec<String>> = report
    .corpora
    .iter()
    .map(|corpus| {
      vec![
        corpus.name.clone(),
        corpus.kind.clone(),
        corpus
          .commit
          .as_deref()
          .map(|commit| commit[..commit.len().min(12)].to_owned())
          .unwrap_or_else(|| "-".to_owned()),
        corpus.repos.to_string(),
        corpus.files.to_string(),
        corpus.loc.to_string(),
        corpus.skipped.clone().unwrap_or_default(),
      ]
    })
    .collect();
  format!("## Corpora\n\n{}", table(&["corpus", "kind", "commit", "repos", "source files", "LOC", "skipped"], &rows))
}

fn cold(report: &PerfReport) -> String {
  let rows: Vec<Vec<String>> = corpus_names(report)
    .into_iter()
    .filter_map(|name| {
      let record = find(report, name, "cold_index").filter(|record| record.is_ok())?;
      let cpu = metric(Some(record), "cpu_total_s");
      Some(vec![
        name.to_owned(),
        format!("{} ({})", cell(metric(Some(record), "index_ms"), ms), spread_cell(Some(record), "index_ms", ms)),
        cell(metric(Some(record), "files_per_s"), int),
        cell(metric(Some(record), "loc_per_s"), int),
        cell(metric(Some(record), "peak_rss_bytes"), mb),
        cell(cpu, |value| format!("{value:.1} s")),
        cell(metric(Some(record), "cpu_utilisation"), |value| format!("{value:.1}x")),
        cell(metric(Some(record), "db_bytes"), mb),
        cell(metric(Some(record), "similarity_blob_bytes"), mb),
        cell(metric(Some(record), "persist_similarity_ms"), ms),
        format!(
          "{}/{}",
          info_u64(Some(record), "parse_failed").unwrap_or(0.0),
          info_u64(Some(record), "parse_partial").unwrap_or(0.0)
        ),
      ])
    })
    .collect();
  format!(
    "## Cold index\n\nIndex time is the median of 3 runs (min-max in brackets) of the run ledger duration of `IndexMode::Full` in a fresh process on an empty database; the OS page cache is warm. Peak RSS and CPU are the median for the whole cold child process. CPU utilisation is CPU seconds divided by wall seconds.\n\n{}",
    table(
      &[
        "corpus",
        "index",
        "files/s",
        "LOC/s",
        "peak RSS",
        "CPU",
        "CPU/wall",
        "DB",
        "similarity blob",
        "persist sim.",
        "parse failed/partial"
      ],
      &rows
    )
  )
}

fn phase_pairs(record: &ScenarioRecord) -> Vec<(String, f64)> {
  let mut phases: Vec<(String, f64)> = record
    .metrics
    .iter()
    .filter_map(|(name, value)| {
      name
        .strip_prefix("phase_")
        .and_then(|rest| rest.strip_suffix("_ms"))
        .map(|phase| (phase.to_owned(), *value))
    })
    .collect();
  phases.sort_by(|left, right| right.1.total_cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
  phases
}

fn phases(report: &PerfReport) -> String {
  let mut out = String::from("## Cold index phases\n\nShare is of the summed phase time; the phases are timed sequentially by the run ledger.\n\n");
  for name in corpus_names(report) {
    let Some(record) = find(report, name, "cold_index").filter(|record| record.is_ok()) else {
      continue;
    };
    let phases = phase_pairs(record);
    let total: f64 = phases.iter().map(|(_, value)| value).sum();
    if total <= 0.0 {
      continue;
    }
    let rows: Vec<Vec<String>> = phases
      .iter()
      .take(TOP_PHASES)
      .map(|(phase, value)| vec![phase.clone(), ms(*value), format!("{:.1}%", value / total * 100.0)])
      .collect();
    out.push_str(&format!("### {name}\n\n{}", table(&["phase", "time", "share"], &rows)));
  }
  out
}

fn warm(report: &PerfReport) -> String {
  let rows: Vec<Vec<String>> = corpus_names(report)
    .into_iter()
    .filter(|name| find(report, name, "warm_startup").is_some_and(|record| record.is_ok()))
    .map(|name| {
      let warm = find(report, name, "warm_startup");
      let barrier = find(report, name, "no_change_barrier");
      vec![
        name.to_owned(),
        cell(metric(warm, "open_ms"), ms),
        cell(metric(warm, "first_barrier_ms"), ms),
        cell(metric(barrier, "barrier_p50_ms"), ms),
        cell(metric(barrier, "barrier_p95_ms"), ms),
        cell(metric(barrier, "barrier_max_ms"), ms),
      ]
    })
    .collect();
  format!(
    "## Warm startup and no-change barrier\n\n`Engine::open` includes rebuilding the symbol table from persisted surfaces. The barrier is `ensure_fresh` with nothing changed, 50 iterations, no watcher (every call is a full stat scan).\n\n{}",
    table(&["corpus", "Engine::open", "first barrier", "barrier p50", "barrier p95", "barrier max"], &rows)
  )
}

fn incremental(report: &PerfReport) -> String {
  let mut rows = Vec::new();
  for name in corpus_names(report) {
    for scenario in INCREMENTAL_SCENARIOS {
      let Some(record) = find(report, name, scenario) else { continue };
      let detail = match &record.outcome {
        Outcome::Ok => String::new(),
        Outcome::Skipped { reason } => format!("skipped: {reason}"),
        Outcome::Failed { reason } => format!("failed: {reason}"),
      };
      rows.push(vec![
        name.to_owned(),
        scenario.to_owned(),
        cell(metric(Some(record), "latency_ms"), ms),
        cell(info_u64(Some(record), "files_touched"), int),
        cell(info_u64(Some(record), "entities_changed"), int),
        cell(info_u64(Some(record), "edges_changed"), int),
        cell(info_u64(Some(record), "renames_detected"), int),
        match (&record.outcome, record.info.get("files_mismatch").and_then(serde_json::Value::as_bool)) {
          (Outcome::Ok, Some(true)) => format!("FILE COUNT MISMATCH: run changed {}", cell(info_u64(Some(record), "files_changed_in_run"), int)),
          _ => detail,
        },
      ]);
    }
  }
  format!(
    "## Incremental updates\n\nLatency is one `ensure_fresh` that detects and applies the change. Entities and edges are the run ledger's added, removed and changed counts. The edges column counts edges rewritten because their evidence content hash changed; it is not semantic churn. Each scenario starts from a settled index, and the note flags any run whose changed-file count differs from the files touched.\n\n{}",
    table(
      &[
        "corpus",
        "scenario",
        "latency",
        "files touched",
        "entities changed",
        "edges rewritten (evidence content hash)",
        "renames detected",
        "note"
      ],
      &rows
    )
  )
}

fn query_rows(report: &PerfReport, suffix: &str) -> Vec<Vec<String>> {
  corpus_names(report)
    .into_iter()
    .filter_map(|name| {
      let record = find(report, name, "query_latency").filter(|record| record.is_ok())?;
      let barrier = metric(find(report, name, "no_change_barrier"), "barrier_p50_ms");
      let mut row = vec![name.to_owned(), cell(barrier, |value| format!("{value:.1}"))];
      for kind in QUERY_KINDS {
        let p50 = metric(Some(record), &format!("{kind}{suffix}_p50_ms"));
        let p95 = metric(Some(record), &format!("{kind}{suffix}_p95_ms"));
        let max = metric(Some(record), &format!("{kind}{suffix}_max_ms"));
        row.push(match (p50, p95, max) {
          (Some(p50), Some(p95), _) => format!("{p50:.1} / {p95:.1}"),
          (Some(p50), None, Some(max)) => format!("{p50:.1} / max {max:.1}"),
          _ => "-".to_owned(),
        });
      }
      Some(row)
    })
    .collect()
}

fn queries(report: &PerfReport) -> String {
  let mut headers = vec!["corpus", "barrier p50"];
  headers.extend(QUERY_KINDS);
  format!(
    "## Query latency (p50 / p95, ms)\n\n`impact` uses depth 3, `context` a 6000 token budget. 30 deterministic targets (hubs plus a stride sample). With fewer than 20 samples there is no p95: the cell shows the max instead (`api` and `unused` take 3 samples, are abandoned after 60 s, and a kind stops sampling once it has spent 30 s). `unused` is left out of the concurrency mix.\n\n### Including the freshness barrier\n\nEach call runs the barrier first; the second column is the no-change barrier p50.\n\n{}### Query alone (barrier skipped)\n\nThe same calls through `QueryEngine::without_barrier`, which answers from the current generation without scanning.\n\n{}",
    table(&headers, &query_rows(report, "")),
    table(&headers, &query_rows(report, "_nobarrier"))
  )
}

fn concurrency(report: &PerfReport) -> String {
  let mut out = String::from(
    "## Concurrency\n\nMixed query workload against one engine in 5 s windows, repeated 3 times; throughput in queries per second and p95 latency per query, median of the 3 windows. The edit-stream variant runs last, after the incremental scenarios.\n\n",
  );
  for (scenario, title) in [("concurrency", "Read-only"), ("concurrency_with_edits", "With a concurrent edit stream")] {
    let mut rows = Vec::new();
    for name in corpus_names(report) {
      let Some(record) = find(report, name, scenario).filter(|record| record.is_ok()) else {
        continue;
      };
      let mut row = vec![name.to_owned()];
      for threads in [1, 4, 8, 16] {
        let qps = metric(Some(record), &format!("n{threads}_throughput_qps"));
        let p95 = metric(Some(record), &format!("n{threads}_p95_ms"));
        row.push(match (qps, p95) {
          (Some(qps), Some(p95)) => format!("{qps:.0} qps / {p95:.1} ms"),
          _ => "-".to_owned(),
        });
      }
      rows.push(row);
    }
    out.push_str(&format!("### {title}\n\n{}", table(&["corpus", "N=1", "N=4", "N=8", "N=16"], &rows)));
  }
  out
}

fn multi(report: &PerfReport) -> String {
  let rows: Vec<Vec<String>> = report
    .records
    .iter()
    .filter(|record| record.scenario == "multi_project" && record.is_ok())
    .map(|record| {
      vec![
        record.corpus.clone(),
        cell(info_u64(Some(record), "projects"), int),
        cell(metric(Some(record), "total_ms"), ms),
        cell(info_u64(Some(record), "distinct_databases"), int),
        cell(info_u64(Some(record), "cross_talk_violations"), int),
      ]
    })
    .collect();
  if rows.is_empty() {
    return String::new();
  }
  format!(
    "## Multi-project\n\n{}",
    table(&["corpus", "projects", "total index time", "distinct databases", "cross-talk violations"], &rows)
  )
}

fn problems(report: &PerfReport) -> String {
  let rows: Vec<Vec<String>> = report
    .records
    .iter()
    .filter_map(|record| match &record.outcome {
      Outcome::Ok => None,
      Outcome::Skipped { reason } => Some(vec![record.corpus.clone(), record.scenario.clone(), "skipped".to_owned(), reason.clone()]),
      Outcome::Failed { reason } => Some(vec![record.corpus.clone(), record.scenario.clone(), "failed".to_owned(), reason.clone()]),
    })
    .collect();
  let mut rows = rows;
  for record in &report.records {
    for (name, value) in &record.info {
      if let Some(kind) = name.strip_suffix("_timed_out_after_s") {
        rows.push(vec![
          record.corpus.clone(),
          format!("{} {kind}", record.scenario),
          "timed out".to_owned(),
          format!("abandoned after {value} s"),
        ]);
      }
    }
  }
  if rows.is_empty() {
    return String::new();
  }
  format!("## Skipped and failed scenarios\n\n{}", table(&["corpus", "scenario", "status", "reason"], &rows))
}

fn profile(report: &PerfReport) -> String {
  let profile = &report.profile;
  let tool = profile.tool.as_deref().unwrap_or("none");
  let path = profile.path.as_deref().map(|path| format!("\n- Profile: `{path}`")).unwrap_or_default();
  format!("## Profiling\n\n- Tool: {tool}{path}\n- {}\n\n", profile.note)
}

fn limits() -> String {
  "## Limits of these numbers\n\n- Synthetic corpora are a relative scale test, not a realistic codebase: bodies are uniform arithmetic-shaped code, names come from a fixed word list, 4% of functions are planted near-duplicates and files average about 175 LOC. The synth-1m row in particular is not representative of a real 1M LOC codebase. ripgrep and hono are the realistic data points.\n- \"Cold\" means a fresh process and an empty database, but a warm OS page cache.\n- The session worker's peak RSS mixes warm-open, query and edit workloads.\n- Cold index and each concurrency level are medians of 3 runs; min-max is stored in each record's `spread` info.\n\n".to_owned()
}

fn describe_entry(entry: &ComparisonEntry) -> Vec<String> {
  let change = entry.change_pct.map(|pct| format!("{pct:+.1}%")).unwrap_or_else(|| "n/a".to_owned());
  let verdict = match entry.verdict {
    Verdict::Regression => format!("**REGRESSION {change}**"),
    Verdict::Improvement => format!("improvement {change}"),
    Verdict::Unchanged => change,
  };
  vec![
    entry.corpus.clone(),
    entry.scenario.clone(),
    entry.metric.clone(),
    format!("{:.1}", entry.previous),
    format!("{:.1}", entry.current),
    verdict,
  ]
}

fn comparison(comparison: &Comparison) -> String {
  let regressions: Vec<Vec<String>> = comparison.regressions().map(describe_entry).collect();
  let improvements: Vec<Vec<String>> = comparison.improvements().map(describe_entry).collect();
  let headers = ["corpus", "scenario", "metric", "previous", "current", "change"];
  let mut out = format!(
    "## Comparison with the previous report\n\nThreshold: {:.0}%. Regressions: {}. Improvements: {}. Absolute changes below 2.0 in the metric's unit and every `_max_ms` metric are ignored as noise. Two runs of identical code on a quiet machine still differ by 10-30% on millisecond-scale metrics, so confirm a regression with a second run.\n\n",
    comparison.threshold_pct,
    regressions.len(),
    improvements.len()
  );
  if !regressions.is_empty() {
    out.push_str(&format!("### Regressions\n\n{}", table(&headers, &regressions)));
  }
  if !improvements.is_empty() {
    out.push_str(&format!("### Improvements\n\n{}", table(&headers, &improvements)));
  }
  if !comparison.only_in_previous.is_empty() {
    out.push_str(&format!("Only in the previous report: {}\n\n", comparison.only_in_previous.join(", ")));
  }
  if !comparison.only_in_current.is_empty() {
    out.push_str(&format!("Only in this report: {}\n\n", comparison.only_in_current.join(", ")));
  }
  out
}

pub fn render(report: &PerfReport) -> String {
  let mut out = format!("# Performance report ({})\n\n", report.suite);
  out.push_str(&environment(report));
  for section in [
    corpora(report),
    cold(report),
    phases(report),
    warm(report),
    incremental(report),
    queries(report),
    concurrency(report),
    multi(report),
    problems(report),
    profile(report),
    limits(),
  ] {
    out.push_str(&section);
  }
  if let Some(found) = &report.comparison {
    out.push_str(&comparison(found));
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::compare::compare;
  use crate::report::{CorpusInfo, Environment, ProfileInfo, SCHEMA_VERSION};

  fn report(index_ms: f64) -> PerfReport {
    PerfReport {
      schema_version: SCHEMA_VERSION,
      suite: "quick".to_owned(),
      date: "2026-10-04".to_owned(),
      environment: Environment::detect(),
      corpora: vec![CorpusInfo {
        name: "self".to_owned(),
        kind: "self".to_owned(),
        commit: None,
        repos: 1,
        files: 10,
        loc: 1000,
        tree_hash: None,
        skipped: None,
      }],
      records: vec![
        ScenarioRecord::ok("self", "cold_index")
          .metric("index_ms", index_ms)
          .metric("phase_analyze_ms", index_ms * 0.6)
          .metric("phase_persist_ms", index_ms * 0.4),
      ],
      profile: ProfileInfo {
        tool: None,
        path: None,
        note: "per-phase timings only".to_owned(),
      },
      total_seconds: 1.0,
      comparison: None,
    }
  }

  #[test]
  fn should_rank_phases_by_time_in_the_phase_table() {
    let text = render(&report(1000.0));
    let analyze = text.find("| analyze |").unwrap();
    let persist = text.find("| persist |").unwrap();
    assert!(analyze < persist);
    assert!(text.contains("60.0%"));
  }

  #[test]
  fn should_highlight_regressions_in_the_comparison_section() {
    let mut current = report(1500.0);
    current.comparison = Some(compare(&report(1000.0), &current, 10.0).unwrap());
    let text = render(&current);
    assert!(text.contains("**REGRESSION +50.0%**"));
    assert!(text.contains("Regressions: 3."));
  }

  #[test]
  fn should_format_milliseconds_by_magnitude() {
    assert_eq!(ms(1.234), "1.2 ms");
    assert_eq!(ms(250.0), "250 ms");
    assert_eq!(ms(12_500.0), "12.5 s");
  }
}
