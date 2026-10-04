use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use atlas_engine::{Engine, WatchHandle};
use atlas_project::{AtlasHome, Registry};
use atlas_query::QueryEngine;

use crate::error::{McpError, ToolError};
use crate::options::ServeOptions;

const REGISTRY_RETRY_DELAY: Duration = Duration::from_millis(50);

pub(crate) struct Session {
  home: AtlasHome,
  project: String,
  query: QueryEngine,
  refresh: Mutex<Refresh>,
}

struct Refresh {
  watching: bool,
  watcher: Option<WatchHandle>,
}

impl Session {
  pub(crate) fn open(options: &ServeOptions) -> Result<Self, McpError> {
    let registry = Registry::load(&options.home)?;
    let project = registry.resolve_project(&options.cwd, options.project.as_deref())?.clone();
    let engine = Arc::new(Engine::open(&project, &options.home, options.engine.clone())?);
    let watcher = if options.watch { start_watcher(&engine) } else { None };
    tracing::info!(project = %project.name, repos = project.repos.len(), watch = options.watch, "bound the MCP server to its project");
    Ok(Self {
      home: options.home.clone(),
      project: project.name.to_string(),
      query: QueryEngine::new(engine),
      refresh: Mutex::new(Refresh { watching: options.watch, watcher }),
    })
  }

  pub(crate) fn query(&self) -> &QueryEngine {
    &self.query
  }

  pub(crate) fn engine(&self) -> &Arc<Engine> {
    self.query.engine()
  }

  pub(crate) fn project_name(&self) -> &str {
    &self.project
  }

  pub(crate) fn repo_names(&self) -> Vec<String> {
    self.engine().project().repos.iter().map(|repo| repo.name.to_string()).collect()
  }

  pub(crate) fn sync_registry(&self) -> Result<(), ToolError> {
    if self.registry_matches()? {
      return Ok(());
    }
    let mut refresh = self.refresh.lock().unwrap_or_else(PoisonError::into_inner);
    let registry = self.load_registry()?;
    let latest = registry.project(&self.project).map_err(|_| ToolError::ProjectGone(self.project.clone()))?;
    if *self.engine().project() == *latest {
      return Ok(());
    }
    tracing::info!(project = %self.project, repos = latest.repos.len(), "project definition changed; reloading the engine");
    self.engine().reload_project(latest)?;
    if refresh.watching {
      refresh.watcher = None;
      refresh.watcher = start_watcher(self.engine());
    }
    Ok(())
  }

  // Concurrent calls compare without the refresh lock; a reload re-reads the registry under the lock so a call holding an older copy never reverts it.
  fn registry_matches(&self) -> Result<bool, ToolError> {
    let registry = self.load_registry()?;
    let latest = registry.project(&self.project).map_err(|_| ToolError::ProjectGone(self.project.clone()))?;
    Ok(*self.engine().project() == *latest)
  }

  fn load_registry(&self) -> Result<Registry, ToolError> {
    match Registry::load(&self.home) {
      Ok(registry) => Ok(registry),
      Err(error) => {
        tracing::debug!(error = %error, "registry read failed; retrying once");
        std::thread::sleep(REGISTRY_RETRY_DELAY);
        Ok(Registry::load(&self.home)?)
      }
    }
  }

  pub(crate) fn shut_down(&self) {
    self.refresh.lock().unwrap_or_else(PoisonError::into_inner).watcher = None;
    match self.engine().persist_similarity() {
      Ok(persisted) => tracing::info!(persisted, "persisted the similarity index on shutdown"),
      Err(error) => tracing::warn!(error = %error, "could not persist the similarity index on shutdown"),
    }
  }
}

fn start_watcher(engine: &Arc<Engine>) -> Option<WatchHandle> {
  match engine.watch() {
    Ok(handle) => Some(handle),
    Err(error) => {
      tracing::warn!(error = %error, "file watcher unavailable; every call falls back to a stat scan");
      None
    }
  }
}
