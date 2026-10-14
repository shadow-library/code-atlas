use atlas_mcp::ServeOptions;
use atlas_project::{ConfigEnv, McpFlags, UserConfig, resolve_mcp};

use crate::cli::McpArgs;
use crate::commands::watch::auto_scip_options;
use crate::context::Context;
use crate::error::CliResult;
use crate::output::Outcome;

pub fn run(ctx: &Context, args: McpArgs) -> CliResult<Outcome> {
  let flags = McpFlags {
    watch: match (args.watch, args.no_watch) {
      (true, _) => Some(true),
      (_, true) => Some(false),
      _ => None,
    },
  };
  let mcp = resolve_mcp(&UserConfig::load(&ctx.home)?, &ConfigEnv::from_process(), flags);
  for warning in &mcp.warnings {
    tracing::warn!(warning = %warning, "ignored an MCP setting");
  }
  let settings = mcp.settings;
  let mut options = ServeOptions::new(ctx.home.clone(), ctx.cwd.clone())
    .with_watch(settings.watch)
    .with_output_limits(settings.max_output_bytes, settings.max_body_lines);
  if settings.watch {
    options.engine.auto_scip = auto_scip_options();
  }
  if let Some(project) = &ctx.project {
    options = options.with_project(project.clone());
  }
  atlas_mcp::serve(options)?;
  Ok(Outcome::Complete)
}
