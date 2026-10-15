use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use atlas_engine::{BoundProject, Engine, EngineOptions, bind_project, current_overlay};
use atlas_project::{AtlasHome, Binding, Project, Registry, Resolution};
use atlas_query::QueryEngine;
use atlas_store::StoreOptions;

use crate::error::{CliError, CliResult};
use crate::output::note;

pub struct Context {
  pub home: AtlasHome,
  pub cwd: PathBuf,
  pub project: Option<String>,
  warned: AtomicBool,
  bound: OnceLock<BoundProject>,
}

impl Context {
  pub fn load(project: Option<String>) -> CliResult<Self> {
    let cwd = std::env::current_dir().map_err(|source| CliError::io("read the working directory of", ".", source))?;
    Ok(Self {
      home: AtlasHome::resolve()?,
      cwd,
      project,
      warned: AtomicBool::new(false),
      bound: OnceLock::new(),
    })
  }

  pub fn registry(&self) -> CliResult<Registry> {
    Ok(Registry::load(&self.home)?)
  }

  pub fn resolve(&self) -> CliResult<Project> {
    let bound = self.bound()?;
    if let Some(caveat) = bound.binding.caveat() {
      note(caveat);
    }
    Ok(bound.project.clone())
  }

  pub fn resolution(&self) -> CliResult<Resolution> {
    let registry = self.registry()?;
    let resolution = registry.resolve(&self.cwd, self.project.as_deref())?;
    let mut warnings = registry.warnings(Some(resolution.project.name.as_str()));
    warnings.extend(resolution.warnings.iter().cloned());
    self.warn_once(&warnings);
    Ok(resolution)
  }

  // One binding per command: the edit refusal check, the engine and the language servers must all see the same checkout.
  pub fn bound(&self) -> CliResult<&BoundProject> {
    if let Some(bound) = self.bound.get() {
      return Ok(bound);
    }
    let registry = self.registry()?;
    let bound = bind_project(&self.home, &registry, &self.cwd, self.project.as_deref(), &StoreOptions::default())?;
    let base = match &bound.binding {
      Binding::Overlay(overlay) => overlay.project.as_str(),
      _ => bound.project.name.as_str(),
    };
    let mut warnings = registry.warnings(Some(base));
    warnings.extend(bound.warnings.iter().cloned());
    self.warn_once(&warnings);
    Ok(self.bound.get_or_init(|| bound))
  }

  pub fn settings_project(&self, project: &Project) -> CliResult<String> {
    Ok(match &self.bound()?.binding {
      Binding::Overlay(overlay) => overlay.project.to_string(),
      _ => project.name.to_string(),
    })
  }

  pub fn engine_home(&self) -> CliResult<AtlasHome> {
    Ok(self.bound()?.engine_home.clone())
  }

  pub fn latest_project(&self, current: &Project) -> CliResult<Project> {
    let registry = self.registry()?;
    if !matches!(self.bound()?.binding, Binding::Overlay(_)) {
      return Ok(registry.project(current.name.as_str())?.clone());
    }
    current_overlay(&registry, &self.cwd, self.project.as_deref())?
      .map(|plan| plan.project)
      .filter(|latest| latest.name == current.name)
      .ok_or_else(|| CliError::OverlayGone(current.name.to_string()))
  }

  fn warn_once(&self, warnings: &[String]) {
    if self.warned.swap(true, Ordering::Relaxed) {
      return;
    }
    for warning in warnings {
      note(format!("warning: {warning}"));
    }
  }

  pub fn open_engine(&self, project: &Project) -> CliResult<Arc<Engine>> {
    Ok(Arc::new(Engine::open(project, &self.engine_home()?, EngineOptions::default())?))
  }

  pub fn open_query(&self, project: &Project) -> CliResult<QueryEngine> {
    Ok(QueryEngine::new(self.open_engine(project)?))
  }
}
