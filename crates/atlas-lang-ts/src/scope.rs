use std::collections::HashMap;

use tree_sitter::Node;

use crate::syntax::{children, text, type_path, unwrap_expression};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Namespace {
  Value,
  Type,
  Both,
}

impl Namespace {
  fn has_value(self) -> bool {
    matches!(self, Self::Value | Self::Both)
  }

  fn has_type(self) -> bool {
    matches!(self, Self::Type | Self::Both)
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKind {
  Function,
  Block,
}

#[derive(Debug)]
pub(crate) struct Binding {
  scope: usize,
  type_hint: Option<Vec<String>>,
}

impl Binding {
  pub(crate) fn type_hint(&self) -> Option<&[String]> {
    self.type_hint.as_deref()
  }
}

#[derive(Debug)]
struct Scope<'s> {
  kind: ScopeKind,
  bound: Vec<(&'s str, Namespace)>,
}

#[derive(Debug, Default)]
pub(crate) struct Scopes<'s> {
  stack: Vec<Scope<'s>>,
  values: HashMap<&'s str, Vec<Binding>>,
  types: HashMap<&'s str, Vec<usize>>,
}

impl<'s> Scopes<'s> {
  pub(crate) fn len(&self) -> usize {
    self.stack.len()
  }

  pub(crate) fn truncate(&mut self, len: usize) {
    while self.stack.len() > len {
      let Some(scope) = self.stack.pop() else { return };
      let index = self.stack.len();
      for (name, namespace) in scope.bound {
        if namespace.has_value() {
          remove_last(self.values.get_mut(name), |binding| binding.scope == index);
        }
        if namespace.has_type() {
          remove_last(self.types.get_mut(name), |scope| *scope == index);
        }
      }
    }
  }

  pub(crate) fn push(&mut self, kind: ScopeKind) {
    self.stack.push(Scope { kind, bound: Vec::new() });
  }

  pub(crate) fn bind(&mut self, name: &'s str, namespace: Namespace, type_hint: Option<Vec<String>>) {
    let Some(index) = self.stack.len().checked_sub(1) else { return };
    self.bind_at(index, name, namespace, type_hint);
  }

  pub(crate) fn bind_in_function(&mut self, name: &'s str, type_hint: Option<Vec<String>>) {
    let index = self.stack.iter().rposition(|scope| scope.kind == ScopeKind::Function).or(self.stack.len().checked_sub(1));
    if let Some(index) = index {
      self.bind_at(index, name, Namespace::Value, type_hint);
    }
  }

  fn bind_at(&mut self, index: usize, name: &'s str, namespace: Namespace, type_hint: Option<Vec<String>>) {
    if name.is_empty() {
      return;
    }
    let type_hint = type_hint.filter(|hint| hint.first().is_some_and(|head| !self.is_local_type(head)));
    let Some(scope) = self.stack.get_mut(index) else { return };
    scope.bound.push((name, namespace));
    if namespace.has_value() {
      self.values.entry(name).or_default().push(Binding { scope: index, type_hint });
    }
    if namespace.has_type() {
      self.types.entry(name).or_default().push(index);
    }
  }

  pub(crate) fn value(&self, name: &str) -> Option<&Binding> {
    self.values.get(name)?.iter().max_by_key(|binding| binding.scope)
  }

  pub(crate) fn is_local_value(&self, name: &str) -> bool {
    self.value(name).is_some()
  }

  pub(crate) fn is_local_type(&self, name: &str) -> bool {
    self.types.get(name).is_some_and(|scopes| !scopes.is_empty())
  }

  pub(crate) fn bind_pattern(&mut self, pattern: Node<'_>, source: &'s str, type_hint: Option<Vec<String>>) {
    if pattern.kind() == "identifier" {
      return self.bind(text(pattern, source), Namespace::Value, type_hint);
    }
    for identifier in pattern_identifiers(pattern) {
      self.bind(text(identifier, source), Namespace::Value, None);
    }
  }

  pub(crate) fn bind_type_parameters(&mut self, declaration: Node<'_>, source: &'s str) {
    let Some(parameters) = declaration.child_by_field_name("type_parameters") else { return };
    for parameter in children(parameters) {
      if let Some(name) = parameter.child_by_field_name("name") {
        self.bind(text(name, source), Namespace::Type, None);
      }
    }
  }

