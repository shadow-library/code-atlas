use atlas_core::Certainty;
use atlas_lsp::{UnitPhase, UnitScope, UnitStatus};
use atlas_query::{EntityRef, RenderText, TextOut};
use serde::Serialize;

use crate::declaration::{ExactDeclaration, ExactImplementations};
use crate::diagnostics::{Collection, ExactDiagnostics, NOTHING_PUBLISHED, Unsettled};
use crate::edit::{ChangedFile, DiagnosticsDelta, DryRunReason, EditOutcome, EditStatus, OperationView};
use crate::map::{Site, SitePlace};
use crate::outline::{ExactOutline, Outline, OutlineSymbol};
use crate::references::{ExactReferences, GraphExtra, References, TargetView};
use crate::servers::IdeMode;
use crate::sync::SyncNotes;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Completeness {
  Complete,
  Partial { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExactCertainty {
  pub class: String,
  pub tag: String,
  pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerView {
  pub server: String,
  pub version: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub typescript: Option<String>,
  pub certainty: ExactCertainty,
  pub completeness: Completeness,
  pub caveats: Vec<String>,
  pub warnings: Vec<String>,
}

impl ServerView {
  pub(crate) fn new(server: &str, version: String, typescript: Option<String>, notes: &SyncNotes) -> Self {
    let certainty = Certainty::CompilerVerified;
    Self {
      server: server.to_owned(),
      certainty: ExactCertainty {
        class: certainty.class().as_str().to_owned(),
        tag: certainty.tag(),
        source: format!("lsp:{server} {version}"),
      },
      version,
      typescript,
      completeness: notes.completeness(),
      caveats: notes.caveats.clone(),
      warnings: notes.warnings.clone(),
    }
  }

  pub fn label(&self) -> String {
    let versions = match &self.typescript {
      Some(typescript) => format!("{} {}, TypeScript {typescript}", self.server, self.version),
      None => format!("{} {}", self.server, self.version),
    };
    format!("({versions}) {}", self.certainty.tag)
  }

  fn head(&self, out: &mut TextOut, title: String) {
    out.line(0, format!("{title}  {}", self.label()));
    if let Completeness::Partial { reason } = &self.completeness {
      out.line(0, format!("PARTIAL: {reason}; the answer may be incomplete"));
    }
  }

  fn tail(&self, out: &mut TextOut, extra: &[String]) {
    let caveats: Vec<&String> = self.caveats.iter().chain(extra).collect();
    if !caveats.is_empty() {
      out.line(0, "caveats:");
      for caveat in caveats {
        out.line(1, caveat);
      }
    }
    if !self.warnings.is_empty() {
      out.line(0, "warnings:");
      for warning in &self.warnings {
        out.line(1, warning);
      }
    }
  }
}

fn relative(place: &SitePlace) -> String {
  match place.column {
    Some(column) => format!("{}:{}:{column}", place.path, place.line),
    None => format!("{}:{}", place.path, place.line),
  }
}

fn site_line(label: &str, site: &Site) -> String {
  let mut line = format!("{label}  {}", relative(&site.place));
  if let Some(snippet) = &site.snippet {
    line = format!("{line}  {snippet}");
  }
  if let Some(reason) = &site.unverified {
    line = format!("{line}  (unverified: {reason})");
  }
  line
}

fn enclosing_label(site: &Site) -> String {
  site.enclosing.as_ref().map_or_else(|| "<file>".to_owned(), |enclosing| enclosing.label())
}

fn entity_text(entity: &EntityRef) -> String {
  format!("{} {}", entity.tagged_short(), entity.kind)
}

fn target_text(target: &TargetView) -> String {
  match target {
    TargetView::Symbol { symbol, name } => format!("{} {name}", entity_text(symbol)),
    TargetView::Position { position } => format!("at {position}"),
  }
}

fn extra_line(extra: &GraphExtra) -> String {
  let reason = match &extra.reason {
    crate::references::ExtraReason::OtherRepo => "other repo".to_owned(),
    crate::references::ExtraReason::NotInServerProject => "not in the server's project".to_owned(),
    crate::references::ExtraReason::Mock { rule } => format!("mock ({rule})"),
    crate::references::ExtraReason::Http { endpoint } => format!("http {endpoint}"),
    crate::references::ExtraReason::Unresolved { detail } => format!("unresolved {detail} [unresolved]"),
  };
  let mut line = match &extra.certainty {
    Some(certainty) => format!("{reason} {}", certainty.tag),
    None => reason,
  };
  if let Some(location) = &extra.location {
    line = format!("{line}  {location}");
  }
  if let Some(from) = &extra.from {
    line = format!("{line}  {}", from.tagged_short());
  }
  match &extra.snippet {
    Some(snippet) => format!("{line}  {snippet}"),
    None => line,
  }
}

impl RenderText for ExactReferences {
  fn render_text(&self, out: &mut TextOut) {
    self.server.head(out, format!("references {}", target_text(&self.target)));
    if self.groups.is_empty() {
      out.line(1, "(no references)");
    }
    for group in &self.groups {
      let title = group.repo.clone().unwrap_or_else(|| "outside the project".to_owned());
      out.line(0, format!("{title} ({})", group.total));
      for symbol in &group.symbols {
        for site in &symbol.sites {
          out.line(1, site_line(&enclosing_label(site), site));
        }
      }
    }
    out.line(0, "outside the language server's view:");
    if self.outside_server_view.is_empty() {
      out.line(1, "(none)");
    }
    for extra in &self.outside_server_view {
      out.line(1, extra_line(extra));
    }
    self.server.tail(out, &self.caveats);
  }
}

impl RenderText for References {
  fn render_text(&self, out: &mut TextOut) {
    match self {
      Self::Exact(answer) => answer.render_text(out),
      Self::Graph(answer) => answer.render_text(out),
    }
  }
}

impl RenderText for ExactDeclaration {
  fn render_text(&self, out: &mut TextOut) {
    self.server.head(out, format!("declaration at {}", self.at));
    if self.definitions.is_empty() && self.declarations.is_empty() {
      out.line(1, "(no definition found)");
    }
    for (kind, sites) in [("definition", &self.definitions), ("declaration", &self.declarations)] {
      for site in sites {
        let place = match &site.place.repo {
          Some(_) => site.place.to_string(),
          None => format!("{} (outside the project)", site.place),
        };
        let mut line = format!("{kind}  {}  {place}", enclosing_label(site));
        if let Some(snippet) = &site.snippet {
          line = format!("{line}  {snippet}");
        }
        if let Some(reason) = &site.unverified {
          line = format!("{line}  (unverified: {reason})");
        }
        out.line(1, line);
      }
    }
    self.server.tail(out, &[]);
  }
}

impl RenderText for ExactImplementations {
  fn render_text(&self, out: &mut TextOut) {
    self.server.head(out, format!("implementations of {}", target_text(&self.target)));
    if self.sites.is_empty() {
      out.line(1, "(no implementations)");
    }
    for site in &self.sites {
      let label = match &site.place.repo {
        Some(repo) => format!("{repo}  {}", enclosing_label(site)),
        None => format!("outside the project  {}", enclosing_label(site)),
      };
      out.line(1, site_line(&label, site));
    }
    self.server.tail(out, &[]);
  }
}

fn outline_nodes(out: &mut TextOut, indent: usize, symbols: &[OutlineSymbol]) {
  for symbol in symbols {
    let mut line = format!("{} {} {}:{}", symbol.name, symbol.kind, symbol.line, symbol.column);
    if let Some(detail) = &symbol.detail {
      line = format!("{line}  {detail}");
    }
    out.line(indent, line);
    outline_nodes(out, indent + 1, &symbol.children);
  }
}

impl RenderText for ExactOutline {
  fn render_text(&self, out: &mut TextOut) {
    self.server.head(out, format!("outline {}", self.file));
    if self.symbols.is_empty() {
      out.line(1, "(no symbols)");
    }
    outline_nodes(out, 1, &self.symbols);
    self.server.tail(out, &[]);
  }
}

impl RenderText for Outline {
  fn render_text(&self, out: &mut TextOut) {
    match self {
      Self::Exact(answer) => answer.render_text(out),
      Self::Graph(answer) => answer.render_text(out),
    }
  }
}

impl RenderText for ExactDiagnostics {
  fn render_text(&self, out: &mut TextOut) {
    let collection = match &self.collection {
      Collection::Pulled => "pulled".to_owned(),
      Collection::Settled => "settled".to_owned(),
      Collection::TimedOut { waited_ms, .. } => format!("timed out after {waited_ms} ms"),
    };
    let flycheck = self.flycheck.map_or_else(String::new, |flycheck| format!(", flycheck: {}", flycheck.as_str()));
    self.server.head(out, format!("diagnostics {}  [{collection}{flycheck}]", self.file));
    if self.shown < self.total {
      out.line(0, format!("{} of {} diagnostics match the filters", self.shown, self.total));
    }
    match (&self.collection, self.groups.is_empty()) {
      (
        Collection::TimedOut {
          published: Unsettled::Nothing, ..
        },
        _,
      ) => out.line(1, NOTHING_PUBLISHED),
      (_, true) => out.line(1, "(no diagnostics)"),
      (_, false) => {}
    }
    for group in &self.groups {
      let count: usize = group.symbols.iter().map(|symbol| symbol.items.len()).sum();
      out.line(0, format!("{} ({count})", group.severity.as_str()));
      for symbol in &group.symbols {
        out.line(1, symbol.enclosing.as_ref().map_or_else(|| "<file>".to_owned(), |enclosing| enclosing.label()));
        for item in &symbol.items {
          let place = match item.place.column {
            Some(column) => format!("{}:{column}", item.place.line),
            None => item.place.line.to_string(),
          };
          let code = item.code.as_ref().map_or_else(String::new, |code| format!(" {code}"));
          let mut line = format!("{place}{code} {}", item.message.lines().next().unwrap_or_default());
          if let Some(reason) = &item.unverified {
            line = format!("{line}  (unverified: {reason})");
          }
          out.line(2, line);
        }
      }
    }
    self.server.tail(out, &self.caveats);
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnitView {
  pub server: String,
  pub scope: String,
  pub phase: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub pid: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub state: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub uptime_secs: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub idle_secs: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub rss_kib: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub version: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub typescript: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub open_documents: Option<usize>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub backoff: Option<String>,
  pub warnings: Vec<String>,
}

impl UnitView {
  pub(crate) fn of(status: &UnitStatus, rss_kib: Option<u64>, lockfiles: &[String]) -> Self {
    let scope = match &status.key.scope {
      UnitScope::Project => "project".to_owned(),
      UnitScope::Repo(repo) => format!("repo {repo}"),
    };
    let mut view = Self {
      server: status.key.kind.name().to_owned(),
      scope,
      phase: String::new(),
      pid: None,
      state: None,
      uptime_secs: None,
      idle_secs: None,
      rss_kib,
      version: None,
      typescript: None,
      open_documents: None,
      backoff: None,
      warnings: lockfiles.iter().map(|path| format!("{path} was rewritten by rust-analyzer since it started")).collect(),
    };
    let summary = match &status.phase {
      UnitPhase::Starting => {
        view.phase = "starting".to_owned();
        None
      }
      UnitPhase::Warming(summary) => {
        view.phase = "warming".to_owned();
        Some(summary)
      }
      UnitPhase::Running(summary) => {
        view.phase = "running".to_owned();
        Some(summary)
      }
      UnitPhase::Backoff { failures, retry_in, last_failure } => {
        view.phase = "backoff".to_owned();
        view.backoff = Some(format!("{failures} failures; retrying in {}s; last: {last_failure}", retry_in.as_secs()));
        None
      }
    };
    if let Some(summary) = summary {
      view.pid = summary.pid;
      view.state = Some(summary.state.to_string());
      view.uptime_secs = Some(summary.uptime.as_secs());
      view.idle_secs = Some(summary.idle.as_secs());
      view.version = summary.server.as_ref().and_then(|info| info.version.clone());
      view.typescript = summary.typescript.as_ref().map(|typescript| typescript.version.clone());
      view.open_documents = Some(summary.open_documents);
    }
    view
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatusAnswer {
  pub enabled: bool,
  pub mode: IdeMode,
  pub units: Vec<UnitView>,
}

impl RenderText for StatusAnswer {
  fn render_text(&self, out: &mut TextOut) {
    let mode = match self.mode {
      IdeMode::Warm => "warm",
      IdeMode::OneShot => "one-shot",
    };
    let enabled = if self.enabled { "enabled" } else { "disabled (lsp.enabled = false)" };
    out.line(0, format!("language servers: {enabled}, {mode}"));
    if self.units.is_empty() {
      out.line(1, "(none running)");
    }
    for unit in &self.units {
      let mut line = format!("{} [{}] {}", unit.server, unit.scope, unit.phase);
      if let Some(pid) = unit.pid {
        line = format!("{line} pid={pid}");
      }
      if let Some(state) = &unit.state {
        line = format!("{line} state={state}");
      }
      if let Some(uptime) = unit.uptime_secs {
        line = format!("{line} uptime={uptime}s");
      }
      if let Some(rss) = unit.rss_kib {
        line = format!("{line} rss={}MiB", rss / 1024);
      }
      if let Some(version) = &unit.version {
        line = format!("{line} version={version}");
      }
      if let Some(typescript) = &unit.typescript {
        line = format!("{line} typescript={typescript}");
      }
      if let Some(backoff) = &unit.backoff {
        line = format!("{line} ({backoff})");
      }
      out.line(1, line);
      for warning in &unit.warnings {
        out.line(2, format!("warning: {warning}"));
      }
    }
  }
}

impl ServerView {
  pub fn versions(&self) -> String {
    match &self.typescript {
      Some(typescript) => format!("{} {}, TypeScript {typescript}", self.server, self.version),
      None => format!("{} {}", self.server, self.version),
    }
  }
}

fn servers_suffix(servers: &[ServerView]) -> String {
  match servers.is_empty() {
    true => String::new(),
    false => format!("  ({})", servers.iter().map(ServerView::versions).collect::<Vec<_>>().join("; ")),
  }
}

fn edit_counts(files: &[ChangedFile]) -> String {
  let edits: usize = files.iter().map(|file| file.edits).sum();
  format!("{} file(s), {edits} edit(s)", files.len())
}

#[derive(Debug, Clone, Copy)]
pub struct EditView<'a> {
  pub outcome: &'a EditOutcome,
  pub plan_note: &'a str,
}

impl RenderText for EditView<'_> {
  fn render_text(&self, out: &mut TextOut) {
    self.outcome.render_outcome(out, Some(self.plan_note));
  }
}

impl RenderText for EditOutcome {
  fn render_text(&self, out: &mut TextOut) {
    self.render_outcome(out, None);
  }
}

impl EditOutcome {
  fn render_outcome(&self, out: &mut TextOut, plan_note: Option<&str>) {
    let operation = self.operation.to_string();
    let servers = servers_suffix(&self.servers);
    match &self.status {
      EditStatus::Applied => out.line(0, format!("{operation}  applied{servers}")),
      EditStatus::DryRun { plan_id, valid_secs, reason } => {
        let why = match reason {
          DryRunReason::Requested => "dry run",
          DryRunReason::NeedsSelection => "dry run (no occurrences or expected_count given)",
        };
        let validity = plan_note.map_or_else(|| format!("apply within {} min", valid_secs / 60), str::to_owned);
        out.line(0, format!("{operation}  {why}, plan {plan_id} ({validity}){servers}"));
      }
      EditStatus::Refused { reason } => out.line(0, format!("REFUSED: {operation}: {reason}{servers}")),
      EditStatus::RolledBack { cause } => out.line(0, format!("FAILED: {operation} was rolled back ({cause}); no file was changed{servers}")),
      EditStatus::PartiallyApplied {
        cause,
        still_changed,
        rollback_errors,
      } => out.line(
        0,
        format!(
          "DEGRADED: {operation} was partially applied and the rollback failed ({cause}); still changed: {}; rollback errors: {}{servers}",
          still_changed.join(", "),
          rollback_errors.join("; ")
        ),
      ),
    }
    if !self.unloaded_projects.is_empty() {
      let consequence = match (&self.status, self.allow_unverified) {
        (EditStatus::DryRun { .. }, false) => "; applying this plan refuses unless allow_unverified is set",
        _ => "",
      };
      out.line(
        0,
        format!(
          "UNLOADED: the language server could not load {}, so sites there were not checked{consequence}",
          self.unloaded_projects.join(", ")
        ),
      );
    }
    if !self.unverified.is_empty() {
      let consequence = match (&self.status, self.allow_unverified) {
        (EditStatus::DryRun { .. }, false) => "; applying this plan refuses unless allow_unverified is set",
        (EditStatus::DryRun { .. }, true) => "; allow_unverified is set, so applying this plan leaves them as they are",
        _ => "",
      };
      out.line(
        0,
        format!(
          "UNVERIFIED: {} site(s) the index resolves to this symbol in the same repo were not renamed by the language server{consequence}:",
          self.unverified.len()
        ),
      );
      for site in &self.unverified {
        out.line(1, site.to_string());
      }
    }
    let heading = match &self.status {
      EditStatus::Applied => "changed",
      EditStatus::DryRun { .. } => "would change",
      EditStatus::Refused { .. } | EditStatus::RolledBack { .. } => "planned, not written:",
      EditStatus::PartiallyApplied { .. } => "planned:",
    };
    if !self.files.is_empty() || !self.resources.is_empty() {
      match heading.ends_with(':') {
        true => out.line(0, format!("{heading} {}", edit_counts(&self.files))),
        false => out.line(0, format!("{heading} {}:", edit_counts(&self.files))),
      }
    }
    let width = self.files.iter().map(|file| file.label().len()).max().unwrap_or(0);
    for file in &self.files {
      out.line(1, format!("{:width$}  {}", file.label(), file.edits));
      if let EditStatus::DryRun { .. } = self.status {
        for hunk in &file.hunks {
          for line in hunk.lines() {
            out.line(2, line);
          }
        }
      }
    }
    for resource in &self.resources {
      out.line(1, resource);
    }
    if !self.candidates.is_empty() {
      out.line(0, "candidates:");
      for candidate in &self.candidates {
        out.line(1, format!("{}  {}", entity_text(candidate), candidate.key));
      }
    }
    if !self.blockers.is_empty() {
      out.line(0, format!("blocked by {} reference(s):", self.blockers.len()));
      for blocker in &self.blockers {
        out.line(1, blocker.to_string());
      }
    }
    if let OperationView::Rename { .. } = self.operation
      && !matches!(self.status, EditStatus::Refused { .. })
    {
      out.line(0, "not renamed, verify:");
      if self.not_renamed.is_empty() {
        out.line(1, "(none)");
      }
      for site in &self.not_renamed {
        out.line(1, site.to_string());
      }
    }
    if !self.applied_from_graph.is_empty() {
      out.line(0, "applied from the graph:");
      for site in &self.applied_from_graph {
        out.line(1, site.to_string());
      }
    }
    if !self.skipped.is_empty() {
      out.line(0, "skipped files:");
      for skipped in &self.skipped {
        out.line(1, skipped);
      }
    }
    if let Some(diagnostics) = &self.diagnostics {
      render_delta(out, diagnostics);
    }
    let mut caveats: Vec<&String> = self.caveats.iter().collect();
    let mut warnings: Vec<String> = self.warnings.clone();
    for server in &self.servers {
      caveats.extend(&server.caveats);
      warnings.extend(server.warnings.iter().cloned());
      if let Completeness::Partial { reason } = &server.completeness {
        warnings.push(format!("{} is in a partial state after the edit: {reason}", server.server));
      }
    }
    if !caveats.is_empty() {
      out.line(0, "caveats:");
      for caveat in caveats {
        out.line(1, caveat);
      }
    }
    if !warnings.is_empty() {
      out.line(0, "warnings:");
      for warning in warnings {
        out.line(1, warning);
      }
    }
  }
}

fn render_delta(out: &mut TextOut, delta: &DiagnosticsDelta) {
  match delta {
    DiagnosticsDelta::Unavailable { reason } => out.line(0, format!("diagnostics after edit: not checked ({reason})")),
    DiagnosticsDelta::Checked {
      files,
      checked,
      new_errors,
      unsettled,
      native_only,
    } => {
      let state = match unsettled.is_empty() {
        true => "settled".to_owned(),
        false => format!("{} of {checked} checked file(s) not known", unsettled.len()),
      };
      let (kind, suffix) = match native_only {
        true => ("native errors", format!("{state}; cargo check not run")),
        false => ("errors", state),
      };
      let scope = match files > checked {
        true => format!("in the {checked} checked file(s) ({} of {files} edited files not checked)", files - checked),
        false => format!("in the {files} edited file(s)"),
      };
      match new_errors.is_empty() {
        true => out.line(0, format!("diagnostics after edit: no new {kind} {scope} ({suffix})")),
        false => out.line(0, format!("diagnostics after edit: {} new {kind} {scope} ({suffix}):", new_errors.len())),
      }
      for error in new_errors {
        let place = match error.column {
          Some(column) => format!("{}:{}:{column}", error.file, error.line),
          None => format!("{}:{}", error.file, error.line),
        };
        let code = error.code.as_ref().map_or_else(String::new, |code| format!(" {code}"));
        out.line(1, format!("{place}{code} {}", error.message));
      }
      if !unsettled.is_empty() {
        out.line(0, "PARTIAL: nothing is known about new errors in:");
        for file in unsettled {
          out.line(1, file);
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::edit::CrossRepo;

  fn rendered(outcome: &EditOutcome) -> String {
    let mut out = TextOut::default();
    outcome.render_text(&mut out);
    out.finish()
  }

  fn outcome(status: EditStatus) -> EditOutcome {
    let mut outcome = EditOutcome::new(
      OperationView::Rename {
        target: "toCents".to_owned(),
        new_name: "cents".to_owned(),
        cross_repo: CrossRepo::Report,
      },
      status,
    );
    outcome.files = vec![ChangedFile {
      repo: "web".to_owned(),
      path: "src/money.ts".to_owned(),
      edits: 1,
      before: "a".to_owned(),
      after: "b".to_owned(),
      hunks: Vec::new(),
    }];
    outcome
  }

  #[test]
  fn should_say_how_many_edited_files_were_checked_and_what_allow_unverified_means() {
    let mut applied = outcome(EditStatus::Applied);
    applied.diagnostics = Some(DiagnosticsDelta::Checked {
      files: 256,
      checked: 20,
      new_errors: Vec::new(),
      unsettled: Vec::new(),
      native_only: false,
    });
    let mut dry = outcome(EditStatus::DryRun {
      plan_id: "p-1".to_owned(),
      valid_secs: 900,
      reason: DryRunReason::Requested,
    });
    dry.allow_unverified = true;
    dry.unverified = vec![crate::edit::VerifySite {
      reason: crate::edit::VerifyReason::OutsideLanguageServer,
      repo: "web".to_owned(),
      path: "src/a.ts".to_owned(),
      line: Some(3),
      column: Some(5),
      from: None,
      edge_kind: None,
      certainty: Some("[resolved]".to_owned()),
      authoritative: true,
      snippet: None,
      note: None,
    }];

    assert!(
      rendered(&applied).contains("diagnostics after edit: no new errors in the 20 checked file(s) (236 of 256 edited files not checked) (settled)\n"),
      "{}",
      rendered(&applied)
    );
    assert!(
      rendered(&dry).contains(
        "UNVERIFIED: 1 site(s) the index resolves to this symbol in the same repo were not renamed by the language server; allow_unverified is set, so applying this plan leaves them as they are:\n"
      ),
      "{}",
      rendered(&dry)
    );
  }

  #[test]
  fn should_lead_with_a_degraded_line_naming_the_files_a_failed_rollback_left_changed() {
    let partial = outcome(EditStatus::PartiallyApplied {
      cause: "replace web:src/use.ts: permission denied".to_owned(),
      still_changed: vec!["web:src/money.ts".to_owned()],
      rollback_errors: vec!["web:src/money.ts: disk full".to_owned()],
    });
    let rolled_back = outcome(EditStatus::RolledBack {
      cause: "changed since planning: web:src/use.ts".to_owned(),
    });

    assert_eq!(
      rendered(&partial),
      concat!(
        "DEGRADED: rename toCents -> cents was partially applied and the rollback failed (replace web:src/use.ts: permission denied); ",
        "still changed: web:src/money.ts; rollback errors: web:src/money.ts: disk full\n",
        "planned: 1 file(s), 1 edit(s)\n",
        "  web:src/money.ts  1\n",
        "not renamed, verify:\n",
        "  (none)\n",
      )
    );
    assert_eq!(
      rendered(&rolled_back),
      concat!(
        "FAILED: rename toCents -> cents was rolled back (changed since planning: web:src/use.ts); no file was changed\n",
        "planned, not written: 1 file(s), 1 edit(s)\n",
        "  web:src/money.ts  1\n",
        "not renamed, verify:\n",
        "  (none)\n",
      )
    );
    assert!(!rolled_back.status.wrote_anything() && partial.status.wrote_anything());
  }
}
