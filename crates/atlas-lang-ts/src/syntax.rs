use atlas_core::{ARG_MAX_TEXT, Span, truncate_text};
use tree_sitter::Node;

pub(crate) fn span_of(node: Node<'_>) -> Span {
  span_between(node, node)
}

pub(crate) fn span_between(first: Node<'_>, last: Node<'_>) -> Span {
  let start = first.start_position();
  let end = last.end_position();
  Span::from_zero_based_rows(
    to_u32(first.start_byte()),
    to_u32(last.end_byte()),
    to_u32(start.row),
    to_u32(start.column),
    to_u32(end.row),
    to_u32(end.column),
  )
}

fn to_u32(value: usize) -> u32 {
  u32::try_from(value).unwrap_or(u32::MAX)
}

pub(crate) fn text<'s>(node: Node<'_>, source: &'s str) -> &'s str {
  source.get(node.byte_range()).unwrap_or_default()
}

pub(crate) fn compact_text(node: Node<'_>, source: &str) -> String {
  snippet(text(node, source))
}

pub(crate) fn snippet(raw: &str) -> String {
  let mut cut = raw.len().min(ARG_MAX_TEXT * 2);
  while !raw.is_char_boundary(cut) {
    cut -= 1;
  }
  let compacted = truncate_text(&raw[..cut]);
  if cut == raw.len() || compacted.ends_with('…') {
    return compacted;
  }
  let mut shortened = compacted;
  while shortened.len() + '…'.len_utf8() > ARG_MAX_TEXT {
    shortened.pop();
  }
  shortened.push('…');
  shortened
}

pub(crate) fn children<'t>(node: Node<'t>) -> Vec<Node<'t>> {
  let mut cursor = node.walk();
  node.named_children(&mut cursor).filter(|child| child.kind() != "comment").collect()
}

pub(crate) fn field_children<'t>(node: Node<'t>, field: &str) -> Vec<Node<'t>> {
  let mut cursor = node.walk();
  node.children_by_field_name(field, &mut cursor).collect()
}

pub(crate) fn has_token(node: Node<'_>, token: &str) -> bool {
  let mut cursor = node.walk();
  node.children(&mut cursor).any(|child| !child.is_named() && child.kind() == token)
}

pub(crate) fn token<'t>(node: Node<'t>, token: &str) -> Option<Node<'t>> {
  let mut cursor = node.walk();
  node.children(&mut cursor).find(|child| !child.is_named() && child.kind() == token)
}

pub(crate) fn first_child<'t>(node: Node<'t>) -> Option<Node<'t>> {
  children(node).into_iter().next()
}

