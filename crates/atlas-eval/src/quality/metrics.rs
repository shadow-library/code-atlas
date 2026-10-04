use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Better {
  Higher,
  Lower,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Metric {
  pub value: f64,
  pub numerator: u64,
  pub denominator: u64,
  pub better: Better,
}

impl Metric {
  pub fn ratio(numerator: u64, denominator: u64, better: Better) -> Option<Self> {
    if denominator == 0 {
      return None;
    }
    Some(Self {
      value: numerator as f64 / denominator as f64,
      numerator,
      denominator,
      better,
    })
  }

  pub fn count(value: u64, better: Better) -> Self {
    Self {
      value: value as f64,
      numerator: value,
      denominator: 1,
      better,
    }
  }
}

pub fn ratio(numerator: u64, denominator: u64) -> Option<f64> {
  Metric::ratio(numerator, denominator, Better::Higher).map(|metric| metric.value)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recall {
  pub expected: u64,
  pub found: u64,
  pub wrong_certainty: u64,
  pub wrong_detail: u64,
  pub missing: u64,
  pub unbound: u64,
}

impl Recall {
  pub fn bound(&self) -> u64 {
    self.found + self.wrong_certainty + self.wrong_detail + self.missing
  }

  pub fn recall_over_bound(&self) -> Option<f64> {
    ratio(self.found, self.bound())
  }

  pub fn recall_over_all(&self) -> Option<f64> {
    ratio(self.found, self.expected)
  }

  pub fn existence_over_bound(&self) -> Option<f64> {
    ratio(self.found + self.wrong_certainty + self.wrong_detail, self.bound())
  }

  pub fn absorb(&mut self, other: &Recall) {
    self.expected += other.expected;
    self.found += other.found;
    self.wrong_certainty += other.wrong_certainty;
    self.wrong_detail += other.wrong_detail;
    self.missing += other.missing;
    self.unbound += other.unbound;
  }

  pub fn record(&mut self, outcome: &RecallOutcome) {
    self.expected += 1;
    match outcome {
      RecallOutcome::Found => self.found += 1,
      RecallOutcome::WrongCertainty { .. } => self.wrong_certainty += 1,
      RecallOutcome::WrongDetail { .. } => self.wrong_detail += 1,
      RecallOutcome::Missing { .. } => self.missing += 1,
      RecallOutcome::Unbound => self.unbound += 1,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RecallOutcome {
  Found,
  WrongCertainty { actual: String },
  WrongDetail { detail: String },
  Missing { detail: String },
  Unbound,
}

impl RecallOutcome {
  pub fn is_found(&self) -> bool {
    matches!(self, Self::Found)
  }

  pub fn describe(&self) -> String {
    match self {
      Self::Found => "found".to_owned(),
      Self::WrongCertainty { actual } => format!("wrong certainty: {actual}"),
      Self::WrongDetail { detail } => format!("wrong detail: {detail}"),
      Self::Missing { detail } => format!("missing: {detail}"),
      Self::Unbound => "unbound".to_owned(),
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prohibition {
  pub checked: u64,
  pub violations: u64,
  pub unbound: u64,
  pub vacuous: u64,
}

impl Prohibition {
  pub fn satisfaction(&self) -> Option<f64> {
    ratio(self.checked - self.violations, self.checked)
  }

  pub fn absorb(&mut self, other: &Prohibition) {
    self.checked += other.checked;
    self.violations += other.violations;
    self.unbound += other.unbound;
    self.vacuous += other.vacuous;
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassCalibration {
  pub judged: u64,
  pub correct: u64,
  pub overclaimed: u64,
  pub underclaimed: u64,
  pub false_edges: u64,
}

impl ClassCalibration {
  pub fn fraction_correct(&self) -> Option<f64> {
    ratio(self.correct, self.judged)
  }

  pub fn fraction_true_relation(&self) -> Option<f64> {
    ratio(self.judged - self.false_edges, self.judged)
  }

  pub fn absorb(&mut self, other: &ClassCalibration) {
    self.judged += other.judged;
    self.correct += other.correct;
    self.overclaimed += other.overclaimed;
    self.underclaimed += other.underclaimed;
    self.false_edges += other.false_edges;
  }
}

pub const CONFIDENCE_BUCKETS: [(f32, f32); 4] = [(0.0, 0.4), (0.4, 0.6), (0.6, 0.8), (0.8, 1.01)];

pub fn bucket_label(index: usize) -> String {
  let (low, high) = CONFIDENCE_BUCKETS[index];
  format!("[{low:.1}, {:.1}{}", high.min(1.0), if high > 1.0 { "]" } else { ")" })
}

pub fn bucket_index(confidence: f32) -> usize {
  CONFIDENCE_BUCKETS
    .iter()
    .position(|(low, high)| confidence >= *low && confidence < *high)
    .unwrap_or(CONFIDENCE_BUCKETS.len() - 1)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calibration {
  pub by_class: BTreeMap<String, ClassCalibration>,
  pub heuristic_buckets: Vec<ClassCalibration>,
}

impl Calibration {
  pub fn new() -> Self {
    Self {
      by_class: BTreeMap::new(),
      heuristic_buckets: vec![ClassCalibration::default(); CONFIDENCE_BUCKETS.len()],
    }
  }

  pub fn record(&mut self, class: &str, confidence: Option<f32>, verdict: JudgedEdge) {
    let apply = |cell: &mut ClassCalibration| {
      cell.judged += 1;
      match verdict {
        JudgedEdge::Correct => cell.correct += 1,
        JudgedEdge::Overclaimed => cell.overclaimed += 1,
        JudgedEdge::Underclaimed => cell.underclaimed += 1,
        JudgedEdge::False => cell.false_edges += 1,
      }
    };
    apply(self.by_class.entry(class.to_owned()).or_default());
    if let (Some(confidence), "heuristic") = (confidence, class) {
      if self.heuristic_buckets.is_empty() {
        self.heuristic_buckets = vec![ClassCalibration::default(); CONFIDENCE_BUCKETS.len()];
      }
      apply(&mut self.heuristic_buckets[bucket_index(confidence)]);
    }
  }

  pub fn absorb(&mut self, other: &Calibration) {
    for (class, cell) in &other.by_class {
      self.by_class.entry(class.clone()).or_default().absorb(cell);
    }
    if self.heuristic_buckets.is_empty() {
      self.heuristic_buckets = vec![ClassCalibration::default(); CONFIDENCE_BUCKETS.len()];
    }
    for (mine, theirs) in self.heuristic_buckets.iter_mut().zip(&other.heuristic_buckets) {
      mine.absorb(theirs);
    }
  }

  pub fn heuristic_monotone(&self) -> Option<bool> {
    let fractions: Vec<f64> = self
      .heuristic_buckets
      .iter()
      .filter(|cell| cell.judged > 0)
      .filter_map(ClassCalibration::fraction_true_relation)
      .collect();
    if fractions.len() < 2 {
      return None;
    }
    Some(fractions.windows(2).all(|pair| pair[1] + 1e-9 >= pair[0]))
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgedEdge {
  Correct,
  Overclaimed,
  Underclaimed,
  False,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delta {
  pub metric: String,
  pub before: f64,
  pub after: f64,
  pub change: f64,
  pub better: Better,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
  pub improved: Vec<Delta>,
  pub regressed: Vec<Delta>,
  pub unchanged: usize,
  pub added: Vec<String>,
  pub removed: Vec<String>,
}

const EPSILON: f64 = 1e-9;

pub fn compare_metrics(previous: &BTreeMap<String, Metric>, current: &BTreeMap<String, Metric>) -> Comparison {
  let mut comparison = Comparison::default();
  for (name, now) in current {
    let Some(before) = previous.get(name) else {
      comparison.added.push(name.clone());
      continue;
    };
    let change = now.value - before.value;
    if change.abs() <= EPSILON {
      comparison.unchanged += 1;
      continue;
    }
    let delta = Delta {
      metric: name.clone(),
      before: before.value,
      after: now.value,
      change,
      better: now.better,
    };
    let improved = match now.better {
      Better::Higher => change > 0.0,
      Better::Lower => change < 0.0,
    };
    match improved {
      true => comparison.improved.push(delta),
      false => comparison.regressed.push(delta),
    }
  }
  comparison.removed = previous.keys().filter(|name| !current.contains_key(*name)).cloned().collect();
  comparison
}

#[cfg(test)]
mod tests {
  use super::*;

  fn higher(value: f64) -> Metric {
    Metric {
      value,
      numerator: 0,
      denominator: 1,
      better: Better::Higher,
    }
  }

  fn lower(value: f64) -> Metric {
    Metric {
      better: Better::Lower,
      ..higher(value)
    }
  }

  #[test]
  fn should_report_no_ratio_for_an_empty_denominator() {
    assert_eq!(ratio(3, 0), None);
    assert_eq!(ratio(3, 4), Some(0.75));
    assert!(Metric::ratio(0, 0, Better::Higher).is_none());
  }

  #[test]
  fn should_keep_unbound_expectations_out_of_the_bound_recall() {
    let mut recall = Recall::default();
    recall.record(&RecallOutcome::Found);
    recall.record(&RecallOutcome::Found);
    recall.record(&RecallOutcome::Missing { detail: String::new() });
    recall.record(&RecallOutcome::WrongCertainty { actual: "heuristic".into() });
    recall.record(&RecallOutcome::Unbound);
    assert_eq!((recall.expected, recall.bound(), recall.unbound), (5, 4, 1));
    assert_eq!(recall.recall_over_bound(), Some(0.5));
    assert_eq!(recall.recall_over_all(), Some(0.4));
    assert_eq!(recall.existence_over_bound(), Some(0.75));
  }

  #[test]
  fn should_sum_recall_tallies() {
    let mut total = Recall::default();
    total.absorb(&Recall {
      expected: 3,
      found: 2,
      missing: 1,
      ..Recall::default()
    });
    total.absorb(&Recall {
      expected: 2,
      found: 1,
      unbound: 1,
      ..Recall::default()
    });
    assert_eq!(
      total,
      Recall {
        expected: 5,
        found: 3,
        missing: 1,
        unbound: 1,
        ..Recall::default()
      }
    );
  }

  #[test]
  fn should_compute_prohibition_satisfaction_over_checked_only() {
    let prohibition = Prohibition {
      checked: 8,
      violations: 2,
      unbound: 3,
      vacuous: 1,
    };
    assert_eq!(prohibition.satisfaction(), Some(0.75));
    assert_eq!(Prohibition::default().satisfaction(), None);
  }

  #[test]
  fn should_bucket_heuristic_confidence_and_check_monotonicity() {
    let mut calibration = Calibration::new();
    calibration.record("heuristic", Some(0.3), JudgedEdge::False);
    calibration.record("heuristic", Some(0.3), JudgedEdge::Correct);
    calibration.record("heuristic", Some(0.7), JudgedEdge::Correct);
    calibration.record("heuristic", Some(0.7), JudgedEdge::Correct);
    calibration.record("heuristic", Some(1.0), JudgedEdge::Correct);
    calibration.record("resolved", None, JudgedEdge::Correct);
    assert_eq!(bucket_index(0.3), 0);
    assert_eq!(bucket_index(0.4), 1);
    assert_eq!(bucket_index(1.0), 3);
    assert_eq!(calibration.by_class["heuristic"].judged, 5);
    assert_eq!(calibration.heuristic_buckets[0].fraction_correct(), Some(0.5));
    assert_eq!(calibration.heuristic_monotone(), Some(true));
    calibration.record("heuristic", Some(0.7), JudgedEdge::False);
    calibration.record("heuristic", Some(0.7), JudgedEdge::False);
    calibration.record("heuristic", Some(1.0), JudgedEdge::False);
    calibration.record("heuristic", Some(1.0), JudgedEdge::False);
    assert_eq!(calibration.heuristic_monotone(), Some(false));
  }

  #[test]
  fn should_separate_true_relation_from_correct_claim() {
    let mut cell = ClassCalibration::default();
    for verdict in [JudgedEdge::Correct, JudgedEdge::Overclaimed, JudgedEdge::Underclaimed, JudgedEdge::False] {
      Calibration::new().record("resolved", None, verdict);
      cell.judged += 1;
      match verdict {
        JudgedEdge::Correct => cell.correct += 1,
        JudgedEdge::Overclaimed => cell.overclaimed += 1,
        JudgedEdge::Underclaimed => cell.underclaimed += 1,
        JudgedEdge::False => cell.false_edges += 1,
      }
    }
    assert_eq!(cell.fraction_correct(), Some(0.25));
    assert_eq!(cell.fraction_true_relation(), Some(0.75));
  }

  #[test]
  fn should_classify_deltas_by_metric_direction() {
    let previous = BTreeMap::from([
      ("recall".to_owned(), higher(0.5)),
      ("violations".to_owned(), lower(4.0)),
      ("flat".to_owned(), higher(1.0)),
      ("gone".to_owned(), higher(1.0)),
      ("precision".to_owned(), higher(0.9)),
    ]);
    let current = BTreeMap::from([
      ("recall".to_owned(), higher(0.75)),
      ("violations".to_owned(), lower(6.0)),
      ("flat".to_owned(), higher(1.0)),
      ("new".to_owned(), higher(0.1)),
      ("precision".to_owned(), higher(0.8)),
    ]);
    let comparison = compare_metrics(&previous, &current);
    assert_eq!(comparison.improved.iter().map(|delta| delta.metric.as_str()).collect::<Vec<_>>(), ["recall"]);
    assert_eq!(
      comparison.regressed.iter().map(|delta| delta.metric.as_str()).collect::<Vec<_>>(),
      ["precision", "violations"]
    );
    assert_eq!(comparison.unchanged, 1);
    assert_eq!(comparison.added, ["new"]);
    assert_eq!(comparison.removed, ["gone"]);
    assert_eq!(comparison.improved[0].change, 0.25);
  }

  #[test]
  fn should_treat_a_falling_lower_is_better_metric_as_an_improvement() {
    let previous = BTreeMap::from([("violations".to_owned(), lower(4.0))]);
    let current = BTreeMap::from([("violations".to_owned(), lower(1.0))]);
    let comparison = compare_metrics(&previous, &current);
    assert_eq!(comparison.improved.len(), 1);
    assert!(comparison.regressed.is_empty());
  }
}
