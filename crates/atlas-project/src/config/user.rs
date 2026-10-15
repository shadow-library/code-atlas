use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use toml::Value;

use super::families::FamilyCatalog;
use super::keys::{Keyed, McpSettings, RESERVED_KEYS, as_words, flatten};
use super::layers::{ConfigEntry, Origin};
use crate::error::ProjectError;
use crate::globs::PathGlobs;
use crate::home::AtlasHome;
use crate::registry::ProjectSettings;

const SUPPORTED_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct UserConfig {
  path: PathBuf,
  present: bool,
  pub(crate) defaults: Vec<(String, Value)>,
  pub(crate) default_exclude: Vec<String>,
  pub(crate) mcp: Vec<(String, Value)>,
  reserved: Vec<(String, Value)>,
  families: FamilyCatalog,
  warnings: Vec<String>,
}

impl UserConfig {
  pub fn load(home: &AtlasHome) -> Result<Self, ProjectError> {
    let path = home.config_path();
    let mut config = match fs::read_to_string(&path) {
      Ok(text) => Self::parse(path, &text)?,
      Err(source) if source.kind() == io::ErrorKind::NotFound => Self::absent(path),
      Err(source) => return Err(ProjectError::io("read", &path, source)),
    };
    config.families = FamilyCatalog::load(&home.detectors_dir(), &mut config.warnings)?;
    Ok(config)
  }

  pub fn path(&self) -> &Path {
    &self.path
  }

  pub fn exists(&self) -> bool {
    self.present
  }

  pub fn warnings(&self) -> &[String] {
    &self.warnings
  }

  pub(crate) fn families(&self) -> &FamilyCatalog {
    &self.families
  }

  pub fn route_families(&self) -> Vec<&str> {
    self.families.names().collect()
  }

  pub(crate) fn origin(&self) -> Origin {
    Origin::UserConfig { path: self.path.clone() }
  }

  pub(crate) fn default_exclude_entry(&self) -> ConfigEntry {
    let origin = match self.default_exclude.is_empty() {
      true => Origin::BuiltIn,
      false => self.origin(),
    };
    ConfigEntry {
      key: "defaults.exclude".to_owned(),
      value: Some(Value::Array(self.default_exclude.iter().cloned().map(Value::String).collect())),
      origins: vec![origin],
    }
  }

  pub(crate) fn reserved_entries(&self) -> Vec<ConfigEntry> {
    self
      .reserved
      .iter()
      .map(|(key, value)| ConfigEntry {
        key: key.clone(),
        value: Some(value.clone()),
        origins: vec![self.origin()],
      })
      .collect()
  }

  fn absent(path: PathBuf) -> Self {
    Self {
      path,
      present: false,
      defaults: Vec::new(),
      default_exclude: Vec::new(),
      mcp: Vec::new(),
      reserved: Vec::new(),
      families: FamilyCatalog::default(),
      warnings: Vec::new(),
    }
  }

  fn parse(path: PathBuf, text: &str) -> Result<Self, ProjectError> {
    let invalid = |message: String| ProjectError::ConfigParse { path: path.clone(), message };
    let table: toml::Table = toml::from_str(text).map_err(|error| invalid(atlas_detect::toml_error_summary(text, &error)))?;
    let mut config = Self {
      present: true,
      ..Self::absent(path.clone())
    };
    for (name, value) in &table {
      match (name.as_str(), value) {
        ("version", Value::Integer(SUPPORTED_VERSION)) => {}
        ("version", other) => {
          return Err(invalid(format!(
            "unsupported version {}; this catl reads version {SUPPORTED_VERSION}",
            atlas_detect::describe_value(other)
          )));
        }
        ("defaults", Value::Table(defaults)) => config.read_defaults(defaults).map_err(invalid)?,
        ("mcp", Value::Table(mcp)) => {
          let known = config.known("", flatten("mcp", mcp), |key| McpSettings::KEYS.iter().any(|spec| spec.name == key));
          config.mcp = validated::<McpSettings>(known, "").map_err(invalid)?;
        }
        ("agents" | "worktrees" | "hooks", Value::Table(section)) => {
          let known = config.known("", flatten(name, section), |key| RESERVED_KEYS.iter().any(|reserved| reserved.spec.name == key));
          config.reserved.extend(validated_reserved(known).map_err(invalid)?);
        }
        ("defaults" | "mcp" | "agents" | "worktrees" | "hooks", _) => return Err(invalid(format!("{name} must be a table"))),
        (other, _) => config.warnings.push(format!("config.toml: unknown key {other} ignored")),
      }
    }
    Ok(config)
  }

  fn known(&mut self, prefix: &str, entries: Vec<(String, Value)>, is_known: impl Fn(&str) -> bool) -> Vec<(String, Value)> {
    let (known, unknown): (Vec<_>, Vec<_>) = entries.into_iter().partition(|(key, _)| is_known(key));
    self
      .warnings
      .extend(unknown.into_iter().map(|(key, _)| format!("config.toml: unknown key {prefix}{key} ignored")));
    known
  }

  fn read_defaults(&mut self, defaults: &toml::Table) -> Result<(), String> {
    let (exclude, settings): (Vec<_>, Vec<_>) = flatten("", defaults).into_iter().partition(|(key, _)| key == "exclude");
    if let Some((_, value)) = exclude.first() {
      let globs = as_words(value).map_err(|message| format!("defaults.exclude: {message}"))?;
      PathGlobs::new(&globs).map_err(|error| format!("defaults.exclude: {error}"))?;
      self.default_exclude = globs;
    }
    let known = self.known("defaults.", settings, |key| ProjectSettings::KEYS.iter().any(|spec| spec.name == key));
    self.defaults = validated::<ProjectSettings>(known, "defaults.")?;
    Ok(())
  }
}

