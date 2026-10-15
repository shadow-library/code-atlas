use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use atlas_engine::{Engine, WatchHandle, bind_project, current_overlay};
use atlas_ide::{DiscoveryLauncher, IdeMode, IdeOptions, IdeService, OverlayEdits};
use atlas_project::{AtlasHome, Binding, OverlayLease, OverlayReport, Project, Registry};
use atlas_query::QueryEngine;

use crate::error::{McpError, ToolError};
use crate::options::ServeOptions;

const REGISTRY_RETRY_DELAY: Duration = Duration::from_millis(50);

pub(crate) struct Session {
  home: AtlasHome,
  cwd: PathBuf,
  explicit: Option<String>,
  project: String,
  binding: Binding,
  ide: IdeService,
  refresh: Mutex<Refresh>,
  _overlay: Option<OverlayLease>,
}

struct Refresh {
  watching: bool,
  watcher: Option<WatchHandle>,
}

impl Session {
  pub(crate) fn open(options: &ServeOptions) -> Result<Self, McpError> {
    let registry = Registry::load(&options.home)?;
    let bound = bind_project(&options.home, &registry, &options.cwd, options.project.as_deref(), &options.engine.store)?;
    let project = bound.project;
    let engine = Arc::new(Engine::open(&project, &bound.engine_home, options.engine.clone())?);
    let watcher = if options.watch { start_watcher(&engine) } else { None };
    tracing::info!(project = %project.name, repos = project.repos.len(), watch = options.watch, "bound the MCP server to its project");
    if let Some(caveat) = bound.binding.caveat() {
      tracing::warn!(caveat = %caveat, "started outside the indexed checkout; answers carry a caveat and edits are refused");
    }
    if let Binding::Overlay(overlay) = &bound.binding {
      tracing::info!(overlay = %overlay.describe(), "answering from the worktree's own index; edits write the worktree");
    }
    log_config_warnings(&registry, base_name(&bound.binding, &project));
    for warning in &bound.warnings {
      tracing::warn!(warning = %warning, "ignored a configuration value");
    }
    let launcher = options.launcher.as_ref().map_or_else(|| Arc::new(DiscoveryLauncher) as _, |handle| Arc::clone(&handle.0));
    let ide_options = IdeOptions::for_project(&project, &bound.engine_home, IdeMode::Warm).in_overlay(OverlayEdits::of(&bound.binding, bound.base_db.as_deref()));
    let ide = IdeService::new(engine, launcher, ide_options);
    Ok(Self {
      home: options.home.clone(),
      cwd: options.cwd.clone(),
      explicit: options.project.clone(),
      project: project.name.to_string(),
      binding: bound.binding,
      ide,
      refresh: Mutex::new(Refresh { watching: options.watch, watcher }),
      _overlay: bound.overlay,
    })
  }

  pub(crate) fn query(&self) -> &QueryEngine {
    self.ide.query()
  }

  pub(crate) fn ide(&self) -> &IdeService {
    &self.ide
  }

  pub(crate) fn engine(&self) -> &Arc<Engine> {
    self.ide.query().engine()
  }

  pub(crate) fn project_name(&self) -> &str {
    &self.project
  }

  pub(crate) fn caveat(&self) -> Option<String> {
    self.binding.caveat()
  }

  pub(crate) fn overlay_report(&self) -> Option<(String, OverlayReport)> {
    let Binding::Overlay(overlay) = &self.binding else { return None };
    Some((overlay.project.to_string(), overlay.report(&self.home)))
  }

  pub(crate) fn edit_refusal(&self) -> Option<String> {
    self.binding.edit_refusal()
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
    let latest = self.latest_project(&registry)?;
    if *self.engine().project() == latest {
      return Ok(());
    }
    tracing::info!(project = %self.project, repos = latest.repos.len(), "project definition changed; reloading the engine");
    log_config_warnings(&registry, base_name(&self.binding, &latest));
    self.engine().reload_project(&latest)?;
    self.ide.reload(&latest);
    if refresh.watching {
      refresh.watcher = None;
      refresh.watcher = start_watcher(self.engine());
    }
    Ok(())
  }

  // Concurrent calls compare without the refresh lock; a reload re-reads the registry under the lock so a call holding an older copy never reverts it.
  fn registry_matches(&self) -> Result<bool, ToolError> {
    let registry = self.load_registry()?;
    Ok(*self.engine().project() == self.latest_project(&registry)?)
  }

  // An overlay is re-derived from the registry, never replaced by its base project, whose paths are the main checkout's.
  fn latest_project(&self, registry: &Registry) -> Result<Project, ToolError> {
    if !matches!(self.binding, Binding::Overlay(_)) {
      return registry.project(&self.project).cloned().map_err(|_| ToolError::ProjectGone(self.project.clone()));
    }
    current_overlay(registry, &self.cwd, self.explicit.as_deref())?
      .map(|plan| plan.project)
      .filter(|latest| latest.name.as_str() == self.project)
      .ok_or_else(|| ToolError::OverlayGone(self.project.clone()))
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
    shut_down_in_order(|| self.ide.shutdown(), || self.persist_similarity());
  }

  fn persist_similarity(&self) {
    match self.engine().persist_similarity() {
      Ok(persisted) => tracing::info!(persisted, "persisted the similarity index on shutdown"),
      Err(error) => tracing::warn!(error = %error, "could not persist the similarity index on shutdown"),
    }
  }
}

fn shut_down_in_order(stop_language_servers: impl FnOnce(), persist_similarity: impl FnOnce()) {
  stop_language_servers();
  persist_similarity();
}

fn base_name<'a>(binding: &'a Binding, project: &'a Project) -> &'a str {
  match binding {
    Binding::Overlay(overlay) => overlay.project.as_str(),
    _ => project.name.as_str(),
  }
}

fn log_config_warnings(registry: &Registry, project: &str) {
  for warning in registry.warnings(Some(project)) {
    tracing::warn!(warning = %warning, "ignored a configuration value");
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

#[cfg(test)]
mod tests {
  use std::cell::RefCell;

  use super::*;

  #[test]
  fn should_stop_language_servers_before_persisting_the_similarity_index() {
    let order = RefCell::new(Vec::new());

    shut_down_in_order(|| order.borrow_mut().push("language servers"), || order.borrow_mut().push("similarity"));

    assert_eq!(*order.borrow(), ["language servers", "similarity"]);
  }
}
