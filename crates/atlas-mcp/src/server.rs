use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use atlas_engine::Engine;
use atlas_ide::EditStatus;
use atlas_query::{
  ApiRequest, ChangesRequest, ContextRequest, ExplainRequest, ImpactRequest, PatternsRequest, QueryEngine, SearchRequest, Targeted, TestsForRequest, TraverseRequest, UnusedRequest,
};
use rmcp::ErrorData;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::service::RequestContext;
use rmcp::transport::IntoTransport;
use rmcp::{RoleServer, ServerHandler, ServiceExt, tool, tool_handler, tool_router};

use crate::arguments::{
  self, ApiArgs, ContextArgs, EditArgs, Format, ImpactArgs, NoArgs, PatternsArgs, SearchArgs, SimilarArgs, SimilarPlan, StatusArgs, SymbolArgs, SymbolPlan, TargetArgs,
  TraverseArgs, UnusedArgs,
};
use crate::descriptions as text;
use crate::error::{McpError, ToolError};
use crate::guidance::{self, ToolProfile};
use crate::options::ServeOptions;
use crate::render;
use crate::session::Session;
use crate::telemetry::{Telemetry, TelemetrySnapshot};

type QueryHook = Arc<dyn Fn(&str) + Send + Sync>;

pub(crate) struct Shared {
  pub(crate) session: Session,
  telemetry: Telemetry,
  before_query: Option<QueryHook>,
  profile: ToolProfile,
  max_output_bytes: usize,
  max_body_lines: usize,
}

#[derive(Clone)]
pub struct AtlasServer {
  shared: Arc<Shared>,
  tool_router: ToolRouter<Self>,
}

impl AtlasServer {
  pub fn open(options: &ServeOptions) -> Result<Self, McpError> {
    Self::build(options, None)
  }

  #[doc(hidden)]
  pub fn open_with_query_hook(options: &ServeOptions, hook: impl Fn(&str) + Send + Sync + 'static) -> Result<Self, McpError> {
    Self::build(options, Some(Arc::new(hook)))
  }

  fn build(options: &ServeOptions, before_query: Option<QueryHook>) -> Result<Self, McpError> {
    let profile = ToolProfile::from_setting(&options.tools)?;
    let tool_router = profile_router(compact_router(Self::tool_router()), &profile);
    Ok(Self {
      shared: Arc::new(Shared {
        session: Session::open(options)?,
        telemetry: Telemetry::new(),
        before_query,
        profile,
        max_output_bytes: options.max_output_bytes,
        max_body_lines: options.max_body_lines,
      }),
      tool_router,
    })
  }

  pub fn project_name(&self) -> &str {
    self.shared.session.project_name()
  }

  #[doc(hidden)]
  pub fn engine(&self) -> &Arc<Engine> {
    self.shared.session.engine()
  }

  pub fn telemetry(&self) -> TelemetrySnapshot {
    self.shared.telemetry.snapshot()
  }

  pub async fn run<T, E, A>(self, transport: T, shutdown: impl Future<Output = ()>) -> Result<(), McpError>
  where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
  {
    let shared = Arc::clone(&self.shared);
    let warm = Arc::clone(&shared);
    tokio::task::spawn_blocking(move || {
      if let Err(error) = warm.session.engine().ensure_fresh() {
        tracing::warn!(error = %error, "warm-up index update failed; the first tool call retries");
      }
    });
    let outcome = serve_until(self, transport, shutdown).await;
    let finished = tokio::task::spawn_blocking(move || shared.session.shut_down()).await;
    if let Err(error) = finished {
      tracing::error!(error = %error, "shutdown cleanup failed");
    }
    outcome
  }

