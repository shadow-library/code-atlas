use std::fmt;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;

use atlas_engine::EngineOptions;
use atlas_ide::Launcher;
use atlas_project::{AtlasHome, McpSettings};

#[derive(Clone)]
pub(crate) struct LauncherHandle(pub(crate) Arc<dyn Launcher>);

impl fmt::Debug for LauncherHandle {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str("Launcher")
  }
}

#[derive(Debug, Clone)]
pub struct ServeOptions {
  pub home: AtlasHome,
  pub project: Option<String>,
  pub cwd: PathBuf,
  pub watch: bool,
  pub engine: EngineOptions,
  pub max_output_bytes: usize,
  pub max_body_lines: usize,
  pub(crate) launcher: Option<LauncherHandle>,
}

pub const DEFAULT_MAX_OUTPUT_BYTES: usize = McpSettings::DEFAULT_MAX_OUTPUT_BYTES;
pub const DEFAULT_MAX_BODY_LINES: usize = McpSettings::DEFAULT_MAX_BODY_LINES;

fn server_engine_options() -> EngineOptions {
  let mut options = EngineOptions::default();
  let cores = std::thread::available_parallelism().map_or(options.store.readers, NonZeroUsize::get);
  options.store.readers = options.store.readers.max(cores);
  options
}

impl ServeOptions {
  pub fn new(home: AtlasHome, cwd: impl Into<PathBuf>) -> Self {
    Self {
      home,
      project: None,
      cwd: cwd.into(),
      watch: false,
      engine: server_engine_options(),
      max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
      max_body_lines: DEFAULT_MAX_BODY_LINES,
      launcher: None,
    }
  }

  pub fn with_project(mut self, project: impl Into<String>) -> Self {
    self.project = Some(project.into());
    self
  }

  pub fn with_output_limits(mut self, max_output_bytes: usize, max_body_lines: usize) -> Self {
    self.max_output_bytes = max_output_bytes;
    self.max_body_lines = max_body_lines;
    self
  }

  pub fn with_watch(mut self, watch: bool) -> Self {
    self.watch = watch;
    self
  }

  #[doc(hidden)]
  pub fn with_launcher(mut self, launcher: Arc<dyn Launcher>) -> Self {
    self.launcher = Some(LauncherHandle(launcher));
    self
  }
}
