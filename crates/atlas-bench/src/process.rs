use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{BenchError, BenchResult};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Usage {
  pub wall_s: f64,
  pub user_s: f64,
  pub sys_s: f64,
  pub peak_rss_bytes: u64,
}

impl Usage {
  pub fn cpu_s(&self) -> f64 {
    self.user_s + self.sys_s
  }
}

fn leading_number(line: &str) -> Option<f64> {
  line.split_whitespace().next()?.parse().ok()
}

fn trailing_number(line: &str) -> Option<f64> {
  line.rsplit(':').next()?.trim().parse().ok()
}

fn clock_seconds(text: &str) -> Option<f64> {
  text
    .split(':')
    .try_fold(0.0, |total, part| part.trim().parse::<f64>().ok().map(|value| total * 60.0 + value))
}

fn parse_bsd(stderr: &str) -> Option<Usage> {
  let mut usage = Usage {
    wall_s: 0.0,
    user_s: 0.0,
    sys_s: 0.0,
    peak_rss_bytes: 0,
  };
  let mut seen_times = false;
  let mut seen_rss = false;
  for line in stderr.lines() {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if let [real, "real", user, "user", sys, "sys"] = fields.as_slice() {
      usage.wall_s = real.parse().ok()?;
      usage.user_s = user.parse().ok()?;
      usage.sys_s = sys.parse().ok()?;
      seen_times = true;
    } else if line.trim_end().ends_with("maximum resident set size") {
      usage.peak_rss_bytes = leading_number(line)? as u64;
      seen_rss = true;
    }
  }
  (seen_times && seen_rss).then_some(usage)
}

fn parse_gnu(stderr: &str) -> Option<Usage> {
  let mut usage = Usage {
    wall_s: 0.0,
    user_s: 0.0,
    sys_s: 0.0,
    peak_rss_bytes: 0,
  };
  let mut found = 0;
  for line in stderr.lines().map(str::trim) {
    if line.starts_with("User time (seconds)") {
      usage.user_s = trailing_number(line)?;
      found += 1;
    } else if line.starts_with("System time (seconds)") {
      usage.sys_s = trailing_number(line)?;
      found += 1;
    } else if line.starts_with("Maximum resident set size (kbytes)") {
      usage.peak_rss_bytes = trailing_number(line)? as u64 * 1024;
      found += 1;
    } else if line.starts_with("Elapsed (wall clock) time") {
      usage.wall_s = clock_seconds(line.rsplit_once("): ")?.1)?;
      found += 1;
    }
  }
  (found == 4).then_some(usage)
}

pub fn parse_usage(stderr: &str) -> BenchResult<Usage> {
  parse_bsd(stderr)
    .or_else(|| parse_gnu(stderr))
    .ok_or_else(|| BenchError::Usage(stderr.lines().rev().take(6).collect::<Vec<_>>().join(" | ")))
}

pub fn run_measured(exe: &Path, args: &[String], mode: &str) -> BenchResult<(String, Usage)> {
  let flag = if cfg!(target_os = "macos") { "-l" } else { "-v" };
  let output = Command::new("/usr/bin/time")
    .arg(flag)
    .arg(exe)
    .args(args)
    .output()
    .map_err(|source| BenchError::io("spawn", Path::new("/usr/bin/time"), source))?;
  let stderr = String::from_utf8_lossy(&output.stderr);
  if !output.status.success() {
    let tail: Vec<&str> = stderr.lines().rev().take(12).collect();
    return Err(BenchError::Worker {
      mode: mode.to_owned(),
      detail: format!("{} :: {}", output.status, tail.into_iter().rev().collect::<Vec<_>>().join(" | ")),
    });
  }
  let usage = parse_usage(&stderr)?;
  Ok((String::from_utf8_lossy(&output.stdout).into_owned(), usage))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_parse_bsd_time_output() {
    let stderr =
      "worker noise\n        2.52 real        9.81 user         1.03 sys\n           412565504  maximum resident set size\n                   0  average shared memory size\n";
    assert_eq!(
      parse_usage(stderr).unwrap(),
      Usage {
        wall_s: 2.52,
        user_s: 9.81,
        sys_s: 1.03,
        peak_rss_bytes: 412_565_504
      }
    );
  }

  #[test]
  fn should_parse_gnu_time_output() {
    let stderr = "\tCommand being timed: \"x\"\n\tUser time (seconds): 3.50\n\tSystem time (seconds): 0.25\n\tElapsed (wall clock) time (h:mm:ss or m:ss): 1:02.50\n\tMaximum resident set size (kbytes): 2048\n";
    assert_eq!(
      parse_usage(stderr).unwrap(),
      Usage {
        wall_s: 62.5,
        user_s: 3.5,
        sys_s: 0.25,
        peak_rss_bytes: 2_097_152
      }
    );
  }

  #[test]
  fn should_reject_output_without_usage_lines() {
    assert!(matches!(parse_usage("nothing useful"), Err(BenchError::Usage(_))));
  }

  #[test]
  fn should_sum_user_and_system_time() {
    let usage = Usage {
      wall_s: 1.0,
      user_s: 2.0,
      sys_s: 0.5,
      peak_rss_bytes: 0,
    };
    assert_eq!(usage.cpu_s(), 2.5);
  }
}
