mod extras;
mod gate;
mod notify;
mod plans;
mod projects;
mod rename;
mod symbol_ops;
mod text_ops;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use atlas_core::{ContentHash, RelPath, RepoName, RunId};
use atlas_edit::{ApplyOptions, ApplyReport, EditError, EditPlan, EditScope, FileDiff, FileEdits, FileTarget, ResourceOp};
use atlas_lsp::{LspError, ServerKind};
use atlas_query::{EntityRef, Freshness, Response};
use serde::{Deserialize, Serialize};

pub use extras::{VerifyReason, VerifySite};
pub use notify::{DiagnosticsDelta, NewDiagnostic};
pub use plans::{IdePlan, PLAN_VERSION, PlanGuard};

pub(crate) use extras::Reach;
pub(crate) use gate::WriteGate;
pub(crate) use plans::{PLAN_TTL, PlanCache};

use crate::error::{IdeError, IdeResult};
use crate::render::ServerView;
use crate::servers::kind_of;
use crate::service::{IdeService, Session, guard};
use crate::sync::{LedgerChanges, change_list};
use crate::target::ProjectFiles;

pub const REPLACE_BODY_NOTE: &str = "replace_body: restate all decorators/attributes or none; restating one replaces the group, and text without them keeps the existing ones";
pub const REGEX_NOTE: &str = "regex: `.` matches newlines; use `.*?` or `[^\\n]*` for single-line matches; write ${1} to end a group number before other text";
pub const REPLACE_IN_FILES_NOTE: &str = "replace_in_files: without occurrences or expected_count the call is a dry run; pass the ids or the count from it to apply";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossRepo {
  #[default]
  Report,
  Apply,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOperation {
  Rename {
    target: String,
    new_name: String,
    cross_repo: CrossRepo,
    allow_unverified: bool,
  },
  ReplaceBody {
    target: String,
    text: String,
  },
  InsertBefore {
    target: String,
    text: String,
  },
  InsertAfter {
    target: String,
    text: String,
  },
  SafeDelete {
    target: String,
  },
  ReplaceContent {
    path: String,
    pattern: String,
    text: String,
    regex: bool,
    all: bool,
  },
  ReplaceInFiles {
    pattern: String,
    text: String,
    regex: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    occurrences: Option<Vec<String>>,
    expected_count: Option<usize>,
  },
  Apply {
    plan_id: String,
  },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRequest {
  pub operation: EditOperation,
  pub dry_run: bool,
}

impl EditRequest {
  pub fn new(operation: EditOperation) -> Self {
    Self { operation, dry_run: false }
  }

  pub fn dry_run(operation: EditOperation) -> Self {
    Self { operation, dry_run: true }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum OperationView {
  Rename { target: String, new_name: String, cross_repo: CrossRepo },
  ReplaceBody { target: String },
  InsertBefore { target: String },
  InsertAfter { target: String },
  SafeDelete { target: String },
  ReplaceContent { path: String },
  ReplaceInFiles { pattern: String },
}

impl fmt::Display for OperationView {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Rename { target, new_name, .. } => write!(formatter, "rename {target} -> {new_name}"),
      Self::ReplaceBody { target } => write!(formatter, "replace_body {target}"),
      Self::InsertBefore { target } => write!(formatter, "insert_before {target}"),
      Self::InsertAfter { target } => write!(formatter, "insert_after {target}"),
      Self::SafeDelete { target } => write!(formatter, "safe_delete {target}"),
      Self::ReplaceContent { path } => write!(formatter, "replace_content {path}"),
      Self::ReplaceInFiles { pattern } => write!(formatter, "replace_in_files {pattern:?}"),
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DryRunReason {
  Requested,
  NeedsSelection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EditStatus {
  Applied,
  DryRun {
    plan_id: String,
    valid_secs: u64,
    reason: DryRunReason,
  },
  Refused {
    reason: String,
  },
  RolledBack {
    cause: String,
  },
  PartiallyApplied {
    cause: String,
    still_changed: Vec<String>,
    rollback_errors: Vec<String>,
  },
}

impl EditStatus {
  pub fn wrote_anything(&self) -> bool {
    matches!(self, Self::Applied | Self::PartiallyApplied { .. })
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
  pub repo: String,
  pub path: String,
  pub edits: usize,
  pub before: String,
  pub after: String,
  pub hunks: Vec<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub outside_worktree: Option<String>,
}

impl ChangedFile {
  pub fn label(&self) -> String {
    format!("{}:{}", self.repo, self.path)
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditOutcome {
  pub operation: OperationView,
  pub status: EditStatus,
  pub servers: Vec<ServerView>,
  pub files: Vec<ChangedFile>,
  pub resources: Vec<String>,
  pub not_renamed: Vec<VerifySite>,
  pub unverified: Vec<VerifySite>,
  pub unloaded_projects: Vec<String>,
  pub allow_unverified: bool,
  pub applied_from_graph: Vec<VerifySite>,
  pub blockers: Vec<VerifySite>,
  pub candidates: Vec<EntityRef>,
  pub skipped: Vec<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub diagnostics: Option<DiagnosticsDelta>,
  pub caveats: Vec<String>,
  pub warnings: Vec<String>,
}

impl EditOutcome {
  pub(crate) fn new(operation: OperationView, status: EditStatus) -> Self {
    Self {
      operation,
      status,
      servers: Vec::new(),
      files: Vec::new(),
      resources: Vec::new(),
      not_renamed: Vec::new(),
      unverified: Vec::new(),
      unloaded_projects: Vec::new(),
      allow_unverified: false,
      applied_from_graph: Vec::new(),
      blockers: Vec::new(),
      candidates: Vec::new(),
      skipped: Vec::new(),
      diagnostics: None,
      caveats: Vec::new(),
      warnings: Vec::new(),
    }
  }

  pub(crate) fn refused(operation: OperationView, reason: impl Into<String>) -> Self {
    Self::new(operation, EditStatus::Refused { reason: reason.into() })
  }

  fn of_plan(plan: &IdePlan, status: EditStatus) -> Self {
    Self {
      files: plan.files.clone(),
      resources: plan.resources.clone(),
      not_renamed: plan.not_renamed.clone(),
      unverified: plan.unverified.clone(),
      unloaded_projects: plan.unloaded_projects.clone(),
      allow_unverified: plan.allow_unverified,
      applied_from_graph: plan.applied_from_graph.clone(),
      caveats: plan.caveats.clone(),
      ..Self::new(plan.operation.clone(), status)
    }
  }
}

pub(crate) fn respond(freshness: Freshness, answer: EditOutcome) -> IdeResult<Response<EditOutcome>> {
  Ok(Response { freshness, answer })
}

pub(crate) fn refusal_of(error: &EditError) -> Option<String> {
  match error {
    EditError::OutOfScope { .. }
    | EditError::Stale { .. }
    | EditError::ReadOnly { .. }
    | EditError::NotText { .. }
    | EditError::RegionUnparseable(_)
    | EditError::NotLineAligned(_)
    | EditError::InvalidSpan { .. }
    | EditError::InvalidPlan(_)
    | EditError::AlreadyExists { .. }
    | EditError::Missing { .. }
    | EditError::NoMatch { .. }
    | EditError::MultipleMatches { .. }
    | EditError::ExpectedCountMismatch { .. }
    | EditError::UnknownOccurrences { .. }
    | EditError::EmptyMatch { .. } => Some(format!("{error}; nothing was written")),
    _ => None,
  }
}

pub(crate) fn planning_error(operation: &OperationView, error: EditError) -> IdeResult<EditOutcome> {
  match refusal_of(&error) {
    Some(reason) => Ok(EditOutcome::refused(operation.clone(), reason)),
    None => Err(IdeError::Edit(error)),
  }
}

pub(crate) fn sync_refusal(error: IdeError) -> Result<String, IdeError> {
  match error {
    IdeError::Lsp(LspError::SyncIncomplete { server, reason }) => Ok(format!(
      "{server} could not be brought in sync with the files on disk ({reason}); an edit never proceeds on a partial view; retry once the server settles"
    )),
    other => Err(other),
  }
}

pub(crate) fn changed_files(plan: &EditPlan, originals: &BTreeMap<FileTarget, String>, diffs: &[FileDiff]) -> IdeResult<Vec<ChangedFile>> {
  let mut files = Vec::with_capacity(plan.files.len());
  for edits in &plan.files {
    let original = originals
      .get(&edits.file)
      .ok_or_else(|| IdeError::InvalidRequest(format!("no original text for {}", edits.file)))?;
    let after = edits.apply_to(original).map_err(EditError::from)?;
    let hunks = match diffs.iter().find(|diff| diff.file == edits.file) {
      Some(diff) => diff.hunks.iter().map(ToString::to_string).collect(),
      None => atlas_edit::file_diff(&edits.file, original, &edits.edits, &[])
        .map_err(EditError::from)?
        .hunks
        .iter()
        .map(ToString::to_string)
        .collect(),
    };
    files.push(ChangedFile {
      repo: edits.file.repo.to_string(),
      path: edits.file.path.to_string(),
      edits: edits.edits.len(),
      before: edits.expected.to_string(),
      after: ContentHash::of(after.as_bytes()).to_string(),
      hunks,
      outside_worktree: None,
    });
  }
  Ok(files)
}

pub(crate) fn resource_labels(resources: &[ResourceOp]) -> Vec<String> {
  resources.iter().map(ToString::to_string).collect()
}

pub(crate) fn file_targets(plan: &EditPlan) -> Vec<FileTarget> {
  let mut targets: Vec<FileTarget> = plan.files.iter().map(|file| file.file.clone()).collect();
  for op in &plan.resources {
    match op {
      ResourceOp::Create { file, .. } | ResourceOp::Delete { file, .. } => targets.push(file.clone()),
      ResourceOp::Rename { from, to, .. } => targets.extend([from.clone(), to.clone()]),
    }
  }
  targets.sort();
  targets.dedup();
  targets
}

pub(crate) fn merge_edits(plan: &mut EditPlan, file: FileEdits) {
  match plan.files.iter_mut().find(|existing| existing.file == file.file) {
    Some(existing) => {
      let mut edits = std::mem::take(&mut existing.edits);
      edits.extend(file.edits);
      *existing = FileEdits::new(existing.file.clone(), existing.expected, edits);
    }
    None => plan.files.push(file),
  }
}

impl IdeService {
  pub fn edit(&self, request: &EditRequest) -> IdeResult<Response<EditOutcome>> {
    let _one_mutation = guard(&self.edits);
    self.dispatch(request).map(|response| self.mark_outside_worktree(response))
  }

  pub fn apply_plan(&self, plan: IdePlan) -> IdeResult<Response<EditOutcome>> {
    let _one_mutation = guard(&self.edits);
    self.apply_planned(plan).map(|response| self.mark_outside_worktree(response))
  }

  fn mark_outside_worktree(&self, mut response: Response<EditOutcome>) -> Response<EditOutcome> {
    let Some(overlay) = &self.overlay else { return response };
    let project = self.query.engine().project();
    for file in &mut response.answer.files {
      if overlay.repos.contains(&file.repo) {
        continue;
      }
      file.outside_worktree = project.repo(&file.repo).map(|repo| repo.path.display().to_string());
    }
    response
  }

  fn dispatch(&self, request: &EditRequest) -> IdeResult<Response<EditOutcome>> {
    let dry_run = request.dry_run;
    match &request.operation {
      EditOperation::Rename {
        target,
        new_name,
        cross_repo,
        allow_unverified,
      } => self.rename(&rename::RenameCall {
        spec: target,
        new_name,
        cross_repo: *cross_repo,
        allow_unverified: *allow_unverified,
        dry_run,
      }),
      EditOperation::ReplaceBody { target, text } => self.symbol_edit(target, symbol_ops::SymbolEdit::ReplaceBody(text.clone()), dry_run),
      EditOperation::InsertBefore { target, text } => self.symbol_edit(target, symbol_ops::SymbolEdit::InsertBefore(text.clone()), dry_run),
      EditOperation::InsertAfter { target, text } => self.symbol_edit(target, symbol_ops::SymbolEdit::InsertAfter(text.clone()), dry_run),
      EditOperation::SafeDelete { target } => self.safe_delete(target, dry_run),
      EditOperation::ReplaceContent { path, pattern, text, regex, all } => self.replace_content(path, pattern, text, *regex, *all, dry_run),
      EditOperation::ReplaceInFiles {
        pattern,
        text,
        regex,
        include,
        exclude,
        occurrences,
        expected_count,
      } => self.replace_in_files(
        &text_ops::FilesRequest {
          pattern,
          text,
          regex: *regex,
          include,
          exclude,
          occurrences: occurrences.as_deref(),
          expected_count: *expected_count,
        },
        dry_run,
      ),
      EditOperation::Apply { plan_id } => match dry_run {
        true => Err(IdeError::InvalidRequest(
          "operation apply has no dry run; the plan was shown by the dry run that made it".to_owned(),
        )),
        false => {
          let plan = self.take_plan(plan_id)?;
          self.apply_planned(plan)
        }
      },
    }
  }

  fn apply_planned(&self, plan: IdePlan) -> IdeResult<Response<EditOutcome>> {
    let project = self.query.engine().project().name.to_string();
    let files = self.files();
    let freshness = self.query.fresh()?;
    let refusal = if plan.version != PLAN_VERSION {
      let found = if plan.version == 0 {
        "no version".to_owned()
      } else {
        format!("version {}", plan.version)
      };
      Some(format!(
        "plan {} has {found}; this code-atlas reads plan version {PLAN_VERSION}; run the dry run again; nothing was written",
        plan.id
      ))
    } else if plan.project != project {
      Some(format!("plan {} was made for project {}, not {project}; nothing was written", plan.id, plan.project))
    } else {
      self.guard_violation(&plan, &freshness, &files)
    };
    if let Some(reason) = refusal {
      return respond(freshness, EditOutcome::of_plan(&plan, EditStatus::Refused { reason }));
    }
    let units = self.warm_units(&files, &plan.edits);
    let locks: Vec<_> = units.iter().map(|unit| std::sync::Arc::clone(&unit.lock)).collect();
    let _held: Vec<_> = locks.iter().map(|lock| guard(lock)).collect();
    let mut sessions = Vec::new();
    let mut warnings = Vec::new();
    for unit in &units {
      match self.connect_live(unit, freshness.run.map(RunId), &files, &[]) {
        Ok(session) => sessions.push(session),
        Err(error) => warnings.push(format!("{} is not used for this edit: {error}", unit.kind.name())),
      }
    }
    let mut response = self.execute(freshness, plan, &mut sessions)?;
    response.answer.warnings.extend(warnings);
    Ok(response)
  }

  fn guard_violation(&self, plan: &IdePlan, freshness: &Freshness, files: &ProjectFiles) -> Option<String> {
    let PlanGuard::Project { run } = plan.guard else { return None };
    if freshness.run == run {
      return None;
    }
    let changes = change_list(self.query.engine().store(), files, run.map(RunId), freshness.run.map(RunId), &mut LedgerChanges::default());
    let changed = match changes {
      atlas_lsp::ChangeList::Known(changes) if changes.is_empty() => return None,
      atlas_lsp::ChangeList::Known(changes) => changes.len().to_string(),
      atlas_lsp::ChangeList::Unknown => "an unknown number of".to_owned(),
    };
    Some(format!(
      "{changed} files changed since plan {} was made (run {} -> {}); a rename or safe delete depends on every file, so run the dry run again; nothing was written",
      plan.id,
      run.map_or_else(|| "none".to_owned(), |run| run.to_string()),
      freshness.run.map_or_else(|| "none".to_owned(), |run| run.to_string())
    ))
  }

  pub(crate) fn warm_units(&self, files: &ProjectFiles, plan: &EditPlan) -> Vec<crate::service::Unit> {
    let mut units: Vec<crate::service::Unit> = Vec::new();
    for target in file_targets(plan) {
      let Ok(file) = files.file(&target.repo, &target.path).or_else(|_| planned_file(files, &target)) else {
        continue;
      };
      let Some(kind) = kind_of(&file.absolute) else { continue };
      if units.iter().any(|unit| unit.kind == kind) {
        continue;
      }
      if let Some(unit) = self.warm_unit(&file) {
        units.push(unit);
      }
    }
    units.sort_by_key(|unit| unit.kind == ServerKind::TypeScript);
    units
  }

  pub(crate) fn conclude(&self, freshness: Freshness, plan: IdePlan, mut sessions: Vec<Session<'_>>, dry_run: Option<DryRunReason>) -> IdeResult<Response<EditOutcome>> {
    if let Some(reason) = dry_run {
      let ttl = self.plan_ttl();
      let mut outcome = EditOutcome::of_plan(
        &plan,
        EditStatus::DryRun {
          plan_id: plan.id.clone(),
          valid_secs: ttl.as_secs(),
          reason,
        },
      );
      outcome.servers = sessions.into_iter().map(Session::finish).collect();
      guard(&self.plans).insert(plan);
      return respond(freshness, outcome);
    }
    self.execute(freshness, plan, &mut sessions)
  }

  fn execute(&self, freshness: Freshness, plan: IdePlan, sessions: &mut Vec<Session<'_>>) -> IdeResult<Response<EditOutcome>> {
    if !plan.unloaded_projects.is_empty() && !plan.allow_unverified {
      let reason = format!(
        "the language server could not load {} (unreadable tsconfig, no file to open), so sites there were not checked; refused unless allow_unverified is set; nothing was written",
        plan.unloaded_projects.join(", ")
      );
      let mut outcome = EditOutcome::of_plan(&plan, EditStatus::Refused { reason });
      outcome.servers = std::mem::take(sessions).into_iter().map(Session::finish).collect();
      return respond(freshness, outcome);
    }
    if !plan.unverified.is_empty() && !plan.allow_unverified {
      let reason = format!(
        "{} site(s) the index resolves to this symbol in the same repo were not renamed by the language server (listed under unverified); \
         a rename that leaves them would break the build, so it is refused unless allow_unverified is set; nothing was written",
        plan.unverified.len()
      );
      let mut outcome = EditOutcome::of_plan(&plan, EditStatus::Refused { reason });
      outcome.servers = std::mem::take(sessions).into_iter().map(Session::finish).collect();
      return respond(freshness, outcome);
    }
    let engine = self.query.engine();
    let project = engine.project();
    let scope = match EditScope::new(&project) {
      Ok(scope) => scope,
      Err(error) => return respond(freshness, EditOutcome::of_plan(&plan, EditStatus::Refused { reason: error.to_string() })),
    };
    let mut warnings = Vec::new();
    let mut baselines = Vec::with_capacity(sessions.len());
    for session in sessions.iter_mut() {
      let paths = notify::touched_paths(session, &plan.edits, None);
      baselines.push(notify::baseline(session, &paths, freshness.run.map(RunId), &mut warnings));
    }
    if let PlanGuard::Project { .. } = plan.guard {
      let latest = self.query.fresh()?;
      if let Some(reason) = self.guard_violation(&plan, &latest, &self.files()) {
        let mut outcome = EditOutcome::of_plan(&plan, EditStatus::Refused { reason });
        outcome.servers = std::mem::take(sessions).into_iter().map(Session::finish).collect();
        return respond(latest, outcome);
      }
    }
    let targets: Vec<(RepoName, RelPath)> = file_targets(&plan.edits).into_iter().map(|file| (file.repo, file.path)).collect();
    let lock_db = self.overlay.as_ref().map_or_else(|| engine.db_path(), |overlay| overlay.base_db.clone());
    let options = ApplyOptions::new(edit_lock_path(&lock_db));
    tracing::info!(plan = %plan.id, files = targets.len(), "applying an edit plan");
    let Some(_writing) = self.writes.enter() else { return Err(IdeError::Interrupted) };
    let (applied, report) = engine.write_then_refresh(&targets, || atlas_edit::apply(&plan.edits, &scope, &options))?;
    let mut after = Freshness::from_report(&report);
    after.notes = freshness.notes;
    let report = match applied {
      Ok(report) => report,
      Err(error) => return self.failed(after, &plan, error, sessions, warnings),
    };
    let mut outcome = EditOutcome::of_plan(&plan, EditStatus::Applied);
    outcome.warnings = warnings;
    outcome.warnings.extend(swept_note(&report));
    let generation = after.run.map(RunId);
    let mut deltas = Vec::new();
    for (session, baseline) in sessions.iter_mut().zip(baselines) {
      deltas.push(notify::after_edit(session, &plan.edits, &report, baseline, generation, &mut outcome.warnings));
    }
    outcome.diagnostics = Some(notify::combine(deltas));
    outcome.servers = std::mem::take(sessions).into_iter().map(Session::finish).collect();
    respond(after, outcome)
  }

  fn failed(&self, freshness: Freshness, plan: &IdePlan, error: EditError, sessions: &mut Vec<Session<'_>>, warnings: Vec<String>) -> IdeResult<Response<EditOutcome>> {
    let status = match error {
      EditError::RolledBack { cause } => EditStatus::RolledBack { cause: cause.to_string() },
      EditError::PartiallyApplied { cause, applied, failed } => EditStatus::PartiallyApplied {
        cause: cause.to_string(),
        still_changed: applied,
        rollback_errors: failed,
      },
      other => match refusal_of(&other) {
        Some(reason) => EditStatus::Refused { reason },
        None => EditStatus::Refused {
          reason: format!("{other}; nothing was written"),
        },
      },
    };
    let mut outcome = EditOutcome::of_plan(plan, status);
    outcome.warnings = warnings;
    if let EditStatus::PartiallyApplied { .. } = outcome.status {
      let generation = freshness.run.map(RunId);
      for session in sessions.iter_mut() {
        let open = notify::touched_paths(session, &plan.edits, None);
        if let Err(error) = session.resync(generation, atlas_lsp::SyncDemand::Best, &open) {
          outcome
            .warnings
            .push(format!("{} could not re-read the partially edited files: {error}", session.server_name()));
        }
      }
      outcome.diagnostics = Some(DiagnosticsDelta::Unavailable {
        reason: "the edit was only partially applied".to_owned(),
      });
    }
    outcome.servers = std::mem::take(sessions).into_iter().map(Session::finish).collect();
    respond(freshness, outcome)
  }
}

fn planned_file(files: &ProjectFiles, target: &FileTarget) -> IdeResult<crate::target::FileRef> {
  let root = files.root(&target.repo).ok_or_else(|| IdeError::UnknownRepo {
    repo: target.repo.to_string(),
    repos: files.names(),
  })?;
  Ok(crate::target::FileRef {
    repo: target.repo.clone(),
    path: target.path.clone(),
    absolute: root.join(target.path.as_str()),
  })
}

fn swept_note(report: &ApplyReport) -> Option<String> {
  match report.swept.is_empty() {
    true => None,
    false => Some(format!(
      "removed {} temporary files left by an interrupted edit: {}",
      report.swept.len(),
      report.swept.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
    )),
  }
}

pub(crate) fn edit_lock_path(db_path: &std::path::Path) -> PathBuf {
  let mut name = db_path.as_os_str().to_owned();
  name.push(".edit.lock");
  PathBuf::from(name)
}
