use std::sync::Arc;
use std::time::Instant;

use atlas_project::AtlasHome;
use atlas_query::{AlsoChangeFile, AlsoChangeRequest, QueryEngine};
use serde_json::{Value, json};

use super::SessionMarkers;
use super::edits::{Accepted, Silence, Target, edit_event, run_limited, screen, settle};

const POST_TOOL_USE: &str = "PostToolUse";
const MAX_NOTE_CHARS: usize = 600;
const FILES_SHOWN: usize = 4;
const FILES_FETCHED: usize = 12;

fn noted_key(session: &str, file: &str) -> String {
  format!("also-change\n{session}\n{file}")
}

fn edited_key(session: &str, file: &str) -> String {
  format!("edited\n{session}\n{file}")
}

struct Answered {
  file: String,
  files: Vec<AlsoChangeFile>,
}

pub(super) fn also_change<Seen, Claim>(home: &AtlasHome, project: Option<&str>, event: &Value, markers: SessionMarkers<Seen, Claim>) -> Option<Value>
where
  Seen: Fn(&str) -> bool + Send + 'static,
  Claim: Fn(&str) -> bool + Send + Sync + 'static,
{
  let started = Instant::now();
  let elapsed_ms = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
  let Some(edit) = edit_event(event, POST_TOOL_USE) else {
    tracing::debug!("no also-change note: not a PostToolUse event of Write, Edit or MultiEdit with an absolute file_path, an absolute cwd and a session_id");
    return None;
  };
  if let Err(silence) = screen(&edit.file) {
    tracing::info!(elapsed_ms = elapsed_ms(), reason = %silence.reason(), "no also-change note");
    return None;
  }
  let session = edit.session.clone();
  let home = home.clone();
  let project = project.map(str::to_owned);
  let seen = markers.seen;
  let claim = Arc::new(markers.claim);
  let worker_claim = Arc::clone(&claim);
  let worked = run_limited("also-change", started, move |deadline| {
    let target = Target {
      home: &home,
      project: project.as_deref(),
      cwd: &edit.cwd,
      file: &edit.file,
      deadline,
    };
    let opened = target.open(|file| {
      worker_claim(&edited_key(&edit.session, file));
      match seen(&noted_key(&edit.session, file)) {
        true => Err(Silence::AlreadyNoted),
        false => Ok(()),
      }
    })?;
    let mut request = AlsoChangeRequest::new([opened.file.clone()]);
    request.limit = FILES_FETCHED;
    let response = QueryEngine::new(opened.engine.clone())
      .also_change(&request)
      .map_err(|_| Silence::Failed("ranking the files to change"))?;
    settle(&opened.engine, &response.freshness, Accepted::ChangedFiles)?;
    let files = response
      .answer
      .files
      .into_iter()
      .filter(|file| !seen(&edited_key(&edit.session, &format!("{}:{}", file.repo, file.path))))
      .take(FILES_SHOWN)
      .collect();
    Ok(Answered { file: opened.file, files })
  });
  let answered = match worked {
    Ok(answered) => answered,
    Err(silence) => {
      tracing::info!(elapsed_ms = elapsed_ms(), reason = %silence.reason(), "no also-change note");
      return None;
    }
  };
  let note = match note(&answered.file, &answered.files) {
    Ok(note) => note,
    Err(silence) => {
      claim(&noted_key(&session, &answered.file));
      tracing::info!(elapsed_ms = elapsed_ms(), reason = %silence.reason(), "no also-change note");
      return None;
    }
  };
  if !claim(&noted_key(&session, &answered.file)) {
    tracing::debug!("no also-change note: another hook noted this file first");
    return None;
  }
  tracing::info!(
    elapsed_ms = elapsed_ms(),
    note_chars = note.chars().count(),
    files = answered.files.len(),
    "noted files that may also change"
  );
  Some(json!({ "hookSpecificOutput": { "hookEventName": POST_TOOL_USE, "additionalContext": note } }))
}

fn line(file: &AlsoChangeFile) -> String {
  let tag = match file.certainty.authoritative {
    true => String::new(),
    false => format!(" {}", file.certainty.tag),
  };
  format!("  {}:{} — {}{tag}", file.repo, file.path, file.reason)
}

fn note(changed: &str, files: &[AlsoChangeFile]) -> Result<String, Silence> {
  if files.is_empty() {
    return Err(Silence::NothingFound);
  }
  let header = format!("Code Atlas: files close to {changed} in the code graph, which often change with it (leads to check, not certain):");
  let mut used = header.chars().count();
  let mut lines = vec![header];
  for file in files {
    let line = line(file);
    let size = line.chars().count() + 1;
    if used + size <= MAX_NOTE_CHARS {
      used += size;
      lines.push(line);
    }
  }
  match lines.len() {
    1 => Err(Silence::TooLong(MAX_NOTE_CHARS)),
    _ => Ok(lines.join("\n")),
  }
}

#[cfg(test)]
mod tests {
  use atlas_query::CertaintyTag;

  use super::*;

  fn file(path: &str, reason: &str, tag: Option<&str>) -> AlsoChangeFile {
    AlsoChangeFile {
      repo: "shelf-web".to_owned(),
      path: path.to_owned(),
      score: 1.0,
      reason: reason.to_owned(),
      certainty: CertaintyTag {
        class: if tag.is_some() { "heuristic" } else { "resolved" }.to_owned(),
        confidence: tag.map(|_| 0.6),
        freshness: None,
        authoritative: tag.is_none(),
        tag: tag.unwrap_or("[resolved]").to_owned(),
      },
    }
  }

  #[test]
  fn should_list_the_files_with_their_reason_and_tag_only_uncertain_ones() {
    let written = note(
      "shelf-web:src/lib/shelf.ts",
      &[
        file("src/ui/shelf-view.ts", "calls changed function loadShelf", None),
        file("src/api/routes.ts", "same contract values", Some("[heuristic 0.64]")),
      ],
    )
    .unwrap();
    assert_eq!(
      written,
      "Code Atlas: files close to shelf-web:src/lib/shelf.ts in the code graph, which often change with it (leads to check, not certain):\n  shelf-web:src/ui/shelf-view.ts — calls changed function loadShelf\n  shelf-web:src/api/routes.ts — same contract values [heuristic 0.64]"
    );
    assert!(!written.contains("atlas_"));
    assert!(matches!(note("shelf-web:a.ts", &[]), Err(Silence::NothingFound)));
  }

  #[test]
  fn should_keep_the_note_within_its_character_cap() {
    let files: Vec<AlsoChangeFile> = (0..4)
      .map(|index| file(&format!("src/{}/{index}.ts", "deep/".repeat(30)), "imported by changed file src/a.ts", None))
      .collect();
    let written = note("shelf-web:src/a.ts", &files).unwrap();
    assert!(written.chars().count() <= MAX_NOTE_CHARS, "{}", written.chars().count());
    assert_eq!(written.lines().count(), 3);
  }

  #[test]
  fn should_key_markers_by_session_and_file() {
    assert_eq!(noted_key("s1", "web:src/a.ts"), "also-change\ns1\nweb:src/a.ts");
    assert_eq!(edited_key("s1", "web:src/a.ts"), "edited\ns1\nweb:src/a.ts");
  }
}
