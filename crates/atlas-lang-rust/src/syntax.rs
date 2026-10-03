use atlas_core::{Modifier, Modifiers, TypePath, Visibility};
use tree_sitter::Node;

use crate::source::{Doc, clip, normalize_whitespace, signature_text};

const MAX_PATH_SEGMENTS: usize = 64;
const MAX_GENERIC_ARGS_BYTES: usize = 256;

pub(crate) fn named_children<'t>(node: Node<'t>) -> Vec<Node<'t>> {
  let mut cursor = node.walk();
  node.named_children(&mut cursor).collect()
}

pub(crate) fn children<'t>(node: Node<'t>) -> Vec<Node<'t>> {
  let mut cursor = node.walk();
  node.children(&mut cursor).collect()
}

pub(crate) fn is_comment(node: Node<'_>) -> bool {
  matches!(node.kind(), "line_comment" | "block_comment")
}

pub(crate) fn name_of<'t>(doc: Doc<'t>, node: Node<'t>) -> Option<(String, Node<'t>)> {
  let name = node.child_by_field_name("name")?;
  let text = doc.identifier(name);
  if text.is_empty() || text == "_" {
    return None;
  }
  Some((text.to_owned(), name))
}

pub(crate) fn path_segments(doc: Doc<'_>, node: Node<'_>) -> Option<Vec<String>> {
  let mut segments = Vec::new();
  push_path(doc, node, &mut segments, 0)?;
  if segments.is_empty() {
    return None;
  }
  Some(segments)
}

fn push_path(doc: Doc<'_>, node: Node<'_>, segments: &mut Vec<String>, depth: usize) -> Option<()> {
  if segments.len() >= MAX_PATH_SEGMENTS || depth >= MAX_PATH_SEGMENTS {
    return None;
  }
  match node.kind() {
    "identifier" | "type_identifier" | "field_identifier" | "shorthand_field_identifier" | "primitive_type" | "self" | "crate" | "super" | "metavariable" => {
      let text = doc.identifier(node);
      if text.is_empty() {
        return None;
      }
      segments.push(text.to_owned());
    }
    "scoped_identifier" | "scoped_type_identifier" => {
      if let Some(path) = node.child_by_field_name("path") {
        push_path(doc, path, segments, depth + 1)?;
      }
      push_path(doc, node.child_by_field_name("name")?, segments, depth + 1)?;
    }
    "generic_type" | "generic_type_with_turbofish" => push_path(doc, node.child_by_field_name("type")?, segments, depth + 1)?,
    "generic_function" => push_path(doc, node.child_by_field_name("function")?, segments, depth + 1)?,
    "bracketed_type" => {
      let inner = node.named_child(0)?;
      if inner.kind() != "qualified_type" {
        return push_type_segment(doc, inner, segments, depth + 1);
      }
      push_type_segment(doc, inner.child_by_field_name("type")?, segments, depth + 1)?;
      let alias = inner.child_by_field_name("alias")?;
      let mut trait_path = Vec::new();
      push_path(doc, alias, &mut trait_path, depth + 1)?;
      let generic_args = match alias.kind() {
        "generic_type" => alias.child_by_field_name("type_arguments").map(|arguments| generic_args_text(doc, arguments)),
        _ => None,
      };
      segments.push(trait_segment(&TypePath {
        segments: trait_path,
        generic_args,
      }));
    }
    _ => return None,
  }
  Some(())
}

fn push_type_segment(doc: Doc<'_>, node: Node<'_>, segments: &mut Vec<String>, depth: usize) -> Option<()> {
  if push_path(doc, node, segments, depth).is_some() {
    return Some(());
  }
  let text = signature_text(doc.text(node));
  if text.is_empty() {
    return None;
  }
  segments.push(text);
  Some(())
}

pub(crate) fn trait_segment(trait_ref: &TypePath) -> String {
  let name = trait_ref.name().unwrap_or("_");
  match &trait_ref.generic_args {
    Some(args) => format!("<{name}<{args}>>"),
    None => format!("<{name}>"),
  }
}

pub(crate) fn type_path(doc: Doc<'_>, node: Node<'_>) -> TypePath {
  let generic_args = match node.kind() {
    "generic_type" => node.child_by_field_name("type_arguments").map(|arguments| generic_args_text(doc, arguments)),
    _ => None,
  };
  match path_segments(doc, node) {
    Some(segments) => TypePath { segments, generic_args },
    None => {
      let text = signature_text(doc.text(node));
      TypePath::new(vec![if text.is_empty() { "_".to_owned() } else { text }])
    }
  }
}

fn generic_args_text(doc: Doc<'_>, arguments: Node<'_>) -> String {
  let text = doc.text(arguments);
  let inner = text.strip_prefix('<').unwrap_or(text);
  let inner = inner.strip_suffix('>').unwrap_or(inner);
  normalize_whitespace(clip(inner, MAX_GENERIC_ARGS_BYTES))
}

pub(crate) fn visibility(doc: Doc<'_>, item: Node<'_>) -> Visibility {
  let Some(modifier) = named_children(item).into_iter().find(|child| child.kind() == "visibility_modifier") else {
    return Visibility::Private;
  };
  let Some(scope) = modifier.named_child(0) else { return Visibility::Public };
  match normalize_whitespace(doc.text(scope)).as_str() {
    "crate" => Visibility::Crate,
    "self" => Visibility::Private,
    path => Visibility::Restricted(path.to_owned()),
  }
}

