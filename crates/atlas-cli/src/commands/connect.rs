use std::ffi::OsStr;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use atlas_project::{Binding, Resolution};
use serde_json::{Value, json};

use crate::cli::{ConnectClaudeArgs, ConnectCommand, ConnectScope};
use crate::confirm::confirm;
use crate::context::Context;
use crate::diff::unified;
use crate::error::{CliError, CliResult};
use crate::json_edit::{JsonEditError, upsert_server};
use crate::output::{Outcome, emit, emit_line, note};

pub(crate) const SERVER_NAME: &str = "code-atlas";
const BINARY_NAME: &str = "catl";
const CLAUDE_CONFIG_DIR_VAR: &str = "CLAUDE_CONFIG_DIR";
const CLAUDE_CONFIG_FILE: &str = ".claude.json";
const HOME_VAR: &str = "CODE_ATLAS_HOME";
const GUIDANCE_BEGIN: &str = "<!-- code-atlas:begin -->";
const GUIDANCE_END: &str = "<!-- code-atlas:end -->";
const GUIDANCE_FILES: [&str; 2] = ["CLAUDE.md", "AGENTS.md"];
const NEW_GUIDANCE_FILE: &str = "AGENTS.md";

struct Planned {
  path: PathBuf,
  before: Option<String>,
  after: String,
}

impl Planned {
  fn changed(&self) -> bool {
    self.before.as_deref() != Some(self.after.as_str())
  }

  fn change(&self) -> Change {
    match (&self.before, self.changed()) {
      (_, false) => Change::Unchanged,
      (None, true) => Change::Created,
      (Some(_), true) => Change::Updated,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
  Unchanged,
  Created,
  Updated,
}

impl Change {
  fn label(self) -> &'static str {
    match self {
      Self::Unchanged => "unchanged",
      Self::Created => "created",
      Self::Updated => "updated",
    }
  }
}

pub fn run(ctx: &Context, command: ConnectCommand) -> CliResult<Outcome> {
  match command {
    ConnectCommand::Claude(args) => claude(ctx, args),
  }
}

fn claude(ctx: &Context, args: ConnectClaudeArgs) -> CliResult<Outcome> {
  let resolution = ctx.resolution()?;
  let project = &resolution.project;
  if let Some(caveat) = resolution.binding.caveat() {
    note(caveat);
  }
  let executable = std::env::current_exe()
    .and_then(fs::canonicalize)
    .map_err(|source| CliError::io("locate", "the catl executable", source))?;
  let root = connection_root(ctx, &resolution)?;
  let atlas_home = std::env::var_os(HOME_VAR).filter(|value| !value.is_empty());
  let mut plans = vec![plan_guidance(&root, project.name.as_str())?];
  let registration = match args.scope {
    ConnectScope::User => {
      let server = user_entry(&executable, atlas_home.as_deref());
      probe_claude(&server)?;
      Some(plan_registration(server))
    }
    ConnectScope::Project => {
      if atlas_home.is_some() {
        note(format!(
          "warning: {HOME_VAR} is set but was left out of the project .mcp.json because that file is usually committed; the server will use the default home"
        ));
      }
      let server = project_entry(&project_command(&executable), project.name.as_str());
      plans.insert(0, plan_mcp_config(root.join(".mcp.json"), &server)?);
      None
    }
  };
  for plan in &plans {
    emit_line(format!("{}: {}", plan.path.display(), plan.change().label()))?;
    if plan.changed() {
      emit(&unified(&plan.path, plan.before.as_deref(), &plan.after))?;
    }
  }
  if let Some(registration) = &registration {
    registration.show()?;
  }
  let mut overwritten: Vec<String> = plans
    .iter()
    .filter(|plan| plan.changed() && plan.before.is_some())
    .map(|plan| plan.path.display().to_string())
    .collect();
  overwritten.extend(registration.iter().filter(|registration| registration.replaces()).map(Registration::label));
  if !overwritten.is_empty() {
    confirm(&format!("apply these changes to {}", overwritten.join(", ")), args.yes)?;
  }
  for plan in plans.iter().filter(|plan| plan.changed()) {
    write(plan)?;
  }
  if let Some(registration) = registration.filter(Registration::changed) {
    register_user_scope(&registration.after)?;
    emit_line(format!("registered {SERVER_NAME} in the Claude Code user scope"))?;
  }
  Ok(Outcome::Complete)
}

fn connection_root(ctx: &Context, resolution: &Resolution) -> CliResult<PathBuf> {
  if let Binding::UnresolvedWorktree(unresolved) = &resolution.binding {
    return Ok(unresolved.root.clone());
  }
  if let Binding::Worktree(worktree) = &resolution.binding {
    let counterpart = resolution
      .project
      .repo(worktree.repo.as_str())
      .and_then(|repo| repo.path.strip_prefix(&worktree.indexed_checkout).ok())
      .map(|relative| worktree.worktree.join(relative))
      .filter(|path| path.is_dir());
    return Ok(counterpart.unwrap_or_else(|| worktree.worktree.clone()));
  }
  let cwd = fs::canonicalize(&ctx.cwd).map_err(|source| CliError::io("canonicalize", &ctx.cwd, source))?;
  let repo = resolution
    .project
    .repos
    .iter()
    .filter(|repo| cwd.starts_with(&repo.path))
    .max_by_key(|repo| repo.path.components().count());
  Ok(repo.map_or(cwd, |repo| repo.path.clone()))
}

fn user_entry(executable: &Path, home: Option<&OsStr>) -> Value {
  let mut entry = json!({
    "type": "stdio",
    "command": executable.display().to_string(),
    "args": ["mcp", "--watch"],
  });
  if let Some(home) = home {
    entry["env"] = json!({ HOME_VAR: home.to_string_lossy() });
  }
  entry
}

fn project_entry(command: &str, project: &str) -> Value {
  json!({
    "type": "stdio",
    "command": command,
    "args": ["mcp", "--project", project, "--watch"],
  })
}

fn project_command(executable: &Path) -> String {
  if find_on_path(BINARY_NAME).and_then(|found| fs::canonicalize(found).ok()).as_deref() == Some(executable) {
    return BINARY_NAME.to_owned();
  }
  note(format!(
    "warning: `{BINARY_NAME}` on PATH is missing or is not this binary, so .mcp.json names {} directly, which makes the file machine-specific; put this {BINARY_NAME} on PATH and rerun before committing it",
    executable.display()
  ));
  executable.display().to_string()
}

fn find_on_path(name: &str) -> Option<PathBuf> {
  let path = std::env::var_os("PATH")?;
  std::env::split_paths(&path)
    .filter(|directory| directory.is_absolute())
    .map(|directory| directory.join(name))
    .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
  use std::os::unix::fs::PermissionsExt;
  fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
  path.is_file()
}

enum Existing {
  Absent,
  Present(Value),
  Unknown(String),
}

struct Registration {
  config: PathBuf,
  before: Existing,
  after: Value,
}

impl Registration {
  fn label(&self) -> String {
    format!("{} (user-scope {SERVER_NAME} server)", self.config.display())
  }

