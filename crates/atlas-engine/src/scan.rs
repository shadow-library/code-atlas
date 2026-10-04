use std::time::{SystemTime, UNIX_EPOCH};

use atlas_core::{Language, RelPath, RepoName};
use atlas_project::{ChangeSet, DiscoveredFile, SkipReason, detect_changes, discover, is_manifest_name};

use crate::report::SkippedFile;
use crate::state::RepoState;

pub(crate) struct ScannedRepo {
  pub files: Vec<DiscoveredFile>,
  pub changes: ChangeSet,
  pub walk_errors: Vec<String>,
  pub skipped: Vec<SkippedFile>,
}

pub(crate) enum ScanOutcome {
  Scanned(Box<ScannedRepo>),
  Missing { reason: String },
}

pub(crate) struct RepoScan {
  pub repo: RepoName,
  pub started_ns: i64,
  pub outcome: ScanOutcome,
}

impl RepoScan {
  pub fn needs_run(&self, state: &RepoState) -> bool {
    match &self.outcome {
      ScanOutcome::Scanned(scanned) => scanned.changes.needs_reindex(),
      ScanOutcome::Missing { .. } => !state.stamps.is_empty() || state.graph.is_some(),
    }
  }
}

pub(crate) fn now_ns() -> i64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map_or(0, |elapsed| i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX))
}

pub(crate) fn scan_repo(state: &RepoState, max_file_bytes: u64) -> RepoScan {
  let started_ns = now_ns();
  let repo = state.config.name.clone();
  let outcome = match discover(&state.config, max_file_bytes) {
    Err(error) => ScanOutcome::Missing { reason: error.to_string() },
    Ok(result) => {
      let changes = detect_changes(&state.config.path, &result.files, &state.stamps, state.scan_started_ns.unwrap_or(i64::MIN));
      ScanOutcome::Scanned(Box::new(ScannedRepo {
        files: result.files,
        changes,
        walk_errors: result.walk_errors,
        skipped: skipped_files(&repo, &result.skipped),
      }))
    }
  };
  RepoScan { repo, started_ns, outcome }
}

pub(crate) fn skipped_files(repo: &RepoName, skipped: &[(RelPath, SkipReason)]) -> Vec<SkippedFile> {
  skipped
    .iter()
    .map(|(path, reason)| SkippedFile {
      repo: repo.clone(),
      path: path.clone(),
      cause: reason.into(),
    })
    .collect()
}

pub(crate) fn is_code_file(path: &RelPath) -> bool {
  Language::from_path(path.as_str()).is_some()
}

pub(crate) fn is_manifest(path: &RelPath) -> bool {
  is_manifest_name(path.file_name())
}
