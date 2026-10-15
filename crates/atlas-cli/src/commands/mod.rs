mod agents;
mod config;
mod debug;
mod doctor;
mod edit;
mod enrich;
mod hook;
mod ide;
mod index;
mod init;
mod lsp_probe;
mod mcp;
mod overlays;
mod project;
mod query;
mod scip;
mod watch;

use std::io::Read;
use std::path::Path;

use crate::cli::{Command, QueryCommand};
use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::output::Outcome;

pub fn run(project: Option<String>, command: Command) -> CliResult<Outcome> {
  let ctx = Context::load(project)?;
  match command {
    Command::Project(command) => project::run(&ctx, command),
    Command::Config(command) => config::run(&ctx, command),
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
    Command::Find(args) => ide::find(&ctx, args),
    Command::Outline(args) => ide::outline(&ctx, args),
    Command::Refs(args) => ide::refs(&ctx, args),
    Command::Decl(args) => ide::decl(&ctx, args),
    Command::Impls(args) => ide::impls(&ctx, args),
    Command::Hierarchy(args) => ide::hierarchy(&ctx, args),
    Command::Diagnostics(args) => ide::diagnostics(&ctx, args),
    Command::Edit(command) => edit::run(&ctx, command),
    Command::Lsp(command) => ide::lsp(&ctx, command),
    Command::Agents(command) => agents::run(&ctx, command),
    Command::Connect(command) => agents::connect(&ctx, command),
    Command::Hook(args) => hook::run(&ctx, args.hook),
    Command::Mcp(args) => mcp::run(&ctx, args),
    Command::Debug(command) => debug::run(&ctx, command),
    Command::Doctor(args) => doctor::run(&ctx, args),
  }
}

pub(crate) fn read_input(source: &str) -> CliResult<String> {
  if source == "-" {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).map_err(|error| CliError::io("read", "<stdin>", error))?;
    return Ok(text);
  }
  std::fs::read_to_string(source).map_err(|error| CliError::io("read", Path::new(source), error))
}
