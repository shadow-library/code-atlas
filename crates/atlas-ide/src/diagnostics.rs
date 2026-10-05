use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use atlas_lsp::protocol::{Diagnostic, DiagnosticSeverity, DocumentDiagnosticParams, DocumentDiagnosticReport, Location, PublishDiagnosticsParams, TextDocumentIdentifier};
use atlas_lsp::{Lease, ServerKind, ServerState, SyncDemand, uri_from_path};
use atlas_query::Response;
use serde::Serialize;
use serde_json::json;

use crate::error::{IdeError, IdeResult};
use crate::map::{Enclosing, SiteMapper, SitePlace};
use crate::render::ServerView;
use crate::service::{IdeService, Session};
use crate::target::FileRef;

pub(crate) const PUSH_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const PUSH_QUIET: Duration = Duration::from_millis(300);
const CLOSE_PUBLISH: Duration = Duration::from_secs(1);
const FLYCHECK: &str = "rust-analyzer/flycheck";
const FLYCHECK_START: Duration = Duration::from_secs(5);
const FLYCHECK_PUBLISH: Duration = Duration::from_millis(500);
pub(crate) const NATIVE_SOURCE: &str = "rust-analyzer";
const CANNOT_FIND_MODULE: &str = "2307";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
  Error,
  Warning,
  Info,
  Hint,
}

impl Severity {
  pub(crate) fn of(severity: Option<DiagnosticSeverity>) -> Self {
    match severity {
      Some(DiagnosticSeverity::Error) | None => Self::Error,
      Some(DiagnosticSeverity::Warning) => Self::Warning,
      Some(DiagnosticSeverity::Information) => Self::Info,
      Some(DiagnosticSeverity::Hint) => Self::Hint,
    }
  }

