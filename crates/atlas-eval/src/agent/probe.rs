use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::bash_guard::{BASH_TOOL, denied_by_bash_guard};
use super::claude::{ATLAS_TOOL_PREFIX, CORE_ATLAS_TOOLS, ClaudeConfig, EMPTY_MCP_CONFIG, FORCED_TOOLS, HookWiring, SessionRequest, SessionTools, host_env, session_invocation};
use super::error::{AgentError, AgentResult};
use super::hooks::{USER_PROMPT_SUBMIT, hook_metrics};
use super::plan::AtlasSurface;
use super::process::{ProcessExit, run_streaming};
use super::record::write_json;
use super::transcript::{ToolCall, Transcript};
use super::workspace::{check_isolated_location, scratch_dir};

pub const PROJECT_CANARY: &str = "ATLAS-EVAL-PROJECT-CANARY-5821";
const PROBE_TIMEOUT: Duration = Duration::from_secs(300);
const PROBE_MAX_TURNS: u32 = 6;
const HOOKED_PROBE_MAX_TURNS: u32 = 1;
const FORCED_PROBE_MAX_TURNS: u32 = 3;
const FORCED_PROBE_COMMAND: &str = "grep -rn ATLAS-EVAL-GUARD-PROBE .";
const MCP_TOOL_PREFIX: &str = "mcp__";
const EMPTY_MCP_FILE: &str = "empty-mcp.json";
const MCP_FILE: &str = "mcp.json";
const HOOKED_SETTINGS_FILE: &str = "atlas-hooks.json";
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
  #[serde(default)]
  pub hooks: Vec<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub atlas_hooks_seen: Option<Vec<String>>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub atlas_surface_seen: Option<SurfaceSeen>,
  pub project_claude_md: Visibility,
  pub user_claude_md: Visibility,
  pub hello_file: HelloFile,
  pub tool_calls: Vec<String>,
  pub cost_usd: Option<f64>,
  pub answer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceSeen {
  pub builtin_tools: Vec<String>,
  pub atlas_tools: Vec<String>,
  pub bash_commands: Vec<String>,
  pub bash_denied: Vec<String>,
}

impl SurfaceSeen {
  pub fn of(transcript: &Transcript) -> Self {
    let tools = transcript.init.as_ref().map(|init| init.tools.as_slice()).unwrap_or_default();
    let bash: Vec<_> = transcript.tool_calls.iter().filter(|call| call.name == BASH_TOOL).collect();
    let command = |call: &&ToolCall| call.input_str("command").unwrap_or_default().to_owned();
    Self {
      builtin_tools: sorted(tools.iter().filter(|tool| !tool.starts_with(MCP_TOOL_PREFIX)).cloned()),
      atlas_tools: sorted(tools.iter().filter_map(|tool| tool.strip_prefix(ATLAS_TOOL_PREFIX)).map(str::to_owned)),
      bash_commands: bash.iter().map(command).collect(),
      bash_denied: bash.iter().filter(|call| denied_by_bash_guard(call)).map(command).collect(),
    }
  }

  pub fn problem(&self) -> Option<String> {
    let expected_builtin = sorted(FORCED_TOOLS.iter().map(|tool| (*tool).to_owned()));
    if self.builtin_tools != expected_builtin {
      return Some(format!(
        "the forced probe session listed the built-in tools [{}], expected exactly [{}]",
        self.builtin_tools.join(", "),
        expected_builtin.join(", ")
      ));
    }
    let expected_atlas = sorted(CORE_ATLAS_TOOLS.iter().map(|tool| (*tool).to_owned()));
    if self.atlas_tools != expected_atlas {
      return Some(format!(
        "the forced probe session listed the Code Atlas tools [{}], expected exactly the core profile [{}]",
        self.atlas_tools.join(", "),
        expected_atlas.join(", ")
      ));
    }
    if self.bash_denied.is_empty() {
      return Some(format!(
        "no Bash call of the forced probe session came back refused with the guard's reason (Bash calls: [{}]), so it is not shown that Claude Code honours the guard's deny under bypassPermissions",
        self.bash_commands.join("; ")
      ));
    }
    None
  }

