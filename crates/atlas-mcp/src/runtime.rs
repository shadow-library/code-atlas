use std::future::Future;
use std::time::Duration;

use atlas_project::ProjectError;
use rmcp::RoleServer;
use rmcp::transport::{IntoTransport, stdio};
use tracing_subscriber::EnvFilter;

use crate::error::McpError;
use crate::options::ServeOptions;
use crate::server::AtlasServer;
use crate::unbound::UnboundServer;

const RUNTIME_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

pub fn serve(options: ServeOptions) -> Result<(), McpError> {
  init_stderr_logging();
  let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(McpError::Runtime)?;
  let signals = {
    let _entered = runtime.enter();
    TerminationSignals::register()
  };
  let server = Server::open(&options)?;
  pause_for_signal_test();
  let outcome = runtime.block_on(server.run(stdio(), signals.wait()));
  runtime.shutdown_timeout(RUNTIME_SHUTDOWN_GRACE);
  outcome
}

pub enum Server {
  Bound(AtlasServer),
  Unbound(UnboundServer),
}

impl Server {
  pub fn open(options: &ServeOptions) -> Result<Self, McpError> {
    match AtlasServer::open(options) {
      Err(McpError::Project(ProjectError::NotInProject { cwd })) if options.project.is_none() => {
        tracing::info!(cwd = %cwd.display(), "no registered project contains the working directory; serving atlas_status only");
        Ok(Self::Unbound(UnboundServer::new(cwd)))
      }
      Err(McpError::Project(ignored @ ProjectError::WorktreeIgnored { .. })) => {
        tracing::info!(reason = %ignored, "worktrees are switched off; serving atlas_status only");
        Ok(Self::Unbound(UnboundServer::explaining(&options.cwd, ignored.to_string())))
      }
      opened => opened.map(Self::Bound),
    }
  }

  pub async fn run<T, E, A>(self, transport: T, shutdown: impl Future<Output = ()>) -> Result<(), McpError>
  where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
  {
    match self {
      Self::Bound(server) => server.run(transport, shutdown).await,
      Self::Unbound(server) => server.run(transport, shutdown).await,
    }
  }
}

/// Test-only: `ATLAS_MCP_TEST_DELAY_BEFORE_RUN_MS` holds the process between signal registration and serving so tests can signal it deterministically.
fn pause_for_signal_test() {
  let Some(millis) = std::env::var("ATLAS_MCP_TEST_DELAY_BEFORE_RUN_MS").ok().and_then(|value| value.parse::<u64>().ok()) else {
    return;
  };
  tracing::info!(millis, "test delay before serving");
  std::thread::sleep(Duration::from_millis(millis));
}

fn init_stderr_logging() {
  let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
  let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).with_ansi(false).try_init();
}

#[cfg(unix)]
struct TerminationSignals {
  streams: Option<(tokio::signal::unix::Signal, tokio::signal::unix::Signal)>,
}

#[cfg(unix)]
impl TerminationSignals {
  fn register() -> Self {
    use tokio::signal::unix::{SignalKind, signal};

    let streams = match (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) {
      (Ok(terminate), Ok(interrupt)) => Some((terminate, interrupt)),
      _ => {
        tracing::warn!("cannot install signal handlers; shutdown relies on stdin EOF");
        None
      }
    };
    Self { streams }
  }

  async fn wait(self) {
    let Some((mut terminate, mut interrupt)) = self.streams else {
      return std::future::pending().await;
    };
    tokio::select! {
      _ = terminate.recv() => {}
      _ = interrupt.recv() => {}
    }
  }
}

#[cfg(not(unix))]
struct TerminationSignals;

#[cfg(not(unix))]
impl TerminationSignals {
  fn register() -> Self {
    Self
  }

  async fn wait(self) {
    if tokio::signal::ctrl_c().await.is_err() {
      std::future::pending::<()>().await;
    }
  }
}
