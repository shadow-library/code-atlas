use atlas_core::{DefaultExportTarget, EntityKind, ExportFact, Modifier, Modifiers, ParamFact, Visibility};
use tree_sitter::Node;

use super::{Container, Export, Extractor, FunctionScope, Site, Step, ThisBinding};
use crate::args::{parameter_name, summarize};
use crate::fingerprint::fingerprint;
use crate::modules::{export_list, require_bindings, require_call, require_fact};
use crate::scope::{Namespace, ScopeKind, pattern_identifiers};
use crate::signature::{decorator_ranges, doc_comment, signature};
use crate::syntax::{ChainHead, children, field_children, first_child, has_token, is_function_value, member_chain, snippet, span_of, string_value, text, token, unwrap_expression};

impl<'s, 't> Extractor<'s, 't> {
  pub(super) fn declaration(&mut self, node: Node<'t>, site: Site<'t>, decorators: Vec<Node<'t>>) {
    match node.kind() {
      "function_declaration" | "generator_function_declaration" | "function_signature" => self.function_symbol(node, site),
      "class_declaration" | "abstract_class_declaration" | "class" => self.class_symbol(node, site, decorators),
      "interface_declaration" => self.interface_symbol(node, site),
      "type_alias_declaration" => self.type_alias_symbol(node, site),
      "enum_declaration" => self.enum_symbol(node, site),
      "internal_module" | "module" => self.namespace_symbol(node, site),
      "lexical_declaration" | "variable_declaration" => self.variable_symbols(node, site),
      "ambient_declaration" => self.ambient(node, site),
      _ => self.push_except_name(node),
    }
  }

  fn ambient(&mut self, node: Node<'t>, site: Site<'t>) {
    let site = Site { ambient: true, ..site };
    if let Some(global) = token(node, "global") {
      let body = children(node).into_iter().find(|child| child.kind() == "statement_block");
      return self.namespace_with(vec!["global".to_owned()], global, node, body, site);
    }
    if let Some(inner) = first_child(node) {
      self.declaration(inner, site, Vec::new());
    }
  }

  fn function_symbol(&mut self, node: Node<'t>, site: Site<'t>) {
    let Some(name_node) = node.child_by_field_name("name").filter(|name| !text(*name, self.source).is_empty()) else {
      return self.function_scope(node, FunctionScope::default());
    };
    let name = text(name_node, self.source).to_owned();
    let parent = self.container_parent();
    let item_path = self.child_path(parent, &name);
    let local = self.add_symbol(
      EntityKind::Function,
      name,
      item_path,
      parent,
      (span_of(site.outer), span_of(name_node)),
      site.export.visibility(),
    );
    self.describe_function(local, node, node, site.outer, declare_modifier(site.ambient), "");
    self.function_scope(node, FunctionScope::with_owner(local));
  }

  pub(super) fn describe_function(&mut self, local: atlas_core::LocalSymbolId, function: Node<'t>, start: Node<'t>, anchor: Node<'t>, extra: Modifiers, prefix: &str) {
    let body = function.child_by_field_name("body");
    let end = body.map_or(function.end_byte(), |body| body.start_byte());
    let signature = signature(self.source, start.start_byte()..end, &decorator_ranges(&[start, function])).map(|signature| format!("{prefix}{signature}"));
    let doc = doc_comment(anchor, self.source);
    let params = self.params(function);
    let fingerprint = body.map(fingerprint);
    let modifiers = function_modifiers(function).into_iter().fold(extra, Modifiers::with);
    let Some(symbol) = self.symbol_mut(local) else { return };
    symbol.signature = signature;
    symbol.doc = doc;
    symbol.params = params;
    symbol.body = fingerprint;
    symbol.modifiers = symbol.modifiers.iter().fold(modifiers, Modifiers::with);
  }

  pub(super) fn params(&self, function: Node<'_>) -> Vec<ParamFact> {
    if let Some(parameter) = function.child_by_field_name("parameter") {
      return vec![ParamFact {
        name: text(parameter, self.source).to_owned(),
        type_text: None,
        attributes: Vec::new(),
        span: span_of(parameter),
      }];
    }
    let Some(parameters) = function.child_by_field_name("parameters") else {
      return Vec::new();
    };
    children(parameters)
      .into_iter()
      .filter_map(|parameter| {
        Some(ParamFact {
          name: parameter_name(parameter, self.source)?,
          type_text: parameter.child_by_field_name("type").map(|annotation| type_text(annotation, self.source)),
          attributes: field_children(parameter, "decorator")
            .into_iter()
            .filter_map(|decorator| self.attribute(decorator))
            .collect(),
          span: span_of(parameter),
        })
      })
      .collect()
  }

