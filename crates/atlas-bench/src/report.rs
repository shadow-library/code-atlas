use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::compare::Comparison;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
  Ok,
  Skipped { reason: String },
  Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioRecord {
  pub corpus: String,
  pub scenario: String,
  pub outcome: Outcome,
  pub metrics: BTreeMap<String, f64>,
  pub info: BTreeMap<String, serde_json::Value>,
}

impl ScenarioRecord {
  fn with(corpus: &str, scenario: &str, outcome: Outcome) -> Self {
    Self {
      corpus: corpus.to_owned(),
      scenario: scenario.to_owned(),
      outcome,
      metrics: BTreeMap::new(),
      info: BTreeMap::new(),
    }
  }

  pub fn ok(corpus: &str, scenario: &str) -> Self {
    Self::with(corpus, scenario, Outcome::Ok)
  }

  pub fn skipped(corpus: &str, scenario: &str, reason: impl Into<String>) -> Self {
    Self::with(corpus, scenario, Outcome::Skipped { reason: reason.into() })
  }

  pub fn failed(corpus: &str, scenario: &str, reason: impl Into<String>) -> Self {
    Self::with(corpus, scenario, Outcome::Failed { reason: reason.into() })
  }

  pub fn metric(mut self, name: &str, value: f64) -> Self {
    self.metrics.insert(name.to_owned(), value);
    self
  }

  pub fn info(mut self, name: &str, value: impl Serialize) -> Self {
    self.info.insert(name.to_owned(), serde_json::to_value(value).unwrap_or(serde_json::Value::Null));
    self
  }

  pub fn is_ok(&self) -> bool {
    self.outcome == Outcome::Ok
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorpusInfo {
  pub name: String,
  pub kind: String,
  pub commit: Option<String>,
  pub repos: usize,
  pub files: u64,
  pub loc: u64,
  pub tree_hash: Option<String>,
  pub skipped: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
  pub os: String,
  pub arch: String,
  pub logical_cpus: usize,
  pub rayon_threads: usize,
  pub build_profile: String,
  pub usage_source: String,
}

impl Environment {
  pub fn detect() -> Self {
    Self {
      os: std::env::consts::OS.to_owned(),
      arch: std::env::consts::ARCH.to_owned(),
      logical_cpus: std::thread::available_parallelism().map(usize::from).unwrap_or(1),
      rayon_threads: rayon::current_num_threads(),
      build_profile: if cfg!(debug_assertions) { "debug" } else { "release" }.to_owned(),
      usage_source: if cfg!(target_os = "macos") { "/usr/bin/time -l" } else { "/usr/bin/time -v" }.to_owned(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileInfo {
  pub tool: Option<String>,
  pub path: Option<String>,
  pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerfReport {
  pub schema_version: u32,
  pub suite: String,
  pub date: String,
  pub environment: Environment,
  pub corpora: Vec<CorpusInfo>,
  pub records: Vec<ScenarioRecord>,
  pub profile: ProfileInfo,
  pub total_seconds: f64,
  pub comparison: Option<Comparison>,
}

pub fn civil_date(unix_seconds: u64) -> String {
  let days = (unix_seconds / 86_400) as i64;
  let shifted = days + 719_468;
  let era = shifted.div_euclid(146_097);
  let day_of_era = shifted.rem_euclid(146_097);
  let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
  let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
  let month_index = (5 * day_of_year + 2) / 153;
  let day = day_of_year - (153 * month_index + 2) / 5 + 1;
  let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
  let year = year_of_era + era * 400 + i64::from(month <= 2);
  format!("{year:04}-{month:02}-{day:02}")
}

pub fn today() -> String {
  let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0);
  civil_date(seconds)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_convert_unix_seconds_to_civil_dates() {
    assert_eq!(civil_date(0), "1970-01-01");
    assert_eq!(civil_date(951_782_400), "2000-02-29");
    assert_eq!(civil_date(1_791_072_000), "2026-10-04");
  }

  #[test]
  fn should_round_trip_a_record_through_json() {
    let record = ScenarioRecord::ok("self", "cold_index")
      .metric("wall_ms", 12.5)
      .info("phases", serde_json::json!({ "plan": 3 }));
    let text = serde_json::to_string(&record).unwrap();
    assert_eq!(serde_json::from_str::<ScenarioRecord>(&text).unwrap(), record);
  }

  #[test]
  fn should_tag_skipped_outcomes_with_their_reason() {
    let text = serde_json::to_string(&ScenarioRecord::skipped("hono", "cold_index", "offline")).unwrap();
    assert!(text.contains("\"status\":\"skipped\""));
    assert!(text.contains("\"reason\":\"offline\""));
  }
}