  pub fn as_str(self) -> &'static str {
    match self {
      Self::Error => "error",
      Self::Warning => "warning",
      Self::Info => "info",
      Self::Hint => "hint",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LineRange {
  pub start: u32,
  pub end: u32,
}

impl LineRange {
  pub fn parse(text: &str) -> IdeResult<Self> {
    let invalid = || IdeError::InvalidRequest(format!("lines must be START-END with 1-based START <= END, got {text:?}"));
    let (start, end) = text.trim().split_once('-').ok_or_else(invalid)?;
    let (start, end) = (start.trim().parse::<u32>().map_err(|_| invalid())?, end.trim().parse::<u32>().map_err(|_| invalid())?);
    match start >= 1 && start <= end {
      true => Ok(Self { start, end }),
      false => Err(invalid()),
    }
  }

  fn contains(&self, line: u32) -> bool {
    self.start <= line && line <= self.end
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsRequest {
  pub file: String,
  pub lines: Option<LineRange>,
  pub min_severity: Severity,
}

impl DiagnosticsRequest {
  pub fn new(file: impl Into<String>) -> Self {
    Self {
      file: file.into(),
      lines: None,
      min_severity: Severity::Hint,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Collection {
  Pulled,
  Settled,
  TimedOut { waited_ms: u64, published: Unsettled },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unsettled {
  Nothing,
  StillChanging,
}

pub const NOTHING_PUBLISHED: &str = "nothing known: the server published no diagnostics within 10 s";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Flycheck {
  Complete,
  TimedOut,
  NotRun,
}

impl Flycheck {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Complete => "complete",
      Self::TimedOut => "timed_out",
      Self::NotRun => "not_run",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiagnosticItem {
  pub place: SitePlace,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub code: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub source: Option<String>,
  pub message: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub unverified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SymbolDiagnostics {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub enclosing: Option<Enclosing>,
  pub items: Vec<DiagnosticItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeverityGroup {
  pub severity: Severity,
  pub symbols: Vec<SymbolDiagnostics>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExactDiagnostics {
  pub file: FileRef,
  pub server: ServerView,
  pub collection: Collection,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub flycheck: Option<Flycheck>,
  pub total: usize,
  pub shown: usize,
  pub groups: Vec<SeverityGroup>,
  pub caveats: Vec<String>,
}

struct Collected {
  diagnostics: Vec<Diagnostic>,
  partial: Vec<String>,
  collection: Collection,
  flycheck: Option<Flycheck>,
  caveats: Vec<String>,
}

impl IdeService {
  pub fn diagnostics(&self, request: &DiagnosticsRequest) -> IdeResult<Response<ExactDiagnostics>> {
    self.ensure_enabled()?;
    let files = self.files();
    let freshness = self.query.fresh()?;
    let file = files.locate_spec(&request.file)?;
    let (server, (collected, items)) = self.exact(&freshness, &files, &file, SyncDemand::Best, std::slice::from_ref(&file.absolute), |session| {
      let collected = collect(session, &file.absolute)?;
      session.notes.partial.extend(collected.partial.iter().cloned());
      let uri = uri_from_path(&file.absolute).map_err(atlas_lsp::LspError::from)?;
      let mut mapper = SiteMapper::new(session);
      let mut items = Vec::with_capacity(collected.diagnostics.len());
      for diagnostic in &collected.diagnostics {
        let site = mapper.map(&Location {
          uri: uri.clone(),
          range: diagnostic.range,
        })?;
        items.push((
          Severity::of(diagnostic.severity),
          site.enclosing,
          DiagnosticItem {
            place: site.place,
            code: diagnostic.code.as_ref().map(ToString::to_string),
            source: diagnostic.source.clone(),
            message: diagnostic.message.trim().to_owned(),
            unverified: site.unverified,
          },
        ));
      }
      Ok((collected, items))
    })?;
    let mut caveats = collected.caveats;
    caveats.extend(unresolved_modules(&items));
    let total = items.len();
    let shown: Vec<_> = items
      .into_iter()
      .filter(|(severity, _, item)| *severity <= request.min_severity && request.lines.is_none_or(|lines| lines.contains(item.place.line)))
      .collect();
    Ok(Response {
      freshness,
      answer: ExactDiagnostics {
        file,
        server,
        collection: collected.collection,
        flycheck: collected.flycheck,
        total,
        shown: shown.len(),
        groups: grouped(shown),
        caveats,
      },
    })
  }
}

pub(crate) struct Republished {
  pub after: u64,
  pub version: i32,
  pub quiet: Quiet,
}

pub(crate) fn republish(session: &mut Session<'_>, paths: &[PathBuf]) -> IdeResult<Vec<(PathBuf, Result<Republished, String>)>> {
  if session.kind != ServerKind::TypeScript {
    return Ok(
      paths
        .iter()
        .map(|path| {
          let after = session.lease.diagnostics_sequence();
          let touched = match session.lease.touch_document(path) {
            Ok(Some(version)) => Ok(Republished {
              after,
              version,
              quiet: ANY_PUBLISH,
            }),
            Ok(None) => Err("the file is not open in the server or changed while checking".to_owned()),
            Err(error) => Err(format!("re-sending the file failed: {error}")),
          };
          (path.clone(), touched)
        })
        .collect(),
    );
  }
  let before = session.lease.diagnostics_sequence();
  let closed = session.lease.close_documents(paths)?;
  let deadline = Instant::now() + CLOSE_PUBLISH;
  for path in &closed {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if !remaining.is_zero() {
      session.lease.wait_diagnostics(path, before, remaining);
    }
  }
  let after = session.lease.diagnostics_sequence();
  let generation = session.lease.synced_generation();
  session.resync(generation, SyncDemand::Best, paths)?;
  Ok(
    paths
      .iter()
      .map(|path| {
        let reopened = match session.lease.open_document(path) {
          Some(document) => Ok(Republished {
            after,
            version: document.version,
            quiet: TYPESCRIPT_REOPEN,
          }),
          None => Err("the file could not be reopened in the server".to_owned()),
        };
        (path.clone(), reopened)
      })
      .collect(),
  )
}

fn collect(session: &mut Session<'_>, path: &Path) -> IdeResult<Collected> {
  let mut caveats = Vec::new();
  let mut partial = Vec::new();
  let (mut diagnostics, collection, version) = match session.capabilities().diagnostic_provider.clone() {
    Some(options) => (pull(session, path, options.identifier.clone())?, Collection::Pulled, None),
    None => {
      if session.lease.open_document(path).is_none() {
        return Err(IdeError::NotOpened {
          file: path.display().to_string(),
          server: session.server_name().to_owned(),
          reason: "it is not open after syncing (too large or unreadable)".to_owned(),
        });
      }
      let republished = republish(session, &[path.to_path_buf()])?.into_iter().next().map(|(_, republished)| republished);
      let Some(Ok(Republished { after, version, quiet })) = republished else {
        return Err(IdeError::SourceChanged { file: path.display().to_string() });
      };
      let (published, collection) = wait_published(&session.lease, path, after, version, Instant::now() + session.diagnostics_timeout(), quiet)?;
      if let Collection::TimedOut { published, .. } = &collection {
        partial.push(match published {
          Unsettled::StillChanging => "diagnostics were still changing when the 10 s wait ended".to_owned(),
          Unsettled::Nothing => NOTHING_PUBLISHED.to_owned(),
        });
      }
      (published.map(|params| params.diagnostics).unwrap_or_default(), collection, Some(version))
    }
  };
  let lease = &session.lease;
  let flycheck = match session.kind {
    ServerKind::RustAnalyzer => Some(flycheck(session, path)?),
    ServerKind::TypeScript => None,
  };
  match (flycheck, &collection) {
    (Some(Flycheck::Complete), Collection::Pulled) => merge(&mut diagnostics, latest(lease, path, None)),
    (Some(Flycheck::Complete), _) => diagnostics = latest(lease, path, version),
    (Some(state), _) => {
      let before = diagnostics.len();
      diagnostics.retain(|diagnostic| diagnostic.source.as_deref() == Some(NATIVE_SOURCE));
      partial.push(format!(
        "cargo check (flycheck) {}; only rust-analyzer's native diagnostics are shown",
        match state {
          Flycheck::TimedOut => "did not finish in time",
          Flycheck::NotRun | Flycheck::Complete => "did not run",
        }
      ));
      if before > diagnostics.len() {
        caveats.push(format!("{} results of an earlier cargo check were omitted", before - diagnostics.len()));
      }
    }
    (None, _) => {}
  }
  Ok(Collected {
    diagnostics,
    partial,
    collection,
    flycheck,
    caveats,
  })
}

pub(crate) fn pull(session: &Session<'_>, path: &Path, identifier: Option<String>) -> IdeResult<Vec<Diagnostic>> {
  let report: DocumentDiagnosticReport = session.request(
    "textDocument/diagnostic",
    DocumentDiagnosticParams {
      text_document: TextDocumentIdentifier {
        uri: uri_from_path(path).map_err(atlas_lsp::LspError::from)?,
      },
      identifier,
      previous_result_id: None,
    },
  )?;
  match report {
    DocumentDiagnosticReport::Full { items, .. } => Ok(items),
    DocumentDiagnosticReport::Unchanged { .. } => Err(IdeError::Lsp(atlas_lsp::LspError::Decode {
      method: "textDocument/diagnostic".to_owned(),
      message: "the server answered `unchanged` to a request without a previous result id".to_owned(),
    })),
  }
}

pub(crate) fn accepts(params: &PublishDiagnosticsParams, version: i32) -> bool {
  params.version.is_none_or(|published| published >= version)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Quiet {
  pub publishes: usize,
  pub lone: Duration,
}

pub(crate) const ANY_PUBLISH: Quiet = Quiet { publishes: 1, lone: PUSH_QUIET };

pub(crate) const TYPESCRIPT_REOPEN: Quiet = Quiet {
  publishes: 2,
  lone: Duration::from_millis(1500),
};

pub(crate) fn wait_published(lease: &Lease, path: &Path, after: u64, version: i32, deadline: Instant, quiet: Quiet) -> IdeResult<(Option<PublishDiagnosticsParams>, Collection)> {
  let started = Instant::now();
  let mut cursor = after;
  let mut accepted: Option<PublishDiagnosticsParams> = None;
  let mut count = 0;
  loop {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let required = match count >= quiet.publishes {
      true => PUSH_QUIET,
      false => quiet.lone,
    };
    let window = match accepted {
      Some(_) => required.min(remaining),
      None => remaining,
    };
    let arrived = match window.is_zero() {
      true => None,
      false => lease.wait_diagnostics(path, cursor, window),
    };
    if arrived.is_none() {
      if let ServerState::Exited { reason } = lease.state() {
        return Err(IdeError::Unavailable {
          server: lease.kind().name().to_owned(),
          reason: format!("it exited while publishing diagnostics ({reason})"),
          alternative: None,
        });
      }
      let waited_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
      let collection = match (&accepted, window >= required) {
        (Some(_), true) => Collection::Settled,
        (Some(_), false) => Collection::TimedOut {
          waited_ms,
          published: Unsettled::StillChanging,
        },
        (None, _) => Collection::TimedOut {
          waited_ms,
          published: Unsettled::Nothing,
        },
      };
      return Ok((accepted, collection));
    }
    let Some((sequence, params)) = lease.diagnostics(path) else { continue };
    cursor = sequence;
    if accepts(&params, version) {
      accepted = Some(params);
      count += 1;
    }
  }
}

fn latest(lease: &Lease, path: &Path, version: Option<i32>) -> Vec<Diagnostic> {
  let settled = lease.diagnostics_sequence();
  lease.wait_diagnostics(path, settled, FLYCHECK_PUBLISH);
  lease
    .diagnostics(path)
    .map(|(_, params)| params)
    .filter(|params| version.is_none_or(|version| accepts(params, version)))
    .map(|params| params.diagnostics)
    .unwrap_or_default()
}

fn merge(into: &mut Vec<Diagnostic>, more: Vec<Diagnostic>) {
  for diagnostic in more {
    let duplicate = into
      .iter()
      .any(|known| known.range == diagnostic.range && known.message == diagnostic.message && known.code == diagnostic.code);
    if !duplicate {
      into.push(diagnostic);
    }
  }
}

fn flycheck(session: &Session<'_>, path: &Path) -> IdeResult<Flycheck> {
  let lease = &session.lease;
  let before = lease.progress(FLYCHECK);
  lease.notify(
    "rust-analyzer/runFlycheck",
    json!({"textDocument": {"uri": uri_from_path(path).map_err(atlas_lsp::LspError::from)?}}),
  )?;
  let started = lease.wait_progress(FLYCHECK, FLYCHECK_START, |count| count.begun > before.begun);
  if started.begun == before.begun {
    return Ok(Flycheck::NotRun);
  }
  let timeout = Duration::from_secs(session.settings.check_timeout_secs);
  let finished = lease.wait_progress(FLYCHECK, timeout, |count| count.active == 0 && count.ended > before.ended);
  match finished.active == 0 && finished.ended > before.ended {
    true => Ok(Flycheck::Complete),
    false => Ok(Flycheck::TimedOut),
  }
}

fn unresolved_modules(items: &[(Severity, Option<Enclosing>, DiagnosticItem)]) -> Vec<String> {
  let mut modules: Vec<String> = items
    .iter()
    .filter(|(_, _, item)| item.code.as_deref() == Some(CANNOT_FIND_MODULE))
    .filter_map(|(_, _, item)| quoted(&item.message))
    .collect();
  modules.sort();
  modules.dedup();
  modules
    .into_iter()
    .map(|module| format!("the language server cannot resolve {module}; references through it are invisible to exact operations"))
    .collect()
}

fn quoted(message: &str) -> Option<String> {
  let start = message.find('\'')? + 1;
  let end = start + message[start..].find('\'')?;
  Some(message[start..end].to_owned())
}

fn enclosing_key(enclosing: &Option<Enclosing>) -> String {
  match enclosing {
    Some(Enclosing::Graph { symbol }) => symbol.key.clone(),
    Some(Enclosing::Lsp { name, kind }) => format!("{kind} {name}"),
    None => String::new(),
  }
}

fn grouped(mut items: Vec<(Severity, Option<Enclosing>, DiagnosticItem)>) -> Vec<SeverityGroup> {
  items.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.2.place.cmp(&right.2.place)));
  let mut groups: Vec<SeverityGroup> = Vec::new();
  for (severity, enclosing, item) in items {
    if groups.last().is_none_or(|last| last.severity != severity) {
      groups.push(SeverityGroup { severity, symbols: Vec::new() });
    }
    let Some(group) = groups.last_mut() else { continue };
    let key = enclosing_key(&enclosing);
    match group.symbols.iter_mut().find(|symbol| enclosing_key(&symbol.enclosing) == key) {
      Some(symbol) => symbol.items.push(item),
      None => group.symbols.push(SymbolDiagnostics { enclosing, items: vec![item] }),
    }
  }
  groups
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_parse_line_ranges_and_refuse_inverted_ones() {
    assert_eq!(LineRange::parse("3-9").unwrap(), LineRange { start: 3, end: 9 });
    assert_eq!(LineRange::parse(" 4 - 4 ").unwrap(), LineRange { start: 4, end: 4 });
    assert!(LineRange::parse("9-3").is_err());
    assert!(LineRange::parse("0-3").is_err());
    assert!(LineRange::parse("3").is_err());
  }

  #[test]
  fn should_name_modules_typescript_cannot_resolve() {
    let item = |code: &str, message: &str| {
      (
        Severity::Error,
        None,
        DiagnosticItem {
          place: SitePlace {
            repo: None,
            path: "a.ts".to_owned(),
            line: 1,
            column: Some(1),
          },
          code: Some(code.to_owned()),
          source: Some("typescript".to_owned()),
          message: message.to_owned(),
          unverified: None,
        },
      )
    };
    let items = vec![
      item("2307", "Cannot find module '@acme/sdk' or its corresponding type declarations."),
      item("2307", "Cannot find module '@acme/sdk' or its corresponding type declarations."),
      item("2304", "Cannot find name 'x'."),
    ];

    assert_eq!(
      unresolved_modules(&items),
      vec!["the language server cannot resolve @acme/sdk; references through it are invisible to exact operations".to_owned()]
    );
  }
}
