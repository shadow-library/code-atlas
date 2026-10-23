use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::bash_guard::shell_quote;
use super::claude::{SERENA_SERVER_NAME, SERENA_TOOL_PREFIX};
use super::error::{AgentError, AgentResult};
use super::process::{run_captured, tail};
use super::transcript::ToolResult;

pub const INCUMBENT_INSTRUCTIONS_VERSION: u32 = 2;
pub const GRAPHIFY_SECTION: &str = "## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

Rules:
- For codebase questions, first run `graphify query \"<question>\"` when graphify-out/graph.json exists. Use `graphify path \"<A>\" \"<B>\"` for relationships and `graphify explain \"<concept>\"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).";
pub const LINKED_PARAGRAPH: &str = "graphify-out/graph.json also covers the linked repository `{repo}`, checked out read-only at `{view}/`: its nodes are prefixed `{repo}::` and their source paths are relative to that checkout. Its graph alone is graphify-out/linked-{repo}.json (pass it with `--graph`). `graphify update .` rebuilds only this repository, so after it graph.json no longer covers `{repo}`; query graphify-out/linked-{repo}.json for it.";

pub const SERENA_CONFIG: &str = "language_backend: LSP
line_ending: native
gui_log_window: false
web_dashboard: false
web_dashboard_open_on_launch: false
web_dashboard_interface:
web_dashboard_listen_address: 127.0.0.1
web_dashboard_trusted_hosts:
  - 127.0.0.1
  - localhost
jetbrains_plugin_server_address: 127.0.0.1
jetbrains_launch_command:
log_level: 20
trace_lsp_communication: false
ls_specific_settings: {}
ignored_paths: []
read_only_memory_patterns: []
ignored_memory_patterns: []
tool_timeout: 240
excluded_tools: []
included_optional_tools: []
fixed_tools: []
base_modes:
  - interactive
  - editing
default_modes:
default_max_tool_answer_chars: 150000
token_count_estimator: CHAR_COUNT
symbol_info_budget: 10
project_serena_folder_location: \"$projectDir/.serena\"
trusted_project_path_patterns: []
projects: []
";

pub const SERENA_HOME_DIR: &str = "serena-home";
pub const SERENA_CONFIG_FILE: &str = "serena_config.yml";
pub const SERENA_LANGUAGE_SERVERS: &str = "language_servers";
pub const INCUMBENT_HOOKS_FILE: &str = "incumbent-hooks.json";
pub const SERENA_HOME_VAR: &str = "SERENA_HOME";
pub const DEFAULT_SERENA_BIN: &str = "serena";
pub const DEFAULT_SERENA_HOOKS_BIN: &str = "serena-hooks";
pub const DEFAULT_GRAPHIFY_BIN: &str = "graphify";
pub const SERENA_EDIT_TOOLS: [&str; 7] = [
  "replace_symbol_body",
  "insert_after_symbol",
  "insert_before_symbol",
  "rename_symbol",
  "replace_content",
  "replace_in_files",
  "safe_delete_symbol",
];
const LANGUAGE_SERVER_CACHE: &str = "serena-language-servers";
const SERENA_MCP_ARGS: [&str; 3] = ["start-mcp-server", "--context=claude-code", "--project-from-cwd"];
const HOOK_CLIENT: &str = "--client=claude-code";
const SESSION_START: &str = "SessionStart";
const PRE_TOOL_USE: &str = "PreToolUse";
const SESSION_END: &str = "SessionEnd";
const VERSION_TIMEOUT: Duration = Duration::from_secs(60);
const WARM_UP_PROJECT: &str = "serena-warm-up";
const WARM_UP_HOME: &str = "home";
// Without an explicit language, Serena's project auto-creation asks on stdin which further languages to enable.
const WARM_UP_LANGUAGE: &str = "typescript";
const WARM_UP_SOURCE: (&str, &str) = ("probe.ts", "export function probe(): string {\n  return \"ATLAS-EVAL-SERENA-WARM-UP\";\n}\n");
const GRAPHIFY_LINK_DIR: &str = "incumbent-bin";
const HOOK_DENIAL_MARKERS: [&str; 2] = ["Too many consecutive", "symbolic tool"];
const OUTPUT_TAIL_BYTES: usize = 2000;

pub fn incumbent_instructions_digest() -> String {
  blake3::hash(format!("{GRAPHIFY_SECTION}\n\n{LINKED_PARAGRAPH}").as_bytes()).to_hex().to_string()
}

pub fn linked_graph_file(repo: &str) -> String {
  format!("linked-{repo}.json")
}

pub fn incumbent_instructions(linked: Option<(&str, &Path)>) -> String {
  match linked {
    None => GRAPHIFY_SECTION.to_owned(),
    Some((repo, view)) => format!(
      "{GRAPHIFY_SECTION}\n\n{}",
      LINKED_PARAGRAPH.replace("{repo}", repo).replace("{view}", &view.display().to_string())
    ),
  }
}

pub fn serena_edit(call_name: &str) -> bool {
  call_name.strip_prefix(SERENA_TOOL_PREFIX).is_some_and(|tool| SERENA_EDIT_TOOLS.contains(&tool))
}

pub fn denied_by_serena_hook(result: &ToolResult) -> bool {
  result.is_error && HOOK_DENIAL_MARKERS.iter().all(|marker| result.text.contains(marker))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncumbentRequest {
  pub serena: PathBuf,
  pub serena_hooks: PathBuf,
  pub graphify: PathBuf,
  pub home_template: Option<PathBuf>,
}

impl Default for IncumbentRequest {
  fn default() -> Self {
    Self {
      serena: PathBuf::from(DEFAULT_SERENA_BIN),
      serena_hooks: PathBuf::from(DEFAULT_SERENA_HOOKS_BIN),
      graphify: PathBuf::from(DEFAULT_GRAPHIFY_BIN),
      home_template: None,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncumbentRecord {
  pub serena_bin: String,
  pub serena_version: Option<String>,
  pub serena_hooks_bin: String,
  pub graphify_bin: String,
  pub graphify_version: Option<String>,
  pub language_servers: String,
  pub instructions_version: u32,
  pub instructions_blake3: String,
}

impl IncumbentRecord {
  pub fn setup(&self) -> String {
    IncumbentRun {
      serena_version: self.serena_version.clone(),
      graphify_version: self.graphify_version.clone(),
      instructions_blake3: self.instructions_blake3.clone(),
      ..IncumbentRun::default()
    }
    .setup()
  }

  pub fn describe(&self) -> String {
    let version = |version: &Option<String>| version.clone().unwrap_or_else(|| "unknown version".to_owned());
    format!(
      "{} ({}), {} ({}), instructions version {} (blake3 {})",
      self.serena_bin,
      version(&self.serena_version),
      self.graphify_bin,
      version(&self.graphify_version),
      self.instructions_version,
      self.instructions_blake3
    )
  }
}

impl IncumbentRun {
  pub fn setup(&self) -> String {
    let version = |version: &Option<String>| version.clone().unwrap_or_else(|| "unknown".to_owned());
    format!(
      "{}, {}, instructions {}",
      version(&self.serena_version),
      version(&self.graphify_version),
      self.instructions_blake3
    )
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncumbentRun {
  pub serena_version: Option<String>,
  pub graphify_version: Option<String>,
  pub graph_key: String,
  pub graph_roots: Vec<String>,
  pub graph_build_ms: u64,
  pub graph_nodes: u64,
  pub graph_edges: u64,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub excluded: Vec<String>,
  pub instructions_blake3: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncumbentTools {
  pub serena: PathBuf,
  pub serena_version: Option<String>,
  pub serena_hooks: PathBuf,
  pub graphify: PathBuf,
  pub graphify_version: Option<String>,
  pub language_servers: PathBuf,
  pub timeout: Duration,
}

impl IncumbentTools {
  pub fn setup(&self) -> String {
    IncumbentRun {
      serena_version: self.serena_version.clone(),
      graphify_version: self.graphify_version.clone(),
      instructions_blake3: incumbent_instructions_digest(),
      ..IncumbentRun::default()
    }
    .setup()
  }

  pub fn resolve(request: &IncumbentRequest, path_var: Option<&OsStr>, cache_dir: &Path, timeout: Duration) -> AgentResult<Self> {
    let language_servers = match &request.home_template {
      Some(template) => template.join(SERENA_LANGUAGE_SERVERS),
      None => cache_dir.join(LANGUAGE_SERVER_CACHE),
    };
    let serena = resolve_binary(&request.serena, path_var)?;
    let graphify = resolve_binary(&request.graphify, path_var)?;
    Ok(Self {
      serena_version: version_of(&serena),
      graphify_version: version_of(&graphify),
      serena,
      serena_hooks: resolve_binary(&request.serena_hooks, path_var)?,
      graphify,
      language_servers,
      timeout,
    })
  }

  pub fn record(&self) -> IncumbentRecord {
    IncumbentRecord {
      serena_bin: self.serena.display().to_string(),
      serena_version: self.serena_version.clone(),
      serena_hooks_bin: self.serena_hooks.display().to_string(),
      graphify_bin: self.graphify.display().to_string(),
      graphify_version: self.graphify_version.clone(),
      language_servers: self.language_servers.display().to_string(),
      instructions_version: INCUMBENT_INSTRUCTIONS_VERSION,
      instructions_blake3: incumbent_instructions_digest(),
    }
  }

  pub fn probe_key(&self) -> String {
    let fields = [
      self.serena.display().to_string(),
      self.serena_version.clone().unwrap_or_default(),
      self.serena_hooks.display().to_string(),
      self.graphify.display().to_string(),
      self.graphify_version.clone().unwrap_or_default(),
      incumbent_instructions_digest(),
      SERENA_CONFIG.to_owned(),
    ];
    blake3::hash(fields.join("\0").as_bytes()).to_hex().to_string()
  }

  pub fn serena_home(&self, scratch: &Path) -> AgentResult<PathBuf> {
    let home = scratch.join(SERENA_HOME_DIR);
    fs::create_dir_all(&home).map_err(|error| AgentError::io(&home, error))?;
    fs::create_dir_all(&self.language_servers).map_err(|error| AgentError::io(&self.language_servers, error))?;
    let config = home.join(SERENA_CONFIG_FILE);
    fs::write(&config, SERENA_CONFIG).map_err(|error| AgentError::io(&config, error))?;
    let link = home.join(SERENA_LANGUAGE_SERVERS);
    symlink(&self.language_servers, &link)?;
    Ok(home)
  }

  pub fn mcp_config(&self, serena_home: &Path) -> Value {
    json!({
      "mcpServers": {
        SERENA_SERVER_NAME: {
          "type": "stdio",
          "command": self.serena.display().to_string(),
          "args": SERENA_MCP_ARGS,
          "env": { SERENA_HOME_VAR: serena_home.display().to_string() }
        }
      }
    })
  }

  pub fn hook_settings(&self, serena_home: &Path) -> Value {
    let command = |hook: &str| {
      json!({
        "type": "command",
        "command": format!(
          "{SERENA_HOME_VAR}={} {} {hook} {HOOK_CLIENT}",
          shell_quote(&serena_home.display().to_string()),
          shell_quote(&self.serena_hooks.display().to_string())
        )
      })
    };
    json!({
      "hooks": {
        SESSION_START: [{ "hooks": [command("activate")] }],
        PRE_TOOL_USE: [
          { "hooks": [command("remind")] },
          { "matcher": format!("{SERENA_TOOL_PREFIX}*"), "hooks": [command("auto-approve")] }
        ],
        SESSION_END: [{ "hooks": [command("cleanup")] }]
      }
    })
  }

  pub fn session_path(&self, scratch: &Path, host_path: Option<&OsStr>) -> AgentResult<Option<String>> {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let found = resolve_binary(Path::new(DEFAULT_GRAPHIFY_BIN), host_path).ok();
    if found.as_deref().map(canonical) == Some(canonical(&self.graphify)) {
      return Ok(None);
    }
    let dir = scratch.join(GRAPHIFY_LINK_DIR);
    fs::create_dir_all(&dir).map_err(|error| AgentError::io(&dir, error))?;
    let link = dir.join(DEFAULT_GRAPHIFY_BIN);
    if fs::symlink_metadata(&link).is_ok() {
      fs::remove_file(&link).map_err(|error| AgentError::io(&link, error))?;
    }
    symlink(&self.graphify, &link)?;
    let mut dirs = vec![dir];
    dirs.extend(host_path.map(std::env::split_paths).into_iter().flatten());
    let joined = std::env::join_paths(dirs).map_err(|error| AgentError::IncumbentUnavailable {
      reason: format!("cannot put the graphify link first on PATH: {error}"),
    })?;
    Ok(Some(joined.to_string_lossy().into_owned()))
  }

  pub fn warm_up(&self, scratch: &Path) -> AgentResult<()> {
    let _lock = cache_lock(&self.language_servers)?;
    let home = self.serena_home(scratch)?;
    let project = scratch.join(WARM_UP_PROJECT);
    let user_home = scratch.join(WARM_UP_HOME);
    for dir in [&project, &user_home] {
      fs::create_dir_all(dir).map_err(|error| AgentError::io(dir, error))?;
    }
    let (name, content) = WARM_UP_SOURCE;
    fs::write(project.join(name), content).map_err(|error| AgentError::io(project.join(name), error))?;
    let mut command = Command::new(&self.serena);
    command
      .args(["project", "index"])
      .arg(&project)
      .args(["--language", WARM_UP_LANGUAGE, "--log-level", "ERROR"])
      .current_dir(&project)
      .env(SERENA_HOME_VAR, &home)
      .env("HOME", &user_home);
    let output = run_captured(command, None, self.timeout);
    if output.exit.succeeded() {
      return Ok(());
    }
    Err(AgentError::IncumbentUnavailable {
      reason: format!(
        "`serena project index` on a one-file TypeScript project failed ({:?}), so Serena cannot start its language server: {}",
        output.exit,
        tail(&format!("{}{}", output.stdout, output.stderr), OUTPUT_TAIL_BYTES)
      ),
    })
  }
}

pub fn cache_lock(dir: &Path) -> AgentResult<File> {
  let name = dir
    .file_name()
    .map(|name| format!("{}.lock", name.to_string_lossy()))
    .unwrap_or_else(|| "cache.lock".to_owned());
  let path = dir.with_file_name(name);
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).map_err(|error| AgentError::io(parent, error))?;
  }
  let file = File::create(&path).map_err(|error| AgentError::io(&path, error))?;
  file.lock().map_err(|error| AgentError::io(&path, error))?;
  Ok(file)
}

pub fn resolve_binary(requested: &Path, path_var: Option<&OsStr>) -> AgentResult<PathBuf> {
  let unavailable = |reason: String| AgentError::IncumbentUnavailable { reason };
  if requested.components().count() > 1 {
    return requested
      .canonicalize()
      .ok()
      .filter(|path| path.is_file())
      .ok_or_else(|| unavailable(format!("{} is not a file", requested.display())));
  }
  path_var
    .map(std::env::split_paths)
    .into_iter()
    .flatten()
    .map(|dir| dir.join(requested))
    .find(|candidate| candidate.is_file())
    .map(|found| std::path::absolute(&found).unwrap_or(found))
    .ok_or_else(|| unavailable(format!("`{}` is not on PATH", requested.display())))
}

fn version_of(binary: &Path) -> Option<String> {
  let sandbox = tempfile::tempdir().ok()?;
  let mut command = Command::new(binary);
  command
    .arg("--version")
    .env(SERENA_HOME_VAR, sandbox.path().join(SERENA_HOME_DIR))
    .env("HOME", sandbox.path());
  let output = run_captured(command, None, VERSION_TIMEOUT);
  let printed = output.stdout.lines().rev().find(|line| !line.trim().is_empty())?.trim().to_owned();
  output.exit.succeeded().then_some(printed)
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> AgentResult<()> {
  std::os::unix::fs::symlink(target, link).map_err(|error| AgentError::io(link, error))
}

#[cfg(not(unix))]
fn symlink(target: &Path, link: &Path) -> AgentResult<()> {
  std::os::windows::fs::symlink_dir(target, link).map_err(|error| AgentError::io(link, error))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn tools() -> IncumbentTools {
    IncumbentTools {
      serena: PathBuf::from("/opt/serena/bin/serena"),
      serena_version: Some("Serena 1.6.1".to_owned()),
      serena_hooks: PathBuf::from("/opt/serena/bin/serena-hooks"),
      graphify: PathBuf::from("/opt/graphify/bin/graphify"),
      graphify_version: Some("graphify 0.8.26".to_owned()),
      language_servers: PathBuf::from("/eval/runs/.cache/serena-language-servers"),
      timeout: Duration::from_secs(5),
    }
  }

  #[test]
  fn should_render_the_serena_server_with_the_run_home_as_the_only_mcp_server() {
    assert_eq!(
      tools().mcp_config(Path::new("/tmp/ae-1/serena-home")),
      json!({"mcpServers": {"serena": {
        "type": "stdio",
        "command": "/opt/serena/bin/serena",
        "args": ["start-mcp-server", "--context=claude-code", "--project-from-cwd"],
        "env": {"SERENA_HOME": "/tmp/ae-1/serena-home"}
      }}})
    );
  }

  #[test]
  fn should_install_the_four_serena_hooks_with_the_run_home_in_each_command() {
    let command = |hook: &str| json!({"type": "command", "command": format!("SERENA_HOME='/tmp/ae 1/serena-home' /opt/serena/bin/serena-hooks {hook} --client=claude-code")});
    assert_eq!(
      tools().hook_settings(Path::new("/tmp/ae 1/serena-home")),
      json!({"hooks": {
        "SessionStart": [{"hooks": [command("activate")]}],
        "PreToolUse": [{"hooks": [command("remind")]}, {"matcher": "mcp__serena__*", "hooks": [command("auto-approve")]}],
        "SessionEnd": [{"hooks": [command("cleanup")]}]
      }})
    );
  }

  #[test]
  fn should_append_the_linked_paragraph_only_for_a_linked_task() {
    assert_eq!(incumbent_instructions(None), GRAPHIFY_SECTION);
    assert_eq!(
      incumbent_instructions(Some(("web", Path::new("/tmp/ae-1/linked-view")))),
      format!(
        "{GRAPHIFY_SECTION}\n\ngraphify-out/graph.json also covers the linked repository `web`, checked out read-only at `/tmp/ae-1/linked-view/`: its nodes are prefixed `web::` and their source paths are relative to that checkout. Its graph alone is graphify-out/linked-web.json (pass it with `--graph`). `graphify update .` rebuilds only this repository, so after it graph.json no longer covers `web`; query graphify-out/linked-web.json for it."
      )
    );
    assert!(GRAPHIFY_SECTION.starts_with("## graphify\n\nThis project has a knowledge graph at graphify-out/"));
    assert!(GRAPHIFY_SECTION.ends_with("- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost)."));
    assert_eq!(GRAPHIFY_SECTION.lines().count(), 9);
    assert_eq!(INCUMBENT_INSTRUCTIONS_VERSION, 2);
  }

  #[test]
  fn should_lay_out_a_serena_home_under_the_scratch_directory_only() {
    let scratch = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let tools = IncumbentTools {
      language_servers: cache.path().join("serena-language-servers"),
      ..tools()
    };
    let home = tools.serena_home(scratch.path()).unwrap();
    assert_eq!(home, scratch.path().join("serena-home"));
    assert_eq!(fs::read_to_string(home.join("serena_config.yml")).unwrap(), SERENA_CONFIG);
    assert_eq!(fs::read_link(home.join("language_servers")).unwrap(), cache.path().join("serena-language-servers"));
    assert!(cache.path().join("serena-language-servers").is_dir());
    let entries: Vec<String> = fs::read_dir(scratch.path())
      .unwrap()
      .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
      .collect();
    assert_eq!(entries, vec!["serena-home"]);
    for (key, value) in [
      ("web_dashboard", "false"),
      ("web_dashboard_open_on_launch", "false"),
      ("gui_log_window", "false"),
      ("projects", "[]"),
    ] {
      assert!(SERENA_CONFIG.lines().any(|line| line == format!("{key}: {value}")), "{key}");
    }
  }

  #[test]
  fn should_take_the_language_server_cache_from_the_home_template_when_given() {
    let bin = tempfile::tempdir().unwrap();
    for name in ["serena", "serena-hooks", "graphify"] {
      fs::write(bin.path().join(name), "").unwrap();
    }
    let path = std::env::join_paths([bin.path()]).unwrap();
    let request = IncumbentRequest {
      home_template: Some(PathBuf::from("/templates/serena")),
      ..IncumbentRequest::default()
    };
    let resolved = IncumbentTools::resolve(&request, Some(&path), Path::new("/eval/runs/.cache"), Duration::from_secs(1)).unwrap();
    assert_eq!(resolved.language_servers, PathBuf::from("/templates/serena/language_servers"));
    assert_eq!(resolved.graphify, bin.path().join("graphify"));
    let default = IncumbentTools::resolve(&IncumbentRequest::default(), Some(&path), Path::new("/eval/runs/.cache"), Duration::from_secs(1)).unwrap();
    assert_eq!(default.language_servers, PathBuf::from("/eval/runs/.cache/serena-language-servers"));
    let missing = IncumbentTools::resolve(&IncumbentRequest::default(), None, Path::new("/c"), Duration::from_secs(1)).unwrap_err();
    assert_eq!(missing.to_string(), "incumbent arm (Serena + graphify): `serena` is not on PATH");
  }

  #[test]
  fn should_put_the_graphify_directory_first_on_path_only_when_path_finds_another_graphify() {
    let first = tempfile::tempdir().unwrap();
    fs::write(first.path().join("graphify"), "").unwrap();
    let host = std::env::join_paths([first.path(), Path::new("/usr/bin")]).unwrap();
    let on_path = IncumbentTools {
      graphify: first.path().join("graphify"),
      ..tools()
    };
    let scratch = tempfile::tempdir().unwrap();
    assert_eq!(on_path.session_path(scratch.path(), Some(&host)).unwrap(), None);
    let linked_first = std::env::join_paths([scratch.path(), Path::new("/usr/bin")]).unwrap();
    std::os::unix::fs::symlink(first.path().join("graphify"), scratch.path().join("graphify")).unwrap();
    assert_eq!(on_path.session_path(scratch.path(), Some(&linked_first)).unwrap(), None, "the same binary behind a link");
    assert_eq!(
      tools().session_path(scratch.path(), Some(&host)).unwrap(),
      Some(format!("{}/incumbent-bin:{}:/usr/bin", scratch.path().display(), first.path().display()))
    );
    let bin: Vec<String> = fs::read_dir(scratch.path().join("incumbent-bin"))
      .unwrap()
      .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
      .collect();
    assert_eq!(bin, vec!["graphify"]);
    assert_eq!(
      fs::read_link(scratch.path().join("incumbent-bin/graphify")).unwrap(),
      PathBuf::from("/opt/graphify/bin/graphify")
    );
  }

  #[test]
  fn should_recognise_serena_edit_tools_and_hook_denials() {
    assert!(serena_edit("mcp__serena__replace_symbol_body"));
    assert!(!serena_edit("mcp__serena__find_symbol"));
    assert!(!serena_edit("replace_symbol_body"));
    let result = |text: &str, is_error: bool| ToolResult {
      text: text.to_owned(),
      is_error,
      ..ToolResult::default()
    };
    let read = "Too many consecutive read calls of files without using symbolic tools. You can continue using read now if needed, the counter was reset.";
    assert!(denied_by_serena_hook(&result(read, true)));
    assert!(!denied_by_serena_hook(&result(read, false)), "a file that quotes the reason is no denial");
    assert!(denied_by_serena_hook(&result("Too many consecutive non-symbolic tool calls (mixed grep and read).", true)));
    assert!(!denied_by_serena_hook(&result("1\texport const a = 1;", true)));
  }
}
