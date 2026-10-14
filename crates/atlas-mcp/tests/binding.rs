mod common;

use std::fs;
use std::future::pending;
use std::path::{Path, PathBuf};
use std::process::Command;

use atlas_mcp::{McpError, ServeOptions, Server};
use atlas_project::{AtlasHome, ProjectError, Registry, RepoSpec};
use common::text_of;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const LIB_RS: &str = "pub fn shipping_fee(weight: u32) -> u32 {\n  weight * 3\n}\n";

struct Running {
  client: RunningService<RoleClient, ()>,
  task: JoinHandle<Result<(), McpError>>,
  _shutdown: oneshot::Sender<()>,
}

impl Running {
  async fn start(server: Server) -> Self {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    let (shutdown, signal) = oneshot::channel::<()>();
    let task = tokio::spawn(server.run(server_io, async move {
      if signal.await.is_err() {
        pending::<()>().await;
      }
    }));
    let client = ().serve(client_io).await.unwrap();
    Self {
      client,
      task,
      _shutdown: shutdown,
    }
  }

  async fn call(&self, tool: &'static str, arguments: Value) -> CallToolResult {
    let Value::Object(arguments) = arguments else { panic!("arguments must be an object") };
    self.client.call_tool(CallToolRequestParams::new(tool).with_arguments(arguments)).await.unwrap()
  }

  async fn stop(self) {
    self.client.cancel().await.unwrap();
    self.task.await.unwrap().unwrap();
  }
}

fn git(cwd: &Path, args: &[&str]) {
  let output = Command::new("git")
    .args(args)
    .current_dir(cwd)
    .env("GIT_CONFIG_GLOBAL", "/dev/null")
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .env("GIT_AUTHOR_NAME", "atlas")
    .env("GIT_AUTHOR_EMAIL", "atlas@example.invalid")
    .env("GIT_COMMITTER_NAME", "atlas")
    .env("GIT_COMMITTER_EMAIL", "atlas@example.invalid")
    .output()
    .unwrap();
  assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
}

fn shop_with_worktree(root: &Path) -> (PathBuf, PathBuf) {
  let main = root.join("shop");
  fs::create_dir_all(main.join("src")).unwrap();
  fs::write(main.join("Cargo.toml"), "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2024\"\n").unwrap();
  fs::write(main.join("src/lib.rs"), LIB_RS).unwrap();
  git(&main, &["init", "--quiet", "--initial-branch=trunk"]);
  git(&main, &["add", "."]);
  git(&main, &["commit", "--quiet", "--message", "seed"]);
  git(&main, &["worktree", "add", "--quiet", "-b", "feature", ".worktrees/feature"]);
  let worktree = main.join(".worktrees/feature");
  (main, worktree)
}

fn scratch() -> (TempDir, PathBuf, AtlasHome) {
  let dir = tempfile::tempdir().unwrap();
  let root = fs::canonicalize(dir.path()).unwrap();
  let home = AtlasHome::rooted_at(root.join("home"));
  (dir, root, home)
}

