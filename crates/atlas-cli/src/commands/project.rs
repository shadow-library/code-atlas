use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use atlas_core::Language;
use atlas_project::{LspUnit, Project, ProjectSettings, RepoConfig, RepoFilters, RepoSpec};

use crate::cli::{AddRepoArgs, ConfigArgs, CreateArgs, DeleteArgs, JsonFlag, LspUnitArg, ProjectCommand, RemoveRepoArgs, RenameRepoArgs, SetRepoArgs, ShowArgs, Toggle};
use crate::confirm::confirm;
use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::output::{Outcome, emit_json, emit_line};

pub fn run(ctx: &Context, command: ProjectCommand) -> CliResult<Outcome> {
  match command {
    ProjectCommand::Create(args) => create(ctx, args),
    ProjectCommand::AddRepo(args) => add_repo(ctx, args),
    ProjectCommand::SetRepo(args) => set_repo(ctx, args),
    ProjectCommand::RemoveRepo(args) => remove_repo(ctx, args),
    ProjectCommand::RenameRepo(args) => rename_repo(ctx, args),
    ProjectCommand::List(args) => list(ctx, args),
    ProjectCommand::Show(args) => show(ctx, args),
    ProjectCommand::Delete(args) => delete(ctx, args),
    ProjectCommand::Config(args) => config(ctx, args),
  }
}

fn parse_repo_spec(raw: &str) -> CliResult<RepoSpec> {
  if raw.is_empty() {
    return Err(CliError::InvalidRepoSpec(raw.to_owned()));
  }
  if Path::new(raw).exists() {
    return Ok(RepoSpec::new(raw));
  }
  match raw.rsplit_once('=') {
    Some((path, name)) if !path.is_empty() && !name.is_empty() && !name.contains('/') => Ok(RepoSpec::new(path).named(name)),
    Some(_) => Err(CliError::InvalidRepoSpec(raw.to_owned())),
    None => Ok(RepoSpec::new(raw)),
  }
}

fn describe(project: &Project) -> String {
  let mut text = format!("{} ({} repos)\n", project.name, project.repos.len());
  for repo in &project.repos {
    text.push_str(&format!("  {}  {}\n", repo.name, repo.path.display()));
    for line in filter_lines(repo) {
      text.push_str(&format!("    {line}\n"));
    }
  }
  text
}

fn create(ctx: &Context, args: CreateArgs) -> CliResult<Outcome> {
  let specs = args.repos.iter().map(|raw| parse_repo_spec(raw)).collect::<CliResult<Vec<_>>>()?;
  let mut registry = ctx.registry()?;
  let project = registry.create_project(&args.name, specs)?;
  emit_line(format!("created project {}", describe(project).trim_end()))?;
  Ok(Outcome::Complete)
}

fn add_repo(ctx: &Context, args: AddRepoArgs) -> CliResult<Outcome> {
  let mut spec = RepoSpec::new(args.path);
  spec.name = args.name;
  spec.include = args.filters.include;
  spec.exclude = args.filters.exclude;
  spec.languages = args.filters.languages.as_deref().map(parse_languages).transpose()?;
  let mut registry = ctx.registry()?;
  let repo = registry.add_repo(&args.project_name, spec)?;
  emit_line(format!("added repo {} ({}) to project {}", repo.name, repo.path.display(), args.project_name))?;
  Ok(Outcome::Complete)
}

const LANGUAGE_HELP: &str = "rust, ts, typescript, tsx, javascript or jsx";

fn parse_languages(raw: &[String]) -> CliResult<Vec<Language>> {
  let mut languages = Vec::new();
  for name in raw {
    let selected: Vec<Language> = match name.trim().to_ascii_lowercase().as_str() {
      "rust" => vec![Language::Rust],
      "ts" => vec![Language::TypeScript, Language::Tsx, Language::JavaScript, Language::Jsx],
      other => vec![
        other
          .parse::<Language>()
          .map_err(|_| CliError::InvalidSetting(format!("unknown language {name:?}; expected {LANGUAGE_HELP}")))?,
      ],
    };
    languages.extend(selected);
  }
  languages.sort();
  languages.dedup();
  Ok(languages)
}

