use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use atlas_core::{ContentHash, RunId};
use atlas_edit::{AppliedChange, ApplyReport, EditPlan, FileTarget, ResourceOp};
use atlas_lsp::protocol::{Diagnostic, DiagnosticSeverity};
use atlas_lsp::{Lease, LineIndex, ServerKind, ServerState, SyncDemand, canonical_path};
use serde::Serialize;

use crate::diagnostics::{NATIVE_SOURCE, PUSH_QUIET, Republished, accepts, pull, republish};
use crate::service::Session;

const MAX_CHECKED_FILES: usize = 20;
const POLL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewDiagnostic {
  pub file: String,
  pub line: u32,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub column: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub code: Option<String>,
  pub message: String,
  pub server: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DiagnosticsDelta {
  Checked {
    files: usize,
    checked: usize,
    new_errors: Vec<NewDiagnostic>,
    unsettled: Vec<String>,
    native_only: bool,
  },
  Unavailable {
    reason: String,
  },
}

type ErrorKey = (Option<String>, String);

enum Collected {
  Settled(Vec<Diagnostic>),
  Unsettled(String),
}

pub(crate) struct Baseline {
  errors: BTreeMap<PathBuf, Result<Vec<ErrorKey>, String>>,
}

fn absolute(session: &Session<'_>, file: &FileTarget) -> Option<PathBuf> {
  let root = session.files.root(&file.repo)?;
  let path = root.join(file.path.as_str());
  session.kind.language_id(&path)?;
  let path = canonical_path(&path).unwrap_or(path);
  session.covers(&path).then_some(path)
}

pub(crate) fn touched_paths(session: &Session<'_>, plan: &EditPlan, report: Option<&ApplyReport>) -> Vec<PathBuf> {
  let mut files: Vec<&FileTarget> = plan.files.iter().map(|file| &file.file).collect();
  match report {
    None => files.extend(plan.resources.iter().filter_map(|op| match op {
      ResourceOp::Rename { from, .. } | ResourceOp::Delete { file: from, .. } => Some(from),
      ResourceOp::Create { .. } => None,
    })),
    Some(report) => {
      files = report
        .changes
        .iter()
        .filter_map(|change| match change {
          AppliedChange::Modified { file, .. } | AppliedChange::Created { file, .. } => Some(file),
          AppliedChange::Renamed { to, .. } => Some(to),
          AppliedChange::Deleted { .. } => None,
        })
        .collect();
    }
  }
  let mut paths: Vec<PathBuf> = files.into_iter().filter_map(|file| absolute(session, file)).filter(|path| path.is_file()).collect();
  paths.sort();
  paths.dedup();
  paths
}

pub(crate) fn baseline(session: &mut Session<'_>, paths: &[PathBuf], generation: Option<RunId>, warnings: &mut Vec<String>) -> Baseline {
  let checked: Vec<PathBuf> = paths.iter().take(MAX_CHECKED_FILES).cloned().collect();
  if let Err(error) = session.resync(generation, SyncDemand::Best, &checked) {
    warnings.push(format!(
      "{} could not open the files to edit, so errors before the edit are unknown: {error}",
      session.server_name()
    ));
    return Baseline {
      errors: paths.iter().map(|path| (path.clone(), Err("diagnostics before the edit are unknown".to_owned()))).collect(),
    };
  }
  let collected = collect(session, &checked);
  let mut errors: BTreeMap<PathBuf, Result<Vec<ErrorKey>, String>> = collected
    .into_iter()
    .map(|(path, collected)| {
      let keys = match collected {
        Collected::Settled(diagnostics) => Ok(diagnostics.iter().filter(|diagnostic| is_error(diagnostic)).map(key).collect()),
        Collected::Unsettled(reason) => Err(format!("before the edit: {reason}")),
      };
      (path, keys)
    })
    .collect();
  for path in paths.iter().skip(MAX_CHECKED_FILES) {
    errors.insert(path.clone(), Err(format!("not checked (only the first {MAX_CHECKED_FILES} edited files are)")));
  }
  Baseline { errors }
}

pub(crate) fn after_edit(
  session: &mut Session<'_>,
  plan: &EditPlan,
  report: &ApplyReport,
  baseline: Baseline,
  generation: Option<RunId>,
  warnings: &mut Vec<String>,
) -> DiagnosticsDelta {
  let server = session.server_name().to_owned();
  let paths = touched_paths(session, plan, Some(report));
  if let Err(error) = session.resync(generation, SyncDemand::Best, &paths) {
    warnings.push(format!("{server} was not told about the edit ({error}); it re-reads the files when it restarts"));
    return DiagnosticsDelta::Unavailable {
      reason: format!("{server} could not re-read the edited files"),
    };
  }
  verify_overlays(session, report, warnings);
  let renamed = renamed_sources(session, report);
  let checked: Vec<PathBuf> = paths.iter().take(MAX_CHECKED_FILES).cloned().collect();
  let mut new_errors = Vec::new();
  let mut unsettled = Vec::new();
  for (path, collected) in collect(session, &checked) {
    let shown = session.files.of_absolute(&path).map_or_else(|| path.display().to_string(), |file| file.to_string());
    let before_path = renamed.get(&path).cloned().unwrap_or_else(|| path.clone());
    let before = baseline.errors.get(&before_path).cloned().unwrap_or_else(|| Err("before the edit: not checked".to_owned()));
    let (diagnostics, mut before) = match (collected, before) {
      (Collected::Settled(diagnostics), Ok(before)) => (diagnostics, before),
      (Collected::Unsettled(reason), _) => {
        unsettled.push(format!("{shown} (after the edit: {reason})"));
        continue;
      }
      (Collected::Settled(_), Err(reason)) => {
        unsettled.push(format!("{shown} ({reason})"));
        continue;
      }
    };
    let text = std::fs::read_to_string(&path).ok();
    for diagnostic in unmatched_errors(&mut before, &diagnostics) {
      let place = text.as_deref().and_then(|text| place(session, text, diagnostic));
      new_errors.push(NewDiagnostic {
        file: shown.clone(),
        line: place.map_or(diagnostic.range.start.line + 1, |(line, _)| line),
        column: place.map(|(_, column)| column),
        code: diagnostic.code.as_ref().map(ToString::to_string),
        message: diagnostic.message.lines().next().unwrap_or_default().trim().to_owned(),
        server: server.clone(),
      });
    }
  }
  DiagnosticsDelta::Checked {
    files: paths.len(),
    checked: checked.len(),
    new_errors,
    unsettled,
    native_only: session.kind == ServerKind::RustAnalyzer,
  }
}

pub(crate) fn combine(deltas: Vec<DiagnosticsDelta>) -> DiagnosticsDelta {
  if deltas.is_empty() {
    return DiagnosticsDelta::Unavailable {
      reason: "no language server is running for the edited files; diagnostics were not checked".to_owned(),
    };
  }
  let mut combined: Option<DiagnosticsDelta> = None;
  let mut reasons = Vec::new();
  for delta in deltas {
    match (delta, combined.as_mut()) {
      (DiagnosticsDelta::Unavailable { reason }, _) => reasons.push(reason),
      (checked @ DiagnosticsDelta::Checked { .. }, None) => combined = Some(checked),
      (
        DiagnosticsDelta::Checked {
          files,
          checked,
          new_errors,
          unsettled,
          native_only,
        },
        Some(DiagnosticsDelta::Checked {
          files: total,
          checked: checked_total,
          new_errors: errors,
          unsettled: open,
          native_only: native,
        }),
      ) => {
        *total += files;
        *checked_total += checked;
        errors.extend(new_errors);
        open.extend(unsettled);
        *native |= native_only;
      }
      (DiagnosticsDelta::Checked { .. }, Some(DiagnosticsDelta::Unavailable { .. })) => {}
    }
  }
  match combined {
    Some(DiagnosticsDelta::Checked {
      files,
      checked,
      new_errors,
      mut unsettled,
      native_only,
    }) => {
      unsettled.extend(reasons);
      DiagnosticsDelta::Checked {
        files,
        checked,
        new_errors,
        unsettled,
        native_only,
      }
    }
    _ => DiagnosticsDelta::Unavailable { reason: reasons.join("; ") },
  }
}

fn verify_overlays(session: &Session<'_>, report: &ApplyReport, warnings: &mut Vec<String>) {
  let written: BTreeMap<&FileTarget, ContentHash> = report
    .changes
    .iter()
    .filter_map(|change| match change {
      AppliedChange::Modified { file, after, .. } => Some((file, *after)),
      _ => None,
    })
    .collect();
  let moved: Vec<&FileTarget> = report
    .changes
    .iter()
    .filter_map(|change| match change {
      AppliedChange::Renamed { from, .. } => Some(from),
      _ => None,
    })
    .collect();
  for change in &report.changes {
    let (file, expected) = match change {
      AppliedChange::Modified { file, .. } if moved.contains(&file) => continue,
      AppliedChange::Modified { file, after, .. } | AppliedChange::Created { file, after } => (file, Some(*after)),
      AppliedChange::Renamed { from, to } => (to, written.get(from).copied()),
      AppliedChange::Deleted { .. } => continue,
    };
    let Some(path) = absolute(session, file) else { continue };
    let expected = expected.or_else(|| std::fs::read(&path).ok().map(|bytes| ContentHash::of(&bytes)));
    let held = session.lease.open_document(&path).map(|doc| doc.hash);
    if held.is_none() || held != expected {
      warnings.push(format!(
        "{} does not hold the edited text of {file}; its next answers about that file may be stale until it re-reads it",
        session.server_name()
      ));
    }
  }
}

fn renamed_sources(session: &Session<'_>, report: &ApplyReport) -> BTreeMap<PathBuf, PathBuf> {
  report
    .changes
    .iter()
    .filter_map(|change| match change {
      AppliedChange::Renamed { from, to } => Some((absolute(session, to)?, absolute(session, from)?)),
      _ => None,
    })
    .collect()
}

fn unmatched_errors<'d>(before: &mut Vec<ErrorKey>, after: &'d [Diagnostic]) -> Vec<&'d Diagnostic> {
  let mut fresh = Vec::new();
  for diagnostic in after.iter().filter(|diagnostic| is_error(diagnostic)) {
    match before.iter().position(|seen| *seen == key(diagnostic)) {
      Some(index) => {
        before.swap_remove(index);
      }
      None => fresh.push(diagnostic),
    }
  }
  fresh
}

