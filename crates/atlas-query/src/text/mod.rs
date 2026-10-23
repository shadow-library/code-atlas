mod also_change;
mod api;
pub(crate) mod context;
mod counterparts;
pub(crate) mod docs;
mod explain;
mod impact;
mod overview;
mod patterns;
mod reuse_check;
mod search;
mod similar;
mod status;
mod symbol;
mod symbols;
mod tests_for;
mod traverse;
mod unused;

use serde::Serialize;

use crate::engine::Snippet;
use crate::error::QueryResult;
use crate::freshness::{Freshness, FreshnessState, ScipState};
use crate::model::{Caveat, EntityRef, Hop, Listing, MacroGapScope, Response, Targeted};

pub use context::context_item_line;

const FAILED_FILES_SHOWN: usize = 10;

pub trait RenderText {
  fn render_text(&self, out: &mut TextOut);
}

#[derive(Debug, Default)]
pub struct TextOut {
  buffer: String,
}

impl TextOut {
  pub fn line(&mut self, indent: usize, text: impl AsRef<str>) {
    for _ in 0..indent {
      self.buffer.push_str("  ");
    }
    self.buffer.push_str(text.as_ref().trim_end());
    self.buffer.push('\n');
  }

  pub fn finish(self) -> String {
    self.buffer
  }
}

pub(crate) fn rendered_chars(render: impl FnOnce(&mut TextOut)) -> usize {
  let mut out = TextOut::default();
  render(&mut out);
  out.buffer.chars().count()
}

pub fn render_text<T: RenderText>(response: &Response<T>) -> String {
  let mut out = TextOut::default();
  render_freshness(&response.freshness, &mut out);
  response.answer.render_text(&mut out);
  out.finish()
}

pub fn render_json<T: Serialize>(response: &Response<T>) -> QueryResult<String> {
  let mut text = serde_json::to_string_pretty(response)?;
  text.push('\n');
  Ok(text)
}

fn render_freshness(freshness: &Freshness, out: &mut TextOut) {
  let run = freshness.run.map_or_else(|| "none".to_owned(), |run| run.to_string());
  let stats = format!("+{} files, {}ms", freshness.files_updated, freshness.duration_ms);
  let mut notes: Vec<String> = Vec::new();
  if freshness.stale_inferred_facts > 0 {
    notes.push(format!("{} stale inferred facts (hidden unless include_stale=true)", freshness.stale_inferred_facts));
  }
  if freshness.files_failed_analysis > 0 {
    notes.push(format!("{} files failed analysis", freshness.files_failed_analysis));
  }
  if freshness.skipped_files > 0 {
    notes.push(format!("skipped {} files (see status)", freshness.skipped_files));
  }
  if let ScipState::Behind { files } = freshness.scip {
    notes.push(format!("scip behind on {files} files"));
  }
  let suffix: String = notes.iter().map(|note| format!(" | {note}")).collect();
  match &freshness.state {
    FreshnessState::Fresh => out.line(0, format!("fresh run={run} ({stats}){suffix}")),
    FreshnessState::Degraded { reason, failed_files } => {
      out.line(0, format!("DEGRADED: {reason} run={run} ({stats}){suffix}"));
      for file in failed_files.iter().take(FAILED_FILES_SHOWN) {
        let location = match &file.path {
          Some(path) => format!("{}:{path}", file.repo),
          None => file.repo.clone(),
        };
        out.line(1, format!("affected {location}: {}", file.reason));
      }
      omitted(out, 1, failed_files.len().saturating_sub(FAILED_FILES_SHOWN));
    }
  }
  for note in &freshness.notes {
    out.line(0, format!("note: {note}"));
  }
}

pub(crate) fn entity_head(entity: &EntityRef) -> String {
  match &entity.location {
    Some(location) => format!("{} {} {location}", entity.short, entity.kind),
    None => format!("{} {}", entity.short, entity.kind),
  }
}

pub(crate) fn entity_line(entity: &EntityRef) -> String {
  match entity.tag() {
    Some(tag) => format!("{} {tag}", entity_head(entity)),
    None => entity_head(entity),
  }
}

pub(crate) fn hop_text(hop: &Hop) -> String {
  format!("{} {} {}", hop.phrase, hop.entity, hop.certainty.tag)
}

pub(crate) fn counted(count: usize, noun: &str) -> String {
  match count {
    1 => format!("1 {noun}"),
    many => format!("{many} {noun}s"),
  }
}

pub(crate) fn labelled_text(entity: &EntityRef, relation: &Hop) -> String {
  format!("{} ({})", entity.tagged_short(), relation.certainty.tag)
}

pub(crate) fn phrased_text(entity: &EntityRef, relation: &Hop) -> String {
  format!("{} {} ({})", relation.phrase, entity.tagged_short(), relation.certainty.tag)
}

pub(crate) fn path_text(hops: &[Hop]) -> String {
  hops.iter().map(hop_text).collect::<Vec<_>>().join(" > ")
}

pub(crate) fn relation_text(relation: &Hop, path: &[Hop]) -> String {
  match path.is_empty() {
    true => hop_text(relation),
    false => path_text(path),
  }
}

