use atlas_mcp::ServeOptions;

use crate::cli::McpArgs;
use crate::context::Context;
use crate::error::CliResult;
use crate::output::Outcome;

pub fn run(ctx: &Context, args: McpArgs) -> CliResult<Outcome> {
  let mut options = ServeOptions::new(ctx.home.clone(), ctx.cwd.clone()).with_watch(args.watch);
  if let Some(project) = &ctx.project {
    options = options.with_project(project.clone());
  }
  atlas_mcp::serve(options)?;
  Ok(Outcome::Complete)
}