fn is_error(diagnostic: &Diagnostic) -> bool {
  matches!(diagnostic.severity, Some(DiagnosticSeverity::Error) | None)
}

fn key(diagnostic: &Diagnostic) -> ErrorKey {
  (diagnostic.code.as_ref().map(ToString::to_string), diagnostic.message.trim().to_owned())
}

fn place(session: &Session<'_>, text: &str, diagnostic: &Diagnostic) -> Option<(u32, u32)> {
  let index = LineIndex::new(text, session.lease.line_breaks());
  let offset = index.offset(diagnostic.range.start, session.lease.encoding()).ok()?;
  let span = index.graph_span(offset..offset).ok()?;
  Some((span.start_line, span.start_col + 1))
}

fn collect(session: &mut Session<'_>, paths: &[PathBuf]) -> Vec<(PathBuf, Collected)> {
  let deadline = Instant::now() + session.diagnostics_timeout();
  let kind = session.kind;
  let native = |diagnostics: Vec<Diagnostic>| match kind {
    ServerKind::RustAnalyzer => diagnostics.into_iter().filter(|diagnostic| diagnostic.source.as_deref() == Some(NATIVE_SOURCE)).collect(),
    ServerKind::TypeScript => diagnostics,
  };
  if let Some(options) = session.capabilities().diagnostic_provider.clone() {
    return paths
      .iter()
      .map(|path| {
        let collected = match pull(session, path, options.identifier.clone()) {
          Ok(diagnostics) => Collected::Settled(native(diagnostics)),
          Err(error) => Collected::Unsettled(format!("pulling diagnostics failed: {error}")),
        };
        (path.clone(), collected)
      })
      .collect();
  }
  let republished = match republish(session, paths) {
    Ok(republished) => republished,
    Err(error) => {
      return paths
        .iter()
        .map(|path| (path.clone(), Collected::Unsettled(format!("re-sending the file failed: {error}"))))
        .collect();
    }
  };
  wait_all(&session.lease, republished, deadline)
    .into_iter()
    .map(|(path, collected)| {
      let collected = match collected {
        Collected::Settled(diagnostics) => Collected::Settled(native(diagnostics)),
        unsettled => unsettled,
      };
      (path, collected)
    })
    .collect()
}

