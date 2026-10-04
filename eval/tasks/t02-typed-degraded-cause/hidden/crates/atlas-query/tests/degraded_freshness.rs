use std::time::Duration;

use atlas_engine::{DegradedCause, FreshnessHeader, FreshnessReport, ScanKind, ScipLag};
use atlas_query::{Freshness, FreshnessState};

fn header() -> FreshnessHeader {
  FreshnessHeader {
    run: None,
    last_run: None,
    files_updated: 0,
    duration: Duration::from_millis(4),
    scan: ScanKind::StatScan,
    scip: ScipLag::NeverRun,
    stale_inferred_facts: 0,
    files_failed_analysis: 0,
  }
}

#[test]
fn should_surface_the_typed_cause_as_the_degraded_reason() {
  let cases = [
    (DegradedCause::ProjectConfigChanged, "project configuration changed; reopen the engine"),
    (DegradedCause::UpdateFailed { error: "lock timeout".to_owned() }, "index update failed: lock timeout"),
    (DegradedCause::FilesNotIndexed { count: 2 }, "2 file(s) or repositories could not be indexed"),
  ];
  for (cause, expected) in cases {
    let report = FreshnessReport::Degraded {
      header: header(),
      failed_files: Vec::new(),
      cause,
    };

    let freshness = Freshness::from_report(&report);

    assert_eq!(
      freshness.state,
      FreshnessState::Degraded {
        reason: expected.to_owned(),
        failed_files: Vec::new()
      }
    );
    let json = serde_json::to_value(&freshness).unwrap();
    assert_eq!(json["state"], "degraded");
    assert_eq!(json["reason"], expected);
  }
}
