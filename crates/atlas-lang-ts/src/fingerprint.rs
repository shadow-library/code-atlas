use atlas_core::{BodyFingerprintInput, MAX_BODY_TOKENS, NormToken, token_code};
use tree_sitter::Node;

use crate::syntax::span_of;

enum Visit<'a> {
  Skip,
  Token(NormToken<'a>),
  Descend,
}

pub(crate) fn fingerprint(body: Node<'_>) -> BodyFingerprintInput {
  let mut tokens = Vec::new();
  let mut count: u32 = 0;
  let mut cursor = body.walk();
  loop {
    let node = cursor.node();
    let descend = match classify(node) {
      Visit::Skip => false,
      Visit::Token(token) => {
        count = count.saturating_add(1);
        if tokens.len() < MAX_BODY_TOKENS {
          tokens.push(token_code(token));
        }
        false
      }
      Visit::Descend => true,
    };
    if descend && cursor.goto_first_child() {
      continue;
    }
    loop {
      if cursor.goto_next_sibling() {
        break;
      }
      if !cursor.goto_parent() {
        return BodyFingerprintInput {
          normalized_tokens: tokens,
          token_count: count,
          line_count: span_of(body).line_count(),
        };
      }
    }
  }
}

fn classify(node: Node<'_>) -> Visit<'_> {
  if node.is_missing() {
    return Visit::Skip;
  }
  let kind = node.kind();
  match kind {
    "comment" => Visit::Skip,
    "string" | "regex" | "string_fragment" | "escape_sequence" | "jsx_text" => Visit::Token(NormToken::StringLiteral),
    "number" => Visit::Token(NormToken::NumberLiteral),
    "this" | "super" | "true" | "false" | "null" | "undefined" => Visit::Token(NormToken::Keyword(kind)),
    _ if node.child_count() > 0 => Visit::Descend,
    _ if node.is_named() => Visit::Token(NormToken::Identifier),
    _ if kind.chars().all(|c| c.is_ascii_alphabetic()) => Visit::Token(NormToken::Keyword(kind)),
    _ => Visit::Token(NormToken::Punctuation(kind)),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn body_of(source: &str) -> BodyFingerprintInput {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let tree = parser.parse(source, None).unwrap();
    let function = tree.root_node().child(0).unwrap();
    fingerprint(function.child_by_field_name("body").unwrap())
  }

  #[test]
  fn should_abstract_identifiers_and_literals() {
    let a = body_of("function a(x: number) { return x + 1; }");
    let b = body_of("function b(y: number) {\n  return y + 2;\n}");
    assert_eq!(a.normalized_tokens, b.normalized_tokens);
    assert_eq!((a.token_count, a.line_count, b.line_count), (7, 1, 3));
    assert_eq!(
      a.normalized_tokens,
      [
        token_code(NormToken::Punctuation("{")),
        token_code(NormToken::Keyword("return")),
        token_code(NormToken::Identifier),
        token_code(NormToken::Punctuation("+")),
        token_code(NormToken::NumberLiteral),
        token_code(NormToken::Punctuation(";")),
        token_code(NormToken::Punctuation("}")),
      ]
    );
  }

  #[test]
  fn should_distinguish_operators_with_the_same_shape() {
    let modulo = body_of("function a(n: number) { return n % 2 === 0; }");
    let bitwise = body_of("function b(n: number) { return n & 1 === 0; }");
    assert_ne!(modulo.normalized_tokens, bitwise.normalized_tokens);
  }
}
