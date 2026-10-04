mod common;

use std::fs;
use std::sync::Arc;

use atlas_engine::{DegradedCause, EngineOptions, FreshnessReport, PassContext, PassError, PassOutput, ProjectPass};

use common::{acme_copy, fresh, open, open_with};

struct FailingPass;

impl ProjectPass for FailingPass {
  fn name(&self) -> &'static str {
    "failing"
  }

  fn run(&self, _tx: &atlas_store::ReadTx<'_>, _context: &PassContext<'_>) -> Result<PassOutput, PassError> {
    panic!("broken pass");
  }
}

#[test]
fn should_render_every_cause_as_the_human_readable_reason() {
  assert_eq!(DegradedCause::ProjectConfigChanged.to_string(), "project configuration changed; reopen the engine");
  assert_eq!(DegradedCause::UpdateFailed { error: "disk full".to_owned() }.to_string(), "index update failed: disk full");
  assert_eq!(
    DegradedCause::PartialRun {
      reason: "project pass link failed".to_owned()
    }
    .to_string(),
    "project pass link failed"
  );
  assert_eq!(DegradedCause::FilesNotIndexed { count: 3 }.to_string(), "3 file(s) or repositories could not be indexed");
}

#[test]
fn should_report_a_project_configuration_change_as_a_typed_cause() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let home = scratch.path().join("home");
  let mut fewer = project.clone();
  fewer.repos.retain(|repo| repo.name.as_str() != "acme-web");
  let old = open(&fewer, &home);
  fresh(&old);
  let grown = open(&project, &home);
  fresh(&grown);

  let report = fresh(&old);

  let FreshnessReport::Degraded { cause, failed_files, .. } = &report else {
    panic!("{report:?}")
  };
  assert_eq!(cause, &DegradedCause::ProjectConfigChanged);
  assert!(failed_files.is_empty());
}

#[test]
fn should_report_a_missing_repository_as_a_partial_run_then_as_unindexed() {
  let scratch = tempfile::tempdir().unwrap();
  let tree = scratch.path().join("tree");
  let project = acme_copy(&tree);
  let engine = open(&project, &scratch.path().join("home"));
  fresh(&engine);
  fs::remove_dir_all(tree.join("acme-web")).unwrap();

  let purged = fresh(&engine);
  let FreshnessReport::Degraded { cause, failed_files, .. } = &purged else {
    panic!("{purged:?}")
  };
  assert_eq!(
    cause,
    &DegradedCause::PartialRun {
      reason: "1 file(s) or repo(s) failed".to_owned()
    }
  );
  assert_eq!(failed_files.len(), 1, "{failed_files:?}");

  let settled = fresh(&engine);
  let FreshnessReport::Degraded { cause, failed_files, .. } = &settled else {
    panic!("{settled:?}")
  };
  assert_eq!(cause, &DegradedCause::FilesNotIndexed { count: 1 });
  assert_eq!(cause.to_string(), "1 file(s) or repositories could not be indexed");
  assert_eq!(failed_files.len(), 1, "{failed_files:?}");
}

#[test]
fn should_carry_the_partial_run_reason_of_a_failed_project_pass() {
  let scratch = tempfile::tempdir().unwrap();
  let project = acme_copy(&scratch.path().join("tree"));
  let options = EngineOptions {
    passes: vec![Arc::new(FailingPass)],
    ..EngineOptions::default()
  };
  let engine = open_with(&project, &scratch.path().join("home"), options);

  let report = fresh(&engine);

  let FreshnessReport::Degraded {
    cause: DegradedCause::PartialRun { reason },
    ..
  } = &report
  else {
    panic!("{report:?}")
  };
  assert!(reason.contains("project pass failing failed: panicked: broken pass"), "{reason}");
}
