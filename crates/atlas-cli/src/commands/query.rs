use std::fs;
use std::path::Path;

use atlas_core::Language;
use atlas_project::{Binding, Project};
use atlas_query::{
  ApiRequest, ChangesRequest, ContextRequest, CounterpartsRequest, DEFAULT_DOCS_LIMIT, DocsRequest, ExplainRequest, ImpactRequest, NotesRequest, PathRequest, PatternsRequest,
  QueryEngine, ReadRequest, RenderText, Response, SearchRequest, SimilarRequest, SymbolRequest, TargetFilter, Targeted, TestsForRequest, TraversalKind, TraverseRequest,
  UnusedRequest, render_json, render_text,
};
use serde::Serialize;

use crate::cli::{
  ApiArgs, ChangesArgs, CodeLanguage, ContextArgs, CounterpartsArgs, DocsArgs, ExplainArgs, ImpactArgs, NotesCommand, PathArgs, PatternsArgs, QueryCommand, ReadArgs, SearchArgs,
  SimilarArgs, StatusArgs, SymbolArgs, TargetFilterArgs, TestsForArgs, TraverseArgs, UnusedArgs,
};
use crate::commands::{overlays, read_input};
use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::output::{Outcome, emit};

pub fn status(ctx: &Context, args: StatusArgs) -> CliResult<Outcome> {
  let (query, project) = open(ctx)?;
  let mut response = query.status()?;
  let binding = &ctx.bound()?.binding;
  if let Binding::Overlay(overlay) = binding {
    response.answer.settings_project = Some(overlay.project.to_string());
  }
  let overlays = overlays::report(&ctx.home, binding, project.name.as_str());
  let mut text = render(&response, args.json)?;
  match (args.json, overlays.json) {
    (true, Some((key, value))) => {
      let mut document: serde_json::Value = serde_json::from_str(&text)?;
      document[key] = value;
      text = serde_json::to_string_pretty(&document)? + "\n";
    }
    (true, None) => {}
    (false, _) => overlays.lines.iter().for_each(|line| {
      text.push_str(line);
      text.push('\n');
    }),
  }
  emit(&text)?;
  Ok(Outcome::degraded_if(response.freshness.is_degraded()))
}

pub fn run(ctx: &Context, command: QueryCommand) -> CliResult<Outcome> {
  match command {
    QueryCommand::Overview(args) => {
      let (query, _) = open(ctx)?;
      finish(query.overview_with(args.docs)?, args.common.json)
    }
    QueryCommand::Search(args) => search(ctx, args),
    QueryCommand::Symbol(args) => symbol(ctx, args),
    QueryCommand::Callers(args) => traverse(ctx, TraversalKind::Callers, args.traverse, args.via_http),
    QueryCommand::Callees(args) => traverse(ctx, TraversalKind::Callees, args.traverse, args.via_http),
    QueryCommand::Dependents(args) => traverse(ctx, TraversalKind::Dependents, args, false),
    QueryCommand::Dependencies(args) => traverse(ctx, TraversalKind::Dependencies, args, false),
    QueryCommand::Impact(args) => impact(ctx, args),
    QueryCommand::TestsFor(args) => tests_for(ctx, args),
    QueryCommand::Counterparts(args) => counterparts(ctx, args),
    QueryCommand::Docs(args) => docs(ctx, args),
    QueryCommand::Path(args) => path(ctx, args),
    QueryCommand::Api(args) => api(ctx, args),
    QueryCommand::Similar(args) => similar(ctx, args),
    QueryCommand::Unused(args) => unused(ctx, args),
    QueryCommand::Patterns(args) => patterns(ctx, args),
    QueryCommand::Context(args) => context(ctx, args),
    QueryCommand::Explain(args) => explain(ctx, args),
    QueryCommand::Read(args) => read(ctx, args),
    QueryCommand::Changes(args) => changes(ctx, args),
  }
}

fn open(ctx: &Context) -> CliResult<(QueryEngine, Project)> {
  let project = ctx.resolve()?;
  Ok((ctx.open_query(&project)?, project))
}

fn target_filter(args: &TargetFilterArgs) -> TargetFilter {
  TargetFilter {
    kinds: args.kinds.clone(),
    repo: args.repo.clone(),
  }
}

fn search(ctx: &Context, args: SearchArgs) -> CliResult<Outcome> {
  let (query, project) = open(ctx)?;
  let mut request = SearchRequest::new(args.text);
  request.kinds = args.kinds;
  if let Some(repo) = args.repo {
    let Some(scope) = query.search_scope(&repo)? else {
      let repos: Vec<String> = project.repos.iter().map(|repo| repo.name.to_string()).collect();
      return Err(CliError::InvalidInput(format!(
        "repo {repo:?} is not in project {:?}; its repos are: {}",
        project.name.as_str(),
        repos.join(", ")
      )));
    };
    request.repo = Some(scope.repo);
    request.path_prefix = scope.path_prefix;
  }
  request.include_stale = args.include_stale;
  if let Some(limit) = args.limit {
    request.limit = limit;
  }
  finish(query.search(&request)?, args.common.json)
}

