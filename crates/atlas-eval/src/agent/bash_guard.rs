use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use super::error::{AgentError, AgentResult};
use super::hooks::PRE_TOOL_USE;
use super::process::run_captured;
use super::shell::refused_program;
use super::transcript::{HookPhase, ToolCall, Transcript};

pub const BASH_GUARD_REASON: &str = "Code Atlas eval (forced surface): Bash search and read commands are refused in this session. \
Find code with atlas_search or atlas_context, list a file's symbols with atlas_symbol mode=outline, and open files with Read. \
A filter after a pipe (for example `bun test | grep fail`) is allowed.";
pub const BASH_TOOL: &str = "Bash";
const GUARD_ARGS: &str = "agent bash-guard";
const GUARD_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_EVENT_BYTES: u64 = 64 * 1024 * 1024;
const DENY: &str = "deny";
const SHELL: &str = "/bin/sh";
const SELF_CHECK: [(&str, Verdict); 2] = [("grep -rn foo src", Verdict::Deny), ("bun test | grep fail", Verdict::Allow)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
  Allow,
  Deny,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HookOutput {
  hook_specific_output: Decision,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Decision {
  hook_event_name: &'static str,
  permission_decision: &'static str,
  permission_decision_reason: &'static str,
}

pub fn deny_output() -> String {
  let output = HookOutput {
    hook_specific_output: Decision {
      hook_event_name: PRE_TOOL_USE,
      permission_decision: DENY,
      permission_decision_reason: BASH_GUARD_REASON,
    },
  };
  serde_json::to_string(&output).unwrap_or_default()
}

pub fn bash_guard_output(event: &str) -> Option<String> {
  let event: Value = serde_json::from_str(event).ok()?;
  if event.get("tool_name")?.as_str()? != BASH_TOOL {
    return None;
  }
  let command = event.get("tool_input")?.get("command")?.as_str()?;
  refused_program(command).map(|_| deny_output())
}

// Claude Code reads exit code 2 as a block and any other non-zero code as a hook error, so every path answers through stdout and exits 0.
pub fn run_bash_guard(input: &mut impl Read, output: &mut impl Write) {
  let decision = catch_unwind(AssertUnwindSafe(|| {
    let mut event = String::new();
    input.take(MAX_EVENT_BYTES).read_to_string(&mut event).ok()?;
    bash_guard_output(&event)
  }));
  if let Ok(Some(text)) = decision {
    let _ = output.write_all(text.as_bytes()).and_then(|()| output.flush());
  }
}

pub fn bash_guard_command(binary: &Path) -> String {
  format!("{} {GUARD_ARGS} || true", shell_quote(&binary.display().to_string()))
}

pub fn bash_guard_group(binary: &Path) -> Value {
  json!({
    "matcher": BASH_TOOL,
    "hooks": [{ "type": "command", "command": bash_guard_command(binary), "timeout": GUARD_TIMEOUT.as_secs() }]
  })
}

pub fn check_bash_guard(binary: &Path) -> AgentResult<()> {
  let command = bash_guard_command(binary);
  for (bash, verdict) in SELF_CHECK {
    let event = json!({"hook_event_name": PRE_TOOL_USE, "tool_name": BASH_TOOL, "tool_input": {"command": bash}}).to_string();
    let mut shell = Command::new(SHELL);
    shell.arg("-c").arg(&command);
    let output = run_captured(shell, Some(event.as_bytes()), GUARD_TIMEOUT);
    let expected = match verdict {
      Verdict::Deny => deny_output(),
      Verdict::Allow => String::new(),
    };
    if output.exit.succeeded() && output.stdout == expected {
      continue;
    }
    return Err(AgentError::AtlasSurfaceUnavailable {
      reason: format!(
        "the Bash guard self-check failed: `{command}` answered {:?} (exit {:?}, stderr {:?}) to the Bash command `{bash}`, expected {expected:?}",
        output.stdout,
        output.exit,
        output.stderr.trim()
      ),
    });
  }
  Ok(())
}

pub fn denied_by_bash_guard(call: &ToolCall) -> bool {
  call.name == BASH_TOOL && call.result.as_ref().is_some_and(|result| result.text.contains(BASH_GUARD_REASON))
}

pub fn bash_guard_denials(transcript: &Transcript) -> u64 {
  // With --include-hook-events a denial is reported twice, as the guard's hook event and as the refused tool result; either alone also counts.
  let results = transcript.tool_calls.iter().filter(|call| denied_by_bash_guard(call)).count();
  let hook_events = transcript
    .hooks
    .iter()
    .filter(|hook| hook.event == PRE_TOOL_USE)
    .filter(|hook| matches!(&hook.phase, HookPhase::Finished { stdout, .. } if is_guard_denial(stdout)))
    .count();
  results.max(hook_events) as u64
}

fn is_guard_denial(stdout: &str) -> bool {
  let Ok(output) = serde_json::from_str::<Value>(stdout.trim()) else {
    return false;
  };
  let decision = &output["hookSpecificOutput"];
  decision["permissionDecision"] == DENY && decision["permissionDecisionReason"] == BASH_GUARD_REASON
}

fn shell_quote(text: &str) -> String {
  let plain = !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@,=".contains(c));
  if plain {
    return text.to_owned();
  }
  format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use super::super::transcript::{HookEvent, ToolResult};
  use super::*;

  const FORCED: &str = include_str!("../../tests/fixtures/forced-transcript.jsonl");
  const DENY_JSON: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"Code Atlas eval (forced surface): Bash search and read commands are refused in this session. Find code with atlas_search or atlas_context, list a file's symbols with atlas_symbol mode=outline, and open files with Read. A filter after a pipe (for example `bun test | grep fail`) is allowed."}}"#;

  fn event(command: &str) -> String {
    json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": command, "description": "d"}}).to_string()
  }

  fn guarded(input: &[u8]) -> String {
    let mut output = Vec::new();
    run_bash_guard(&mut &input[..], &mut output);
    String::from_utf8(output).unwrap()
  }

  #[test]
  fn should_deny_bash_search_and_read_commands() {
    let denied = [
      "grep -rn x src",
      "cd a && rg x",
      "ls",
      "/usr/bin/find . -name y",
      "cat f | head",
      "sed -n 1,5p f",
      "git grep x",
      "echo $(grep x f)",
      "FOO=1 grep x",
    ];
    for command in denied {
      assert_eq!(guarded(event(command).as_bytes()), DENY_JSON, "{command}");
    }
  }

  #[test]
  fn should_allow_builds_tests_filters_after_a_pipe_and_quoted_text() {
    let allowed = [
      "bun test",
      "bun test | grep fail",
      "sed -i s/a/b/ f",
      "cd apps/x",
      "bun scripts/build.ts 'packages/*' > /dev/null && cd app && bun test t.spec.ts",
      "echo \"grep x\"",
      "cat 'unterminated",
      "echo $(grep x f",
    ];
    for command in allowed {
      assert_eq!(guarded(event(command).as_bytes()), "", "{command}");
    }
  }

  #[test]
  fn should_allow_every_event_it_cannot_read_or_that_is_not_a_bash_call() {
    let read = json!({"tool_name": "Read", "tool_input": {"command": "grep x f", "file_path": "/t/f"}}).to_string();
    let no_command = json!({"tool_name": "Bash", "tool_input": {"command": 3}}).to_string();
    for input in [read.as_bytes(), no_command.as_bytes(), b"grep -rn x src", b"", &[0xff, 0xfe]] {
      assert_eq!(guarded(input), "");
    }
  }

  #[test]
  fn should_print_the_exact_deny_decision() {
    assert_eq!(deny_output(), DENY_JSON);
    assert_eq!(bash_guard_output(&event("rg -n foo")).as_deref(), Some(DENY_JSON));
  }

  #[test]
  fn should_quote_the_guard_binary_for_the_hook_shell() {
    assert_eq!(bash_guard_command(Path::new("/opt/atlas-eval")), "/opt/atlas-eval agent bash-guard || true");
    assert_eq!(bash_guard_command(Path::new("/opt/eval tools/it's")), "'/opt/eval tools/it'\\''s' agent bash-guard || true");
    assert_eq!(
      bash_guard_group(Path::new("/opt/atlas-eval")),
      json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "/opt/atlas-eval agent bash-guard || true", "timeout": 10}]})
    );
  }

  #[test]
  fn should_count_each_denial_of_a_recorded_transcript_once() {
    let transcript = Transcript::parse(FORCED, &[]);
    assert_eq!(bash_guard_denials(&transcript), 2);
    let denied: Vec<&str> = transcript
      .tool_calls
      .iter()
      .filter(|call| denied_by_bash_guard(call))
      .filter_map(|call| call.input_str("command"))
      .collect();
    assert_eq!(denied, vec!["grep -rn deletedAt src", "cat src/users/user.entity.ts"]);
    let hooks_only = Transcript {
      hooks: transcript.hooks.clone(),
      ..Transcript::default()
    };
    assert_eq!(bash_guard_denials(&hooks_only), 2);
    let other_reason = Transcript {
      hooks: vec![HookEvent {
        event: PRE_TOOL_USE.to_owned(),
        name: "PreToolUse:Bash".to_owned(),
        phase: HookPhase::Finished {
          outcome: Some("success".to_owned()),
          exit_code: Some(0),
          stdout: r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"no"}}"#.to_owned(),
          stderr: String::new(),
        },
      }],
      ..Transcript::default()
    };
    assert_eq!(bash_guard_denials(&other_reason), 0);
  }

  #[test]
  fn should_not_take_a_denial_of_another_tool_for_a_guard_denial() {
    let call = ToolCall {
      id: "t".to_owned(),
      name: "Read".to_owned(),
      input: json!({}),
      elapsed_ms: None,
      result: Some(ToolResult {
        text: BASH_GUARD_REASON.to_owned(),
        is_error: true,
        ..ToolResult::default()
      }),
      turn: 1,
    };
    assert!(!denied_by_bash_guard(&call));
  }

  #[test]
  fn should_fail_the_self_check_of_a_guard_that_denies_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let silent = dir.path().join("silent guard");
    std::fs::write(&silent, "#!/bin/sh\ncat > /dev/null\n").unwrap();
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      std::fs::set_permissions(&silent, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let error = check_bash_guard(&silent).unwrap_err().to_string();
    assert!(error.starts_with("--atlas-surface forced: the Bash guard self-check failed: `"), "{error}");
    assert!(
      error.contains("silent guard' agent bash-guard || true` answered \"\" (exit Exited { code: Some(0) }"),
      "{error}"
    );
    assert!(error.contains("to the Bash command `grep -rn foo src`"), "{error}");
    let missing = check_bash_guard(&PathBuf::from("/definitely/missing/atlas-eval")).unwrap_err().to_string();
    assert!(missing.contains("answered \"\" (exit Exited { code: Some(0) }, stderr \"/bin/sh: "), "{missing}");
    assert!(missing.contains("/definitely/missing/atlas-eval"), "{missing}");
  }
}
