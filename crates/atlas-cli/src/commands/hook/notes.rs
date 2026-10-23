use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use atlas_core::{Language, RunId};
use atlas_engine::{BarrierMode, Engine, EngineOptions, ExistingBinding, bind_existing_project};
use atlas_project::{AtlasHome, Registry, is_doc_name, is_manifest_name, is_note_path};
use atlas_query::{AnchoredNote, FileNotesAnswer, FileNotesRequest, FreshnessState, NoteState, QueryEngine, Targeted};
use atlas_store::{Store, StoreOptions};
use serde_json::{Value, json};

use super::counterpart::{locate, touched};
use super::{SessionMarkers, TIME_LIMIT, WAIT_MARGIN};

const POST_TOOL_USE: &str = "PostToolUse";
const MAX_NOTE_CHARS: usize = 1_200;
const STALE_WORDING: &str = "stale: the code changed after this note was written";
const FOOTER: &str = "Read the note before changing the code it describes; a stale note may describe code that is gone.";

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
  NotCode,
  NoProject,
  OtherCheckout,
  OverlayNotSeeded(String),
  OutsideProject,
  ANote,
  AlreadyNoted,
  NeverIndexed,
  AlreadyAnswered,
  NotIndexed,
  Stale(String),
  Resynced,
  RunInProgress,
  NoNote,
  TooLong,
  Failed(&'static str),
}

