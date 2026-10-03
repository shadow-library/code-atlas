use tree_sitter::Parser;

#[test]
fn should_parse_rust_with_the_pinned_tree_sitter_core() {
  let mut parser = Parser::new();
  parser.set_language(&tree_sitter_rust::LANGUAGE.into()).expect("rust grammar must match tree-sitter ABI");
  let tree = parser.parse("fn main() {}", None).expect("parse must produce a tree");
  let root = tree.root_node();
  assert_eq!(root.kind(), "source_file");
  assert!(!root.has_error());
  assert_eq!(root.child(0).map(|node| node.kind()), Some("function_item"));
}
