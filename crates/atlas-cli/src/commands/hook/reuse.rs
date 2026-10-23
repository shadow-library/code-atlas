use std::fs;
use std::path::Path;
use std::time::Instant;

use atlas_project::AtlasHome;
use atlas_query::{IndexState, QueryEngine, ReuseCandidate, ReuseRequest, ReuseUnit};
use serde_json::{Value, json};

use super::SessionMarkers;
use super::edits::{Accepted, EditEvent, Silence, Target, clip, edit_event, run_limited, screen, settle};

const PRE_TOOL_USE: &str = "PreToolUse";
const MAX_NOTE_CHARS: usize = 600;
const MAX_SIGNATURE_CHARS: usize = 100;

#[derive(Debug, Clone, PartialEq)]
struct Shown {
  unit: String,
  candidate: String,
  line: String,
  single: String,
}

fn shown_key(session: &str, unit: &str, candidate: &str) -> String {
  format!("reuse\n{session}\n{unit}\n{candidate}")
}

pub(super) fn reuse<Seen, Claim>(home: &AtlasHome, project: Option<&str>, event: &Value, markers: SessionMarkers<Seen, Claim>) -> Option<Value>
where
  Seen: Fn(&str) -> bool + Send + 'static,
  Claim: Fn(&str) -> bool,
{
  let started = Instant::now();
  let elapsed_ms = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
  let Some(edit) = edit_event(event, PRE_TOOL_USE) else {
    tracing::debug!("no reuse note: not a PreToolUse event of Write, Edit or MultiEdit with an absolute file_path, an absolute cwd and a session_id");
    return None;
  };
  if let Err(silence) = screen(&edit.file) {
    tracing::info!(elapsed_ms = elapsed_ms(), reason = %silence.reason(), "no reuse note");
    return None;
  }
  let session = edit.session.clone();
  let home = home.clone();
  let project = project.map(str::to_owned);
  let seen = markers.seen;
  let worked = run_limited("reuse", started, move |deadline| {
    let target = Target {
      home: &home,
      project: project.as_deref(),
      cwd: &edit.cwd,
      file: &edit.file,
      deadline,
    };
    check(&target, &edit, &seen)
  });
  let shown = match worked.and_then(|lines| if lines.is_empty() { Err(Silence::NothingFound) } else { Ok(lines) }) {
    Ok(shown) => shown,
    Err(silence) => {
      tracing::info!(elapsed_ms = elapsed_ms(), reason = %silence.reason(), "no reuse note");
      return None;
    }
  };
  let shown: Vec<Shown> = shown
    .into_iter()
    .filter(|shown| (markers.claim)(&shown_key(&session, &shown.unit, &shown.candidate)))
    .collect();
  let Some(note) = note(&shown) else {
    tracing::debug!("no reuse note: another hook noted these candidates first");
    return None;
  };
  tracing::info!(
    elapsed_ms = elapsed_ms(),
    note_chars = note.chars().count(),
    candidates = shown.len(),
    "noted reuse candidates"
  );
  Some(json!({ "hookSpecificOutput": { "hookEventName": PRE_TOOL_USE, "additionalContext": note } }))
}

fn check(target: &Target<'_>, edit: &EditEvent, seen: &impl Fn(&str) -> bool) -> Result<Vec<Shown>, Silence> {
  let opened = target.open(|_| Ok(()))?;
  let text = new_text(edit, &edit.file, opened.bound.project.settings.max_file_bytes)?;
  let request = ReuseRequest {
    repo_path: opened.file.clone(),
    text: Some(text),
    lines: None,
  };
  let response = QueryEngine::new(opened.engine.clone())
    .reuse_check(&request)
    .map_err(|_| Silence::Failed("checking for reuse"))?;
  settle(&opened.engine, &response.freshness, Accepted::ChangedFiles)?;
  let answer = &response.answer;
  match answer.index_state {
    IndexState::Current => {}
    IndexState::NotIndexed if !edit.file.exists() => {}
    IndexState::NotIndexed | IndexState::Changed => return Err(Silence::TargetChanged),
  }
  let mut lines = Vec::new();
  for unit in &answer.units {
    for candidate in &unit.candidates {
      if seen(&shown_key(&edit.session, &unit.name, &candidate.entity.key)) {
        continue;
      }
      lines.push(Shown {
        unit: unit.name.clone(),
        candidate: candidate.entity.key.clone(),
        line: line(&answer.file.repo, unit, candidate),
        single: single(&answer.file.repo, unit, candidate),
      });
    }
  }
  Ok(lines)
}

