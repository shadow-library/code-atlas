use atlas_core::Span;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::{LspError, LspResult};
use crate::protocol::{Position, Range};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PositionEncoding {
  #[serde(rename = "utf-8")]
  Utf8,
  #[serde(rename = "utf-16")]
  Utf16,
  #[serde(rename = "utf-32")]
  Utf32,
}

impl PositionEncoding {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Utf8 => "utf-8",
      Self::Utf16 => "utf-16",
      Self::Utf32 => "utf-32",
    }
  }

  pub fn parse(name: &str) -> Option<Self> {
    match name {
      "utf-8" => Some(Self::Utf8),
      "utf-16" => Some(Self::Utf16),
      "utf-32" => Some(Self::Utf32),
      _ => None,
    }
  }

  fn units(self, character: char) -> u32 {
    match self {
      Self::Utf8 => character.len_utf8() as u32,
      Self::Utf16 => character.len_utf16() as u32,
      Self::Utf32 => 1,
    }
  }
}

pub fn negotiate_encoding(offered: &[PositionEncoding], chosen: Option<&str>) -> LspResult<PositionEncoding> {
  let Some(name) = chosen else {
    return Ok(PositionEncoding::Utf16);
  };
  let Some(encoding) = PositionEncoding::parse(name) else {
    return Err(LspError::UnsupportedEncoding { encoding: name.to_owned() });
  };
  if encoding != PositionEncoding::Utf16 && !offered.contains(&encoding) {
    tracing::warn!(encoding = encoding.as_str(), "language server chose a position encoding the client did not offer");
  }
  Ok(encoding)
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum PositionError {
  #[error("line {line} is past the last line {last}")]
  LineOutOfRange { line: u32, last: u32 },
  #[error("position {line}:{character} points inside a character")]
  InsideCharacter { line: u32, character: u32 },
  #[error("position {line}:{character} is past the end of the line ({len} units)")]
  PastLineEnd { line: u32, character: u32, len: u32 },
  #[error("byte offset {offset} is past the end of the text ({len} bytes)")]
  OffsetOutOfRange { offset: usize, len: usize },
  #[error("byte offset {offset} is not on a character boundary")]
  NotACharBoundary { offset: usize },
  #[error("byte offset {offset} splits a line break")]
  InsideLineBreak { offset: usize },
  #[error("range end {end} precedes its start {start}")]
  InvertedRange { start: usize, end: usize },
  #[error("byte offset {offset} does not fit a graph span")]
  TooLarge { offset: usize },
}

/// rust-analyzer breaks lines on `\n` only (a lone `\r` is a character); tsserver follows ECMAScript and also breaks on `\r`, U+2028 and U+2029.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineBreaks {
  Lf,
  Ecma,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PastEnd {
  Clamp,
  Reject,
}

#[derive(Debug, Clone, Copy)]
struct Line {
  start: usize,
  content_end: usize,
}

#[derive(Debug, Clone)]
pub struct LineIndex<'a> {
  text: &'a str,
  lines: Vec<Line>,
  graph_line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
  pub fn new(text: &'a str, breaks: LineBreaks) -> Self {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, character)) = chars.next() {
      let (content_end, next_start) = match (breaks, character) {
        (_, '\n') if text[..at].ends_with('\r') => (at - 1, at + 1),
        (_, '\n') => (at, at + 1),
        (LineBreaks::Ecma, '\r') if chars.next_if(|(_, next)| *next == '\n').is_some() => (at, at + 2),
        (LineBreaks::Ecma, '\r' | '\u{2028}' | '\u{2029}') => (at, at + character.len_utf8()),
        _ => continue,
      };
      lines.push(Line { start, content_end });
      start = next_start;
    }
    lines.push(Line { start, content_end: text.len() });
    let graph_line_starts = std::iter::once(0).chain(text.match_indices('\n').map(|(at, _)| at + 1)).collect();
    Self { text, lines, graph_line_starts }
  }

  pub fn text(&self) -> &'a str {
    self.text
  }

  pub fn line_count(&self) -> u32 {
    u32::try_from(self.lines.len()).unwrap_or(u32::MAX)
  }

  pub fn offset(&self, position: Position, encoding: PositionEncoding) -> Result<usize, PositionError> {
    self.offset_with(position, encoding, PastEnd::Clamp)
  }

  pub fn offset_strict(&self, position: Position, encoding: PositionEncoding) -> Result<usize, PositionError> {
    self.offset_with(position, encoding, PastEnd::Reject)
  }

  fn offset_with(&self, position: Position, encoding: PositionEncoding, past_end: PastEnd) -> Result<usize, PositionError> {
    let line = self.line(position.line)?;
    let content = &self.text[line.start..line.content_end];
    let mut units = 0u32;
    for (byte, character) in content.char_indices() {
      if units == position.character {
        return Ok(line.start + byte);
      }
      units += encoding.units(character);
      if units > position.character {
        return Err(PositionError::InsideCharacter {
          line: position.line,
          character: position.character,
        });
      }
    }
    if units < position.character && past_end == PastEnd::Reject {
      return Err(PositionError::PastLineEnd {
        line: position.line,
        character: position.character,
        len: units,
      });
    }
    Ok(line.content_end)
  }

  pub fn position(&self, offset: usize, encoding: PositionEncoding) -> Result<Position, PositionError> {
    if offset > self.text.len() {
      return Err(PositionError::OffsetOutOfRange { offset, len: self.text.len() });
    }
    if !self.text.is_char_boundary(offset) {
      return Err(PositionError::NotACharBoundary { offset });
    }
    let index = self.lines.partition_point(|line| line.start <= offset) - 1;
    let line = self.lines[index];
    if offset > line.content_end {
      return Err(PositionError::InsideLineBreak { offset });
    }
    Ok(Position {
      line: u32::try_from(index).unwrap_or(u32::MAX),
      character: self.text[line.start..offset].chars().map(|character| encoding.units(character)).sum(),
    })
  }

  pub fn offsets(&self, range: Range, encoding: PositionEncoding) -> Result<std::ops::Range<usize>, PositionError> {
    ordered(self.offset(range.start, encoding)?, self.offset(range.end, encoding)?)
  }

  pub fn offsets_strict(&self, range: Range, encoding: PositionEncoding) -> Result<std::ops::Range<usize>, PositionError> {
    ordered(self.offset_strict(range.start, encoding)?, self.offset_strict(range.end, encoding)?)
  }

  pub fn range(&self, offsets: std::ops::Range<usize>, encoding: PositionEncoding) -> Result<Range, PositionError> {
    let offsets = ordered(offsets.start, offsets.end)?;
    Ok(Range {
      start: self.position(offsets.start, encoding)?,
      end: self.position(offsets.end, encoding)?,
    })
  }

  pub fn span(&self, range: Range, encoding: PositionEncoding) -> Result<Span, PositionError> {
    self.graph_span(self.offsets(range, encoding)?)
  }

  pub fn graph_span(&self, offsets: std::ops::Range<usize>) -> Result<Span, PositionError> {
    let offsets = ordered(offsets.start, offsets.end)?;
    let (start_line, start_col) = self.graph_position(offsets.start)?;
    let (end_line, end_col) = self.graph_position(offsets.end)?;
    let byte = |offset: usize| u32::try_from(offset).map_err(|_| PositionError::TooLarge { offset });
    Ok(Span {
      start_byte: byte(offsets.start)?,
      end_byte: byte(offsets.end)?,
      start_line,
      start_col,
      end_line,
      end_col,
    })
  }

  fn graph_position(&self, offset: usize) -> Result<(u32, u32), PositionError> {
    if offset > self.text.len() {
      return Err(PositionError::OffsetOutOfRange { offset, len: self.text.len() });
    }
    let index = self.graph_line_starts.partition_point(|start| *start <= offset) - 1;
    let too_large = PositionError::TooLarge { offset };
    let line = u32::try_from(index + 1).map_err(|_| too_large.clone())?;
    let col = u32::try_from(offset - self.graph_line_starts[index]).map_err(|_| too_large)?;
    Ok((line, col))
  }

  fn line(&self, line: u32) -> Result<Line, PositionError> {
    let last = self.line_count() - 1;
    usize::try_from(line)
      .ok()
      .and_then(|index| self.lines.get(index))
      .copied()
      .ok_or(PositionError::LineOutOfRange { line, last })
  }
}

