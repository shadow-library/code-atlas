use atlas_core::{ARG_MAX_DEPTH, ArgSummary};
use tree_sitter::Node;

use crate::literals::{decode_raw_string_literal, decode_string_literal};
use crate::source::{Doc, short_text};
use crate::summary::capped;
use crate::syntax::children;

pub(crate) fn token_tree_args(doc: Doc<'_>, tree: Node<'_>, depth: usize) -> Vec<ArgSummary> {
  if depth >= ARG_MAX_DEPTH {
    return vec![ArgSummary::Truncated];
  }
  let groups = split_top_level(&inner_tokens(tree));
  capped(&groups, |group| summarize_group(doc, group, depth))
}

pub(crate) fn inner_tokens(tree: Node<'_>) -> Vec<Node<'_>> {
  let mut tokens = children(tree);
  if tokens.first().is_some_and(|first| is_open_delimiter(first.kind())) {
    tokens.remove(0);
  }
  if tokens.last().is_some_and(|last| is_close_delimiter(last.kind()) && !last.is_missing()) {
    tokens.pop();
  }
  tokens
}

fn is_open_delimiter(kind: &str) -> bool {
  matches!(kind, "(" | "[" | "{")
}

fn is_close_delimiter(kind: &str) -> bool {
  matches!(kind, ")" | "]" | "}")
}

pub(crate) fn split_top_level<'t>(tokens: &[Node<'t>]) -> Vec<Vec<Node<'t>>> {
  tokens
    .split(|token| token.kind() == ",")
    .filter(|group| !group.is_empty())
    .map(<[Node<'t>]>::to_vec)
    .collect()
}

fn summarize_group(doc: Doc<'_>, group: &[Node<'_>], depth: usize) -> ArgSummary {
  if depth >= ARG_MAX_DEPTH {
    return ArgSummary::Truncated;
  }
  if let [name, equals, value @ ..] = group
    && equals.kind() == "="
    && !value.is_empty()
    && let Some(name) = word(doc, *name)
  {
    return ArgSummary::Named {
      name: name.to_owned(),
      value: Box::new(summarize_group(doc, value, depth + 1)),
    };
  }
  if let [single] = group
    && let Some(literal) = literal(doc, *single)
  {
    return literal;
  }
  if let [minus, number] = group
    && minus.kind() == "-"
    && matches!(number.kind(), "integer_literal" | "float_literal")
  {
    return ArgSummary::Number(format!("-{}", doc.text(*number)));
  }
  if let [callee @ .., arguments] = group
    && arguments.kind() == "token_tree"
    && let Some(callee) = token_path(doc, callee)
  {
    return ArgSummary::Call {
      callee,
      args: token_tree_args(doc, *arguments, depth + 1),
    };
  }
  if let Some(path) = token_path(doc, group) {
    return ArgSummary::Path(path);
  }
  match (group.first(), group.last()) {
    (Some(first), Some(last)) => ArgSummary::Other(short_text(doc.slice(first.start_byte(), last.end_byte()))),
    _ => ArgSummary::Truncated,
  }
}

fn literal(doc: Doc<'_>, token: Node<'_>) -> Option<ArgSummary> {
  let text = doc.text(token);
  match token.kind() {
    "string_literal" => Some(ArgSummary::Str(decode_string_literal(text))),
    "raw_string_literal" => Some(ArgSummary::Str(decode_raw_string_literal(text))),
    "integer_literal" | "float_literal" => Some(ArgSummary::Number(text.to_owned())),
    "boolean_literal" | "identifier" if matches!(text, "true" | "false") => Some(ArgSummary::Bool(text == "true")),
    _ => None,
  }
}

fn token_path(doc: Doc<'_>, tokens: &[Node<'_>]) -> Option<Vec<String>> {
  let tokens = match tokens.first() {
    Some(first) if first.kind() == "::" => &tokens[1..],
    _ => tokens,
  };
  if tokens.is_empty() || tokens.len() % 2 == 0 {
    return None;
  }
  let mut segments = Vec::with_capacity(tokens.len() / 2 + 1);
  for (index, token) in tokens.iter().enumerate() {
    let is_segment = index % 2 == 0;
    match (is_segment, token.kind()) {
      (true, _) => segments.push(word(doc, *token)?.to_owned()),
      (false, "::") => {}
      (false, _) => return None,
    }
  }
  Some(segments)
}

pub(crate) fn word<'t>(doc: Doc<'t>, token: Node<'_>) -> Option<&'t str> {
  if matches!(
    token.kind(),
    "string_literal" | "raw_string_literal" | "char_literal" | "integer_literal" | "float_literal" | "token_tree"
  ) {
    return None;
  }
  let text = doc.identifier(token);
  let mut chars = text.chars();
  let starts_like_identifier = chars.next().is_some_and(|first| first.is_alphabetic() || first == '_' || first == '$');
  (starts_like_identifier && chars.all(|next| next.is_alphanumeric() || next == '_')).then_some(text)
}
