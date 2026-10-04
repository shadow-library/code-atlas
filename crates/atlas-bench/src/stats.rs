use std::time::Duration;

use serde::{Deserialize, Serialize};

pub fn millis(duration: Duration) -> f64 {
  duration.as_secs_f64() * 1000.0
}

pub fn percentile(sorted: &[f64], percent: f64) -> f64 {
  if sorted.is_empty() {
    return 0.0;
  }
  let rank = ((percent / 100.0) * sorted.len() as f64).ceil() as usize;
  sorted[rank.clamp(1, sorted.len()) - 1]
}

pub const MIN_PERCENTILE_SAMPLES: usize = 20;

pub fn median(samples: &[f64]) -> f64 {
  let mut sorted = samples.to_vec();
  sorted.sort_by(f64::total_cmp);
  percentile(&sorted, 50.0)
}

pub fn range(samples: &[f64]) -> (f64, f64) {
  let low = samples.iter().copied().fold(f64::INFINITY, f64::min);
  let high = samples.iter().copied().fold(f64::NEG_INFINITY, f64::max);
  (low, high)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
  pub count: usize,
  pub min: f64,
  pub p50: f64,
  pub p95: f64,
  pub max: f64,
  pub mean: f64,
}

impl Summary {
  pub fn of(samples: &[f64]) -> Self {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mean = if sorted.is_empty() { 0.0 } else { sorted.iter().sum::<f64>() / sorted.len() as f64 };
    Self {
      count: sorted.len(),
      min: sorted.first().copied().unwrap_or(0.0),
      p50: percentile(&sorted, 50.0),
      p95: percentile(&sorted, 95.0),
      max: sorted.last().copied().unwrap_or(0.0),
      mean,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_use_nearest_rank_percentiles() {
    let sorted = [1.0, 2.0, 3.0, 4.0];
    assert_eq!(percentile(&sorted, 50.0), 2.0);
    assert_eq!(percentile(&sorted, 95.0), 4.0);
    assert_eq!(percentile(&sorted, 100.0), 4.0);
    assert_eq!(percentile(&sorted, 0.0), 1.0);
  }

  #[test]
  fn should_pick_the_95th_of_a_hundred_samples() {
    let sorted: Vec<f64> = (1..=100).map(f64::from).collect();
    assert_eq!(percentile(&sorted, 50.0), 50.0);
    assert_eq!(percentile(&sorted, 95.0), 95.0);
  }

  #[test]
  fn should_return_zero_for_no_samples() {
    assert_eq!(percentile(&[], 95.0), 0.0);
    assert_eq!(Summary::of(&[]).count, 0);
  }

  #[test]
  fn should_summarise_unsorted_samples() {
    let summary = Summary::of(&[9.0, 1.0, 5.0, 3.0, 7.0]);
    assert_eq!(summary.count, 5);
    assert_eq!(summary.min, 1.0);
    assert_eq!(summary.p50, 5.0);
    assert_eq!(summary.p95, 9.0);
    assert_eq!(summary.max, 9.0);
    assert_eq!(summary.mean, 5.0);
  }

  #[test]
  fn should_take_the_median_and_range_of_repeats() {
    assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
    assert_eq!(range(&[3.0, 1.0, 2.0]), (1.0, 3.0));
  }

  #[test]
  fn should_convert_durations_to_milliseconds() {
    assert_eq!(millis(Duration::from_micros(1500)), 1.5);
  }
}
