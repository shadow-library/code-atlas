use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::error::{LspError, LspResult};
use crate::position::{LineBreaks, PositionEncoding};
use crate::process::ServerCommand;
use crate::protocol::{ClientCapabilities, ClientInfo, InitializeParams, WorkspaceFolder};
use crate::rust_analyzer::{self, RustAnalyzerSettings};
use crate::typescript::{self, TypeScriptSettings};
use crate::uri::{canonical_path, uri_from_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServerKind {
  RustAnalyzer,
  TypeScript,
}

impl ServerKind {
  pub fn name(self) -> &'static str {
    match self {
      Self::RustAnalyzer => "rust-analyzer",
      Self::TypeScript => "typescript-language-server",
    }
  }

  pub fn line_breaks(self) -> LineBreaks {
    match self {
      Self::RustAnalyzer => LineBreaks::Lf,
      Self::TypeScript => LineBreaks::Ecma,
    }
  }

  pub fn readiness(self) -> Readiness {
    match self {
      Self::RustAnalyzer => Readiness::Quiescent {
        ignored_progress: rust_analyzer::IGNORED_PROGRESS,
      },
      Self::TypeScript => Readiness::Announced {
        notification: typescript::VERSION_NOTIFICATION,
      },
    }
  }

  pub fn language_id(self, path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?;
    match self {
      Self::RustAnalyzer => rust_analyzer::language_id(extension),
      Self::TypeScript => typescript::language_id(extension),
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
  Quiescent { ignored_progress: &'static [&'static str] },
  Announced { notification: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
  Installed,
  Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
  pub initialize: Duration,
  pub ready: Duration,
  pub announcement: Duration,
  pub request: Duration,
  pub first_request: Duration,
}

impl Timeouts {
  pub fn defaults(kind: ServerKind, launch: Launch) -> Self {
    let initialize = match launch {
      Launch::Installed => Duration::from_secs(60),
      Launch::Download => Duration::from_secs(300),
    };
    let (ready, first_request) = match kind {
      ServerKind::RustAnalyzer => (Duration::from_secs(300), Duration::from_secs(60)),
      ServerKind::TypeScript => (Duration::from_secs(120), Duration::from_secs(120)),
    };
    Self {
      initialize,
      ready,
      announcement: Duration::from_secs(5),
      request: Duration::from_secs(60),
      first_request,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnitScope {
  Project,
  Repo(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitKey {
  pub project: String,
  pub kind: ServerKind,
  pub scope: UnitScope,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitFolder {
  pub name: String,
  pub root: PathBuf,
}

impl UnitFolder {
  pub fn new(name: impl Into<String>, root: impl Into<PathBuf>) -> Self {
    Self {
      name: name.into(),
      root: root.into(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitSettings {
  RustAnalyzer(RustAnalyzerSettings),
  TypeScript(TypeScriptSettings),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerUnit {
  pub project: String,
  pub scope: UnitScope,
  pub folders: Vec<UnitFolder>,
  pub settings: UnitSettings,
  pub command: ServerCommand,
  pub launch: Launch,
  pub timeouts: Timeouts,
}

impl ServerUnit {
  pub fn new(project: impl Into<String>, folders: Vec<UnitFolder>, settings: UnitSettings, command: ServerCommand, launch: Launch) -> Self {
    let kind = match settings {
      UnitSettings::RustAnalyzer(_) => ServerKind::RustAnalyzer,
      UnitSettings::TypeScript(_) => ServerKind::TypeScript,
    };
    Self {
      project: project.into(),
      scope: UnitScope::Project,
      folders,
      settings,
      command,
      launch,
      timeouts: Timeouts::defaults(kind, launch),
    }
  }

  pub fn kind(&self) -> ServerKind {
    match self.settings {
      UnitSettings::RustAnalyzer(_) => ServerKind::RustAnalyzer,
      UnitSettings::TypeScript(_) => ServerKind::TypeScript,
    }
  }

  pub fn key(&self) -> UnitKey {
    UnitKey {
      project: self.project.clone(),
      kind: self.kind(),
      scope: self.scope.clone(),
    }
  }

  pub fn same_configuration(&self, other: &Self) -> bool {
    let mut mine = self.folders.clone();
    let mut theirs = other.folders.clone();
    mine.sort();
    theirs.sort();
    self.command == other.command && self.settings == other.settings && mine == theirs
  }

  pub fn spec(&self) -> LspResult<ServerSpec> {
    if self.folders.is_empty() {
      return Err(invalid(format!("{} unit of project {} has no workspace folders", self.kind().name(), self.project)));
    }
    let folders = self
      .folders
      .iter()
      .map(|folder| {
        Ok(UnitFolder {
          name: folder.name.clone(),
          root: canonical_path(&folder.root)?,
        })
      })
      .collect::<LspResult<Vec<_>>>()?;
    let (initialization_options, configuration) = match &self.settings {
      UnitSettings::RustAnalyzer(settings) => {
        settings.validate(&folders)?;
        let value = settings.to_json();
        (value.clone(), Some((rust_analyzer::SECTION, value)))
      }
      UnitSettings::TypeScript(settings) => (settings.initialization_options(), None),
    };
    Ok(ServerSpec {
      kind: self.kind(),
      folders,
      initialization_options,
      configuration,
    })
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerSpec {
  pub kind: ServerKind,
  pub folders: Vec<UnitFolder>,
  pub initialization_options: Value,
  pub configuration: Option<(&'static str, Value)>,
}

impl ServerSpec {
  pub fn line_breaks(&self) -> LineBreaks {
    self.kind.line_breaks()
  }

  pub fn readiness(&self) -> Readiness {
    self.kind.readiness()
  }

  pub fn capabilities(&self) -> ClientCapabilities {
    ClientCapabilities::standard(vec![PositionEncoding::Utf8, PositionEncoding::Utf16])
  }

  pub fn workspace_folders(&self) -> LspResult<Vec<WorkspaceFolder>> {
    self
      .folders
      .iter()
      .map(|folder| {
        Ok(WorkspaceFolder {
          uri: uri_from_path(&folder.root)?,
          name: folder.name.clone(),
        })
      })
      .collect()
  }

  pub fn configuration_for(&self, section: Option<&str>) -> Value {
    match (&self.configuration, section) {
      (Some((name, value)), Some(section)) if *name == section => value.clone(),
      _ => Value::Null,
    }
  }

  pub fn contains(&self, path: &Path) -> bool {
    self.folders.iter().any(|folder| path.starts_with(&folder.root))
  }

  pub fn initialize_params(&self) -> LspResult<InitializeParams> {
    let workspace_folders = self.workspace_folders()?;
    Ok(InitializeParams {
      process_id: Some(std::process::id()),
      client_info: Some(ClientInfo {
        name: "code-atlas".to_owned(),
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
      }),
      root_uri: workspace_folders.first().map(|folder| folder.uri.clone()),
      capabilities: self.capabilities(),
      initialization_options: Some(self.initialization_options.clone()),
      workspace_folders: Some(workspace_folders),
    })
  }
}

pub(crate) fn invalid(reason: String) -> LspError {
  LspError::InvalidUnit { reason }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;
  use crate::rust_analyzer::CheckOnSave;

  fn rust_unit(root: &Path) -> ServerUnit {
    let settings = RustAnalyzerSettings::new(vec![root.join("Cargo.toml")], root.parent().unwrap().join("lsp-target"));
    ServerUnit::new(
      "demo",
      vec![UnitFolder::new("core", root)],
      UnitSettings::RustAnalyzer(settings),
      ServerCommand::new("rust-analyzer"),
      Launch::Installed,
    )
  }

  #[test]
  fn should_pick_line_breaks_and_language_ids_per_server() {
    assert_eq!(ServerKind::RustAnalyzer.line_breaks(), LineBreaks::Lf);
    assert_eq!(ServerKind::TypeScript.line_breaks(), LineBreaks::Ecma);
    assert_eq!(ServerKind::RustAnalyzer.language_id(Path::new("/w/src/lib.rs")), Some("rust"));
    assert_eq!(ServerKind::RustAnalyzer.language_id(Path::new("/w/Cargo.toml")), None);
    assert_eq!(ServerKind::TypeScript.language_id(Path::new("/w/a.tsx")), Some("typescriptreact"));
    assert_eq!(ServerKind::TypeScript.language_id(Path::new("/w/a.mts")), Some("typescript"));
    assert_eq!(ServerKind::TypeScript.language_id(Path::new("/w/a.cjs")), Some("javascript"));
    assert_eq!(ServerKind::TypeScript.language_id(Path::new("/w/tsconfig.json")), None);
  }

  #[test]
  fn should_lengthen_initialize_for_a_download_launch() {
    assert_eq!(Timeouts::defaults(ServerKind::TypeScript, Launch::Installed).initialize, Duration::from_secs(60));
    assert_eq!(Timeouts::defaults(ServerKind::TypeScript, Launch::Download).initialize, Duration::from_secs(300));
    assert_eq!(Timeouts::defaults(ServerKind::RustAnalyzer, Launch::Installed).ready, Duration::from_secs(300));
    assert_eq!(Timeouts::defaults(ServerKind::TypeScript, Launch::Installed).first_request, Duration::from_secs(120));
  }

  #[test]
  fn should_build_rust_analyzer_initialize_params_from_the_unit() {
    let work = tempfile::tempdir().unwrap();
    let root = canonical_path(&work.path().join("core")).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let spec = rust_unit(&root).spec().unwrap();

    let params = serde_json::to_value(spec.initialize_params().unwrap()).unwrap();

    let root_uri = uri_from_path(&root).unwrap();
    assert_eq!(params["rootUri"], json!(root_uri));
    assert_eq!(params["workspaceFolders"], json!([{"uri": root_uri, "name": "core"}]));
    assert_eq!(params["clientInfo"]["name"], json!("code-atlas"));
    assert_eq!(params["initializationOptions"]["linkedProjects"], json!([root.join("Cargo.toml")]));
    assert_eq!(params["initializationOptions"]["checkOnSave"], json!(false));
    assert_eq!(spec.configuration_for(Some("rust-analyzer")), params["initializationOptions"]);
    assert_eq!(spec.configuration_for(Some("editor")), Value::Null);
    assert_eq!(spec.configuration_for(None), Value::Null);
  }

  #[test]
  fn should_refuse_a_unit_without_folders_or_with_a_target_dir_inside_a_folder() {
    let work = tempfile::tempdir().unwrap();
    let root = canonical_path(work.path()).unwrap();
    let mut empty = rust_unit(&root.join("core"));
    empty.folders.clear();
    let mut inside = rust_unit(&root);
    inside.settings = UnitSettings::RustAnalyzer(RustAnalyzerSettings::new(vec![root.join("Cargo.toml")], root.join("target")));

    assert_eq!(
      empty.spec().unwrap_err().to_string(),
      "invalid language server unit: rust-analyzer unit of project demo has no workspace folders"
    );
    assert!(
      matches!(inside.spec(), Err(LspError::InvalidUnit { ref reason }) if reason.contains("inside workspace folder")),
      "{:?}",
      inside.spec()
    );
  }

  #[test]
  fn should_compare_unit_configuration_ignoring_folder_order_and_timeouts() {
    let unit = rust_unit(Path::new("/w/core"));
    let mut reordered = unit.clone();
    reordered.folders.push(UnitFolder::new("api", "/w/api"));
    let mut swapped = unit.clone();
    swapped.folders.insert(0, UnitFolder::new("api", "/w/api"));
    let mut slower = unit.clone();
    slower.timeouts.ready = Duration::from_secs(900);
    let mut checking = unit.clone();
    if let UnitSettings::RustAnalyzer(settings) = &mut checking.settings {
      settings.check_on_save = CheckOnSave::On;
    }

    assert!(reordered.same_configuration(&swapped));
    assert!(unit.same_configuration(&slower));
    assert!(!unit.same_configuration(&checking));
    assert!(!unit.same_configuration(&reordered));
  }
}
