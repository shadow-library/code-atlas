use atlas_core::{EntityKind, LocalSymbolId, Modifier, Modifiers, RefKind, Visibility};
use tree_sitter::Node;

use super::declarations::declare_modifier;
use super::{Container, Export, Extractor, FunctionScope, Site, Step, ThisBinding};
use crate::scope::{Namespace, ScopeKind};
use crate::signature::{decorator_ranges, doc_comment, signature};
use crate::syntax::{
  ChainHead, children, field_children, has_token, is_function_value, member_chain, property_name, span_between, span_of, text, token, type_path, unwrap_expression,
};

impl<'s, 't> Extractor<'s, 't> {
  pub(super) fn class_symbol(&mut self, node: Node<'t>, site: Site<'t>, mut decorators: Vec<Node<'t>>) {
    decorators.extend(field_children(node, "decorator"));
    let named = node.child_by_field_name("name").filter(|name| !text(*name, self.source).is_empty());
    let anonymous_default = (site.export == Export::Default).then(|| token(site.outer, "default")).flatten();
    let Some(name_node) = named.or(anonymous_default) else {
      return self.class_scope(node, None, decorators);
    };
    let name = named.map_or_else(|| "default".to_owned(), |name| text(name, self.source).to_owned());
    let parent = self.container_parent();
    let item_path = self.child_path(parent, &name);
    let local = self.add_symbol(
      EntityKind::Class,
      name,
      item_path,
      parent,
      (span_of(site.outer), span_of(name_node)),
      site.export.visibility(),
    );
    let body = node.child_by_field_name("body");
    let end = body.map_or(node.end_byte(), |body| body.start_byte());
    let signature = signature(self.source, node.start_byte()..end, &decorator_ranges(&[node]));
    let doc = doc_comment(site.outer, self.source);
    let attributes = decorators.iter().filter_map(|decorator| self.attribute(*decorator)).collect();
    let (extends, implements) = heritage_paths(node, self.source);
    let mut modifiers = declare_modifier(site.ambient);
    if node.kind() == "abstract_class_declaration" {
      modifiers.insert(Modifier::Abstract);
    }
    if let Some(symbol) = self.symbol_mut(local) {
      symbol.signature = signature;
      symbol.doc = doc;
      symbol.attributes = attributes;
      symbol.extends = extends;
      symbol.implements = implements;
      symbol.modifiers = modifiers;
    }
    self.class_scope(node, Some(local), decorators);
  }

  pub(super) fn local_class(&mut self, node: Node<'t>) {
    let decorators = field_children(node, "decorator");
    self.class_scope(node, None, decorators);
  }

  fn class_scope(&mut self, node: Node<'t>, symbol: Option<LocalSymbolId>, decorators: Vec<Node<'t>>) {
    self.checkpoint();
    if symbol.is_some() {
      self.owner = symbol;
    }
    self.container = Container::Local;
    self.this_binding = if symbol.is_some() { ThisBinding::EnclosingClass } else { ThisBinding::Rebound };
    self.scopes.push(ScopeKind::Block);
    self.scopes.bind_type_parameters(node, self.source);
    if node.kind() == "class"
      && let Some(name) = node.child_by_field_name("name")
    {
      self.scopes.bind(text(name, self.source), Namespace::Both, None);
    }
    let mut steps: Vec<Step<'t>> = decorators.into_iter().map(Step::Visit).collect();
    steps.extend(node.child_by_field_name("type_parameters").map(Step::Visit));
    steps.extend(self.heritage(node));
    let mut pending = Vec::new();
    for member in node.child_by_field_name("body").map(children).unwrap_or_default() {
      match (member.kind(), symbol) {
        ("decorator", _) => pending.push(member),
        (_, Some(parent)) => steps.push(Step::Member {
          node: member,
          parent,
          decorators: std::mem::take(&mut pending),
        }),
        (_, None) => {
          steps.extend(pending.drain(..).map(Step::Visit));
          steps.push(Step::Visit(member));
        }
      }
    }
    steps.extend(pending.into_iter().map(Step::Visit));
    self.push_steps(steps);
  }

