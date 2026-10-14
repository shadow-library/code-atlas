use atlas_core::{ARG_MAX_DEPTH, ARG_MAX_WIDTH, ArgSummary, ImportFact, ObjectEntry, ObjectKey, ReceiverHint, RefKind, Span};
use tree_sitter::Node;

use super::{Extractor, ThisBinding};
use crate::args::{summarize, summarize_arguments};
use crate::modules::{module_specifier, require_fact};
use crate::syntax::{
  Chain, ChainHead, children, compact_text, field_children, first_child, has_token, member_chain, span_between, span_of, string_value, text, type_path, unwrap_expression,
};
use crate::testing::recognize;

impl<'s, 't> Extractor<'s, 't> {
  pub(super) fn call(&mut self, node: Node<'t>) {
    let arguments = node.child_by_field_name("arguments");
    let Some(mut callee) = node.child_by_field_name("function") else {
      return self.push_all(children(node));
    };
    let mut span = span_of(node);
    if node.child_by_field_name("type_arguments").is_some()
      && let Some(awaited) = awaited_callee(callee)
    {
      callee = awaited;
      span = span_between(callee, node);
    }
    match callee.kind() {
      "import" => {
        if !self.in_error {
          self.facts.imports.push(ImportFact::TsDynamicImport {
            specifier: module_specifier(node, self.source),
            scope: self.owner,
            span: span_of(node),
          });
        }
        return self.push_all(arguments);
      }
      "super" => return self.push_all(arguments),
      "identifier" if text(callee, self.source) == "require" && !self.scopes.is_local_value("require") => {
        if !self.in_error {
          self.facts.imports.push(require_fact(node, Vec::new(), self.owner, node, self.source));
        }
        return self.push_all(arguments);
      }
      _ => {}
    }
    if self.tests_enabled
      && let Some(test) = recognize(node, self.source)
      && self.is_test_head(test.head)
    {
      return self.test_symbol(node, test);
    }
    let args = summarize_arguments(arguments, self.source);
    self.mark_callee(callee);
    match arguments.filter(|arguments| arguments.kind() == "template_string") {
      Some(_) => self.tag_reference(callee, args, span),
      None => self.call_reference(callee, args, span),
    }
    if let Some(arguments) = arguments {
      self.value_refs(arguments);
    }
    let mut nodes = vec![callee];
    nodes.extend(node.child_by_field_name("type_arguments"));
    nodes.extend(arguments);
    self.push_all(nodes);
  }

  fn mark_callee(&mut self, callee: Node<'t>) {
    let mut current = callee;
    self.callees.insert(current.id());
    while matches!(current.kind(), "parenthesized_expression" | "non_null_expression")
      && let Some(inner) = first_child(current)
    {
      self.callees.insert(inner.id());
      current = inner;
    }
  }

  fn tag_reference(&mut self, tag: Node<'t>, args: Vec<ArgSummary>, span: Span) {
    let chain = member_chain(tag, self.source);
    match chain.head {
      ChainHead::Expr(root) if chain.segments.is_empty() && root.kind() == "call_expression" => {
        let path = vec![format!("({})", compact_text(root, self.source))];
        self.reference(RefKind::Call, path, None, args, span);
      }
      _ => self.call_reference(tag, args, span),
    }
  }

  fn call_reference(&mut self, callee: Node<'t>, args: Vec<ArgSummary>, span: Span) {
    let chain = member_chain(callee, self.source);
    match chain.head {
      ChainHead::Identifier(head) => {
        let name = text(head, self.source);
        if chain.segments.is_empty() {
          if !self.scopes.is_local_value(name) {
            self.reference(RefKind::Call, vec![name.to_owned()], None, args, span);
          }
          return;
        }
        if let Some(binding) = self.scopes.value(name) {
          let receiver = binding
            .type_hint()
            .map_or_else(|| ReceiverHint::Expr(name.to_owned()), |hint| ReceiverHint::TypeHint(hint.to_vec()));
          let path = std::iter::once(name.to_owned()).chain(chain.segments).collect();
          return self.reference(RefKind::MethodCall, path, Some(receiver), args, span);
        }
        let path = std::iter::once(name.to_owned()).chain(chain.segments).collect();
        if self.module.namespaces.iter().any(|namespace| namespace == name) {
          return self.reference(RefKind::Call, path, None, args, span);
        }
        self.reference(RefKind::MethodCall, path, Some(ReceiverHint::Identifier(name.to_owned())), args, span);
      }
      ChainHead::This | ChainHead::Super if !chain.segments.is_empty() => {
        let (head, receiver) = match chain.head {
          ChainHead::This => ("this", self.this_receiver()),
          _ => ("super", ReceiverHint::Expr("super".to_owned())),
        };
        let path = std::iter::once(head.to_owned()).chain(chain.segments).collect();
        self.reference(RefKind::MethodCall, path, Some(receiver), args, span);
      }
      ChainHead::Expr(root) if !chain.segments.is_empty() => {
        let receiver = self.expression_receiver(root);
        let path = std::iter::once(format!("({})", compact_text(root, self.source))).chain(chain.segments).collect();
        self.reference(RefKind::MethodCall, path, Some(receiver), args, span);
      }
      _ => {}
    }
  }

