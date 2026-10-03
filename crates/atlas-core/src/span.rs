use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
  pub line: u32,
  pub col: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Span {
  pub start_byte: u32,
  pub end_byte: u32,
  pub start_line: u32,
  pub start_col: u32,
  pub end_line: u32,
  pub end_col: u32,
}

impl Span {
  pub fn from_zero_based_rows(start_byte: u32, end_byte: u32, start_row: u32, start_col: u32, end_row: u32, end_col: u32) -> Self {
    Self {
      start_byte,
      end_byte,
      start_line: start_row.saturating_add(1),
      start_col,
      end_line: end_row.saturating_add(1),
      end_col,
    }
  }

  pub fn of_text(source: &str, start_byte: usize, end_byte: usize) -> Option<Self> {
    if start_byte > end_byte || end_byte > source.len() {
      return None;
    }
    let start = Self::position_of(source, start_byte);
    let end = Self::position_of(source, end_byte);
    Some(Self {
      start_byte: u32::try_from(start_byte).ok()?,
      end_byte: u32::try_from(end_byte).ok()?,
      start_line: start.line,
      start_col: start.col,
      end_line: end.line,
      end_col: end.col,
    })
  }

  fn position_of(source: &str, byte: usize) -> Position {
    let before = &source.as_bytes()[..byte];
    let line_start = before.iter().rposition(|b| *b == b'\n').map_or(0, |index| index + 1);
    let newlines = before.iter().filter(|b| **b == b'\n').count();
    Position {
      line: u32::try_from(newlines).unwrap_or(u32::MAX).saturating_add(1),
      col: u32::try_from(byte - line_start).unwrap_or(u32::MAX),
    }
  }

  pub fn start(&self) -> Position {
    Position {
      line: self.start_line,
      col: self.start_col,
    }
  }

  pub fn end(&self) -> Position {
    Position {
      line: self.end_line,
      col: self.end_col,
    }
  }

  pub fn len(&self) -> u32 {
    self.end_byte.saturating_sub(self.start_byte)
  }

  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  pub fn line_count(&self) -> u32 {
    self.end_line.saturating_sub(self.start_line) + 1
  }

  pub fn contains(&self, other: &Span) -> bool {
    self.start_byte <= other.start_byte && other.end_byte <= self.end_byte
  }

  pub fn contains_byte(&self, byte: u32) -> bool {
    self.start_byte <= byte && byte < self.end_byte
  }

  pub fn is_well_formed(&self) -> bool {
    self.start_byte <= self.end_byte && self.start_line >= 1 && (self.start_line, self.start_col) <= (self.end_line, self.end_col)
  }

  pub fn text<'a>(&self, source: &'a str) -> Option<&'a str> {
    source.get(usize::try_from(self.start_byte).ok()?..usize::try_from(self.end_byte).ok()?)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_use_one_based_lines_and_zero_based_byte_columns() {
    let source = "fn a() {}\n  fn bé() {}\n";
    let start = source.find("bé").unwrap();
    let span = Span::of_text(source, start, start + "bé".len()).unwrap();
    assert_eq!(
      span,
      Span {
        start_byte: 15,
        end_byte: 18,
        start_line: 2,
        start_col: 5,
        end_line: 2,
        end_col: 8
      }
    );
    assert_eq!(span.text(source), Some("bé"));
    assert_eq!(span.line_count(), 1);
  }

  #[test]
  fn should_convert_zero_based_tree_sitter_rows() {
    let span = Span::from_zero_based_rows(0, 9, 0, 0, 0, 9);
    assert_eq!(span.start(), Position { line: 1, col: 0 });
    assert_eq!(span.end(), Position { line: 1, col: 9 });
    assert!(span.is_well_formed());
  }

  #[test]
  fn should_saturate_rows_at_the_u32_limit() {
    let span = Span::from_zero_based_rows(0, 0, u32::MAX, 0, u32::MAX, 0);
    assert_eq!((span.start_line, span.end_line), (u32::MAX, u32::MAX));
  }

  #[test]
  fn should_span_multiple_lines_and_the_end_of_file() {
    let source = "a\nb\nc";
    let span = Span::of_text(source, 0, source.len()).unwrap();
    assert_eq!((span.start_line, span.end_line, span.end_col), (1, 3, 1));
    assert_eq!(span.line_count(), 3);
    let empty = Span::of_text(source, 5, 5).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.start(), Position { line: 3, col: 1 });
  }

  #[test]
  fn should_reject_out_of_range_offsets() {
    assert_eq!(Span::of_text("abc", 2, 1), None);
    assert_eq!(Span::of_text("abc", 0, 4), None);
  }

  #[test]
  fn should_test_containment_by_byte_range() {
    let outer = Span::of_text("0123456789", 1, 8).unwrap();
    let inner = Span::of_text("0123456789", 2, 5).unwrap();
    assert!(outer.contains(&inner));
    assert!(!inner.contains(&outer));
    assert!(outer.contains_byte(1));
    assert!(!outer.contains_byte(8));
  }

  #[test]
  fn should_detect_malformed_spans() {
    let reversed = Span {
      start_byte: 5,
      end_byte: 1,
      start_line: 1,
      start_col: 0,
      end_line: 1,
      end_col: 0,
    };
    assert!(!reversed.is_well_formed());
    let zero_line = Span {
      start_byte: 0,
      end_byte: 1,
      start_line: 0,
      start_col: 0,
      end_line: 1,
      end_col: 1,
    };
    assert!(!zero_line.is_well_formed());
  }
}
