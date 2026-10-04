use std::ffi::OsStr;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use atlas_project::Project;
use serde_json::{Value, json};

use crate::cli::{ConnectClaudeArgs, ConnectCommand, ConnectScope};
use crate::confirm::confirm;
use crate::context::Context;
use crate::diff::unified;
use crate::error::{CliError, CliResult};
use crate::json_edit::{JsonEditError, upsert_server};
use crate::output::{Outcome, emit, emit_line, note};

const SERVER_NAME: &str = "code-atlas";
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
}

pub fn run(ctx: &Context, command: ConnectCommand) -> CliResult<Outcome> {
  match command {
    ConnectCommand::Claude(args) => claude(ctx, args),
  }
}

fn claude(ctx: &Context, args: ConnectClaudeArgs) -> CliResult<Outcome> {
  let project = ctx.resolve()?;
  let executable = std::env::current_exe()
    .and_then(fs::canonicalize)
    .map_err(|source| CliError::io("locate", "the code-atlas executable", source))?;
  let root = connection_root(ctx, &project)?;
  let atlas_home = std::env::var_os(HOME_VAR).filter(|value| !value.is_empty());
  let user_scope = args.scope == ConnectScope::User;
  let server = server_entry(&executable, project.name.as_str(), atlas_home.as_deref().filter(|_| user_scope));
  if user_scope {
    probe_claude(&server)?;
  } else if atlas_home.is_some() {
    note(format!(
      "warning: {HOME_VAR} is set but was left out of the project .mcp.json because that file is usually committed; the server will use the default home"
    ));
  }
  let mut plans = vec![plan_guidance(&root, project.name.as_str())?];
  if !user_scope {
    plans.insert(0, plan_mcp_config(root.join(".mcp.json"), &server)?);
  }
  for plan in &plans {
    let status = match (&plan.before, plan.changed()) {
      (_, false) => "unchanged",
      (None, true) => "created",
      (Some(_), true) => "updated",
    };
    emit_line(format!("{}: {status}", plan.path.display()))?;
    if plan.changed() {
      emit(&unified(&plan.path, plan.before.as_deref(), &plan.after))?;
    }
  }
  let overwritten: Vec<String> = plans
    .iter()
    .filter(|plan| plan.changed() && plan.before.is_some())
    .map(|plan| plan.path.display().to_string())
    .collect();
  if !overwritten.is_empty() {
    confirm(&format!("apply these changes to {}", overwritten.join(", ")), args.yes)?;
  }
  for plan in plans.iter().filter(|plan| plan.changed()) {
    write(plan)?;
  }
  if user_scope {
    register_user_scope(&server)?;
    emit_line(format!("registered {SERVER_NAME} in the Claude Code user scope"))?;
  }
  Ok(Outcome::Complete)
}

fn connection_root(ctx: &Context, project: &Project) -> CliResult<PathBuf> {
  let cwd = fs::canonicalize(&ctx.cwd).map_err(|source| CliError::io("canonicalize", &ctx.cwd, source))?;
  let repo = project
    .repos
    .iter()
    .filter(|repo| cwd.starts_with(&repo.path))
    .max_by_key(|repo| repo.path.components().count());
  Ok(repo.map_or(cwd, |repo| repo.path.clone()))
}

fn server_entry(executable: &Path, project: &str, home: Option<&OsStr>) -> Value {
  let mut entry = json!({
    "type": "stdio",
    "command": executable.display().to_string(),
    "args": ["mcp", "--project", project],
  });
  if let Some(home) = home {
    entry["env"] = json!({ HOME_VAR: home.to_string_lossy() });
  }
  entry
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
     - When a relation drives a decision, run `explain` on it to see its evidence.\n\
     - Treat results tagged `[heuristic]` or `[inferred]` as leads to verify in the source; untagged results are authoritative.\n\
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
