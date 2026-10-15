use std::sync::Arc;

use atlas_ide::{
  Completeness, DeclarationRequest, DiagnosticsRequest, DiscoveryLauncher, IdeMode, IdeOptions, IdeService, LineRange, Outline, OutlineRequest, OverlayEdits, ReadSource,
  References, ReferencesRequest, ServerView, Severity, UnitCheck, UnitCheckState,
};
use atlas_project::Project;
use atlas_query::{FindRequest, Freshness, HierarchyRequest, NameMatch, OutlineParse, RenderText, Response, Targeted};
use serde::Serialize;

use crate::cli::{DeclArgs, DiagnosticsArgs, FindArgs, HierarchyArgs, ImplsArgs, LspCheckArgs, LspCommand, OutlineArgs, RefsArgs, SeverityArg, SourceArg};
use crate::commands::lsp_probe::{self, Resolved, RustAnalyzerProbe, TypeScriptProbe};
use crate::commands::query::{finish, finish_targeted, render};
use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::interrupt;
use crate::output::{Outcome, emit, emit_line};

pub struct OneShot {
  pub ide: Arc<IdeService>,
  pub project: Project,
}

impl OneShot {
  pub fn open(ctx: &Context) -> CliResult<Self> {
    let project = ctx.resolve()?;
    let engine = ctx.open_engine(&project)?;
    let bound = ctx.bound()?;
    let options = IdeOptions::for_project(&project, &bound.engine_home, IdeMode::OneShot).in_overlay(OverlayEdits::of(&bound.binding, bound.base_db.as_deref()));
    let ide = Arc::new(IdeService::new(engine, Arc::new(DiscoveryLauncher), options));
    interrupt::watch(Arc::clone(&ide))?;
    Ok(Self { ide, project })
  }
}

impl Drop for OneShot {
  fn drop(&mut self) {
    self.ide.shutdown();
  }
}

fn source(arg: SourceArg) -> ReadSource {
  match arg {
    SourceArg::Lsp => ReadSource::Lsp,
    SourceArg::Graph => ReadSource::Graph,
  }
}

fn severity(arg: SeverityArg) -> Severity {
  match arg {
    SeverityArg::Error => Severity::Error,
    SeverityArg::Warning => Severity::Warning,
    SeverityArg::Info => Severity::Info,
    SeverityArg::Hint => Severity::Hint,
  }
}

fn conclude(freshness: &Freshness, server: Option<&ServerView>) -> Outcome {
  let partial = server.is_some_and(|server| matches!(server.completeness, Completeness::Partial { .. }));
  Outcome::degraded_if(partial || freshness.is_degraded())
}

fn finish_exact<T: RenderText + Serialize>(response: &Response<T>, json: bool, server: Option<&ServerView>) -> CliResult<Outcome> {
  emit(&render(response, json)?)?;
  Ok(conclude(&response.freshness, server))
}

fn finish_exact_targeted<T: RenderText + Serialize>(response: &Response<Targeted<T>>, json: bool, server: impl Fn(&T) -> &ServerView) -> CliResult<Outcome> {
  emit(&render(response, json)?)?;
  match &response.answer {
    Targeted::Found(answer) => Ok(conclude(&response.freshness, Some(server(answer)))),
    _ => Ok(Outcome::Unresolved),
  }
}

pub fn find(ctx: &Context, args: FindArgs) -> CliResult<Outcome> {
  let query = ctx.open_query(&ctx.resolve()?)?;
  let mut request = FindRequest::new(args.pattern);
  request.path = args.path;
  request.body = args.body;
  request.kinds = args.kinds;
  request.exclude_kinds = args.exclude_kinds;
  if args.substring {
    request.name_match = NameMatch::Substring;
  }
  if let Some(depth) = args.depth {
    request.depth = depth;
  }
  if let Some(max) = args.max {
    request.max_matches = max;
  }
  let response = query.find(&request)?;
  let matched = !response.answer.matches.items.is_empty();
  let outcome = finish(response, args.common.json)?;
  Ok(match (matched, outcome) {
    (false, Outcome::Complete) => Outcome::Unresolved,
    (_, outcome) => outcome,
  })
}