  async fn call<F>(&self, tool: &'static str, format: Option<Format>, query: F) -> CallToolResult
  where
    F: FnOnce(&Shared) -> Result<String, ToolError> + Send + 'static,
  {
    let started = Instant::now();
    let shared = Arc::clone(&self.shared);
    let joined = tokio::task::spawn_blocking(move || {
      if let Some(hook) = &shared.before_query {
        hook(tool);
      }
      shared.session.sync_registry()?;
      query(&shared)
    })
    .await;
    let outcome = joined.unwrap_or_else(|error| Err(ToolError::from_join(tool, error)));
    let caveat = self.shared.session.caveat();
    let (result, bytes, failed, truncated) = match outcome {
      Ok(text) => {
        let format = format.unwrap_or_default();
        let (text, truncated) = render::cap_with_caveat(text, format, self.shared.max_output_bytes, caveat.as_deref());
        let bytes = text.len();
        (CallToolResult::success(vec![ContentBlock::text(text)]), bytes, false, truncated)
      }
      Err(error) => {
        let caveat = caveat.filter(|_| !matches!(error, ToolError::Refused(_)));
        let message = render::with_caveat(error.to_string(), Format::Text, caveat.as_deref());
        tracing::warn!(tool, error = %message, "tool call failed");
        let bytes = message.len();
        (CallToolResult::error(vec![ContentBlock::text(message)]), bytes, true, 0)
      }
    };
    self.shared.telemetry.record(tool, started.elapsed(), bytes, failed, truncated);
    result
  }
}

pub(crate) async fn serve_until<S, T, E, A>(server: S, transport: T, shutdown: impl Future<Output = ()>) -> Result<(), McpError>
where
  S: ServerHandler,
  T: IntoTransport<RoleServer, E, A>,
  E: std::error::Error + Send + Sync + 'static,
{
  let running = server.serve(transport).await.map_err(|error| McpError::Transport(error.to_string()))?;
  let cancel = running.cancellation_token();
  let waiting = running.waiting();
  tokio::pin!(waiting);
  let outcome = tokio::select! {
    outcome = &mut waiting => outcome,
    () = shutdown => {
      cancel.cancel();
      waiting.await
    }
  };
  outcome.map(|_| ()).map_err(|error| McpError::Transport(error.to_string()))
}

pub(crate) fn compact_router<S>(mut router: ToolRouter<S>) -> ToolRouter<S> {
  for route in router.map.values_mut() {
    route.attr.input_schema = Arc::new(compact_schema(&route.attr.input_schema));
  }
  router
}

fn profile_router(mut router: ToolRouter<AtlasServer>, profile: &ToolProfile) -> ToolRouter<AtlasServer> {
  router.map.retain(|name, _| profile.contains(name));
  router
}

fn compact_schema(schema: &rmcp::model::JsonObject) -> rmcp::model::JsonObject {
  use serde_json::Value;
  let mut schema = schema.clone();
  schema.remove("$schema");
  let defs = match schema.remove("$defs") {
    Some(Value::Object(defs)) => defs,
    _ => serde_json::Map::new(),
  };
  let Some(Value::Object(properties)) = schema.get_mut("properties") else { return schema };
  for property in properties.values_mut() {
    let Value::Object(property) = property else { continue };
    if let Some(Value::Array(variants)) = property.remove("anyOf") {
      let mut rest: Vec<Value> = variants.into_iter().filter(|variant| variant != &serde_json::json!({"type": "null"})).collect();
      match (rest.len(), rest.pop()) {
        (1, Some(Value::Object(only))) => property.extend(only),
        (_, last) => {
          rest.extend(last);
          property.insert("anyOf".to_owned(), Value::Array(rest));
        }
      }
    }
    if let Some(Value::String(reference)) = property.remove("$ref")
      && let Some(Value::Object(definition)) = defs.get(reference.trim_start_matches("#/$defs/"))
    {
      property.extend(definition.clone());
    }
    if let Some(Value::Array(types)) = property.get_mut("type") {
      types.retain(|kind| kind != "null");
      if types.len() == 1 {
        let only = types.remove(0);
        property.insert("type".to_owned(), only);
      }
    }
    if property.get("type").and_then(Value::as_str) == Some("integer") {
      property.remove("format");
    }
  }
  schema
}

fn removed_tool_replacement(name: &str, profile: &ToolProfile) -> Option<String> {
  let (replacement, usage) = match name {
    "atlas_read" => ("atlas_symbol", "atlas_symbol mode=source"),
    "atlas_changes" => ("atlas_status", "atlas_status since_run"),
    _ => return None,
  };
  match profile.contains(replacement) {
    true => Some(format!("{name} was removed; use {usage}")),
    false => Some(format!("{name} was removed; its replacement is not in profile {}", profile.name())),
  }
}

fn check_repo(shared: &Shared, repo: Option<&str>) -> Result<(), ToolError> {
  let Some(repo) = repo else { return Ok(()) };
  let repos = shared.session.repo_names();
  if repos.iter().any(|name| name == repo) {
    return Ok(());
  }
  Err(ToolError::invalid(format!(
    "repo {repo:?} is not in project {:?}; its repos are: {}",
    shared.session.project_name(),
    repos.join(", ")
  )))
}

fn query(shared: &Shared) -> &QueryEngine {
  shared.session.query()
}

#[tool_router]
impl AtlasServer {
  #[tool(name = "atlas_context", description = text::CONTEXT, annotations(read_only_hint = true))]
  async fn context(&self, Parameters(args): Parameters<ContextArgs>) -> CallToolResult {
    self
      .call("atlas_context", args.format, move |shared| {
        arguments::non_empty("task", &args.task)?;
        let mut request = ContextRequest::new(args.task);
        if let Some(budget) = arguments::budget(args.budget)? {
          request.budget = budget;
        }
        request.seeds = args.seeds.unwrap_or_default();
        request.known = args.known.unwrap_or_default();
        render::plain(&query(shared).context(&request)?, args.format.unwrap_or_default())
      })
      .await
  }

