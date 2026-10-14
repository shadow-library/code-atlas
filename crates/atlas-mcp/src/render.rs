use atlas_query::{EntityRef, QueryEngine, QueryError, RenderText, Response, SearchRequest, Snippet, Targeted, TextOut, render_json, render_text};
use serde::Serialize;
use serde_json::{Value, json};

use crate::arguments::Format;
use crate::error::ToolError;

const SUGGESTIONS: usize = 5;
const CAVEAT_KEY: &str = "caveat";
const CAVEAT_FIELD_OVERHEAD: usize = 16;

pub(crate) fn plain<T: RenderText + Serialize>(response: &Response<T>, format: Format) -> Result<String, ToolError> {
  match format {
    Format::Text => Ok(render_text(response)),
    Format::Json => Ok(render_json(response)?),
  }
}

pub(crate) fn targeted<T: RenderText + Serialize>(query: &QueryEngine, response: &Response<Targeted<T>>, format: Format) -> Result<String, ToolError> {
  targeted_noted(query, response, format, None)
}

pub(crate) fn targeted_noted<T: RenderText + Serialize>(query: &QueryEngine, response: &Response<Targeted<T>>, format: Format, note: Option<String>) -> Result<String, ToolError> {
  let Targeted::NotFound { spec, .. } = &response.answer else {
    return noted(plain(response, format)?, format, note);
  };
  let suggestions = suggest(query, spec);
  match format {
    Format::Text => Ok(with_suggestions_text(render_text(response), &suggestions)),
    Format::Json => edit_json(&render_json(response)?, |document| {
      if let Some(answer) = document.get_mut("answer").and_then(Value::as_object_mut) {
        answer.insert("did_you_mean".to_owned(), json!(suggestions));
      }
    }),
  }
}

fn noted(text: String, format: Format, note: Option<String>) -> Result<String, ToolError> {
  let Some(note) = note else { return Ok(text) };
  match format {
    Format::Text => Ok(format!("{text}note: {note}\n")),
    Format::Json => edit_json(&text, |document| {
      if let Some(answer) = document.get_mut("answer").and_then(Value::as_object_mut) {
        answer.insert("note".to_owned(), json!(note));
      }
    }),
  }
}

pub(crate) fn answer_text(answer: &impl RenderText) -> String {
  let mut out = TextOut::default();
  answer.render_text(&mut out);
  out.finish()
}

pub(crate) fn cap_body(snippet: &mut Snippet, max_lines: usize) -> usize {
  let Snippet::Text { lines, omitted_lines, .. } = snippet else { return 0 };
  let dropped = lines.len().saturating_sub(max_lines);
  lines.truncate(max_lines);
  *omitted_lines += dropped;
  dropped
}

pub(crate) fn body_note(dropped: usize, max_lines: usize) -> Option<String> {
  (dropped > 0).then(|| format!("body capped at {max_lines} lines, {dropped} more not shown; target a narrower symbol (a nested method, or [repo:]path:line) to read the rest"))
}

pub(crate) fn cap(text: String, format: Format, max_bytes: usize) -> (String, usize) {
  if text.len() <= max_bytes {
    return (text, 0);
  }
  let total = text.len();
  let mut end = max_bytes;
  while !text.is_char_boundary(end) {
    end -= 1;
  }
  let note = format!("output truncated: {end} of {total} bytes; narrow the query (limit/depth/kind/repo) or use atlas_symbol mode=source on a specific symbol");
  let capped = match format {
    Format::Text => format!("{}\n[{note}]\n", &text[..end]),
    Format::Json => json!({ "truncated": true, "bytes": total, "note": note, "partial_text": &text[..end] }).to_string() + "\n",
  };
  (capped, total - end)
}

pub(crate) fn cap_with_caveat(text: String, format: Format, max_bytes: usize, caveat: Option<&str>) -> (String, usize) {
  let Some(caveat) = caveat else { return cap(text, format, max_bytes) };
  let (capped, dropped) = cap(text, format, max_bytes.saturating_sub(caveat_overhead(caveat)));
  (with_caveat(capped, format, Some(caveat)), dropped)
}

fn caveat_overhead(caveat: &str) -> usize {
  json!(caveat).to_string().len() + CAVEAT_FIELD_OVERHEAD
}

pub(crate) fn with_caveat(text: String, format: Format, caveat: Option<&str>) -> String {
  let Some(caveat) = caveat else { return text };
  let as_json = match format {
    Format::Json => with_json_field(&text, CAVEAT_KEY, caveat),
    Format::Text => None,
  };
  as_json.unwrap_or_else(|| {
    let separator = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
    format!("{text}{separator}{caveat}\n")
  })
}

fn with_json_field(text: &str, key: &str, value: &str) -> Option<String> {
  if !matches!(serde_json::from_str::<Value>(text), Ok(Value::Object(_))) {
    return None;
  }
  let body = text.trim_end().strip_suffix('}')?.trim_end();
  let separator = if body.ends_with('{') { "" } else { "," };
  Some(format!("{body}{separator}\n  {}: {}\n}}\n", json!(key), json!(value)))
}

pub(crate) fn edit_json(json: &str, edit: impl FnOnce(&mut Value)) -> Result<String, ToolError> {
  let mut document: Value = serde_json::from_str(json).map_err(QueryError::from)?;
  edit(&mut document);
  let mut text = serde_json::to_string_pretty(&document).map_err(QueryError::from)?;
  text.push('\n');
  Ok(text)
}