impl Silence {
  fn reason(&self) -> String {
    match self {
      Self::NotCode => "the file is a doc or not a source or manifest file".to_owned(),
      Self::NoProject => "no project resolves for the working directory".to_owned(),
      Self::OtherCheckout => "the working directory is answered from another checkout".to_owned(),
      Self::OverlayNotSeeded(label) => format!("the worktree overlay {label} is not seeded yet"),
      Self::OutsideProject => "the file is in no repo of the project".to_owned(),
      Self::ANote => "the file is a note".to_owned(),
      Self::AlreadyNoted => "this file was already noted in this session".to_owned(),
      Self::NeverIndexed => "the project has never been indexed".to_owned(),
      Self::AlreadyAnswered => "this file was already answered in this session at this index generation".to_owned(),
      Self::NotIndexed => "the file is not indexed".to_owned(),
      Self::Stale(reason) => format!("the index is not fresh: {reason}"),
      Self::Resynced => "the index changed while the notes were read".to_owned(),
      Self::RunInProgress => "an index run started while the notes were read".to_owned(),
      Self::NoNote => "no note is anchored to the file".to_owned(),
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

pub(super) fn notes<Seen, Claim>(home: &AtlasHome, project: Option<&str>, event: &Value, markers: SessionMarkers<Seen, Claim>) -> Option<Value>
where
  Seen: Fn(&str) -> bool + Send + 'static,
  Claim: Fn(&str) -> bool,
{
  let started = Instant::now();
  let elapsed_ms = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
  let Some(touched) = touched(event) else {
    tracing::debug!("no notes: not a PostToolUse event with an absolute file_path, an absolute cwd and a session_id");
    return None;
  };
  if let Err(silence) = screen(&touched.file) {
    tracing::info!(elapsed_ms = elapsed_ms(), reason = %silence.reason(), "no notes");
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
  let spawned = thread::Builder::new().name("notes".to_owned()).spawn(move || {
    let _ = sender.send(job.run());
  });
  if let Err(error) = spawned {
    tracing::debug!(error = %error, "no notes: cannot start the worker");
    return None;
  }
  let answered = match receiver.recv_timeout(TIME_LIMIT) {
    Ok(Ok(answered)) => answered,
    Ok(Err(silence)) => {
      tracing::info!(elapsed_ms = elapsed_ms(), reason = %silence.reason(), "no notes");
      return None;
    }
    Err(error) => {
      tracing::info!(elapsed_ms = elapsed_ms(), limit_ms = TIME_LIMIT.as_millis() as u64, error = %error, "no notes: the time limit passed");
      return None;
    }
  };
  (markers.claim)(&answered_key(&session, &answered.file, answered.generation));
  let note = match answered.note {
    Ok(note) => note,
    Err(silence) => {
      tracing::info!(elapsed_ms = elapsed_ms(), reason = %silence.reason(), "no notes");
      return None;
    }
  };
  if !(markers.claim)(&noted_key(&session, &answered.file)) {
    tracing::debug!("no notes: another hook noted this file first");
    return None;
  }
  tracing::info!(elapsed_ms = elapsed_ms(), note_chars = note.chars().count(), "noted the notes anchored to a file");
  Some(json!({ "hookSpecificOutput": { "hookEventName": POST_TOOL_USE, "additionalContext": note } }))
}

fn screen(file: &Path) -> Result<(), Silence> {
  let name = file.file_name().and_then(|name| name.to_str()).unwrap_or_default();
  let code = Language::from_path(name).is_some() || is_manifest_name(name);
  match code && !is_doc_name(name) {
    true => Ok(()),
    false => Err(Silence::NotCode),
  }
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
    if is_note_path(&path) {
      return Err(Silence::ANote);
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
    let request = FileNotesRequest {
      file: file.clone(),
      require_fresh: true,
    };
    let response = QueryEngine::new(Arc::clone(&engine))
      .notes_for_file(&request)
      .map_err(|_| Silence::Failed("reading the notes"))?;
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

fn status_text(note: &AnchoredNote) -> String {
  let status = match note.note.status {
    NoteState::Stale => STALE_WORDING,
    other => other.as_str(),
  };
  let mentioned: Vec<&str> = note
    .note
    .anchors
    .iter()
    .filter_map(|anchor| anchor.certainty.as_ref())
    .map(|certainty| certainty.tag.as_str())
    .collect();
  match (mentioned.first(), mentioned.len() == note.note.anchors.len()) {
    (Some(tag), true) => format!("{status}; anchored by a mention {tag}"),
    _ => status.to_owned(),
  }
}

fn note(answer: &FileNotesAnswer) -> Result<String, Silence> {
  if answer.notes.is_empty() {
    return Err(Silence::NoNote);
  }
  let header = format!("Code Atlas: notes earlier sessions left about {}:{}:", answer.file.repo, answer.file.path);
  let size = |line: &str| line.chars().count() + 1;
  let mut used = size(&header) + FOOTER.chars().count();
  let mut lines = vec![header];
  for anchored in &answer.notes {
    let head = format!("  {}:{} — {}", anchored.note.repo, anchored.note.path, anchored.note.absolute_path);
    let status = status_text(anchored);
    let sections: Vec<String> = anchored
      .sections
      .iter()
      .map(|section| format!("    {} [{status}]: {}", section.heading, section.text))
      .collect();
    let Some(first) = sections.first() else { continue };
    if used + size(&head) + size(first) > MAX_NOTE_CHARS {
      continue;
    }
    used += size(&head) + size(first);
    lines.push(head);
    lines.push(first.clone());
    for section in sections.iter().skip(1) {
      if used + size(section) <= MAX_NOTE_CHARS {
        used += size(section);
        lines.push(section.clone());
      }
    }
  }
  if lines.len() == 1 {
    return Err(Silence::TooLong);
  }
  lines.push(FOOTER.to_owned());
  Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
  use atlas_query::{AnchorStatus, AnchorView, CertaintyTag, Location, NoteExcerpt, NoteView};

  use super::*;

  fn anchored(status: NoteState, mention: bool, text: &str) -> AnchoredNote {
    AnchoredNote {
      note: NoteView {
        repo: "web".to_owned(),
        path: ".code-atlas/notes/retries.md".to_owned(),
        absolute_path: "/w/web/.code-atlas/notes/retries.md".to_owned(),
        title: "Retry budget".to_owned(),
        status,
        anchors: vec![AnchorView {
          target: "web:src/lib/backoff.ts#backoffDelay".to_owned(),
          status: AnchorStatus::Current,
          declared: !mention,
          entity: None,
          certainty: mention.then(|| CertaintyTag {
            class: "heuristic".to_owned(),
            confidence: Some(0.7),
            freshness: None,
            authoritative: false,
            tag: "[heuristic 0.70]".to_owned(),
          }),
        }],
      },
      sections: vec![NoteExcerpt {
        heading: "Retry budget".to_owned(),
        start_line: 5,
        text: text.to_owned(),
      }],
    }
  }

  fn answer(notes: Vec<AnchoredNote>) -> FileNotesAnswer {
    FileNotesAnswer {
      file: Location {
        repo: "web".to_owned(),
        path: "src/lib/backoff.ts".to_owned(),
        line: None,
        column: None,
      },
      notes,
      caveats: Vec::new(),
    }
  }

  #[test]
  fn should_write_the_note_heading_status_and_excerpt_naming_no_tool() {
    let current = note(&answer(vec![anchored(NoteState::Current, false, "`backoffDelay` caps the wait.")])).unwrap();
    assert_eq!(
      current,
      "Code Atlas: notes earlier sessions left about web:src/lib/backoff.ts:\n  web:.code-atlas/notes/retries.md — /w/web/.code-atlas/notes/retries.md\n    Retry budget [current]: `backoffDelay` caps the wait.\nRead the note before changing the code it describes; a stale note may describe code that is gone."
    );
    let stale = note(&answer(vec![anchored(NoteState::Stale, false, "x")])).unwrap();
    assert!(stale.contains("Retry budget [stale: the code changed after this note was written]: x"), "{stale}");
    let mentioned = note(&answer(vec![anchored(NoteState::Unstamped, true, "x")])).unwrap();
    assert!(mentioned.contains("[unstamped; anchored by a mention [heuristic 0.70]]: x"), "{mentioned}");
    assert!(!current.contains("atlas_") && !stale.contains("atlas_"));
    assert!(matches!(note(&answer(Vec::new())), Err(Silence::NoNote)));
  }

  #[test]
  fn should_keep_the_note_within_its_character_cap() {
    let long = "y".repeat(400);
    let notes: Vec<AnchoredNote> = (0..5).map(|_| anchored(NoteState::Current, false, &long)).collect();
    let text = note(&answer(notes)).unwrap();
    assert!(text.chars().count() <= MAX_NOTE_CHARS, "{}", text.chars().count());
    assert_eq!(text.matches("Retry budget [current]").count(), 2);
  }

  #[test]
  fn should_screen_out_docs_and_files_that_are_neither_code_nor_manifests_before_binding() {
    assert!(screen(Path::new("/w/web/src/lib/backoff.ts")).is_ok());
    assert!(screen(Path::new("/w/web/src/lib/backoff.test.ts")).is_ok());
    assert!(screen(Path::new("/w/web/package.json")).is_ok());
    assert!(matches!(screen(Path::new("/w/web/.code-atlas/notes/retries.md")), Err(Silence::NotCode)));
    assert!(matches!(screen(Path::new("/w/web/notes.txt")), Err(Silence::NotCode)));
  }

  #[test]
  fn should_key_markers_by_the_located_file_and_the_answer_marker_also_by_generation() {
    assert_eq!(noted_key("s1", "web:src/a.ts"), "noted\ns1\nweb:src/a.ts");
    assert_eq!(answered_key("s1", "web:src/a.ts", 7), "answered\ns1\nweb:src/a.ts\n7");
  }
}