fn symbol(ctx: &Context, args: SymbolArgs) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let mut request = SymbolRequest::new(args.target);
  request.filter = target_filter(&args.filter);
  request.body = args.body;
  finish_targeted(query.symbol(&request)?, args.common.json)
}

fn traverse(ctx: &Context, kind: TraversalKind, args: TraverseArgs, via_http: bool) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let mut request = TraverseRequest::new(args.target);
  request.filter = target_filter(&args.filter);
  request.min_confidence = args.min_confidence;
  request.via_http = via_http;
  if let Some(depth) = args.depth {
    request.depth = depth;
  }
  if let Some(limit) = args.limit {
    request.limit = limit;
  }
  finish_targeted(query.traverse(kind, &request)?, args.common.json)
}

fn impact(ctx: &Context, args: ImpactArgs) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let mut request = ImpactRequest::new(args.targets);
  request.filter = target_filter(&args.filter);
  request.min_confidence = args.min_confidence;
  if let Some(depth) = args.depth {
    request.depth = depth;
  }
  if let Some(limit) = args.limit {
    request.limit = limit;
  }
  finish_targeted(query.impact(&request)?, args.common.json)
}

fn tests_for(ctx: &Context, args: TestsForArgs) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let mut request = TestsForRequest::new(args.target);
  request.filter = target_filter(&args.filter);
  request.min_confidence = args.min_confidence;
  if let Some(limit) = args.limit {
    request.limit = limit;
  }
  finish_targeted(query.tests_for(&request)?, args.common.json)
}

fn counterparts(ctx: &Context, args: CounterpartsArgs) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let mut request = CounterpartsRequest::new(args.target);
  request.filter = target_filter(&args.filter);
  let response = query.counterparts(&request)?;
  let none = matches!(&response.answer, Targeted::Found(answer) if answer.counterparts.is_empty());
  Ok(match finish_targeted(response, args.common.json)? {
    Outcome::Complete if none => Outcome::Unresolved,
    outcome => outcome,
  })
}

fn docs(ctx: &Context, args: DocsArgs) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let request = DocsRequest {
    query: args.query,
    target: args.about,
    notes_only: args.notes,
    limit: args.limit.unwrap_or(DEFAULT_DOCS_LIMIT),
  };
  let response = query.docs(&request)?;
  let none = matches!(&response.answer, Targeted::Found(answer) if answer.sections.is_empty());
  Ok(match finish_targeted(response, args.common.json)? {
    Outcome::Complete if none => Outcome::Unresolved,
    outcome => outcome,
  })
}

fn path(ctx: &Context, args: PathArgs) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let mut request = PathRequest::new(args.from, args.to);
  request.filter = target_filter(&args.filter);
  if let Some(max_depth) = args.max_depth {
    request.max_depth = max_depth;
  }
  let response = query.path(&request)?;
  let none = matches!(&response.answer, Targeted::Found(answer) if !answer.found());
  Ok(match finish_targeted(response, args.common.json)? {
    Outcome::Complete if none => Outcome::Unresolved,
    outcome => outcome,
  })
}

pub fn notes(ctx: &Context, command: NotesCommand) -> CliResult<Outcome> {
  match command {
    NotesCommand::List(args) => {
      let (query, _) = open(ctx)?;
      let response = query.notes(&NotesRequest { stale_only: args.stale })?;
      let none = response.answer.notes.is_empty();
      Ok(match finish(response, args.common.json)? {
        Outcome::Complete if none => Outcome::Unresolved,
        outcome => outcome,
      })
    }
    NotesCommand::Stamp(args) => crate::commands::edit::stamp_notes(ctx, args),
  }
}

fn api(ctx: &Context, args: ApiArgs) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let mut request = ApiRequest {
    repo: args.repo,
    endpoint: args.endpoint,
    min_confidence: args.min_confidence,
    ..ApiRequest::default()
  };
  if let Some(limit) = args.limit {
    request.limit = limit;
  }
  finish(query.api(&request)?, args.common.json)
}

fn similar(ctx: &Context, args: SimilarArgs) -> CliResult<Outcome> {
  let (query, project) = open(ctx)?;
  if let (Some(source), Some(language)) = (&args.code, args.language) {
    let mut request = SimilarRequest::from_code(code_language(language), read_input(source)?);
    request.filter = target_filter(&args.filter);
    request.include_stale = args.include_stale;
    if let Some(limit) = args.limit {
      request.limit = limit;
    }
    if let Some(threshold) = args.threshold {
      request.threshold = threshold;
    }
    return finish(query.similar_to_code(&request)?, args.common.json);
  }
  let target = match (&args.snippet, &args.target) {
    (Some(snippet), _) => snippet_target(snippet, &ctx.cwd, &project)?,
    (None, Some(target)) => target.clone(),
    (None, None) => return Err(CliError::InvalidSnippet(String::new())),
  };
  let mut request = SimilarRequest::new(target);
  request.filter = target_filter(&args.filter);
  request.include_stale = args.include_stale;
  if let Some(limit) = args.limit {
    request.limit = limit;
  }
  if let Some(threshold) = args.threshold {
    request.threshold = threshold;
  }
  finish_targeted(query.similar(&request)?, args.common.json)
}