pub fn hierarchy(ctx: &Context, args: HierarchyArgs) -> CliResult<Outcome> {
  let query = ctx.open_query(&ctx.resolve()?)?;
  let mut request = HierarchyRequest::new(args.target);
  if let Some(depth) = args.depth {
    request.depth = depth;
  }
  finish_targeted(query.hierarchy(&request)?, args.common.json)
}

pub fn outline(ctx: &Context, args: OutlineArgs) -> CliResult<Outcome> {
  let one = OneShot::open(ctx)?;
  let mut request = OutlineRequest::new(args.file);
  request.source = source(args.source);
  if let Some(depth) = args.depth {
    request.depth = depth;
  }
  let response = one.ide.outline(&request)?;
  emit(&render(&response, args.common.json)?)?;
  match &response.answer {
    Outline::Exact(exact) => Ok(conclude(&response.freshness, Some(&exact.server))),
    Outline::Graph(Targeted::Found(found)) => Ok(Outcome::degraded_if(found.parse != OutlineParse::Complete || response.freshness.is_degraded())),
    Outline::Graph(_) => Ok(Outcome::Unresolved),
  }
}

pub fn refs(ctx: &Context, args: RefsArgs) -> CliResult<Outcome> {
  let one = OneShot::open(ctx)?;
  let mut request = ReferencesRequest::new(args.target);
  request.source = source(args.source);
  let response = one.ide.references(&request)?;
  emit(&render(&response, args.common.json)?)?;
  match &response.answer {
    References::Exact(exact) => match exact.as_ref() {
      Targeted::Found(answer) => Ok(conclude(&response.freshness, Some(&answer.server))),
      _ => Ok(Outcome::Unresolved),
    },
    References::Graph(graph) => match graph.as_ref() {
      Targeted::Found(_) => Ok(conclude(&response.freshness, None)),
      _ => Ok(Outcome::Unresolved),
    },
  }
}

pub fn decl(ctx: &Context, args: DeclArgs) -> CliResult<Outcome> {
  let one = OneShot::open(ctx)?;
  let request = match (args.target, args.path, args.regex) {
    (Some(target), _, _) => DeclarationRequest::Position { target },
    (None, Some(path), Some(regex)) => DeclarationRequest::Regex { path, regex },
    _ => return Err(CliError::InvalidInput("decl needs a position, or --path with --regex".to_owned())),
  };
  let response = one.ide.declaration(&request)?;
  let found = !response.answer.definitions.is_empty() || !response.answer.declarations.is_empty();
  let outcome = finish_exact(&response, args.common.json, Some(&response.answer.server))?;
  Ok(match (found, outcome) {
    (false, Outcome::Complete) => Outcome::Unresolved,
    (_, outcome) => outcome,
  })
}

pub fn impls(ctx: &Context, args: ImplsArgs) -> CliResult<Outcome> {
  let one = OneShot::open(ctx)?;
  let response = one.ide.implementations(&args.target)?;
  finish_exact_targeted(&response, args.common.json, |answer| &answer.server)
}

pub fn diagnostics(ctx: &Context, args: DiagnosticsArgs) -> CliResult<Outcome> {
  let one = OneShot::open(ctx)?;
  let mut request = DiagnosticsRequest::new(args.file);
  request.lines = args.lines.as_deref().map(LineRange::parse).transpose()?;
  request.min_severity = severity(args.min_severity);
  let response = one.ide.diagnostics(&request)?;
  finish_exact(&response, args.common.json, Some(&response.answer.server))
}

pub fn lsp(ctx: &Context, command: LspCommand) -> CliResult<Outcome> {
  match command {
    LspCommand::Check(args) => check(ctx, args),
  }
}

#[derive(Debug, Serialize)]
struct CheckReport {
  project: String,
  enabled: bool,
  typescript: TypeScriptProbe,
  rust_analyzer: RustAnalyzerProbe,
  units: Vec<UnitCheck>,
}

pub struct ServerReport {
  pub typescript: TypeScriptProbe,
  pub rust_analyzer: RustAnalyzerProbe,
  pub units: Vec<UnitCheck>,
}