  pub(super) fn function_scope(&mut self, function: Node<'t>, scope: FunctionScope<'t>) {
    self.checkpoint();
    if scope.owner.is_some() {
      self.owner = scope.owner;
    }
    self.container = Container::Local;
    if rebinds_this(function) {
      self.this_binding = ThisBinding::Rebound;
    }
    self.scopes.push(ScopeKind::Function);
    self.scopes.bind_type_parameters(function, self.source);
    self.scopes.bind_parameters(function, self.source);
    if matches!(function.kind(), "function_expression" | "function" | "generator_function")
      && let Some(name) = function.child_by_field_name("name")
    {
      self.scopes.bind(text(name, self.source), Namespace::Value, None);
    }
    let body = function.child_by_field_name("body");
    let statements = body.filter(|body| body.kind() == "statement_block").map(children);
    if let Some(statements) = &statements {
      self.scopes.declare_statements(statements, self.source);
    }
    let mut steps: Vec<Step<'t>> = scope.decorators.into_iter().chain(field_children(function, "decorator")).map(Step::Visit).collect();
    steps.extend(function.child_by_field_name("type_parameters").map(Step::Visit));
    steps.extend(function.child_by_field_name("parameter").map(Step::Visit));
    for parameter in function.child_by_field_name("parameters").map(children).unwrap_or_default() {
      match scope.property_fields.iter().find(|(node, _)| *node == parameter) {
        Some((_, field)) => steps.extend([Step::Owner(Some(*field)), Step::Visit(parameter), Step::Owner(self.owner)]),
        None => steps.push(Step::Visit(parameter)),
      }
    }
    steps.extend(function.child_by_field_name("return_type").map(Step::Visit));
    match statements {
      Some(statements) => steps.extend(statements.into_iter().map(Step::Visit)),
      None => steps.extend(body.map(Step::Visit)),
    }
    self.push_steps(steps);
  }

  pub(super) fn export_statement(&mut self, node: Node<'t>) {
    if !self.declares_symbols() {
      return self.push_all(children(node));
    }
    let decorators = field_children(node, "decorator");
    let export = if has_token(node, "default") { Export::Default } else { Export::Named };
    if let Some(declaration) = node.child_by_field_name("declaration") {
      return self.declaration(
        declaration,
        Site {
          export,
          outer: node,
          ambient: false,
        },
        decorators,
      );
    }
    if export == Export::Default {
      return self.default_export(node, decorators);
    }
    if has_token(node, "=") {
      let Some(value) = children(node).into_iter().find(|child| child.kind() != "decorator") else {
        return;
      };
      self.facts.exports.push(ExportFact::CommonJs {
        exported: None,
        value: summarize(value, self.source, 0),
        span: span_of(node),
      });
      return self.push_all([value]);
    }
    let exports = export_list(node, self.source);
    self.facts.exports.extend(exports);
  }

  fn default_export(&mut self, node: Node<'t>, decorators: Vec<Node<'t>>) {
    let Some(value) = node.child_by_field_name("value") else { return };
    match value.kind() {
      "class" => self.class_symbol(
        value,
        Site {
          export: Export::Default,
          outer: node,
          ambient: false,
        },
        decorators,
      ),
      "identifier" => self.facts.exports.push(ExportFact::Default {
        target: DefaultExportTarget::Identifier(text(value, self.source).to_owned()),
        span: span_of(node),
      }),
      _ if is_function_value(value) => self.default_function(node, value),
      _ => {
        self.facts.exports.push(ExportFact::Default {
          target: DefaultExportTarget::Expression(summarize(value, self.source, 0)),
          span: span_of(node),
        });
        self.push_all([value]);
      }
    }
  }

  fn default_function(&mut self, outer: Node<'t>, function: Node<'t>) {
    let named = function.child_by_field_name("name").filter(|name| !text(*name, self.source).is_empty());
    let Some(name_node) = named.or_else(|| token(outer, "default")) else {
      return self.function_scope(function, FunctionScope::default());
    };
    let name = named.map_or_else(|| "default".to_owned(), |name| text(name, self.source).to_owned());
    let item_path = self.child_path(None, &name);
    let local = self.add_symbol(
      EntityKind::Function,
      name,
      item_path,
      None,
      (span_of(outer), span_of(name_node)),
      Visibility::ExportedDefault,
    );
    self.describe_function(local, function, function, outer, Modifiers::empty(), "");
    self.function_scope(function, FunctionScope::with_owner(local));
  }

