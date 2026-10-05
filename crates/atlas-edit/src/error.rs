use std::io;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

use crate::occurrence::OccurrenceId;
use crate::plan::FileTarget;
use crate::scope::Refusal;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum PlanViolation {
  #[error("the plan changes nothing")]
  Empty,
  #[error("{file} appears more than once")]
  DuplicateFile { file: String },
  #[error("{file} has no edits")]
  NoEdits { file: FileTarget },
  #[error("{file}: edit {start}..{end} ends before it starts")]
  Inverted { file: FileTarget, start: usize, end: usize },
  #[error("{file}: edit {index} is not sorted by position")]
  Unsorted { file: FileTarget, index: usize },
  #[error("{file}: edits {}..{} and {}..{} overlap", .first.0, .first.1, .second.0, .second.1)]
  Overlap { file: FileTarget, first: (usize, usize), second: (usize, usize) },
  #[error("{file}: edit {start}..{end} is outside the file ({len} bytes)")]
  OutOfBounds { file: FileTarget, start: usize, end: usize, len: usize },
  #[error("{file}: offset {offset} is not on a character boundary")]
  NotCharBoundary { file: FileTarget, offset: usize },
  #[error("{file}: an edit touches the byte order mark")]
  TouchesBom { file: FileTarget },
  #[error("{path}: {reason}")]
  ResourceConflict { path: FileTarget, reason: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotTextReason {
  Binary,
  NotUtf8,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EditError {
  #[error("invalid edit plan: {0}")]
  InvalidPlan(#[from] PlanViolation),
  #[error("refused: {} outside the editable scope", render_refusals(.refused))]
  OutOfScope { refused: Vec<Refusal> },
  #[error("changed since planning: {}", .paths.join(", "))]
  Stale { paths: Vec<String> },
  #[error("{path} already exists")]
  AlreadyExists { path: String },
  #[error("{path} is read-only; make it writable first")]
  ReadOnly { path: String },
  #[error("{path} does not exist")]
  Missing { path: String },
  #[error("{path} is not a text file ({reason:?})")]
  NotText { path: String, reason: NotTextReason },
  #[error("{operation} {}: {source}", .path.display())]
  Io {
    operation: &'static str,
    path: PathBuf,
    #[source]
    source: io::Error,
  },
  #[error("timed out after {}ms waiting for the edit lock {}", .waited.as_millis(), .path.display())]
  LockTimeout { path: PathBuf, waited: Duration },
  #[error("edit rolled back: {cause}")]
  RolledBack { cause: Box<EditError> },
  #[error("DEGRADED: edit partially applied and rollback failed ({cause}); still changed: {}; rollback errors: {}", .applied.join(", "), .failed.join("; "))]
  PartiallyApplied {
    cause: Box<EditError>,
    applied: Vec<String>,
    failed: Vec<String>,
  },
  #[error("the pattern must not be empty")]
  EmptyPattern,
  #[error("invalid regex {pattern:?}: {message}")]
  InvalidRegex { pattern: String, message: String },
  #[error("the replacement refers to capture group {reference:?}, which the pattern does not define; write ${{1}} to end a group number before other text, or $$ for a literal $")]
  UnknownCaptureGroup { reference: String },
  #[error("the pattern matches an empty string at byte {offset} of {path}; anchor it to text")]
  EmptyMatch { path: String, offset: usize },
  #[error("no match in {path}")]
  NoMatch { path: String },
  #[error("{count} matches; pass all or narrow the pattern ({})", render_ids(.ids))]
  MultipleMatches { count: usize, ids: Vec<OccurrenceId> },
  #[error("expected {expected} matches, found {found}")]
  ExpectedCountMismatch { expected: usize, found: usize },
  #[error("unknown or stale occurrences: {}", render_ids(.ids))]
  UnknownOccurrences { ids: Vec<OccurrenceId> },
  #[error("no occurrences selected")]
  EmptySelection,
  #[error("occurrence id {id} is not unique in this call; narrow the search")]
  OccurrenceCollision { id: OccurrenceId },
  #[error("invalid occurrence id {0:?}")]
  InvalidOccurrenceId(String),
  #[error("invalid glob {pattern:?}: {message}")]
  InvalidGlob { pattern: String, message: String },
  #[error(transparent)]
  Project(#[from] atlas_project::ProjectError),
  #[error("unknown repo {0:?}")]
  UnknownRepo(String),
  #[error("the new text is empty")]
  EmptyText,
  #[error("declaration span {start}..{end} is invalid for a {len}-byte file")]
  InvalidSpan { start: usize, end: usize, len: usize },
  #[error("the declaration shares its line with other code ({0}); edit it with replace_content")]
  NotLineAligned(&'static str),
}

pub type EditResult<T> = Result<T, EditError>;

impl EditError {
  pub(crate) fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
    Self::Io {
      operation,
      path: path.into(),
      source,
    }
  }
}

fn render_refusals(refused: &[Refusal]) -> String {
  refused.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}

fn render_ids(ids: &[OccurrenceId]) -> String {
  ids.iter().map(OccurrenceId::as_str).collect::<Vec<_>>().join(" ")
}
