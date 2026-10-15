use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use atlas_core::RepoName;
use serde::Serialize;
use toml::Value;

use super::env::ConfigEnv;
use super::keys::{KeyClass, Keyed, McpSettings, ToolsSetting, find_spec};
use super::user::UserConfig;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "layer", rename_all = "snake_case")]
pub enum Origin {
  BuiltIn,
  UserConfig { path: PathBuf },
  Registry { path: PathBuf },
  Repo { repo: RepoName, path: PathBuf },
  Env { var: String },
  Flag { flag: String },
}

impl fmt::Display for Origin {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::BuiltIn => formatter.write_str("built-in"),
      Self::UserConfig { .. } => formatter.write_str("config.toml"),
      Self::Registry { .. } => formatter.write_str("projects.toml"),
      Self::Repo { repo, .. } => write!(formatter, "{repo}/.code-atlas.toml"),
      Self::Env { var } => write!(formatter, "env:{var}"),
      Self::Flag { flag } => write!(formatter, "flag:{flag}"),
    }
  }
}

impl KeyClass {
  pub(crate) fn admits(self, origin: &Origin) -> Result<(), &'static str> {
    match (self, origin) {
      (Self::Fact, Origin::Env { .. } | Origin::Flag { .. }) => Err("it changes indexed facts, so only config.toml, projects.toml or a repository's .code-atlas.toml may set it"),
      (Self::Executable, Origin::Repo { .. }) => Err("a repository file may not name an executable"),
      (Self::Credential, Origin::Repo { .. }) => Err("a repository file may not decide where credentials are read or sent"),
      (Self::Runtime, Origin::Repo { .. }) => Err("a repository file may only set index keys and max_file_bytes"),
      _ => Ok(()),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigEntry {
  pub key: String,
  pub value: Option<Value>,
  pub origins: Vec<Origin>,
}

impl ConfigEntry {
  pub fn is_built_in(&self) -> bool {
    self.origins == [Origin::BuiltIn]
  }
}

pub(crate) const FAIL_CLOSED_KEYS: [&str; 1] = ["mcp.tools"];

pub(crate) struct Layered<T> {
  pub(crate) value: T,
  origins: BTreeMap<&'static str, Origin>,
  pub(crate) warnings: Vec<String>,
  pub(crate) errors: Vec<String>,
}

impl<T: Keyed> Default for Layered<T> {
  fn default() -> Self {
    Self {
      value: T::default(),
      origins: BTreeMap::new(),
      warnings: Vec::new(),
      errors: Vec::new(),
    }
  }
}

impl<T: Keyed> Layered<T> {
  pub(crate) fn apply(&mut self, origin: &Origin, key: &str, value: &Value) {
    let Some(spec) = find_spec(key) else {
      self.warnings.push(format!("{origin}: unknown key {key} ignored"));
      return;
    };
    if let Err(reason) = spec.class.admits(origin) {
      self.warnings.push(format!("{origin}: {key} ignored: {reason}"));
      return;
    }
    if !T::KEYS.iter().any(|known| known.name == spec.name) {
      self.warnings.push(format!("{origin}: {key} ignored: only config.toml may set it"));
      return;
    }
    let mut candidate = self.value.clone();
    match candidate.set(spec.name, value).and_then(|()| candidate.check()) {
      Ok(()) => {
        self.value = candidate;
        self.origins.insert(spec.name, origin.clone());
      }
      Err(message) if FAIL_CLOSED_KEYS.contains(&spec.name) => self.errors.push(format!("{origin}: {key}: {message}")),
      Err(message) => self.warnings.push(format!("{origin}: {key} ignored: {message}")),
    }
  }

  pub(crate) fn entries(&self) -> Vec<ConfigEntry> {
    T::KEYS
      .iter()
      .map(|spec| ConfigEntry {
        key: spec.name.to_owned(),
        value: self.value.get(spec.name),
        origins: vec![self.origins.get(spec.name).cloned().unwrap_or(Origin::BuiltIn)],
      })
      .collect()
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpFlags {
  pub watch: Option<bool>,
  pub tools: Option<ToolsSetting>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMcp {
  pub settings: McpSettings,
  pub entries: Vec<ConfigEntry>,
  pub warnings: Vec<String>,
  pub errors: Vec<String>,
}

impl ResolvedMcp {
  pub fn origin(&self, key: &str) -> Origin {
    self
      .entries
      .iter()
      .find(|entry| entry.key == key)
      .and_then(|entry| entry.origins.first().cloned())
      .unwrap_or(Origin::BuiltIn)
  }
}

pub fn resolve_mcp(config: &UserConfig, env: &ConfigEnv, flags: McpFlags) -> ResolvedMcp {
  let mut layered = Layered::<McpSettings>::default();
  for (key, value) in &config.mcp {
    layered.apply(&config.origin(), key, value);
  }
  for entry in env.entries::<McpSettings>() {
    layered.apply(&entry.origin, entry.key, &entry.value);
  }
  for key in FAIL_CLOSED_KEYS {
    if let Some((origin, message)) = env.unparsable(key) {
      layered.errors.push(format!("{origin}: {key}: {message}"));
    }
  }
  if let Some(watch) = flags.watch {
    let flag = if watch { "--watch" } else { "--no-watch" };
    layered.apply(&Origin::Flag { flag: flag.to_owned() }, "mcp.watch", &Value::Boolean(watch));
  }
  if let Some(tools) = &flags.tools {
    layered.apply(&Origin::Flag { flag: "--tools".to_owned() }, "mcp.tools", &tools.to_value());
  }
  ResolvedMcp {
    entries: layered.entries(),
    settings: layered.value,
    warnings: layered.warnings,
    errors: layered.errors,
  }
}

#[cfg(test)]
mod tests {
  use std::fs;

  use super::*;
  use crate::home::AtlasHome;
  use crate::registry::ProjectSettings;

  fn user_config(text: &str) -> (tempfile::TempDir, UserConfig) {
    let dir = tempfile::tempdir().unwrap();
    let home = AtlasHome::rooted_at(dir.path());
    fs::create_dir_all(home.config_dir()).unwrap();
    fs::write(home.config_path(), text).unwrap();
    let config = UserConfig::load(&home).unwrap();
    (dir, config)
  }

  #[test]
  fn should_resolve_mcp_runtime_keys_with_flags_over_env_over_config_over_built_in() {
    let (_dir, config) = user_config("[mcp]\nwatch = false\nmax_body_lines = 200\nmax_output_bytes = 5000\n");
    let env = ConfigEnv::from_pairs([("CODE_ATLAS_MCP_WATCH", "true"), ("CODE_ATLAS_MCP_MAX_OUTPUT_BYTES", "7000")]);

    let without_flag = resolve_mcp(&config, &env, McpFlags::default());
    let with_flag = resolve_mcp(
      &config,
      &env,
      McpFlags {
        watch: Some(false),
        ..McpFlags::default()
      },
    );

    assert_eq!(
      (without_flag.settings.watch, without_flag.settings.max_output_bytes, without_flag.settings.max_body_lines),
      (true, 7000, 200)
    );
    let origins: Vec<String> = with_flag.entries.iter().map(|entry| format!("{} {}", entry.key, entry.origins[0])).collect();
    assert_eq!(
      origins,
      [
        "mcp.watch flag:--no-watch",
        "mcp.tools built-in",
        "mcp.max_output_bytes env:CODE_ATLAS_MCP_MAX_OUTPUT_BYTES",
        "mcp.max_body_lines config.toml"
      ]
    );
    assert!(!with_flag.settings.watch);
  }

  #[test]
  fn should_fail_closed_on_an_invalid_tools_value_from_any_layer_instead_of_serving_more_tools() {
    let (_dir, config) = user_config("[mcp]\ntools = \"core\"\n");
    let typo = ConfigEnv::from_pairs([("CODE_ATLAS_MCP_TOOLS", "readonly")]);
    let broken_list = ConfigEnv::from_pairs([("CODE_ATLAS_MCP_TOOLS", "[\"atlas_search\"")]);

    let from_typo = resolve_mcp(&config, &typo, McpFlags::default());
    let from_broken_list = resolve_mcp(&config, &broken_list, McpFlags::default());

    assert_eq!(
      from_typo.errors,
      ["env:CODE_ATLAS_MCP_TOOLS: mcp.tools: expected \"core\", \"read-only\", \"full\" or an array of tool names, got \"readonly\""]
    );
    assert!(from_typo.warnings.is_empty(), "{:?}", from_typo.warnings);
    assert_eq!(
      from_broken_list.errors,
      ["env:CODE_ATLAS_MCP_TOOLS: mcp.tools: expected a TOML array such as [\"a\", \"b\"], got \"[\\\"atlas_search\\\"\""]
    );
    assert!(resolve_mcp(&config, &ConfigEnv::default(), McpFlags::default()).errors.is_empty());
  }

  #[test]
  fn should_let_the_tools_flag_win_over_env_and_config() {
    let (_dir, config) = user_config("[mcp]\ntools = \"full\"\n");
    let env = ConfigEnv::from_pairs([("CODE_ATLAS_MCP_TOOLS", "read-only")]);

    let from_env = resolve_mcp(&config, &env, McpFlags::default());
    let from_flag = resolve_mcp(
      &config,
      &env,
      McpFlags {
        tools: Some(ToolsSetting::Core),
        ..McpFlags::default()
      },
    );

    assert_eq!(from_env.settings.tools, ToolsSetting::ReadOnly);
    assert_eq!(from_flag.settings.tools, ToolsSetting::Core);
    assert_eq!(from_flag.origin("mcp.tools").to_string(), "flag:--tools");
    assert_eq!(from_env.origin("mcp.tools").to_string(), "env:CODE_ATLAS_MCP_TOOLS");
  }

  #[test]
  fn should_never_let_a_flag_or_env_change_a_fact_key() {
    let mut layered = Layered::<ProjectSettings>::default();
    layered.apply(
      &Origin::Flag {
        flag: "--max-file-bytes".to_owned(),
      },
      "max_file_bytes",
      &Value::Integer(5),
    );
    layered.apply(
      &Origin::Env {
        var: "CODE_ATLAS_MAX_FILE_BYTES".to_owned(),
      },
      "max_file_bytes",
      &Value::Integer(5),
    );

    assert_eq!(layered.value.max_file_bytes, ProjectSettings::default().max_file_bytes);
    assert_eq!(
      layered.warnings,
      [
        "flag:--max-file-bytes: max_file_bytes ignored: it changes indexed facts, so only config.toml, projects.toml or a repository's .code-atlas.toml may set it",
        "env:CODE_ATLAS_MAX_FILE_BYTES: max_file_bytes ignored: it changes indexed facts, so only config.toml, projects.toml or a repository's .code-atlas.toml may set it",
      ]
    );
  }

  #[test]
  fn should_reject_executable_credential_and_machine_keys_from_a_repository_file() {
    let origin = Origin::Repo {
      repo: RepoName::new("api").unwrap(),
      path: PathBuf::from("/src/api/.code-atlas.toml"),
    };
    let mut layered = Layered::<ProjectSettings>::default();
    for (key, value) in [
      ("lsp.typescript_command", Value::Array(vec![Value::String("/tmp/evil".to_owned())])),
      ("llm.api_key_env", Value::String("AWS_SECRET_ACCESS_KEY".to_owned())),
      ("llm.openai_base_url", Value::String("https://attacker.example".to_owned())),
      ("agents.binary", Value::String("/tmp/evil".to_owned())),
      ("lsp.max_servers", Value::Integer(64)),
      ("max_file_bytes", Value::Integer(2048)),
    ] {
      layered.apply(&origin, key, &value);
    }

    assert_eq!(layered.value.lsp.typescript_command, None);
    assert_eq!((layered.value.llm.api_key_env.as_deref(), layered.value.llm.openai_base_url.as_deref()), (None, None));
    assert_eq!((layered.value.lsp.max_servers, layered.value.max_file_bytes), (2, 2048));
    assert_eq!(
      layered.warnings,
      [
        "api/.code-atlas.toml: lsp.typescript_command ignored: a repository file may not name an executable",
        "api/.code-atlas.toml: llm.api_key_env ignored: a repository file may not decide where credentials are read or sent",
        "api/.code-atlas.toml: llm.openai_base_url ignored: a repository file may not decide where credentials are read or sent",
        "api/.code-atlas.toml: agents.binary ignored: a repository file may not name an executable",
        "api/.code-atlas.toml: lsp.max_servers ignored: a repository file may only set index keys and max_file_bytes",
      ]
    );
  }

  #[test]
  fn should_keep_the_lower_layer_when_a_value_is_invalid() {
    let mut layered = Layered::<ProjectSettings>::default();
    let origin = Origin::Env {
      var: "CODE_ATLAS_LSP_MAX_SERVERS".to_owned(),
    };
    layered.apply(&origin, "lsp.max_servers", &Value::Integer(0));
    assert_eq!(layered.value.lsp.max_servers, 2);
    assert_eq!(
      layered.warnings,
      ["env:CODE_ATLAS_LSP_MAX_SERVERS: lsp.max_servers ignored: lsp.max_servers must be at least 1"]
    );
  }
}
