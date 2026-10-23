#![allow(dead_code)]

use std::fs;
use std::future::pending;
use std::path::{Path, PathBuf};

use atlas_mcp::{AtlasServer, McpError, ServeOptions};
use atlas_project::{AtlasHome, Registry, RepoSpec};
use rmcp::model::{CallToolRequestParams, CallToolResult, ErrorCode};
use rmcp::service::{RunningService, ServiceError};
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub const ACME_REPOS: [&str; 4] = ["acme-api", "acme-sdk", "acme-web", "acme-billing"];
pub const GLOBEX_REPOS: [&str; 2] = ["globex-api", "globex-worker"];

pub struct Harness {
  pub scratch: TempDir,
  pub home: AtlasHome,
  pub server: AtlasServer,
  pub client: RunningService<RoleClient, ()>,
  pub task: JoinHandle<Result<(), McpError>>,
  pub shutdown: oneshot::Sender<()>,
}

fn copy_dir(from: &Path, to: &Path) {
  fs::create_dir_all(to).unwrap();
  for entry in fs::read_dir(from).unwrap() {
    let entry = entry.unwrap();
    let target = to.join(entry.file_name());
    match entry.file_type().unwrap().is_dir() {
      true => copy_dir(&entry.path(), &target),
      false => {
        fs::copy(entry.path(), &target).unwrap();
      }
    }
  }
}

pub fn repo_path(scratch: &Path, repo: &str) -> PathBuf {
  scratch.join("tree").join(repo)
}

pub fn materialise(scratch: &Path, group: &str, repo: &str) -> PathBuf {
  materialise_from(scratch, &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(group), repo)
}

fn materialise_from(scratch: &Path, source: &Path, repo: &str) -> PathBuf {
  let target = repo_path(scratch, repo);
  copy_dir(&source.join(repo), &target);
  target
}

pub fn register(scratch: &Path, home: &AtlasHome, project: &str, group: &str, repos: &[&str]) {
  let specs = repos.iter().map(|repo| RepoSpec::new(materialise(scratch, group, repo)).named(*repo)).collect();
  Registry::load(home).unwrap().create_project(project, specs).unwrap();
}

pub async fn start_counterpart_corpus() -> Harness {
  let scratch = tempfile::tempdir().unwrap();
  let home = AtlasHome::rooted_at(scratch.path().join("home"));
  let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../atlas-query/tests/counterpart-corpus");
  let specs = ["notes-web", "notes-server"]
    .iter()
    .map(|repo| RepoSpec::new(materialise_from(scratch.path(), &corpus, repo)).named(*repo))
    .collect();
  Registry::load(&home).unwrap().create_project("notes", specs).unwrap();
  let options = ServeOptions::new(home.clone(), scratch.path()).with_project("notes");
  serve(scratch, home, AtlasServer::open(&options).unwrap()).await
}

pub async fn start(repos: &[&str]) -> Harness {
  launch(repos, false, None::<fn(&str)>, None).await
}

pub async fn start_watching(repos: &[&str]) -> Harness {
  launch(repos, true, None::<fn(&str)>, None).await
}

pub async fn start_limited(repos: &[&str], max_output_bytes: usize, max_body_lines: usize) -> Harness {
  launch(repos, false, None::<fn(&str)>, Some((max_output_bytes, max_body_lines))).await
}

pub async fn start_with(repos: &[&str], hook: Option<impl Fn(&str) + Send + Sync + 'static>) -> Harness {
  launch(repos, false, hook, None).await
}

pub async fn start_configured(repos: &[&str], hook: impl Fn(&str) + Send + Sync + 'static, configure: impl FnOnce(&mut ServeOptions)) -> Harness {
  launch_configured(repos, false, Some(hook), None, configure).await
}

async fn launch(repos: &[&str], watch: bool, hook: Option<impl Fn(&str) + Send + Sync + 'static>, limits: Option<(usize, usize)>) -> Harness {
  launch_configured(repos, watch, hook, limits, |_| {}).await
}

async fn launch_configured(
  repos: &[&str],
  watch: bool,
  hook: Option<impl Fn(&str) + Send + Sync + 'static>,
  limits: Option<(usize, usize)>,
  configure: impl FnOnce(&mut ServeOptions),
) -> Harness {
  let scratch = tempfile::tempdir().unwrap();
  let home = AtlasHome::rooted_at(scratch.path().join("home"));
  register(scratch.path(), &home, "acme", "acme", repos);
  register(scratch.path(), &home, "globex", "globex", &GLOBEX_REPOS);
  let mut options = ServeOptions::new(home.clone(), scratch.path()).with_project("acme").with_watch(watch);
  if let Some((bytes, lines)) = limits {
    options = options.with_output_limits(bytes, lines);
  }
  configure(&mut options);
  let server = match hook {
    Some(hook) => AtlasServer::open_with_query_hook(&options, hook).unwrap(),
    None => AtlasServer::open(&options).unwrap(),
  };
  serve(scratch, home, server).await
}

async fn serve(scratch: TempDir, home: AtlasHome, server: AtlasServer) -> Harness {
  let (server_io, client_io) = tokio::io::duplex(1 << 20);
  let (shutdown, signal) = oneshot::channel::<()>();
  let task = tokio::spawn(server.clone().run(server_io, async move {
    if signal.await.is_err() {
      pending::<()>().await;
    }
  }));
  let client = ().serve(client_io).await.unwrap();
  Harness {
    scratch,
    home,
    server,
    client,
    task,
    shutdown,
  }
}

impl Harness {
  pub async fn call(&self, tool: &'static str, arguments: Value) -> CallToolResult {
    self.try_call(tool, arguments).await.unwrap()
  }

  pub async fn try_call(&self, tool: &'static str, arguments: Value) -> Result<CallToolResult, ServiceError> {
    let Value::Object(arguments) = arguments else { panic!("arguments must be an object") };
    self.client.call_tool(CallToolRequestParams::new(tool).with_arguments(arguments)).await
  }

  pub async fn text(&self, tool: &'static str, arguments: Value) -> String {
    let result = self.call(tool, arguments).await;
    let text = text_of(&result);
    assert_ne!(result.is_error, Some(true), "{tool} failed: {text}");
    text
  }

  pub async fn failure(&self, tool: &'static str, arguments: Value) -> String {
    let result = self.call(tool, arguments).await;
    assert_eq!(result.is_error, Some(true), "{tool} should have failed: {}", text_of(&result));
    text_of(&result)
  }

  pub async fn stop(self) -> Result<(), McpError> {
    self.client.cancel().await.unwrap();
    self.task.await.unwrap()
  }
}

pub fn text_of(result: &CallToolResult) -> String {
  result
    .content
    .iter()
    .filter_map(|block| block.as_text())
    .map(|text| text.text.as_str())
    .collect::<Vec<_>>()
    .join("\n")
}

pub fn invalid_params(error: ServiceError) -> String {
  match error {
    ServiceError::McpError(data) => {
      assert_eq!(data.code, ErrorCode::INVALID_PARAMS, "{data:?}");
      data.message.to_string()
    }
    other => panic!("expected an MCP invalid_params error, got {other:?}"),
  }
}
