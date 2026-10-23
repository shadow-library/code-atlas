use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::claude::{ClaudeConfig, HookWiring, SERENA_SERVER_NAME, SessionRequest, SessionTools, host_env, session_invocation};
use super::error::{AgentError, AgentResult};
use super::hooks::{PRE_TOOL_USE, hook_added_context, hook_metrics};
use super::incumbent::{INCUMBENT_HOOKS_FILE, IncumbentTools, incumbent_instructions};
use super::process::{ProcessExit, run_streaming};
use super::record::write_json;
use super::transcript::Transcript;

pub const SESSION_START: &str = "SessionStart";
pub const INCUMBENT_PROBE_DIR: &str = "incumbent";
pub const INCUMBENT_PROBE_FILE: &str = "probe.json";
const PROBE_TIMEOUT: Duration = Duration::from_secs(300);
const PROBE_MAX_TURNS: u32 = 1;
const PROBE_PROMPT: &str = "Reply with the single word ok. Use no tool.";
const PROBE_SOURCE: (&str, &str) = ("probe.ts", "export const probe = \"ATLAS-EVAL-INCUMBENT-PROBE\";\n");
const MCP_FILE: &str = "mcp.json";
const CONNECTED: &str = "connected";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IncumbentProbe {
  pub tools_blake3: String,
  pub exit: ProcessExit,
  pub started: bool,
  pub model_turns: usize,
  pub serena_status: Option<String>,
  pub hooks_seen: Vec<String>,
  pub session_start_injected: bool,
  pub cost_usd: Option<f64>,
}

impl IncumbentProbe {
  pub fn problem(&self, out_dir: &Path) -> Option<String> {
    let logs = format!("see {} and {}", out_dir.join("claude.stderr.log").display(), out_dir.join("transcript.jsonl").display());
    if !self.started {
      return Some(format!(
        "the incumbent probe session did not start (no init event, exit {:?}), so it could not show the Serena hooks; {logs}",
        self.exit
      ));
    }
    if self.serena_status.as_deref() != Some(CONNECTED) {
      let status = self
        .serena_status
        .as_deref()
        .map_or_else(|| "was not listed".to_owned(), |status| format!("reported status `{status}`"));
      return Some(format!("the {SERENA_SERVER_NAME} MCP server of the incumbent probe session {status}; {logs}"));
    }
    (!self.session_start_injected).then(|| {
      format!(
        "the incumbent probe session reported no {SESSION_START} hook event that succeeded and added context, so Claude Code does not run the Serena hooks under the eval flags, or stream-json does not report them, and no incumbent session could show that its hooks ran (hooks seen: {}); {logs}",
        if self.hooks_seen.is_empty() { "none".to_owned() } else { self.hooks_seen.join(", ") }
      )
    })
  }
}

pub fn session_start_injected(transcript: &Transcript) -> bool {
  transcript.hooks.iter().any(|hook| hook.event == SESSION_START && hook_added_context(hook))
}

pub fn incumbent_hooks_problem(transcript: &Transcript) -> Option<String> {
  transcript.init.as_ref()?;
  let metrics = hook_metrics(transcript);
  if !session_start_injected(transcript) {
    return Some(match metrics.ran(SESSION_START) {
      true => format!("the session's {SESSION_START} hook (`serena-hooks activate`) did not succeed with added context; see its hook_response in the transcript"),
      false => format!("the session reported no {SESSION_START} hook event, so the Serena hooks did not run"),
    });
  }
  metrics.only_failed(PRE_TOOL_USE).then(|| {
    format!(
      "every {PRE_TOOL_USE} hook of the session wrote to stderr and none added context, so the Serena hook commands are broken (see their hook_response stderr in the transcript)"
    )
  })
}

pub struct IncumbentProbeRequest<'a> {
  pub tools: &'a IncumbentTools,
  pub claude: &'a ClaudeConfig,
  pub out_dir: &'a Path,
  pub scratch: &'a Path,
  pub tree_env: Vec<(String, String)>,
}

