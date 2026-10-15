use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use atlas_mcp::guidance::REMINDER;
use atlas_project::Registry;
use serde_json::{Value, json};

use crate::cli::HookName;
use crate::context::Context;
use crate::error::CliResult;
use crate::output::{Outcome, emit_line};

const MAX_INPUT_BYTES: u64 = 4 * 1024 * 1024;
const PRE_TOOL_USE: &str = "PreToolUse";
const REMINDED_DIR: &str = "reminded";
const MARKER_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const SEARCH_PROGRAMS: [&str; 6] = ["grep", "egrep", "fgrep", "rg", "ag", "ack"];

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
      |cwd| in_project(ctx, cwd),
      |session| first_in_session(&ctx.home.hook_state_dir().join(REMINDED_DIR), session),
    ),
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

fn remind(event: &Value, in_project: impl Fn(&Path) -> bool, first_in_session: impl Fn(&str) -> bool) -> Option<Value> {
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
  (in_project(&cwd) && first_in_session(session)).then(|| json!({ "hookSpecificOutput": { "hookEventName": PRE_TOOL_USE, "additionalContext": REMINDER } }))
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

fn in_project(ctx: &Context, cwd: &Path) -> bool {
  Registry::load(&ctx.home).is_ok_and(|registry| registry.resolve(cwd, None).is_ok())
}

fn first_in_session(directory: &Path, session: &str) -> bool {
  if let Err(error) = fs::create_dir_all(directory) {
    tracing::debug!(path = %directory.display(), error = %error, "cannot create the reminder state directory");
    return false;
  }
  let marker = directory.join(blake3::hash(session.as_bytes()).to_hex().as_str());
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
    let answer = remind(&event("Grep", json!({"pattern": "x"})), |_| true, once);
    assert_eq!(answer, Some(json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": REMINDER}})));
    assert_eq!(remind(&event("Glob", json!({"pattern": "**/*.rs"})), |_| true, once), None);
    assert_eq!(remind(&event("Grep", json!({})), |_| false, |_| true), None);
    assert_eq!(remind(&event("Read", json!({})), |_| true, |_| true), None);
    assert_eq!(remind(&event("Bash", json!({"command": "cargo test"})), |_| true, |_| true), None);
    assert!(remind(&event("Bash", json!({"command": "cd src && rg -n foo"})), |_| true, |_| true).is_some());
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
