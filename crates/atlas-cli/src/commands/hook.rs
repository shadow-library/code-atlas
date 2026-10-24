mod also_change;
mod counterpart;
mod edits;
mod notes;
mod reuse;
mod seed;

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use atlas_mcp::guidance::reminder;
use atlas_project::{ConfigEnv, McpFlags, Registry, resolve_mcp};
use serde_json::{Value, json};

use crate::cli::HookName;
use crate::commands::mcp::tool_profile;
use crate::context::Context;
use crate::error::CliResult;
use crate::output::{Outcome, emit_line};

const MAX_INPUT_BYTES: u64 = 4 * 1024 * 1024;
const PRE_TOOL_USE: &str = "PreToolUse";
const REMINDED_DIR: &str = "reminded";
const SEEDED_DIR: &str = "seeded";
const COUNTERPART_DIR: &str = "counterpart";
const NOTES_DIR: &str = "notes";
const REUSE_DIR: &str = "reuse";
const ALSO_CHANGE_DIR: &str = "also-change";
const TIME_LIMIT: Duration = Duration::from_secs(3);
const WAIT_MARGIN: Duration = Duration::from_millis(250);
const MARKER_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const SEARCH_PROGRAMS: [&str; 6] = ["grep", "egrep", "fgrep", "rg", "ag", "ack"];

struct SessionMarkers<Seen, Claim> {
  seen: Seen,
  claim: Claim,
}

fn markers_in(directory: PathBuf) -> SessionMarkers<impl Fn(&str) -> bool + Send + 'static, impl Fn(&str) -> bool + Send + Sync + 'static> {
  let seen_in = directory.clone();
  SessionMarkers {
    seen: move |key: &str| marker_path(&seen_in, key).exists(),
    claim: move |key: &str| first_in_session(&directory, key),
  }
}

pub fn run(ctx: &Context, hook: HookName) -> CliResult<Outcome> {
  let mut input = String::new();
  if let Err(error) = std::io::stdin().take(MAX_INPUT_BYTES).read_to_string(&mut input) {
    tracing::debug!(error = %error, "cannot read the hook input");
    return Ok(Outcome::Complete);
  }
  let Ok(event) = serde_json::from_str::<Value>(&input) else {
    tracing::debug!("the hook input is not JSON");
    return Ok(Outcome::Complete);
  };
  let answer = match hook {
    HookName::Remind => remind(
      &event,
      |cwd| project_reminder(ctx, cwd),
      |session| first_in_session(&ctx.home.hook_state_dir().join(REMINDED_DIR), session),
    ),
    HookName::Seed => seed::seed(&ctx.home, ctx.project.as_deref(), &event, &markers_in(ctx.home.hook_state_dir().join(SEEDED_DIR))),
    HookName::Counterpart => counterpart::counterpart(&ctx.home, ctx.project.as_deref(), &event, markers_in(ctx.home.hook_state_dir().join(COUNTERPART_DIR))),
    HookName::Notes => notes::notes(&ctx.home, ctx.project.as_deref(), &event, markers_in(ctx.home.hook_state_dir().join(NOTES_DIR))),
    HookName::Reuse => reuse::reuse(&ctx.home, ctx.project.as_deref(), &event, markers_in(ctx.home.hook_state_dir().join(REUSE_DIR))),
    HookName::AlsoChange => also_change::also_change(&ctx.home, ctx.project.as_deref(), &event, markers_in(ctx.home.hook_state_dir().join(ALSO_CHANGE_DIR))),
  };
  if let Some(answer) = answer {
    emit_line(answer.to_string())?;
  }
  Ok(Outcome::Complete)
}

fn pre_tool_use(event: &Value) -> Option<&str> {
  (event.get("hook_event_name")?.as_str()? == PRE_TOOL_USE).then_some(())?;
  event.get("tool_name")?.as_str()
}

fn remind(event: &Value, reminder_in: impl Fn(&Path) -> Option<String>, first_in_session: impl Fn(&str) -> bool) -> Option<Value> {
  let tool = pre_tool_use(event)?;
  let searching = match tool {
    "Grep" | "Glob" => true,
    "Bash" => event
      .get("tool_input")
      .and_then(|input| input.get("command"))
      .and_then(Value::as_str)
      .is_some_and(searches_text),
    _ => false,
  };
  if !searching {
    return None;
  }
  let cwd = event.get("cwd").and_then(Value::as_str).map(PathBuf::from)?;
  let session = event.get("session_id").and_then(Value::as_str).filter(|session| !session.is_empty())?;
  let reminder = reminder_in(&cwd)?;
  first_in_session(session).then(|| json!({ "hookSpecificOutput": { "hookEventName": PRE_TOOL_USE, "additionalContext": reminder } }))
}

