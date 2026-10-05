use std::fs;
use std::path::PathBuf;

use atlas_lsp::{DiscoveryStep, StepOutcome, TsServerSource, TypeScriptDiscovery, discover_typescript};
use atlas_project::{LspSettings, Project};
use atlas_scip::locate_rust_analyzer;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepLine {
  pub label: String,
  pub status: &'static str,
  pub detail: String,
}

impl StepLine {
  fn of(step: &DiscoveryStep) -> Self {
    let (status, detail) = match &step.outcome {
      StepOutcome::Found { detail } => ("found", detail.clone()),
      StepOutcome::Missing { reason } => ("missing", reason.clone()),
      StepOutcome::Skipped { reason } => ("skipped", reason.clone()),
    };
    Self {
      label: step.label.clone(),
      status,
      detail,
    }
  }

  pub fn text(&self) -> String {
    format!("{}: {} ({})", self.label, self.status, self.detail)
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Resolved {
  Found { summary: String },
  Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeScriptProbe {
  pub steps: Vec<StepLine>,
  pub result: Resolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RustAnalyzerProbe {
  pub result: Resolved,
}

pub fn typescript_roots(project: &Project) -> Vec<PathBuf> {
  project
    .repos
    .iter()
    .filter(|repo| repo.path.join("package.json").is_file() || repo.path.join("tsconfig.json").is_file())
    .map(|repo| repo.path.clone())
    .collect()
}

pub fn probe_typescript(settings: &LspSettings, roots: &[PathBuf], download: bool) -> TypeScriptProbe {
  let discovery = discover_typescript(&TypeScriptDiscovery {
    command: settings.typescript_command.as_deref(),
    repo_roots: roots,
    download,
  });
  let result = match discovery.result {
    Ok(found) => {
      let tsserver = match found.tsserver {
        Some(choice) => match choice.source {
          TsServerSource::Workspace { repo_root } => format!(", TypeScript {} from {}", choice.version, repo_root.display()),
          TsServerSource::Bundled => format!(", TypeScript {} bundled", choice.version),
        },
        None => String::new(),
      };
      Resolved::Found {
        summary: format!("{} (server {}{tsserver})", found.command.program.display(), found.server_version),
      }
    }
    Err(error) => Resolved::Unavailable { reason: error.to_string() },
  };
  TypeScriptProbe {
    steps: discovery.steps.iter().map(StepLine::of).collect(),
    result,
  }
}

pub fn probe_rust_analyzer() -> RustAnalyzerProbe {
  let result = match locate_rust_analyzer() {
    Ok((path, version)) => Resolved::Found {
      summary: format!("{} ({version})", path.display()),
    },
    Err(reason) => Resolved::Unavailable { reason },
  };
  RustAnalyzerProbe { result }
}

#[derive(Deserialize)]
struct PackageVersion {
  version: Option<String>,
}

pub fn typescript_seven_repos(project: &Project) -> Vec<(String, String)> {
  project
    .repos
    .iter()
    .filter_map(|repo| {
      let text = fs::read_to_string(repo.path.join("node_modules/typescript/package.json")).ok()?;
      let version = serde_json::from_str::<PackageVersion>(&text).ok()?.version?;
      let major: u32 = version.split('.').next()?.parse().ok()?;
      (major >= 7).then(|| (repo.name.to_string(), version))
    })
    .collect()
}
