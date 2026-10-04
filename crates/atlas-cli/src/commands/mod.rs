mod connect;
mod debug;
mod doctor;
mod enrich;
mod index;
mod init;
mod mcp;
mod project;
mod query;
mod scip;
mod watch;

use crate::cli::{Command, QueryCommand};
use crate::context::Context;
use crate::error::CliResult;
use crate::output::Outcome;

pub fn run(project: Option<String>, command: Command) -> CliResult<Outcome> {
  let ctx = Context::load(project)?;
  match command {
    Command::Project(command) => project::run(&ctx, command),
    Command::Init => init::run(&ctx),
    Command::Index(args) => index::run(&ctx, args),
    Command::Status(args) => query::status(&ctx, args),
    Command::Watch => watch::run(&ctx),
    Command::Enrich(args) => enrich::run(&ctx, args),
    Command::Scip => scip::run(&ctx),
    Command::Query(command) => query::run(&ctx, command),
    Command::Search(args) => query::run(&ctx, QueryCommand::Search(args)),
    Command::Impact(args) => query::run(&ctx, QueryCommand::Impact(args)),
    Command::Context(args) => query::run(&ctx, QueryCommand::Context(args)),
    Command::Callers(args) => query::run(&ctx, QueryCommand::Callers(args)),
    Command::Callees(args) => query::run(&ctx, QueryCommand::Callees(args)),
    Command::Symbol(args) => query::run(&ctx, QueryCommand::Symbol(args)),
    Command::Api(args) => query::run(&ctx, QueryCommand::Api(args)),
    Command::Explain(args) => query::run(&ctx, QueryCommand::Explain(args)),
    Command::Read(args) => query::run(&ctx, QueryCommand::Read(args)),
    Command::TestsFor(args) => query::run(&ctx, QueryCommand::TestsFor(args)),
    Command::Connect(command) => connect::run(&ctx, command),
    Command::Mcp(args) => mcp::run(&ctx, args),
    Command::Debug(command) => debug::run(&ctx, command),
    Command::Doctor => doctor::run(&ctx),
  }
}