fn searches_text(command: &str) -> bool {
  command
    .split(['|', ';', '&', '\n', '(', ')'])
    .map(|segment| segment.split_whitespace().skip_while(|word| word.contains('=')).collect::<Vec<_>>())
    .any(|words| match words.as_slice() {
      ["git", "grep", ..] => true,
      [program, ..] => SEARCH_PROGRAMS.contains(&Path::new(program).file_name().and_then(|name| name.to_str()).unwrap_or(program)),
      [] => false,
    })
}

fn project_reminder(ctx: &Context, cwd: &Path) -> Option<String> {
  let registry = Registry::load(&ctx.home).ok()?;
  registry.resolve(cwd, None).ok()?;
  match tool_profile(&resolve_mcp(registry.user_config(), &ConfigEnv::default(), McpFlags::default())) {
    Ok(profile) => reminder(&profile),
    Err(error) => {
      tracing::debug!(error = %error, "cannot resolve the tool profile for the reminder");
      None
    }
  }
}

fn first_in_session(directory: &Path, session: &str) -> bool {
  if let Err(error) = fs::create_dir_all(directory) {
    tracing::debug!(path = %directory.display(), error = %error, "cannot create the reminder state directory");
    return false;
  }
  let marker = marker_path(directory, session);
  match OpenOptions::new().write(true).create_new(true).open(&marker) {
    Ok(_) => {
      forget_old_sessions(directory);
      true
    }
    Err(error) if error.kind() == ErrorKind::AlreadyExists => false,
    Err(error) => {
      tracing::debug!(path = %marker.display(), error = %error, "cannot record the reminder");
      false
    }
  }
}

fn marker_path(directory: &Path, session: &str) -> PathBuf {
  directory.join(blake3::hash(session.as_bytes()).to_hex().as_str())
}

fn forget_old_sessions(directory: &Path) {
  let Ok(entries) = fs::read_dir(directory) else { return };
  let Some(cutoff) = SystemTime::now().checked_sub(MARKER_LIFETIME) else { return };
  for entry in entries.flatten() {
    let stale = entry.metadata().and_then(|metadata| metadata.modified()).is_ok_and(|modified| modified < cutoff);
    if stale {
      let _ = fs::remove_file(entry.path());
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn event(tool: &str, input: Value) -> Value {
    json!({
      "session_id": "s1",
      "transcript_path": "/t.jsonl",
      "cwd": "/src/app",
      "permission_mode": "default",
      "hook_event_name": "PreToolUse",
      "tool_name": tool,
      "tool_input": input,
      "tool_use_id": "toolu_1"
    })
  }

  #[test]
  fn should_remind_on_text_searches_inside_a_project_once_per_session_without_deciding_anything() {
    let reminded = std::cell::Cell::new(false);
    let once = |_: &str| !reminded.replace(true);
    let text = |_: &Path| Some("Use the index.".to_owned());
    let answer = remind(&event("Grep", json!({"pattern": "x"})), text, once);
    assert_eq!(
      answer,
      Some(json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": "Use the index."}}))
    );
    assert_eq!(remind(&event("Glob", json!({"pattern": "**/*.rs"})), text, once), None);
    assert_eq!(remind(&event("Grep", json!({})), |_| None, |_| true), None);
    assert_eq!(remind(&event("Read", json!({})), text, |_| true), None);
    assert_eq!(remind(&event("Bash", json!({"command": "cargo test"})), text, |_| true), None);
    assert!(remind(&event("Bash", json!({"command": "cd src && rg -n foo"})), text, |_| true).is_some());
  }

  #[test]
  fn should_keep_the_session_unclaimed_when_there_is_no_reminder_to_give() {
    let claimed = std::cell::Cell::new(false);
    let answer = remind(&event("Grep", json!({"pattern": "x"})), |_| None, |_| !claimed.replace(true));
    assert_eq!((answer, claimed.get()), (None, false));
  }

  #[test]
  fn should_recognise_text_search_commands() {
    for command in ["grep -rn foo .", "rg foo", "LC_ALL=C grep x", "git grep -n x", "cat a | /usr/bin/grep b", "(cd x; ag y)"] {
      assert!(searches_text(command), "{command}");
    }
    for command in ["cargo build", "git status", "echo grep", "ls -la"] {
      assert!(!searches_text(command), "{command}");
    }
  }

  #[test]
  fn should_record_a_session_once_and_survive_concurrent_hooks() {
    let dir = tempfile::tempdir().unwrap();
    assert!(first_in_session(dir.path(), "abc"));
    assert!(!first_in_session(dir.path(), "abc"));
    assert!(first_in_session(dir.path(), "def"));
  }
}