struct Waiting {
  path: PathBuf,
  republished: Result<Republished, String>,
  cursor: u64,
  publishes: usize,
  accepted: Option<Vec<Diagnostic>>,
  last: Option<Instant>,
}

impl Waiting {
  fn settled(&self, now: Instant) -> bool {
    let (Ok(republished), Some(last), Some(_)) = (&self.republished, self.last, &self.accepted) else {
      return false;
    };
    let required = match self.publishes >= republished.quiet.publishes {
      true => PUSH_QUIET,
      false => republished.quiet.lone,
    };
    now.saturating_duration_since(last) >= required
  }
}

fn wait_all(lease: &Lease, republished: Vec<(PathBuf, Result<Republished, String>)>, deadline: Instant) -> Vec<(PathBuf, Collected)> {
  let mut waiting: Vec<Waiting> = republished
    .into_iter()
    .map(|(path, republished)| Waiting {
      cursor: republished.as_ref().map_or(0, |republished| republished.after),
      path,
      republished,
      publishes: 0,
      accepted: None,
      last: None,
    })
    .collect();
  loop {
    let now = Instant::now();
    for entry in waiting.iter_mut() {
      let Ok(republished) = &entry.republished else { continue };
      let Some((sequence, params)) = lease.diagnostics(&entry.path) else { continue };
      if sequence <= entry.cursor {
        continue;
      }
      entry.cursor = sequence;
      if accepts(&params, republished.version) {
        entry.accepted = Some(params.diagnostics);
        entry.publishes += 1;
        entry.last = Some(now);
      }
    }
    let pending = waiting.iter().filter(|entry| entry.republished.is_ok() && !entry.settled(now)).count();
    if pending == 0 || now >= deadline || matches!(lease.state(), ServerState::Exited { .. }) {
      break;
    }
    if let Some(entry) = waiting.iter().find(|entry| entry.republished.is_ok() && !entry.settled(now)) {
      lease.wait_diagnostics(&entry.path, entry.cursor, POLL.min(deadline.saturating_duration_since(now)));
    }
  }
  let now = Instant::now();
  waiting
    .into_iter()
    .map(|entry| {
      let settled = entry.settled(now);
      let collected = match (entry.republished, entry.accepted) {
        (Err(reason), _) => Collected::Unsettled(reason),
        (Ok(_), Some(diagnostics)) if settled => Collected::Settled(diagnostics),
        (Ok(_), Some(_)) => Collected::Unsettled("diagnostics were still changing when the 10 s wait ended; nothing is known".to_owned()),
        (Ok(_), None) => Collected::Unsettled("the server published no diagnostics within 10 s; nothing is known".to_owned()),
      };
      (entry.path, collected)
    })
    .collect()
}