fn suggest(query: &QueryEngine, spec: &str) -> Vec<String> {
  let answer = match query.search(&SearchRequest::new(spec)) {
    Ok(response) => response.answer,
    Err(error) => {
      tracing::debug!(error = %error, "could not compute did-you-mean suggestions");
      return Vec::new();
    }
  };
  answer
    .exact_name
    .iter()
    .chain(&answer.strong)
    .chain(&answer.related)
    .take(SUGGESTIONS)
    .map(|hit| suggestion_line(&hit.entity))
    .collect()
}

fn suggestion_line(entity: &EntityRef) -> String {
  let location = entity.location.as_ref().map(|location| format!(" {location}")).unwrap_or_default();
  let tag = entity.tag().map(|tag| format!(" {tag}")).unwrap_or_default();
  format!("{} {}{location}{tag}  {}", entity.short, entity.kind, entity.key)
}

fn with_suggestions_text(mut text: String, suggestions: &[String]) -> String {
  if suggestions.is_empty() {
    text.push_str("no similar names found; try atlas_search with different words\n");
    return text;
  }
  text.push_str("did you mean (pass the full key to be exact):\n");
  for suggestion in suggestions {
    text.push_str("  ");
    text.push_str(suggestion);
    text.push('\n');
  }
  text
}

#[cfg(test)]
mod tests {
  use atlas_core::{Certainty, Confidence, Entity, EntityKey, EntityKind};

  use super::*;

  #[test]
  fn should_cut_text_on_a_char_boundary_and_report_the_dropped_bytes() {
    let (capped, dropped) = cap("aéééé".to_owned(), Format::Text, 4);

    assert!(capped.starts_with("aé\n[output truncated: 3 of 9 bytes;"), "{capped}");
    assert_eq!(dropped, 6);
  }

  #[test]
  fn should_leave_output_within_the_cap_untouched() {
    assert_eq!(cap("abc".to_owned(), Format::Json, 3), ("abc".to_owned(), 0));
  }

  #[test]
  fn should_append_a_caveat_line_to_text_and_a_last_caveat_field_to_json() {
    let text = with_caveat("fresh run=1\nanswer".to_owned(), Format::Text, Some("worktree caveat: x"));
    let json_text = with_caveat("{\n  \"zeta\": 1,\n  \"alpha\": [1, 2]\n}\n".to_owned(), Format::Json, Some("worktree caveat: \"x\""));

    assert_eq!(text, "fresh run=1\nanswer\nworktree caveat: x\n");
    assert_eq!(json_text, "{\n  \"zeta\": 1,\n  \"alpha\": [1, 2],\n  \"caveat\": \"worktree caveat: \\\"x\\\"\"\n}\n");
    assert_eq!(serde_json::from_str::<Value>(&json_text).unwrap()["caveat"], "worktree caveat: \"x\"");
    assert_eq!(with_caveat("{}".to_owned(), Format::Json, Some("c")), "{\n  \"caveat\": \"c\"\n}\n");
    assert_eq!(with_caveat("same\n".to_owned(), Format::Text, None), "same\n");
  }

  #[test]
  fn should_keep_the_caveat_within_the_output_cap() {
    let caveat = "worktree caveat: x";
    let fits = "a".repeat(40);
    let max = fits.len() + caveat_overhead(caveat);

    let (untouched, kept) = cap_with_caveat(fits.clone(), Format::Text, max, Some(caveat));
    let (capped, dropped) = cap_with_caveat(format!("{fits}b"), Format::Text, max, Some(caveat));
    let (json_capped, _) = cap_with_caveat(format!("{{\"answer\":\"{}\"}}", "a".repeat(200)), Format::Json, 120, Some(caveat));

    assert_eq!((untouched.as_str(), kept), (format!("{fits}\n{caveat}\n").as_str(), 0));
    assert!(untouched.len() <= max);
    assert_eq!(dropped, 1);
    assert!(capped.starts_with(&format!("{fits}\n[output truncated: 40 of 41 bytes;")), "{capped}");
    assert!(capped.ends_with(&format!("]\n{caveat}\n")), "{capped}");
    let document: Value = serde_json::from_str(&json_capped).unwrap();
    assert_eq!((document["truncated"].clone(), document["caveat"].clone()), (json!(true), json!(caveat)));
  }

  #[test]
  fn should_wrap_truncated_json_as_a_valid_document() {
    let (capped, dropped) = cap("{\"a\":\"éé\"}".to_owned(), Format::Json, 6);

    let document: Value = serde_json::from_str(&capped).unwrap();
    assert_eq!(document["truncated"], true);
    assert_eq!(document["bytes"], 12);
    assert_eq!(document["partial_text"], "{\"a\":\"");
    assert_eq!(dropped, 6);
  }

  #[test]
  fn should_tag_a_non_authoritative_suggestion_and_leave_an_authoritative_one_bare() {
    let tagged = Certainty::heuristic(Confidence::new(0.8).unwrap(), "route match");
    let key = EntityKey::Other {
      kind: EntityKind::AuthScheme,
      repo: None,
      name: "bearer".to_owned(),
    };
    let heuristic = EntityRef::of(&Entity::new(key.clone(), tagged.clone()));
    let syntactic = EntityRef::of(&Entity::new(key, Certainty::Syntactic));

    assert_eq!(suggestion_line(&syntactic), format!("{} {}  {}", syntactic.short, syntactic.kind, syntactic.key));
    assert_eq!(
      suggestion_line(&heuristic),
      format!("{} {} {}  {}", heuristic.short, heuristic.kind, tagged.tag(), heuristic.key)
    );
  }
}
