use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use atlas_core::RunId;
use atlas_engine::{BarrierMode, Engine, EngineOptions, ExistingBinding, bind_existing_project};
use atlas_project::{AtlasHome, Project, Registry};
use atlas_query::{
  Counterpart, CounterpartSymbol, CounterpartsAnswer, CounterpartsRequest, FreshnessState, QueryEngine, Targeted, is_test_or_generated_path, may_have_counterparts,
};
use atlas_store::{Store, StoreOptions};
use serde_json::{Value, json};

use super::{SessionMarkers, TIME_LIMIT, WAIT_MARGIN};

const POST_TOOL_USE: &str = "PostToolUse";
const MAX_NOTE_CHARS: usize = 1_199;
const NOTE_SHARED_SHOWN: usize = 2;
const HOOK_MIN_CONFIDENCE: f32 = 0.6;

pub(super) struct Touched {
  pub session: String,
  pub cwd: PathBuf,
  pub file: PathBuf,
}

struct Job<Seen> {
  home: AtlasHome,
  project: Option<String>,
  cwd: PathBuf,
  file: PathBuf,
  session: String,
  seen: Seen,
  deadline: Instant,
}

struct Answered {
  file: String,
  generation: i64,
  note: Result<String, Silence>,
}

#[derive(Debug)]
enum Silence {
  NotSource,
  TestOrGenerated,
  NoProject,
  OtherCheckout,
  OverlayNotSeeded(String),
  OutsideProject,
  AlreadyNoted,
  NeverIndexed,
  AlreadyAnswered,
  NotIndexed,
  Stale(String),
  Resynced,
  RunInProgress,
  NoCounterpart,
  BelowFloor,
  TooLong,
  Failed(&'static str),
}

impl Silence {
  fn reason(&self) -> String {
    match self {
      Self::NotSource => "the file is not a Rust or TypeScript source file".to_owned(),
      Self::TestOrGenerated => "the file is a test or generated file".to_owned(),
      Self::NoProject => "no project resolves for the working directory".to_owned(),
      Self::OtherCheckout => "the working directory is answered from another checkout".to_owned(),
      Self::OverlayNotSeeded(label) => format!("the worktree overlay {label} is not seeded yet"),
      Self::OutsideProject => "the file is in no repo of the project".to_owned(),
      Self::AlreadyNoted => "this file was already noted in this session".to_owned(),
      Self::NeverIndexed => "the project has never been indexed".to_owned(),
      Self::AlreadyAnswered => "this file was already answered in this session at this index generation".to_owned(),
      Self::NotIndexed => "the file is not indexed".to_owned(),
      Self::Stale(reason) => format!("the index is not fresh: {reason}"),
      Self::Resynced => "the index changed while the counterparts were computed".to_owned(),
      Self::RunInProgress => "an index run started while the counterparts were computed".to_owned(),
      Self::NoCounterpart => "the file has no counterpart".to_owned(),
      Self::BelowFloor => format!("every counterpart is below the hook's confidence floor of {HOOK_MIN_CONFIDENCE:.2}"),
      Self::TooLong => format!("the note would exceed {MAX_NOTE_CHARS} characters"),
      Self::Failed(step) => format!("failed while {step}"),
    }
  }
}

fn noted_key(session: &str, file: &str) -> String {
  format!("noted\n{session}\n{file}")
}

fn answered_key(session: &str, file: &str, generation: i64) -> String {
  format!("answered\n{session}\n{file}\n{generation}")
}

pub(super) fn counterpart<Seen, Claim>(home: &AtlasHome, project: Option<&str>, event: &Value, markers: SessionMarkers<Seen, Claim>) -> Option<Value>
where
  Seen: Fn(&str) -> bool + Send + 'static,
  Claim: Fn(&str) -> bool,
{
  let started = Instant::now();
  let elapsed_ms = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
  let Some(touched) = touched(event) else {
    tracing::debug!("no counterpart note: not a PostToolUse event with an absolute file_path, an absolute cwd and a session_id");
    return None;
  };
  if let Err(silence) = screen(&touched.file) {
    tracing::info!(elapsed_ms = elapsed_ms(), reason = %silence.reason(), "no counterpart note");
    return None;
  }
  let session = touched.session.clone();
  let job = Job {
    home: home.clone(),
    project: project.map(str::to_owned),
    cwd: touched.cwd,
    file: touched.file,
    session: touched.session,
    seen: markers.seen,
    deadline: started + TIME_LIMIT,
  };
  let (sender, receiver) = mpsc::channel();
  let spawned = thread::Builder::new().name("counterpart".to_owned()).spawn(move || {
    let _ = sender.send(job.run());
  });
  if let Err(error) = spawned {
    tracing::debug!(error = %error, "no counterpart note: cannot start the worker");
    return None;
  }
  let answered = match receiver.recv_timeout(TIME_LIMIT) {
    Ok(Ok(answered)) => answered,
    Ok(Err(silence)) => {
      tracing::info!(elapsed_ms = elapsed_ms(), reason = %silence.reason(), "no counterpart note");
      return None;
    }
    Err(error) => {
      tracing::info!(elapsed_ms = elapsed_ms(), limit_ms = TIME_LIMIT.as_millis() as u64, error = %error, "no counterpart note: the time limit passed");
      return None;
    }
  };
  (markers.claim)(&answered_key(&session, &answered.file, answered.generation));
  let note = match answered.note {
    Ok(note) => note,
    Err(silence) => {
      tracing::info!(elapsed_ms = elapsed_ms(), reason = %silence.reason(), "no counterpart note");
      return None;
    }
  };
  if !(markers.claim)(&noted_key(&session, &answered.file)) {
    tracing::debug!("no counterpart note: another hook noted this file first");
    return None;
  }
  tracing::info!(elapsed_ms = elapsed_ms(), note_chars = note.chars().count(), "noted a counterpart");
  Some(json!({ "hookSpecificOutput": { "hookEventName": POST_TOOL_USE, "additionalContext": note } }))
}

pub(super) fn touched(event: &Value) -> Option<Touched> {
  (event.get("hook_event_name")?.as_str()? == POST_TOOL_USE).then_some(())?;
  let file = PathBuf::from(event.get("tool_input")?.get("file_path")?.as_str()?);
  let cwd = PathBuf::from(event.get("cwd")?.as_str()?);
  let session = event.get("session_id")?.as_str().filter(|session| !session.is_empty())?.to_owned();
  (file.is_absolute() && cwd.is_absolute()).then_some(Touched { session, cwd, file })
}

fn screen(file: &Path) -> Result<(), Silence> {
  let name = file.file_name().and_then(|name| name.to_str()).unwrap_or_default();
  if is_test_or_generated_path(name) {
    return Err(Silence::TestOrGenerated);
  }
  match may_have_counterparts(name) {
    true => Ok(()),
    false => Err(Silence::NotSource),
  }
}

pub(super) fn locate(project: &Project, file: &Path) -> Option<(String, String)> {
  let file = fs::canonicalize(file).ok()?;
  project
    .repos
    .iter()
    .filter_map(|repo| {
      let root = fs::canonicalize(&repo.path).ok()?;
      let relative = file.strip_prefix(&root).ok()?;
      let segments: Option<Vec<&str>> = relative
        .components()
        .map(|component| match component {
          Component::Normal(segment) => segment.to_str(),
          _ => None,
        })
        .collect();
      Some((root.components().count(), repo.name.to_string(), segments?.join("/")))
    })
    .filter(|(_, _, path)| !path.is_empty())
    .max_by_key(|(depth, _, _)| *depth)
    .map(|(_, repo, path)| (repo, path))
}

impl<Seen: Fn(&str) -> bool> Job<Seen> {
  fn run(self) -> Result<Answered, Silence> {
    let registry = Registry::load(&self.home).map_err(|_| Silence::Failed("loading the registry"))?;
    let bound = match bind_existing_project(&self.home, &registry, &self.cwd, self.project.as_deref(), &self.store_options()) {
      Ok(ExistingBinding::Bound(bound)) => *bound,
      Ok(ExistingBinding::OverlayNotSeeded { label }) => return Err(Silence::OverlayNotSeeded(label)),
      Err(_) => return Err(Silence::NoProject),
    };
    if bound.binding.caveat().is_some() {
      return Err(Silence::OtherCheckout);
    }
    let (repo, path) = locate(&bound.project, &self.file).ok_or(Silence::OutsideProject)?;
    if is_test_or_generated_path(&path) {
      return Err(Silence::TestOrGenerated);
    }
    let file = format!("{repo}:{path}");
    if (self.seen)(&noted_key(&self.session, &file)) {
      return Err(Silence::AlreadyNoted);
    }
    let db_path = bound.engine_home.db_path(&bound.project.name);
    if !db_path.is_file() {
      return Err(Silence::NeverIndexed);
    }
    let store = Store::open_with(&db_path, self.store_options()).map_err(|_| Silence::Failed("opening the database"))?;
    let persisted = Engine::persisted_generation_in(&store).map_err(|_| Silence::Failed("reading the index generation"))?;
    drop(store);
    let RunId(persisted) = persisted.ok_or(Silence::NeverIndexed)?;
    if (self.seen)(&answered_key(&self.session, &file, persisted)) {
      return Err(Silence::AlreadyAnswered);
    }
    let options = EngineOptions {
      store: self.store_options(),
      barrier: BarrierMode::CheckOnly,
      ..EngineOptions::default()
    };
    let engine = Arc::new(Engine::open(&bound.project, &bound.engine_home, options).map_err(|_| Silence::Failed("opening the engine"))?);
    let mut request = CounterpartsRequest::new(file.clone());
    request.require_fresh = true;
    let response = QueryEngine::new(Arc::clone(&engine))
      .counterparts(&request)
      .map_err(|_| Silence::Failed("computing the counterparts"))?;
    let freshness = &response.freshness;
    let Some(run) = freshness.run else { return Err(Silence::NeverIndexed) };
    if let FreshnessState::Degraded { reason, .. } = &freshness.state {
      return Err(Silence::Stale(reason.clone()));
    }
    let generation = engine.persisted_generation().map_err(|_| Silence::Failed("reading the index generation"))?;
    if !freshness.notes.is_empty() || generation != Some(RunId(run)) {
      return Err(Silence::Resynced);
    }
    if engine.index_run_in_progress().map_err(|_| Silence::Failed("reading the run marker"))? {
      return Err(Silence::RunInProgress);
    }
    match &response.answer {
      Targeted::Found(answer) => Ok(Answered {
        file,
        generation: run,
        note: note(answer),
      }),
      Targeted::Ambiguous { .. } | Targeted::NotFound { .. } => Err(Silence::NotIndexed),
    }
  }

