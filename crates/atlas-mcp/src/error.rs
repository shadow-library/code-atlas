use atlas_engine::EngineError;
use atlas_ide::IdeError;
use atlas_project::ProjectError;
use atlas_query::QueryError;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum McpError {
  #[error(transparent)]
  Project(#[from] ProjectError),
  #[error(transparent)]
  Engine(#[from] EngineError),
  #[error("cannot start the async runtime: {0}")]
  Runtime(#[source] std::io::Error),
  #[error("MCP transport failed: {0}")]
  Transport(String),
}

#[derive(Debug, Error)]
pub(crate) enum ToolError {
  #[error("{0}")]
  Invalid(String),
  #[error(transparent)]
  Query(#[from] QueryError),
  #[error(transparent)]
  Ide(#[from] IdeError),
  #[error("{0}")]
  Refused(String),
  #[error("cannot read the project registry: {0}")]
  Registry(#[from] ProjectError),
  #[error(transparent)]
  Engine(#[from] EngineError),
  #[error("project {0:?} is no longer in the registry; this server stays bound to it and cannot answer")]
  ProjectGone(String),
  #[error("internal error while running {tool}: {message}; the server is still running, retry or use another tool")]
  Panicked { tool: &'static str, message: String },
}

impl ToolError {
  pub(crate) fn invalid(message: impl Into<String>) -> Self {
    Self::Invalid(message.into())
  }

  pub(crate) fn from_join(tool: &'static str, error: tokio::task::JoinError) -> Self {
    let message = match error.try_into_panic() {
      Ok(payload) => panic_message(payload.as_ref()),
      Err(_) => "the query was cancelled during shutdown".to_owned(),
    };
    tracing::error!(tool, message = %message, "query failed unexpectedly");
    Self::Panicked { tool, message }
  }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
  if let Some(message) = payload.downcast_ref::<&str>() {
    return (*message).to_owned();
  }
  if let Some(message) = payload.downcast_ref::<String>() {
    return message.clone();
  }
  "panic with a non-text payload".to_owned()
}
