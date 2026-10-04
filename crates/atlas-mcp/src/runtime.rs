use std::time::Duration;

use rmcp::transport::stdio;
use tracing_subscriber::EnvFilter;

use crate::error::McpError;
use crate::options::ServeOptions;
use crate::server::AtlasServer;

const RUNTIME_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

pub fn serve(options: ServeOptions) -> Result<(), McpError> {
  init_stderr_logging();
  let server = AtlasServer::open(&options)?;
  let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(McpError::Runtime)?;
  let outcome = runtime.block_on(server.run(stdio(), termination_signal()));
  runtime.shutdown_timeout(RUNTIME_SHUTDOWN_GRACE);
  outcome
}

fn init_stderr_logging() {
  let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
  let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).with_ansi(false).try_init();
}

#[cfg(unix)]
async fn termination_signal() {
  use tokio::signal::unix::{SignalKind, signal};

  let (Ok(mut terminate), Ok(mut interrupt)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) else {
    tracing::warn!("cannot install signal handlers; shutdown relies on stdin EOF");
    return std::future::pending().await;
  };
  tokio::select! {
    _ = terminate.recv() => {}
    _ = interrupt.recv() => {}
  }
}

#[cfg(not(unix))]
async fn termination_signal() {
  if tokio::signal::ctrl_c().await.is_err() {
    std::future::pending::<()>().await;
  }
}
