use atlas_core::{ARG_MAX_TEXT, Span, truncate_text};
use tree_sitter::Node;

use atlas_core::Deadline;

const MAX_SIGNATURE_BYTES: usize = 512;
const SCAN_FACTOR: usize = 4;

pub(crate) struct LineIndex {
  starts: Vec<usize>,
  len: usize,
}

impl LineIndex {
  pub(crate) fn new(text: &str) -> Self {
    let starts = std::iter::once(0).chain(text.match_indices('\n').map(|(index, _)| index + 1)).collect();
    Self { starts, len: text.len() }
  }

  pub(crate) fn span(&self, start: usize, end: usize) -> Span {
    let end = end.min(self.len);
    let start = start.min(end);
    let (start_line, start_col) = self.position(start);
    let (end_line, end_col) = self.position(end);
    Span {
      start_byte: saturate(start),
      end_byte: saturate(end),
      start_line,
      start_col,
      end_line,
      end_col,
    }
  }

  fn position(&self, byte: usize) -> (u32, u32) {
    let line = self.starts.partition_point(|start| *start <= byte).saturating_sub(1);
    let line_start = self.starts.get(line).copied().unwrap_or(0);
    (saturate(line + 1), saturate(byte - line_start))
  }
}

fn saturate(value: usize) -> u32 {
  u32::try_from(value).unwrap_or(u32::MAX)
}

#[derive(Clone, Copy)]
pub(crate) struct Doc<'t> {
  text: &'t str,
  base: usize,
  offset: usize,
  deadline: &'t Deadline,
}

impl<'t> Doc<'t> {
  pub(crate) fn root(text: &'t str, deadline: &'t Deadline) -> Self {
    Self {
      text,
      base: 0,
      offset: 0,
      deadline,
    }
  }

  pub(crate) fn embedded(text: &'t str, base: usize, offset: usize, deadline: &'t Deadline) -> Self {
    Self { text, base, offset, deadline }
  }

  pub(crate) fn deadline(&self) -> &'t Deadline {
    self.deadline
  }

  pub(crate) fn text(&self, node: Node<'_>) -> &'t str {
    self.slice(node.start_byte(), node.end_byte())
  }

  pub(crate) fn identifier(&self, node: Node<'_>) -> &'t str {
    let text = self.text(node);
    text.strip_prefix("r#").unwrap_or(text)
  }

  pub(crate) fn slice(&self, start: usize, end: usize) -> &'t str {
    self.text.get(start..end).unwrap_or("")
  }

  pub(crate) fn absolute(&self, byte: usize) -> usize {
    self.base + byte.saturating_sub(self.offset)
  }
}

pub(crate) fn normalize_whitespace(text: &str) -> String {
  text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn clip(text: &str, max: usize) -> &str {
  if text.len() <= max {
    return text;
  }
  let mut end = max;
  while !text.is_char_boundary(end) {
    end -= 1;
  }
  &text[..end]
}

pub(crate) fn short_text(text: &str) -> String {
  let clipped = clip(text, ARG_MAX_TEXT * SCAN_FACTOR);
  let mut summary = truncate_text(clipped);
  if clipped.len() < text.len() && !summary.ends_with('…') {
    let mut end = summary.len().min(ARG_MAX_TEXT - '…'.len_utf8());
    while !summary.is_char_boundary(end) {
      end -= 1;
    }
    summary.truncate(end);
    summary.push('…');
  }
  summary
}

pub(crate) fn signature_text(text: &str) -> String {
  let normalized = normalize_whitespace(clip(text, MAX_SIGNATURE_BYTES * SCAN_FACTOR));
  let trimmed = normalized.trim_end_matches([';', ' ']);
  if trimmed.len() <= MAX_SIGNATURE_BYTES {
    return trimmed.to_owned();
  }
  format!("{}…", clip(trimmed, MAX_SIGNATURE_BYTES - '…'.len_utf8()))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_compute_spans_identical_to_core_span_of_text() {
    let text = "fn a() {}\n  fn bé() {}\n";
    let index = LineIndex::new(text);
    for (start, end) in [(0, 2), (15, 18), (10, 24), (24, 24)] {
      assert_eq!(index.span(start, end), Span::of_text(text, start, end).unwrap());
    }
  }

  #[test]
  fn should_clamp_out_of_range_spans_to_the_text() {
    let index = LineIndex::new("abc");
    assert_eq!(index.span(5, 9), Span::of_text("abc", 3, 3).unwrap());
  }

  #[test]
  fn should_bound_short_text_with_an_ellipsis_when_the_input_is_clipped() {
    let long = format!("{}{}", " ".repeat(700), "x".repeat(10));
    let summary = short_text(&long);
    assert_eq!(summary, "…");
    let words = "word ".repeat(400);
    let summary = short_text(&words);
    assert!(summary.ends_with('…'));
    assert!(summary.len() <= ARG_MAX_TEXT);
  }

  #[test]
  fn should_normalise_signatures_and_strip_trailing_semicolons() {
    assert_eq!(signature_text("pub fn f(\n  a: u8,\n) -> u8;"), "pub fn f( a: u8, ) -> u8");
    let long = format!("fn f({})", "a: u8, ".repeat(200));
    let signature = signature_text(&long);
    assert!(signature.ends_with('…'));
    assert!(signature.len() <= MAX_SIGNATURE_BYTES);
  }
}