  fn store_options(&self) -> StoreOptions {
    StoreOptions {
      lock_timeout: self.remaining(),
      busy_timeout: self.remaining(),
      ..StoreOptions::default()
    }
  }

  fn remaining(&self) -> Duration {
    self.deadline.saturating_duration_since(Instant::now()).saturating_sub(WAIT_MARGIN)
  }
}

fn note(answer: &CounterpartsAnswer) -> Result<String, Silence> {
  if answer.counterparts.is_empty() {
    return Err(Silence::NoCounterpart);
  }
  let strong: Vec<&Counterpart> = answer
    .counterparts
    .iter()
    .filter(|counterpart| counterpart.certainty.confidence.is_some_and(|confidence| confidence >= HOOK_MIN_CONFIDENCE))
    .collect();
  let Some(best) = strong.first() else { return Err(Silence::BelowFloor) };
  let several = strong.len() > 1;
  let holder = match (several, best.repo == answer.file.repo) {
    (true, _) => "linked files hold",
    (false, false) => "a linked repo holds",
    (false, true) => "another package of its repo holds",
  };
  let examples: Vec<String> = best.shared.rarest.iter().take(NOTE_SHARED_SHOWN).map(|token| format!("'{token}'")).collect();
  let header = format!(
    "Code Atlas {}: {holder} the other side of {}:{} ({} shared values such as {}):",
    best.certainty.tag,
    answer.file.repo,
    answer.file.path,
    best.shared.count,
    examples.join(", ")
  );
  let footer = match several {
    true => "Read those files before changing what this one sends, reads or names.",
    false => "Read that file before changing what this one sends, reads or names.",
  };
  let heads: Vec<String> = strong.iter().map(|counterpart| counterpart_line(counterpart, several)).collect();
  let size = |lines: &[String]| lines.iter().map(|line| line.chars().count() + 1).sum::<usize>();
  let mut used = size(&heads) + header.chars().count() + 1 + footer.chars().count();
  if used > MAX_NOTE_CHARS {
    return Err(Silence::TooLong);
  }
  let mut fits = |group: &[String]| {
    let fitting = used + size(group) <= MAX_NOTE_CHARS;
    if fitting {
      used += size(group);
    }
    fitting
  };
  let mut lines = vec![header];
  for (counterpart, head) in strong.iter().zip(heads) {
    lines.push(head);
    if !counterpart.symbols.is_empty() {
      let symbols: Vec<String> = counterpart.symbols.iter().map(CounterpartSymbol::located).collect();
      let symbols = [format!("    most shared in {}", symbols.join(", "))];
      if fits(&symbols) {
        lines.extend(symbols);
      }
    }
    let mut values = counterpart.only_there.iter().map(|value| format!("      {}  {}", value.line, value.text));
    let Some(first) = values.next() else { continue };
    let opening = ["    only on that side, next to shared values:".to_owned(), first];
    if !fits(&opening) {
      continue;
    }
    lines.extend(opening);
    for value in values {
      let value = [value];
      if fits(&value) {
        lines.extend(value);
      }
    }
  }
  lines.push(footer.to_owned());
  Ok(lines.join("\n"))
}

fn counterpart_line(counterpart: &Counterpart, tagged: bool) -> String {
  let tag = match tagged {
    true => format!(" {}", counterpart.certainty.tag),
    false => String::new(),
  };
  format!("  {}:{}{tag} — {}", counterpart.repo, counterpart.path, counterpart.absolute_path)
}

#[cfg(test)]
mod tests {
  use atlas_query::{CertaintyTag, Location, OnlyThere, SharedValues};