  pub(crate) fn bind_parameters(&mut self, function: Node<'_>, source: &'s str) {
    if let Some(parameter) = function.child_by_field_name("parameter") {
      self.bind_pattern(parameter, source, None);
    }
    let Some(parameters) = function.child_by_field_name("parameters") else { return };
    for parameter in children(parameters) {
      let (pattern, hint) = match parameter.kind() {
        "required_parameter" | "optional_parameter" => (
          parameter.child_by_field_name("pattern"),
          parameter.child_by_field_name("type").and_then(|annotation| type_path(annotation, source)),
        ),
        _ => (Some(parameter), None),
      };
      if let Some(pattern) = pattern {
        self.bind_pattern(pattern, source, hint);
      }
    }
  }

  pub(crate) fn declare_statements(&mut self, statements: &[Node<'_>], source: &'s str) {
    for statement in statements {
      self.declare_statement(*statement, source);
    }
  }

  fn declare_statement(&mut self, statement: Node<'_>, source: &'s str) {
    let Some(statement) = unwrap_declaration(statement) else { return };
    let name = statement.child_by_field_name("name").map(|name| text(name, source));
    match statement.kind() {
      "lexical_declaration" | "variable_declaration" => {
        for declarator in children(statement).into_iter().filter(|child| child.kind() == "variable_declarator") {
          if let Some(pattern) = declarator.child_by_field_name("name") {
            self.bind_pattern(pattern, source, declarator_type_hint(declarator, source));
          }
        }
      }
      "function_declaration" | "generator_function_declaration" | "function_signature" | "internal_module" | "module" => {
        if let Some(name) = name {
          self.bind(name, Namespace::Value, None);
        }
      }
      "class_declaration" | "abstract_class_declaration" | "enum_declaration" => {
        if let Some(name) = name {
          self.bind(name, Namespace::Both, None);
        }
      }
      "interface_declaration" | "type_alias_declaration" => {
        if let Some(name) = name {
          self.bind(name, Namespace::Type, None);
        }
      }
      _ => {}
    }
  }
}

fn unwrap_declaration(mut statement: Node<'_>) -> Option<Node<'_>> {
  while matches!(statement.kind(), "ambient_declaration" | "export_statement") {
    statement = statement
      .child_by_field_name("declaration")
      .or_else(|| children(statement).into_iter().next())
      .filter(|inner| inner.kind() != "statement_block")?;
  }
  Some(statement)
}

pub(crate) fn declarator_type_hint(declarator: Node<'_>, source: &str) -> Option<Vec<String>> {
  if let Some(hint) = declarator.child_by_field_name("type").and_then(|annotation| type_path(annotation, source)) {
    return Some(hint);
  }
  let value = unwrap_expression(declarator.child_by_field_name("value")?);
  let value = match value.kind() {
    "await_expression" => children(value).into_iter().next()?,
    _ => value,
  };
  if value.kind() != "new_expression" {
    return None;
  }
  type_path(value.child_by_field_name("constructor")?, source)
}

pub(crate) fn pattern_identifiers(pattern: Node<'_>) -> Vec<Node<'_>> {
  let mut found = Vec::new();
  let mut pending = vec![pattern];
  while let Some(node) = pending.pop() {
    match node.kind() {
      "identifier" | "shorthand_property_identifier_pattern" => found.push(node),
      "object_pattern" | "array_pattern" | "rest_pattern" => pending.extend(children(node).into_iter().rev()),
      "pair_pattern" => pending.extend(node.child_by_field_name("value")),
      "object_assignment_pattern" | "assignment_pattern" => pending.extend(node.child_by_field_name("left")),
      "required_parameter" | "optional_parameter" => pending.extend(node.child_by_field_name("pattern")),
      _ => {}
    }
  }
  found
}

fn remove_last<T>(entries: Option<&mut Vec<T>>, matches: impl Fn(&T) -> bool) {
  let Some(entries) = entries else { return };
  if let Some(position) = entries.iter().rposition(matches) {
    entries.remove(position);
  }
}
