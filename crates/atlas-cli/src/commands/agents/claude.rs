use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use atlas_mcp::guidance::{SERVER_NAME, ToolProfile};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::target::read;
use crate::error::{CliError, CliResult};

const CLAUDE: &str = "claude";
const CLAUDE_CONFIG_DIR_VAR: &str = "CLAUDE_CONFIG_DIR";
const CLAUDE_CONFIG_FILE: &str = ".claude.json";
const SETTINGS_FILE: &str = "settings.json";
const LOCAL_SETTINGS_FILE: &str = "settings.local.json";
const HOOK_TIMEOUT_SECS: u64 = 10;
const SEARCH_TOOLS_MATCHER: &str = "Grep|Glob|Bash";
const READ_TOOL_MATCHER: &str = "Read";
const WRITE_TOOLS_MATCHER: &str = "Write|Edit|MultiEdit";
const REMIND_SUFFIX: &str = "hook remind";
const SEED_SUFFIX: &str = "hook seed";
const COUNTERPART_SUFFIX: &str = "hook counterpart";
const NOTES_SUFFIX: &str = "hook notes";
const REUSE_SUFFIX: &str = "hook reuse";
const ALSO_CHANGE_SUFFIX: &str = "hook also-change";
const NEVER_FAIL: &str = " || true";
pub(super) const HOME_VAR: &str = "CODE_ATLAS_HOME";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum HookEvent {
  #[default]
  PreToolUse,
  UserPromptSubmit,
  PostToolUse,
  #[serde(rename = "PostToolUse:notes")]
  PostToolUseNotes,
  #[serde(rename = "PreToolUse:reuse")]
  PreToolUseReuse,
  #[serde(rename = "PostToolUse:also-change")]
  PostToolUseAlsoChange,
}

impl HookEvent {
  pub(super) fn name(self) -> &'static str {
    match self {
      Self::PreToolUse | Self::PreToolUseReuse => "PreToolUse",
      Self::UserPromptSubmit => "UserPromptSubmit",
      Self::PostToolUse | Self::PostToolUseNotes | Self::PostToolUseAlsoChange => "PostToolUse",
    }
  }

  pub(super) fn is_pre_tool_use(&self) -> bool {
    *self == Self::PreToolUse
  }

  fn suffix(self) -> &'static str {
    match self {
      Self::PreToolUse => REMIND_SUFFIX,
      Self::UserPromptSubmit => SEED_SUFFIX,
      Self::PostToolUse => COUNTERPART_SUFFIX,
      Self::PostToolUseNotes => NOTES_SUFFIX,
      Self::PreToolUseReuse => REUSE_SUFFIX,
      Self::PostToolUseAlsoChange => ALSO_CHANGE_SUFFIX,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegistrationScope {
  User,
  Local,
}

impl RegistrationScope {
  fn flag(self) -> &'static str {
    match self {
      Self::User => "user",
      Self::Local => "local",
    }
  }
}

fn non_empty_var(name: &str) -> Option<PathBuf> {
  std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from)
}

pub(crate) fn claude_config_path() -> Option<PathBuf> {
  non_empty_var(CLAUDE_CONFIG_DIR_VAR)
    .or_else(|| non_empty_var("HOME"))
    .map(|directory| directory.join(CLAUDE_CONFIG_FILE))
}

pub(super) fn user_settings_path() -> Option<PathBuf> {
  non_empty_var(CLAUDE_CONFIG_DIR_VAR)
    .or_else(|| non_empty_var("HOME").map(|home| home.join(".claude")))
    .map(|directory| directory.join(SETTINGS_FILE))
}

pub(super) fn local_settings_path(root: &Path) -> PathBuf {
  root.join(".claude").join(LOCAL_SETTINGS_FILE)
}

pub(super) fn is_local_settings(path: &Path) -> bool {
  path.file_name().is_some_and(|name| name == LOCAL_SETTINGS_FILE)
}

pub(super) fn server_entry(command: &str, home: Option<&OsStr>) -> Value {
  let mut entry = json!({ "type": "stdio", "command": command, "args": ["mcp", "--watch"] });
  if let Some(home) = home {
    entry["env"] = json!({ HOME_VAR: home.to_string_lossy() });
  }
  entry
}

