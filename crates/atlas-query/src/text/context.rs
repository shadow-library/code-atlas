use std::collections::BTreeMap;

use crate::engine::Snippet;
use crate::model::Listing;
use crate::queries::{ContextAnswer, ContextItem, LineRange, OmissionReason, OmittedItem, RangeBasis, SectionSummary};
use crate::text::{RenderText, TextOut, caveats, entity_head, snippet};

pub(crate) const MUST_READ: &str = "MUST READ";
pub(crate) const SKIM: &str = "SKIM";
pub(crate) const PATTERN: &str = "PATTERN";
pub(crate) const REUSE: &str = "REUSE CANDIDATES";
pub(crate) const IMPACT: &str = "IMPACT";
pub(crate) const TESTS: &str = "TESTS";
pub(crate) const UNCERTAIN: &str = "UNCERTAIN";

fn range(lines: &LineRange) -> String {
  let span = match lines.start == lines.end {
    true => lines.start.to_string(),
    false => format!("{}-{}", lines.start, lines.end),
  };
  match lines.basis {
    RangeBasis::Symbol => span,
    RangeBasis::Site => format!("{span} (site)"),
  }
}

fn place(item: &ContextItem) -> String {
  let entity = &item.entity;
  let located = match (&entity.location, &item.lines) {
    (Some(location), Some(lines)) => format!("{} {} {}:{}:{}", entity.short, entity.kind, location.repo, location.path, range(lines)),
    _ => entity_head(entity),
  };
  match entity.tag() {
    Some(tag) => format!("{located} {tag}"),
    None => located,
  }
}

pub fn context_item_line(item: &ContextItem) -> String {
  let flag = if item.uncertain { " UNCERTAIN" } else { "" };
  let edit = if item.read_directly { " (named in the task: read the file directly)" } else { "" };
  format!("{}{flag}{edit}  why: {}", place(item), item.why)
}

pub(crate) fn section_title(out: &mut TextOut, title: &str, count: usize) {
  if count > 0 {
    out.line(0, format!("{title} ({count})"));
  }
}

fn section(out: &mut TextOut, title: &str, items: &[ContextItem], summaries: &[SectionSummary], render: impl Fn(&mut TextOut, &ContextItem)) {
  if items.is_empty() {
    return;
  }
  section_title(out, title, items.len());
  for item in items {
    render(out, item);
  }
  for summary in summaries.iter().filter(|summary| summary.section == title) {
    let repos: Vec<String> = summary.by_repo.iter().map(|(repo, count)| format!("{repo} {count}")).collect();
    out.line(
      1,
      format!("{} more omitted ({} uncertain; {}): {}", summary.omitted, summary.uncertain, repos.join(", "), summary.hint),
    );
  }
}

pub(crate) fn mention_item(out: &mut TextOut, item: &ContextItem) {
  out.line(1, context_item_line(item));
}

pub(crate) fn excerpt_lines(out: &mut TextOut, excerpt: &Snippet) {
  snippet(out, 2, excerpt);
}

pub(crate) fn skim_item(out: &mut TextOut, item: &ContextItem) {
  out.line(1, context_item_line(item));
  if let Some(signature) = &item.signature {
    out.line(2, format!("sig {signature}"));
  }
  if let Some(doc) = &item.doc {
    out.line(2, format!("doc {doc}"));
  }
  if let Some(excerpt) = &item.excerpt {
    excerpt_lines(out, excerpt);
  }
  for member in &item.skeleton {
    out.line(3, member);
  }
}

pub(crate) fn must_read_item(out: &mut TextOut, item: &ContextItem) {
  let Some(body) = &item.body else { return skim_item(out, item) };
  out.line(1, context_item_line(item));
  snippet(out, 2, body);
  for member in &item.skeleton {
    out.line(3, member);
  }
}

