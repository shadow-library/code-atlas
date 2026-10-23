use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Instant;

use atlas_core::RunId;
use atlas_engine::{BarrierMode, BoundProject, Engine, EngineOptions, ExistingBinding, bind_existing_project, deferred_for_changed_files};
use atlas_project::{AtlasHome, Registry};
use atlas_query::{Freshness, FreshnessState, is_test_or_generated_path, may_have_counterparts};
use atlas_store::{Store, StoreOptions};
use serde_json::Value;

use super::counterpart::locate;
use super::{TIME_LIMIT, WAIT_MARGIN};

const EDIT_TOOLS: [&str; 3] = ["Write", "Edit", "MultiEdit"];

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EditEvent {
  pub session: String,
  pub cwd: PathBuf,
  pub file: PathBuf,
  pub tool: String,
  pub input: Value,
}

pub(super) fn edit_event(event: &Value, hook_event: &str) -> Option<EditEvent> {
  (event.get("hook_event_name")?.as_str()? == hook_event).then_some(())?;
  let tool = event.get("tool_name")?.as_str().filter(|tool| EDIT_TOOLS.contains(tool))?.to_owned();
  let input = event.get("tool_input")?.clone();
  let file = PathBuf::from(input.get("file_path")?.as_str()?);
  let cwd = PathBuf::from(event.get("cwd")?.as_str()?);
  let session = event.get("session_id")?.as_str().filter(|session| !session.is_empty())?.to_owned();
  (file.is_absolute() && cwd.is_absolute()).then_some(EditEvent { session, cwd, file, tool, input })
}

#[derive(Debug)]
pub(super) enum Silence {
  NotSource,
  NoProject,
  OtherCheckout,
  OverlayNotSeeded(String),
  OutsideProject,
  AlreadyNoted,
  NeverIndexed,
  EditDoesNotApply,
  TooLarge(u64),
  Stale(String),
  TargetChanged,
  Resynced,
  RunInProgress,
  NothingFound,
  TooLong(usize),
  TimeLimit,
  Failed(&'static str),
}

impl Silence {
  pub fn reason(&self) -> String {
    match self {
      Self::NotSource => "the file is not a Rust or TypeScript source file, or is a test or generated file".to_owned(),
      Self::NoProject => "no project resolves for the working directory".to_owned(),
      Self::OtherCheckout => "the working directory is answered from another checkout".to_owned(),
      Self::OverlayNotSeeded(label) => format!("the worktree overlay {label} is not seeded yet"),
      Self::OutsideProject => "the file is in no repo of the project".to_owned(),
      Self::AlreadyNoted => "this file was already noted in this session".to_owned(),
      Self::NeverIndexed => "the project has never been indexed".to_owned(),
      Self::EditDoesNotApply => "the edit does not apply to the file on disk".to_owned(),
      Self::TooLarge(limit) => format!("the file is over the {limit} byte max_file_bytes limit"),
      Self::Stale(reason) => format!("the index is not fresh: {reason}"),
      Self::TargetChanged => "the edited file changed since the last index run".to_owned(),
      Self::Resynced => "the index changed while answering".to_owned(),
      Self::RunInProgress => "an index run started while answering".to_owned(),
      Self::NothingFound => "nothing to note".to_owned(),
      Self::TooLong(limit) => format!("the note would exceed {limit} characters"),
      Self::TimeLimit => format!("the time limit of {} ms passed", TIME_LIMIT.as_millis()),
      Self::Failed(step) => format!("failed while {step}"),
    }
  }
}

pub(super) fn screen(file: &Path) -> Result<(), Silence> {
  let name = file.file_name().and_then(|name| name.to_str()).unwrap_or_default();
  match may_have_counterparts(name) {
    true => Ok(()),
    false => Err(Silence::NotSource),
  }
}

pub(super) fn run_limited<T: Send + 'static>(name: &str, started: Instant, work: impl FnOnce(Instant) -> Result<T, Silence> + Send + 'static) -> Result<T, Silence> {
  let deadline = started + TIME_LIMIT;
  let (sender, receiver) = mpsc::channel();
  thread::Builder::new()
    .name(name.to_owned())
    .spawn(move || {
      let _ = sender.send(work(deadline));
    })
    .map_err(|_| Silence::Failed("starting the worker"))?;
  match receiver.recv_timeout(TIME_LIMIT) {
    Ok(answered) => answered,
    Err(RecvTimeoutError::Timeout) => Err(Silence::TimeLimit),
    Err(RecvTimeoutError::Disconnected) => Err(Silence::Failed("answering")),
  }
}

pub(super) struct Opened {
  pub bound: BoundProject,
  pub engine: Arc<Engine>,
  pub file: String,
}

pub(super) struct Target<'a> {
  pub home: &'a AtlasHome,
  pub project: Option<&'a str>,
  pub cwd: &'a Path,
  pub file: &'a Path,
  pub deadline: Instant,
}

