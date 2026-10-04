use std::num::NonZeroUsize;
use std::path::PathBuf;

use atlas_engine::EngineOptions;
use atlas_project::AtlasHome;

#[derive(Debug, Clone)]
pub struct ServeOptions {
  pub home: AtlasHome,
  pub project: Option<String>,
  pub cwd: PathBuf,
  pub watch: bool,
  pub engine: EngineOptions,
  pub max_output_bytes: usize,
  pub max_body_lines: usize,
}

pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 100_000;
pub const DEFAULT_MAX_BODY_LINES: usize = 400;

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
}