  fn variable_symbols(&mut self, node: Node<'t>, site: Site<'t>) {
    let Site { export, outer, ambient } = site;
    let keyword = declaration_keyword(node, self.source);
    let value_kind = if keyword == "const" { EntityKind::Constant } else { EntityKind::Variable };
    let parent = self.container_parent();
    let enclosing = self.owner;
    let mut steps = Vec::new();
    for declarator in children(node).into_iter().filter(|child| child.kind() == "variable_declarator") {
      let Some(pattern) = declarator.child_by_field_name("name") else { continue };
      let value = declarator.child_by_field_name("value");
      if let Some(call) = value.and_then(|value| require_call(value, self.source)) {
        if !self.in_error {
          let bindings = require_bindings(pattern, self.source);
          self.facts.imports.push(require_fact(call, bindings, self.owner, declarator, self.source));
        }
        continue;
      }
      if pattern.kind() != "identifier" {
        for identifier in pattern_identifiers(pattern) {
          let name = text(identifier, self.source).to_owned();
          let item_path = self.child_path(parent, &name);
          let signature = format!("{keyword} {name}");
          let local = self.add_symbol(value_kind.clone(), name, item_path, parent, (span_of(declarator), span_of(identifier)), export.visibility());
          self.describe_value(local, Some(signature), outer, ambient);
        }
        steps.extend([Step::Owner(enclosing), Step::Visit(pattern)]);
        steps.extend(value.map(Step::Visit));
        continue;
      }
      let name = text(pattern, self.source).to_owned();
      if name.is_empty() {
        continue;
      }
      let function = value.map(unwrap_expression).filter(|value| is_function_value(*value));
      let kind = if function.is_some() { EntityKind::Function } else { value_kind.clone() };
      let item_path = self.child_path(parent, &name);
      let local = self.add_symbol(kind, name, item_path, parent, (span_of(declarator), span_of(pattern)), export.visibility());
      match function {
        Some(function) => self.describe_function(local, function, declarator, outer, declare_modifier(ambient), &format!("{keyword} ")),
        None => {
          let signature = signature(self.source, declarator.byte_range(), &[]).map(|signature| format!("{keyword} {signature}"));
          self.describe_value(local, signature, outer, ambient);
        }
      }
      steps.push(Step::Owner(Some(local)));
      steps.extend(declarator.child_by_field_name("type").map(Step::Visit));
      steps.extend(value.map(Step::Visit));
    }
    self.checkpoint();
    self.container = Container::Local;
    self.push_steps(steps);
  }

  fn describe_value(&mut self, local: atlas_core::LocalSymbolId, signature: Option<String>, anchor: Node<'_>, ambient: bool) {
    let doc = doc_comment(anchor, self.source);
    let Some(symbol) = self.symbol_mut(local) else { return };
    symbol.signature = signature;
    symbol.doc = doc;
    symbol.modifiers = declare_modifier(ambient);
  }

  pub(super) fn local_variables(&mut self, node: Node<'t>) {
    let hoisted = node.kind() == "variable_declaration";
    let mut nodes = Vec::new();
    for declarator in children(node).into_iter().filter(|child| child.kind() == "variable_declarator") {
      let pattern = declarator.child_by_field_name("name");
      let value = declarator.child_by_field_name("value");
      if hoisted && let Some(pattern) = pattern {
        self.bind_var(pattern);
      }
      let require = value.and_then(|value| require_call(value, self.source)).filter(|_| !self.scopes.is_local_value("require"));
      if let (Some(call), Some(pattern)) = (require, pattern) {
        if !self.in_error {
          let bindings = require_bindings(pattern, self.source);
          self.facts.imports.push(require_fact(call, bindings, self.owner, declarator, self.source));
        }
        continue;
      }
      nodes.extend(pattern);
      nodes.extend(declarator.child_by_field_name("type"));
      nodes.extend(value);
    }
    self.push_all(nodes);
  }

  fn type_alias_symbol(&mut self, node: Node<'t>, site: Site<'t>) {
    let Some(name_node) = node.child_by_field_name("name").filter(|name| !text(*name, self.source).is_empty()) else {
      return self.type_scope(node);
    };
    let name = text(name_node, self.source).to_owned();
    let parent = self.container_parent();
    let item_path = self.child_path(parent, &name);
    let local = self.add_symbol(
      EntityKind::TypeAlias,
      name,
      item_path,
      parent,
      (span_of(site.outer), span_of(name_node)),
      site.export.visibility(),
    );
    let signature = signature(self.source, node.byte_range(), &[]);
    self.describe_value(local, signature, site.outer, site.ambient);
    self.checkpoint();
    self.owner = Some(local);
    self.container = Container::Local;
    self.scopes.push(ScopeKind::Block);
    self.scopes.bind_type_parameters(node, self.source);
    self.push_except_name(node);
  }