impl Target<'_> {
  pub fn store_options(&self) -> StoreOptions {
    let remaining = self.deadline.saturating_duration_since(Instant::now()).saturating_sub(WAIT_MARGIN);
    StoreOptions {
      lock_timeout: remaining,
      busy_timeout: remaining,
      ..StoreOptions::default()
    }
  }

  pub fn open(&self, located: impl FnOnce(&str) -> Result<(), Silence>) -> Result<Opened, Silence> {
    let registry = Registry::load(self.home).map_err(|_| Silence::Failed("loading the registry"))?;
    let bound = match bind_existing_project(self.home, &registry, self.cwd, self.project, &self.store_options()) {
      Ok(ExistingBinding::Bound(bound)) => *bound,
      Ok(ExistingBinding::OverlayNotSeeded { label }) => return Err(Silence::OverlayNotSeeded(label)),
      Err(_) => return Err(Silence::NoProject),
    };
    if bound.binding.caveat().is_some() {
      return Err(Silence::OtherCheckout);
    }
    let (repo, path) = locate_maybe_new(&bound, self.file).ok_or(Silence::OutsideProject)?;
    if is_test_or_generated_path(&path) {
      return Err(Silence::NotSource);
    }
    let file = format!("{repo}:{path}");
    located(&file)?;
    let db_path = bound.engine_home.db_path(&bound.project.name);
    if !db_path.is_file() {
      return Err(Silence::NeverIndexed);
    }
    let store = Store::open_with(&db_path, self.store_options()).map_err(|_| Silence::Failed("opening the database"))?;
    let persisted = Engine::persisted_generation_in(&store).map_err(|_| Silence::Failed("reading the index generation"))?;
    drop(store);
    persisted.ok_or(Silence::NeverIndexed)?;
    let options = EngineOptions {
      store: self.store_options(),
      barrier: BarrierMode::CheckOnly,
      ..EngineOptions::default()
    };
    let engine = Arc::new(Engine::open(&bound.project, &bound.engine_home, options).map_err(|_| Silence::Failed("opening the engine"))?);
    Ok(Opened { bound, engine, file })
  }
}

fn locate_maybe_new(bound: &BoundProject, file: &Path) -> Option<(String, String)> {
  if let Some(found) = locate(&bound.project, file) {
    return Some(found);
  }
  let parent = fs::canonicalize(file.parent()?).ok()?;
  let (repo, directory) = locate(&bound.project, &parent).or_else(|| {
    bound
      .project
      .repos
      .iter()
      .find(|repo| fs::canonicalize(&repo.path).is_ok_and(|root| root == parent))
      .map(|repo| (repo.name.to_string(), String::new()))
  })?;
  let name = file.file_name()?.to_str()?;
  Some(match directory.is_empty() {
    true => (repo, name.to_owned()),
    false => (repo, format!("{directory}/{name}")),
  })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Accepted {
  FreshOnly,
  ChangedFiles,
}

pub(super) fn settle(engine: &Engine, freshness: &Freshness, accepted: Accepted) -> Result<i64, Silence> {
  let Some(run) = freshness.run else { return Err(Silence::NeverIndexed) };
  if let FreshnessState::Degraded { reason, failed_files } = &freshness.state {
    let deferred = failed_files.is_empty() && deferred_for_changed_files(reason);
    if accepted == Accepted::FreshOnly || !deferred {
      return Err(Silence::Stale(reason.clone()));
    }
  }
  let generation = engine.persisted_generation().map_err(|_| Silence::Failed("reading the index generation"))?;
  if !freshness.notes.is_empty() || generation != Some(RunId(run)) {
    return Err(Silence::Resynced);
  }
  if engine.index_run_in_progress().map_err(|_| Silence::Failed("reading the run marker"))? {
    return Err(Silence::RunInProgress);
  }
  Ok(run)
}

pub(super) fn clip(text: &str, limit: usize) -> String {
  match text.char_indices().nth(limit) {
    Some((end, _)) => format!("{}…", &text[..end]),
    None => text.to_owned(),
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  fn event(name: &str, tool: &str, input: Value, session: Value) -> Value {
    json!({"session_id": session, "cwd": "/src/web", "hook_event_name": name, "tool_name": tool, "tool_input": input})
  }

  #[test]
  fn should_read_only_edit_events_of_the_named_hook_with_an_absolute_file_a_cwd_and_a_session() {
    let write = edit_event(
      &event("PreToolUse", "Write", json!({"file_path": "/src/web/a.ts", "content": "x"}), json!("s1")),
      "PreToolUse",
    )
    .unwrap();
    assert_eq!(
      (write.session.as_str(), write.cwd.as_path(), write.file.as_path(), write.tool.as_str()),
      ("s1", Path::new("/src/web"), Path::new("/src/web/a.ts"), "Write")
    );
    assert_eq!(write.input["content"], "x");
    let edit = json!({"file_path": "/src/web/a.ts"});
    assert!(edit_event(&event("PostToolUse", "MultiEdit", edit.clone(), json!("s1")), "PostToolUse").is_some());
    assert!(edit_event(&event("PostToolUse", "Edit", edit.clone(), json!("s1")), "PreToolUse").is_none());
    assert!(edit_event(&event("PreToolUse", "Read", edit.clone(), json!("s1")), "PreToolUse").is_none());
    assert!(edit_event(&event("PreToolUse", "Edit", json!({"file_path": "a.ts"}), json!("s1")), "PreToolUse").is_none());
    assert!(edit_event(&event("PreToolUse", "Edit", edit, json!("")), "PreToolUse").is_none());
  }

  #[test]
  fn should_screen_out_files_that_are_not_source_before_binding() {
    assert!(screen(Path::new("/w/web/src/retry.ts")).is_ok());
    assert!(screen(Path::new("/w/server/src/lib.rs")).is_ok());
    for file in ["/w/web/README.md", "/w/web/src/retry.test.ts", "/w/web/src/api.gen.ts", "/w/web/package.json"] {
      assert!(matches!(screen(Path::new(file)), Err(Silence::NotSource)), "{file}");
    }
  }

  #[test]
  fn should_clip_on_a_char_boundary() {
    assert_eq!(clip("ééé", 2), "éé…");
    assert_eq!(clip("ab", 2), "ab");
  }
}
