use atlas_project::{AtlasHome, Project};

use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::output::Outcome;

pub fn run(ctx: &Context) -> CliResult<Outcome> {
  let project = ctx.resolve()?;
  serve(&project, &ctx.home)?;
  Ok(Outcome::Complete)
}

fn serve(_project: &Project, _home: &AtlasHome) -> CliResult<()> {
  Err(CliError::McpUnavailable)
}
