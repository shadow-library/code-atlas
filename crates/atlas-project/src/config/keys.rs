use std::path::Path;

use serde::Serialize;
use toml::Value;

use crate::registry::{LspUnit, ProjectSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyClass {
  Fact,
  Runtime,
  Executable,
  Credential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueKind {
  Flag,
  Number,
  Text,
  List,
  TextOrList,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct KeySpec {
  pub(crate) name: &'static str,
  pub(crate) class: KeyClass,
  pub(crate) kind: ValueKind,
}

const fn key(name: &'static str, class: KeyClass, kind: ValueKind) -> KeySpec {
  KeySpec { name, class, kind }
}

pub(crate) const PROJECT_KEYS: [KeySpec; 18] = [
  key("max_file_bytes", KeyClass::Fact, ValueKind::Number),
  key("llm.enabled", KeyClass::Runtime, ValueKind::Flag),
  key("llm.provider", KeyClass::Runtime, ValueKind::Text),
  key("llm.model", KeyClass::Runtime, ValueKind::Text),
  key("llm.openai_base_url", KeyClass::Credential, ValueKind::Text),
  key("llm.api_key_env", KeyClass::Credential, ValueKind::Text),
  key("scip.enabled", KeyClass::Runtime, ValueKind::Flag),
  key("scip.debounce_secs", KeyClass::Runtime, ValueKind::Number),
  key("lsp.enabled", KeyClass::Runtime, ValueKind::Flag),
  key("lsp.download", KeyClass::Runtime, ValueKind::Flag),
  key("lsp.typescript_command", KeyClass::Executable, ValueKind::List),
  key("lsp.unit", KeyClass::Runtime, ValueKind::Text),
  key("lsp.max_servers", KeyClass::Runtime, ValueKind::Number),
  key("lsp.idle_secs", KeyClass::Runtime, ValueKind::Number),
  key("lsp.ready_timeout_secs", KeyClass::Runtime, ValueKind::Number),
  key("lsp.check_timeout_secs", KeyClass::Runtime, ValueKind::Number),
  key("lsp.exclude", KeyClass::Runtime, ValueKind::List),
  key("detect.route_families", KeyClass::Fact, ValueKind::List),
];

pub(crate) const MCP_KEYS: [KeySpec; 4] = [
  key("mcp.watch", KeyClass::Runtime, ValueKind::Flag),
  key("mcp.tools", KeyClass::Runtime, ValueKind::TextOrList),
  key("mcp.max_output_bytes", KeyClass::Runtime, ValueKind::Number),
  key("mcp.max_body_lines", KeyClass::Runtime, ValueKind::Number),
];

pub(crate) struct ReservedKey {
  pub(crate) spec: KeySpec,
  validate: fn(&Value) -> Result<(), String>,
}

impl ReservedKey {
  pub(crate) fn check(&self, value: &Value) -> Result<(), String> {
    (self.validate)(value)
  }
}

const fn reserved(name: &'static str, class: KeyClass, kind: ValueKind, validate: fn(&Value) -> Result<(), String>) -> ReservedKey {
  ReservedKey {
    spec: key(name, class, kind),
    validate,
  }
}

pub(crate) const RESERVED_KEYS: [ReservedKey; 8] = [
  reserved("agents.binary", KeyClass::Executable, ValueKind::Text, check_binary),
  reserved("agents.guidance", KeyClass::Runtime, ValueKind::Text, |value| {
    choice(value, &["none", "auto", "local", "tracked"])
  }),
  reserved("agents.claude.enabled", KeyClass::Runtime, ValueKind::Flag, |value| as_flag(value).map(drop)),
  reserved("agents.claude.scope", KeyClass::Runtime, ValueKind::Text, |value| {
    choice(value, &["user", "local", "project"])
  }),
  reserved("agents.claude.hooks", KeyClass::Runtime, ValueKind::List, check_hooks),
  reserved("agents.codex.enabled", KeyClass::Runtime, ValueKind::Flag, |value| as_flag(value).map(drop)),
  reserved("worktrees.mode", KeyClass::Runtime, ValueKind::Text, |value| choice(value, &["overlay", "main", "off"])),
  reserved("worktrees.gc_after_days", KeyClass::Runtime, ValueKind::Number, |value| as_positive(value).map(drop)),
];

pub(crate) fn find_spec(name: &str) -> Option<KeySpec> {
  PROJECT_KEYS
    .iter()
    .chain(MCP_KEYS.iter())
    .chain(RESERVED_KEYS.iter().map(|reserved| &reserved.spec))
    .find(|spec| spec.name == name)
    .copied()
}

pub(crate) fn env_var(name: &str) -> String {
  format!("CODE_ATLAS_{}", name.replace('.', "_").to_ascii_uppercase())
}

pub(crate) trait Keyed: Default + Clone {
  const KEYS: &'static [KeySpec];

  fn get(&self, key: &str) -> Option<Value>;

  fn set(&mut self, key: &str, value: &Value) -> Result<(), String>;

  fn check(&self) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolsSetting {
  Core,
  ReadOnly,
  Full,
  Only(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpSettings {
  pub watch: bool,
  pub tools: ToolsSetting,
  pub max_output_bytes: usize,
  pub max_body_lines: usize,
}

impl McpSettings {
  pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 100_000;
  pub const DEFAULT_MAX_BODY_LINES: usize = 400;
}

impl Default for McpSettings {
  fn default() -> Self {
    Self {
      watch: false,
      tools: ToolsSetting::Full,
      max_output_bytes: Self::DEFAULT_MAX_OUTPUT_BYTES,
      max_body_lines: Self::DEFAULT_MAX_BODY_LINES,
    }
  }
}

impl Keyed for McpSettings {
  const KEYS: &'static [KeySpec] = &MCP_KEYS;

  fn get(&self, key: &str) -> Option<Value> {
    match key {
      "mcp.watch" => Some(Value::Boolean(self.watch)),
      "mcp.tools" => Some(match &self.tools {
        ToolsSetting::Core => Value::String("core".to_owned()),
        ToolsSetting::ReadOnly => Value::String("read-only".to_owned()),
        ToolsSetting::Full => Value::String("full".to_owned()),
        ToolsSetting::Only(tools) => words(tools),
      }),
      "mcp.max_output_bytes" => Some(number(self.max_output_bytes as u64)),
      "mcp.max_body_lines" => Some(number(self.max_body_lines as u64)),
      _ => None,
    }
  }

  fn set(&mut self, key: &str, value: &Value) -> Result<(), String> {
    match key {
      "mcp.watch" => self.watch = as_flag(value)?,
      "mcp.tools" => self.tools = as_tools(value)?,
      "mcp.max_output_bytes" => self.max_output_bytes = as_size(as_positive(value)?)?,
      "mcp.max_body_lines" => self.max_body_lines = as_size(as_positive(value)?)?,
      _ => return Err(format!("unknown key {key}")),
    }
    Ok(())
  }

  fn check(&self) -> Result<(), String> {
    Ok(())
  }
}

impl Keyed for ProjectSettings {
  const KEYS: &'static [KeySpec] = &PROJECT_KEYS;

  fn get(&self, key: &str) -> Option<Value> {
    let text = |value: &Option<String>| value.clone().map(Value::String);
    match key {
      "max_file_bytes" => Some(number(self.max_file_bytes)),
      "detect.route_families" => Some(words(&self.detect.route_families)),
      "llm.enabled" => Some(Value::Boolean(self.llm.enabled)),
      "llm.provider" => text(&self.llm.provider),
      "llm.model" => text(&self.llm.model),
      "llm.openai_base_url" => text(&self.llm.openai_base_url),
      "llm.api_key_env" => text(&self.llm.api_key_env),
      "scip.enabled" => Some(Value::Boolean(self.scip.enabled)),
      "scip.debounce_secs" => Some(number(self.scip.debounce_secs)),
      "lsp.enabled" => Some(Value::Boolean(self.lsp.enabled)),
      "lsp.download" => Some(Value::Boolean(self.lsp.download)),
      "lsp.typescript_command" => self.lsp.typescript_command.as_deref().map(words),
      "lsp.unit" => Some(Value::String(
        match self.lsp.unit {
          LspUnit::Project => "project",
          LspUnit::Repo => "repo",
        }
        .to_owned(),
      )),
      "lsp.max_servers" => Some(number(self.lsp.max_servers as u64)),
      "lsp.idle_secs" => Some(number(self.lsp.idle_secs)),
      "lsp.ready_timeout_secs" => Some(number(self.lsp.ready_timeout_secs)),
      "lsp.check_timeout_secs" => Some(number(self.lsp.check_timeout_secs)),
      "lsp.exclude" => Some(words(&self.lsp.exclude)),
      _ => None,
    }
  }

  fn set(&mut self, key: &str, value: &Value) -> Result<(), String> {
    match key {
      "max_file_bytes" => self.max_file_bytes = as_number(value)?,
      "detect.route_families" => self.detect.route_families = as_words(value)?,
      "llm.enabled" => self.llm.enabled = as_flag(value)?,
      "llm.provider" => self.llm.provider = Some(as_text(value)?),
      "llm.model" => self.llm.model = Some(as_text(value)?),
      "llm.openai_base_url" => self.llm.openai_base_url = Some(as_unechoed_text(value)?),
      "llm.api_key_env" => self.llm.api_key_env = Some(as_unechoed_text(value)?),
      "scip.enabled" => self.scip.enabled = as_flag(value)?,
      "scip.debounce_secs" => self.scip.debounce_secs = as_number(value)?,
      "lsp.enabled" => self.lsp.enabled = as_flag(value)?,
      "lsp.download" => self.lsp.download = as_flag(value)?,
      "lsp.typescript_command" => self.lsp.typescript_command = Some(as_words(value)?),
      "lsp.unit" => {
        self.lsp.unit = match as_text(value)?.as_str() {
          "project" => LspUnit::Project,
          "repo" => LspUnit::Repo,
          other => return Err(format!("expected \"project\" or \"repo\", got {other:?}")),
        }
      }
      "lsp.max_servers" => self.lsp.max_servers = as_size(as_number(value)?)?,
      "lsp.idle_secs" => self.lsp.idle_secs = as_number(value)?,
      "lsp.ready_timeout_secs" => self.lsp.ready_timeout_secs = as_number(value)?,
      "lsp.check_timeout_secs" => self.lsp.check_timeout_secs = as_number(value)?,
      "lsp.exclude" => self.lsp.exclude = as_words(value)?,
      _ => return Err(format!("unknown key {key}")),
    }
    Ok(())
  }

  fn check(&self) -> Result<(), String> {
    self.validate()
  }
}

pub(crate) fn flatten(prefix: &str, table: &toml::Table) -> Vec<(String, Value)> {
  let mut entries = Vec::new();
  for (name, value) in table {
    let dotted = match prefix.is_empty() {
      true => name.clone(),
      false => format!("{prefix}.{name}"),
    };
    match value {
      Value::Table(inner) => entries.extend(flatten(&dotted, inner)),
      other => entries.push((dotted, other.clone())),
    }
  }
  entries
}

pub(crate) fn parse_env_value(kind: ValueKind, raw: &str) -> Result<Value, String> {
  match kind {
    ValueKind::Flag => match raw.trim().to_ascii_lowercase().as_str() {
      "true" | "1" | "on" | "yes" => Ok(Value::Boolean(true)),
      "false" | "0" | "off" | "no" => Ok(Value::Boolean(false)),
      _ => Err(format!("expected true or false, got {raw:?}")),
    },
    ValueKind::Number => raw.trim().parse::<i64>().map(Value::Integer).map_err(|_| format!("expected a whole number, got {raw:?}")),
    ValueKind::Text => Ok(Value::String(raw.to_owned())),
    ValueKind::List => parse_array(raw),
    ValueKind::TextOrList if raw.trim_start().starts_with('[') => parse_array(raw),
    ValueKind::TextOrList => Ok(Value::String(raw.to_owned())),
  }
}

fn parse_array(raw: &str) -> Result<Value, String> {
  toml::from_str::<toml::Table>(&format!("value = {raw}"))
    .ok()
    .and_then(|mut table| table.remove("value"))
    .filter(Value::is_array)
    .ok_or_else(|| format!("expected a TOML array such as [\"a\", \"b\"], got {raw:?}"))
}

fn number(value: u64) -> Value {
  Value::Integer(i64::try_from(value).unwrap_or(i64::MAX))
}

fn words(values: &[String]) -> Value {
  Value::Array(values.iter().cloned().map(Value::String).collect())
}

pub(crate) fn as_flag(value: &Value) -> Result<bool, String> {
  value.as_bool().ok_or_else(|| format!("expected true or false, got {value}"))
}

fn as_number(value: &Value) -> Result<u64, String> {
  value
    .as_integer()
    .and_then(|number| u64::try_from(number).ok())
    .ok_or_else(|| format!("expected a non-negative whole number, got {value}"))
}

fn as_positive(value: &Value) -> Result<u64, String> {
  match as_number(value)? {
    0 => Err("must be greater than 0".to_owned()),
    number => Ok(number),
  }
}

fn as_size(value: u64) -> Result<usize, String> {
  usize::try_from(value).map_err(|_| format!("{value} is too large"))
}

fn as_unechoed_text(value: &Value) -> Result<String, String> {
  value.as_str().map(str::to_owned).ok_or_else(|| "expected a string".to_owned())
}

fn as_text(value: &Value) -> Result<String, String> {
  value.as_str().map(str::to_owned).ok_or_else(|| format!("expected a string, got {value}"))
}

pub(crate) fn as_words(value: &Value) -> Result<Vec<String>, String> {
  let array = value.as_array().ok_or_else(|| format!("expected an array of strings, got {value}"))?;
  array.iter().map(as_text).collect()
}

fn as_tools(value: &Value) -> Result<ToolsSetting, String> {
  if let Some(profile) = value.as_str() {
    return match profile {
      "core" => Ok(ToolsSetting::Core),
      "read-only" => Ok(ToolsSetting::ReadOnly),
      "full" => Ok(ToolsSetting::Full),
      other => Err(format!("expected \"core\", \"read-only\", \"full\" or an array of tool names, got {other:?}")),
    };
  }
  let tools = as_words(value)?;
  if tools.is_empty() || tools.iter().any(|tool| tool.trim().is_empty()) {
    return Err("a tool list must name at least one tool and no empty names".to_owned());
  }
  Ok(ToolsSetting::Only(tools))
}

fn choice(value: &Value, options: &[&str]) -> Result<(), String> {
  let text = as_text(value)?;
  match options.contains(&text.as_str()) {
    true => Ok(()),
    false => Err(format!("expected one of {}, got {text:?}", options.join(", "))),
  }
}

const CLAUDE_HOOKS: [&str; 2] = ["auto-approve", "remind"];

fn check_hooks(value: &Value) -> Result<(), String> {
  let hooks = as_words(value)?;
  if let Some(unknown) = hooks.iter().find(|hook| !CLAUDE_HOOKS.contains(&hook.as_str())) {
    return Err(format!("expected hooks from {}, got {unknown:?}", CLAUDE_HOOKS.join(", ")));
  }
  match hooks.iter().enumerate().any(|(index, hook)| hooks[..index].contains(hook)) {
    true => Err("each hook may be listed once".to_owned()),
    false => Ok(()),
  }
}

fn check_binary(value: &Value) -> Result<(), String> {
  let binary = as_text(value)?;
  match binary.is_empty() || (binary.contains('/') && !Path::new(&binary).is_absolute()) {
    true => Err(format!("expected a command name such as \"catl\" or an absolute path, got {binary:?}")),
    false => Ok(()),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_round_trip_every_project_key_through_get_and_set() {
    let mut changed = ProjectSettings::default();
    changed.lsp.typescript_command = Some(vec!["/bin/tls".to_owned()]);
    changed.llm.provider = Some("anthropic".to_owned());
    changed.llm.model = Some("m".to_owned());
    changed.llm.openai_base_url = Some("http://localhost".to_owned());
    changed.llm.api_key_env = Some("KEY".to_owned());
    for spec in PROJECT_KEYS {
      let value = changed.get(spec.name).unwrap_or_else(|| panic!("{} has no value", spec.name));
      let mut target = ProjectSettings::default();
      target.set(spec.name, &value).unwrap();
      assert_eq!(target.get(spec.name), Some(value), "{}", spec.name);
    }
  }

  #[test]
  fn should_round_trip_every_mcp_key_through_get_and_set() {
    for spec in MCP_KEYS {
      let value = McpSettings::default().get(spec.name).unwrap();
      let mut target = McpSettings::default();
      target.set(spec.name, &value).unwrap();
      assert_eq!(target, McpSettings::default(), "{}", spec.name);
    }
  }

  #[test]
  fn should_name_environment_variables_after_the_key() {
    assert_eq!(env_var("lsp.max_servers"), "CODE_ATLAS_LSP_MAX_SERVERS");
    assert_eq!(env_var("max_file_bytes"), "CODE_ATLAS_MAX_FILE_BYTES");
  }

  #[test]
  fn should_parse_environment_values_by_kind() {
    assert_eq!(parse_env_value(ValueKind::Flag, "Off"), Ok(Value::Boolean(false)));
    assert_eq!(parse_env_value(ValueKind::Number, " 12 "), Ok(Value::Integer(12)));
    assert_eq!(
      parse_env_value(ValueKind::List, "[\"a\", \"b\"]"),
      Ok(Value::Array(vec![Value::String("a".to_owned()), Value::String("b".to_owned())]))
    );
    assert_eq!(parse_env_value(ValueKind::TextOrList, "core"), Ok(Value::String("core".to_owned())));
    assert_eq!(parse_env_value(ValueKind::Flag, "maybe"), Err("expected true or false, got \"maybe\"".to_owned()));
    assert_eq!(
      parse_env_value(ValueKind::List, "a,b"),
      Err("expected a TOML array such as [\"a\", \"b\"], got \"a,b\"".to_owned())
    );
  }

  #[test]
  fn should_validate_reserved_keys() {
    let check = |name: &str, value: Value| RESERVED_KEYS.iter().find(|reserved| reserved.spec.name == name).unwrap().check(&value);
    assert_eq!(check("agents.binary", Value::String("catl".to_owned())), Ok(()));
    assert_eq!(check("agents.binary", Value::String("/usr/local/bin/catl".to_owned())), Ok(()));
    assert!(check("agents.binary", Value::String("bin/catl".to_owned())).is_err());
    assert_eq!(
      check("worktrees.mode", Value::String("copy".to_owned())),
      Err("expected one of overlay, main, off, got \"copy\"".to_owned())
    );
    assert_eq!(check("worktrees.gc_after_days", Value::Integer(0)), Err("must be greater than 0".to_owned()));
    let hooks = |names: &[&str]| Value::Array(names.iter().map(|name| Value::String((*name).to_owned())).collect());
    assert_eq!(check("agents.claude.hooks", hooks(&["auto-approve", "remind"])), Ok(()));
    assert_eq!(check("agents.claude.hooks", hooks(&[])), Ok(()));
    assert_eq!(
      check("agents.claude.hooks", hooks(&["activate"])),
      Err("expected hooks from auto-approve, remind, got \"activate\"".to_owned())
    );
    assert_eq!(check("agents.claude.hooks", hooks(&["remind", "remind"])), Err("each hook may be listed once".to_owned()));
  }
}
