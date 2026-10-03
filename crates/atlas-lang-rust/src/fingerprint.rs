use atlas_core::{BodyFingerprintInput, MAX_BODY_TOKENS, NormToken, token_code};
use tree_sitter::Node;

use crate::syntax::{Walk, is_comment, walk_tree};

pub(crate) fn body_fingerprint(body: Node<'_>, line_count: u32) -> BodyFingerprintInput {
  let mut normalized_tokens = Vec::new();
  let mut token_count: u32 = 0;
  walk_tree(body, |node| {
    if is_comment(node) || node.is_missing() {
      return Walk::Skip;
    }
    let token = match node.kind() {
      "string_literal" | "raw_string_literal" | "char_literal" => NormToken::StringLiteral,
      "integer_literal" | "float_literal" => NormToken::NumberLiteral,
      _ if node.child_count() > 0 => return Walk::Descend,
      kind => leaf_token(node, kind),
    };
    token_count = token_count.saturating_add(1);
    if normalized_tokens.len() < MAX_BODY_TOKENS {
      normalized_tokens.push(token_code(token));
    }
    Walk::Skip
  });
  BodyFingerprintInput {
    normalized_tokens,
    token_count,
    line_count,
  }
}

fn leaf_token<'k>(node: Node<'_>, kind: &'k str) -> NormToken<'k> {
  if node.is_named() {
    return match kind {
      "self" | "crate" | "super" => NormToken::Keyword(kind),
      "mutable_specifier" => NormToken::Keyword("mut"),
      _ => NormToken::Identifier,
    };
  }
  if kind.starts_with(|first: char| first.is_ascii_alphabetic()) {
    return NormToken::Keyword(kind);
  }
  NormToken::Punctuation(kind)
}