#[cfg(test)]
mod tests {
  use atlas_lsp::protocol::NumberOrString;

  use super::*;

  const ERROR: u8 = 1;
  const WARNING: u8 = 2;

  fn diagnostic(severity: u8, code: &str, message: &str) -> Diagnostic {
    serde_json::from_value(serde_json::json!({
      "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
      "severity": severity,
      "code": code,
      "message": message
    }))
    .unwrap()
  }

  #[test]
  fn should_report_only_errors_that_were_not_there_before_counting_repeats() {
    let mut before = vec![(Some("2307".to_owned()), "Cannot find module 'a'.".to_owned())];
    let after = vec![
      diagnostic(ERROR, "2307", "Cannot find module 'a'."),
      diagnostic(ERROR, "2307", "Cannot find module 'a'."),
      diagnostic(WARNING, "6133", "'x' is declared but never used."),
      diagnostic(ERROR, "2551", "Property 'findOne' does not exist."),
    ];

    let fresh: Vec<(Option<NumberOrString>, &str)> = unmatched_errors(&mut before, &after)
      .into_iter()
      .map(|diagnostic| (diagnostic.code.clone(), diagnostic.message.as_str()))
      .collect();

    assert_eq!(
      fresh,
      vec![
        (Some(NumberOrString::String("2307".to_owned())), "Cannot find module 'a'."),
        (Some(NumberOrString::String("2551".to_owned())), "Property 'findOne' does not exist.")
      ]
    );
    assert!(before.is_empty());
  }
}