fn validated<T: Keyed>(entries: Vec<(String, Value)>, prefix: &str) -> Result<Vec<(String, Value)>, String> {
  let mut settings = T::default();
  for (key, value) in &entries {
    settings.set(key, value).map_err(|message| format!("{prefix}{key}: {message}"))?;
  }
  settings.check()?;
  Ok(entries)
}

fn validated_reserved(entries: Vec<(String, Value)>) -> Result<Vec<(String, Value)>, String> {
  for (key, value) in &entries {
    let Some(reserved) = RESERVED_KEYS.iter().find(|reserved| reserved.spec.name == key) else {
      continue;
    };
    reserved.check(value).map_err(|message| format!("{key}: {message}"))?;
  }
  Ok(entries)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn parse(text: &str) -> Result<UserConfig, String> {
    UserConfig::parse(PathBuf::from("/c/config.toml"), text).map_err(|error| error.to_string())
  }

  #[test]
  fn should_treat_a_missing_file_as_no_layer() {
    let dir = tempfile::tempdir().unwrap();
    let config = UserConfig::load(&AtlasHome::rooted_at(dir.path())).unwrap();
    assert!(!config.exists());
    assert_eq!((config.defaults.len(), config.default_exclude.len(), config.mcp.len()), (0, 0, 0));
  }

  #[test]
  fn should_parse_every_section_of_the_schema() {
    let config = parse(
      "version = 1\n[defaults]\nmax_file_bytes = 2048\nexclude = [\"**/*.gen.ts\"]\n[defaults.lsp]\nmax_servers = 3\n[defaults.llm]\nprovider = \"openai-compatible\"\nopenai_base_url = \"http://localhost:8080\"\napi_key_env = \"LOCAL_KEY\"\n[mcp]\nwatch = true\ntools = \"core\"\n[agents]\nbinary = \"catl\"\nguidance = \"auto\"\n[agents.claude]\nscope = \"user\"\n[worktrees]\nmode = \"overlay\"\ngc_after_days = 14\n",
    )
    .unwrap();

    assert_eq!(config.default_exclude, ["**/*.gen.ts"]);
    let keys: Vec<&str> = config.defaults.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys, ["llm.api_key_env", "llm.openai_base_url", "llm.provider", "lsp.max_servers", "max_file_bytes"]);
    assert_eq!(config.mcp.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(), ["mcp.tools", "mcp.watch"]);
    let reserved: Vec<String> = config.reserved_entries().into_iter().map(|entry| entry.key).collect();
    assert_eq!(
      reserved,
      ["agents.binary", "agents.claude.scope", "agents.guidance", "worktrees.gc_after_days", "worktrees.mode"]
    );
  }

  #[test]
  fn should_warn_about_unknown_keys_so_newer_files_still_load() {
    let config = parse(
      "version = 1\nbogus = 1\n[future]\nx = 1\n[defaults]\nbogus = 1\nmax_file_bytes = 2048\n[defaults.lsp]\nnew_knob = true\n[mcp]\nprofile_v2 = \"x\"\nwatch = true\n[agents.cursor]\nenabled = true\n[worktrees]\nshape = 1\n",
    )
    .unwrap();

    assert_eq!(config.defaults, [("max_file_bytes".to_owned(), Value::Integer(2048))]);
    assert_eq!(config.mcp, [("mcp.watch".to_owned(), Value::Boolean(true))]);
    assert_eq!(
      config.warnings(),
      [
        "config.toml: unknown key agents.cursor.enabled ignored",
        "config.toml: unknown key bogus ignored",
        "config.toml: unknown key defaults.bogus ignored",
        "config.toml: unknown key defaults.lsp.new_knob ignored",
        "config.toml: unknown key future ignored",
        "config.toml: unknown key mcp.profile_v2 ignored",
        "config.toml: unknown key worktrees.shape ignored",
      ]
    );
  }

  #[test]
  fn should_reject_bad_values_of_known_keys_and_future_versions_with_the_path() {
    for (text, message) in [
      ("[defaults.lsp]\nmax_servers = 0\n", "lsp.max_servers must be at least 1"),
      ("[defaults.lsp]\nunit = \"galaxy\"\n", "defaults.lsp.unit: expected \"project\" or \"repo\""),
      ("[defaults]\nexclude = \"dist\"\n", "defaults.exclude: expected an array of strings, got a string"),
      ("[mcp]\nmax_body_lines = 0\n", "mcp.max_body_lines: must be greater than 0"),
      (
        "[agents]\nbinary = \"./catl\"\n",
        "agents.binary: expected a command name such as \"catl\" or an absolute path",
      ),
      (
        "[defaults.llm]\napi_key_env = \"sk-ant-secret value\"\n",
        "llm.api_key_env must name an environment variable (letters, digits and underscores, not starting with a digit); the value is not shown in case it is a key",
      ),
      (
        "[defaults.llm]\nopenai_base_url = \"https://user:sk-secret@llm.example/v1\"\n",
        "llm.openai_base_url must not carry credentials (user:password@); name the key variable with llm.api_key_env instead",
      ),
      ("version = 2\n", "unsupported version 2; this catl reads version 1"),
      ("version = \"sk-FAKEKEY123\"\n", "unsupported version a string; this catl reads version 1"),
      ("mcp = 1\n", "mcp must be a table"),
    ] {
      assert_eq!(parse(text), Err(format!("invalid config file /c/config.toml: {message}")), "{text}");
    }
    assert!(parse("[defaults\n").unwrap_err().starts_with("invalid config file /c/config.toml: "));
  }
}