  use super::*;

  fn event(name: &str, file: Value, session: Value) -> Value {
    json!({"session_id": session, "cwd": "/src/web", "hook_event_name": name, "tool_name": "Read", "tool_input": {"file_path": file}})
  }

  #[test]
  fn should_read_only_post_tool_use_events_with_an_absolute_file_a_cwd_and_a_session() {
    let read = touched(&event("PostToolUse", json!("/src/web/a.ts"), json!("s1"))).unwrap();
    assert_eq!(
      (read.session.as_str(), read.cwd.as_path(), read.file.as_path()),
      ("s1", Path::new("/src/web"), Path::new("/src/web/a.ts"))
    );
    assert!(touched(&event("PreToolUse", json!("/src/web/a.ts"), json!("s1"))).is_none());
    assert!(touched(&event("PostToolUse", json!("a.ts"), json!("s1"))).is_none());
    assert!(touched(&event("PostToolUse", json!(3), json!("s1"))).is_none());
    assert!(touched(&event("PostToolUse", json!("/src/web/a.ts"), json!(""))).is_none());
    assert!(touched(&json!({"hook_event_name": "PostToolUse", "session_id": "s", "cwd": "/src"})).is_none());
  }

  fn counterpart(repo: &str, values: usize, confidence: f32) -> Counterpart {
    Counterpart {
      repo: repo.to_owned(),
      path: "src/ops/op-specs.ts".to_owned(),
      absolute_path: format!("/w/{repo}/src/ops/op-specs.ts"),
      score: 30.0,
      certainty: CertaintyTag {
        class: "heuristic".to_owned(),
        confidence: Some(confidence),
        freshness: None,
        authoritative: false,
        tag: format!("[heuristic {confidence:.2}]"),
      },
      shared: SharedValues {
        count: 14,
        rarest: vec!["draft_slug".to_owned(), "revision_mark".to_owned(), "section_anchor".to_owned()],
      },
      symbols: vec![CounterpartSymbol {
        name: "SPECS".to_owned(),
        kind: "constant".to_owned(),
        start_line: 6,
        end_line: 20,
        shared_lines: 12,
      }],
      only_there: (0..values)
        .map(|index| OnlyThere {
          token: format!("page.op_{index}"),
          line: 7 + index as u32,
          text: format!("'page.op_{index}': {{ required: {{ field_{index}: 'string' }}, optional: {{}} }}, {}", "x".repeat(60)),
        })
        .collect(),
    }
  }