pub(super) fn hook_groups(event: HookEvent, binary: &str, home: Option<&OsStr>) -> Vec<Value> {
  let environment = home.map(|home| format!("{HOME_VAR}={} ", shell_word(&home.to_string_lossy()))).unwrap_or_default();
  let command = format!("{environment}{} {}{NEVER_FAIL}", shell_word(binary), event.suffix());
  let hooks = json!([{ "type": "command", "command": command, "timeout": HOOK_TIMEOUT_SECS }]);
  match event {
    HookEvent::PreToolUse => vec![json!({ "matcher": SEARCH_TOOLS_MATCHER, "hooks": hooks })],
    HookEvent::UserPromptSubmit => vec![json!({ "hooks": hooks })],
    HookEvent::PostToolUse | HookEvent::PostToolUseNotes => vec![json!({ "matcher": READ_TOOL_MATCHER, "hooks": hooks })],
    HookEvent::PreToolUseReuse | HookEvent::PostToolUseAlsoChange => vec![json!({ "matcher": WRITE_TOOLS_MATCHER, "hooks": hooks })],
  }
}

pub(super) fn is_our_remind_group(group: &Value) -> bool {
  is_our_hook_group(HookEvent::PreToolUse, group)
}

pub(super) fn is_our_seed_group(group: &Value) -> bool {
  is_our_hook_group(HookEvent::UserPromptSubmit, group)
}

pub(super) fn is_our_counterpart_group(group: &Value) -> bool {
  is_our_hook_group(HookEvent::PostToolUse, group)
}

pub(super) fn is_our_notes_group(group: &Value) -> bool {
  is_our_hook_group(HookEvent::PostToolUseNotes, group)
}

pub(super) fn is_our_reuse_group(group: &Value) -> bool {
  is_our_hook_group(HookEvent::PreToolUseReuse, group)
}

pub(super) fn is_our_also_change_group(group: &Value) -> bool {
  is_our_hook_group(HookEvent::PostToolUseAlsoChange, group)
}

fn is_our_hook_group(event: HookEvent, group: &Value) -> bool {
  let Some([hook]) = group.get("hooks").and_then(Value::as_array).map(Vec::as_slice) else {
    return false;
  };
  let Some(command) = hook.get("command").and_then(Value::as_str) else { return false };
  let command = command.strip_suffix(NEVER_FAIL).unwrap_or(command);
  let Some(invocation) = command.strip_suffix(event.suffix()).map(str::trim_end) else {
    return false;
  };
  let program = match invocation.strip_suffix('\'') {
    Some(quoted) => quoted.rsplit('\'').next().unwrap_or(quoted),
    None => invocation.rsplit(' ').next().unwrap_or(invocation),
  };
  Path::new(program).file_name().is_some_and(|name| name.to_string_lossy().starts_with("catl"))
}

pub(super) fn permission_rules(profile: &ToolProfile) -> Vec<Value> {
  profile.read_only_tools().iter().map(|tool| Value::String(format!("mcp__{SERVER_NAME}__{tool}"))).collect()
}

pub(super) fn is_our_permission(rule: &Value) -> bool {
  let server = format!("mcp__{SERVER_NAME}");
  rule.as_str().is_some_and(|rule| rule == server || rule.starts_with(&format!("{server}__")))
}

fn shell_word(word: &str) -> String {
  match word.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:".contains(c)) {
    true => word.to_owned(),
    false => format!("'{}'", word.replace('\'', "'\\''")),
  }
}

pub(super) fn legacy_local_registrations(config: &Path) -> Vec<(PathBuf, Value)> {
  let Ok(Some(text)) = read(config) else { return Vec::new() };
  let Ok(document) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
  let Some(projects) = document.get("projects").and_then(Value::as_object) else {
    return Vec::new();
  };
  projects
    .iter()
    .filter_map(|(path, project)| Some((PathBuf::from(path), project.get("mcpServers")?.get(SERVER_NAME)?.clone())))
    .filter(|(path, entry)| path.is_dir() && super::target::is_legacy(entry))
    .collect()
}

fn add_json_command(scope: RegistrationScope, entry: &Value) -> Vec<String> {
  ["mcp", "add-json", "--scope", scope.flag(), SERVER_NAME, &entry.to_string()].map(str::to_owned).to_vec()
}

fn remove_command(scope: RegistrationScope) -> Vec<String> {
  ["mcp", "remove", "--scope", scope.flag(), SERVER_NAME].map(str::to_owned).to_vec()
}

pub(super) fn printable(arguments: &[String], cwd: Option<&Path>) -> String {
  let quoted: Vec<String> = arguments.iter().map(|argument| shell_word(argument)).collect();
  let command = format!("{CLAUDE} {}", quoted.join(" "));
  match cwd {
    Some(cwd) => format!("(cd {} && {command})", shell_word(&cwd.display().to_string())),
    None => command,
  }
}