pub(crate) fn test_item(out: &mut TextOut, item: &ContextItem) {
  out.line(1, context_item_line(item));
  if let Some(run) = &item.run {
    out.line(2, format!("run: {run}"));
  }
}

pub(crate) fn omitted_section(out: &mut TextOut, omitted: &Listing<OmittedItem>, by_reason: &BTreeMap<OmissionReason, usize>, full: bool) {
  if omitted.is_empty() {
    return;
  }
  if full {
    out.line(0, format!("OMITTED ({})", omitted.total));
    for item in &omitted.items {
      out.line(1, format!("{} [{}]", item.label, item.reason.as_str()));
    }
    if omitted.omitted() > 0 {
      out.line(1, format!("{} more omitted items (scores with explain=true)", omitted.omitted()));
    }
    return;
  }
  let mut counts: Vec<(OmissionReason, usize)> = by_reason
    .iter()
    .filter(|(reason, _)| !reason.is_listed_when_collapsed())
    .map(|(reason, count)| (*reason, *count))
    .collect();
  counts.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
  let parts: Vec<String> = counts.iter().map(|(reason, count)| reason_count(omitted, *reason, *count)).collect();
  match parts.is_empty() {
    true => out.line(0, format!("OMITTED ({})", omitted.total)),
    false => out.line(0, format!("OMITTED ({}): {} (scores with explain=true)", omitted.total, parts.join("; "))),
  }
  listed(out, omitted, by_reason, OmissionReason::is_seed_diagnostic, "seed diagnostics");
  listed(out, omitted, by_reason, |reason| reason == OmissionReason::Held, "known entries holding items back");
}

fn listed(out: &mut TextOut, omitted: &Listing<OmittedItem>, by_reason: &BTreeMap<OmissionReason, usize>, selects: impl Fn(OmissionReason) -> bool, noun: &str) {
  let shown: Vec<&OmittedItem> = omitted.items.iter().filter(|item| selects(item.reason)).collect();
  for item in &shown {
    out.line(1, format!("{} [{}]", item.label, item.reason.as_str()));
  }
  let total: usize = by_reason.iter().filter(|(reason, _)| selects(**reason)).map(|(_, count)| count).sum();
  if total > shown.len() {
    out.line(1, format!("{} more {noun} (list them with explain=true)", total - shown.len()));
  }
}

fn reason_count(omitted: &Listing<OmittedItem>, reason: OmissionReason, count: usize) -> String {
  let aggregate = omitted.items.iter().find(|item| item.reason == reason && reason == OmissionReason::BelowConfidenceFloor);
  match aggregate {
    Some(item) => item.label.clone(),
    None => format!("{count} {}", reason.as_str()),
  }
}

impl RenderText for ContextAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("context for {:?} (~{} of {} tokens)", self.task, self.estimated_tokens, self.budget));
    let summaries = &self.summaries;
    section(out, MUST_READ, &self.must_read, summaries, must_read_item);
    section(out, SKIM, &self.skim, summaries, skim_item);
    section(out, PATTERN, &self.patterns, summaries, mention_item);
    section(out, REUSE, &self.reuse, summaries, mention_item);
    section(out, IMPACT, &self.impact, summaries, mention_item);
    section(out, TESTS, &self.tests, summaries, test_item);
    section(out, UNCERTAIN, &self.uncertain, summaries, mention_item);
    omitted_section(out, &self.omitted, &self.omitted_by_reason, self.explain.is_some());
    if let Some(explain) = &self.explain {
      out.line(0, "EXPLAIN seeds");
      for seed in &explain.seeds {
        out.line(1, seed);
      }
      out.line(0, "EXPLAIN expansions");
      for expansion in &explain.expansions {
        out.line(1, expansion);
      }
      out.line(0, "EXPLAIN scores");
      for scored in &explain.scores {
        out.line(1, format!("{:.6} {}", scored.score, scored.key));
      }
    }
    caveats(out, &self.caveats);
  }
}