  fn answer(counterparts: Vec<Counterpart>) -> CounterpartsAnswer {
    CounterpartsAnswer {
      file: Location {
        repo: "web".to_owned(),
        path: "src/ops/op-switch.ts".to_owned(),
        line: None,
        column: None,
      },
      counterparts,
      caveats: Vec::new(),
    }
  }

  #[test]
  fn should_write_a_short_note_naming_no_tool_and_tag_each_file_only_when_there_are_two() {
    let single = note(&answer(vec![counterpart("server", 1, 0.72)])).unwrap();
    assert_eq!(
      single,
      format!(
        "Code Atlas [heuristic 0.72]: a linked repo holds the other side of web:src/ops/op-switch.ts (14 shared values such as 'draft_slug', 'revision_mark'):\n  server:src/ops/op-specs.ts — /w/server/src/ops/op-specs.ts\n    most shared in SPECS 6-20\n    only on that side, next to shared values:\n      7  'page.op_0': {{ required: {{ field_0: 'string' }}, optional: {{}} }}, {}\nRead that file before changing what this one sends, reads or names.",
        "x".repeat(60)
      )
    );
    let pair = note(&answer(vec![counterpart("server", 0, 0.72), counterpart("web", 0, 0.6)])).unwrap();
    assert_eq!(
      pair,
      "Code Atlas [heuristic 0.72]: linked files hold the other side of web:src/ops/op-switch.ts (14 shared values such as 'draft_slug', 'revision_mark'):\n  server:src/ops/op-specs.ts [heuristic 0.72] — /w/server/src/ops/op-specs.ts\n    most shared in SPECS 6-20\n  web:src/ops/op-specs.ts [heuristic 0.60] — /w/web/src/ops/op-specs.ts\n    most shared in SPECS 6-20\nRead those files before changing what this one sends, reads or names."
    );
    assert!(!single.contains("atlas_") && !pair.contains("atlas_"));
    assert!(matches!(note(&answer(Vec::new())), Err(Silence::NoCounterpart)));
  }

