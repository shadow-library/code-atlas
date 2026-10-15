use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;

use toml::Value;

use super::keys::{KeySpec, Keyed, MCP_KEYS, PROJECT_KEYS, env_var, parse_env_value};
use super::layers::{FAIL_CLOSED_KEYS, Origin};

const PREFIX: &str = "CODE_ATLAS_";
const NOT_UTF8: &str = "the value is not valid UTF-8";
const NOT_SETTINGS: [&str; 3] = ["CODE_ATLAS_HOME", "CODE_ATLAS_BIN", "CODE_ATLAS_BUILD_COMMIT"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigEnv {
  vars: BTreeMap<String, String>,
  non_utf8: BTreeSet<String>,
}

pub(crate) struct EnvEntry {
  pub(crate) origin: Origin,
  pub(crate) key: &'static str,
  pub(crate) value: Value,
}

impl ConfigEnv {
  pub fn from_process() -> Self {
    Self::from_os_pairs(std::env::vars_os())
  }

  pub(crate) fn from_os_pairs(pairs: impl IntoIterator<Item = (OsString, OsString)>) -> Self {
    let mut utf8 = Vec::new();
    let mut non_utf8 = BTreeSet::new();
    for (name, value) in pairs {
      let Ok(name) = name.into_string() else { continue };
      match value.into_string() {
        Ok(value) => utf8.push((name, value)),
        Err(_) if name.starts_with(PREFIX) && !NOT_SETTINGS.contains(&name.as_str()) => {
          non_utf8.insert(name);
        }
        Err(_) => {}
      }
    }
    Self {
      non_utf8,
      ..Self::from_pairs(utf8)
    }
  }

  pub fn from_pairs<K: Into<String>, V: Into<String>>(pairs: impl IntoIterator<Item = (K, V)>) -> Self {
    let vars = pairs
      .into_iter()
      .map(|(name, value)| (name.into(), value.into()))
      .filter(|(name, value)| name.starts_with(PREFIX) && !NOT_SETTINGS.contains(&name.as_str()) && !value.is_empty())
      .collect();
    Self { vars, non_utf8: BTreeSet::new() }
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

  pub(crate) fn unparsable(&self, key: &str) -> Option<(Origin, String)> {
    let spec = MCP_KEYS.iter().chain(PROJECT_KEYS.iter()).find(|spec| spec.name == key)?;
    let var = env_var(spec.name);
    if self.non_utf8.contains(&var) {
      return Some((Origin::Env { var }, NOT_UTF8.to_owned()));
    }
    let message = parse_env_value(spec.kind, self.vars.get(&var)?).err()?;
    Some((Origin::Env { var }, message))
  }

  pub(crate) fn warnings(&self) -> Vec<String> {
    let readable = self.vars.iter().filter_map(|(var, raw)| {
      let Some(spec) = find_spec(var) else {
        return Some(format!("env:{var}: unknown setting variable ignored"));
      };
      let origin = Origin::Env { var: var.clone() };
      if let Err(reason) = spec.class.admits(&origin) {
        return Some(format!("env:{var}: {} ignored: {reason}", spec.name));
      }
      if FAIL_CLOSED_KEYS.contains(&spec.name) {
        return None;
      }
      parse_env_value(spec.kind, raw).err().map(|message| format!("env:{var}: {} ignored: {message}", spec.name))
    });
    let undecodable = self.non_utf8.iter().filter_map(|var| {
      let Some(spec) = find_spec(var) else {
        return Some(format!("env:{var}: unknown setting variable ignored"));
      };
      if let Err(reason) = spec.class.admits(&Origin::Env { var: var.clone() }) {
        return Some(format!("env:{var}: {} ignored: {reason}", spec.name));
      }
      match FAIL_CLOSED_KEYS.contains(&spec.name) {
        true => None,
        false => Some(format!("env:{var}: {} ignored: {NOT_UTF8}", spec.name)),
      }
    });
    readable.chain(undecodable).collect()
  }
}

fn find_spec(var: &str) -> Option<&'static KeySpec> {
  PROJECT_KEYS.iter().chain(MCP_KEYS.iter()).find(|spec| env_var(spec.name) == var)
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

  #[cfg(unix)]
  #[test]
  fn should_record_non_utf8_setting_values_as_unparsable_instead_of_dropping_them() {
    use std::os::unix::ffi::OsStringExt;
    let invalid = OsString::from_vec(vec![0x66, 0xff, 0x6f]);
    let env = ConfigEnv::from_os_pairs([
      (OsString::from("CODE_ATLAS_MCP_TOOLS"), invalid.clone()),
      (OsString::from("CODE_ATLAS_LSP_ENABLED"), invalid.clone()),
      (OsString::from("CODE_ATLAS_HOME"), invalid.clone()),
      (OsString::from("CODE_ATLAS_MAX_FILE_BYTES"), invalid.clone()),
      (OsString::from("PATH"), invalid),
    ]);
    assert!(env.entries::<ProjectSettings>().is_empty());
    assert_eq!(
      env.unparsable("mcp.tools"),
      Some((
        Origin::Env {
          var: "CODE_ATLAS_MCP_TOOLS".to_owned()
        },
        "the value is not valid UTF-8".to_owned()
      ))
    );
    assert_eq!(
      env.warnings(),
      [
        "env:CODE_ATLAS_LSP_ENABLED: lsp.enabled ignored: the value is not valid UTF-8",
        "env:CODE_ATLAS_MAX_FILE_BYTES: max_file_bytes ignored: it changes indexed facts, so only config.toml, projects.toml or a repository's .code-atlas.toml may set it"
      ]
    );
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
        "env:CODE_ATLAS_LSP_MAX_SERVERS: lsp.max_servers ignored: expected a whole number, got a string",
        "env:CODE_ATLAS_MAX_FILE_BYTES: max_file_bytes ignored: it changes indexed facts, so only config.toml, projects.toml or a repository's .code-atlas.toml may set it",
      ]
    );
  }
}