pub(super) fn probe(example: Option<(RegistrationScope, Option<&Path>, &Value)>) -> CliResult<()> {
  let status = Command::new(CLAUDE)
    .arg("--version")
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status();
  match status {
    Ok(_) => Ok(()),
    Err(error) => Err(CliError::ClaudeUnavailable {
      reason: error.to_string(),
      command: example.map_or_else(
        || printable(&remove_command(RegistrationScope::User), None),
        |(scope, cwd, entry)| printable(&add_json_command(scope, entry), cwd),
      ),
    }),
  }
}

fn claude(arguments: &[String], cwd: Option<&Path>) -> Command {
  let mut command = Command::new(CLAUDE);
  command.args(arguments).stdin(Stdio::null());
  if let Some(cwd) = cwd {
    command.current_dir(cwd);
  }
  command
}

pub(super) fn register(scope: RegistrationScope, cwd: Option<&Path>, entry: &Value, previous: Option<&Value>) -> CliResult<()> {
  let _ = claude(&remove_command(scope), cwd).stdout(Stdio::null()).stderr(Stdio::null()).status();
  let Err(error) = run(&add_json_command(scope, entry), cwd) else { return Ok(()) };
  if let Some(previous) = previous
    && let Err(restore) = run(&add_json_command(scope, previous), cwd)
  {
    tracing::warn!(error = %restore, "could not restore the previous Claude Code entry");
  }
  Err(error)
}

pub(super) fn unregister(scope: RegistrationScope, cwd: Option<&Path>) -> CliResult<()> {
  run(&remove_command(scope), cwd)
}