  #[tool(name = "atlas_search", description = text::SEARCH, annotations(read_only_hint = true))]
  async fn search(&self, Parameters(args): Parameters<SearchArgs>) -> CallToolResult {
    self
      .call("atlas_search", args.format, move |shared| {
        arguments::non_empty("query", &args.query)?;
        check_repo(shared, args.repo.as_deref())?;
        let mut request = SearchRequest::new(args.query);
        request.kinds = arguments::kinds(args.kind.as_deref())?;
        request.repo = args.repo;
        if let Some(limit) = arguments::limit(args.limit)? {
          request.limit = limit;
        }
        render::plain(&query(shared).search(&request)?, args.format.unwrap_or_default())
      })
      .await
  }

  #[tool(name = "atlas_symbol", description = text::SYMBOL, annotations(read_only_hint = true))]
  async fn symbol(&self, Parameters(args): Parameters<SymbolArgs>) -> CallToolResult {
    self
      .call("atlas_symbol", args.format, move |shared| {
        let format = args.format.unwrap_or_default();
        let ide = shared.session.ide();
        match arguments::symbol_plan(args)? {
          SymbolPlan::Info(request) => {
            let mut response = query(shared).symbol(&request)?;
            let dropped = match &mut response.answer {
              Targeted::Found(found) => found.body.as_mut().map_or(0, |body| render::cap_body(body, shared.max_body_lines)),
              _ => 0,
            };
            render::targeted_noted(query(shared), &response, format, render::body_note(dropped, shared.max_body_lines))
          }
          SymbolPlan::Source(request) => {
            let mut response = query(shared).read_symbol(&request)?;
            let dropped = match &mut response.answer {
              Targeted::Found(found) => render::cap_body(&mut found.source, shared.max_body_lines),
              _ => 0,
            };
            render::targeted_noted(query(shared), &response, format, render::body_note(dropped, shared.max_body_lines))
          }
          SymbolPlan::Find(request) => render::plain(&ide.find(&request)?, format),
          SymbolPlan::Outline(request) => render::plain(&ide.outline(&request)?, format),
          SymbolPlan::References(request) => render::plain(&ide.references(&request)?, format),
          SymbolPlan::Declaration(request) => render::plain(&ide.declaration(&request)?, format),
          SymbolPlan::Implementations(target) => render::targeted(query(shared), &ide.implementations(&target)?, format),
          SymbolPlan::Hierarchy(request) => render::targeted(query(shared), &query(shared).hierarchy(&request)?, format),
          SymbolPlan::Diagnostics(request) => render::plain(&ide.diagnostics(&request)?, format),
        }
      })
      .await
  }