fn ordered(start: usize, end: usize) -> Result<std::ops::Range<usize>, PositionError> {
  match end < start {
    true => Err(PositionError::InvertedRange { start, end }),
    false => Ok(start..end),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use LineBreaks::{Ecma, Lf};
  use PositionEncoding::{Utf8, Utf16, Utf32};

  fn at(line: u32, character: u32) -> Position {
    Position { line, character }
  }

  fn boundaries(text: &str) -> impl Iterator<Item = usize> + '_ {
    text.char_indices().map(|(offset, _)| offset).chain([text.len()])
  }

  #[test]
  fn should_convert_utf16_columns_around_astral_characters() {
    let text = "a😀b = 1;";
    let index = LineIndex::new(text, Lf);
    let b = text.find('b').unwrap();

    assert_eq!(index.position(b, Utf16), Ok(at(0, 3)));
    assert_eq!(index.offset(at(0, 3), Utf16), Ok(b));
    assert_eq!(index.offset(at(0, 2), Utf16), Err(PositionError::InsideCharacter { line: 0, character: 2 }));
    assert_eq!(index.position(b, Utf8), Ok(at(0, 5)));
    assert_eq!(index.position(b, Utf32), Ok(at(0, 2)));
    assert_eq!(index.offset(at(0, 2), Utf32), Ok(b));
  }

  #[test]
  fn should_convert_every_encoding_over_mixed_multi_byte_text() {
    let text = "fn é() {}\n  let ü = \"€😀\"; x\n";
    let index = LineIndex::new(text, Lf);
    let x = text.rfind('x').unwrap();

    assert_eq!(index.position(x, Utf8), Ok(at(1, 22)));
    assert_eq!(index.position(x, Utf16), Ok(at(1, 17)));
    assert_eq!(index.position(x, Utf32), Ok(at(1, 16)));
  }

  #[test]
  fn should_round_trip_every_char_boundary_in_every_encoding_and_convention() {
    let text = "α\r\nβ😀γ\rδ\n\n€x\u{2028}y\u{2029}z\r\n";
    for breaks in [Lf, Ecma] {
      let index = LineIndex::new(text, breaks);
      for encoding in [Utf8, Utf16, Utf32] {
        for offset in boundaries(text) {
          match index.position(offset, encoding) {
            Ok(position) => assert_eq!(index.offset_strict(position, encoding), Ok(offset), "{breaks:?} {encoding:?} offset {offset}"),
            Err(error) => assert_eq!(error, PositionError::InsideLineBreak { offset }, "{breaks:?} {encoding:?}"),
          }
        }
      }
    }
  }

  #[test]
  fn should_not_break_rust_analyzer_lines_on_a_lone_cr() {
    let text = "// one\rtwo\npub fn foo() {}\n";
    let index = LineIndex::new(text, Lf);
    let foo = text.find("foo").unwrap();

    assert_eq!(index.line_count(), 3);
    assert_eq!(index.position(foo, Utf8), Ok(at(1, 7)));
    assert_eq!(index.position(text.find("two").unwrap(), Utf16), Ok(at(0, 7)));
  }

  #[test]
  fn should_break_typescript_lines_on_a_lone_cr_and_unicode_separators() {
    let separators = "const s = \"x\u{2028}y\";\nfunction foo() {}\n";
    let lone_cr = "// one\rtwo\nfunction foo() {}\n";
    let ts = LineIndex::new(separators, Ecma);
    let cr = LineIndex::new(lone_cr, Ecma);

    assert_eq!(ts.line_count(), 4);
    assert_eq!(ts.position(separators.find('y').unwrap(), Utf16), Ok(at(1, 0)));
    assert_eq!(ts.position(separators.find("foo").unwrap(), Utf16), Ok(at(2, 9)));
    assert_eq!(cr.position(lone_cr.find("two").unwrap(), Utf16), Ok(at(1, 0)));
    assert_eq!(cr.position(lone_cr.find("foo").unwrap(), Utf16), Ok(at(2, 9)));
    assert_eq!(LineIndex::new(separators, Lf).position(separators.find("foo").unwrap(), Utf16), Ok(at(1, 9)));
  }

  #[test]
  fn should_treat_crlf_as_one_break_in_both_conventions() {
    let text = "ab\r\ncd";
    for breaks in [Lf, Ecma] {
      let index = LineIndex::new(text, breaks);
      assert_eq!(index.line_count(), 2);
      assert_eq!(index.offset(at(1, 0), Utf16), Ok(4));
      assert_eq!(index.offset(at(0, 99), Utf16), Ok(2), "{breaks:?}");
      assert_eq!(index.position(3, Utf16), Err(PositionError::InsideLineBreak { offset: 3 }), "{breaks:?}");
    }
  }

  #[test]
  fn should_clamp_past_the_line_end_for_reads_and_reject_it_for_edits() {
    let index = LineIndex::new("ab\r\ncd", Lf);

    assert_eq!(index.offset(at(0, 3), Utf16), Ok(2));
    assert_eq!(index.offset_strict(at(0, 2), Utf16), Ok(2));
    assert_eq!(index.offset_strict(at(0, 3), Utf16), Err(PositionError::PastLineEnd { line: 0, character: 3, len: 2 }));
    assert_eq!(
      index.offsets_strict(Range { start: at(1, 0), end: at(1, 5) }, Utf16),
      Err(PositionError::PastLineEnd { line: 1, character: 5, len: 2 })
    );
  }

  #[test]
  fn should_reject_offsets_inside_a_character_or_past_the_end() {
    let index = LineIndex::new("ab\r\né", Lf);

    assert_eq!(index.position(5, Utf16), Err(PositionError::NotACharBoundary { offset: 5 }));
    assert_eq!(index.position(7, Utf16), Err(PositionError::OffsetOutOfRange { offset: 7, len: 6 }));
  }

  #[test]
  fn should_reject_a_line_past_the_end_and_accept_the_empty_line_after_a_trailing_newline() {
    let index = LineIndex::new("a\n", Lf);

    assert_eq!(index.offset(at(1, 0), Utf16), Ok(2));
    assert_eq!(index.offset(at(2, 0), Utf16), Err(PositionError::LineOutOfRange { line: 2, last: 1 }));
    assert_eq!(LineIndex::new("", Ecma).offset(at(0, 0), Utf8), Ok(0));
  }

  #[test]
  fn should_reject_a_utf8_column_inside_a_multi_byte_character() {
    let index = LineIndex::new("é=1", Lf);

    assert_eq!(index.offset(at(0, 1), Utf8), Err(PositionError::InsideCharacter { line: 0, character: 1 }));
    assert_eq!(index.offset(at(0, 2), Utf8), Ok(2));
  }

  #[test]
  fn should_convert_a_range_to_offsets_and_a_graph_span() {
    let text = "x\r\n  fé(1);\r\n";
    let index = LineIndex::new(text, Lf);
    let range = Range { start: at(1, 2), end: at(1, 4) };

    assert_eq!(index.offsets(range, Utf16), Ok(5..8));
    let span = index.span(range, Utf16).unwrap();
    assert_eq!((span.start_line, span.start_col, span.end_line, span.end_col), (2, 2, 2, 5));
    assert_eq!(span.text(text), Some("fé"));
    assert_eq!(index.range(5..8, Utf8), Ok(Range { start: at(1, 2), end: at(1, 5) }));
  }

  #[test]
  fn should_build_graph_spans_identical_to_span_of_text() {
    let text = "a\rb\r\nc\u{2028}d\n\né😀\n";
    for breaks in [Lf, Ecma] {
      let index = LineIndex::new(text, breaks);
      for start in boundaries(text) {
        for end in boundaries(text).filter(|end| *end >= start) {
          assert_eq!(index.graph_span(start..end).ok(), Span::of_text(text, start, end), "{breaks:?} {start}..{end}");
        }
      }
    }
  }

  #[test]
  fn should_reject_an_inverted_range() {
    let index = LineIndex::new("abc", Lf);

    assert_eq!(
      index.offsets(Range { start: at(0, 2), end: at(0, 1) }, Utf8),
      Err(PositionError::InvertedRange { start: 2, end: 1 })
    );
    let inverted = std::ops::Range { start: 2, end: 1 };
    assert_eq!(index.graph_span(inverted), Err(PositionError::InvertedRange { start: 2, end: 1 }));
  }

  #[test]
  fn should_negotiate_the_position_encoding() {
    let offered = [Utf8, Utf16];
    let cases: [(Option<&str>, Option<PositionEncoding>); 5] = [
      (None, Some(Utf16)),
      (Some("utf-8"), Some(Utf8)),
      (Some("utf-16"), Some(Utf16)),
      (Some("utf-32"), Some(Utf32)),
      (Some("utf-7"), None),
    ];
    for (chosen, expected) in cases {
      assert_eq!(negotiate_encoding(&offered, chosen).ok(), expected, "{chosen:?}");
    }
    assert_eq!(negotiate_encoding(&[Utf16], Some("utf-8")).ok(), Some(Utf8));
  }
}
