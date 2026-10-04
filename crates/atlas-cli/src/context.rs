use std::path::PathBuf;
use std::sync::Arc;

use atlas_engine::{Engine, EngineOptions};
use atlas_project::{AtlasHome, Project, Registry};
use atlas_query::QueryEngine;

use crate::error::{CliError, CliResult};

pub struct Context {
  pub home: AtlasHome,
  pub cwd: PathBuf,
  pub project: Option<String>,
}

impl Context {
  pub fn load(project: Option<String>) -> CliResult<Self> {
    let cwd = std::env::current_dir().map_err(|source| CliError::io("read the working directory of", ".", source))?;
    Ok(Self {
      home: AtlasHome::resolve()?,
      cwd,
      project,
    })
  }

  pub fn registry(&self) -> CliResult<Registry> {
    Ok(Registry::load(&self.home)?)
  }

  pub fn resolve(&self) -> CliResult<Project> {
    let registry = self.registry()?;
    Ok(registry.resolve_project(&self.cwd, self.project.as_deref())?.clone())
  }

  pub fn open_engine(&self, project: &Project) -> CliResult<Arc<Engine>> {
    Ok(Arc::new(Engine::open(project, &self.home, EngineOptions::default())?))
  }

  pub fn open_query(&self, project: &Project) -> CliResult<QueryEngine> {
    Ok(QueryEngine::new(self.open_engine(project)?))
  }
}