  pub fn describe(&self) -> String {
    format!(
      "built-in tools {}; Code Atlas tools {}; Bash refused: {}",
      self.builtin_tools.join(", "),
      self.atlas_tools.join(", "),
      self.bash_denied.join("; ")
    )
  }
}

fn sorted(items: impl Iterator<Item = String>) -> Vec<String> {
  let set: BTreeSet<String> = items.collect();
  set.into_iter().collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookedProbe {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub atlas_hooks_blake3: Option<String>,
  pub exit: ProcessExit,
  pub hooks_seen: Vec<String>,
  pub user_prompt_submit_seen: bool,
  #[serde(default)]
  pub started: bool,
  #[serde(default)]
  pub model_turns: usize,
  pub cost_usd: Option<f64>,
  #[serde(default, skip_serializing_if = "AtlasSurface::is_standard")]
  pub atlas_surface: AtlasSurface,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub surface_seen: Option<SurfaceSeen>,
}

pub struct HookedProbeRequest<'a> {
  pub out_dir: &'a Path,
  pub scratch: &'a Path,
  pub settings: &'a str,
  pub mcp_config: &'a str,
  pub atlas_hooks_blake3: Option<&'a str>,
  pub surface: AtlasSurface,
  pub tree_env: Vec<(String, String)>,
}

pub fn run_hooked_probe(request: &HookedProbeRequest<'_>, claude: &ClaudeConfig) -> AgentResult<HookedProbe> {
  let (out_dir, scratch) = (request.out_dir, request.scratch);
  let tree = scratch.join("tree");
  fs::create_dir_all(&tree).map_err(|error| AgentError::io(&tree, error))?;
  check_isolated_location(&tree, &[out_dir])?;
  let settings = scratch.join(HOOKED_SETTINGS_FILE);
  let (mcp_file, prompt, max_turns) = match request.surface {
    AtlasSurface::Standard => (EMPTY_MCP_FILE, probe_prompt(), HOOKED_PROBE_MAX_TURNS),
    AtlasSurface::Forced => (MCP_FILE, forced_probe_prompt(), FORCED_PROBE_MAX_TURNS),
  };
  let mcp_config = scratch.join(mcp_file);
  let mut files = vec![
    (settings.clone(), request.settings),
    (out_dir.join(HOOKED_SETTINGS_FILE), request.settings),
    (mcp_config.clone(), request.mcp_config),
  ];
  if request.surface == AtlasSurface::Forced {
    files.push((out_dir.join(MCP_FILE), request.mcp_config));
  }
  for (path, content) in files {
    fs::write(&path, content).map_err(|error| AgentError::io(&path, error))?;
  }
  let session = SessionRequest {
    prompt,
    tree,
    mcp_config,
    max_turns,
    tools: SessionTools::Atlas(request.surface),
    config_dir: scratch.join("claude-config"),
    project_instructions: None,
    tree_env: request.tree_env.clone(),
    hooks: HookWiring::Installed { settings },
  };
  let invocation = session_invocation(claude, &session, &host_env());
  write_json(&out_dir.join("invocation.json"), &invocation.redacted())?;
  let transcript_path = out_dir.join("transcript.jsonl");
  let output = run_streaming(invocation.command(), PROBE_TIMEOUT, &transcript_path, &out_dir.join("claude.stderr.log")).map_err(|error| AgentError::io(&transcript_path, error))?;
  let text = fs::read_to_string(&transcript_path).map_err(|error| AgentError::io(&transcript_path, error))?;
  let transcript = Transcript::parse(&text, &output.line_offsets_ms);
  let probe = HookedProbe {
    atlas_hooks_blake3: request.atlas_hooks_blake3.map(str::to_owned),
    exit: output.exit,
    hooks_seen: hooks_seen(&transcript),
    user_prompt_submit_seen: hook_metrics(&transcript).ran(USER_PROMPT_SUBMIT),
    started: transcript.init.is_some(),
    model_turns: transcript.turns.len(),
    cost_usd: transcript.result.as_ref().and_then(|result| result.total_cost_usd),
    atlas_surface: request.surface,
    surface_seen: (request.surface == AtlasSurface::Forced).then(|| SurfaceSeen::of(&transcript)),
  };
  write_json(&out_dir.join("probe.json"), &probe)?;
  Ok(probe)
}