pub(crate) fn item_modifiers(item: Node<'_>) -> Modifiers {
  let mut modifiers = Modifiers::empty();
  for child in children(item) {
    match child.kind() {
      "function_modifiers" => {
        for modifier in children(child) {
          if let Some(modifier) = keyword_modifier(modifier.kind()) {
            modifiers.insert(modifier);
          }
        }
      }
      "unsafe" => modifiers.insert(Modifier::Unsafe),
      "mutable_specifier" => modifiers.insert(Modifier::Mutable),
      _ => {}
    }
  }
  if item.child_by_field_name("type_parameters").is_some() {
    modifiers.insert(Modifier::Generic);
  }
  modifiers
}

fn keyword_modifier(kind: &str) -> Option<Modifier> {
  match kind {
    "async" => Some(Modifier::Async),
    "const" => Some(Modifier::Const),
    "unsafe" => Some(Modifier::Unsafe),
    "extern_modifier" => Some(Modifier::Extern),
    "default" => Some(Modifier::Default),
    _ => None,
  }
}

pub(crate) struct Decorated<'t> {
  pub(crate) node: Node<'t>,
  pub(crate) attributes: Vec<Node<'t>>,
  pub(crate) docs: Vec<Node<'t>>,
  pub(crate) start: usize,
}

pub(crate) fn decorated_children(container: Node<'_>) -> Vec<Decorated<'_>> {
  let mut items = Vec::new();
  let mut attributes = Vec::new();
  let mut docs = Vec::new();
  let mut start = None;
  for child in children(container) {
    match child.kind() {
      "attribute_item" => {
        start.get_or_insert(child.start_byte());
        attributes.push(child);
      }
      "line_comment" | "block_comment" => {
        if is_outer_doc(child) {
          start.get_or_insert(child.start_byte());
          docs.push(child);
        }
      }
      _ if !child.is_named() => {
        attributes.clear();
        docs.clear();
        start = None;
      }
      _ => items.push(Decorated {
        node: child,
        attributes: std::mem::take(&mut attributes),
        docs: std::mem::take(&mut docs),
        start: start.take().unwrap_or(child.start_byte()),
      }),
    }
  }
  items
}

fn is_outer_doc(comment: Node<'_>) -> bool {
  comment.child_by_field_name("outer").is_some() && comment.child_by_field_name("doc").is_some()
}

fn is_inner_doc(comment: Node<'_>) -> bool {
  comment.child_by_field_name("inner").is_some() && comment.child_by_field_name("doc").is_some()
}

pub(crate) fn inner_docs(container: Node<'_>) -> Vec<Node<'_>> {
  children(container).into_iter().filter(|child| is_comment(*child) && is_inner_doc(*child)).collect()
}

pub(crate) fn doc_lines(doc: Doc<'_>, comment: Node<'_>) -> Vec<String> {
  let Some(content) = comment.child_by_field_name("doc") else { return Vec::new() };
  let text = doc.text(content);
  if comment.kind() == "line_comment" {
    let line = text.trim_end_matches(['\n', '\r']);
    return vec![line.strip_prefix(' ').unwrap_or(line).to_owned()];
  }
  let lines: Vec<String> = text
    .lines()
    .map(|line| {
      let line = line.trim();
      line.strip_prefix("* ").or_else(|| line.strip_prefix('*')).unwrap_or(line).to_owned()
    })
    .collect();
  let first = lines.iter().position(|line| !line.is_empty()).unwrap_or(lines.len());
  let last = lines.iter().rposition(|line| !line.is_empty()).map_or(first, |index| index + 1);
  lines[first..last.max(first)].to_vec()
}

pub(crate) fn join_doc(lines: Vec<String>) -> Option<String> {
  let joined = lines.join("\n");
  let trimmed = joined.trim();
  if trimmed.is_empty() {
    return None;
  }
  Some(trimmed.to_owned())
}

pub(crate) fn signature(doc: Doc<'_>, item: Node<'_>, end: usize) -> String {
  let start = item.start_byte();
  let end = end.clamp(start, item.end_byte());
  let mut text = String::new();
  let mut cursor = start;
  for comment in comments_within(item, end) {
    text.push_str(doc.slice(cursor, comment.0));
    text.push(' ');
    cursor = comment.1;
  }
  text.push_str(doc.slice(cursor.min(end), end));
  signature_text(&text)
}

fn comments_within(item: Node<'_>, end: usize) -> Vec<(usize, usize)> {
  let mut ranges = Vec::new();
  walk_tree(item, |node| {
    if node.start_byte() >= end {
      return Walk::Skip;
    }
    if is_comment(node) && node.id() != item.id() {
      ranges.push((node.start_byte(), node.end_byte().min(end)));
      return Walk::Skip;
    }
    Walk::Descend
  });
  ranges
}

pub(crate) enum Walk {
  Descend,
  Skip,
}

pub(crate) fn walk_tree<'t>(root: Node<'t>, mut visit: impl FnMut(Node<'t>) -> Walk) {
  let mut cursor = root.walk();
  'nodes: loop {
    if matches!(visit(cursor.node()), Walk::Descend) && cursor.goto_first_child() {
      continue;
    }
    loop {
      if cursor.goto_next_sibling() {
        continue 'nodes;
      }
      if !cursor.goto_parent() {
        return;
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use atlas_core::ImplInfo;

  use super::*;

  #[test]
  fn should_render_trait_segments_exactly_like_core_impl_prefixes() {
    let generic = TypePath {
      segments: vec!["From".into()],
      generic_args: Some("io::Error".into()),
    };
    let plain = TypePath::new(vec!["fmt".into(), "Display".into()]);
    for trait_ref in [generic, plain] {
      let info = ImplInfo {
        self_type: TypePath::new(vec!["T".into()]),
        trait_ref: Some(trait_ref.clone()),
      };
      assert_eq!(info.item_prefix()[1], trait_segment(&trait_ref));
    }
  }
}