fn new_text(edit: &EditEvent, file: &Path, max_bytes: u64) -> Result<String, Silence> {
  let text = |name: &str, input: &Value| input.get(name).and_then(Value::as_str).map(str::to_owned);
  let capped = |text: String| match text.len() as u64 > max_bytes {
    true => Err(Silence::TooLarge(max_bytes)),
    false => Ok(text),
  };
  if edit.tool == "Write" {
    return capped(text("content", &edit.input).ok_or(Silence::EditDoesNotApply)?);
  }
  let edits: Vec<Value> = match edit.tool.as_str() {
    "MultiEdit" => edit.input.get("edits").and_then(Value::as_array).cloned().ok_or(Silence::EditDoesNotApply)?,
    _ => vec![edit.input.clone()],
  };
  let mut current = match fs::metadata(file) {
    Ok(metadata) if metadata.len() > max_bytes => return Err(Silence::TooLarge(max_bytes)),
    Ok(_) => fs::read(file).ok().map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
    Err(_) => None,
  };
  for change in &edits {
    let (Some(old), Some(new)) = (text("old_string", change), text("new_string", change)) else {
      return Err(Silence::EditDoesNotApply);
    };
    let all = change.get("replace_all").and_then(Value::as_bool).unwrap_or(false);
    current = Some(match current {
      None if old.is_empty() => new,
      Some(before) if !old.is_empty() && (all || before.matches(old.as_str()).count() == 1) && before.contains(&old) => match all {
        true => before.replace(&old, &new),
        false => before.replacen(&old, &new, 1),
      },
      _ => return Err(Silence::EditDoesNotApply),
    });
  }
  capped(current.ok_or(Silence::EditDoesNotApply)?)
}

fn place(repo: &str, candidate: &ReuseCandidate) -> String {
  let Some(location) = &candidate.entity.location else {
    return String::new();
  };
  let file = match location.repo == repo {
    true => location.path.clone(),
    false => format!("{}:{}", location.repo, location.path),
  };
  let signature = candidate
    .signature
    .as_deref()
    .map(|signature| format!(" `{}`", clip(&signature.split_whitespace().collect::<Vec<_>>().join(" "), MAX_SIGNATURE_CHARS)))
    .unwrap_or_default();
  format!(" — {file}:{}-{}{signature}", candidate.start_line, candidate.end_line)
}

fn single(repo: &str, unit: &ReuseUnit, candidate: &ReuseCandidate) -> String {
  format!(
    "Code Atlas {}: `{}` ({}) looks like existing `{}`{}. Reuse or extend it if it fits.",
    candidate.certainty.tag,
    unit.name,
    unit.status.label(),
    candidate.entity.short,
    place(repo, candidate)
  )
}

fn line(repo: &str, unit: &ReuseUnit, candidate: &ReuseCandidate) -> String {
  format!(
    "  {} `{}` ({}) looks like `{}`{}",
    candidate.certainty.tag,
    unit.name,
    unit.status.label(),
    candidate.entity.short,
    place(repo, candidate)
  )
}