  #[tool(
    name = "atlas_edit",
    description = text::edit(),
    annotations(destructive_hint = true, read_only_hint = false, idempotent_hint = false)
  )]
  async fn edit(&self, Parameters(args): Parameters<EditArgs>) -> CallToolResult {
    self
      .call("atlas_edit", args.format, move |shared| {
        if let Some(refusal) = shared.session.edit_refusal() {
          return Err(ToolError::Refused(refusal));
        }
        let format = args.format.unwrap_or_default();
        let request = arguments::edit_request(args)?;
        let response = shared.session.ide().edit(&request)?;
        let rendered = render::plain(&response, format)?;
        match response.answer.status {
          EditStatus::Refused { .. } | EditStatus::RolledBack { .. } | EditStatus::PartiallyApplied { .. } => {
            Err(ToolError::Refused(render::cap(rendered, format, shared.max_output_bytes).0))
          }
          EditStatus::Applied | EditStatus::DryRun { .. } => Ok(rendered),
        }
      })
      .await
  }

  #[tool(name = "atlas_callers", description = text::CALLERS, annotations(read_only_hint = true))]
  async fn callers(&self, Parameters(args): Parameters<TraverseArgs>) -> CallToolResult {
    self
      .call("atlas_callers", args.format, move |shared| {
        arguments::non_empty("target", &args.target)?;
        let mut request = TraverseRequest::new(args.target);
        request.depth = arguments::depth(args.depth, request.depth)?;
        request.via_http = args.via_http.unwrap_or(false);
        render::targeted(query(shared), &query(shared).callers(&request)?, args.format.unwrap_or_default())
      })
      .await
  }

  #[tool(name = "atlas_callees", description = text::CALLEES, annotations(read_only_hint = true))]
  async fn callees(&self, Parameters(args): Parameters<TraverseArgs>) -> CallToolResult {
    self
      .call("atlas_callees", args.format, move |shared| {
        arguments::non_empty("target", &args.target)?;
        let mut request = TraverseRequest::new(args.target);
        request.depth = arguments::depth(args.depth, request.depth)?;
        request.via_http = args.via_http.unwrap_or(false);
        render::targeted(query(shared), &query(shared).callees(&request)?, args.format.unwrap_or_default())
      })
      .await
  }

  #[tool(name = "atlas_impact", description = text::IMPACT, annotations(read_only_hint = true))]
  async fn impact(&self, Parameters(args): Parameters<ImpactArgs>) -> CallToolResult {
    self
      .call("atlas_impact", args.format, move |shared| {
        if args.targets.is_empty() || args.targets.iter().any(|target| target.trim().is_empty()) {
          return Err(ToolError::invalid("targets must be a non-empty list of non-empty strings"));
        }
        let mut request = ImpactRequest::new(args.targets);
        request.depth = arguments::depth(args.depth, request.depth)?;
        render::targeted(query(shared), &query(shared).impact(&request)?, args.format.unwrap_or_default())
      })
      .await
  }

  #[tool(name = "atlas_tests_for", description = text::TESTS_FOR, annotations(read_only_hint = true))]
  async fn tests_for(&self, Parameters(args): Parameters<TargetArgs>) -> CallToolResult {
    self
      .call("atlas_tests_for", args.format, move |shared| {
        arguments::non_empty("target", &args.target)?;
        render::targeted(
          query(shared),
          &query(shared).tests_for(&TestsForRequest::new(args.target))?,
          args.format.unwrap_or_default(),
        )
      })
      .await
  }

  #[tool(name = "atlas_api", description = text::API, annotations(read_only_hint = true))]
  async fn api(&self, Parameters(args): Parameters<ApiArgs>) -> CallToolResult {
    self
      .call("atlas_api", args.format, move |shared| {
        check_repo(shared, args.repo.as_deref())?;
        let request = ApiRequest {
          repo: args.repo,
          endpoint: args.endpoint,
          ..ApiRequest::default()
        };
        render::plain(&query(shared).api(&request)?, args.format.unwrap_or_default())
      })
      .await
  }

  #[tool(name = "atlas_similar", description = text::SIMILAR, annotations(read_only_hint = true))]
  async fn similar(&self, Parameters(args): Parameters<SimilarArgs>) -> CallToolResult {
    self
      .call("atlas_similar", args.format, move |shared| {
        let format = args.format.unwrap_or_default();
        match arguments::similar_plan(args)? {
          SimilarPlan::Indexed(request) => render::targeted(query(shared), &query(shared).similar(&request)?, format),
          SimilarPlan::Draft(request) => render::plain(&query(shared).similar_to_code(&request)?, format),
        }
      })
      .await
  }

  #[tool(name = "atlas_unused", description = text::UNUSED, annotations(read_only_hint = true))]
  async fn unused(&self, Parameters(args): Parameters<UnusedArgs>) -> CallToolResult {
    self
      .call("atlas_unused", args.format, move |shared| {
        check_repo(shared, args.repo.as_deref())?;
        let request = UnusedRequest {
          kinds: arguments::kinds(args.kind.as_deref())?,
          repo: args.repo,
          ..UnusedRequest::default()
        };
        render::plain(&query(shared).unused(&request)?, args.format.unwrap_or_default())
      })
      .await
  }

  #[tool(name = "atlas_patterns", description = text::PATTERNS, annotations(read_only_hint = true))]
  async fn patterns(&self, Parameters(args): Parameters<PatternsArgs>) -> CallToolResult {
    self
      .call("atlas_patterns", args.format, move |shared| {
        arguments::non_empty("topic", &args.topic)?;
        render::plain(&query(shared).patterns(&PatternsRequest::new(args.topic))?, args.format.unwrap_or_default())
      })
      .await
  }

  #[tool(name = "atlas_explain", description = text::EXPLAIN, annotations(read_only_hint = true))]
  async fn explain(&self, Parameters(args): Parameters<TargetArgs>) -> CallToolResult {
    self
      .call("atlas_explain", args.format, move |shared| {
        arguments::non_empty("target", &args.target)?;
        render::targeted(query(shared), &query(shared).explain(&ExplainRequest::new(args.target))?, args.format.unwrap_or_default())
      })
      .await
  }

  #[tool(name = "atlas_overview", description = text::OVERVIEW, annotations(read_only_hint = true))]
  async fn overview(&self, Parameters(args): Parameters<NoArgs>) -> CallToolResult {
    self
      .call("atlas_overview", args.format, move |shared| {
        render::plain(&query(shared).overview()?, args.format.unwrap_or_default())
      })
      .await
  }

  #[tool(name = "atlas_status", description = text::STATUS, annotations(read_only_hint = true))]
  async fn status(&self, Parameters(args): Parameters<StatusArgs>) -> CallToolResult {
    self
      .call("atlas_status", args.format, move |shared| {
        let format = args.format.unwrap_or_default();
        if args.since_run.is_some_and(|since_run| since_run < 0) {
          return Err(ToolError::invalid("since_run must not be negative"));
        }
        let response = query(shared).status()?;
        let changes = args
          .since_run
          .map(|since_run| {
            query(shared).changes(&ChangesRequest {
              since_run: Some(since_run),
              limit: 0,
            })
          })
          .transpose()?;
        let servers = shared.session.ide().status();
        let telemetry = shared.telemetry.snapshot();
        match format {
          Format::Text => {
            let mut text = render::plain(&response, format)? + &render::answer_text(&servers);
            if let (Some(since_run), Some(changes)) = (args.since_run, &changes) {
              let current = changes.freshness.run.map_or_else(|| "none".to_owned(), |run| run.to_string());
              text.push_str(&format!(
                "changes since run {since_run} (index at run {current}):\n{}",
                render::answer_text(&changes.answer)
              ));
            }
            Ok(text + &telemetry.render_text())
          }
          Format::Json => render::edit_json(&render::plain(&response, format)?, |document| {
            if let Ok(value) = serde_json::to_value(&servers) {
              document["language_servers"] = value;
            }
            if let Some(value) = changes.as_ref().and_then(|changes| serde_json::to_value(&changes.answer).ok()) {
              document["changes"] = value;
            }
            if let Ok(value) = serde_json::to_value(&telemetry) {
              document["mcp_telemetry"] = value;
            }
          }),
        }
      })
      .await
  }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AtlasServer {
  async fn call_tool(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
    if let Some(replacement) = removed_tool_replacement(&request.name, &self.shared.profile) {
      return Ok(CallToolResult::error(vec![ContentBlock::text(replacement)]).into());
    }
    if let Some(refusal) = self.shared.profile.refusal(&request.name) {
      return Ok(CallToolResult::error(vec![ContentBlock::text(refusal)]).into());
    }
    self.tool_router.call(ToolCallContext::new(self, request, context)).await
  }

  fn get_info(&self) -> ServerConfig {
    ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(guidance::instructions(&self.shared.profile))
  }
}

