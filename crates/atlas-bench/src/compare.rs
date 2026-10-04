use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::report::{PerfReport, ScenarioRecord};

pub const REGRESSION_THRESHOLD_PCT: f64 = 10.0;
const NOISE_FLOOR: f64 = 2.0;
const HIGHER_IS_BETTER_SUFFIXES: [&str; 2] = ["_per_s", "_qps"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
  Regression,
  Improvement,
  Unchanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparisonEntry {
  pub corpus: String,
  pub scenario: String,
  pub metric: String,
  pub previous: f64,
  pub current: f64,
  pub change_pct: Option<f64>,
  pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
  pub threshold_pct: f64,
  pub entries: Vec<ComparisonEntry>,
  pub only_in_previous: Vec<String>,
  pub only_in_current: Vec<String>,
}

impl Comparison {
  pub fn regressions(&self) -> impl Iterator<Item = &ComparisonEntry> {
    self.entries.iter().filter(|entry| entry.verdict == Verdict::Regression)
  }

  pub fn improvements(&self) -> impl Iterator<Item = &ComparisonEntry> {
    self.entries.iter().filter(|entry| entry.verdict == Verdict::Improvement)
  }
}

fn is_extreme(metric: &str) -> bool {
  metric.ends_with("_max_ms")
}

pub fn higher_is_better(metric: &str) -> bool {
  HIGHER_IS_BETTER_SUFFIXES.iter().any(|suffix| metric.ends_with(suffix))
}

fn judge(metric: &str, previous: f64, current: f64, threshold_pct: f64) -> (Option<f64>, Verdict) {
  if previous == 0.0 {
    return (None, Verdict::Unchanged);
  }
  let change_pct = (current - previous) / previous * 100.0;
  if (current - previous).abs() < NOISE_FLOOR || change_pct.abs() <= threshold_pct {
    return (Some(change_pct), Verdict::Unchanged);
  }
  let worse = if higher_is_better(metric) { current < previous } else { current > previous };
  (Some(change_pct), if worse { Verdict::Regression } else { Verdict::Improvement })
}

fn index(records: &[ScenarioRecord]) -> BTreeMap<(&str, &str), &ScenarioRecord> {
  records
    .iter()
    .filter(|record| record.is_ok())
    .map(|record| ((record.corpus.as_str(), record.scenario.as_str()), record))
    .collect()
}

fn spread_pct(record: &ScenarioRecord, metric: &str, median: f64) -> f64 {
  let bounds = record.info.get("spread").and_then(|spread| spread.get(metric)).and_then(serde_json::Value::as_array);
  let Some([low, high]) = bounds.map(Vec::as_slice) else { return 0.0 };
  match (low.as_f64(), high.as_f64()) {
    (Some(low), Some(high)) if median != 0.0 => (high - low).abs() / median.abs() * 100.0,
    _ => 0.0,
  }
}

fn mismatch(previous: &PerfReport, current: &PerfReport) -> Option<String> {
  let (before, after) = (&previous.environment, &current.environment);
  let differences = [
    ("suite", previous.suite != current.suite),
    ("schema version", previous.schema_version != current.schema_version),
    ("os", before.os != after.os),
    ("arch", before.arch != after.arch),
    ("logical CPUs", before.logical_cpus != after.logical_cpus),
    ("rayon threads", before.rayon_threads != after.rayon_threads),
    ("build profile", before.build_profile != after.build_profile),
  ];
  let names: Vec<&str> = differences.iter().filter(|(_, differs)| *differs).map(|(name, _)| *name).collect();
  (!names.is_empty()).then(|| format!("the reports are not comparable, they differ in: {}", names.join(", ")))
}

pub fn compare(previous: &PerfReport, current: &PerfReport, threshold_pct: f64) -> Result<Comparison, String> {
  if let Some(reason) = mismatch(previous, current) {
    return Err(reason);
  }
  let before = index(&previous.records);
  let after = index(&current.records);
  let mut entries = Vec::new();
  for ((corpus, scenario), now) in &after {
    let Some(then) = before.get(&(*corpus, *scenario)) else { continue };
    for (metric, current_value) in now.metrics.iter().filter(|(metric, _)| !is_extreme(metric)) {
      let Some(previous_value) = then.metrics.get(metric) else { continue };
      let observed = spread_pct(then, metric, *previous_value).max(spread_pct(now, metric, *current_value));
      let (change_pct, verdict) = judge(metric, *previous_value, *current_value, threshold_pct.max(observed));
      entries.push(ComparisonEntry {
        corpus: (*corpus).to_owned(),
        scenario: (*scenario).to_owned(),
        metric: metric.clone(),
        previous: *previous_value,
        current: *current_value,
        change_pct,
        verdict,
      });
    }
  }
  let label = |(corpus, scenario): &(&str, &str)| format!("{corpus}/{scenario}");
  Ok(Comparison {
    threshold_pct,
    entries,
    only_in_previous: before.keys().filter(|key| !after.contains_key(*key)).map(label).collect(),
    only_in_current: after.keys().filter(|key| !before.contains_key(*key)).map(label).collect(),
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::report::{Environment, ProfileInfo, SCHEMA_VERSION};

  fn compare_ok(previous: &PerfReport, current: &PerfReport, threshold: f64) -> Comparison {
    compare(previous, current, threshold).unwrap()
  }

  fn report(records: Vec<ScenarioRecord>) -> PerfReport {
    PerfReport {
      schema_version: SCHEMA_VERSION,
      suite: "quick".to_owned(),
      date: "2026-10-04".to_owned(),
      environment: Environment::detect(),
      corpora: Vec::new(),
      records,
      profile: ProfileInfo {
        tool: None,
        path: None,
        note: String::new(),
      },
      total_seconds: 0.0,
      comparison: None,
    }
  }

  fn record(wall_ms: f64, files_per_s: f64) -> ScenarioRecord {
    ScenarioRecord::ok("self", "cold_index").metric("wall_ms", wall_ms).metric("files_per_s", files_per_s)
  }

  fn verdicts(comparison: &Comparison) -> BTreeMap<&str, Verdict> {
    comparison.entries.iter().map(|entry| (entry.metric.as_str(), entry.verdict)).collect()
  }

  #[test]
  fn should_flag_slower_latency_and_lower_throughput_above_the_threshold() {
    let comparison = compare_ok(&report(vec![record(1000.0, 500.0)]), &report(vec![record(1200.0, 400.0)]), 10.0);
    let found = verdicts(&comparison);
    assert_eq!(found["wall_ms"], Verdict::Regression);
    assert_eq!(found["files_per_s"], Verdict::Regression);
    assert_eq!(comparison.regressions().count(), 2);
  }

  #[test]
  fn should_flag_faster_latency_and_higher_throughput_as_improvements() {
    let comparison = compare_ok(&report(vec![record(1000.0, 500.0)]), &report(vec![record(800.0, 700.0)]), 10.0);
    let found = verdicts(&comparison);
    assert_eq!(found["wall_ms"], Verdict::Improvement);
    assert_eq!(found["files_per_s"], Verdict::Improvement);
  }

  #[test]
  fn should_ignore_changes_at_or_below_the_threshold() {
    let comparison = compare_ok(&report(vec![record(1000.0, 500.0)]), &report(vec![record(1100.0, 450.0)]), 10.0);
    assert!(comparison.entries.iter().all(|entry| entry.verdict == Verdict::Unchanged));
  }

  #[test]
  fn should_ignore_absolute_changes_below_the_noise_floor() {
    let comparison = compare_ok(&report(vec![record(1.0, 500.0)]), &report(vec![record(2.9, 500.0)]), 10.0);
    assert_eq!(verdicts(&comparison)["wall_ms"], Verdict::Unchanged);
  }

  #[test]
  fn should_leave_single_sample_maxima_out_of_the_comparison() {
    let previous = report(vec![
      ScenarioRecord::ok("self", "query_latency").metric("symbol_p95_ms", 10.0).metric("symbol_max_ms", 10.0),
    ]);
    let current = report(vec![
      ScenarioRecord::ok("self", "query_latency").metric("symbol_p95_ms", 10.0).metric("symbol_max_ms", 90.0),
    ]);
    let comparison = compare_ok(&previous, &current, 10.0);
    assert_eq!(comparison.entries.iter().map(|entry| entry.metric.as_str()).collect::<Vec<_>>(), vec!["symbol_p95_ms"]);
  }

  #[test]
  fn should_report_the_change_percentage() {
    let comparison = compare_ok(&report(vec![record(1000.0, 500.0)]), &report(vec![record(1250.0, 500.0)]), 10.0);
    let entry = comparison.entries.iter().find(|entry| entry.metric == "wall_ms").unwrap();
    assert_eq!(entry.change_pct, Some(25.0));
  }

  #[test]
  fn should_list_scenarios_present_in_only_one_report() {
    let previous = report(vec![record(10.0, 1.0)]);
    let current = report(vec![ScenarioRecord::ok("hono", "cold_index").metric("wall_ms", 5.0)]);
    let comparison = compare_ok(&previous, &current, 10.0);
    assert_eq!(comparison.only_in_previous, vec!["self/cold_index".to_owned()]);
    assert_eq!(comparison.only_in_current, vec!["hono/cold_index".to_owned()]);
    assert!(comparison.entries.is_empty());
  }

  #[test]
  fn should_skip_records_that_did_not_complete() {
    let previous = report(vec![ScenarioRecord::skipped("self", "cold_index", "offline")]);
    let comparison = compare_ok(&previous, &report(vec![record(10.0, 1.0)]), 10.0);
    assert!(comparison.entries.is_empty());
  }

  #[test]
  fn should_not_judge_a_metric_whose_previous_value_was_zero() {
    let comparison = compare_ok(&report(vec![record(0.0, 500.0)]), &report(vec![record(50.0, 500.0)]), 10.0);
    let entry = comparison.entries.iter().find(|entry| entry.metric == "wall_ms").unwrap();
    assert_eq!((entry.change_pct, entry.verdict), (None, Verdict::Unchanged));
  }

  #[test]
  fn should_widen_the_threshold_to_the_observed_spread() {
    let spread = |low: f64, high: f64| serde_json::json!({ "wall_ms": [low, high] });
    let previous = report(vec![record(1000.0, 500.0).info("spread", spread(800.0, 1200.0))]);
    let current = report(vec![record(1250.0, 500.0)]);
    assert_eq!(verdicts(&compare_ok(&previous, &current, 10.0))["wall_ms"], Verdict::Unchanged);
    let tight = report(vec![record(1000.0, 500.0).info("spread", spread(990.0, 1010.0))]);
    assert_eq!(verdicts(&compare_ok(&tight, &current, 10.0))["wall_ms"], Verdict::Regression);
  }

  #[test]
  fn should_refuse_to_compare_reports_from_different_suites_or_environments() {
    let previous = report(vec![record(1.0, 1.0)]);
    let mut other_suite = report(vec![record(1.0, 1.0)]);
    other_suite.suite = "full".to_owned();
    assert!(compare(&previous, &other_suite, 10.0).unwrap_err().contains("suite"));
    let mut other_machine = report(vec![record(1.0, 1.0)]);
    other_machine.environment.logical_cpus += 1;
    assert!(compare(&previous, &other_machine, 10.0).unwrap_err().contains("logical CPUs"));
  }
}
