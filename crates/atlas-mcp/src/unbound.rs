use std::future::Future;
use std::path::PathBuf;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::transport::IntoTransport;
use rmcp::{RoleServer, ServerHandler, tool, tool_handler, tool_router};
use serde_json::json;

use crate::arguments::{Format, StatusArgs};
use crate::descriptions as text;
use crate::error::McpError;
use crate::server::{compact_router, serve_until};

#[derive(Clone)]
pub struct UnboundServer {
  cwd: PathBuf,
  tool_router: ToolRouter<Self>,
}

impl UnboundServer {
  pub fn new(cwd: impl Into<PathBuf>) -> Self {
    Self {
      cwd: cwd.into(),
      tool_router: compact_router(Self::tool_router()),
    }
  }

  pub async fn run<T, E, A>(self, transport: T, shutdown: impl Future<Output = ()>) -> Result<(), McpError>
  where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
  {
    serve_until(self, transport, shutdown).await
  }

  fn explanation(&self) -> String {
    format!(
      "no project is bound: {} is not inside any registered Code Atlas project, so this server answers nothing but atlas_status. To index this repo, run `catl init -p <project>` in it (adds it to that project, creating the project if needed) or `catl project create <name> --repo <path>`, then restart this MCP server.",
      self.cwd.display()
    )
  }
}

#[tool_router]
impl UnboundServer {
  #[tool(name = "atlas_status", description = text::UNBOUND_STATUS, annotations(read_only_hint = true))]
  async fn status(&self, Parameters(args): Parameters<StatusArgs>) -> CallToolResult {
    let explanation = self.explanation();
    let text = match args.format.unwrap_or_default() {
      Format::Text => format!("{explanation}\n"),
      Format::Json => json!({ "bound": false, "cwd": self.cwd.display().to_string(), "message": explanation }).to_string() + "\n",
    };
    CallToolResult::success(vec![ContentBlock::text(text)])
  }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for UnboundServer {
  fn get_info(&self) -> ServerConfig {
    ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(text::UNBOUND_INSTRUCTIONS)
  }
}