  fn this_receiver(&self) -> ReceiverHint {
    match self.this_binding {
      ThisBinding::EnclosingClass => ReceiverHint::This,
      ThisBinding::Rebound => ReceiverHint::Expr("this".to_owned()),
    }
  }

  fn expression_receiver(&self, root: Node<'_>) -> ReceiverHint {
    let asserted = match root.kind() {
      "as_expression" | "satisfies_expression" | "type_assertion" => {
        let parts = children(root);
        let annotated = if root.kind() == "type_assertion" { parts.first() } else { parts.last() };
        annotated.and_then(|annotated| type_path(*annotated, self.source))
      }
      _ => None,
    };
    asserted.map_or_else(|| ReceiverHint::Expr(compact_text(root, self.source)), ReceiverHint::TypeHint)
  }

  pub(super) fn instantiation(&mut self, node: Node<'t>) {
    let constructor = node.child_by_field_name("constructor");
    let arguments = node.child_by_field_name("arguments");
    if let Some(constructor) = constructor {
      self.mark_callee(constructor);
      let chain = member_chain(constructor, self.source);
      if let ChainHead::Identifier(head) = chain.head
        && !self.scopes.is_local_value(text(head, self.source))
        && let Some(path) = chain.static_path(self.source)
      {
        let args = summarize_arguments(arguments, self.source);
        self.reference(RefKind::Instantiation, path, None, args, span_of(node));
      }
    }
    if let Some(arguments) = arguments {
      self.value_refs(arguments);
    }
    let mut nodes: Vec<Node<'t>> = constructor.into_iter().collect();
    nodes.extend(node.child_by_field_name("type_arguments"));
    nodes.extend(arguments);
    self.push_all(nodes);
  }

  pub(super) fn decorator(&mut self, node: Node<'t>) {
    let Some(expression) = first_child(node) else { return };
    let (target, arguments) = match expression.kind() {
      "call_expression" => (expression.child_by_field_name("function"), expression.child_by_field_name("arguments")),
      _ => (Some(expression), None),
    };
    let Some(target) = target else { return self.push_all(children(expression)) };
    let chain = member_chain(target, self.source);
    match chain.head {
      ChainHead::Identifier(head) => {
        if !self.scopes.is_local_value(text(head, self.source))
          && let Some(path) = chain.static_path(self.source)
        {
          let args = summarize_arguments(arguments, self.source);
          self.reference(RefKind::Decorator, path, None, args, span_of(node));
        }
      }
      _ => self.push_all([target]),
    }
    if let Some(arguments) = arguments {
      self.value_refs(arguments);
      self.push_all([arguments]);
    }
  }

  pub(super) fn jsx_element(&mut self, node: Node<'t>) {
    let attributes = field_children(node, "attribute");
    if let Some(path) = node.child_by_field_name("name").and_then(|name| self.component_path(name)) {
      let props = jsx_props(&attributes, self.source);
      self.reference(RefKind::JsxElement, path, None, props, span_of(node));
    }
    self.push_all(attributes);
  }

  fn component_path(&self, name: Node<'_>) -> Option<Vec<String>> {
    let path = match name.kind() {
      "identifier" => {
        let tag = text(name, self.source);
        tag.starts_with(|c: char| c.is_uppercase()).then(|| vec![tag.to_owned()])?
      }
      "member_expression" => {
        let chain = member_chain(name, self.source);
        let ChainHead::Identifier(_) = chain.head else { return None };
        chain.static_path(self.source)?
      }
      _ => return None,
    };
    let head = path.first()?;
    (!self.scopes.is_local_value(head)).then_some(path)
  }

