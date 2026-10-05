use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use atlas_core::{ContentHash, RunId};

use crate::docs::{FileStamp, OpenDocs};
use crate::error::{LspError, LspResult};
use crate::pool::{Server, lock};
use crate::protocol::{
  DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, FileChangeType, FileEvent, TextDocumentContentChangeEvent,
  TextDocumentIdentifier, TextDocumentItem, VersionedTextDocumentIdentifier,
};
use crate::ready::Settle;
use crate::spec::Readiness;
use crate::uri::{canonical_path, uri_from_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
  Created(ContentHash),
  Changed(ContentHash),
  Deleted,
}

impl ChangeKind {
  fn event(self) -> FileChangeType {
    match self {
      Self::Created(_) => FileChangeType::Created,
      Self::Changed(_) => FileChangeType::Changed,
      Self::Deleted => FileChangeType::Deleted,
    }
  }

  fn hash(self) -> Option<ContentHash> {
    match self {
      Self::Created(hash) | Self::Changed(hash) => Some(hash),
      Self::Deleted => None,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
  pub path: PathBuf,
  pub kind: ChangeKind,
}

impl FileChange {
  pub fn new(path: impl Into<PathBuf>, kind: ChangeKind) -> Self {
    Self { path: path.into(), kind }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeList {
  Known(Vec<FileChange>),
  Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDemand {
  Best,
  Exact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncLimits {
  pub overlay_files: usize,
  pub watched_files: usize,
  pub overlay_bytes: usize,
  pub exact_overlay_bytes: usize,
  pub open_documents: usize,
  pub open_bytes: usize,
  pub settle: Duration,
  pub quiescence: Duration,
}

impl Default for SyncLimits {
  fn default() -> Self {
    Self {
      overlay_files: 64,
      watched_files: 1000,
      overlay_bytes: 8 * 1024 * 1024,
      exact_overlay_bytes: 64 * 1024 * 1024,
      open_documents: 64,
      open_bytes: 32 * 1024 * 1024,
      settle: Duration::from_millis(500),
      quiescence: Duration::from_secs(30),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartReason {
  Crashed,
  SyncLost(String),
  ConfigurationChanged,
  UnknownChanges,
  TooManyChanges { count: usize, limit: usize },
  Unconfirmed { count: usize },
  NeverReady { waited: Duration },
}

impl RestartReason {
  pub(crate) fn is_failure(&self) -> bool {
    matches!(self, Self::Crashed | Self::NeverReady { .. })
  }
}

impl fmt::Display for RestartReason {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Crashed => formatter.write_str("the server exited"),
      Self::SyncLost(reason) => write!(formatter, "document sync was lost ({reason})"),
      Self::ConfigurationChanged => formatter.write_str("the unit configuration changed"),
      Self::UnknownChanges => formatter.write_str("the changes since the server's generation are unknown"),
      Self::TooManyChanges { count, limit } => write!(formatter, "{count} files changed (more than {limit})"),
      Self::Unconfirmed { count } => write!(formatter, "the server cannot confirm {count} changed files"),
      Self::NeverReady { waited } => write!(formatter, "the server was still not ready after {}s", waited.as_secs_f64()),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
  UnsupportedLanguage,
  Missing,
  Unreadable(String),
  NotUtf8,
  OverBudget { bytes: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
  pub path: PathBuf,
  pub reason: SkipReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
  pub restarted: Option<RestartReason>,
  pub opened: Vec<PathBuf>,
  pub changed: Vec<PathBuf>,
  pub closed: Vec<PathBuf>,
  pub watched: usize,
  pub skipped: Vec<Skipped>,
  pub drifted: Vec<PathBuf>,
  pub settle: Settle,
}

impl Default for SyncReport {
  fn default() -> Self {
    Self {
      restarted: None,
      opened: Vec::new(),
      changed: Vec::new(),
      closed: Vec::new(),
      watched: 0,
      skipped: Vec::new(),
      drifted: Vec::new(),
      settle: Settle::NotNeeded,
    }
  }
}

#[derive(Debug, Default)]
pub(crate) struct SyncState {
  pub(crate) docs: OpenDocs,
  pub(crate) generation: Option<RunId>,
  pub(crate) unconfirmed: BTreeSet<PathBuf>,
}

pub(crate) enum SyncPlan {
  Apply(BTreeMap<PathBuf, ChangeKind>),
  Restart(RestartReason),
}

pub(crate) enum Applied {
  Synced(SyncReport),
  Stale,
  Restart(RestartReason),
}

struct Pass {
  budget: usize,
  events: BTreeMap<PathBuf, FileChangeType>,
  overlaid: BTreeSet<PathBuf>,
  report: SyncReport,
}

impl Pass {
  fn new(budget: usize) -> Self {
    Self {
      budget,
      events: BTreeMap::new(),
      overlaid: BTreeSet::new(),
      report: SyncReport::default(),
    }
  }

  fn skip(&mut self, path: &Path, reason: SkipReason) {
    self.report.skipped.push(Skipped { path: path.to_path_buf(), reason });
  }

  fn drift(&mut self, path: &Path, expected: Option<ChangeKind>, on_disk: Option<ContentHash>) {
    let drifted = match (expected, on_disk) {
      (None, _) => false,
      (Some(ChangeKind::Deleted), on_disk) => on_disk.is_some(),
      (Some(ChangeKind::Created(hash) | ChangeKind::Changed(hash)), on_disk) => on_disk != Some(hash),
    };
    if drifted {
      self.report.drifted.push(path.to_path_buf());
    }
  }
}

enum Disk {
  Present(FileStamp, Vec<u8>),
  Missing,
  Unreadable(String),
}

fn read_disk(path: &Path) -> Disk {
  let failed = |error: io::Error| match error.kind() {
    io::ErrorKind::NotFound => Disk::Missing,
    _ => Disk::Unreadable(error.to_string()),
  };
  let metadata = match fs::metadata(path) {
    Ok(metadata) => metadata,
    Err(error) => return failed(error),
  };
  match fs::read(path) {
    Ok(bytes) => Disk::Present(FileStamp::of(&metadata), bytes),
    Err(error) => failed(error),
  }
}

fn file_name(path: &Path) -> &str {
  path.file_name().and_then(|name| name.to_str()).unwrap_or_default()
}

fn is_project_config(path: &Path) -> bool {
  let name = file_name(path);
  name == "package.json" || ((name.starts_with("tsconfig") || name.starts_with("jsconfig")) && name.ends_with(".json"))
}

fn is_build_input(path: &Path) -> bool {
  let name = file_name(path);
  let in_cargo_dir = path.parent().is_some_and(|parent| file_name(parent) == ".cargo");
  matches!(name, "Cargo.toml" | "Cargo.lock" | "build.rs") || (in_cargo_dir && name.starts_with("config"))
}

enum Freshness {
  Current,
  Behind,
  Stale,
}

fn freshness(synced: Option<RunId>, since: Option<RunId>, target: Option<RunId>) -> Freshness {
  if synced >= target {
    return Freshness::Current;
  }
  match synced == since {
    true => Freshness::Behind,
    false => Freshness::Stale,
  }
}

pub(crate) struct SyncRequest<'a> {
  pub(crate) since: Option<RunId>,
  pub(crate) generation: Option<RunId>,
  pub(crate) changes: &'a BTreeMap<PathBuf, ChangeKind>,
  pub(crate) open: &'a [PathBuf],
  pub(crate) demand: SyncDemand,
  pub(crate) limits: &'a SyncLimits,
}

impl Server {
  pub(crate) fn plan(&self, changes: &ChangeList, limits: &SyncLimits) -> LspResult<SyncPlan> {
    let ChangeList::Known(changes) = changes else {
      return Ok(SyncPlan::Restart(RestartReason::UnknownChanges));
    };
    let mut relevant = BTreeMap::new();
    for change in changes {
      let path = canonical_path(&change.path)?;
      if self.spec.contains(&path) {
        relevant.insert(path, change.kind);
      }
    }
    if relevant.len() > limits.watched_files {
      return Ok(SyncPlan::Restart(RestartReason::TooManyChanges {
        count: relevant.len(),
        limit: limits.watched_files,
      }));
    }
    Ok(SyncPlan::Apply(relevant))
  }

  pub(crate) fn apply(&self, request: &SyncRequest<'_>) -> LspResult<Applied> {
    let limits = request.limits;
    let required = request.open.iter().map(|path| canonical_path(path)).collect::<Result<BTreeSet<_>, _>>()?;
    let budget = match request.demand {
      SyncDemand::Best => limits.overlay_bytes,
      SyncDemand::Exact => limits.exact_overlay_bytes,
    };
    let mut pass = Pass::new(budget);
    let mut sync = lock(&self.sync);
    let no_changes = BTreeMap::new();
    let changes = match freshness(sync.generation, request.since, request.generation) {
      Freshness::Current => &no_changes,
      Freshness::Behind => request.changes,
      Freshness::Stale => return Ok(Applied::Stale),
    };
    for path in &required {
      self.overlay(&mut sync.docs, path, changes.get(path).copied(), &mut pass)?;
    }
    let handled_written = changes
      .iter()
      .filter(|(path, kind)| kind.hash().is_some() && self.spec.kind.language_id(path).is_some())
      .count();
    let overlay_every_file = request.demand == SyncDemand::Exact || handled_written <= limits.overlay_files;
    for (path, kind) in changes {
      pass.events.insert(path.clone(), kind.event());
      if required.contains(path) {
        continue;
      }
      if kind.hash().is_none() {
        self.forget_deleted(&mut sync.docs, path, &mut pass)?;
        continue;
      }
      let handled = self.spec.kind.language_id(path).is_some();
      if handled && (overlay_every_file || sync.docs.get(path).is_some()) {
        self.overlay(&mut sync.docs, path, Some(*kind), &mut pass)?;
      }
    }
    for path in sync.docs.paths() {
      if !changes.contains_key(&path) && !required.contains(&path) {
        self.refresh(&mut sync.docs, &path, &mut pass)?;
      }
    }
    let activity = self.tracker.activity();
    let announced: Vec<PathBuf> = pass.events.keys().cloned().collect();
    if !announced.is_empty() {
      let changes = std::mem::take(&mut pass.events)
        .into_iter()
        .map(|(path, change)| {
          Ok(FileEvent {
            uri: uri_from_path(&path)?,
            change,
          })
        })
        .collect::<LspResult<Vec<_>>>()?;
      self.notify("workspace/didChangeWatchedFiles", DidChangeWatchedFilesParams { changes })?;
    }
    let keep: HashSet<PathBuf> = required.into_iter().collect();
    for victim in sync.docs.excess(limits.open_documents, limits.open_bytes, &keep) {
      self.close(&mut sync.docs, &victim, &mut pass)?;
    }
    let SyncState { docs, unconfirmed, generation } = &mut *sync;
    if let Readiness::Announced { .. } = self.spec.readiness() {
      unconfirmed.extend(announced.iter().filter(|path| self.needs_confirmation(path) && !pass.overlaid.contains(*path)).cloned());
    }
    unconfirmed.retain(|path| !pass.overlaid.contains(path) && docs.get(path).is_none());
    let unconfirmed: Vec<PathBuf> = unconfirmed.iter().cloned().collect();
    *generation = (*generation).max(request.generation);
    self.open_documents.store(docs.len(), Ordering::Relaxed);
    drop(sync);
    pass.report.watched = announced.len();
    pass.report.settle = match self.spec.readiness() {
      Readiness::Quiescent { .. } => {
        let quiet = announced.iter().all(|path| pass.overlaid.contains(path) && !is_build_input(path));
        let window = if quiet { Duration::ZERO } else { limits.settle };
        self.tracker.settle(activity, window, limits.quiescence)?
      }
      Readiness::Announced { .. } if unconfirmed.is_empty() => Settle::NotNeeded,
      Readiness::Announced { .. } if request.demand == SyncDemand::Exact => return Ok(Applied::Restart(RestartReason::Unconfirmed { count: unconfirmed.len() })),
      Readiness::Announced { .. } => Settle::Unconfirmed { paths: unconfirmed },
    };
    if request.demand == SyncDemand::Exact {
      self.check_exact(&pass.report)?;
    }
    Ok(Applied::Synced(pass.report))
  }

  fn needs_confirmation(&self, path: &Path) -> bool {
    self.spec.kind.language_id(path).is_some() || is_project_config(path)
  }

  fn check_exact(&self, report: &SyncReport) -> LspResult<()> {
    let incomplete = |reason: String| LspError::SyncIncomplete {
      server: self.spec.kind.name().to_owned(),
      reason,
    };
    if let Settle::Busy { status } = &report.settle {
      return Err(incomplete(format!("the server is still {status}")));
    }
    let not_overlaid = report
      .skipped
      .iter()
      .find(|skipped| self.spec.kind.language_id(&skipped.path).is_some() && matches!(skipped.reason, SkipReason::OverBudget { .. } | SkipReason::Unreadable(_)));
    match not_overlaid {
      Some(skipped) => Err(incomplete(format!("{} could not be sent as an overlay ({:?})", skipped.path.display(), skipped.reason))),
      None => Ok(()),
    }
  }

  pub(crate) fn close_documents(&self, paths: &[PathBuf]) -> LspResult<Vec<PathBuf>> {
    let mut pass = Pass::new(0);
    let mut sync = lock(&self.sync);
    for path in paths {
      self.close(&mut sync.docs, &canonical_path(path)?, &mut pass)?;
    }
    self.open_documents.store(sync.docs.len(), Ordering::Relaxed);
    Ok(pass.report.closed)
  }

  fn overlay(&self, docs: &mut OpenDocs, path: &Path, expected: Option<ChangeKind>, pass: &mut Pass) -> LspResult<()> {
    let Some(language_id) = self.spec.kind.language_id(path) else {
      pass.skip(path, SkipReason::UnsupportedLanguage);
      return Ok(());
    };
    let (stamp, bytes) = match read_disk(path) {
      Disk::Present(stamp, bytes) => (stamp, bytes),
      Disk::Missing => {
        self.forget_deleted(docs, path, pass)?;
        pass.events.insert(path.to_path_buf(), FileChangeType::Deleted);
        pass.drift(path, expected, None);
        pass.skip(path, SkipReason::Missing);
        return Ok(());
      }
      Disk::Unreadable(reason) => return self.give_up(docs, path, SkipReason::Unreadable(reason), pass),
    };
    let hash = ContentHash::of(&bytes);
    pass.drift(path, expected, Some(hash));
    if docs.get(path).is_some_and(|doc| doc.hash == hash) {
      docs.touch(path, stamp);
      pass.overlaid.insert(path.to_path_buf());
      return Ok(());
    }
    let Ok(text) = String::from_utf8(bytes) else {
      return self.give_up(docs, path, SkipReason::NotUtf8, pass);
    };
    if text.len() > pass.budget {
      return self.give_up(docs, path, SkipReason::OverBudget { bytes: text.len() }, pass);
    }
    pass.budget -= text.len();
    let bytes = text.len();
    let uri = uri_from_path(path)?;
    let version = docs.next_version();
    match docs.get(path) {
      Some(_) => {
        self.notify(
          "textDocument/didChange",
          DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier { uri, version },
            content_changes: vec![TextDocumentContentChangeEvent { range: None, text }],
          },
        )?;
        pass.report.changed.push(path.to_path_buf());
      }
      None => {
        self.notify(
          "textDocument/didOpen",
          DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
              uri,
              language_id: language_id.to_owned(),
              version,
              text,
            },
          },
        )?;
        pass.report.opened.push(path.to_path_buf());
      }
    }
    docs.record(path, version, hash, bytes, stamp);
    pass.overlaid.insert(path.to_path_buf());
    Ok(())
  }

  /// An open document that cannot be refreshed is closed, because its stale overlay would mask the file on disk.
  fn give_up(&self, docs: &mut OpenDocs, path: &Path, reason: SkipReason, pass: &mut Pass) -> LspResult<()> {
    if self.close(docs, path, pass)? {
      pass.events.entry(path.to_path_buf()).or_insert(FileChangeType::Changed);
    }
    pass.skip(path, reason);
    Ok(())
  }

  fn refresh(&self, docs: &mut OpenDocs, path: &Path, pass: &mut Pass) -> LspResult<()> {
    let stamp = match fs::metadata(path) {
      Ok(metadata) => FileStamp::of(&metadata),
      Err(error) if error.kind() == io::ErrorKind::NotFound => {
        self.close(docs, path, pass)?;
        pass.events.insert(path.to_path_buf(), FileChangeType::Deleted);
        pass.report.drifted.push(path.to_path_buf());
        return Ok(());
      }
      Err(error) => return self.give_up(docs, path, SkipReason::Unreadable(error.to_string()), pass),
    };
    if docs.get(path).is_some_and(|doc| doc.stamp == stamp) {
      return Ok(());
    }
    let before = pass.report.opened.len() + pass.report.changed.len();
    self.overlay(docs, path, None, pass)?;
    if pass.report.opened.len() + pass.report.changed.len() > before {
      pass.report.drifted.push(path.to_path_buf());
    }
    Ok(())
  }

  /// Closing a document whose file is gone makes tsserver drop the script at once (`closeOpenFile` -> `handleDeletedFile`).
  fn forget_deleted(&self, docs: &mut OpenDocs, path: &Path, pass: &mut Pass) -> LspResult<()> {
    let was_open = self.close(docs, path, pass)?;
    let confirmable = matches!(self.spec.readiness(), Readiness::Announced { .. }) && path.symlink_metadata().is_err();
    let Some(language_id) = self.spec.kind.language_id(path).filter(|_| confirmable) else {
      return Ok(());
    };
    if !was_open {
      let uri = uri_from_path(path)?;
      self.notify(
        "textDocument/didOpen",
        DidOpenTextDocumentParams {
          text_document: TextDocumentItem {
            uri: uri.clone(),
            language_id: language_id.to_owned(),
            version: docs.reserve_version(),
            text: String::new(),
          },
        },
      )?;
      self.notify(
        "textDocument/didClose",
        DidCloseTextDocumentParams {
          text_document: TextDocumentIdentifier { uri },
        },
      )?;
    }
    pass.overlaid.insert(path.to_path_buf());
    Ok(())
  }

  fn close(&self, docs: &mut OpenDocs, path: &Path, pass: &mut Pass) -> LspResult<bool> {
    if docs.get(path).is_none() {
      return Ok(false);
    }
    self.notify(
      "textDocument/didClose",
      DidCloseTextDocumentParams {
        text_document: TextDocumentIdentifier { uri: uri_from_path(path)? },
      },
    )?;
    docs.remove(path);
    pass.report.closed.push(path.to_path_buf());
    Ok(true)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_recognise_project_configuration_and_build_inputs() {
    assert!(is_project_config(Path::new("/w/tsconfig.json")));
    assert!(is_project_config(Path::new("/w/tsconfig.build.json")));
    assert!(is_project_config(Path::new("/w/jsconfig.json")));
    assert!(is_project_config(Path::new("/w/package.json")));
    assert!(!is_project_config(Path::new("/w/README.md")));
    assert!(is_build_input(Path::new("/w/Cargo.toml")));
    assert!(is_build_input(Path::new("/w/Cargo.lock")));
    assert!(is_build_input(Path::new("/w/build.rs")));
    assert!(is_build_input(Path::new("/w/.cargo/config.toml")));
    assert!(!is_build_input(Path::new("/w/src/config.rs")));
  }

  #[test]
  fn should_classify_the_server_generation_against_a_sync_request() {
    let run = |id| Some(RunId(id));

    assert!(matches!(freshness(run(3), run(2), run(3)), Freshness::Current));
    assert!(matches!(freshness(run(4), run(2), run(3)), Freshness::Current));
    assert!(matches!(freshness(run(2), run(2), run(3)), Freshness::Behind));
    assert!(matches!(freshness(None, None, run(1)), Freshness::Behind));
    assert!(matches!(freshness(run(1), run(2), run(3)), Freshness::Stale));
  }
}
