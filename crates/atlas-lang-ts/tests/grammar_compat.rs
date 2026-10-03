use tree_sitter::{Language, Parser};

fn parse_first_statement(language: Language, source: &str) -> String {
  let mut parser = Parser::new();
  parser.set_language(&language).expect("grammar must match tree-sitter ABI");
  let tree = parser.parse(source, None).expect("parse must produce a tree");
  let root = tree.root_node();
  assert!(!root.has_error());
  root.child(0).map(|node| node.kind().to_owned()).unwrap_or_default()
}

#[test]
fn should_parse_typescript_with_the_pinned_tree_sitter_core() {
  assert_eq!(
    parse_first_statement(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), "const x = 1;"),
    "lexical_declaration"
  );
}

#[test]
fn should_parse_tsx_with_the_pinned_tree_sitter_core() {
  assert_eq!(
    parse_first_statement(tree_sitter_typescript::LANGUAGE_TSX.into(), "const x = <div />;"),
    "lexical_declaration"
  );
}

#[test]
fn should_parse_javascript_with_the_pinned_tree_sitter_core() {
  assert_eq!(parse_first_statement(tree_sitter_javascript::LANGUAGE.into(), "const x = 1;"), "lexical_declaration");
}