#[cfg(test)]
mod tests {
  use atlas_project::ToolsSetting;

  use super::*;

  fn routed(router: &ToolRouter<AtlasServer>) -> Vec<String> {
    let mut names: Vec<String> = router.map.keys().map(ToString::to_string).collect();
    names.sort_unstable();
    names
  }

  #[test]
  fn should_route_exactly_the_tools_the_guidance_knows() {
    assert_eq!(routed(&AtlasServer::tool_router()), guidance::TOOLS);
  }

  #[test]
  fn should_route_only_the_tools_of_the_profile() {
    let core = ToolProfile::from_setting(&ToolsSetting::Core).unwrap();
    let read_only = ToolProfile::from_setting(&ToolsSetting::ReadOnly).unwrap();

    assert_eq!(routed(&profile_router(AtlasServer::tool_router(), &core)), guidance::CORE_TOOLS);
    assert_eq!(routed(&profile_router(AtlasServer::tool_router(), &read_only)), read_only.tools());
    assert_eq!(routed(&profile_router(AtlasServer::tool_router(), &ToolProfile::full())), guidance::TOOLS);
  }

  #[test]
  fn should_name_a_removed_tool_replacement_only_when_the_profile_serves_it() {
    let read_only = ToolProfile::from_setting(&ToolsSetting::ReadOnly).unwrap();
    let only_search = ToolProfile::from_setting(&ToolsSetting::Only(vec!["atlas_search".to_owned()])).unwrap();

    assert_eq!(
      removed_tool_replacement("atlas_read", &read_only).as_deref(),
      Some("atlas_read was removed; use atlas_symbol mode=source")
    );
    assert_eq!(
      removed_tool_replacement("atlas_changes", &only_search).as_deref(),
      Some("atlas_changes was removed; its replacement is not in profile custom")
    );
    assert_eq!(removed_tool_replacement("atlas_search", &only_search), None);
  }

  #[test]
  fn should_name_no_other_tool_in_a_description_so_every_profile_stays_consistent() {
    for route in AtlasServer::tool_router().map.values() {
      let description = route.attr.description.as_deref().unwrap_or_default();
      let mut texts = vec![description.to_owned()];
      if let Some(serde_json::Value::Object(properties)) = route.attr.input_schema.get("properties") {
        texts.extend(properties.values().filter_map(|property| property.get("description")?.as_str().map(str::to_owned)));
      }
      for text in texts {
        let others: Vec<&str> = guidance::TOOLS.into_iter().filter(|tool| *tool != route.attr.name && text.contains(tool)).collect();
        assert!(others.is_empty(), "{} names {others:?}: {text}", route.attr.name);
      }
    }
  }
}
