use scip::types::{Occurrence, PositionEncoding, occurrence::Typed_range};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColumnEncoding {
  Utf8,
  Utf16,
  Utf32,
}

impl ColumnEncoding {
  pub(crate) fn of_document(declared: PositionEncoding, tool: &str) -> Self {
    match declared {
      PositionEncoding::UTF8CodeUnitOffsetFromLineStart => Self::Utf8,
      PositionEncoding::UTF16CodeUnitOffsetFromLineStart => Self::Utf16,
      PositionEncoding::UTF32CodeUnitOffsetFromLineStart => Self::Utf32,
      PositionEncoding::UnspecifiedPositionEncoding if tool == "rust-analyzer" => Self::Utf8,
      PositionEncoding::UnspecifiedPositionEncoding => Self::Utf16,
    }
  }

  pub(crate) fn is_declared(declared: PositionEncoding) -> bool {
    declared != PositionEncoding::UnspecifiedPositionEncoding
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScipRange {
  pub start_line: u32,
  pub start_col: u32,
  pub end_line: u32,
  pub end_col: u32,
}

impl ScipRange {
  pub(crate) fn of(occurrence: &Occurrence) -> Option<Self> {
    let parts: [i32; 4] = match occurrence.range.as_slice() {
      [line, start, end] => [*line, *start, *line, *end],
      [start_line, start, end_line, end] => [*start_line, *start, *end_line, *end],
      [] => return Self::of_typed(occurrence),
      _ => return None,
    };
    Self::from_parts(parts)
  }

  fn of_typed(occurrence: &Occurrence) -> Option<Self> {
    match occurrence.typed_range.as_ref()? {
      Typed_range::SingleLineRange(range) => Self::from_parts([range.line, range.start_character, range.line, range.end_character]),
      Typed_range::MultiLineRange(range) => Self::from_parts([range.start_line, range.start_character, range.end_line, range.end_character]),
      _ => None,
    }
  }

  fn from_parts(parts: [i32; 4]) -> Option<Self> {
    let [start_line, start_col, end_line, end_col] = parts.map(|part| u32::try_from(part).ok());
    let range = Self {
      start_line: start_line?,
      start_col: start_col?,
      end_line: end_line?,
      end_col: end_col?,
    };
    ((range.start_line, range.start_col) <= (range.end_line, range.end_col)).then_some(range)
  }
}

pub(crate) struct LineIndex<'a> {
  source: &'a str,
  line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
  pub(crate) fn new(source: &'a str) -> Self {
    let line_starts = std::iter::once(0).chain(source.match_indices('\n').map(|(index, _)| index + 1)).collect();
    Self { source, line_starts }
  }

  pub(crate) fn byte_range(&self, range: ScipRange, encoding: ColumnEncoding) -> Option<(usize, usize)> {
    let start = self.byte_offset(range.start_line, range.start_col, encoding)?;
    let end = self.byte_offset(range.end_line, range.end_col, encoding)?;
    (start <= end).then_some((start, end))
  }

  fn byte_offset(&self, line: u32, column: u32, encoding: ColumnEncoding) -> Option<usize> {
    let line = usize::try_from(line).ok()?;
    let start = *self.line_starts.get(line)?;
    let end = self.line_starts.get(line + 1).map_or(self.source.len(), |next| next - 1);
    let text = self.source.get(start..end)?;
    let column = usize::try_from(column).ok()?;
    let within_line = match encoding {
      ColumnEncoding::Utf8 => text.is_char_boundary(column).then_some(column)?,
      ColumnEncoding::Utf16 => offset_by_units(text, column, char::len_utf16)?,
      ColumnEncoding::Utf32 => offset_by_units(text, column, |_| 1)?,
    };
    Some(start + within_line)
  }
}

fn offset_by_units(text: &str, target: usize, width: impl Fn(char) -> usize) -> Option<usize> {
  let mut units = 0;
  for (offset, character) in text.char_indices() {
    if units == target {
      return Some(offset);
    }
    if units > target {
      return None;
    }
    units += width(character);
  }
  (units == target).then_some(text.len())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn range(line: u32, start: u32, end: u32) -> ScipRange {
    ScipRange {
      start_line: line,
      start_col: start,
      end_line: line,
      end_col: end,
    }
  }

  #[test]
  fn should_convert_utf16_columns_past_a_non_bmp_character() {
    let source = "a\nlet s = \"😀é\"; foo();\n";
    let index = LineIndex::new(source);
    let utf16_foo = range(1, 15, 18);
    let (start, end) = index.byte_range(utf16_foo, ColumnEncoding::Utf16).unwrap();
    assert_eq!(&source[start..end], "foo");
    let (wrong_start, wrong_end) = index.byte_range(utf16_foo, ColumnEncoding::Utf8).unwrap();
    assert_ne!(&source[wrong_start..wrong_end], "foo");
  }

  #[test]
  fn should_convert_utf32_columns() {
    let source = "x = '😀'; bar\n";
    let index = LineIndex::new(source);
    let (start, end) = index.byte_range(range(0, 9, 12), ColumnEncoding::Utf32).unwrap();
    assert_eq!(&source[start..end], "bar");
  }

  #[test]
  fn should_reject_columns_inside_a_surrogate_pair() {
    let index = LineIndex::new("😀x");
    assert_eq!(index.byte_range(range(0, 1, 2), ColumnEncoding::Utf16), None);
  }

  #[test]
  fn should_reject_columns_past_the_end_of_the_line() {
    let index = LineIndex::new("ab\ncd\n");
    assert_eq!(index.byte_range(range(0, 0, 3), ColumnEncoding::Utf8), None);
    assert_eq!(index.byte_range(range(0, 0, 2), ColumnEncoding::Utf8), Some((0, 2)));
    assert_eq!(index.byte_range(range(5, 0, 1), ColumnEncoding::Utf8), None);
  }

  #[test]
  fn should_convert_multi_line_ranges() {
    let source = "ab\ncd\nef";
    let index = LineIndex::new(source);
    let multi = ScipRange {
      start_line: 0,
      start_col: 1,
      end_line: 2,
      end_col: 1,
    };
    assert_eq!(index.byte_range(multi, ColumnEncoding::Utf8), Some((1, 7)));
  }

  #[test]
  fn should_parse_three_and_four_element_ranges_and_reject_malformed_ones() {
    let mut occurrence = Occurrence::new();
    occurrence.range = vec![3, 4, 9];
    assert_eq!(ScipRange::of(&occurrence), Some(range(3, 4, 9)));
    occurrence.range = vec![3, 4, 5, 2];
    assert_eq!(
      ScipRange::of(&occurrence),
      Some(ScipRange {
        start_line: 3,
        start_col: 4,
        end_line: 5,
        end_col: 2
      })
    );
    occurrence.range = vec![3, 9, 4];
    assert_eq!(ScipRange::of(&occurrence), None);
    occurrence.range = vec![-1, 0, 1];
    assert_eq!(ScipRange::of(&occurrence), None);
    occurrence.range = vec![1, 2];
    assert_eq!(ScipRange::of(&occurrence), None);
  }
}
