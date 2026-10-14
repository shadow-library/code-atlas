use std::collections::BTreeMap;

use toml::Value;

use super::keys::{Keyed, MCP_KEYS, PROJECT_KEYS, env_var, parse_env_value};
use super::layers::Origin;

const PREFIX: &str = "CODE_ATLAS_";
const NOT_SETTINGS: [&str; 3] = ["CODE_ATLAS_HOME", "CODE_ATLAS_BIN", "CODE_ATLAS_BUILD_COMMIT"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigEnv {
  vars: BTreeMap<String, String>,
}

pub(crate) struct EnvEntry {
  pub(crate) origin: Origin,
  pub(crate) key: &'static str,
  pub(crate) value: Value,
}

impl ConfigEnv {
  pub fn from_process() -> Self {
    Self::from_pairs(std::env::vars_os().filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?))))
  }

  pub fn from_pairs<K: Into<String>, V: Into<String>>(pairs: impl IntoIterator<Item = (K, V)>) -> Self {
    let vars = pairs
      .into_iter()
      .map(|(name, value)| (name.into(), value.into()))
      .filter(|(name, value)| name.starts_with(PREFIX) && !NOT_SETTINGS.contains(&name.as_str()) && !value.is_empty())
      .collect();
    Self { vars }
  }

  pub(crate) fn entries<T: Keyed>(&self) -> Vec<EnvEntry> {
    T::KEYS
      .iter()
      .filter_map(|spec| {
        let var = env_var(spec.name);
        let origin = Origin::Env { var: var.clone() };
        let value = parse_env_value(spec.kind, self.vars.get(&var)?).ok()?;
        spec.class.admits(&origin).ok()?;
        Some(EnvEntry { origin, key: spec.name, value })
      })
      .collect()
  }

  pub(crate) fn warnings(&self) -> Vec<String> {
    self
      .vars
      .iter()
      .filter_map(|(var, raw)| {
        let Some(spec) = PROJECT_KEYS.iter().chain(MCP_KEYS.iter()).find(|spec| env_var(spec.name) == *var) else {
          return Some(format!("env:{var}: unknown setting variable ignored"));
        };
        let origin = Origin::Env { var: var.clone() };
        if let Err(reason) = spec.class.admits(&origin) {
          return Some(format!("env:{var}: {} ignored: {reason}", spec.name));
        }
        parse_env_value(spec.kind, raw).err().map(|message| format!("env:{var}: {} ignored: {message}", spec.name))
      })
      .collect()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::registry::ProjectSettings;

  #[test]
  fn should_keep_only_non_empty_setting_variables() {
    let env = ConfigEnv::from_pairs([
      ("CODE_ATLAS_HOME", "/iso"),
      ("CODE_ATLAS_LSP_ENABLED", "false"),
      ("CODE_ATLAS_SCIP_ENABLED", ""),
      ("PATH", "/bin"),
    ]);
    let entries: Vec<(&str, Value)> = env.entries::<ProjectSettings>().into_iter().map(|entry| (entry.key, entry.value)).collect();
    assert_eq!(entries, [("lsp.enabled", Value::Boolean(false))]);
    assert!(env.warnings().is_empty());
  }

  #[test]
  fn should_warn_about_unknown_unparsable_and_fact_variables() {
    let env = ConfigEnv::from_pairs([
      ("CODE_ATLAS_MAX_FILE_BYTES", "5"),
      ("CODE_ATLAS_LSP_MAX_SERVERS", "many"),
      ("CODE_ATLAS_LSP_ENABLE", "true"),
    ]);
    assert!(env.entries::<ProjectSettings>().is_empty());
    assert_eq!(
      env.warnings(),
      [
        "env:CODE_ATLAS_LSP_ENABLE: unknown setting variable ignored",
        "env:CODE_ATLAS_LSP_MAX_SERVERS: lsp.max_servers ignored: expected a whole number, got \"many\"",
        "env:CODE_ATLAS_MAX_FILE_BYTES: max_file_bytes ignored: it changes indexed facts, so only config.toml, projects.toml or a repository's .code-atlas.toml may set it",
      ]
    );
  }
}
