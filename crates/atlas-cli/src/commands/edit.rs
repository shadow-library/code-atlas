use std::fs;
use std::path::Path;

use atlas_ide::{CrossRepo, EditOperation, EditOutcome, EditRequest, EditStatus, EditView, IdePlan};
use atlas_query::{Response, render_json, render_text};

use crate::cli::{ApplyArgs, CrossRepoArg, EditCommand, EditOutput, RenameArgs, ReplaceArgs, ReplaceInFilesArgs, SafeDeleteArgs, SymbolEditArgs};
use crate::commands::ide::OneShot;
use crate::commands::read_input;
use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::output::{Outcome, emit};

pub fn run(ctx: &Context, command: EditCommand) -> CliResult<Outcome> {
  dispatch(ctx, command).map_err(CliError::for_edit)
}

fn dispatch(ctx: &Context, command: EditCommand) -> CliResult<Outcome> {
  if let Some(refusal) = ctx.bound()?.binding.edit_refusal() {
    return Err(CliError::EditOutsideIndexedCheckout(refusal));
  }
  match command {
    EditCommand::Apply(args) => apply(ctx, args),
    EditCommand::Rename(args) => rename(ctx, args),
    EditCommand::ReplaceBody(args) => symbol_edit(ctx, args, |target, text| EditOperation::ReplaceBody { target, text }),
    EditCommand::InsertBefore(args) => symbol_edit(ctx, args, |target, text| EditOperation::InsertBefore { target, text }),
    EditCommand::InsertAfter(args) => symbol_edit(ctx, args, |target, text| EditOperation::InsertAfter { target, text }),
    EditCommand::SafeDelete(args) => safe_delete(ctx, args),
    EditCommand::Replace(args) => replace(ctx, args),
    EditCommand::ReplaceInFiles(args) => replace_in_files(ctx, args),
  }
}

fn replacement(text: Option<String>, text_file: Option<String>) -> CliResult<String> {
  match (text, text_file) {
    (Some(text), _) => Ok(text),
    (None, Some(source)) => read_input(&source),
    (None, None) => Err(CliError::InvalidInput("give --text or --text-file".to_owned())),
  }
}

fn rename(ctx: &Context, args: RenameArgs) -> CliResult<Outcome> {
  let operation = EditOperation::Rename {
    target: args.target,
    new_name: args.new_name,
    cross_repo: match args.cross_repo {
      CrossRepoArg::Report => CrossRepo::Report,
      CrossRepoArg::Apply => CrossRepo::Apply,
    },
    allow_unverified: args.allow_unverified,
  };
  submit(ctx, operation, &args.output)
}

fn symbol_edit(ctx: &Context, args: SymbolEditArgs, operation: impl FnOnce(String, String) -> EditOperation) -> CliResult<Outcome> {
  let text = replacement(args.text, args.text_file)?;
  submit(ctx, operation(args.target, text), &args.output)
}

fn safe_delete(ctx: &Context, args: SafeDeleteArgs) -> CliResult<Outcome> {
  submit(ctx, EditOperation::SafeDelete { target: args.target }, &args.output)
}

fn replace(ctx: &Context, args: ReplaceArgs) -> CliResult<Outcome> {
  let operation = EditOperation::ReplaceContent {
    path: args.path,
    pattern: args.pattern,
    text: replacement(args.text, args.text_file)?,
    regex: args.regex,
    all: args.all,
  };
  submit(ctx, operation, &args.output)
}

fn replace_in_files(ctx: &Context, args: ReplaceInFilesArgs) -> CliResult<Outcome> {
  let operation = EditOperation::ReplaceInFiles {
    pattern: args.pattern,
    text: replacement(args.text, args.text_file)?,
    regex: args.regex,
    include: args.include,
    exclude: args.exclude,
    occurrences: Some(args.occurrences).filter(|occurrences| !occurrences.is_empty()),
    expected_count: args.expected_count,
  };
  submit(ctx, operation, &args.output)
}