  fn changed(&self) -> bool {
    match &self.before {
      Existing::Present(before) => !same_server(before, &self.after),
      Existing::Absent | Existing::Unknown(_) => true,
    }
  }

  fn replaces(&self) -> bool {
    !matches!(self.before, Existing::Absent) && self.changed()
  }

  fn show(&self) -> CliResult<()> {
    let label = PathBuf::from(format!("{}#mcpServers.{SERVER_NAME}", self.config.display()));
    let after = pretty(&self.after)?;
    match &self.before {
      Existing::Unknown(reason) => {
        emit_line(format!("{}: replacing an entry that cannot be read ({reason})", label.display()))?;
        emit(&unified(&label, None, &after))
      }
      Existing::Absent => {
        emit_line(format!("{}: {}", label.display(), Change::Created.label()))?;
        emit(&unified(&label, None, &after))
      }
      Existing::Present(_) if !self.changed() => emit_line(format!("{}: {}", label.display(), Change::Unchanged.label())),
      Existing::Present(before) => {
        emit_line(format!("{}: {}", label.display(), Change::Updated.label()))?;
        emit(&unified(&label, Some(&pretty(before)?), &after))
      }
    }
  }
}

fn pretty(value: &Value) -> CliResult<String> {
  Ok(serde_json::to_string_pretty(value)? + "\n")
}

fn same_server(left: &Value, right: &Value) -> bool {
  let view = |entry: &Value| {
    let env = entry.get("env").filter(|env| env.as_object().is_some_and(|env| !env.is_empty())).cloned();
    (entry.get("command").cloned(), entry.get("args").cloned(), env)
  };
  view(left) == view(right)
}

pub(crate) fn claude_config_path() -> Option<PathBuf> {
  let non_empty = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
  non_empty(CLAUDE_CONFIG_DIR_VAR)
    .or_else(|| non_empty("HOME"))
    .map(|directory| directory.join(CLAUDE_CONFIG_FILE))
}

fn plan_registration(after: Value) -> Registration {
  let Some(config) = claude_config_path() else {
    return Registration {
      config: PathBuf::from(CLAUDE_CONFIG_FILE),
      before: Existing::Unknown("no home directory".to_owned()),
      after,
    };
  };
  let before = match fs::read_to_string(&config) {
    Ok(text) => match serde_json::from_str::<Value>(&text) {
      Ok(document) => document
        .get("mcpServers")
        .and_then(|servers| servers.get(SERVER_NAME))
        .cloned()
        .map_or(Existing::Absent, Existing::Present),
      Err(error) => Existing::Unknown(error.to_string()),
    },
    Err(error) if error.kind() == ErrorKind::NotFound => Existing::Absent,
    Err(error) => Existing::Unknown(error.to_string()),
  };
  Registration { config, before, after }
}

fn add_json_command(server: &Value) -> Vec<String> {
  ["mcp", "add-json", "--scope", "user", SERVER_NAME, &server.to_string()].map(str::to_owned).to_vec()
}

fn printable(arguments: &[String]) -> String {
  let quoted: Vec<String> = arguments
    .iter()
    .map(|argument| match argument.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:".contains(c)) {
      true => argument.clone(),
      false => format!("'{}'", argument.replace('\'', "'\\''")),
    })
    .collect();
  format!("claude {}", quoted.join(" "))
}