fn run(arguments: &[String], cwd: Option<&Path>) -> CliResult<()> {
  let unavailable = |reason: String| CliError::ClaudeUnavailable {
    reason,
    command: printable(arguments, cwd),
  };
  let output = claude(arguments, cwd).output().map_err(|error| unavailable(error.to_string()))?;
  if output.status.success() {
    return Ok(());
  }
  Err(unavailable(format!(
    "`claude {}` failed ({}): {}",
    arguments[..2].join(" "),
    output.status,
    String::from_utf8_lossy(&output.stderr).trim()
  )))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_render_a_remind_hook_that_can_never_fail_and_carries_the_atlas_home() {
    let groups = hook_groups(HookEvent::PreToolUse, "/opt/my tools/catl", Some(OsStr::new("/iso home")));
    assert_eq!(
      groups,
      [json!({"matcher": "Grep|Glob|Bash", "hooks": [{"type": "command", "command": "CODE_ATLAS_HOME='/iso home' '/opt/my tools/catl' hook remind || true", "timeout": 10}]})]
    );
    assert_eq!(hook_groups(HookEvent::PreToolUse, "catl", None)[0]["hooks"][0]["command"], "catl hook remind || true");
    assert!(groups.iter().chain(&hook_groups(HookEvent::PreToolUse, "catl", None)).all(is_our_remind_group));
  }

  #[test]
  fn should_render_a_seed_hook_without_a_matcher_that_can_never_fail() {
    let groups = hook_groups(HookEvent::UserPromptSubmit, "catl", Some(OsStr::new("/iso")));
    assert_eq!(
      groups,
      [json!({"hooks": [{"type": "command", "command": "CODE_ATLAS_HOME=/iso catl hook seed || true", "timeout": 10}]})]
    );
    assert!(groups.iter().all(is_our_seed_group));
    assert!(!groups.iter().any(is_our_remind_group));
    assert!(!hook_groups(HookEvent::PreToolUse, "catl", None).iter().any(is_our_seed_group));
  }

  #[test]
  fn should_render_a_counterpart_hook_on_reads_that_can_never_fail() {
    let groups = hook_groups(HookEvent::PostToolUse, "catl", Some(OsStr::new("/iso")));
    assert_eq!(
      groups,
      [json!({"matcher": "Read", "hooks": [{"type": "command", "command": "CODE_ATLAS_HOME=/iso catl hook counterpart || true", "timeout": 10}]})]
    );
    assert!(groups.iter().all(is_our_counterpart_group));
    assert!(!groups.iter().any(is_our_remind_group) && !groups.iter().any(is_our_seed_group));
    assert!(!hook_groups(HookEvent::PreToolUse, "catl", None).iter().any(is_our_counterpart_group));
  }

  #[test]
  fn should_render_a_notes_hook_on_reads_next_to_the_counterpart_hook_without_claiming_it() {
    let groups = hook_groups(HookEvent::PostToolUseNotes, "catl", None);
    assert_eq!(
      groups,
      [json!({"matcher": "Read", "hooks": [{"type": "command", "command": "catl hook notes || true", "timeout": 10}]})]
    );
    assert!(groups.iter().all(is_our_notes_group));
    assert!(!groups.iter().any(is_our_counterpart_group));
    assert!(!hook_groups(HookEvent::PostToolUse, "catl", None).iter().any(is_our_notes_group));
    assert_eq!(HookEvent::PostToolUseNotes.name(), "PostToolUse");
    assert_eq!(serde_json::to_value(HookEvent::PostToolUseNotes).unwrap(), json!("PostToolUse:notes"));
  }

  #[test]
  fn should_render_the_reuse_and_also_change_hooks_on_writes_each_recorded_under_its_own_event() {
    let reuse = hook_groups(HookEvent::PreToolUseReuse, "catl", None);
    let also = hook_groups(HookEvent::PostToolUseAlsoChange, "catl", Some(OsStr::new("/iso")));
    assert_eq!(
      reuse,
      [json!({"matcher": "Write|Edit|MultiEdit", "hooks": [{"type": "command", "command": "catl hook reuse || true", "timeout": 10}]})]
    );
    assert_eq!(
      also,
      [json!({"matcher": "Write|Edit|MultiEdit", "hooks": [{"type": "command", "command": "CODE_ATLAS_HOME=/iso catl hook also-change || true", "timeout": 10}]})]
    );
    assert!(reuse.iter().all(is_our_reuse_group) && also.iter().all(is_our_also_change_group));
    assert!(!reuse.iter().any(is_our_remind_group) && !also.iter().any(is_our_counterpart_group) && !also.iter().any(is_our_notes_group));
    assert!(!hook_groups(HookEvent::PreToolUse, "catl", None).iter().any(is_our_reuse_group));
    assert_eq!((HookEvent::PreToolUseReuse.name(), HookEvent::PostToolUseAlsoChange.name()), ("PreToolUse", "PostToolUse"));
    assert_eq!(serde_json::to_value(HookEvent::PreToolUseReuse).unwrap(), json!("PreToolUse:reuse"));
    assert_eq!(serde_json::to_value(HookEvent::PostToolUseAlsoChange).unwrap(), json!("PostToolUse:also-change"));
  }

  #[test]
  fn should_recognise_only_catl_remind_groups_as_ours() {
    let group = |command: &str| json!({"matcher": "Bash", "hooks": [{"type": "command", "command": command}]});
    assert!(is_our_remind_group(&group("catl hook remind")));
    assert!(is_our_remind_group(&group("CODE_ATLAS_HOME=/x /usr/local/bin/catl hook remind || true")));
    assert!(!is_our_remind_group(&group("serena-hooks remind --client=claude-code")));
    assert!(!is_our_remind_group(&group("catl hook activate || true")));
    assert!(!is_our_remind_group(
      &json!({"matcher": "Bash", "hooks": [{"command": "catl hook remind"}, {"command": "lint"}]})
    ));
    assert!(is_our_seed_group(&json!({"hooks": [{"type": "command", "command": "/opt/catl hook seed || true"}]})));
    assert!(!is_our_seed_group(&json!({"hooks": [{"type": "command", "command": "my-seeder hook seed"}]})));
  }

  #[test]
  fn should_allow_exactly_the_read_only_tools_of_the_profile_and_own_every_rule_naming_the_server() {
    let rules = permission_rules(&ToolProfile::full());
    assert_eq!(rules.len(), 14);
    assert_eq!(rules[0], "mcp__code-atlas__atlas_api");
    assert!(!rules.contains(&json!("mcp__code-atlas__atlas_edit")));
    let core = ToolProfile::from_setting(&atlas_project::ToolsSetting::Core).unwrap();
    assert_eq!(
      permission_rules(&core),
      [
        "mcp__code-atlas__atlas_context",
        "mcp__code-atlas__atlas_impact",
        "mcp__code-atlas__atlas_search",
        "mcp__code-atlas__atlas_status",
        "mcp__code-atlas__atlas_symbol"
      ]
    );
    for rule in ["mcp__code-atlas", "mcp__code-atlas__atlas_edit", "mcp__code-atlas__atlas_search"] {
      assert!(is_our_permission(&json!(rule)), "{rule}");
    }
    for rule in ["mcp__code-atlas-dev__x", "Bash(ls)", "mcp__other__atlas_search"] {
      assert!(!is_our_permission(&json!(rule)), "{rule}");
    }
  }

  #[test]
  fn should_print_a_local_registration_with_its_directory() {
    let command = printable(&add_json_command(RegistrationScope::Local, &json!({"command": "catl"})), Some(Path::new("/src/my repo")));
    assert_eq!(command, "(cd '/src/my repo' && claude mcp add-json --scope local code-atlas '{\"command\":\"catl\"}')");
  }
}
