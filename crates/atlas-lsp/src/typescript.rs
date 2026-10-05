use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Map, Value, json};

pub const VERSION_NOTIFICATION: &str = "$/typescriptVersion";
pub const DEFAULT_MAX_TSSERVER_MEMORY_MB: u32 = 3072;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeScriptSettings {
  pub tsserver_path: Option<PathBuf>,
  pub max_tsserver_memory_mb: u32,
}

impl Default for TypeScriptSettings {
  fn default() -> Self {
    Self {
      tsserver_path: None,
      max_tsserver_memory_mb: DEFAULT_MAX_TSSERVER_MEMORY_MB,
    }
  }
}

impl TypeScriptSettings {
  pub fn initialization_options(&self) -> Value {
    let mut tsserver = Map::new();
    if let Some(path) = &self.tsserver_path {
      tsserver.insert("path".to_owned(), json!(path));
    }
    tsserver.insert("logVerbosity".to_owned(), json!("off"));
    // With the default "auto", a partial-semantic syntax server answers while the project loads, so references would only cover open files.
    tsserver.insert("useSyntaxServer".to_owned(), json!("never"));
    json!({
      "hostInfo": "code-atlas",
      "tsserver": tsserver,
      "maxTsServerMemory": self.max_tsserver_memory_mb,
      "preferences": {"providePrefixAndSuffixTextForRename": true, "allowRenameOfImportPath": false}
    })
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TypeScriptVersion {
  pub version: String,
  pub source: Option<String>,
}

pub(crate) fn language_id(extension: &str) -> Option<&'static str> {
  match extension {
    "ts" | "mts" | "cts" => Some("typescript"),
    "tsx" => Some("typescriptreact"),
    "js" | "mjs" | "cjs" => Some("javascript"),
    "jsx" => Some("javascriptreact"),
    _ => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_render_typescript_initialization_options() {
    let bundled = TypeScriptSettings::default();
    let workspace = TypeScriptSettings {
      tsserver_path: Some(PathBuf::from("/w/node_modules/typescript/lib/tsserver.js")),
      max_tsserver_memory_mb: 1024,
    };

    assert_eq!(
      bundled.initialization_options(),
      json!({
        "hostInfo": "code-atlas",
        "tsserver": {"logVerbosity": "off", "useSyntaxServer": "never"},
        "maxTsServerMemory": 3072,
        "preferences": {"providePrefixAndSuffixTextForRename": true, "allowRenameOfImportPath": false}
      })
    );
    assert_eq!(
      workspace.initialization_options()["tsserver"],
      json!({"path": "/w/node_modules/typescript/lib/tsserver.js", "logVerbosity": "off", "useSyntaxServer": "never"})
    );
    assert_eq!(workspace.initialization_options()["maxTsServerMemory"], json!(1024));
  }
}
