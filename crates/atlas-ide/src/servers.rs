use std::path::{Path, PathBuf};
use std::time::Duration;

use atlas_core::{ContentHash, RepoName};
use atlas_engine::LanguageUnits;
use atlas_lsp::{
  Launch, LspError, PoolOptions, RustAnalyzerSettings, ServerCommand, ServerKind, ServerUnit, SyncLimits, TsServerSource, TypeScriptDiscovery, TypeScriptSettings, UnitFolder,
  UnitScope, UnitSettings, canonical_path, discover_typescript, rust_analyzer_server,
};
use atlas_project::{LspSettings, LspUnit};
use atlas_scip::locate_rust_analyzer;
use serde::Serialize;

use crate::error::{IdeError, IdeResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitMode {
  Project,
  Repo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeSettings {
  pub enabled: bool,
  pub download: bool,
  pub typescript_command: Option<Vec<String>>,
  pub unit: UnitMode,
  pub max_servers: usize,
  pub idle_secs: u64,
  pub ready_timeout_secs: u64,
  pub check_timeout_secs: u64,
}

impl Default for IdeSettings {
  fn default() -> Self {
    Self {
      enabled: true,
      download: true,
      typescript_command: None,
      unit: UnitMode::Project,
      max_servers: 2,
      idle_secs: 600,
      ready_timeout_secs: 300,
      check_timeout_secs: 120,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdeMode {
  Warm,
  OneShot,
}

impl IdeMode {
  pub(crate) fn pool_options(self, settings: &IdeSettings, sync: SyncLimits) -> PoolOptions {
    let base = match self {
      Self::Warm => PoolOptions::warm(),
      Self::OneShot => PoolOptions::one_shot(),
    };
    PoolOptions {
      max_servers: settings.max_servers.max(1),
      idle_timeout: Duration::from_secs(settings.idle_secs),
      sync,
      ..base
    }
  }
}

pub struct LaunchRequest<'a> {
  pub kind: ServerKind,
  pub repo_roots: &'a [PathBuf],
  pub settings: &'a IdeSettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TsServerOrigin {
  Workspace { repo_root: PathBuf },
  Bundled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsServer {
  pub path: Option<PathBuf>,
  pub version: String,
  pub origin: TsServerOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
  pub command: ServerCommand,
  pub launch: Launch,
  pub server_version: String,
  pub tsserver: Option<TsServer>,
}

pub trait Launcher: Send + Sync {
  fn launch(&self, request: &LaunchRequest<'_>) -> IdeResult<LaunchSpec>;
}

impl From<&LspSettings> for IdeSettings {
  fn from(settings: &LspSettings) -> Self {
    Self {
      enabled: settings.enabled,
      download: settings.download,
      typescript_command: settings.typescript_command.clone(),
      unit: match settings.unit {
        LspUnit::Project => UnitMode::Project,
        LspUnit::Repo => UnitMode::Repo,
      },
      max_servers: settings.max_servers,
      idle_secs: settings.idle_secs,
      ready_timeout_secs: settings.ready_timeout_secs,
      check_timeout_secs: settings.check_timeout_secs,
    }
  }
}

const RUST_ANALYZER_HINT: &str = "install it with `rustup component add rust-analyzer`";

pub struct DiscoveryLauncher;

impl Launcher for DiscoveryLauncher {
  fn launch(&self, request: &LaunchRequest<'_>) -> IdeResult<LaunchSpec> {
    let discovered = match request.kind {
      ServerKind::RustAnalyzer => {
        let (binary, version) = locate_rust_analyzer().map_err(|reason| unavailable(request.kind, format!("{reason}; {RUST_ANALYZER_HINT}")))?;
        rust_analyzer_server(binary, version)
      }
      ServerKind::TypeScript => {
        let discovery = discover_typescript(&TypeScriptDiscovery {
          command: request.settings.typescript_command.as_deref(),
          repo_roots: request.repo_roots,
          download: request.settings.download,
        });
        for step in &discovery.steps {
          tracing::debug!(step = step.label, outcome = ?step.outcome, "typescript-language-server discovery");
        }
        match discovery.result {
          Ok(discovered) => discovered,
          Err(LspError::Unavailable(reason)) => return Err(unavailable(request.kind, reason)),
          Err(error) => return Err(error.into()),
        }
      }
    };
    Ok(LaunchSpec {
      command: discovered.command,
      launch: discovered.launch,
      server_version: discovered.server_version,
      tsserver: discovered.tsserver.map(|choice| TsServer {
        path: choice.path,
        version: choice.version,
        origin: match choice.source {
          TsServerSource::Workspace { repo_root } => TsServerOrigin::Workspace { repo_root },
          TsServerSource::Bundled => TsServerOrigin::Bundled,
        },
      }),
    })
  }
}

pub(crate) fn kind_of(path: &Path) -> Option<ServerKind> {
  [ServerKind::RustAnalyzer, ServerKind::TypeScript].into_iter().find(|kind| kind.language_id(path).is_some())
}

#[derive(Debug, Clone)]
pub(crate) struct UnitPlan {
  pub unit: ServerUnit,
  pub roots: Vec<PathBuf>,
  pub cargo_dirs: Vec<PathBuf>,
}

pub(crate) struct UnitInputs<'a> {
  pub project: &'a str,
  pub kind: ServerKind,
  pub scope: &'a UnitScope,
  pub units: &'a LanguageUnits,
  pub settings: &'a IdeSettings,
  pub cache_dir: &'a Path,
}

impl UnitInputs<'_> {
  fn admits(&self, repo: &RepoName) -> bool {
    match self.scope {
      UnitScope::Project => true,
      UnitScope::Repo(name) => repo.as_str() == name,
    }
  }

  pub(crate) fn folders(&self) -> Vec<UnitFolder> {
    let folders = match self.kind {
      ServerKind::RustAnalyzer => self.units.rust.as_ref().map(|unit| &unit.folders),
      ServerKind::TypeScript => self.units.typescript.as_ref().map(|unit| &unit.folders),
    };
    folders
      .into_iter()
      .flatten()
      .filter(|folder| self.admits(&folder.repo))
      .map(|folder| UnitFolder::new(folder.repo.as_str(), &folder.root))
      .collect()
  }

  fn linked_projects(&self) -> Vec<PathBuf> {
    let projects = self.units.rust.as_ref().map(|unit| &unit.linked_projects);
    projects
      .into_iter()
      .flatten()
      .filter(|project| self.admits(&project.repo))
      .map(|project| project.path.clone())
      .collect()
  }

  pub(crate) fn plan(&self, launch: &LaunchSpec) -> IdeResult<UnitPlan> {
    let folders = self.folders();
    let roots = folders.iter().map(|folder| canonical(&folder.root)).collect::<IdeResult<Vec<_>>>()?;
    let (settings, cargo_dirs) = match self.kind {
      ServerKind::RustAnalyzer => {
        let linked = self.linked_projects().iter().map(canonical).collect::<IdeResult<Vec<_>>>()?;
        let cargo_dirs: Vec<PathBuf> = linked.iter().filter_map(|manifest| manifest.parent()).map(Path::to_path_buf).collect();
        let target_dir = self.cache_dir.join("lsp-target").join(fingerprint(&roots, &linked));
        (UnitSettings::RustAnalyzer(RustAnalyzerSettings::new(linked, target_dir)), cargo_dirs)
      }
      ServerKind::TypeScript => {
        let settings = TypeScriptSettings {
          tsserver_path: launch.tsserver.as_ref().and_then(|tsserver| tsserver.path.clone()),
          ..TypeScriptSettings::default()
        };
        (UnitSettings::TypeScript(settings), Vec::new())
      }
    };
    let mut unit = ServerUnit::new(self.project, folders, settings, launch.command.clone(), launch.launch);
    unit.scope = self.scope.clone();
    unit.timeouts.ready = Duration::from_secs(self.settings.ready_timeout_secs);
    Ok(UnitPlan { unit, roots, cargo_dirs })
  }
}

fn canonical(path: impl AsRef<Path>) -> IdeResult<PathBuf> {
  Ok(canonical_path(path.as_ref()).map_err(atlas_lsp::LspError::from)?)
}

fn fingerprint(roots: &[PathBuf], linked: &[PathBuf]) -> String {
  let mut parts: Vec<String> = roots.iter().chain(linked).map(|path| path.display().to_string()).collect();
  parts.sort();
  ContentHash::of(parts.join("\n").as_bytes()).to_string()[..16].to_owned()
}

impl UnitPlan {
  pub(crate) fn covers(&self, file: &Path) -> Result<(), String> {
    if !self.roots.iter().any(|root| file.starts_with(root)) {
      return Err("its repo is not one of the server's workspace folders (the index has no files of this language there)".to_owned());
    }
    if self.unit.kind() == ServerKind::RustAnalyzer && !self.cargo_dirs.iter().any(|dir| file.starts_with(dir)) {
      return Err("no Cargo workspace or package found by the index contains it".to_owned());
    }
    Ok(())
  }
}

pub(crate) fn unavailable(kind: ServerKind, reason: impl Into<String>) -> IdeError {
  IdeError::Unavailable {
    server: kind.name().to_owned(),
    reason: reason.into(),
    alternative: None,
  }
}

#[cfg(test)]
mod tests {
  use atlas_engine::{CargoProject, RustUnit, TypeScriptUnit, UnitFolder as EngineFolder};

  use super::*;

  fn repo(name: &str) -> RepoName {
    RepoName::new(name).unwrap()
  }

  fn units(root: &Path) -> LanguageUnits {
    LanguageUnits {
      rust: Some(RustUnit {
        folders: vec![
          EngineFolder {
            repo: repo("core"),
            root: root.join("core"),
          },
          EngineFolder {
            repo: repo("api"),
            root: root.join("api"),
          },
        ],
        linked_projects: vec![
          CargoProject {
            repo: repo("core"),
            manifest: atlas_core::RelPath::new("Cargo.toml").unwrap(),
            path: root.join("core/Cargo.toml"),
          },
          CargoProject {
            repo: repo("api"),
            manifest: atlas_core::RelPath::new("Cargo.toml").unwrap(),
            path: root.join("api/Cargo.toml"),
          },
        ],
        undiscovered: Vec::new(),
      }),
      typescript: Some(TypeScriptUnit {
        folders: vec![EngineFolder {
          repo: repo("web"),
          root: root.join("web"),
        }],
      }),
    }
  }

  fn launch() -> LaunchSpec {
    LaunchSpec {
      command: ServerCommand::new("/bin/server"),
      launch: Launch::Installed,
      server_version: "1.0".to_owned(),
      tsserver: Some(TsServer {
        path: Some(PathBuf::from("/w/node_modules/typescript/lib/tsserver.js")),
        version: "5.9.3".to_owned(),
        origin: TsServerOrigin::Workspace { repo_root: PathBuf::from("/w") },
      }),
    }
  }

  #[test]
  fn should_link_every_cargo_project_and_build_outside_the_repos() {
    let work = tempfile::tempdir().unwrap();
    let root = canonical_path(work.path()).unwrap();
    let units = units(&root);
    let settings = IdeSettings::default();
    let cache = root.join("cache");
    let inputs = UnitInputs {
      project: "demo",
      kind: ServerKind::RustAnalyzer,
      scope: &UnitScope::Project,
      units: &units,
      settings: &settings,
      cache_dir: &cache,
    };

    let plan = inputs.plan(&launch()).unwrap();

    let UnitSettings::RustAnalyzer(rust) = &plan.unit.settings else {
      panic!("expected rust-analyzer settings")
    };
    assert_eq!(rust.linked_projects, vec![root.join("core/Cargo.toml"), root.join("api/Cargo.toml")]);
    assert!(rust.target_dir.starts_with(cache.join("lsp-target")), "{}", rust.target_dir.display());
    assert_eq!(
      plan.unit.folders,
      vec![UnitFolder::new("core", root.join("core")), UnitFolder::new("api", root.join("api"))]
    );
    assert_eq!(plan.unit.timeouts.ready, Duration::from_secs(300));
    assert_eq!(plan.covers(&root.join("api/src/lib.rs")), Ok(()));
    assert!(plan.covers(&root.join("web/src/a.ts")).is_err());
  }

  #[test]
  fn should_scope_a_repo_unit_to_that_repo_and_pass_the_chosen_tsserver() {
    let work = tempfile::tempdir().unwrap();
    let root = canonical_path(work.path()).unwrap();
    let units = units(&root);
    let settings = IdeSettings {
      unit: UnitMode::Repo,
      ..IdeSettings::default()
    };
    let scope = UnitScope::Repo("api".to_owned());
    let rust = UnitInputs {
      project: "demo",
      kind: ServerKind::RustAnalyzer,
      scope: &scope,
      units: &units,
      settings: &settings,
      cache_dir: &root.join("cache"),
    }
    .plan(&launch())
    .unwrap();
    let typescript = UnitInputs {
      project: "demo",
      kind: ServerKind::TypeScript,
      scope: &UnitScope::Project,
      units: &units,
      settings: &settings,
      cache_dir: &root.join("cache"),
    }
    .plan(&launch())
    .unwrap();

    let UnitSettings::RustAnalyzer(linked) = &rust.unit.settings else {
      panic!("expected rust-analyzer settings")
    };
    assert_eq!(linked.linked_projects, vec![root.join("api/Cargo.toml")]);
    assert_eq!(rust.unit.scope, scope);
    assert_eq!(
      typescript.unit.settings,
      UnitSettings::TypeScript(TypeScriptSettings {
        tsserver_path: Some(PathBuf::from("/w/node_modules/typescript/lib/tsserver.js")),
        ..TypeScriptSettings::default()
      })
    );
  }

  #[test]
  fn should_take_the_project_lsp_settings() {
    let settings = LspSettings {
      enabled: false,
      download: false,
      typescript_command: Some(vec!["/opt/tls".to_owned(), "--stdio".to_owned()]),
      unit: LspUnit::Repo,
      max_servers: 3,
      idle_secs: 60,
      ready_timeout_secs: 90,
      check_timeout_secs: 30,
    };

    assert_eq!(
      IdeSettings::from(&settings),
      IdeSettings {
        enabled: false,
        download: false,
        typescript_command: Some(vec!["/opt/tls".to_owned(), "--stdio".to_owned()]),
        unit: UnitMode::Repo,
        max_servers: 3,
        idle_secs: 60,
        ready_timeout_secs: 90,
        check_timeout_secs: 30,
      }
    );
    assert_eq!(IdeSettings::from(&LspSettings::default()), IdeSettings::default());
  }

  #[test]
  fn should_surface_an_unusable_configured_typescript_command_verbatim() {
    let settings = IdeSettings {
      typescript_command: Some(vec!["/nonexistent/typescript-language-server".to_owned()]),
      download: false,
      ..IdeSettings::default()
    };

    let error = DiscoveryLauncher
      .launch(&LaunchRequest {
        kind: ServerKind::TypeScript,
        repo_roots: &[],
        settings: &settings,
      })
      .unwrap_err()
      .to_string();

    assert!(
      error.starts_with("typescript-language-server is unavailable: lsp.typescript_command (/nonexistent/typescript-language-server): "),
      "{error}"
    );
  }

  #[test]
  fn should_pick_the_server_by_file_extension() {
    assert_eq!(kind_of(Path::new("/w/src/lib.rs")), Some(ServerKind::RustAnalyzer));
    assert_eq!(kind_of(Path::new("/w/src/a.tsx")), Some(ServerKind::TypeScript));
    assert_eq!(kind_of(Path::new("/w/src/a.mjs")), Some(ServerKind::TypeScript));
    assert_eq!(kind_of(Path::new("/w/README.md")), None);
  }
}