fn code_language(language: CodeLanguage) -> Language {
  match language {
    CodeLanguage::Rust => Language::Rust,
    CodeLanguage::Ts => Language::TypeScript,
    CodeLanguage::Tsx => Language::Tsx,
    CodeLanguage::Js => Language::JavaScript,
  }
}

fn unused(ctx: &Context, args: UnusedArgs) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let mut request = UnusedRequest {
    kinds: args.kinds,
    repo: args.repo,
    ..UnusedRequest::default()
  };
  if let Some(limit) = args.limit {
    request.limit = limit;
  }
  finish(query.unused(&request)?, args.common.json)
}

fn patterns(ctx: &Context, args: PatternsArgs) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let mut request = PatternsRequest::new(args.topic);
  request.excerpts = !args.no_excerpts;
  if let Some(limit) = args.limit {
    request.limit = limit;
  }
  finish(query.patterns(&request)?, args.common.json)
}

fn context(ctx: &Context, args: ContextArgs) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let mut request = ContextRequest::new(args.task);
  request.seeds = args.seeds;
  request.known = args.known;
  request.explain = args.explain;
  if let Some(depth) = args.depth {
    request.depth = depth;
  }
  request.min_confidence = args.min_confidence;
  if let Some(budget) = args.budget {
    request.budget = budget;
  }
  finish(query.context(&request)?, args.common.json)
}

fn explain(ctx: &Context, args: ExplainArgs) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let mut request = ExplainRequest::new(args.target);
  request.filter = target_filter(&args.filter);
  finish_targeted(query.explain(&request)?, args.common.json)
}

fn read(ctx: &Context, args: ReadArgs) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let mut request = ReadRequest::new(args.target);
  request.filter = target_filter(&args.filter);
  request.body = !args.signature;
  request.context = args.context;
  finish_targeted(query.read_symbol(&request)?, args.common.json)
}

fn changes(ctx: &Context, args: ChangesArgs) -> CliResult<Outcome> {
  let (query, _) = open(ctx)?;
  let request = ChangesRequest {
    since_run: args.since_run,
    limit: args.limit.unwrap_or_default(),
  };
  finish(query.changes(&request)?, args.common.json)
}

pub fn render<T: RenderText + Serialize>(response: &Response<T>, json: bool) -> CliResult<String> {
  match json {
    true => Ok(render_json(response)?),
    false => Ok(render_text(response)),
  }
}

pub fn finish<T: RenderText + Serialize>(response: Response<T>, json: bool) -> CliResult<Outcome> {
  emit(&render(&response, json)?)?;
  Ok(Outcome::degraded_if(response.freshness.is_degraded()))
}

pub fn finish_targeted<T: RenderText + Serialize>(response: Response<Targeted<T>>, json: bool) -> CliResult<Outcome> {
  emit(&render(&response, json)?)?;
  if !matches!(response.answer, Targeted::Found(_)) {
    return Ok(Outcome::Unresolved);
  }
  Ok(Outcome::degraded_if(response.freshness.is_degraded()))
}

fn snippet_target(raw: &str, cwd: &Path, project: &Project) -> CliResult<String> {
  let invalid = || CliError::InvalidSnippet(raw.to_owned());
  let (location, range) = raw.rsplit_once(':').ok_or_else(invalid)?;
  let (start, end) = range.split_once('-').ok_or_else(invalid)?;
  let (Ok(start), Ok(end)) = (start.parse::<u32>(), end.parse::<u32>()) else {
    return Err(invalid());
  };
  if start == 0 || start > end || location.is_empty() {
    return Err(invalid());
  }
  match repo_relative(location, cwd, project) {
    Some((repo, path)) => Ok(format!("{repo}:{path}:{start}-{end}")),
    None => Ok(raw.to_owned()),
  }
}

fn repo_relative(location: &str, cwd: &Path, project: &Project) -> Option<(String, String)> {
  let canonical = fs::canonicalize(cwd.join(location)).ok().filter(|path| path.is_file())?;
  let (repo, relative) = project
    .repos
    .iter()
    .filter_map(|repo| canonical.strip_prefix(&repo.path).ok().map(|relative| (repo, relative)))
    .max_by_key(|(repo, _)| repo.path.components().count())?;
  let relative = relative.components().map(|part| part.as_os_str().to_str()).collect::<Option<Vec<_>>>()?.join("/");
  Some((repo.name.to_string(), relative))
}