#[tokio::test]
async fn should_answer_from_the_main_checkout_with_a_caveat_and_refuse_edits_in_a_worktree() {
  let (_dir, root, home) = scratch();
  let (main, worktree) = shop_with_worktree(&root);
  Registry::load(&home).unwrap().create_project("shop", vec![RepoSpec::new(&main)]).unwrap();
  let server = Server::open(&ServeOptions::new(home, worktree.join("src"))).unwrap();
  let caveat = format!(
    "worktree caveat: answers describe the main checkout at {}, not the git worktree at {} (repo shop of project shop); edits are refused here",
    main.display(),
    worktree.display()
  );
  let refusal = format!(
    "refusing to edit from the git worktree at {}: project shop indexes the main checkout at {} (repo shop), so the edit would write there, not into this worktree; nothing was written. Edit the worktree's files directly, or run the edit from {}",
    worktree.display(),
    main.display(),
    main.display()
  );
  let running = Running::start(server).await;

  let search = running.call("atlas_search", json!({ "query": "shipping_fee" })).await;
  let as_json = running.call("atlas_search", json!({ "query": "shipping_fee", "format": "json" })).await;
  let edit = running
    .call(
      "atlas_edit",
      json!({ "operation": "replace_content", "path": "src/lib.rs", "pattern": "weight * 3", "text": "weight * 4" }),
    )
    .await;
  let dry_run = running
    .call(
      "atlas_edit",
      json!({ "operation": "replace_content", "path": "src/lib.rs", "pattern": "weight * 3", "text": "weight * 4", "dry_run": true }),
    )
    .await;

  assert_ne!(search.is_error, Some(true), "{}", text_of(&search));
  assert!(text_of(&search).contains("shipping_fee"), "{}", text_of(&search));
  assert!(text_of(&search).ends_with(&format!("\n{caveat}\n")), "{}", text_of(&search));
  let document: Value = serde_json::from_str(&text_of(&as_json)).unwrap();
  assert_eq!(document["caveat"], caveat);
  assert_eq!((edit.is_error, text_of(&edit)), (Some(true), refusal.clone()));
  assert_eq!((dry_run.is_error, text_of(&dry_run)), (Some(true), refusal));
  assert_eq!(fs::read_to_string(main.join("src/lib.rs")).unwrap(), LIB_RS);
  assert_eq!(fs::read_to_string(worktree.join("src/lib.rs")).unwrap(), LIB_RS);
  running.stop().await;
}

#[tokio::test]
async fn should_refuse_edits_from_a_worktree_of_a_repo_inside_a_registered_parent_folder() {
  let (_dir, root, home) = scratch();
  let workspace = root.join("workspace");
  let (app, worktree) = shop_with_worktree(&workspace);
  Registry::load(&home).unwrap().create_project("ws", vec![RepoSpec::new(&workspace)]).unwrap();
  let running = Running::start(Server::open(&ServeOptions::new(home, &worktree)).unwrap()).await;

  let search = running.call("atlas_search", json!({ "query": "shipping_fee" })).await;
  let edit = running
    .call(
      "atlas_edit",
      json!({ "operation": "replace_content", "path": "shop/src/lib.rs", "pattern": "weight * 3", "text": "weight * 4" }),
    )
    .await;

  assert!(
    text_of(&search).ends_with(&format!(
      "worktree caveat: answers describe the main checkout at {}, not the git worktree at {} (repo workspace of project ws); edits are refused here\n",
      app.display(),
      worktree.display()
    )),
    "{}",
    text_of(&search)
  );
  assert_eq!(edit.is_error, Some(true));
  assert!(
    text_of(&edit).starts_with(&format!("refusing to edit from the git worktree at {}:", worktree.display())),
    "{}",
    text_of(&edit)
  );
  assert_eq!(fs::read_to_string(app.join("src/lib.rs")).unwrap(), LIB_RS);
  running.stop().await;
}

#[tokio::test]
async fn should_refuse_edits_from_a_worktree_whose_git_file_no_longer_resolves() {
  let (_dir, root, home) = scratch();
  let (main, worktree) = shop_with_worktree(&root);
  fs::remove_dir_all(main.join(".git/worktrees/feature")).unwrap();
  Registry::load(&home).unwrap().create_project("shop", vec![RepoSpec::new(&main)]).unwrap();
  let running = Running::start(Server::open(&ServeOptions::new(home, &worktree)).unwrap()).await;

  let edit = running
    .call(
      "atlas_edit",
      json!({ "operation": "replace_content", "path": "src/lib.rs", "pattern": "weight * 3", "text": "weight * 4" }),
    )
    .await;

  assert_eq!(edit.is_error, Some(true));
  assert!(
    text_of(&edit).starts_with(&format!(
      "refusing to edit from {}: its .git entry does not resolve to a git checkout, so the worktree could not be resolved",
      worktree.display()
    )),
    "{}",
    text_of(&edit)
  );
  assert_eq!(fs::read_to_string(main.join("src/lib.rs")).unwrap(), LIB_RS);
  running.stop().await;
}