fn probe_claude(server: &Value) -> CliResult<()> {
  match Command::new("claude")
    .arg("--version")
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status()
  {
    Ok(_) => Ok(()),
    Err(error) => Err(CliError::ClaudeUnavailable {
      reason: error.to_string(),
      command: printable(&add_json_command(server)),
    }),
  }
}

fn register_user_scope(server: &Value) -> CliResult<()> {
  let removal = ["mcp", "remove", "--scope", "user", SERVER_NAME];
  let _ = Command::new("claude")
    .args(removal)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status();
  let arguments = add_json_command(server);
  let output = Command::new("claude")
    .args(&arguments)
    .stdin(Stdio::null())
    .output()
    .map_err(|error| CliError::ClaudeUnavailable {
      reason: error.to_string(),
      command: printable(&arguments),
    })?;
  if output.status.success() {
    return Ok(());
  }
  Err(CliError::ClaudeUnavailable {
    reason: format!("`claude mcp add-json` failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr).trim()),
    command: printable(&arguments),
  })
}

fn read_existing(path: &Path) -> CliResult<Option<String>> {
  match fs::read_to_string(path) {
    Ok(text) => Ok(Some(text)),
    Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
    Err(source) => Err(CliError::io("read", path, source)),
  }
}

fn plan_mcp_config(path: PathBuf, server: &Value) -> CliResult<Planned> {
  let before = read_existing(&path)?;
  let invalid = |message: &str| CliError::InvalidConfig {
    path: path.clone(),
    message: message.to_owned(),
  };
  let Some(text) = before.as_deref().filter(|text| !text.trim().is_empty()) else {
    let mut after = serde_json::to_string_pretty(&json!({ "mcpServers": { SERVER_NAME: server } }))?;
    after.push('\n');
    return Ok(Planned { path, before, after });
  };
  let parsed: Value = serde_json::from_str(text).map_err(|error| invalid(&error.to_string()))?;
  if !parsed.is_object() {
    return Err(invalid("the top level is not an object"));
  }
  let edited = upsert_server(text, &["mcpServers"], SERVER_NAME, server).map_err(|JsonEditError::Unsupported(reason)| invalid(reason))?;
  let after = edited.unwrap_or_else(|| text.to_owned());
  Ok(Planned { path, before, after })
}

fn guidance_block(project: &str) -> String {
  format!(
    "{GUIDANCE_BEGIN}\n\
     ## Code Atlas\n\
     \n\
     This repo is indexed by Code Atlas (project `{project}`), a cross-repo knowledge graph served by the `{SERVER_NAME}` MCP server. Prefer it to grep when a question spans files or repos.\n\
     \n\
     - Start a task with `context`, giving it the task description.\n\
     - Before changing a shared symbol, endpoint or type, run `impact` on it.\n\
     - Before writing a new helper, run `search` and `similar` to find code to reuse.\n\
     - Read only the code the results list. Use `atlas_symbol` with `mode=source` only for code you will not edit yourself; read files you will edit with your own tool.\n\
     - To rename or rewrite a symbol, use `atlas_edit` (exact across files), not find-and-replace. After it changes a file, re-read that file before editing it yourself.\n\
     - After editing, call `atlas_status` with `since_run`, or re-query; there is no reindex tool.\n\
     - When a relation drives a decision, run `explain` on it to see its evidence.\n\
     - Untagged entities and relations tagged `[resolved]`, `[syntactic]`, `[compiler]` or `[declared]` are reliable but not exhaustive; treat anything tagged `[heuristic]` or `[inferred]` as a lead to verify in the source. A tag after an entity name tags the entity; a tag in parentheses or in a `verb target [tag]` step tags the relation.\n\
     - If an answer's first line says DEGRADED, verify it against the files before relying on it.\n\
     {GUIDANCE_END}"
  )
}

fn plan_guidance(root: &Path, project: &str) -> CliResult<Planned> {
  let block = guidance_block(project);
  let mut existing = Vec::new();
  for name in GUIDANCE_FILES {
    let path = root.join(name);
    if let Some(text) = read_existing(&path)? {
      existing.push((path, text));
    }
  }
  let target = existing.iter().find(|(_, text)| text.contains(GUIDANCE_BEGIN)).or(existing.first());
  match target {
    Some((path, text)) => Ok(Planned {
      path: path.clone(),
      before: Some(text.clone()),
      after: upsert_block(path, text, &block)?,
    }),
    None => Ok(Planned {
      path: root.join(NEW_GUIDANCE_FILE),
      before: None,
      after: format!("{block}\n"),
    }),
  }
}

fn upsert_block(path: &Path, text: &str, block: &str) -> CliResult<String> {
  let begins: Vec<usize> = text.match_indices(GUIDANCE_BEGIN).map(|(index, _)| index).collect();
  let ends: Vec<usize> = text.match_indices(GUIDANCE_END).map(|(index, _)| index).collect();
  match (begins.as_slice(), ends.as_slice()) {
    ([], []) => {
      let mut after = text.to_owned();
      if !after.is_empty() && !after.ends_with('\n') {
        after.push('\n');
      }
      if !after.is_empty() && !after.ends_with("\n\n") {
        after.push('\n');
      }
      after.push_str(block);
      after.push('\n');
      Ok(after)
    }
    ([begin], [end]) if begin < end => Ok(format!("{}{block}{}", &text[..*begin], &text[end + GUIDANCE_END.len()..])),
    _ => Err(CliError::MalformedGuidance(path.to_path_buf())),
  }
}

fn write(plan: &Planned) -> CliResult<()> {
  let Some(_) = plan.before else {
    return fs::write(&plan.path, &plan.after).map_err(|source| CliError::io("write", &plan.path, source));
  };
  let target = fs::canonicalize(&plan.path).map_err(|source| CliError::io("canonicalize", &plan.path, source))?;
  let directory = target.parent().unwrap_or(Path::new("."));
  let io = |source| CliError::io("write", &target, source);
  let mut temp = tempfile::NamedTempFile::new_in(directory).map_err(io)?;
  temp.write_all(plan.after.as_bytes()).map_err(io)?;
  let permissions = fs::metadata(&target).map_err(io)?.permissions();
  fs::set_permissions(temp.path(), permissions).map_err(io)?;
  temp.persist(&target).map_err(|error| io(error.error))?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_append_the_block_after_existing_content_without_touching_it() {
    let block = guidance_block("acme");
    let after = upsert_block(Path::new("CLAUDE.md"), "# Mine\nkeep me", &block).unwrap();
    assert_eq!(after, format!("# Mine\nkeep me\n\n{block}\n"));
  }

  #[test]
  fn should_replace_only_the_marked_block() {
    let old = guidance_block("old");
    let text = format!("before\n{old}\nafter\n");
    let after = upsert_block(Path::new("CLAUDE.md"), &text, &guidance_block("new")).unwrap();
    assert_eq!(after, format!("before\n{}\nafter\n", guidance_block("new")));
  }

  #[test]
  fn should_refuse_an_unterminated_block() {
    let text = format!("{GUIDANCE_BEGIN}\nhand written\n");
    assert!(matches!(
      upsert_block(Path::new("CLAUDE.md"), &text, &guidance_block("p")),
      Err(CliError::MalformedGuidance(_))
    ));
  }
}
