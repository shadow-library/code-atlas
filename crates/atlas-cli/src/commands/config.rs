use atlas_project::{ConfigEntry, ProjectError, Registry};
use serde_json::json;

use crate::cli::{ConfigCommand, ConfigShowArgs};
use crate::context::Context;
use crate::error::CliResult;
use crate::output::{Outcome, emit_json, emit_line, note};

pub fn run(ctx: &Context, command: ConfigCommand) -> CliResult<Outcome> {
  match command {
    ConfigCommand::Show(args) => show(ctx, args),
  }
}

fn show(ctx: &Context, args: ConfigShowArgs) -> CliResult<Outcome> {
  let registry = ctx.registry()?;
  let project = selected_project(ctx, &registry)?;
  let report = registry.explain(project.as_deref())?;
  for warning in &report.warnings {
    note(format!("warning: {warning}"));
  }
  let entries: Vec<&ConfigEntry> = report.entries.iter().filter(|entry| args.effective || !entry.is_built_in()).collect();
  if args.json {
    emit_json(&json!({ "project": project, "entries": entries }))?;
    return Ok(Outcome::Complete);
  }
  if entries.is_empty() {
    note("every key has its built-in value; pass --effective to list them");
  }
  for line in lines(&entries, args.origin) {
    emit_line(line)?;
  }
  Ok(Outcome::Complete)
}

fn selected_project(ctx: &Context, registry: &Registry) -> CliResult<Option<String>> {
  if let Some(name) = &ctx.project {
    return Ok(Some(registry.project(name)?.name.to_string()));
  }
  match registry.resolve_project(&ctx.cwd, None) {
    Ok(project) => Ok(Some(project.name.to_string())),
    Err(ProjectError::NotInProject { .. }) => Ok(None),
    Err(error) => Err(error.into()),
  }
}

fn lines(entries: &[&ConfigEntry], with_origin: bool) -> Vec<String> {
  let labels: Vec<String> = entries
    .iter()
    .map(|entry| entry.origins.iter().map(ToString::to_string).collect::<Vec<_>>().join(" + "))
    .collect();
  let width = labels.iter().map(|label| label.chars().count()).max().unwrap_or(0);
  entries
    .iter()
    .zip(labels)
    .map(|(entry, label)| {
      let value = entry.value.as_ref().map_or_else(|| "(unset)".to_owned(), ToString::to_string);
      match with_origin {
        true => format!("{label:<width$}  {} = {value}", entry.key),
        false => format!("{} = {value}", entry.key),
      }
    })
    .collect()
}