  pub(super) fn jsx_expression(&mut self, node: Node<'t>) {
    let parts = children(node);
    if let [single] = parts.as_slice()
      && single.kind() == "identifier"
    {
      self.value_ref_identifier(*single);
    }
    self.push_all(parts);
  }

  pub(super) fn type_identifier(&mut self, node: Node<'t>) {
    let name = text(node, self.source);
    if !name.is_empty() && !self.scopes.is_local_type(name) {
      self.reference(RefKind::TypeRef, vec![name.to_owned()], None, Vec::new(), span_of(node));
    }
  }

  pub(super) fn nested_type(&mut self, node: Node<'t>) {
    let Some(path) = type_path(node, self.source) else { return };
    if path.first().is_some_and(|head| !self.scopes.is_local_type(head) && !self.scopes.is_local_value(head)) {
      self.reference(RefKind::TypeRef, path, None, Vec::new(), span_of(node));
    }
  }

  pub(super) fn value_refs(&mut self, arguments: Node<'t>) {
    if arguments.kind() != "arguments" {
      return;
    }
    let mut pending: Vec<(Node<'t>, usize)> = children(arguments).into_iter().rev().map(|argument| (argument, 0)).collect();
    while let Some((node, depth)) = pending.pop() {
      let node = unwrap_expression(node);
      match node.kind() {
        "identifier" | "shorthand_property_identifier" => self.value_ref_identifier(node),
        "member_expression" => self.value_ref_chain(node),
        "pair" => pending.extend(node.child_by_field_name("value").map(|value| (value, depth))),
        "array" | "object" | "spread_element" if depth < ARG_MAX_DEPTH => pending.extend(children(node).into_iter().rev().map(|item| (item, depth + 1))),
        _ => {}
      }
    }
  }

  pub(super) fn value_ref_identifier(&mut self, node: Node<'t>) {
    let name = text(node, self.source);
    if self.is_ignored_value(name) {
      return;
    }
    self.reference(RefKind::ValueRef, vec![name.to_owned()], None, Vec::new(), span_of(node));
  }

  fn value_ref_chain(&mut self, node: Node<'t>) {
    let chain = member_chain(node, self.source);
    let receiver = match chain.head {
      ChainHead::Identifier(head) if !self.is_ignored_value(text(head, self.source)) => Some(ReceiverHint::Identifier(text(head, self.source).to_owned())),
      ChainHead::This => Some(self.this_receiver()),
      _ => return,
    };
    if let Some(path) = chain.static_path(self.source) {
      self.reference(RefKind::ValueRef, path, receiver, Vec::new(), span_of(node));
    }
  }

  fn is_ignored_value(&self, name: &str) -> bool {
    (IGNORED_GLOBALS.contains(&name) && !self.module.shadowed_globals.iter().any(|shadowed| shadowed == name)) || self.scopes.is_local_value(name)
  }

  pub(super) fn value_read(&mut self, node: Node<'t>) {
    let name = text(node, self.source);
    if self.callees.remove(&node.id()) || name.is_empty() || self.is_ignored_value(name) || self.is_saturated(&[name.to_owned()], None) {
      return;
    }
    // tree-sitter finds the parent of a zero-width (MISSING) node by recursing from the root, one C frame per tree level.
    let Some(parent) = node.parent() else { return };
    if is_binding_position(node, parent) {
      return;
    }
    self.value_ref_identifier(node);
  }

  fn chain_read(&mut self, node: Node<'t>, chain: &Chain<'t>) {
    match chain.head {
      ChainHead::Identifier(head) if !self.is_ignored_value(text(head, self.source)) => {
        let Some(path) = chain.static_path(self.source) else { return };
        let receiver = Some(ReceiverHint::Identifier(text(head, self.source).to_owned()));
        if !self.is_saturated(&path, receiver.as_ref()) {
          self.reference(RefKind::ValueRef, path, receiver, Vec::new(), span_of(node));
        }
      }
      ChainHead::This => {
        let Some(member) = chain.segments.first().filter(|member| !member.starts_with('[')) else {
          return;
        };
        let path = vec!["this".to_owned(), member.clone()];
        let receiver = Some(self.this_receiver());
        if !self.is_saturated(&path, receiver.as_ref()) {
          self.reference(RefKind::ValueRef, path, receiver, Vec::new(), span_of(node));
        }
      }
      _ => {}
    }
  }

  fn is_saturated(&self, path: &[String], receiver: Option<&ReceiverHint>) -> bool {
    self
      .seen_values
      .get(&(self.owner, path.to_vec(), receiver.cloned()))
      .is_some_and(|spans| spans.len() >= MAX_VALUE_SPANS)
  }

  pub(super) fn member_read(&mut self, node: Node<'t>) {
    let chain = member_chain(node, self.source);
    if chain.segments.is_empty() {
      return self.push_all(children(node));
    }
    let (indexes, root) = chain_operands(node);
    let callee = self.callees.remove(&node.id());
    if !callee {
      self.chain_read(node, &chain);
    }
    let mut nodes = indexes;
    if matches!(chain.head, ChainHead::Expr(_)) {
      nodes.push(root);
    }
    self.push_all(nodes);
  }
}

pub(super) const MAX_VALUE_SPANS: usize = 4;

pub(super) const IGNORED_GLOBALS: [&str; 20] = [
  "undefined",
  "NaN",
  "Infinity",
  "globalThis",
  "arguments",
  "console",
  "JSON",
  "Math",
  "Object",
  "Array",
  "Promise",
  "Date",
  "Number",
  "String",
  "Boolean",
  "Error",
  "process",
  "module",
  "exports",
  "require",
];

fn chain_operands(node: Node<'_>) -> (Vec<Node<'_>>, Node<'_>) {
  let mut indexes = Vec::new();
  let mut current = node;
  loop {
    let next = match current.kind() {
      "member_expression" => current.child_by_field_name("object"),
      "subscript_expression" => {
        indexes.extend(current.child_by_field_name("index"));
        current.child_by_field_name("object")
      }
      "non_null_expression" | "parenthesized_expression" => first_child(current).filter(|inner| inner.kind() != "sequence_expression"),
      _ => None,
    };
    let Some(next) = next else { break };
    current = next;
  }
  indexes.reverse();
  (indexes, current)
}

fn is_binding_position(node: Node<'_>, parent: Node<'_>) -> bool {
  let is = |field: &str| parent.child_by_field_name(field) == Some(node);
  match parent.kind() {
    "arrow_function" | "catch_clause" => is("parameter"),
    "assignment_pattern" | "object_assignment_pattern" | "for_in_statement" => is("left"),
    "required_parameter" | "optional_parameter" => !is("value"),
    "export_statement" => !is("value") && !has_token(parent, "="),
    "array_pattern"
    | "object_pattern"
    | "rest_pattern"
    | "pair_pattern"
    | "import_alias"
    | "import_clause"
    | "import_specifier"
    | "import_require_clause"
    | "namespace_import"
    | "namespace_export"
    | "export_specifier"
    | "extends_clause"
    | "nested_identifier"
    | "nested_type_identifier" => true,
    _ => is("name") || is("alias"),
  }
}

fn awaited_callee(mut callee: Node<'_>) -> Option<Node<'_>> {
  let mut unwrapped = false;
  while callee.kind() == "await_expression" {
    callee = first_child(callee)?;
    unwrapped = true;
  }
  unwrapped.then_some(callee)
}

fn jsx_props(attributes: &[Node<'_>], source: &str) -> Vec<ArgSummary> {
  if attributes.is_empty() {
    return Vec::new();
  }
  let mut entries = Vec::new();
  for attribute in attributes {
    if entries.len() == ARG_MAX_WIDTH {
      entries.push(ObjectEntry {
        key: ObjectKey::Spread,
        value: ArgSummary::Truncated,
      });
      break;
    }
    let parts = children(*attribute);
    let entry = match attribute.kind() {
      "jsx_attribute" => {
        let Some(name) = parts.first() else { continue };
        ObjectEntry {
          key: ObjectKey::Ident(text(*name, source).to_owned()),
          value: parts.get(1).map_or(ArgSummary::Bool(true), |value| jsx_value(*value, source)),
        }
      }
      _ => ObjectEntry {
        key: ObjectKey::Spread,
        value: parts.first().map_or(ArgSummary::Truncated, |spread| jsx_value(*spread, source)),
      },
    };
    entries.push(entry);
  }
  vec![ArgSummary::Object(entries)]
}

fn jsx_value(value: Node<'_>, source: &str) -> ArgSummary {
  match value.kind() {
    "string" => ArgSummary::Str(string_value(value, source)),
    "jsx_expression" => first_child(value).map_or(ArgSummary::Truncated, |inner| jsx_value(inner, source)),
    "spread_element" => first_child(value).map_or(ArgSummary::Truncated, |inner| summarize(inner, source, 1)),
    _ => summarize(value, source, 1),
  }
}
