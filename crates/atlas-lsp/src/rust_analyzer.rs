use std::path::PathBuf;

use serde_json::{Value, json};

use crate::error::LspResult;
use crate::spec::{UnitFolder, invalid};
use crate::uri::canonical_path;

pub const SECTION: &str = "rust-analyzer";

/// Flycheck (`cargo check`) and cache priming report progress but do not make answers partial, so they never hold readiness back.
pub(crate) const IGNORED_PROGRESS: &[&str] = &["rust-analyzer/flycheck", "rustAnalyzer/cachePriming"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CheckOnSave {
  #[default]
  Off,
  On,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustAnalyzerSettings {
  pub linked_projects: Vec<PathBuf>,
  pub target_dir: PathBuf,
  pub check_on_save: CheckOnSave,
}

impl RustAnalyzerSettings {
  pub fn new(linked_projects: Vec<PathBuf>, target_dir: impl Into<PathBuf>) -> Self {
    Self {
      linked_projects,
      target_dir: target_dir.into(),
      check_on_save: CheckOnSave::Off,
    }
  }

  pub fn to_json(&self) -> Value {
    json!({
      "linkedProjects": self.linked_projects,
      "cargo": {"targetDir": self.target_dir, "buildScripts": {"enable": true}},
      "procMacro": {"enable": true},
      "checkOnSave": self.check_on_save == CheckOnSave::On,
      "check": {"command": "check"},
      "files": {"watcher": "server"},
      "cachePriming": {"enable": true}
    })
  }

  pub fn lockfiles(&self) -> Vec<PathBuf> {
    let mut lockfiles: Vec<PathBuf> = self
      .linked_projects
      .iter()
      .filter_map(|manifest| manifest.parent())
      .map(|dir| dir.join("Cargo.lock"))
      .collect();
    lockfiles.sort();
    lockfiles.dedup();
    lockfiles
  }

  pub(crate) fn validate(&self, folders: &[UnitFolder]) -> LspResult<()> {
    if let Some(relative) = self.linked_projects.iter().find(|manifest| !manifest.is_absolute()) {
      return Err(invalid(format!("linked project {} is not an absolute path", relative.display())));
    }
    if !self.target_dir.is_absolute() {
      return Err(invalid(format!("cargo target dir {} is not an absolute path", self.target_dir.display())));
    }
    let target_dir = canonical_path(&self.target_dir)?;
    match folders.iter().find(|folder| target_dir.starts_with(&folder.root)) {
      Some(folder) => Err(invalid(format!(
        "cargo target dir {} is inside workspace folder {}; rust-analyzer must build outside the repositories",
        target_dir.display(),
        folder.root.display()
      ))),
      None => Ok(()),
    }
  }
}

pub(crate) fn language_id(extension: &str) -> Option<&'static str> {
  match extension {
    "rs" => Some("rust"),
    _ => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_render_the_rust_analyzer_settings() {
    let settings = RustAnalyzerSettings::new(vec![PathBuf::from("/w/api/Cargo.toml"), PathBuf::from("/w/core/Cargo.toml")], "/cache/lsp-target/abc");

    assert_eq!(
      settings.to_json(),
      json!({
        "linkedProjects": ["/w/api/Cargo.toml", "/w/core/Cargo.toml"],
        "cargo": {"targetDir": "/cache/lsp-target/abc", "buildScripts": {"enable": true}},
        "procMacro": {"enable": true},
        "checkOnSave": false,
        "check": {"command": "check"},
        "files": {"watcher": "server"},
        "cachePriming": {"enable": true}
      })
    );
    assert_eq!(
      RustAnalyzerSettings {
        check_on_save: CheckOnSave::On,
        ..settings
      }
      .to_json()["checkOnSave"],
      json!(true)
    );
  }

  #[test]
  fn should_list_one_lockfile_per_linked_project_directory() {
    let settings = RustAnalyzerSettings::new(
      vec![PathBuf::from("/w/core/Cargo.toml"), PathBuf::from("/w/api/Cargo.toml"), PathBuf::from("/w/core/Cargo.toml")],
      "/cache",
    );

    assert_eq!(settings.lockfiles(), vec![PathBuf::from("/w/api/Cargo.lock"), PathBuf::from("/w/core/Cargo.lock")]);
  }

  #[test]
  fn should_refuse_relative_paths() {
    let folders = [UnitFolder::new("core", "/w/core")];
    let relative_manifest = RustAnalyzerSettings::new(vec![PathBuf::from("core/Cargo.toml")], "/cache");
    let relative_target = RustAnalyzerSettings::new(vec![PathBuf::from("/w/core/Cargo.toml")], "cache");

    assert_eq!(
      relative_manifest.validate(&folders).unwrap_err().to_string(),
      "invalid language server unit: linked project core/Cargo.toml is not an absolute path"
    );
    assert_eq!(
      relative_target.validate(&folders).unwrap_err().to_string(),
      "invalid language server unit: cargo target dir cache is not an absolute path"
    );
  }
}
