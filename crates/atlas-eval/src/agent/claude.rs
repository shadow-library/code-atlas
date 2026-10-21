use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::plan::{AtlasSurface, Condition};

pub const DEFAULT_SESSION_MODEL: &str = "claude-sonnet-5-5";
pub const DEFAULT_JUDGE_MODEL: &str = "claude-sonnet-5-5";
pub const ATLAS_SERVER_NAME: &str = "code-atlas";
pub const ATLAS_TOOL_PREFIX: &str = "mcp__code-atlas__";
pub const BASE_TOOLS: [&str; 6] = ["Read", "Edit", "Write", "Glob", "Grep", "Bash"];
pub const FORCED_TOOLS: [&str; 4] = ["Read", "Edit", "Write", "Bash"];
pub const CORE_ATLAS_TOOLS: [&str; 6] = ["atlas_context", "atlas_search", "atlas_symbol", "atlas_impact", "atlas_edit", "atlas_status"];
pub const EMPTY_MCP_CONFIG: &str = "{\"mcpServers\":{}}";
const INCLUDE_HOOK_EVENTS: &str = "--include-hook-events";

const PASSTHROUGH_ENV: [&str; 10] = ["PATH", "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE", "TERM"];
const PASSTHROUGH_ENV_PREFIXES: [&str; 1] = ["ANTHROPIC_"];
const PASSTHROUGH_CLAUDE_ENV: [&str; 3] = ["CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ConfigIsolation {
  User,
  Isolated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeConfig {
  pub binary: PathBuf,
  pub model: String,
  pub judge_model: String,
  pub config_isolation: ConfigIsolation,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HookWiring {
  #[default]
  Unchanged,
  Observed,
  Installed {
    settings: PathBuf,
  },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTools {
  Baseline,
  Atlas(AtlasSurface),
}

impl SessionTools {
  pub fn of(condition: Condition, surface: AtlasSurface) -> Self {
    match condition {
      Condition::Baseline => Self::Baseline,
      Condition::Atlas => Self::Atlas(surface),
    }
  }

  pub fn builtin(self) -> &'static [&'static str] {
    match self {
      Self::Atlas(AtlasSurface::Forced) => &FORCED_TOOLS,
      Self::Baseline | Self::Atlas(AtlasSurface::Standard) => &BASE_TOOLS,
    }
  }

  pub fn allowed(self) -> Vec<String> {
    let mut tools: Vec<String> = self.builtin().iter().map(|tool| (*tool).to_owned()).collect();
    if let Self::Atlas(_) = self {
      tools.push(format!("mcp__{ATLAS_SERVER_NAME}"));
    }
    tools
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRequest {
  pub prompt: String,
  pub tree: PathBuf,
  pub mcp_config: PathBuf,
  pub max_turns: u32,
  pub tools: SessionTools,
  pub config_dir: PathBuf,
  pub project_instructions: Option<String>,
  pub tree_env: Vec<(String, String)>,
  pub hooks: HookWiring,
}

pub const PROJECT_INSTRUCTION_FILES: [&str; 2] = ["CLAUDE.md", "AGENTS.md"];

pub fn project_instructions(tree: &Path) -> Option<(String, Vec<String>)> {
  let found: Vec<(&str, String)> = PROJECT_INSTRUCTION_FILES
    .iter()
    .filter_map(|name| std::fs::read_to_string(tree.join(name)).ok().map(|content| (*name, content)))
    .filter(|(_, content)| !content.trim().is_empty())
    .collect();
  let names = found.iter().map(|(name, _)| (*name).to_owned()).collect();
  format_project_instructions(&found).map(|text| (text, names))
}

pub fn format_project_instructions(files: &[(&str, String)]) -> Option<String> {
  if files.is_empty() {
    return None;
  }
  let mut text = String::from("# Project instructions\n\nThe repository you are working in ships the following instructions for coding agents.\n");
  for (name, content) in files {
    text.push_str(&format!("\n## {name}\n\n{}\n", content.trim_end()));
  }
  Some(text)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
  pub program: PathBuf,
  pub args: Vec<String>,
  pub env: Vec<(String, String)>,
  pub cwd: PathBuf,
}

impl Invocation {
  pub fn command(&self) -> Command {
    let mut command = Command::new(&self.program);
    command
      .args(&self.args)
      .current_dir(&self.cwd)
      .env_clear()
      .envs(self.env.iter().map(|(key, value)| (key, value)));
    command
  }

  pub fn env_names(&self) -> Vec<String> {
    self.env.iter().map(|(key, _)| key.clone()).collect()
  }

  pub fn redacted(&self) -> RedactedInvocation {
    RedactedInvocation {
      program: self.program.clone(),
      args: self.args.clone(),
      env_names: self.env_names(),
      cwd: self.cwd.clone(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedInvocation {
  pub program: PathBuf,
  pub args: Vec<String>,
  pub env_names: Vec<String>,
  pub cwd: PathBuf,
}

pub fn session_invocation(config: &ClaudeConfig, request: &SessionRequest, host_env: &[(String, String)]) -> Invocation {
  let mut args = isolation_args(&config.model, &request.mcp_config);
  args.extend(strings(&["--output-format", "stream-json", "--verbose"]));
  match &request.hooks {
    HookWiring::Unchanged => {}
    HookWiring::Observed => args.push(INCLUDE_HOOK_EVENTS.to_owned()),
    HookWiring::Installed { settings } => args.extend([INCLUDE_HOOK_EVENTS.to_owned(), "--settings".to_owned(), settings.display().to_string()]),
  }
  args.extend(strings(&["--permission-mode", "bypassPermissions", "--allow-dangerously-skip-permissions"]));
  args.push("--tools".to_owned());
  args.push(request.tools.builtin().join(","));
  args.push("--allowedTools".to_owned());
  args.push(request.tools.allowed().join(","));
  args.push("--add-dir".to_owned());
  args.push(request.tree.display().to_string());
  args.push("--max-turns".to_owned());
  args.push(request.max_turns.to_string());
  if let Some(instructions) = &request.project_instructions {
    args.push("--append-system-prompt".to_owned());
    args.push(instructions.clone());
  }
  args.push("-p".to_owned());
  args.push(request.prompt.clone());
  let mut env = session_env(config.config_isolation, &request.config_dir, host_env);
  env.extend(request.tree_env.iter().cloned());
  Invocation {
    program: config.binary.clone(),
    args,
    env,
    cwd: request.tree.clone(),
  }
}

pub fn judge_invocation(config: &ClaudeConfig, mcp_config: &Path, schema: &str, cwd: &Path, config_dir: &Path, host_env: &[(String, String)]) -> Invocation {
  let mut args = isolation_args(&config.judge_model, mcp_config);
  args.extend(strings(&["--output-format", "json", "--tools", "", "--json-schema"]));
  args.push(schema.to_owned());
  args.push("-p".to_owned());
  Invocation {
    program: config.binary.clone(),
    args,
    env: session_env(config.config_isolation, config_dir, host_env),
    cwd: cwd.to_owned(),
  }
}

fn isolation_args(model: &str, mcp_config: &Path) -> Vec<String> {
  let mut args = strings(&["--model", model, "--strict-mcp-config", "--mcp-config"]);
  args.push(mcp_config.display().to_string());
  args.extend(strings(&["--setting-sources", "", "--disable-slash-commands", "--no-session-persistence"]));
  args
}

pub fn session_env(isolation: ConfigIsolation, config_dir: &Path, host_env: &[(String, String)]) -> Vec<(String, String)> {
  let mut env: Vec<(String, String)> = host_env.iter().filter(|(key, _)| is_passthrough(key)).cloned().collect();
  env.sort();
  env.push(("CLAUDE_CODE_DISABLE_AUTO_MEMORY".to_owned(), "1".to_owned()));
  env.push(("DISABLE_AUTOUPDATER".to_owned(), "1".to_owned()));
  env.push(("ENABLE_TOOL_SEARCH".to_owned(), "false".to_owned()));
  if isolation == ConfigIsolation::Isolated {
    env.push(("CLAUDE_CONFIG_DIR".to_owned(), config_dir.display().to_string()));
  }
  env
}

fn is_passthrough(key: &str) -> bool {
  PASSTHROUGH_ENV.contains(&key) || PASSTHROUGH_CLAUDE_ENV.contains(&key) || PASSTHROUGH_ENV_PREFIXES.iter().any(|prefix| key.starts_with(prefix))
}

pub fn host_env() -> Vec<(String, String)> {
  std::env::vars().collect()
}

fn strings(values: &[&str]) -> Vec<String> {
  values.iter().map(|value| (*value).to_owned()).collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn config(isolation: ConfigIsolation) -> ClaudeConfig {
    ClaudeConfig {
      binary: PathBuf::from("claude"),
      model: "claude-haiku-4-5-20251001".to_owned(),
      judge_model: DEFAULT_JUDGE_MODEL.to_owned(),
      config_isolation: isolation,
    }
  }

  fn request(with_atlas_tools: bool) -> SessionRequest {
    SessionRequest {
      prompt: "do it".to_owned(),
      tree: PathBuf::from("/runs/r1/tree"),
      mcp_config: PathBuf::from("/runs/r1/mcp.json"),
      max_turns: 30,
      tools: if with_atlas_tools {
        SessionTools::Atlas(AtlasSurface::Standard)
      } else {
        SessionTools::Baseline
      },
      config_dir: PathBuf::from("/runs/r1/claude-config"),
      project_instructions: None,
      tree_env: Vec::new(),
      hooks: HookWiring::Unchanged,
    }
  }

  fn host() -> Vec<(String, String)> {
    [
      ("PATH", "/bin"),
      ("HOME", "/home/u"),
      ("CLAUDECODE", "1"),
      ("CLAUDE_CODE_ENTRYPOINT", "cli"),
      ("ANTHROPIC_API_KEY", "k"),
      ("SECRET_TOKEN", "s"),
    ]
    .iter()
    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
    .collect()
  }

  #[test]
  fn should_build_the_exact_baseline_flag_set() {
    let invocation = session_invocation(&config(ConfigIsolation::User), &request(false), &host());
    assert_eq!(
      invocation.args,
      vec![
        "--model",
        "claude-haiku-4-5-20251001",
        "--strict-mcp-config",
        "--mcp-config",
        "/runs/r1/mcp.json",
        "--setting-sources",
        "",
        "--disable-slash-commands",
        "--no-session-persistence",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
        "--allow-dangerously-skip-permissions",
        "--tools",
        "Read,Edit,Write,Glob,Grep,Bash",
        "--allowedTools",
        "Read,Edit,Write,Glob,Grep,Bash",
        "--add-dir",
        "/runs/r1/tree",
        "--max-turns",
        "30",
        "-p",
        "do it",
      ]
    );
    assert_eq!(invocation.cwd, PathBuf::from("/runs/r1/tree"));
  }

  #[test]
  fn should_differ_between_conditions_only_in_the_atlas_permission_and_preamble() {
    let instructions = format_project_instructions(&[("CLAUDE.md", "Use 2 spaces.".to_owned())]);
    let with = |with_atlas_tools: bool, prompt: &str| SessionRequest {
      prompt: prompt.to_owned(),
      project_instructions: instructions.clone(),
      tree_env: vec![("CARGO_TARGET_DIR".to_owned(), "/runs/r1/cargo-target".to_owned())],
      ..request(with_atlas_tools)
    };
    let baseline = session_invocation(&config(ConfigIsolation::User), &with(false, "Fix it."), &host());
    let atlas = session_invocation(&config(ConfigIsolation::User), &with(true, "PREAMBLE\n\nFix it."), &host());
    let differing: Vec<(&String, &String)> = baseline.args.iter().zip(&atlas.args).filter(|(left, right)| left != right).collect();
    assert_eq!(baseline.args.len(), atlas.args.len());
    assert_eq!(
      differing,
      vec![
        (&"Read,Edit,Write,Glob,Grep,Bash".to_owned(), &"Read,Edit,Write,Glob,Grep,Bash,mcp__code-atlas".to_owned()),
        (&"Fix it.".to_owned(), &"PREAMBLE\n\nFix it.".to_owned()),
      ]
    );
    let position = baseline.args.iter().position(|arg| arg == "--append-system-prompt").unwrap();
    assert_eq!(baseline.args[position + 1], atlas.args[position + 1]);
    assert_eq!(baseline.env, atlas.env);
    assert_eq!(baseline.env.last().unwrap(), &("CARGO_TARGET_DIR".to_owned(), "/runs/r1/cargo-target".to_owned()));
  }

  #[test]
  fn should_add_only_the_hook_settings_and_hook_events_to_a_session_with_installed_hooks() {
    let plain = session_invocation(&config(ConfigIsolation::User), &request(true), &host());
    let installed = SessionRequest {
      hooks: HookWiring::Installed {
        settings: PathBuf::from("/runs/r1/atlas-hooks.json"),
      },
      ..request(true)
    };
    let hooked = session_invocation(&config(ConfigIsolation::User), &installed, &host());
    let position = hooked.args.iter().position(|arg| arg == "--verbose").unwrap() + 1;
    assert_eq!(
      &hooked.args[position..position + 3],
      &["--include-hook-events", "--settings", "/runs/r1/atlas-hooks.json"].map(str::to_owned)
    );
    let mut without = hooked.args.clone();
    without.drain(position..position + 3);
    assert_eq!(without, plain.args);
    assert_eq!(hooked.env, plain.env);
    let observed = session_invocation(
      &config(ConfigIsolation::User),
      &SessionRequest {
        hooks: HookWiring::Observed,
        ..request(false)
      },
      &host(),
    );
    assert_eq!(observed.args[position], "--include-hook-events");
    assert_eq!(observed.args.len(), plain.args.len() + 1);
  }

  #[test]
  fn should_narrow_only_the_tool_flags_of_a_forced_atlas_session() {
    let forced = SessionRequest {
      tools: SessionTools::Atlas(AtlasSurface::Forced),
      hooks: HookWiring::Installed {
        settings: PathBuf::from("/runs/r1/atlas-hooks.json"),
      },
      ..request(true)
    };
    let invocation = session_invocation(&config(ConfigIsolation::User), &forced, &host());
    assert_eq!(
      invocation.args,
      vec![
        "--model",
        "claude-haiku-4-5-20251001",
        "--strict-mcp-config",
        "--mcp-config",
        "/runs/r1/mcp.json",
        "--setting-sources",
        "",
        "--disable-slash-commands",
        "--no-session-persistence",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-hook-events",
        "--settings",
        "/runs/r1/atlas-hooks.json",
        "--permission-mode",
        "bypassPermissions",
        "--allow-dangerously-skip-permissions",
        "--tools",
        "Read,Edit,Write,Bash",
        "--allowedTools",
        "Read,Edit,Write,Bash,mcp__code-atlas",
        "--add-dir",
        "/runs/r1/tree",
        "--max-turns",
        "30",
        "-p",
        "do it",
      ]
    );
    let standard = SessionRequest {
      tools: SessionTools::Atlas(AtlasSurface::Standard),
      ..forced.clone()
    };
    let standard = session_invocation(&config(ConfigIsolation::User), &standard, &host());
    let differing: Vec<(&String, &String)> = standard.args.iter().zip(&invocation.args).filter(|(left, right)| left != right).collect();
    assert_eq!(
      differing,
      vec![
        (&"Read,Edit,Write,Glob,Grep,Bash".to_owned(), &"Read,Edit,Write,Bash".to_owned()),
        (
          &"Read,Edit,Write,Glob,Grep,Bash,mcp__code-atlas".to_owned(),
          &"Read,Edit,Write,Bash,mcp__code-atlas".to_owned()
        ),
      ]
    );
    assert_eq!(standard.env, invocation.env);
  }

  #[test]
  fn should_give_baseline_sessions_the_baseline_tools_whatever_the_atlas_surface() {
    assert_eq!(SessionTools::of(Condition::Baseline, AtlasSurface::Forced), SessionTools::Baseline);
    assert_eq!(SessionTools::of(Condition::Atlas, AtlasSurface::Forced), SessionTools::Atlas(AtlasSurface::Forced));
    assert_eq!(SessionTools::Baseline.allowed(), BASE_TOOLS.map(str::to_owned).to_vec());
  }

  #[test]
  fn should_label_injected_project_instructions_by_file() {
    let text = format_project_instructions(&[("CLAUDE.md", "a\n".to_owned()), ("AGENTS.md", "b".to_owned())]).unwrap();
    assert_eq!(
      text,
      "# Project instructions\n\nThe repository you are working in ships the following instructions for coding agents.\n\n## CLAUDE.md\n\na\n\n## AGENTS.md\n\nb\n"
    );
    assert_eq!(format_project_instructions(&[]), None);
  }

  #[test]
  fn should_read_only_non_empty_instruction_files_from_the_tree_root() {
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(tree.path().join("AGENTS.md"), "agents").unwrap();
    std::fs::write(tree.path().join("CLAUDE.md"), "  \n").unwrap();
    let (text, names) = project_instructions(tree.path()).unwrap();
    assert_eq!(names, vec!["AGENTS.md"]);
    assert!(text.ends_with("## AGENTS.md\n\nagents\n"));
    assert_eq!(project_instructions(&tree.path().join("missing")), None);
  }

  #[test]
  fn should_pass_only_allowlisted_environment_and_the_fixed_overrides() {
    let invocation = session_invocation(&config(ConfigIsolation::User), &request(false), &host());
    assert_eq!(
      invocation.env_names(),
      vec![
        "ANTHROPIC_API_KEY",
        "HOME",
        "PATH",
        "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
        "DISABLE_AUTOUPDATER",
        "ENABLE_TOOL_SEARCH"
      ]
    );
  }

  #[test]
  fn should_point_claude_at_a_private_config_dir_when_isolated() {
    let invocation = session_invocation(&config(ConfigIsolation::Isolated), &request(false), &host());
    assert_eq!(invocation.env.last().unwrap(), &("CLAUDE_CONFIG_DIR".to_owned(), "/runs/r1/claude-config".to_owned()));
  }

  #[test]
  fn should_build_a_tool_less_judge_invocation_that_reads_the_prompt_from_stdin() {
    let invocation = judge_invocation(&config(ConfigIsolation::User), Path::new("/e.json"), "{}", Path::new("/w"), Path::new("/c"), &host());
    assert_eq!(&invocation.args[..2], &["--model".to_owned(), DEFAULT_JUDGE_MODEL.to_owned()]);
    assert_eq!(
      &invocation.args[9..],
      &["--output-format", "json", "--tools", "", "--json-schema", "{}", "-p"].map(str::to_owned)
    );
  }
}