#[tokio::test]
async fn should_edit_and_answer_without_a_caveat_in_the_main_checkout() {
  let (_dir, root, home) = scratch();
  let (main, _worktree) = shop_with_worktree(&root);
  Registry::load(&home).unwrap().create_project("shop", vec![RepoSpec::new(&main)]).unwrap();
  let running = Running::start(Server::open(&ServeOptions::new(home, &main)).unwrap()).await;

  let search = running.call("atlas_search", json!({ "query": "shipping_fee" })).await;
  let edit = running
    .call(
      "atlas_edit",
      json!({ "operation": "replace_content", "path": "src/lib.rs", "pattern": "weight * 3", "text": "weight * 4" }),
    )
    .await;

  assert!(!text_of(&search).contains("worktree caveat"), "{}", text_of(&search));
  assert_ne!(edit.is_error, Some(true), "{}", text_of(&edit));
  assert_eq!(fs::read_to_string(main.join("src/lib.rs")).unwrap(), LIB_RS.replace("weight * 3", "weight * 4"));
  running.stop().await;
}

#[tokio::test]
async fn should_serve_only_atlas_status_where_no_project_is_bound() {
  let (_dir, root, home) = scratch();
  let elsewhere = root.join("elsewhere");
  fs::create_dir_all(&elsewhere).unwrap();
  let server = Server::open(&ServeOptions::new(home, &elsewhere)).unwrap();
  assert!(matches!(server, Server::Unbound(_)));
  let running = Running::start(server).await;
  let explanation = format!(
    "no project is bound: {} is not inside any registered Code Atlas project, so this server answers nothing but atlas_status. To index this repo, run `catl init -p <project>` in it (adds it to that project, creating the project if needed) or `catl project create <name> --repo <path>`, then restart this MCP server.",
    elsewhere.display()
  );

  let tools = running.client.list_all_tools().await.unwrap();
  let status = running.call("atlas_status", json!({})).await;
  let as_json = running.call("atlas_status", json!({ "format": "json" })).await;
  let search = running
    .client
    .call_tool(CallToolRequestParams::new("atlas_search").with_arguments(serde_json::Map::new()))
    .await;
  let instructions = running.client.peer_info().unwrap().instructions.clone().unwrap();

  assert_eq!(tools.iter().map(|tool| tool.name.as_ref()).collect::<Vec<_>>(), ["atlas_status"]);
  assert!(tools[0].input_schema["properties"]["format"].is_object(), "{:?}", tools[0].input_schema);
  assert_ne!(status.is_error, Some(true));
  assert_eq!(text_of(&status), format!("{explanation}\n"));
  assert_eq!(
    serde_json::from_str::<Value>(&text_of(&as_json)).unwrap(),
    json!({ "bound": false, "cwd": elsewhere.display().to_string(), "message": explanation })
  );
  assert!(search.is_err(), "{search:?}");
  assert!(instructions.contains("serves only atlas_status"), "{instructions}");
  running.stop().await;
}

#[tokio::test]
async fn should_keep_failing_on_an_ambiguous_directory_or_an_unknown_explicit_project() {
  let (_dir, root, home) = scratch();
  let shared = root.join("shared");
  fs::create_dir_all(&shared).unwrap();
  let mut registry = Registry::load(&home).unwrap();
  registry.create_project("alpha", vec![RepoSpec::new(&shared)]).unwrap();
  registry.create_project("zeta", vec![RepoSpec::new(&shared)]).unwrap();
  let elsewhere = root.join("elsewhere");
  fs::create_dir_all(&elsewhere).unwrap();

  let ambiguous = Server::open(&ServeOptions::new(home.clone(), &shared));
  let unknown = Server::open(&ServeOptions::new(home, &elsewhere).with_project("ghost"));

  assert!(
    matches!(&ambiguous, Err(McpError::Project(ProjectError::Ambiguous { candidates, .. })) if candidates == &["alpha", "zeta"]),
    "{:?}",
    ambiguous.err()
  );
  assert!(
    matches!(&unknown, Err(McpError::Project(ProjectError::ProjectNotFound(name))) if name == "ghost"),
    "{:?}",
    unknown.err()
  );
}