fn submit(ctx: &Context, operation: EditOperation, output: &EditOutput) -> CliResult<Outcome> {
  let one = OneShot::open(ctx)?;
  let request = match output.dry_run {
    true => EditRequest::dry_run(operation),
    false => EditRequest::new(operation),
  };
  let response = one.ide.edit(&request)?;
  let note = match (&response.answer.status, &output.plan_out) {
    (EditStatus::DryRun { plan_id, .. }, Some(path)) => {
      let plan = one
        .ide
        .planned(plan_id)
        .ok_or_else(|| CliError::InvalidInput(format!("plan {plan_id} is no longer cached; run the dry run again")))?;
      write_plan(path, &plan)?;
      Some(format!("plan written to {}; valid until the files change", path.display()))
    }
    (EditStatus::DryRun { .. }, None) => Some("plan not saved; pass --plan-out to apply later".to_owned()),
    _ => None,
  };
  conclude(&response, output.json, note.as_deref())
}

fn apply(ctx: &Context, args: ApplyArgs) -> CliResult<Outcome> {
  let text = fs::read_to_string(&args.plan).map_err(|source| CliError::io("read", &args.plan, source))?;
  let plan: IdePlan = serde_json::from_str(&text).map_err(|error| CliError::InvalidPlan {
    path: args.plan.clone(),
    message: error.to_string(),
  })?;
  let one = OneShot::open(ctx)?;
  let response = one.ide.apply_plan(plan)?;
  conclude(&response, args.json, None)
}

fn write_plan(path: &Path, plan: &IdePlan) -> CliResult<()> {
  let mut text = serde_json::to_string_pretty(plan)?;
  text.push('\n');
  fs::write(path, text).map_err(|source| CliError::io("write", path, source))
}

fn conclude(response: &Response<EditOutcome>, json: bool, plan_note: Option<&str>) -> CliResult<Outcome> {
  let text = match (json, plan_note) {
    (true, _) => render_json(response)?,
    (false, Some(plan_note)) => render_text(&Response {
      freshness: response.freshness.clone(),
      answer: EditView {
        outcome: &response.answer,
        plan_note,
      },
    }),
    (false, None) => render_text(response),
  };
  emit(&text)?;
  Ok(outcome_of(&response.answer.status, response.freshness.is_degraded()))
}

fn outcome_of(status: &EditStatus, degraded: bool) -> Outcome {
  match status {
    EditStatus::Refused { .. } => Outcome::Refused,
    EditStatus::RolledBack { .. } => Outcome::RolledBack,
    EditStatus::PartiallyApplied { .. } => Outcome::PartiallyApplied,
    EditStatus::Applied | EditStatus::DryRun { .. } => Outcome::degraded_if(degraded),
  }
}

#[cfg(test)]
mod tests {
  use atlas_ide::DryRunReason;

  use super::*;
  use crate::error::Exit;

  fn exit(status: &EditStatus, degraded: bool) -> u8 {
    Exit::code(outcome_of(status, degraded).exit())
  }

  #[test]
  fn should_map_every_edit_status_to_its_documented_exit_code() {
    let dry = EditStatus::DryRun {
      plan_id: "p-1".to_owned(),
      valid_secs: 900,
      reason: DryRunReason::Requested,
    };
    let refused = EditStatus::Refused { reason: "stale".to_owned() };
    let rolled_back = EditStatus::RolledBack { cause: "disk full".to_owned() };
    let partial = EditStatus::PartiallyApplied {
      cause: "disk full".to_owned(),
      still_changed: vec!["web:src/a.ts".to_owned()],
      rollback_errors: vec!["web:src/b.ts: read-only".to_owned()],
    };

    assert_eq!(
      [
        exit(&EditStatus::Applied, false),
        exit(&EditStatus::Applied, true),
        exit(&dry, false),
        exit(&refused, false),
        exit(&rolled_back, false),
        exit(&partial, false),
        exit(&partial, true)
      ],
      [0, 3, 0, 5, 6, 7, 7]
    );
  }
}
