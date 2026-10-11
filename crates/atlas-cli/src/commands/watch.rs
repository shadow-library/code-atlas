use std::sync::Arc;
use std::time::Duration;

use atlas_engine::{AutoScipOptions, Engine, EngineOptions, FreshnessReport};
use atlas_project::Project;
use atlas_scip::{RustAnalyzerScip, ScipTypescript};
use tokio::runtime::Runtime;
use tokio::signal::unix::{Signal, SignalKind, signal};

use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::output::{Outcome, note};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const CONFIG_CHANGED_MARKER: &str = "project configuration changed";

struct Shutdown {
  runtime: Runtime,
  interrupt: Signal,
  terminate: Signal,
}

enum Wake {
  Signal,
  Tick,
}

impl Shutdown {
  fn install() -> CliResult<Self> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(CliError::Signal)?;
    let (interrupt, terminate) = runtime
      .block_on(async { Ok::<_, std::io::Error>((signal(SignalKind::interrupt())?, signal(SignalKind::terminate())?)) })
      .map_err(CliError::Signal)?;
    Ok(Self { runtime, interrupt, terminate })
  }

  fn wait(&mut self) -> Wake {
    let Self { runtime, interrupt, terminate } = self;
    runtime.block_on(async {
      tokio::select! {
        _ = interrupt.recv() => Wake::Signal,
        _ = terminate.recv() => Wake::Signal,
        _ = tokio::time::sleep(POLL_INTERVAL) => Wake::Tick,
      }
    })
  }
}

pub(crate) fn auto_scip_options() -> AutoScipOptions {
  AutoScipOptions::with_indexers(Arc::new(RustAnalyzerScip::new()), Arc::new(ScipTypescript::new()))
}

pub fn run(ctx: &Context) -> CliResult<Outcome> {
  let project = ctx.resolve()?;
  let options = EngineOptions {
    auto_scip: auto_scip_options(),
    ..EngineOptions::default()
  };
  let engine = Arc::new(Engine::open(&project, &ctx.home, options)?);
  let report = engine.ensure_fresh()?;
  if !report.is_fresh() {
    tracing::warn!(project = %project.name, "the initial freshness check was degraded");
  }
  let mut shutdown = Shutdown::install()?;
  let mut handle = engine.watch()?;
  note(format!("watching {} ({} repos); press Ctrl-C to stop", project.name, project.repos.len()));
  note(match project.settings.scip.enabled {
    true => format!("automatic SCIP runs after {}s without file changes", project.settings.scip.debounce_secs),
    false => format!("automatic SCIP is off; enable it with `catl project config {} --scip on`", project.name),
  });
  while let Wake::Tick = shutdown.wait() {
    if !config_changed(&engine.ensure_fresh()?) {
      continue;
    }
    drop(handle);
    let reloaded = reload(ctx, &engine, &project)?;
    handle = engine.watch()?;
    note(format!("project configuration changed; reloaded ({} repos)", reloaded.repos.len()));
  }
  drop(handle);
  if let Err(error) = engine.persist_similarity() {
    tracing::warn!(error = %error, "could not persist the similarity index");
  }
  Ok(Outcome::Complete)
}

fn config_changed(report: &FreshnessReport) -> bool {
  matches!(report, FreshnessReport::Degraded { reason, .. } if reason.contains(CONFIG_CHANGED_MARKER))
}

fn reload(ctx: &Context, engine: &Engine, project: &Project) -> CliResult<Project> {
  let registry = ctx.registry()?;
  let current = registry.project(project.name.as_str())?.clone();
  engine.reload_project(&current)?;
  Ok(current)
}