  #[test]
  fn should_note_only_the_counterparts_at_or_above_the_hook_confidence_floor() {
    let mixed = note(&answer(vec![counterpart("server", 0, 0.72), counterpart("web", 0, 0.59)])).unwrap();
    assert_eq!(
      mixed,
      "Code Atlas [heuristic 0.72]: a linked repo holds the other side of web:src/ops/op-switch.ts (14 shared values such as 'draft_slug', 'revision_mark'):\n  server:src/ops/op-specs.ts — /w/server/src/ops/op-specs.ts\n    most shared in SPECS 6-20\nRead that file before changing what this one sends, reads or names."
    );
    assert!(matches!(
      note(&answer(vec![counterpart("server", 2, 0.59), counterpart("web", 0, 0.43)])),
      Err(Silence::BelowFloor)
    ));
  }

  #[test]
  fn should_keep_the_note_within_its_character_cap() {
    let long = note(&answer(vec![counterpart("server", 8, 0.72), counterpart("web", 8, 0.64)])).unwrap();
    assert!(long.chars().count() <= MAX_NOTE_CHARS, "{}", long.chars().count());
    assert!(long.ends_with("Read those files before changing what this one sends, reads or names."), "{long}");
    assert!(long.contains("      7  'page.op_0'"), "{long}");
  }

  #[test]
  fn should_drop_the_group_that_would_bring_the_note_to_1200_characters() {
    let padded = |padding: usize| {
      let mut server = counterpart("server", 1, 0.72);
      server.only_there[0].text = format!("'page.op_0': {{ required: {{ field_0: 'string' }}, optional: {{}} }}, {}", "x".repeat(padding));
      note(&answer(vec![server])).unwrap()
    };
    assert_eq!(padded(771).chars().count(), 1_199);
    assert_eq!(
      padded(772),
      "Code Atlas [heuristic 0.72]: a linked repo holds the other side of web:src/ops/op-switch.ts (14 shared values such as 'draft_slug', 'revision_mark'):\n  server:src/ops/op-specs.ts — /w/server/src/ops/op-specs.ts\n    most shared in SPECS 6-20\nRead that file before changing what this one sends, reads or names."
    );
  }

  #[test]
  fn should_screen_out_files_that_cannot_have_counterparts_before_binding() {
    assert!(screen(Path::new("/w/web/src/ops/op-switch.ts")).is_ok());
    assert!(screen(Path::new("/w/server/src/lib.rs")).is_ok());
    assert!(matches!(screen(Path::new("/w/web/README.md")), Err(Silence::NotSource)));
    assert!(matches!(screen(Path::new("/w/web/package.json")), Err(Silence::NotSource)));
    assert!(matches!(screen(Path::new("/w/web/src/ops/op-switch.test.ts")), Err(Silence::TestOrGenerated)));
    assert!(matches!(screen(Path::new("/w/web/src/api.gen.ts")), Err(Silence::TestOrGenerated)));
  }

  #[test]
  fn should_key_markers_by_the_located_file_and_the_answer_marker_also_by_generation() {
    assert_eq!(noted_key("s1", "web:src/a.ts"), "noted\ns1\nweb:src/a.ts");
    assert_eq!(answered_key("s1", "web:src/a.ts", 7), "answered\ns1\nweb:src/a.ts\n7");
  }
}