pub(crate) fn unwrap_expression(mut node: Node<'_>) -> Node<'_> {
  while matches!(
    node.kind(),
    "parenthesized_expression" | "non_null_expression" | "satisfies_expression" | "as_expression" | "type_assertion"
  ) {
    let inner = match node.kind() {
      "type_assertion" => children(node).into_iter().last(),
      _ => first_child(node),
    };
    let Some(inner) = inner else { return node };
    if inner.kind() == "sequence_expression" {
      return node;
    }
    node = inner;
  }
  node
}

pub(crate) fn is_function_value(node: Node<'_>) -> bool {
  matches!(node.kind(), "arrow_function" | "function_expression" | "function" | "generator_function")
}

pub(crate) fn string_value(node: Node<'_>, source: &str) -> String {
  let mut value = String::new();
  for part in children(node) {
    match part.kind() {
      "escape_sequence" => value.push_str(&decode_escape(text(part, source))),
      _ => value.push_str(text(part, source)),
    }
  }
  value
}

pub(crate) fn template_raw(node: Node<'_>, source: &str) -> String {
  let raw = text(node, source);
  raw.strip_prefix('`').and_then(|inner| inner.strip_suffix('`')).unwrap_or(raw).to_owned()
}

pub(crate) fn decode_escape(raw: &str) -> String {
  let Some(body) = raw.strip_prefix('\\') else { return raw.to_owned() };
  let mut chars = body.chars();
  let Some(first) = chars.next() else { return String::new() };
  let rest = chars.as_str();
  let simple = match first {
    'n' => Some('\n'),
    't' => Some('\t'),
    'r' => Some('\r'),
    'b' => Some('\u{8}'),
    'f' => Some('\u{c}'),
    'v' => Some('\u{b}'),
    '0' if rest.is_empty() => Some('\0'),
    '\n' | '\r' | '\u{2028}' | '\u{2029}' => return String::new(),
    _ => None,
  };
  if let Some(decoded) = simple {
    return decoded.to_string();
  }
  let hex = match first {
    'x' => Some(rest),
    'u' => Some(rest.trim_start_matches('{').trim_end_matches('}')),
    _ => None,
  };
  match hex.and_then(|digits| u32::from_str_radix(digits, 16).ok()).and_then(char::from_u32) {
    Some(decoded) => decoded.to_string(),
    None if hex.is_some() => raw.to_owned(),
    None => body.to_owned(),
  }
}

pub(crate) fn property_name(node: Node<'_>, source: &str) -> String {
  match node.kind() {
    "string" => string_value(node, source),
    "computed_property_name" => compact_text(node, source),
    _ => text(node, source).to_owned(),
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChainHead<'t> {
  Identifier(Node<'t>),
  This,
  Super,
  Expr(Node<'t>),
}

#[derive(Debug, Clone)]
pub(crate) struct Chain<'t> {
  pub(crate) head: ChainHead<'t>,
  pub(crate) segments: Vec<String>,
}

impl Chain<'_> {
  pub(crate) fn static_path(&self, source: &str) -> Option<Vec<String>> {
    let head = match self.head {
      ChainHead::Identifier(node) => text(node, source).to_owned(),
      ChainHead::This => "this".to_owned(),
      ChainHead::Super | ChainHead::Expr(_) => return None,
    };
    Some(std::iter::once(head).chain(self.segments.iter().cloned()).collect())
  }
}

pub(crate) fn member_chain<'t>(node: Node<'t>, source: &str) -> Chain<'t> {
  let mut segments = Vec::new();
  let mut current = node;
  let head = loop {
    match current.kind() {
      "member_expression" => {
        let property = current.child_by_field_name("property");
        segments.push(property.map(|property| text(property, source).to_owned()).unwrap_or_default());
        match current.child_by_field_name("object") {
          Some(object) => current = object,
          None => break ChainHead::Expr(current),
        }
      }
      "subscript_expression" => {
        segments.push(subscript_segment(current.child_by_field_name("index"), source));
        match current.child_by_field_name("object") {
          Some(object) => current = object,
          None => break ChainHead::Expr(current),
        }
      }
      "non_null_expression" | "parenthesized_expression" => match first_child(current) {
        Some(inner) if inner.kind() != "sequence_expression" => current = inner,
        _ => break ChainHead::Expr(current),
      },
      "identifier" => break ChainHead::Identifier(current),
      "this" => break ChainHead::This,
      "super" => break ChainHead::Super,
      _ => break ChainHead::Expr(current),
    }
  };
  segments.reverse();
  if segments.iter().any(String::is_empty) {
    return Chain {
      head: ChainHead::Expr(node),
      segments: Vec::new(),
    };
  }
  Chain { head, segments }
}

fn subscript_segment(index: Option<Node<'_>>, source: &str) -> String {
  let Some(index) = index else { return String::new() };
  match index.kind() {
    "string" => string_value(index, source),
    "number" => text(index, source).to_owned(),
    _ => format!("[{}]", compact_text(index, source)),
  }
}

pub(crate) fn type_path(node: Node<'_>, source: &str) -> Option<Vec<String>> {
  let node = match node.kind() {
    "type_annotation" => first_child(node)?,
    _ => node,
  };
  match node.kind() {
    "type_identifier" | "identifier" => Some(vec![text(node, source).to_owned()]),
    "nested_type_identifier" | "nested_identifier" | "member_expression" => {
      let segments: Vec<String> = text(node, source).split('.').map(|segment| segment.trim().to_owned()).collect();
      (!segments.iter().any(String::is_empty)).then_some(segments)
    }
    "generic_type" => type_path(node.child_by_field_name("name")?, source),
    _ => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_bound_snippets_without_scanning_the_whole_text() {
    assert_eq!(snippet("a   b"), "a b");
    let spaced = format!("{}tail", " ".repeat(10_000));
    assert_eq!(snippet(&spaced), "…");
    let long = "word ".repeat(1_000);
    let compacted = snippet(&long);
    assert!(compacted.len() <= ARG_MAX_TEXT && compacted.ends_with('…'));
  }

  #[test]
  fn should_decode_javascript_escape_sequences() {
    assert_eq!(decode_escape("\\n"), "\n");
    assert_eq!(decode_escape("\\'"), "'");
    assert_eq!(decode_escape("\\x41"), "A");
    assert_eq!(decode_escape("\\u0041"), "A");
    assert_eq!(decode_escape("\\u{1F600}"), "😀");
    assert_eq!(decode_escape("\\u{zz}"), "\\u{zz}");
    assert_eq!(decode_escape("\\\n"), "");
    assert_eq!(decode_escape("\\q"), "q");
  }
}
