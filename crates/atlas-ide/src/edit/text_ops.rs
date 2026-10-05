use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

use atlas_core::RunId;
use atlas_edit::{Disposition, EditError, EditScope, FileTarget, OccurrenceId, Pattern, ReplaceContent, ReplaceInFiles, SkipReason, TextPlan};
use atlas_query::{Freshness, Response};

use super::plans::{PlanDraft, PlanGuard};
use super::{DryRunReason, EditOutcome, OperationView, REPLACE_IN_FILES_NOTE, changed_files, planning_error, respond};
use crate::error::{IdeError, IdeResult};
use crate::service::{IdeService, guard};
use crate::target::{ProjectFiles, read_text};

pub(crate) struct FilesRequest<'a> {
  pub pattern: &'a str,
  pub text: &'a str,
  pub regex: bool,
  pub include: &'a [String],
  pub exclude: &'a [String],
  pub occurrences: Option<&'a [String]>,
  pub expected_count: Option<usize>,
}

fn pattern(text: &str, regex: bool) -> Pattern {
  match regex {
    true => Pattern::Regex(text.to_owned()),
    false => Pattern::Literal(text.to_owned()),
  }
}

impl IdeService {
  pub(crate) fn replace_content(&self, path: &str, find: &str, text: &str, regex: bool, all: bool, dry_run: bool) -> IdeResult<Response<EditOutcome>> {
    let files = self.files();
    let freshness = self.query.fresh()?;
    let file = files.locate_spec(path)?;
    let operation = OperationView::ReplaceContent { path: file.to_string() };
    let project = self.query.engine().project();
    let scope = EditScope::new(&project)?;
    let request = ReplaceContent {
      pattern: pattern(find, regex),
      text: text.to_owned(),
      all,
    };
    let planned = match atlas_edit::replace_content(&scope, &FileTarget::new(file.repo.clone(), file.path.clone()), &request) {
      Ok(planned) => planned,
      Err(error) => return respond(freshness, planning_error(&operation, error)?),
    };
    self.conclude_text(freshness, &files, operation, planned, dry_run.then_some(DryRunReason::Requested))
  }

  pub(crate) fn replace_in_files(&self, request: &FilesRequest<'_>, dry_run: bool) -> IdeResult<Response<EditOutcome>> {
    let operation = OperationView::ReplaceInFiles {
      pattern: request.pattern.to_owned(),
    };
    let files = self.files();
    let freshness = self.query.fresh()?;
    let occurrences = match request.occurrences {
      Some(ids) => Some(ids.iter().map(|id| OccurrenceId::from_str(id)).collect::<Result<Vec<_>, EditError>>()?),
      None => None,
    };
    let project = self.query.engine().project();
    let scope = EditScope::new(&project)?;
    let planned = match atlas_edit::replace_in_files(
      &scope,
      &ReplaceInFiles {
        pattern: pattern(request.pattern, request.regex),
        text: request.text.to_owned(),
        include: request.include.to_vec(),
        exclude: request.exclude.to_vec(),
        occurrences,
        expected_count: request.expected_count,
        dry_run,
      },
    ) {
      Ok(planned) => planned,
      Err(error) => return respond(freshness, planning_error(&operation, error)?),
    };
    let dry_run = match planned.disposition {
      Disposition::DryRun => Some(DryRunReason::Requested),
      Disposition::ImplicitDryRun => Some(DryRunReason::NeedsSelection),
      Disposition::Apply => None,
    };
    let skipped: Vec<String> = planned.skipped.iter().map(|skipped| format!("{} ({})", skipped.file, skip_text(&skipped.reason))).collect();
    let total = planned.total_matches;
    let mut response = self.conclude_text(freshness, &files, operation, planned.text, dry_run)?;
    response.answer.skipped = skipped;
    if dry_run == Some(DryRunReason::NeedsSelection) {
      response.answer.caveats.insert(
        0,
        format!("{total} matches; this call was a dry run because it named neither occurrences nor expected_count ({REPLACE_IN_FILES_NOTE})"),
      );
    }
    Ok(response)
  }

  fn conclude_text(
    &self,
    freshness: Freshness,
    files: &ProjectFiles,
    operation: OperationView,
    planned: TextPlan,
    dry_run: Option<DryRunReason>,
  ) -> IdeResult<Response<EditOutcome>> {
    let mut originals = BTreeMap::new();
    let mut stale = Vec::new();
    for file in &planned.plan.files {
      let root = files.root(&file.file.repo).ok_or_else(|| IdeError::UnknownRepo {
        repo: file.file.repo.to_string(),
        repos: files.names(),
      })?;
      match read_text(&root.join(file.file.path.as_str())) {
        Ok((text, hash)) if hash == file.expected => {
          originals.insert(file.file.clone(), text);
        }
        _ => stale.push(file.file.to_string()),
      }
    }
    if !stale.is_empty() {
      return respond(
        freshness,
        EditOutcome::refused(operation, format!("{} changed while the edit was planned; nothing was written", stale.join(", "))),
      );
    }
    let plan = PlanDraft {
      project: self.query.engine().project().name.to_string(),
      operation,
      guard: PlanGuard::Files,
      files: changed_files(&planned.plan, &originals, &planned.diffs)?,
      resources: Vec::new(),
      edits: planned.plan,
      not_renamed: Vec::new(),
      applied_from_graph: Vec::new(),
      caveats: Vec::new(),
      unverified: Vec::new(),
      unloaded_projects: Vec::new(),
      allow_unverified: false,
    }
    .seal();
    let units = match dry_run {
      Some(_) => Vec::new(),
      None => self.warm_units(files, &plan.edits),
    };
    let locks: Vec<_> = units.iter().map(|unit| Arc::clone(&unit.lock)).collect();
    let _held: Vec<_> = locks.iter().map(|lock| guard(lock)).collect();
    let mut sessions = Vec::new();
    let mut warnings = Vec::new();
    for unit in &units {
      match self.connect_live(unit, freshness.run.map(RunId), files, &[]) {
        Ok(session) => sessions.push(session),
        Err(error) => warnings.push(format!("{} is not used for this edit: {error}", unit.kind.name())),
      }
    }
    let mut response = self.conclude(freshness, plan, sessions, dry_run)?;
    response.answer.warnings.extend(warnings);
    Ok(response)
  }
}

fn skip_text(reason: &SkipReason) -> String {
  match reason {
    SkipReason::Binary => "binary".to_owned(),
    SkipReason::NotUtf8 => "not UTF-8".to_owned(),
    SkipReason::TooLarge { size, limit } => format!("{size} bytes, over the {limit}-byte limit"),
    SkipReason::Unreadable(reason) => format!("unreadable: {reason}"),
  }
}
