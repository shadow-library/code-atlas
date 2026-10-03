use std::ops::Range;

use tree_sitter::Node;

use crate::syntax::{children, field_children, text};

const MAX_SIGNATURE_LEN: usize = 256;

pub(crate) fn signature(source: &str, range: Range<usize>, excluded: &[Range<usize>]) -> Option<String> {
  let mut kept = String::new();
  let mut cursor = range.start;
  for skip in excluded.iter().filter(|skip| skip.start >= range.start && skip.end <= range.end) {
    if skip.start > cursor {
      kept.push_str(source.get(cursor..skip.start)?);
    }
    cursor = cursor.max(skip.end);
    while source.as_bytes().get(cursor).is_some_and(u8::is_ascii_whitespace) && cursor < range.end {
      cursor += 1;
    }
  }
  if cursor < range.end {
    kept.push_str(source.get(cursor..range.end)?);
  }
  normalize(&kept)
}

pub(crate) fn normalize(raw: &str) -> Option<String> {
  let collapsed = raw
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ")
    .replace("( ", "(")
    .replace(", )", ")")
    .replace(" )", ")");
  let trimmed = collapsed.trim_end_matches([';', ',', ' ']);
  if trimmed.is_empty() {
    return None;
  }
  Some(limit(trimmed))
}

fn limit(value: &str) -> String {
  if value.len() <= MAX_SIGNATURE_LEN {
    return value.to_owned();
  }
  let mut end = MAX_SIGNATURE_LEN - '…'.len_utf8();
  while !value.is_char_boundary(end) {
    end -= 1;
  }
  format!("{}…", &value[..end])
}

pub(crate) fn decorator_ranges(nodes: &[Node<'_>]) -> Vec<Range<usize>> {
  let mut ranges = Vec::new();
  for node in nodes {
    ranges.extend(field_children(*node, "decorator").iter().map(Node::byte_range));
    if let Some(parameters) = node.child_by_field_name("parameters") {
      for parameter in children(parameters) {
        ranges.extend(field_children(parameter, "decorator").iter().map(Node::byte_range));
      }
    }
  }
  ranges.sort_by_key(|range| range.start);
  ranges
}

pub(crate) fn doc_comment(first: Node<'_>, source: &str) -> Option<String> {
  let comment = first.prev_sibling()?;
  if comment.kind() != "comment" || first.start_position().row > comment.end_position().row + 1 {
    return None;
  }
  let raw = text(comment, source);
  let body = raw.strip_prefix("/**")?.strip_suffix("*/")?;
  let lines: Vec<&str> = body
    .lines()
    .map(|line| {
      let line = line.trim();
      let line = line.strip_prefix('*').unwrap_or(line);
      line.strip_prefix(' ').unwrap_or(line).trim_end()
    })
    .collect();
  let start = lines.iter().position(|line| !line.is_empty())?;
  let end = lines.iter().rposition(|line| !line.is_empty())?;
  Some(lines[start..=end].join("\n"))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_drop_excluded_ranges_and_collapse_whitespace() {
    let source = "findOne(@Param('id')  id: string,\n  other: number);";
    let start = source.find('@').unwrap();
    let decorator = vec![Range {
      start,
      end: start + "@Param('id')".len(),
    }];
    assert_eq!(signature(source, 0..source.len(), &decorator).as_deref(), Some("findOne(id: string, other: number)"));
  }

  #[test]
  fn should_truncate_long_signatures_with_an_ellipsis() {
    let source = "x".repeat(400);
    let signature = signature(&source, 0..source.len(), &[]).unwrap();
    assert!(signature.len() <= MAX_SIGNATURE_LEN);
    assert!(signature.ends_with('…'));
  }
}