impl ProbeReport {
  pub fn saw_hook(&self, event: &str) -> bool {
    self.hooks.iter().any(|hook| hook.strip_prefix(event).is_some_and(|rest| rest.starts_with(" (")))
  }
}

pub fn probe_prompt() -> String {
  format!(
    "Create a file named {HELLO_FILE} in the current directory containing exactly the text `hi`. \
Then, without using any more tools, answer only from the instructions already in your context and end your reply with exactly these two lines:\n\
{PROJECT_KEY}<the canary token stated in a project CLAUDE.md you were given, or none>\n\
{USER_KEY}<the first markdown heading of user-level instructions from ~/.claude/CLAUDE.md you were given, verbatim, or none>"
  )
}

pub fn forced_probe_prompt() -> String {
  format!(
    "Run exactly this Bash command once, with the Bash tool: `{FORCED_PROBE_COMMAND}`. \
Use no other tool. Then reply with one line saying whether the command ran or was refused, quoting any reason you were given."
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
  let mcp_config = scratch.join(EMPTY_MCP_FILE);
  fs::write(&mcp_config, EMPTY_MCP_CONFIG).map_err(|error| AgentError::io(&mcp_config, error))?;
  let request = SessionRequest {
    prompt: probe_prompt(),
    tree: tree.clone(),
    mcp_config,
    max_turns: PROBE_MAX_TURNS,
    tools: SessionTools::Baseline,
    config_dir: scratch.join("claude-config"),
    project_instructions: None,
    tree_env: Vec::new(),
    hooks: HookWiring::Observed,
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
    hooks: hooks_seen(transcript),
    atlas_hooks_seen: None,
    atlas_surface_seen: None,
    project_claude_md,
    user_claude_md,
    hello_file,
    tool_calls: transcript.tool_calls.iter().map(|call| call.name.clone()).collect(),
    cost_usd: transcript.result.as_ref().and_then(|result| result.total_cost_usd),
    answer,
  }
}

fn hooks_seen(transcript: &Transcript) -> Vec<String> {
  let seen: BTreeSet<String> = transcript.hooks.iter().map(|hook| format!("{} ({})", hook.event, hook.name)).collect();
  seen.into_iter().collect()
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
    assert_eq!(report.hooks, Vec::<String>::new());
    assert!(!report.saw_hook(USER_PROMPT_SUBMIT));
  }

  #[test]
  fn should_list_every_hook_the_session_reported() {
    let synthetic = include_str!("../../tests/fixtures/hooks-transcript.jsonl");
    let report = analyze_probe(ProcessExit::Exited { code: Some(0) }, &Transcript::parse(synthetic, &[]), &None, None);
    assert_eq!(
      report.hooks,
      vec!["PreToolUse (PreToolUse:Bash)", "PreToolUse (PreToolUse:Grep)", "UserPromptSubmit (UserPromptSubmit)"]
    );
    assert!(report.saw_hook(USER_PROMPT_SUBMIT) && report.saw_hook("PreToolUse") && !report.saw_hook("PreTool"));
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
      turn: 1,
    };
    let transcript = answered(&format!("PROJECT_CANARY={PROJECT_CANARY}\nUSER_MD_HEADING=none"), vec![read]);
    let report = analyze_probe(ProcessExit::Exited { code: Some(0) }, &transcript, &Some("bye".to_owned()), None);
    assert!(matches!(report.project_claude_md, Visibility::Unknown { .. }));
    assert!(matches!(report.user_claude_md, Visibility::NotApplicable { .. }));
    assert_eq!(report.hello_file, HelloFile::Different { content: "bye".to_owned() });
  }

  #[test]
  fn should_see_the_forced_surface_and_the_refused_grep_in_a_recorded_session() {
    let seen = SurfaceSeen::of(&Transcript::parse(include_str!("../../tests/fixtures/forced-transcript.jsonl"), &[]));
    assert_eq!(
      seen,
      SurfaceSeen {
        builtin_tools: vec!["Bash", "Edit", "Read", "Write"].into_iter().map(str::to_owned).collect(),
        atlas_tools: vec!["atlas_context", "atlas_edit", "atlas_impact", "atlas_search", "atlas_status", "atlas_symbol"]
          .into_iter()
          .map(str::to_owned)
          .collect(),
        bash_commands: vec!["grep -rn deletedAt src", "cat src/users/user.entity.ts", "bun test | grep fail"]
          .into_iter()
          .map(str::to_owned)
          .collect(),
        bash_denied: vec!["grep -rn deletedAt src", "cat src/users/user.entity.ts"].into_iter().map(str::to_owned).collect(),
      }
    );
    assert_eq!(seen.problem(), None);
  }

  #[test]
  fn should_name_what_the_forced_probe_missed() {
    let standard = SurfaceSeen::of(&Transcript::parse(include_str!("../../tests/fixtures/hooks-transcript.jsonl"), &[]));
    assert_eq!(
      standard.problem().as_deref(),
      Some("the forced probe session listed the built-in tools [Bash, Edit, Glob, Grep, Read, Write], expected exactly [Bash, Edit, Read, Write]")
    );
    let full = SurfaceSeen {
      builtin_tools: sorted(FORCED_TOOLS.iter().map(|tool| (*tool).to_owned())),
      atlas_tools: vec!["atlas_context".to_owned(), "atlas_similar".to_owned()],
      bash_commands: Vec::new(),
      bash_denied: Vec::new(),
    };
    assert_eq!(
      full.problem().as_deref(),
      Some(
        "the forced probe session listed the Code Atlas tools [atlas_context, atlas_similar], expected exactly the core profile [atlas_context, atlas_edit, atlas_impact, atlas_search, atlas_status, atlas_symbol]"
      )
    );
    let allowed = SurfaceSeen {
      atlas_tools: sorted(CORE_ATLAS_TOOLS.iter().map(|tool| (*tool).to_owned())),
      bash_commands: vec!["grep -rn ATLAS-EVAL-GUARD-PROBE .".to_owned()],
      ..full
    };
    assert_eq!(
      allowed.problem().as_deref(),
      Some(
        "no Bash call of the forced probe session came back refused with the guard's reason (Bash calls: [grep -rn ATLAS-EVAL-GUARD-PROBE .]), so it is not shown that Claude Code honours the guard's deny under bypassPermissions"
      )
    );
  }

  #[test]
  fn should_serialize_a_standard_hooked_probe_as_before() {
    let probe = HookedProbe {
      atlas_hooks_blake3: Some("abc".to_owned()),
      exit: ProcessExit::Exited { code: Some(0) },
      hooks_seen: Vec::new(),
      user_prompt_submit_seen: true,
      started: true,
      model_turns: 1,
      cost_usd: None,
      atlas_surface: AtlasSurface::Standard,
      surface_seen: None,
    };
    let json = serde_json::to_value(&probe).unwrap();
    assert_eq!(
      json.as_object().unwrap().keys().collect::<Vec<_>>(),
      vec!["atlas_hooks_blake3", "cost_usd", "exit", "hooks_seen", "model_turns", "started", "user_prompt_submit_seen"]
    );
    assert_eq!(json["atlas_hooks_blake3"], "abc");
    assert_eq!(serde_json::from_value::<HookedProbe>(json).unwrap(), probe);
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