fn note(shown: &[Shown]) -> Option<String> {
  match shown {
    [] => None,
    [only] => Some(clip(&only.single, MAX_NOTE_CHARS)),
    several => {
      let header = "Code Atlas: the code about to be written looks like existing code:";
      let footer = "Reuse or extend it where it fits.";
      let mut used = header.chars().count() + 1 + footer.chars().count();
      let mut lines = vec![header.to_owned()];
      for shown in several {
        let size = shown.line.chars().count() + 1;
        if used + size <= MAX_NOTE_CHARS {
          used += size;
          lines.push(shown.line.clone());
        }
      }
      if lines.len() == 1 {
        return Some(clip(&several[0].single, MAX_NOTE_CHARS));
      }
      lines.push(footer.to_owned());
      Some(lines.join("\n"))
    }
  }
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use atlas_query::{CertaintyTag, EntityRef, Location, ReuseReason, UnitStatus};

  use super::*;

  fn unit(name: &str) -> ReuseUnit {
    ReuseUnit {
      name: name.to_owned(),
      kind: "function".to_owned(),
      start_line: 3,
      end_line: 12,
      tokens: 40,
      status: UnitStatus::New,
      candidates: Vec::new(),
    }
  }

  fn candidate(repo: &str, short: &str, confidence: f32) -> ReuseCandidate {
    ReuseCandidate {
      entity: EntityRef {
        key: format!("ts:{repo}:shelf:src/lib/backoff::{short}#function"),
        id: "1".to_owned(),
        short: short.to_owned(),
        kind: "function".to_owned(),
        location: Some(Location {
          repo: repo.to_owned(),
          path: "src/lib/backoff.ts".to_owned(),
          line: Some(41),
          column: None,
        }),
        certainty: None,
      },
      start_line: 41,
      end_line: 58,
      signature: Some("waitBeforeRetry(error: unknown): number | undefined".to_owned()),
      certainty: CertaintyTag {
        class: "heuristic".to_owned(),
        confidence: Some(confidence),
        freshness: None,
        authoritative: false,
        tag: format!("[heuristic {confidence:.2}]"),
      },
      reason: ReuseReason::NearDuplicate { jaccard: f64::from(confidence) },
    }
  }

  fn shown(repo: &str, unit_name: &str, short: &str, confidence: f32) -> Shown {
    let (unit, candidate) = (unit(unit_name), candidate(repo, short, confidence));
    Shown {
      unit: unit_name.to_owned(),
      candidate: short.to_owned(),
      line: line("shelf-web", &unit, &candidate),
      single: single("shelf-web", &unit, &candidate),
    }
  }

  #[test]
  fn should_write_one_sentence_for_one_candidate_and_a_list_for_several_naming_no_tool() {
    let one = note(&[shown("shelf-web", "pauseAfterFailure", "waitBeforeRetry", 0.74)]).unwrap();
    assert_eq!(
      one,
      "Code Atlas [heuristic 0.74]: `pauseAfterFailure` (new) looks like existing `waitBeforeRetry` — src/lib/backoff.ts:41-58 `waitBeforeRetry(error: unknown): number | undefined`. Reuse or extend it if it fits."
    );
    let two = note(&[
      shown("shelf-web", "pauseAfterFailure", "waitBeforeRetry", 0.74),
      shown("shelf-kit", "retryLater", "retryDelay", 0.5),
    ])
    .unwrap();
    assert_eq!(
      two,
      "Code Atlas: the code about to be written looks like existing code:\n  [heuristic 0.74] `pauseAfterFailure` (new) looks like `waitBeforeRetry` — src/lib/backoff.ts:41-58 `waitBeforeRetry(error: unknown): number | undefined`\n  [heuristic 0.50] `retryLater` (new) looks like `retryDelay` — shelf-kit:src/lib/backoff.ts:41-58 `waitBeforeRetry(error: unknown): number | undefined`\nReuse or extend it where it fits."
    );
    assert!(!one.contains("atlas_") && !two.contains("atlas_"));
    assert!(note(&[]).is_none());
  }

  #[test]
  fn should_keep_the_note_within_its_character_cap() {
    let many: Vec<Shown> = (0..6).map(|index| shown("shelf-web", &format!("unit{index}"), "waitBeforeRetry", 0.7)).collect();
    let long = note(&many).unwrap();
    assert!(long.chars().count() <= MAX_NOTE_CHARS, "{}", long.chars().count());
    assert!(long.ends_with("Reuse or extend it where it fits."), "{long}");
  }

  fn edit(tool: &str, input: Value) -> EditEvent {
    EditEvent {
      session: "s".to_owned(),
      cwd: PathBuf::from("/"),
      file: PathBuf::from("/x.ts"),
      tool: tool.to_owned(),
      input,
    }
  }

  #[test]
  fn should_rebuild_the_text_an_edit_would_write() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.ts");
    fs::write(&file, "one\ntwo\ntwo\n").unwrap();
    let text = |tool: &str, input: Value| new_text(&edit(tool, input), &file, 1_000);
    assert_eq!(text("Write", json!({"content": "fresh"})).unwrap(), "fresh");
    assert_eq!(text("Edit", json!({"old_string": "one", "new_string": "1"})).unwrap(), "1\ntwo\ntwo\n");
    assert!(matches!(text("Edit", json!({"old_string": "two", "new_string": "2"})), Err(Silence::EditDoesNotApply)));
    assert_eq!(text("Edit", json!({"old_string": "two", "new_string": "2", "replace_all": true})).unwrap(), "one\n2\n2\n");
    assert_eq!(
      text(
        "MultiEdit",
        json!({"edits": [{"old_string": "one", "new_string": "1"}, {"old_string": "1\ntwo", "new_string": "x"}]})
      )
      .unwrap(),
      "x\ntwo\n"
    );
    assert!(matches!(text("Edit", json!({"old_string": "three", "new_string": "3"})), Err(Silence::EditDoesNotApply)));
    assert!(matches!(text("Edit", json!({"new_string": "3"})), Err(Silence::EditDoesNotApply)));
    let missing = dir.path().join("new.ts");
    assert_eq!(new_text(&edit("Edit", json!({"old_string": "", "new_string": "made"})), &missing, 1_000).unwrap(), "made");
    let small = |tool: &str, input: Value| new_text(&edit(tool, input), &file, 5);
    assert!(matches!(small("Edit", json!({"old_string": "one", "new_string": "1"})), Err(Silence::TooLarge(5))));
    assert!(matches!(small("Write", json!({"content": "too long"})), Err(Silence::TooLarge(5))));
  }

  #[test]
  fn should_key_markers_by_session_unit_and_candidate() {
    assert_eq!(shown_key("s1", "retryLater", "ts:k"), "reuse\ns1\nretryLater\nts:k");
  }
}