  fn heritage(&mut self, class: Node<'t>) -> Vec<Step<'t>> {
    let mut steps = Vec::new();
    for heritage in children(class).into_iter().filter(|child| child.kind() == "class_heritage") {
      for clause in children(heritage) {
        match clause.kind() {
          "extends_clause" => {
            for value in field_children(clause, "value") {
              steps.extend(self.inherited(value));
            }
            steps.extend(field_children(clause, "type_arguments").into_iter().map(Step::Visit));
          }
          "implements_clause" => steps.push(Step::Visit(clause)),
          _ => steps.extend(self.inherited(clause)),
        }
      }
    }
    steps
  }

  fn inherited(&mut self, value: Node<'t>) -> Option<Step<'t>> {
    let chain = member_chain(value, self.source);
    let ChainHead::Identifier(head) = chain.head else { return Some(Step::Visit(value)) };
    if !self.scopes.is_local_value(text(head, self.source))
      && let Some(path) = chain.static_path(self.source)
    {
      self.reference(RefKind::TypeRef, path, None, Vec::new(), span_of(value));
    }
    None
  }

  pub(super) fn interface_symbol(&mut self, node: Node<'t>, site: Site<'t>) {
    let Some(name_node) = node.child_by_field_name("name").filter(|name| !text(*name, self.source).is_empty()) else {
      return self.type_scope(node);
    };
    let name = text(name_node, self.source).to_owned();
    let parent = self.container_parent();
    let item_path = self.child_path(parent, &name);
    let local = self.add_symbol(
      EntityKind::Interface,
      name,
      item_path,
      parent,
      (span_of(site.outer), span_of(name_node)),
      site.export.visibility(),
    );
    let clauses: Vec<Node<'t>> = children(node).into_iter().filter(|child| child.kind() == "extends_type_clause").collect();
    let extends = clauses
      .iter()
      .flat_map(|clause| field_children(*clause, "type"))
      .filter_map(|extended| type_path(extended, self.source))
      .collect();
    let body = node.child_by_field_name("body");
    let end = body.map_or(node.end_byte(), |body| body.start_byte());
    let signature = signature(self.source, node.start_byte()..end, &[]);
    let doc = doc_comment(site.outer, self.source);
    if let Some(symbol) = self.symbol_mut(local) {
      symbol.signature = signature;
      symbol.doc = doc;
      symbol.extends = extends;
      symbol.modifiers = declare_modifier(site.ambient);
    }
    self.checkpoint();
    self.owner = Some(local);
    self.container = Container::Local;
    self.scopes.push(ScopeKind::Block);
    self.scopes.bind_type_parameters(node, self.source);
    let mut steps: Vec<Step<'t>> = node.child_by_field_name("type_parameters").into_iter().chain(clauses).map(Step::Visit).collect();
    steps.extend(body.map(children).unwrap_or_default().into_iter().map(|member| Step::Member {
      node: member,
      parent: local,
      decorators: Vec::new(),
    }));
    self.push_steps(steps);
  }

  pub(super) fn enum_symbol(&mut self, node: Node<'t>, site: Site<'t>) {
    let Some(name_node) = node.child_by_field_name("name").filter(|name| !text(*name, self.source).is_empty()) else {
      return self.push_except_name(node);
    };
    let name = text(name_node, self.source).to_owned();
    let parent = self.container_parent();
    let item_path = self.child_path(parent, &name);
    let local = self.add_symbol(
      EntityKind::Enum,
      name,
      item_path,
      parent,
      (span_of(site.outer), span_of(name_node)),
      site.export.visibility(),
    );
    let body = node.child_by_field_name("body");
    let end = body.map_or(node.end_byte(), |body| body.start_byte());
    let enum_signature = signature(self.source, node.start_byte()..end, &[]);
    let doc = doc_comment(site.outer, self.source);
    let mut modifiers = declare_modifier(site.ambient);
    if has_token(node, "const") {
      modifiers.insert(Modifier::Const);
    }
    if let Some(symbol) = self.symbol_mut(local) {
      symbol.signature = enum_signature;
      symbol.doc = doc;
      symbol.modifiers = modifiers;
    }
    let mut steps = Vec::new();
    for member in body.map(children).unwrap_or_default() {
      let (name_node, value) = match member.kind() {
        "enum_assignment" => (member.child_by_field_name("name"), member.child_by_field_name("value")),
        "property_identifier" | "string" | "number" => (Some(member), None),
        _ => continue,
      };
      let Some(name_node) = name_node else { continue };
      let name = property_name(name_node, self.source);
      if name.is_empty() {
        continue;
      }
      let item_path = self.child_path(Some(local), &name);
      let variant = self.add_symbol(EntityKind::Variant, name, item_path, Some(local), (span_of(member), span_of(name_node)), Visibility::Public);
      let signature = signature(self.source, member.byte_range(), &[]);
      let doc = doc_comment(member, self.source);
      if let Some(symbol) = self.symbol_mut(variant) {
        symbol.signature = signature;
        symbol.doc = doc;
      }
      if let Some(value) = value {
        steps.extend([Step::Owner(Some(variant)), Step::Visit(value)]);
      }
    }
    self.checkpoint();
    self.container = Container::Local;
    self.push_steps(steps);
  }

  pub(super) fn member(&mut self, node: Node<'t>, parent: LocalSymbolId, decorators: Vec<Node<'t>>) {
    if let Some(key) = node.child_by_field_name("name").filter(|name| name.kind() == "computed_property_name") {
      self.stack.push(Step::Visit(key));
    }
    match node.kind() {
      "method_definition" | "method_signature" | "abstract_method_signature" => self.method_symbol(node, parent, decorators),
      "public_field_definition" | "field_definition" | "property_signature" => self.field_symbol(node, parent, decorators),
      _ => self.push_all(decorators.into_iter().chain([node])),
    }
  }

  fn method_symbol(&mut self, node: Node<'t>, parent: LocalSymbolId, decorators: Vec<Node<'t>>) {
    let name_node = node.child_by_field_name("name");
    let name = name_node.map(|name| property_name(name, self.source)).unwrap_or_default();
    let Some(name_node) = name_node.filter(|_| !name.is_empty()) else {
      return self.function_scope(
        node,
        FunctionScope {
          decorators,
          ..FunctionScope::default()
        },
      );
    };
    let (visibility, modifiers) = member_traits(node, name_node, self.source);
    let first = decorators.first().copied().unwrap_or(node);
    let item_path = self.child_path(Some(parent), &name);
    let constructor = name == "constructor";
    let local = self.add_symbol(
      EntityKind::Method,
      name,
      item_path,
      Some(parent),
      (span_between(first, node), span_of(name_node)),
      visibility,
    );
    let attributes = decorators
      .iter()
      .chain(&field_children(node, "decorator"))
      .filter_map(|decorator| self.attribute(*decorator))
      .collect();
    self.describe_function(local, node, node, first, modifiers, "");
    if let Some(symbol) = self.symbol_mut(local) {
      symbol.attributes = attributes;
    }
    let property_fields = if constructor { self.parameter_properties(node, parent) } else { Vec::new() };
    self.function_scope(
      node,
      FunctionScope {
        owner: Some(local),
        decorators,
        property_fields,
      },
    );
  }

  fn parameter_properties(&mut self, constructor: Node<'t>, class: LocalSymbolId) -> Vec<(Node<'t>, LocalSymbolId)> {
    let mut fields = Vec::new();
    for parameter in constructor.child_by_field_name("parameters").map(children).unwrap_or_default() {
      let is_property = children(parameter)
        .iter()
        .any(|child| matches!(child.kind(), "accessibility_modifier" | "override_modifier"))
        || has_token(parameter, "readonly");
      let Some(pattern) = parameter.child_by_field_name("pattern").filter(|pattern| pattern.kind() == "identifier") else {
        continue;
      };
      if !is_property {
        continue;
      }
      let name = text(pattern, self.source).to_owned();
      let (visibility, modifiers) = member_traits(parameter, pattern, self.source);
      let item_path = self.child_path(Some(class), &name);
      let field = self.add_symbol(EntityKind::Field, name, item_path, Some(class), (span_of(parameter), span_of(pattern)), visibility);
      let signature = signature(self.source, parameter.byte_range(), &decorator_ranges(&[parameter]));
      if let Some(symbol) = self.symbol_mut(field) {
        symbol.signature = signature;
        symbol.modifiers = modifiers;
      }
      fields.push((parameter, field));
    }
    fields
  }

  fn field_symbol(&mut self, node: Node<'t>, parent: LocalSymbolId, decorators: Vec<Node<'t>>) {
    let name_node = node.child_by_field_name("name").or_else(|| node.child_by_field_name("property"));
    let name = name_node.map(|name| property_name(name, self.source)).unwrap_or_default();
    let Some(name_node) = name_node.filter(|_| !name.is_empty()) else {
      return self.push_all(decorators.into_iter().chain([node]));
    };
    let value = node.child_by_field_name("value");
    let function = value.map(unwrap_expression).filter(|value| is_function_value(*value));
    let (visibility, modifiers) = member_traits(node, name_node, self.source);
    let first = decorators.first().copied().unwrap_or(node);
    let own_decorators = field_children(node, "decorator");
    let kind = if function.is_some() { EntityKind::Method } else { EntityKind::Field };
    let item_path = self.child_path(Some(parent), &name);
    let local = self.add_symbol(kind, name, item_path, Some(parent), (span_between(first, node), span_of(name_node)), visibility);
    match function {
      Some(function) => self.describe_function(local, function, node, first, modifiers, ""),
      None => {
        let signature = signature(self.source, node.byte_range(), &decorator_ranges(&[node]));
        let doc = doc_comment(first, self.source);
        if let Some(symbol) = self.symbol_mut(local) {
          symbol.signature = signature;
          symbol.doc = doc;
          symbol.modifiers = modifiers;
        }
      }
    }
    let attributes = decorators.iter().chain(&own_decorators).filter_map(|decorator| self.attribute(*decorator)).collect();
    if let Some(symbol) = self.symbol_mut(local) {
      symbol.attributes = attributes;
    }
    self.checkpoint();
    self.owner = Some(local);
    self.container = Container::Local;
    let mut nodes = decorators;
    nodes.extend(own_decorators);
    nodes.extend(node.child_by_field_name("type"));
    nodes.extend(value);
    self.push_all(nodes);
  }
}

fn heritage_paths(class: Node<'_>, source: &str) -> (Vec<Vec<String>>, Vec<Vec<String>>) {
  let mut extends = Vec::new();
  let mut implements = Vec::new();
  for heritage in children(class).into_iter().filter(|child| child.kind() == "class_heritage") {
    for clause in children(heritage) {
      match clause.kind() {
        "extends_clause" => extends.extend(
          field_children(clause, "value")
            .into_iter()
            .filter_map(|value| member_chain(value, source).static_path(source)),
        ),
        "implements_clause" => implements.extend(children(clause).into_iter().filter_map(|implemented| type_path(implemented, source))),
        _ => extends.extend(member_chain(clause, source).static_path(source)),
      }
    }
  }
  (extends, implements)
}

fn member_traits(node: Node<'_>, name: Node<'_>, source: &str) -> (Visibility, Modifiers) {
  let mut visibility = if name.kind() == "private_property_identifier" {
    Visibility::Private
  } else {
    Visibility::Public
  };
  let mut modifiers = Modifiers::empty();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    match child.kind() {
      "accessibility_modifier" => {
        visibility = match text(child, source) {
          "private" => Visibility::Private,
          "protected" => Visibility::Protected,
          _ => Visibility::Public,
        }
      }
      "override_modifier" => modifiers.insert(Modifier::Override),
      keyword if !child.is_named() => {
        if let Some(modifier) = keyword_modifier(keyword) {
          modifiers.insert(modifier);
        }
      }
      _ => {}
    }
  }
  (visibility, modifiers)
}

fn keyword_modifier(keyword: &str) -> Option<Modifier> {
  match keyword {
    "static" => Some(Modifier::Static),
    "async" => Some(Modifier::Async),
    "get" => Some(Modifier::Getter),
    "set" => Some(Modifier::Setter),
    "*" => Some(Modifier::Generator),
    "abstract" => Some(Modifier::Abstract),
    "readonly" => Some(Modifier::Readonly),
    "declare" => Some(Modifier::Declare),
    "?" => Some(Modifier::Optional),
    "override" => Some(Modifier::Override),
    _ => None,
  }
}
