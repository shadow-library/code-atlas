use std::path::Path;
use std::time::Duration;

use atlas_eval::agent::{ProcessExit, check_bash_guard, deny_output, isolated_command, run_captured};

const GUARD: &str = env!("CARGO_BIN_EXE_atlas-eval");

fn guard(stdin: &[u8]) -> (ProcessExit, String, String) {
  let mut command = isolated_command(Path::new(GUARD));
  command.args(["agent", "bash-guard"]).env("RUST_LOG", "trace");
  let output = run_captured(command, Some(stdin), Duration::from_secs(30));
  (output.exit, output.stdout, output.stderr)
}

fn bash(command: &str) -> Vec<u8> {
  serde_json::json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": command}})
    .to_string()
    .into_bytes()
}

#[test]
fn should_print_the_deny_decision_for_a_search_and_exit_zero_without_stderr() {
  assert_eq!(guard(&bash("grep -rn foo src")), (ProcessExit::Exited { code: Some(0) }, deny_output(), String::new()));
}

#[test]
fn should_stay_silent_and_exit_zero_for_allowed_and_unreadable_events() {
  for stdin in [bash("bun test | grep fail"), b"not json".to_vec(), Vec::new(), vec![0xff, 0x00]] {
    assert_eq!(guard(&stdin), (ProcessExit::Exited { code: Some(0) }, String::new(), String::new()));
  }
}

#[test]
fn should_pass_its_own_self_check() {
  check_bash_guard(Path::new(GUARD)).unwrap();
}
