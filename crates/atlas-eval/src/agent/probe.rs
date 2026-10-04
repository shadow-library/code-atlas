use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::claude::{ClaudeConfig, EMPTY_MCP_CONFIG, SessionRequest, host_env, session_invocation};
use super::error::{AgentError, AgentResult};
use super::process::{ProcessExit, run_streaming};
use super::record::write_json;
use super::transcript::Transcript;
use super::workspace::{check_isolated_location, scratch_dir};

pub const PROJECT_CANARY: &str = "ATLAS-EVAL-PROJECT-CANARY-5821";
const PROBE_TIMEOUT: Duration = Duration::from_secs(300);
const PROBE_MAX_TURNS: u32 = 6;
const HELLO_FILE: &str = "hello.txt";
const PROJECT_KEY: &str = "PROJECT_CANARY=";
const USER_KEY: &str = "USER_MD_HEADING=";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Visibility {
  Seen,
  NotSeen,
  NotApplicable { reason: String },
  Unknown { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HelloFile {
  Matches,
  Missing,
  Different { content: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeReport {
  pub exit: ProcessExit,
  pub claude_code_version: Option<String>,
  pub model_reported: Vec<String>,
  pub permission_mode: Option<String>,
  pub tools: Vec<String>,
  pub mcp_servers: Vec<String>,
  pub skills: Vec<String>,
  pub slash_commands: Vec<String>,
  pub plugins: Vec<String>,
  pub agents: Vec<String>,
  pub project_claude_md: Visibility,
  pub user_claude_md: Visibility,
  pub hello_file: HelloFile,
  pub tool_calls: Vec<String>,
  pub cost_usd: Option<f64>,
  pub answer: Option<String>,
}

pub fn probe_prompt() -> String {
  format!(
    "Create a file named {HELLO_FILE} in the current directory containing exactly the text `hi`. \
Then, without using any more tools, answer only from the instructions already in your context and end your reply with exactly these two lines:\n\
{PROJECT_KEY}<the canary token stated in a project CLAUDE.md you were given, or none>\n\
{USER_KEY}<the first markdown heading of user-level instructions from ~/.claude/CLAUDE.md you were given, verbatim, or none>"
  )
}

pub fn user_claude_md_heading(path: &Path) -> Option<String> {
  let text = fs::read_to_string(path).ok()?;
  text
    .lines()
    .find(|line| line.trim_start().starts_with('#'))
    .map(|line| line.trim_start().trim_start_matches('#').trim().to_owned())
}

pub fn run_probe(out_dir: &Path, claude: &ClaudeConfig, user_claude_md: Option<&Path>) -> AgentResult<ProbeReport> {
  fs::create_dir_all(out_dir).map_err(|error| AgentError::io(out_dir, error))?;
  let scratch = scratch_dir(&std::env::temp_dir(), out_dir, "probe");
  let tree = scratch.join("tree");
  if let Err(error) = fs::remove_dir_all(&scratch)
    && error.kind() != std::io::ErrorKind::NotFound
  {
    return Err(AgentError::io(&scratch, error));
  }
  fs::create_dir_all(&tree).map_err(|error| AgentError::io(&tree, error))?;
  check_isolated_location(&tree, &[out_dir])?;
  let report = probe_in(out_dir, &scratch, &tree, claude, user_claude_md);
  if let Err(error) = fs::remove_dir_all(&scratch) {
    tracing::warn!(scratch = %scratch.display(), %error, "cannot remove the probe tree");
  }
  report
}

fn probe_in(out_dir: &Path, scratch: &Path, tree: &Path, claude: &ClaudeConfig, user_claude_md: Option<&Path>) -> AgentResult<ProbeReport> {
  let tree = tree.to_owned();
  let project_md = tree.join("CLAUDE.md");
  fs::write(&project_md, format!("# Probe project\n\nThe project canary token is {PROJECT_CANARY}.\n")).map_err(|error| AgentError::io(&project_md, error))?;
  let mcp_config = scratch.join("empty-mcp.json");
  fs::write(&mcp_config, EMPTY_MCP_CONFIG).map_err(|error| AgentError::io(&mcp_config, error))?;
  let request = SessionRequest {
    prompt: probe_prompt(),
    tree: tree.clone(),
    mcp_config,
    max_turns: PROBE_MAX_TURNS,
    with_atlas_tools: false,
    config_dir: scratch.join("claude-config"),
    project_instructions: None,
    tree_env: Vec::new(),
  };
  let invocation = session_invocation(claude, &request, &host_env());
  let invocation_path = out_dir.join("invocation.json");
  write_json(&invocation_path, &invocation.redacted())?;
  let transcript_path = out_dir.join("transcript.jsonl");
  let output = run_streaming(invocation.command(), PROBE_TIMEOUT, &transcript_path, &out_dir.join("claude.stderr.log")).map_err(|error| AgentError::io(&transcript_path, error))?;
  let text = fs::read_to_string(&transcript_path).map_err(|error| AgentError::io(&transcript_path, error))?;
  let transcript = Transcript::parse(&text, &output.line_offsets_ms);
  let heading = user_claude_md.and_then(user_claude_md_heading);
  let report = analyze_probe(output.exit, &transcript, &read_hello(&tree), heading.as_deref());
  write_json(&out_dir.join("probe.json"), &report)?;
  Ok(report)
}

fn read_hello(tree: &Path) -> Option<String> {
  fs::read_to_string(tree.join(HELLO_FILE)).ok()
}

pub fn analyze_probe(exit: ProcessExit, transcript: &Transcript, hello: &Option<String>, user_heading: Option<&str>) -> ProbeReport {
  let init = transcript.init.clone().unwrap_or_default();
  let answer = transcript
    .result
    .as_ref()
    .and_then(|result| result.text.clone())
    .or_else(|| transcript.assistant_text.last().cloned());
  let answer_text = answer.clone().unwrap_or_default();
  let touched_claude_md = transcript.tool_calls.iter().any(|call| call.input.to_string().contains("CLAUDE.md"));
  let project_claude_md = match (touched_claude_md, answer_value(&answer_text, PROJECT_KEY)) {
    (true, _) => Visibility::Unknown {
      reason: "the session read a CLAUDE.md with a tool".to_owned(),
    },
    (false, None) => Visibility::Unknown {
      reason: "the answer has no PROJECT_CANARY line".to_owned(),
    },
    (false, Some(value)) if value.contains(PROJECT_CANARY) => Visibility::Seen,
    (false, Some(_)) => Visibility::NotSeen,
  };
  let user_claude_md = match (user_heading, touched_claude_md, answer_value(&answer_text, USER_KEY)) {
    (None, _, _) => Visibility::NotApplicable {
      reason: "no user CLAUDE.md heading to look for".to_owned(),
    },
    (Some(_), true, _) => Visibility::Unknown {
      reason: "the session read a CLAUDE.md with a tool".to_owned(),
    },
    (Some(_), false, None) => Visibility::Unknown {
      reason: "the answer has no USER_MD_HEADING line".to_owned(),
    },
    (Some(heading), false, Some(value)) if value.trim_start_matches('#').trim() == heading => Visibility::Seen,
    (Some(_), false, Some(_)) => Visibility::NotSeen,
  };
  let hello_file = match hello {
    None => HelloFile::Missing,
    Some(content) if content.trim() == "hi" => HelloFile::Matches,
    Some(content) => HelloFile::Different { content: content.clone() },
  };
  ProbeReport {
    exit,
    claude_code_version: init.claude_code_version,
    model_reported: transcript.reported_models(),
    permission_mode: init.permission_mode,
    tools: init.tools,
    mcp_servers: init.mcp_servers.into_iter().map(|server| format!("{} ({})", server.name, server.status)).collect(),
    skills: init.skills,
    slash_commands: init.slash_commands,
    plugins: init.plugins,
    agents: init.agents,
    project_claude_md,
    user_claude_md,
    hello_file,
    tool_calls: transcript.tool_calls.iter().map(|call| call.name.clone()).collect(),
    cost_usd: transcript.result.as_ref().and_then(|result| result.total_cost_usd),
    answer,
  }
}

fn answer_value<'a>(answer: &'a str, key: &str) -> Option<&'a str> {
  answer.lines().rev().find_map(|line| line.trim().trim_matches('`').strip_prefix(key)).map(str::trim)
}

pub fn default_user_claude_md() -> Option<PathBuf> {
  std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude").join("CLAUDE.md"))
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::super::transcript::{ResultInfo, ToolCall};
  use super::*;

  const PROBE: &str = include_str!("../../tests/fixtures/probe-transcript.jsonl");

  #[test]
  fn should_report_the_isolation_seen_in_the_recorded_probe() {
    let transcript = Transcript::parse(PROBE, &[]);
    let report = analyze_probe(ProcessExit::Exited { code: Some(0) }, &transcript, &Some("hi".to_owned()), Some("Developer Preferences"));
    assert_eq!(report.project_claude_md, Visibility::NotSeen);
    assert_eq!(report.user_claude_md, Visibility::NotSeen);
    assert_eq!(report.hello_file, HelloFile::Matches);
    assert_eq!(report.mcp_servers, Vec::<String>::new());
    assert_eq!(report.skills, Vec::<String>::new());
    assert_eq!(report.slash_commands, Vec::<String>::new());
    assert_eq!(report.tools, vec!["Bash", "Edit", "Glob", "Grep", "Read", "Write"]);
    assert_eq!(report.permission_mode.as_deref(), Some("bypassPermissions"));
    assert_eq!(report.tool_calls, vec!["Write"]);
  }

  fn answered(answer: &str, calls: Vec<ToolCall>) -> Transcript {
    Transcript {
      tool_calls: calls,
      result: Some(ResultInfo {
        text: Some(answer.to_owned()),
        ..ResultInfo::default()
      }),
      ..Transcript::default()
    }
  }

  #[test]
  fn should_detect_visible_instructions_from_the_answer() {
    let transcript = answered(&format!("done\nPROJECT_CANARY={PROJECT_CANARY}\nUSER_MD_HEADING=# Developer Preferences"), Vec::new());
    let report = analyze_probe(ProcessExit::Exited { code: Some(0) }, &transcript, &None, Some("Developer Preferences"));
    assert_eq!(report.project_claude_md, Visibility::Seen);
    assert_eq!(report.user_claude_md, Visibility::Seen);
    assert_eq!(report.hello_file, HelloFile::Missing);
  }

  #[test]
  fn should_refuse_to_conclude_when_the_session_read_a_claude_md() {
    let read = ToolCall {
      id: "t".to_owned(),
      name: "Read".to_owned(),
      input: json!({"file_path": "/tree/CLAUDE.md"}),
      elapsed_ms: None,
      result: None,
    };
    let transcript = answered(&format!("PROJECT_CANARY={PROJECT_CANARY}\nUSER_MD_HEADING=none"), vec![read]);
    let report = analyze_probe(ProcessExit::Exited { code: Some(0) }, &transcript, &Some("bye".to_owned()), None);
    assert!(matches!(report.project_claude_md, Visibility::Unknown { .. }));
    assert!(matches!(report.user_claude_md, Visibility::NotApplicable { .. }));
    assert_eq!(report.hello_file, HelloFile::Different { content: "bye".to_owned() });
  }

  #[test]
  fn should_read_the_first_heading_of_a_claude_md() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("CLAUDE.md");
    std::fs::write(&path, "intro\n\n## Developer Preferences\n# Other\n").unwrap();
    assert_eq!(user_claude_md_heading(&path).as_deref(), Some("Developer Preferences"));
    assert_eq!(user_claude_md_heading(&dir.path().join("missing.md")), None);
  }
}
