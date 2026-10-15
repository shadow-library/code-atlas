use std::ffi::OsString;
use std::path::{Path, PathBuf};

use atlas_core::ProjectName;

use crate::error::ProjectError;

const APP_DIR: &str = "code-atlas";
const HOME_OVERRIDE_VAR: &str = "CODE_ATLAS_HOME";
const REGISTRY_FILE: &str = "projects.toml";
const CONFIG_FILE: &str = "config.toml";
const DETECTORS_DIR: &str = "detectors";
const DB_FILE: &str = "atlas.db";
const WIRING_FILE: &str = "wiring.json";
const HOOK_STATE_DIR: &str = "hooks";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasHome {
  config_dir: PathBuf,
  data_dir: PathBuf,
}

impl AtlasHome {
  pub fn resolve() -> Result<Self, ProjectError> {
    Self::from_env(|key| std::env::var_os(key), std::env::home_dir())
  }

  pub fn from_env(get: impl Fn(&str) -> Option<OsString>, home: Option<PathBuf>) -> Result<Self, ProjectError> {
    let non_empty = |key: &str| get(key).filter(|value| !value.is_empty());
    if let Some(root) = non_empty(HOME_OVERRIDE_VAR) {
      let root = std::path::absolute(PathBuf::from(root)).map_err(|source| ProjectError::io("resolve", HOME_OVERRIDE_VAR, source))?;
      return Ok(Self::rooted_at(root));
    }
    let xdg = |key: &str, fallback: &[&str]| -> Result<PathBuf, ProjectError> {
      if let Some(dir) = non_empty(key).map(PathBuf::from).filter(|dir| dir.is_absolute()) {
        return Ok(dir.join(APP_DIR));
      }
      let home = home.as_ref().ok_or(ProjectError::HomeNotFound)?;
      Ok(fallback.iter().fold(home.clone(), |path, segment| path.join(segment)).join(APP_DIR))
    };
    Ok(Self {
      config_dir: xdg("XDG_CONFIG_HOME", &[".config"])?,
      data_dir: xdg("XDG_DATA_HOME", &[".local", "share"])?,
    })
  }

  pub fn rooted_at(root: impl AsRef<Path>) -> Self {
    let root = root.as_ref();
    Self {
      config_dir: root.join("config"),
      data_dir: root.join("data"),
    }
  }

  pub fn config_dir(&self) -> &Path {
    &self.config_dir
  }

  pub fn data_dir(&self) -> &Path {
    &self.data_dir
  }

  pub fn registry_path(&self) -> PathBuf {
    self.config_dir.join(REGISTRY_FILE)
  }

  pub fn config_path(&self) -> PathBuf {
    self.config_dir.join(CONFIG_FILE)
  }

  pub fn detectors_dir(&self) -> PathBuf {
    self.config_dir.join(DETECTORS_DIR)
  }

  pub fn wiring_path(&self) -> PathBuf {
    self.data_dir.join(WIRING_FILE)
  }

  pub fn hook_state_dir(&self) -> PathBuf {
    self.data_dir.join(HOOK_STATE_DIR)
  }

  pub fn project_data_dir(&self, project: &ProjectName) -> PathBuf {
    self.data_dir.join(project.as_str())
  }

  pub fn db_path(&self, project: &ProjectName) -> PathBuf {
    self.project_data_dir(project).join(DB_FILE)
  }
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use super::*;

  fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, OsString> = pairs.iter().map(|(key, value)| ((*key).to_owned(), OsString::from(value))).collect();
    move |key| map.get(key).cloned()
  }

  #[test]
  fn should_use_code_atlas_home_for_config_and_data() {
    let home = AtlasHome::from_env(env(&[("CODE_ATLAS_HOME", "/iso"), ("XDG_CONFIG_HOME", "/xdg")]), Some("/home/u".into())).unwrap();
    assert_eq!(home.config_dir(), Path::new("/iso/config"));
    assert_eq!(home.data_dir(), Path::new("/iso/data"));
  }

  #[test]
  fn should_honour_xdg_variables() {
    let home = AtlasHome::from_env(env(&[("XDG_CONFIG_HOME", "/xc"), ("XDG_DATA_HOME", "/xd")]), Some("/home/u".into())).unwrap();
    assert_eq!(home.config_dir(), Path::new("/xc/code-atlas"));
    assert_eq!(home.data_dir(), Path::new("/xd/code-atlas"));
  }

  #[test]
  fn should_fall_back_to_home_when_xdg_is_unset_empty_or_relative() {
    for vars in [
      vec![],
      vec![("XDG_CONFIG_HOME", ""), ("XDG_DATA_HOME", "")],
      vec![("XDG_CONFIG_HOME", "rel"), ("XDG_DATA_HOME", "rel")],
    ] {
      let home = AtlasHome::from_env(env(&vars), Some("/home/u".into())).unwrap();
      assert_eq!(home.config_dir(), Path::new("/home/u/.config/code-atlas"));
      assert_eq!(home.data_dir(), Path::new("/home/u/.local/share/code-atlas"));
    }
  }

  #[test]
  fn should_fail_without_any_home() {
    assert!(matches!(AtlasHome::from_env(env(&[]), None), Err(ProjectError::HomeNotFound)));
  }

  #[test]
  fn should_place_each_project_database_under_its_own_directory() {
    let home = AtlasHome::rooted_at("/iso");
    let project = ProjectName::new("acme").unwrap();
    assert_eq!(home.db_path(&project), Path::new("/iso/data/acme/atlas.db"));
    assert_eq!(home.registry_path(), Path::new("/iso/config/projects.toml"));
    assert_eq!(home.config_path(), Path::new("/iso/config/config.toml"));
    assert_eq!(home.wiring_path(), Path::new("/iso/data/wiring.json"));
    assert_eq!(home.hook_state_dir(), Path::new("/iso/data/hooks"));
  }
}