pub fn check_servers(ctx: &Context) -> CliResult<(Project, ServerReport)> {
  let one = OneShot::open(ctx)?;
  let settings = &one.project.settings.lsp;
  let report = ServerReport {
    typescript: lsp_probe::probe_typescript(settings, &lsp_probe::typescript_roots(&one.project), settings.download),
    rust_analyzer: lsp_probe::probe_rust_analyzer(),
    units: one.ide.check_servers()?,
  };
  Ok((one.project.clone(), report))
}

fn check(ctx: &Context, args: LspCheckArgs) -> CliResult<Outcome> {
  let (project, report) = check_servers(ctx)?;
  let outcome = check_outcome(&report.units);
  if args.json {
    let view = CheckReport {
      project: project.name.to_string(),
      enabled: project.settings.lsp.enabled,
      typescript: report.typescript,
      rust_analyzer: report.rust_analyzer,
      units: report.units,
    };
    crate::output::emit_json(&view)?;
    return Ok(outcome);
  }
  emit_line(format!("project {}", project.name))?;
  emit_line("typescript-language-server discovery:")?;
  for step in &report.typescript.steps {
    emit_line(format!("  {}", step.text()))?;
  }
  emit_line(format!("  => {}", resolved_text(&report.typescript.result)))?;
  emit_line(format!("rust-analyzer: {}", resolved_text(&report.rust_analyzer.result)))?;
  if report.units.is_empty() {
    emit_line("no Rust or TypeScript files are indexed; nothing to start")?;
  }
  for unit in &report.units {
    for line in unit_lines(unit, &project.settings.lsp.exclude) {
      emit_line(line)?;
    }
  }
  Ok(outcome)
}

pub fn resolved_text(resolved: &Resolved) -> String {
  match resolved {
    Resolved::Found { summary } => summary.clone(),
    Resolved::Unavailable { reason } => format!("unavailable: {reason}"),
  }
}

pub fn unit_detail(unit: &UnitCheck, current_exclude: &[String]) -> (String, Vec<String>) {
  match &unit.state {
    UnitCheckState::Failed { reason } => (reason.clone(), Vec::new()),
    UnitCheckState::Ready {
      ready_ms,
      server_version,
      typescript,
      health_error,
    } => {
      let versions = match typescript {
        Some(typescript) => format!("{server_version}, TypeScript {typescript}"),
        None => server_version.clone(),
      };
      let detail = format!("ready in {:.1} s, version {versions}", *ready_ms as f64 / 1000.0);
      let suggested: String = current_exclude.iter().map(|glob| format!(" --lsp-exclude {glob}")).collect();
      let notes = health_error
        .iter()
        .flat_map(|health| {
          [
            format!("workspace health error: {health}"),
            format!(
              "exact answers from this server are marked PARTIAL and edits are refused until it loads; exclude the failing Cargo projects (for example fixtures); --lsp-exclude replaces the list, so repeat the current globs: catl project config <project>{suggested} --lsp-exclude <glob>"
            ),
          ]
        })
        .collect();
      (detail, notes)
    }
  }
}

pub fn unit_lines(unit: &UnitCheck, current_exclude: &[String]) -> Vec<String> {
  let (detail, notes) = unit_detail(unit, current_exclude);
  let label = match check_outcome(std::slice::from_ref(unit)) {
    Outcome::Complete => "ok  ",
    Outcome::Failed => "FAIL",
    _ => "warn",
  };
  let mut lines = vec![format!("{label}  {} ({}): {detail}", unit.server, unit.scope)];
  lines.extend(notes.into_iter().map(|note| format!("      {note}")));
  lines
}

pub fn check_outcome(units: &[UnitCheck]) -> Outcome {
  if units.iter().any(|unit| matches!(unit.state, UnitCheckState::Failed { .. })) {
    return Outcome::Failed;
  }
  let unhealthy = units.iter().any(|unit| matches!(&unit.state, UnitCheckState::Ready { health_error: Some(_), .. }));
  Outcome::degraded_if(unhealthy)
}