fn filter_lines(repo: &RepoConfig) -> Vec<String> {
  let mut lines = Vec::new();
  if !repo.include.is_empty() {
    lines.push(format!("include {}", repo.include.join(", ")));
  }
  if !repo.exclude.is_empty() {
    lines.push(format!("exclude {}", repo.exclude.join(", ")));
  }
  if let Some(languages) = &repo.languages {
    lines.push(format!("languages {}", languages.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")));
  }
  lines
}

fn filter_changes(before: &RepoConfig, after: &RepoConfig) -> Vec<String> {
  let languages = |repo: &RepoConfig| {
    repo
      .languages
      .as_ref()
      .map_or_else(|| "(all)".to_owned(), |languages| languages.iter().map(ToString::to_string).collect::<Vec<_>>().join(","))
  };
  let globs = |values: &[String]| match values.is_empty() {
    true => "(none)".to_owned(),
    false => values.join(", "),
  };
  [
    ("include", globs(&before.include), globs(&after.include)),
    ("exclude", globs(&before.exclude), globs(&after.exclude)),
    ("languages", languages(before), languages(after)),
  ]
  .into_iter()
  .filter(|(_, old, new)| old != new)
  .map(|(name, old, new)| format!("{name}: {old} -> {new}"))
  .collect()
}

fn set_repo(ctx: &Context, args: SetRepoArgs) -> CliResult<Outcome> {
  let languages = args.languages.as_deref().map(parse_languages).transpose()?;
  let mut registry = ctx.registry()?;
  let (before, after) = registry.set_repo_filters(&args.project_name, &args.repo, |current| {
    let mut updated = match args.clear_filters {
      true => RepoFilters::default(),
      false => current.clone(),
    };
    if !args.include.is_empty() {
      updated.include = args.include.clone();
    }
    if !args.exclude.is_empty() {
      updated.exclude = args.exclude.clone();
    }
    if languages.is_some() {
      updated.languages = languages.clone();
    }
    updated
  })?;
  let changes = filter_changes(&before, &after);
  if changes.is_empty() {
    emit_line("no changes")?;
    return Ok(Outcome::Complete);
  }
  for change in changes {
    emit_line(format!("{}: {change}", args.repo))?;
  }
  Ok(Outcome::Complete)
}

fn remove_repo(ctx: &Context, args: RemoveRepoArgs) -> CliResult<Outcome> {
  let mut registry = ctx.registry()?;
  let repo = registry.remove_repo(&args.project_name, &args.repo)?;
  emit_line(format!(
    "removed repo {} ({}) from project {}; its facts are dropped on the next query or index",
    repo.name,
    repo.path.display(),
    args.project_name
  ))?;
  Ok(Outcome::Complete)
}

fn rename_repo(ctx: &Context, args: RenameRepoArgs) -> CliResult<Outcome> {
  let mut registry = ctx.registry()?;
  registry.rename_repo(&args.project_name, &args.old, &args.new)?;
  emit_line(format!(
    "renamed repo {} to {} in project {}; the next query or index re-keys its facts",
    args.old, args.new, args.project_name
  ))?;
  Ok(Outcome::Complete)
}

fn list(ctx: &Context, args: JsonFlag) -> CliResult<Outcome> {
  let registry = ctx.registry()?;
  if args.json {
    emit_json(&registry.list())?;
    return Ok(Outcome::Complete);
  }
  if registry.list().is_empty() {
    emit_line("no projects registered")?;
    return Ok(Outcome::Complete);
  }
  for project in registry.list() {
    let repos: Vec<&str> = project.repos.iter().map(|repo| repo.name.as_str()).collect();
    emit_line(format!("{}  {}", project.name, repos.join(", ")))?;
  }
  Ok(Outcome::Complete)
}

fn show(ctx: &Context, args: ShowArgs) -> CliResult<Outcome> {
  let registry = ctx.registry()?;
  let project = registry.project(&args.name)?;
  if args.json {
    emit_json(project)?;
    return Ok(Outcome::Complete);
  }
  emit_line(format!("project {}", describe(project).trim_end()))?;
  emit_line(format!("data  {}", ctx.home.project_data_dir(&project.name).display()))?;
  Ok(Outcome::Complete)
}

fn delete(ctx: &Context, args: DeleteArgs) -> CliResult<Outcome> {
  let mut registry = ctx.registry()?;
  let data_dir = ctx.home.project_data_dir(&registry.project(&args.name)?.name);
  if args.purge_data {
    if !data_dir.starts_with(ctx.home.data_dir()) || data_dir == ctx.home.data_dir() {
      return Err(CliError::UnsafePurge(data_dir));
    }
    confirm(&format!("permanently delete project {} and its data in {}", args.name, data_dir.display()), args.yes)?;
  }
  registry.delete_project(&args.name)?;
  emit_line(format!("deleted project {}", args.name))?;
  if !args.purge_data {
    return Ok(Outcome::Complete);
  }
  match fs::remove_dir_all(&data_dir) {
    Ok(()) => emit_line(format!("removed {}", data_dir.display()))?,
    Err(error) if error.kind() == ErrorKind::NotFound => emit_line(format!("no data at {}", data_dir.display()))?,
    Err(source) => return Err(CliError::io("remove", data_dir, source)),
  }
  Ok(Outcome::Complete)
}

const LLM_PROVIDERS: [&str; 6] = ["claude-cli", "claude", "anthropic", "openai-compatible", "openai", "none"];

fn config(ctx: &Context, args: ConfigArgs) -> CliResult<Outcome> {
  let mut registry = ctx.registry()?;
  let current = registry.project(&args.project_name)?.settings.clone();
  let touched = args.reset
    || args.llm.is_some()
    || args.llm_provider.is_some()
    || args.llm_model.is_some()
    || args.scip.is_some()
    || args.scip_debounce.is_some()
    || args.max_file_bytes.is_some()
    || touches_lsp(&args);
  if !touched {
    return show_settings(&current, args.json);
  }
  let updated = apply_settings(&current, &args)?;
  let changes = changed_fields(&current, &updated);
  if changes.is_empty() {
    emit_line("no changes")?;
    return Ok(Outcome::Complete);
  }
  registry.set_settings(&args.project_name, updated)?;
  for change in changes {
    emit_line(change)?;
  }
  Ok(Outcome::Complete)
}

fn touches_lsp(args: &ConfigArgs) -> bool {
  args.lsp.is_some()
    || args.lsp_download.is_some()
    || !args.lsp_ts_command.is_empty()
    || args.lsp_ts_command_clear
    || args.lsp_idle_secs.is_some()
    || args.lsp_max_servers.is_some()
    || args.lsp_unit.is_some()
    || args.lsp_ready_timeout_secs.is_some()
    || args.lsp_check_timeout_secs.is_some()
    || !args.lsp_exclude.is_empty()
    || args.lsp_exclude_clear
}

fn positive(flag: &str, value: u64) -> CliResult<u64> {
  match value {
    0 => Err(CliError::InvalidSetting(format!("{flag} must be greater than 0"))),
    value => Ok(value),
  }
}

fn apply_lsp(settings: &mut ProjectSettings, args: &ConfigArgs) -> CliResult<()> {
  let lsp = &mut settings.lsp;
  if let Some(toggle) = args.lsp {
    lsp.enabled = toggle == Toggle::On;
  }
  if let Some(toggle) = args.lsp_download {
    lsp.download = toggle == Toggle::On;
  }
  if !args.lsp_ts_command.is_empty() {
    lsp.typescript_command = Some(args.lsp_ts_command.clone());
  }
  if args.lsp_ts_command_clear {
    lsp.typescript_command = None;
  }
  if let Some(seconds) = args.lsp_idle_secs {
    lsp.idle_secs = positive("--lsp-idle-secs", seconds)?;
  }
  if let Some(count) = args.lsp_max_servers {
    lsp.max_servers = count;
  }
  if let Some(unit) = args.lsp_unit {
    lsp.unit = match unit {
      LspUnitArg::Project => LspUnit::Project,
      LspUnitArg::Repo => LspUnit::Repo,
    };
  }
  if let Some(seconds) = args.lsp_ready_timeout_secs {
    lsp.ready_timeout_secs = positive("--lsp-ready-timeout-secs", seconds)?;
  }
  if let Some(seconds) = args.lsp_check_timeout_secs {
    lsp.check_timeout_secs = positive("--lsp-check-timeout-secs", seconds)?;
  }
  if !args.lsp_exclude.is_empty() {
    lsp.exclude = args.lsp_exclude.clone();
  }
  if args.lsp_exclude_clear {
    lsp.exclude.clear();
  }
  settings.validate().map_err(CliError::InvalidSetting)
}

fn apply_settings(current: &ProjectSettings, args: &ConfigArgs) -> CliResult<ProjectSettings> {
  let mut settings = if args.reset { ProjectSettings::default() } else { current.clone() };
  if let Some(toggle) = args.llm {
    settings.llm.enabled = toggle == Toggle::On;
  }
  if let Some(provider) = &args.llm_provider {
    if !LLM_PROVIDERS.contains(&provider.as_str()) {
      return Err(CliError::InvalidSetting(format!(
        "unknown LLM provider {provider:?}; expected one of {}",
        LLM_PROVIDERS.join(", ")
      )));
    }
    settings.llm.provider = Some(provider.clone());
  }
  if let Some(model) = &args.llm_model {
    settings.llm.model = Some(model.clone()).filter(|model| !model.trim().is_empty());
  }
  if let Some(toggle) = args.scip {
    settings.scip.enabled = toggle == Toggle::On;
  }
  if let Some(seconds) = args.scip_debounce {
    if seconds == 0 {
      return Err(CliError::InvalidSetting("--scip-debounce must be greater than 0".to_owned()));
    }
    settings.scip.debounce_secs = seconds;
  }
  if let Some(bytes) = args.max_file_bytes {
    if bytes == 0 {
      return Err(CliError::InvalidSetting("--max-file-bytes must be greater than 0".to_owned()));
    }
    settings.max_file_bytes = bytes;
  }
  apply_lsp(&mut settings, args)?;
  Ok(settings)
}

fn setting_lines(settings: &ProjectSettings) -> Vec<(&'static str, String)> {
  let optional = |value: &Option<String>| value.clone().unwrap_or_else(|| "(default)".to_owned());
  vec![
    ("llm.enabled", settings.llm.enabled.to_string()),
    ("llm.provider", optional(&settings.llm.provider)),
    ("llm.model", optional(&settings.llm.model)),
    ("scip.enabled", settings.scip.enabled.to_string()),
    ("scip.debounce_secs", settings.scip.debounce_secs.to_string()),
    ("max_file_bytes", settings.max_file_bytes.to_string()),
    ("lsp.enabled", settings.lsp.enabled.to_string()),
    ("lsp.download", settings.lsp.download.to_string()),
    (
      "lsp.typescript_command",
      settings
        .lsp
        .typescript_command
        .as_ref()
        .map_or_else(|| "(discover)".to_owned(), |command| command.join(" ")),
    ),
    (
      "lsp.unit",
      match settings.lsp.unit {
        LspUnit::Project => "project".to_owned(),
        LspUnit::Repo => "repo".to_owned(),
      },
    ),
    ("lsp.max_servers", settings.lsp.max_servers.to_string()),
    ("lsp.idle_secs", settings.lsp.idle_secs.to_string()),
    ("lsp.ready_timeout_secs", settings.lsp.ready_timeout_secs.to_string()),
    ("lsp.check_timeout_secs", settings.lsp.check_timeout_secs.to_string()),
    (
      "lsp.exclude",
      if settings.lsp.exclude.is_empty() {
        "(none)".to_owned()
      } else {
        settings.lsp.exclude.join(", ")
      },
    ),
  ]
}

fn changed_fields(before: &ProjectSettings, after: &ProjectSettings) -> Vec<String> {
  setting_lines(before)
    .into_iter()
    .zip(setting_lines(after))
    .filter(|((_, old), (_, new))| old != new)
    .map(|((name, old), (_, new))| format!("{name}: {old} -> {new}"))
    .collect()
}

fn show_settings(settings: &ProjectSettings, json: bool) -> CliResult<Outcome> {
  if json {
    emit_json(settings)?;
    return Ok(Outcome::Complete);
  }
  for (name, value) in setting_lines(settings) {
    emit_line(format!("{name} = {value}"))?;
  }
  Ok(Outcome::Complete)
}