pub(crate) fn omitted(out: &mut TextOut, indent: usize, count: usize) {
  if count > 0 {
    out.line(indent, format!("{count} more omitted (raise limit)"));
  }
}

pub(crate) fn listing<T>(out: &mut TextOut, indent: usize, listing: &Listing<T>, item: impl Fn(&mut TextOut, usize, &T)) {
  for entry in &listing.items {
    item(out, indent, entry);
  }
  omitted(out, indent, listing.omitted());
}

pub(crate) fn caveats(out: &mut TextOut, caveats: &[Caveat]) {
  if caveats.is_empty() {
    return;
  }
  out.line(0, "caveats:");
  for caveat in caveats {
    caveat_line(out, caveat);
  }
}

fn caveat_line(out: &mut TextOut, caveat: &Caveat) {
  match caveat {
    Caveat::Truncated { max_nodes } => out.line(1, format!("traversal truncated at {max_nodes} nodes; results are incomplete")),
    Caveat::UnresolvedRisk { name, total, by_reason, sites } => {
      let reasons: Vec<String> = by_reason.iter().map(|(reason, count)| format!("{count} {reason}")).collect();
      out.line(
        1,
        format!("{} named {name:?} could hide relations ({})", counted(*total, "unresolved reference"), reasons.join(", ")),
      );
      for site in sites {
        out.line(2, format!("{} {} ({})", site.location, site.path, site.reason));
      }
      omitted(out, 2, total.saturating_sub(sites.len()));
    }
    Caveat::DynamicMemberCalls { total, sites } => {
      out.line(1, format!("{} could reach this (computed property access)", counted(*total, "dynamic member call")));
      for site in sites {
        out.line(2, format!("{} {}", site.location, site.path));
      }
      omitted(out, 2, total.saturating_sub(sites.len()));
    }
    Caveat::MacroGeneratedRefs { target, scope, total, sites } => {
      let effect = match scope {
        MacroGapScope::Callees => "its callees may be incomplete",
        MacroGapScope::Dependencies => "its dependencies may be incomplete",
        MacroGapScope::Impact => "this concerns the target's own body and does not hide dependents",
      };
      out.line(
        1,
        format!(
          "{} in {target} to possibly macro-generated names {} not resolved; {effect}",
          counted(*total, "reference"),
          if *total == 1 { "was" } else { "were" }
        ),
      );
      for site in sites {
        out.line(2, format!("{} {}", site.location, site.path));
      }
      omitted(out, 2, total.saturating_sub(sites.len()));
    }
    Caveat::ProblemFiles { total, files } => {
      out.line(
        1,
        format!(
          "{} in scope parsed partially or failed; {} relations may be missing",
          counted(*total, "file"),
          if *total == 1 { "its" } else { "their" }
        ),
      );
      for file in files {
        out.line(2, format!("{} {}", file.location, file.status));
      }
      omitted(out, 2, total.saturating_sub(files.len()));
    }
    Caveat::BelowConfidenceFloor { hidden, fallback_tests, floor } => {
      let hidden_items = match (*hidden, *fallback_tests) {
        (relations, 0) => format!("{relations} relations"),
        (0, tests) => counted(tests, "possible test"),
        (relations, tests) => format!("{relations} relations and {}", counted(tests, "possible test")),
      };
      out.line(1, format!("{hidden_items} below confidence {floor:.2} hidden (lower min_confidence)"));
    }
    Caveat::InferredHidden { stale, low_confidence } => {
      let mut parts = Vec::new();
      if *stale > 0 {
        parts.push(format!("{stale} stale (set include_stale=true)"));
      }
      if *low_confidence > 0 {
        parts.push(format!("{low_confidence} low-confidence"));
      }
      out.line(1, format!("inferred facts hidden: {}", parts.join(", ")));
    }
    Caveat::MissingFacts { detail } => out.line(1, detail),
    Caveat::Note { message } => out.line(1, message),
  }
}

pub(crate) fn snippet(out: &mut TextOut, indent: usize, snippet: &Snippet) {
  match snippet {
    Snippet::Text { start_line, lines, omitted_lines } => {
      let width = (*start_line as usize + lines.len()).to_string().len();
      for (offset, line) in lines.iter().enumerate() {
        out.line(indent, format!("{:>width$}| {line}", *start_line as usize + offset));
      }
      if *omitted_lines > 0 {
        out.line(indent, format!("... {omitted_lines} more lines (request the full body: read with body=true)"));
      }
    }
    Snippet::Mismatch { detail } => out.line(indent, format!("SOURCE MISMATCH: {detail}")),
    Snippet::Unavailable { reason } => out.line(indent, format!("source unavailable: {reason}")),
  }
}

impl<T: RenderText> RenderText for Targeted<T> {
  fn render_text(&self, out: &mut TextOut) {
    match self {
      Self::Found(answer) => answer.render_text(out),
      Self::Ambiguous { spec, candidates } => {
        out.line(
          0,
          format!("ambiguous target {spec:?}: {} candidates; pass a full key or narrow with kind/repo", candidates.len()),
        );
        for candidate in candidates {
          out.line(1, format!("{}  {}", entity_line(candidate), candidate.key));
        }
      }
      Self::NotFound { spec, hint } => out.line(0, format!("not found: {spec:?} ({hint})")),
    }
  }
}