  fn namespace_symbol(&mut self, node: Node<'t>, site: Site<'t>) {
    let Some(name_node) = node.child_by_field_name("name") else {
      return self.push_except_name(node);
    };
    let segments: Vec<String> = match name_node.kind() {
      "string" => vec![string_value(name_node, self.source)],
      _ => text(name_node, self.source).split('.').map(|segment| segment.trim().to_owned()).collect(),
    };
    if segments.iter().any(String::is_empty) {
      return self.push_except_name(node);
    }
    let body = node.child_by_field_name("body");
    let outer = match (site.outer == node, node.parent()) {
      (true, Some(parent)) if parent.kind() == "expression_statement" => parent,
      _ => site.outer,
    };
    self.namespace_with(segments, name_node, node, body, Site { outer, ..site });
  }

  fn namespace_with(&mut self, segments: Vec<String>, name_node: Node<'t>, node: Node<'t>, body: Option<Node<'t>>, site: Site<'t>) {
    let parent = self.container_parent();
    let mut item_path = parent
      .and_then(|parent| self.facts.symbol(parent))
      .map(|parent| parent.item_path.clone())
      .unwrap_or_default();
    item_path.extend(segments.iter().cloned());
    let name = segments.last().cloned().unwrap_or_default();
    let local = self.add_symbol(
      EntityKind::Namespace,
      name,
      item_path,
      parent,
      (span_of(site.outer), span_of(name_node)),
      site.export.visibility(),
    );
    let end = body.map_or(node.end_byte(), |body| body.start_byte());
    let signature = signature(self.source, node.start_byte()..end, &[]);
    self.describe_value(local, signature, site.outer, site.ambient);
    self.checkpoint();
    self.owner = Some(local);
    self.container = Container::Namespace(local);
    self.push_all(body.map(children).unwrap_or_default());
  }

  pub(super) fn assignment(&mut self, node: Node<'t>) {
    let (Some(left), Some(right)) = (node.child_by_field_name("left"), node.child_by_field_name("right")) else {
      return self.push_all(children(node));
    };
    let Some(exported) = self.commonjs_target(left) else {
      return self.push_all(children(node));
    };
    if !self.in_error {
      self.facts.exports.push(ExportFact::CommonJs {
        exported: exported.clone(),
        value: summarize(right, self.source, 0),
        span: span_of(node),
      });
    }
    let function = Some(unwrap_expression(right)).filter(|value| is_function_value(*value));
    let module_level = self.container == Container::Module && self.owner.is_none();
    if let (Some(name), Some(function), Some(property), true) = (exported, function, left.child_by_field_name("property"), module_level) {
      let anchor = node.parent().filter(|parent| parent.kind() == "expression_statement").unwrap_or(node);
      let local = self.add_symbol(
        EntityKind::Function,
        name.clone(),
        vec![name],
        None,
        (span_of(node), span_of(property)),
        Visibility::Exported,
      );
      self.describe_function(local, function, node, anchor, Modifiers::empty(), "");
      return self.function_scope(function, FunctionScope::with_owner(local));
    }
    self.push_all(children(node));
  }

  fn commonjs_target(&self, left: Node<'_>) -> Option<Option<String>> {
    let chain = member_chain(left, self.source);
    let ChainHead::Identifier(head) = chain.head else { return None };
    let head = text(head, self.source);
    if self.scopes.is_local_value(head) || chain.segments.iter().any(|segment| segment.starts_with('[')) {
      return None;
    }
    match (head, chain.segments.as_slice()) {
      ("module", [exports]) if exports == "exports" => Some(None),
      ("module", [exports, name]) if exports == "exports" => Some(Some(name.clone())),
      ("exports", [name]) => Some(Some(name.clone())),
      _ => None,
    }
  }
}

impl<'t> FunctionScope<'t> {
  pub(super) fn with_owner(owner: atlas_core::LocalSymbolId) -> Self {
    Self {
      owner: Some(owner),
      ..Self::default()
    }
  }
}

fn rebinds_this(function: Node<'_>) -> bool {
  match function.kind() {
    "arrow_function" => false,
    "method_definition" => function.parent().is_none_or(|parent| parent.kind() != "class_body"),
    _ => true,
  }
}

pub(super) fn declare_modifier(ambient: bool) -> Modifiers {
  if ambient { Modifiers::empty().with(Modifier::Declare) } else { Modifiers::empty() }
}

fn function_modifiers(function: Node<'_>) -> Vec<Modifier> {
  let mut modifiers = Vec::new();
  if has_token(function, "async") {
    modifiers.push(Modifier::Async);
  }
  if has_token(function, "*") || matches!(function.kind(), "generator_function_declaration" | "generator_function") {
    modifiers.push(Modifier::Generator);
  }
  modifiers
}

fn declaration_keyword<'s>(node: Node<'_>, source: &'s str) -> &'s str {
  match node.child_by_field_name("kind") {
    Some(kind) => text(kind, source),
    None => "var",
  }
}

fn type_text(annotation: Node<'_>, source: &str) -> String {
  let raw = text(annotation, source);
  snippet(raw.strip_prefix(':').unwrap_or(raw).trim())
}