pub fn run_incumbent_probe(request: &IncumbentProbeRequest<'_>) -> AgentResult<IncumbentProbe> {
  let (out_dir, scratch) = (request.out_dir, request.scratch);
  let tree = scratch.join("tree");
  fs::create_dir_all(&tree).map_err(|error| AgentError::io(&tree, error))?;
  let (name, content) = PROBE_SOURCE;
  fs::write(tree.join(name), content).map_err(|error| AgentError::io(tree.join(name), error))?;
  let home = request.tools.serena_home(scratch)?;
  let mcp_config = request.tools.mcp_config(&home).to_string();
  let settings = request.tools.hook_settings(&home).to_string();
  for (file, content) in [(MCP_FILE, &mcp_config), (INCUMBENT_HOOKS_FILE, &settings)] {
    for dir in [scratch, out_dir] {
      fs::write(dir.join(file), content).map_err(|error| AgentError::io(dir.join(file), error))?;
    }
  }
  let session = SessionRequest {
    prompt: PROBE_PROMPT.to_owned(),
    tree,
    mcp_config: scratch.join(MCP_FILE),
    max_turns: PROBE_MAX_TURNS,
    tools: SessionTools::Incumbent,
    config_dir: scratch.join("claude-config"),
    project_instructions: Some(incumbent_instructions(None)),
    tree_env: request.tree_env.clone(),
    hooks: HookWiring::Installed {
      settings: scratch.join(INCUMBENT_HOOKS_FILE),
    },
  };
  let invocation = session_invocation(request.claude, &session, &host_env());
  write_json(&out_dir.join("invocation.json"), &invocation.redacted())?;
  let transcript_path = out_dir.join("transcript.jsonl");
  let output = run_streaming(invocation.command(), PROBE_TIMEOUT, &transcript_path, &out_dir.join("claude.stderr.log")).map_err(|error| AgentError::io(&transcript_path, error))?;
  let text = fs::read_to_string(&transcript_path).map_err(|error| AgentError::io(&transcript_path, error))?;
  let transcript = Transcript::parse(&text, &output.line_offsets_ms);
  let hooks_seen: BTreeSet<String> = transcript.hooks.iter().map(|hook| format!("{} ({})", hook.event, hook.name)).collect();
  let probe = IncumbentProbe {
    tools_blake3: request.tools.probe_key(),
    exit: output.exit,
    started: transcript.init.is_some(),
    model_turns: transcript.turns.len(),
    serena_status: transcript
      .init
      .as_ref()
      .and_then(|init| init.mcp_servers.iter().find(|server| server.name == SERENA_SERVER_NAME))
      .map(|server| server.status.clone()),
    hooks_seen: hooks_seen.into_iter().collect(),
    session_start_injected: session_start_injected(&transcript),
    cost_usd: transcript.result.as_ref().and_then(|result| result.total_cost_usd),
  };
  write_json(&out_dir.join(INCUMBENT_PROBE_FILE), &probe)?;
  Ok(probe)
}

#[cfg(test)]
mod tests {
  use super::*;

  const INIT: &str = r#"{"type":"system","subtype":"init","cwd":"/t","tools":["Read"],"mcp_servers":[{"name":"serena","status":"connected"}],"model":"m"}"#;
  const ACTIVATED: &str = r#"{"type":"system","subtype":"hook_response","hook_id":"h1","hook_name":"SessionStart:startup","hook_event":"SessionStart","stdout":"{\"hookSpecificOutput\":{\"hookEventName\":\"SessionStart\",\"additionalContext\":\"activate the project\"}}","stderr":"","exit_code":0,"outcome":"success"}"#;
  const SILENT: &str = r#"{"type":"system","subtype":"hook_response","hook_id":"h1","hook_name":"SessionStart:startup","hook_event":"SessionStart","stdout":"","stderr":"","exit_code":0,"outcome":"success"}"#;
  const REMIND_FAILED: &str = r#"{"type":"system","subtype":"hook_response","hook_id":"h2","hook_name":"PreToolUse:Read","hook_event":"PreToolUse","stdout":"","stderr":"Traceback","exit_code":1,"outcome":"error"}"#;

  fn transcript(lines: &[&str]) -> Transcript {
    Transcript::parse(&lines.join("\n"), &[])
  }

  #[test]
  fn should_require_a_session_start_hook_that_added_context_and_a_working_reminder() {
    assert_eq!(incumbent_hooks_problem(&transcript(&[INIT, ACTIVATED])), None);
    assert_eq!(
      incumbent_hooks_problem(&transcript(&[INIT])),
      Some("the session reported no SessionStart hook event, so the Serena hooks did not run".to_owned())
    );
    assert_eq!(
      incumbent_hooks_problem(&transcript(&[INIT, SILENT])),
      Some("the session's SessionStart hook (`serena-hooks activate`) did not succeed with added context; see its hook_response in the transcript".to_owned())
    );
    assert!(
      incumbent_hooks_problem(&transcript(&[INIT, ACTIVATED, REMIND_FAILED]))
        .unwrap()
        .starts_with("every PreToolUse hook of the session wrote to stderr")
    );
    assert_eq!(incumbent_hooks_problem(&transcript(&[])), None);
  }

  #[test]
  fn should_pass_a_probe_only_when_it_started_connected_serena_and_saw_the_session_start_hook() {
    let probe = IncumbentProbe {
      tools_blake3: "k".to_owned(),
      exit: ProcessExit::Exited { code: Some(0) },
      started: true,
      model_turns: 1,
      serena_status: Some("connected".to_owned()),
      hooks_seen: vec!["SessionStart (SessionStart:startup)".to_owned()],
      session_start_injected: true,
      cost_usd: Some(0.01),
    };
    let dir = Path::new("/b/_probe/incumbent");
    assert_eq!(probe.problem(dir), None);
    let failed = IncumbentProbe {
      serena_status: Some("failed".to_owned()),
      ..probe.clone()
    };
    assert_eq!(
      failed.problem(dir).unwrap(),
      "the serena MCP server of the incumbent probe session reported status `failed`; see /b/_probe/incumbent/claude.stderr.log and /b/_probe/incumbent/transcript.jsonl"
    );
    let unhooked = IncumbentProbe {
      hooks_seen: Vec::new(),
      session_start_injected: false,
      ..probe
    };
    assert!(unhooked.problem(dir).unwrap().starts_with(
      "the incumbent probe session reported no SessionStart hook event that succeeded and added context, so Claude Code does not run the Serena hooks under the eval flags, or stream-json does not report them, and no incumbent session could show that its hooks ran (hooks seen: none)"
    ));
  }
}
