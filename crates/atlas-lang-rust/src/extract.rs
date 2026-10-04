use std::collections::{HashMap, HashSet};

use atlas_core::{
  ArgSummary, AttributeFact, Diagnostic, EntityKind, FileFacts, FileRef, ImplFact, ImplInfo, ImportFact, LocalSymbolId, MAX_REPORTED_PARSE_ERRORS, Modifier, ModuleBody,
  ModuleDeclFact, ParamFact, ParseErrorFact, ParseStatus, Producer, RefKind, Severity, Span, SymbolFact, TestInfo, TestKind, UseBinding, UsePath, Visibility,
};
use tree_sitter::{Node, Tree};

use crate::fingerprint::body_fingerprint;
use crate::refs::{Bindings, Cx};
use crate::source::{Doc, LineIndex, signature_text};
use crate::summary::summarize;
use crate::syntax::{
  Decorated, Walk, children, decorated_children, doc_lines, inner_docs, item_modifiers, join_doc, name_of, named_children, path_segments, signature, type_path, visibility,
  walk_tree,
};
use crate::tokens::token_tree_args;

const MAX_ITEM_NESTING: u32 = 64;
const MAX_USE_NESTING: u32 = 32;
pub(crate) const DEPTH_LIMIT_CODE: &str = "rust.depth_limit";

pub(crate) struct Extractor<'s> {
  source: &'s str,
  lines: LineIndex,
  test_location: TestLocation,
  pub(crate) facts: FileFacts,
  pub(crate) locals: Bindings,
  pub(crate) generics: Bindings,
  pub(crate) local_types: HashSet<String>,
  pub(crate) depth_exceeded: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TestLocation {
  Unit,
  Integration,
}

impl TestLocation {
  fn of(file: &FileRef) -> Self {
    let directories: Vec<&str> = file.path.segments().collect();
    let directories = &directories[..directories.len().saturating_sub(1)];
    match directories.iter().position(|segment| *segment == "tests") {
      Some(index) if !directories[..index].contains(&"src") => Self::Integration,
      _ => Self::Unit,
    }
  }

  fn kind(self) -> TestKind {
    match self {
      Self::Unit => TestKind::Unit,
      Self::Integration => TestKind::Integration,
    }
  }
}

#[derive(Clone)]
struct Container {
  module_suffix: Vec<String>,
  parent: Option<LocalSymbolId>,
  depth: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
  Module,
  Impl,
  Trait,
  Foreign,
}

#[derive(Clone)]
struct Scope {
  role: Role,
  prefix: Vec<String>,
  parent: Option<LocalSymbolId>,
  impl_of: Option<ImplInfo>,
  visibility: Option<Visibility>,
}

impl Scope {
  fn module(container: &Container, role: Role) -> Self {
    Self {
      role,
      prefix: Vec::new(),
      parent: container.parent,
      impl_of: None,
      visibility: None,
    }
  }

  fn item_path(&self, name: &str) -> Vec<String> {
    self.prefix.iter().cloned().chain(std::iter::once(name.to_owned())).collect()
  }
}

impl<'s> Extractor<'s> {
  pub(crate) fn new(file: &FileRef, source: &'s str, producer: Producer) -> Self {
    Self {
      source,
      lines: LineIndex::new(source),
      test_location: TestLocation::of(file),
      facts: FileFacts::new(file.clone(), producer),
      locals: Bindings::default(),
      local_types: HashSet::new(),
      generics: Bindings::default(),
      depth_exceeded: false,
    }
  }

  pub(crate) fn run(mut self, tree: &Tree) -> FileFacts {
    let doc = Doc::root(self.source);
    let root = tree.root_node();
    self.local_types = declared_type_names(doc, root);
    self.facts.parse_status = parse_status(root, &self.lines);
    let file = Container {
      module_suffix: Vec::new(),
      parent: None,
      depth: 0,
    };
    self.facts.file_doc = self.file_doc(doc, root);
    self.items(doc, root, &file);
    self.link_impls();
    self.dedupe_type_refs();
    if self.depth_exceeded {
      tracing::debug!(path = %self.facts.file.path, "rust analyzer skipped syntax nested beyond its depth limit");
      self.facts.diagnostics.push(Diagnostic {
        severity: Severity::Warning,
        code: DEPTH_LIMIT_CODE.to_owned(),
        message: "syntax nested beyond the analyzer depth limit was skipped".to_owned(),
        span: None,
      });
    }
    self.facts
  }

  pub(crate) fn span(&self, doc: Doc<'_>, node: Node<'_>) -> Span {
    self.span_between(doc, node.start_byte(), node.end_byte())
  }

  fn span_between(&self, doc: Doc<'_>, start: usize, end: usize) -> Span {
    self.lines.span(doc.absolute(start), doc.absolute(end))
  }

  fn add(&mut self, mut symbol: SymbolFact) -> LocalSymbolId {
    let local = self.facts.next_local_id();
    symbol.local = local;
    self.facts.symbols.push(symbol);
    local
  }

  fn items(&mut self, doc: Doc<'_>, container_node: Node<'_>, container: &Container) {
    if container.depth >= MAX_ITEM_NESTING {
      self.depth_exceeded = true;
      return;
    }
    let module = Scope::module(container, Role::Module);
    for decorated in decorated_children(container_node) {
      let node = decorated.node;
      match node.kind() {
        "function_item" => self.function(doc, &decorated, container, &module),
        "struct_item" | "union_item" => self.structure(doc, &decorated, container),
        "enum_item" => self.enumeration(doc, &decorated, container),
        "trait_item" => self.trait_item(doc, &decorated, container),
        "impl_item" => self.impl_block(doc, &decorated, container),
        "type_item" => self.type_alias(doc, &decorated, container, &module),
        "const_item" | "static_item" => self.constant(doc, &decorated, container, &module),
        "mod_item" => self.module(doc, &decorated, container),
        "macro_definition" => self.macro_definition(doc, &decorated, container),
        "use_declaration" => self.use_declaration(doc, node, container.parent),
        "extern_crate_declaration" => self.extern_crate(doc, node),
        "foreign_mod_item" => self.foreign_items(doc, node, container),
        "macro_invocation" => self.macro_call(Cx::new(doc, container.parent), node),
        "expression_statement" => {
          if let Some(invocation) = node.named_child(0).filter(|child| child.kind() == "macro_invocation") {
            self.macro_call(Cx::new(doc, container.parent), invocation);
          }
        }
        "ERROR" => self.items(doc, node, &container.nested()),
        _ => {}
      }
    }
  }

  fn symbol(&self, doc: Doc<'_>, decorated: &Decorated<'_>, kind: EntityKind, name: (String, Node<'_>), item_path: Vec<String>, container: &Container) -> SymbolFact {
    let (name, name_node) = name;
    let span = self.span_between(doc, decorated.start, decorated.node.end_byte());
    let name_span = self.span(doc, name_node);
    let mut symbol = SymbolFact::new(LocalSymbolId(0), kind, name, item_path, span, name_span, Visibility::Private);
    symbol.module_suffix = container.module_suffix.clone();
    symbol.parent = container.parent;
    symbol.attributes = self.attributes(doc, &decorated.attributes);
    symbol.doc = documentation(doc, &decorated.docs, &symbol.attributes, Vec::new());
    symbol
  }

  fn attributes(&self, doc: Doc<'_>, items: &[Node<'_>]) -> Vec<AttributeFact> {
    items.iter().filter_map(|item| self.attribute(doc, *item)).collect()
  }

  fn file_doc(&self, doc: Doc<'_>, root: Node<'_>) -> Option<String> {
    let inner: Vec<Node<'_>> = children(root).into_iter().filter(|child| child.kind() == "inner_attribute_item").collect();
    documentation(doc, &inner_docs(root), &self.attributes(doc, &inner), Vec::new())
  }

  pub(crate) fn push_impl(
    &mut self,
    doc: Doc<'_>,
    node: Node<'_>,
    attributes: &[Node<'_>],
    start: usize,
    module_suffix: Vec<String>,
    scope: Option<LocalSymbolId>,
  ) -> Option<ImplInfo> {
    let self_node = node.child_by_field_name("type")?;
    let info = ImplInfo {
      self_type: type_path(doc, self_node),
      trait_ref: node.child_by_field_name("trait").map(|trait_node| type_path(doc, trait_node)),
    };
    let fact = ImplFact {
      self_type: info.self_type.clone(),
      trait_ref: info.trait_ref.clone(),
      generic_params: generic_names(doc, node),
      module_suffix,
      scope,
      attributes: self.attributes(doc, attributes),
      span: self.span_between(doc, start, node.end_byte()),
    };
    self.facts.impls.push(fact);
    Some(info)
  }

  fn attribute(&self, doc: Doc<'_>, item: Node<'_>) -> Option<AttributeFact> {
    let attribute = named_children(item).into_iter().find(|child| child.kind() == "attribute")?;
    let path = path_segments(doc, attribute.named_child(0)?)?;
    let args = match (attribute.child_by_field_name("arguments"), attribute.child_by_field_name("value")) {
      (Some(arguments), _) => token_tree_args(doc, arguments, 0),
      (None, Some(value)) => vec![summarize(doc, value, 0)],
      (None, None) => Vec::new(),
    };
    Some(AttributeFact {
      path,
      args,
      span: self.span(doc, item),
    })
  }

  fn function(&mut self, doc: Doc<'_>, decorated: &Decorated<'_>, container: &Container, scope: &Scope) {
    let node = decorated.node;
    let Some(name) = name_of(doc, node) else { return };
    let body = node.child_by_field_name("body");
    let item_path = scope.item_path(&name.0);
    let kind = match scope.role {
      Role::Module | Role::Foreign => EntityKind::Function,
      Role::Impl | Role::Trait => EntityKind::Method,
    };
    let mut symbol = self.symbol(doc, decorated, kind, name, item_path, container);
    symbol.parent = scope.parent;
    symbol.visibility = scope.visibility.clone().unwrap_or_else(|| visibility(doc, node));
    symbol.modifiers = item_modifiers(node);
    match (scope.role, body) {
      (Role::Trait, Some(_)) => symbol.modifiers.insert(Modifier::Default),
      (Role::Trait, None) => symbol.modifiers.insert(Modifier::Abstract),
      (Role::Foreign, _) => symbol.modifiers.insert(Modifier::Extern),
      _ => {}
    }
    symbol.signature = Some(signature(doc, node, body.map_or(node.end_byte(), |body| body.start_byte())));
    symbol.impl_of = scope.impl_of.clone();
    symbol.params = self.params(doc, node);
    if scope.role == Role::Module
      && let Some(test) = self.test_info(&symbol.attributes)
    {
      symbol.kind = EntityKind::Test;
      symbol.test = Some(test);
    }
    symbol.body = body.map(|body| body_fingerprint(body, self.span(doc, body).line_count()));
    let local = self.add(symbol);
    self.function_refs(Cx::new(doc, Some(local)), node);
  }

  fn params(&self, doc: Doc<'_>, function: Node<'_>) -> Vec<ParamFact> {
    let Some(parameters) = function.child_by_field_name("parameters") else {
      return Vec::new();
    };
    decorated_children(parameters)
      .into_iter()
      .filter_map(|decorated| {
        let node = decorated.node;
        let (name, type_text) = match node.kind() {
          "parameter" => (
            signature_text(doc.text(node.child_by_field_name("pattern")?)),
            node.child_by_field_name("type").map(|kind| signature_text(doc.text(kind))),
          ),
          "self_parameter" => ("self".to_owned(), Some(signature_text(doc.text(node)))),
          _ => return None,
        };
        Some(ParamFact {
          name,
          type_text,
          attributes: self.attributes(doc, &decorated.attributes),
          span: self.span(doc, node),
        })
      })
      .collect()
  }

  fn test_info(&self, attributes: &[AttributeFact]) -> Option<TestInfo> {
    attributes.iter().find_map(|attribute| {
      let path: Vec<&str> = attribute.path.iter().map(String::as_str).collect();
      let (framework, kind) = match path.as_slice() {
        ["test"] => ("rust", self.test_location.kind()),
        ["bench"] => ("rust", TestKind::Bench),
        [framework @ ("rstest" | "test_case")] => (*framework, self.test_location.kind()),
        [framework, .., "test"] => (*framework, self.test_location.kind()),
        _ => return None,
      };
      Some(TestInfo {
        framework: framework.to_owned(),
        kind,
        display_name: None,
      })
    })
  }

  fn structure(&mut self, doc: Doc<'_>, decorated: &Decorated<'_>, container: &Container) {
    let node = decorated.node;
    let Some(name) = name_of(doc, node) else { return };
    let type_name = name.0.clone();
    let body = node.child_by_field_name("body");
    let named_body = body.filter(|body| body.kind() == "field_declaration_list");
    let mut symbol = self.symbol(doc, decorated, EntityKind::Struct, name, vec![type_name.clone()], container);
    symbol.visibility = visibility(doc, node);
    symbol.modifiers = item_modifiers(node);
    symbol.signature = Some(signature(doc, node, named_body.map_or(node.end_byte(), |body| body.start_byte())));
    let local = self.add(symbol);
    let cx = Cx::new(doc, Some(local));
    self.generics.push(generic_names(doc, node));
    self.header_types(cx, node);
    match body {
      Some(fields) if fields.kind() == "field_declaration_list" => self.fields(doc, fields, container, &type_name, local),
      Some(tuple) => self.visit_type(cx, tuple),
      None => {}
    }
    self.generics.pop();
  }

  fn fields(&mut self, doc: Doc<'_>, list: Node<'_>, container: &Container, owner: &str, owner_local: LocalSymbolId) {
    for decorated in decorated_children(list) {
      let node = decorated.node;
      if node.kind() != "field_declaration" {
        continue;
      }
      let Some(name) = name_of(doc, node) else { continue };
      let item_path = vec![owner.to_owned(), name.0.clone()];
      let mut symbol = self.symbol(doc, &decorated, EntityKind::Field, name, item_path, container);
      symbol.parent = Some(owner_local);
      symbol.visibility = visibility(doc, node);
      symbol.signature = Some(signature(doc, node, node.end_byte()));
      let local = self.add(symbol);
      if let Some(field_type) = node.child_by_field_name("type") {
        self.visit_type(Cx::new(doc, Some(local)), field_type);
      }
    }
  }

  fn enumeration(&mut self, doc: Doc<'_>, decorated: &Decorated<'_>, container: &Container) {
    let node = decorated.node;
    let Some(name) = name_of(doc, node) else { return };
    let enum_name = name.0.clone();
    let body = node.child_by_field_name("body");
    let enum_visibility = visibility(doc, node);
    let mut symbol = self.symbol(doc, decorated, EntityKind::Enum, name, vec![enum_name.clone()], container);
    symbol.visibility = enum_visibility.clone();
    symbol.modifiers = item_modifiers(node);
    symbol.signature = Some(signature(doc, node, body.map_or(node.end_byte(), |body| body.start_byte())));
    let local = self.add(symbol);
    self.generics.push(generic_names(doc, node));
    self.header_types(Cx::new(doc, Some(local)), node);
    for variant in body.map(decorated_children).unwrap_or_default() {
      let variant_node = variant.node;
      if variant_node.kind() != "enum_variant" {
        continue;
      }
      let Some(variant_name) = name_of(doc, variant_node) else { continue };
      let item_path = vec![enum_name.clone(), variant_name.0.clone()];
      let mut symbol = self.symbol(doc, &variant, EntityKind::Variant, variant_name, item_path, container);
      symbol.parent = Some(local);
      symbol.visibility = enum_visibility.clone();
      symbol.signature = Some(signature(doc, variant_node, variant_node.end_byte()));
      let variant_local = self.add(symbol);
      let cx = Cx::new(doc, Some(variant_local));
      if let Some(payload) = variant_node.child_by_field_name("body") {
        self.visit_type(cx, payload);
      }
      if let Some(value) = variant_node.child_by_field_name("value") {
        self.visit_expr(cx, value);
      }
    }
    self.generics.pop();
  }

  fn trait_item(&mut self, doc: Doc<'_>, decorated: &Decorated<'_>, container: &Container) {
    let node = decorated.node;
    let Some(name) = name_of(doc, node) else { return };
    let trait_name = name.0.clone();
    let body = node.child_by_field_name("body");
    let trait_visibility = visibility(doc, node);
    let mut symbol = self.symbol(doc, decorated, EntityKind::Trait, name, vec![trait_name.clone()], container);
    symbol.visibility = trait_visibility.clone();
    symbol.modifiers = item_modifiers(node);
    symbol.signature = Some(signature(doc, node, body.map_or(node.end_byte(), |body| body.start_byte())));
    symbol.extends = node
      .child_by_field_name("bounds")
      .map(|bounds| named_children(bounds).into_iter().filter_map(|bound| path_segments(doc, bound)).collect())
      .unwrap_or_default();
    let local = self.add(symbol);
    let cx = Cx::new(doc, Some(local));
    self.generics.push(generic_names(doc, node));
    self.header_types(cx, node);
    if let Some(bounds) = node.child_by_field_name("bounds") {
      self.visit_type(cx, bounds);
    }
    let scope = Scope {
      role: Role::Trait,
      prefix: vec![trait_name],
      parent: Some(local),
      impl_of: None,
      visibility: Some(trait_visibility),
    };
    if let Some(body) = body {
      self.members(doc, body, container, &scope, cx);
    }
    self.generics.pop();
  }

  fn impl_block(&mut self, doc: Doc<'_>, decorated: &Decorated<'_>, container: &Container) {
    let node = decorated.node;
    let Some(self_node) = node.child_by_field_name("type") else { return };
    let trait_node = node.child_by_field_name("trait");
    let Some(info) = self.push_impl(doc, node, &decorated.attributes, decorated.start, container.module_suffix.clone(), container.parent) else {
      return;
    };
    let header = Cx::new(doc, container.parent);
    self.generics.push(generic_names(doc, node));
    if let Some(trait_node) = trait_node {
      self.visit_type(header, trait_node);
    }
    self.visit_type(header, self_node);
    self.header_types(header, node);
    let scope = Scope {
      role: Role::Impl,
      prefix: info.item_prefix(),
      parent: container.parent,
      visibility: info.trait_ref.as_ref().map(|_| Visibility::Public),
      impl_of: Some(info),
    };
    if let Some(body) = node.child_by_field_name("body") {
      self.members(doc, body, container, &scope, header);
    }
    self.generics.pop();
  }

  fn members(&mut self, doc: Doc<'_>, body: Node<'_>, container: &Container, scope: &Scope, cx: Cx<'_>) {
    for decorated in decorated_children(body) {
      let node = decorated.node;
      match node.kind() {
        "function_item" | "function_signature_item" => self.function(doc, &decorated, container, scope),
        "const_item" => self.constant(doc, &decorated, container, scope),
        "type_item" | "associated_type" => self.type_alias(doc, &decorated, container, scope),
        "macro_invocation" => self.macro_call(cx, node),
        _ => {}
      }
    }
  }

  fn header_types(&mut self, cx: Cx<'_>, node: Node<'_>) {
    if let Some(parameters) = node.child_by_field_name("type_parameters") {
      self.visit_type(cx, parameters);
    }
    for clause in children(node).into_iter().filter(|child| child.kind() == "where_clause") {
      self.visit_type(cx, clause);
    }
  }

  fn type_alias(&mut self, doc: Doc<'_>, decorated: &Decorated<'_>, container: &Container, scope: &Scope) {
    let node = decorated.node;
    let Some(name) = name_of(doc, node) else { return };
    let name_id = name.1.id();
    let item_path = scope.item_path(&name.0);
    let mut symbol = self.symbol(doc, decorated, EntityKind::TypeAlias, name, item_path, container);
    symbol.parent = scope.parent;
    symbol.visibility = scope.visibility.clone().unwrap_or_else(|| visibility(doc, node));
    symbol.modifiers = item_modifiers(node);
    symbol.signature = Some(signature(doc, node, node.end_byte()));
    symbol.impl_of = scope.impl_of.clone();
    let local = self.add(symbol);
    let cx = Cx::new(doc, Some(local));
    self.generics.push(generic_names(doc, node));
    for child in named_children(node).into_iter().filter(|child| child.id() != name_id) {
      self.visit_type(cx, child);
    }
    self.generics.pop();
  }

  fn constant(&mut self, doc: Doc<'_>, decorated: &Decorated<'_>, container: &Container, scope: &Scope) {
    let node = decorated.node;
    let value = node.child_by_field_name("value");
    let cx = match name_of(doc, node) {
      Some(name) => {
        let item_path = scope.item_path(&name.0);
        let mut symbol = self.symbol(doc, decorated, EntityKind::Constant, name, item_path, container);
        symbol.parent = scope.parent;
        symbol.visibility = scope.visibility.clone().unwrap_or_else(|| visibility(doc, node));
        symbol.modifiers = item_modifiers(node);
        if node.kind() == "static_item" {
          symbol.modifiers.insert(Modifier::Static);
        }
        if scope.role == Role::Foreign {
          symbol.modifiers.insert(Modifier::Extern);
        }
        let equals = children(node).into_iter().find(|child| child.kind() == "=");
        symbol.signature = Some(signature(doc, node, equals.map_or(node.end_byte(), |equals| equals.start_byte())));
        symbol.impl_of = scope.impl_of.clone();
        symbol.body = value.map(|value| body_fingerprint(value, self.span(doc, value).line_count()));
        Cx::new(doc, Some(self.add(symbol)))
      }
      None => Cx::new(doc, scope.parent),
    };
    if let Some(declared) = node.child_by_field_name("type") {
      self.visit_type(cx, declared);
    }
    if let Some(value) = value {
      self.locals.push(Vec::new());
      self.visit_expr(cx, value);
      self.locals.pop();
    }
  }

  fn module(&mut self, doc: Doc<'_>, decorated: &Decorated<'_>, container: &Container) {
    let node = decorated.node;
    let Some(name) = name_of(doc, node) else { return };
    let module_name = name.0.clone();
    let body = node.child_by_field_name("body");
    let mut symbol = self.symbol(doc, decorated, EntityKind::Module, name, Vec::new(), container);
    symbol.module_suffix.push(module_name.clone());
    if let Some(body) = body {
      let inner: Vec<Node<'_>> = children(body).into_iter().filter(|child| child.kind() == "inner_attribute_item").collect();
      symbol.attributes.extend(self.attributes(doc, &inner));
      let inner_doc = inner_docs(body).into_iter().flat_map(|comment| doc_lines(doc, comment)).collect();
      symbol.doc = documentation(doc, &decorated.docs, &symbol.attributes, inner_doc);
    }
    symbol.visibility = visibility(doc, node);
    symbol.signature = Some(signature(doc, node, body.map_or(node.end_byte(), |body| body.start_byte())));
    if symbol.attributes.iter().any(is_cfg_test) {
      symbol.test = Some(TestInfo {
        framework: "rust".to_owned(),
        kind: TestKind::Suite,
        display_name: None,
      });
    }
    let declaration = ModuleDeclFact {
      name: module_name,
      body: ModuleBody::External,
      path_attr: symbol.attributes.iter().find_map(path_attribute),
      module_suffix: container.module_suffix.clone(),
      visibility: symbol.visibility.clone(),
      attributes: symbol.attributes.clone(),
      span: symbol.span,
    };
    let inline_suffix = symbol.module_suffix.clone();
    let local = self.add(symbol);
    let Some(body) = body else {
      self.facts.module_decls.push(declaration);
      return;
    };
    self.facts.module_decls.push(ModuleDeclFact {
      body: ModuleBody::Inline(local),
      ..declaration
    });
    let inline = Container {
      module_suffix: inline_suffix,
      parent: Some(local),
      depth: container.depth + 1,
    };
    self.items(doc, body, &inline);
  }

  fn macro_definition(&mut self, doc: Doc<'_>, decorated: &Decorated<'_>, container: &Container) {
    let node = decorated.node;
    let Some(name) = name_of(doc, node) else { return };
    let signature = format!("macro_rules! {}", name.0);
    let item_path = vec![name.0.clone()];
    let mut symbol = self.symbol(doc, decorated, EntityKind::Macro, name, item_path, container);
    if symbol.attributes.iter().any(|attribute| attribute.path == ["macro_export"]) {
      symbol.visibility = Visibility::Public;
    }
    symbol.signature = Some(signature);
    self.add(symbol);
  }

  fn foreign_items(&mut self, doc: Doc<'_>, node: Node<'_>, container: &Container) {
    let Some(body) = node.child_by_field_name("body") else { return };
    let scope = Scope::module(container, Role::Foreign);
    for decorated in decorated_children(body) {
      match decorated.node.kind() {
        "function_signature_item" | "function_item" => self.function(doc, &decorated, container, &scope),
        "static_item" | "const_item" => self.constant(doc, &decorated, container, &scope),
        _ => {}
      }
    }
  }

  pub(crate) fn use_declaration(&mut self, doc: Doc<'_>, node: Node<'_>, scope: Option<LocalSymbolId>) {
    let Some(argument) = node.child_by_field_name("argument") else { return };
    let mut paths = Vec::new();
    flatten_use(doc, argument, &[], &mut paths, 0);
    if paths.is_empty() {
      return;
    }
    self.facts.imports.push(ImportFact::RustUse {
      paths,
      visibility: visibility(doc, node),
      scope,
      span: self.span(doc, node),
    });
  }

  fn extern_crate(&mut self, doc: Doc<'_>, node: Node<'_>) {
    let Some(name) = node.child_by_field_name("name").map(|name| doc.identifier(name)).filter(|name| !name.is_empty()) else {
      return;
    };
    self.facts.imports.push(ImportFact::RustExternCrate {
      name: name.to_owned(),
      alias: node.child_by_field_name("alias").map(|alias| doc.identifier(alias).to_owned()),
      span: self.span(doc, node),
    });
  }

  fn link_impls(&mut self) {
    let mut implementors: HashMap<(&[String], &str), Vec<usize>> = HashMap::new();
    for (index, symbol) in self.facts.symbols.iter().enumerate() {
      if let ([name], EntityKind::Struct | EntityKind::Enum | EntityKind::TypeAlias) = (symbol.item_path.as_slice(), &symbol.kind) {
        implementors.entry((symbol.module_suffix.as_slice(), name.as_str())).or_default().push(index);
      }
    }
    let mut links = Vec::new();
    for implementation in &self.facts.impls {
      let (Some(trait_ref), [self_name]) = (&implementation.trait_ref, implementation.self_type.segments.as_slice()) else {
        continue;
      };
      let module_level = implementation
        .scope
        .and_then(|local| self.facts.symbol(local))
        .is_none_or(|scope| scope.kind == EntityKind::Module);
      if !module_level || implementation.generic_params.contains(self_name) {
        continue;
      }
      for index in implementors.get(&(implementation.module_suffix.as_slice(), self_name.as_str())).into_iter().flatten() {
        links.push((*index, trait_ref.segments.clone()));
      }
    }
    for (index, trait_path) in links {
      let implements = &mut self.facts.symbols[index].implements;
      if !implements.contains(&trait_path) {
        implements.push(trait_path);
      }
    }
  }

  fn dedupe_type_refs(&mut self) {
    let mut seen = HashSet::new();
    self
      .facts
      .references
      .retain(|reference| reference.kind != RefKind::TypeRef || seen.insert((reference.from, reference.path.clone())));
  }
}

impl Container {
  fn nested(&self) -> Self {
    Self {
      depth: self.depth + 1,
      ..self.clone()
    }
  }
}

pub(crate) fn generic_names(doc: Doc<'_>, node: Node<'_>) -> Vec<String> {
  let Some(parameters) = node.child_by_field_name("type_parameters") else {
    return Vec::new();
  };
  named_children(parameters)
    .into_iter()
    .filter_map(|parameter| match parameter.kind() {
      "type_parameter" | "const_parameter" => parameter.child_by_field_name("name"),
      "type_identifier" => Some(parameter),
      _ => None,
    })
    .map(|name| doc.identifier(name).to_owned())
    .filter(|name| !name.is_empty())
    .collect()
}

fn documentation(doc: Doc<'_>, comments: &[Node<'_>], attributes: &[AttributeFact], inner: Vec<String>) -> Option<String> {
  let mut lines: Vec<String> = comments.iter().flat_map(|comment| doc_lines(doc, *comment)).collect();
  for attribute in attributes.iter().filter(|attribute| attribute.path == ["doc"]) {
    if let [ArgSummary::Str(text)] = attribute.args.as_slice() {
      lines.extend(text.lines().map(|line| line.strip_prefix(' ').unwrap_or(line).to_owned()));
    }
  }
  lines.extend(inner);
  join_doc(lines)
}

fn is_cfg_test(attribute: &AttributeFact) -> bool {
  attribute.path == ["cfg"] && attribute.args.iter().any(is_test_predicate)
}

fn is_test_predicate(predicate: &ArgSummary) -> bool {
  match predicate {
    ArgSummary::Path(path) => path == &["test"],
    ArgSummary::Call { callee, args } => callee == &["all"] && args.iter().any(is_test_predicate),
    _ => false,
  }
}

fn path_attribute(attribute: &AttributeFact) -> Option<String> {
  match (attribute.path.as_slice(), attribute.args.as_slice()) {
    ([name], [ArgSummary::Str(path)]) if name == "path" => Some(path.clone()),
    _ => None,
  }
}

fn flatten_use(doc: Doc<'_>, node: Node<'_>, prefix: &[String], paths: &mut Vec<UsePath>, depth: u32) {
  if depth >= MAX_USE_NESTING {
    return;
  }
  match node.kind() {
    "use_as_clause" => {
      let segments = node.child_by_field_name("path").and_then(|path| use_segments(doc, path, prefix));
      let alias = node.child_by_field_name("alias").map(|alias| doc.identifier(alias)).filter(|alias| !alias.is_empty());
      if let (Some(segments), Some(alias)) = (segments, alias) {
        paths.push(UsePath {
          segments,
          binding: UseBinding::Alias(alias.to_owned()),
        });
      }
    }
    "use_wildcard" => {
      let segments = match named_children(node).into_iter().next() {
        Some(path) => use_segments(doc, path, prefix),
        None => Some(prefix.to_vec()),
      };
      if let Some(segments) = segments.filter(|segments| !segments.is_empty()) {
        paths.push(UsePath {
          segments,
          binding: UseBinding::Glob,
        });
      }
    }
    "scoped_use_list" => {
      let base = match node.child_by_field_name("path") {
        Some(path) => use_segments(doc, path, prefix),
        None => Some(prefix.to_vec()),
      };
      if let (Some(base), Some(list)) = (base, node.child_by_field_name("list")) {
        flatten_use(doc, list, &base, paths, depth + 1);
      }
    }
    "use_list" => {
      for child in named_children(node) {
        flatten_use(doc, child, prefix, paths, depth + 1);
      }
    }
    _ => {
      if let Some(segments) = use_segments(doc, node, prefix) {
        paths.push(UsePath {
          segments,
          binding: UseBinding::Name,
        });
      }
    }
  }
}

fn use_segments(doc: Doc<'_>, node: Node<'_>, prefix: &[String]) -> Option<Vec<String>> {
  if node.kind() == "self" && !prefix.is_empty() {
    return Some(prefix.to_vec());
  }
  let segments = path_segments(doc, node)?;
  Some(prefix.iter().cloned().chain(segments).collect())
}

fn parse_status(root: Node<'_>, lines: &LineIndex) -> ParseStatus {
  if !root.has_error() {
    return ParseStatus::Ok;
  }
  let mut error_count: u32 = 0;
  let mut first_errors = Vec::new();
  walk_tree(root, |node| {
    let message = if node.is_error() {
      "syntax error".to_owned()
    } else if node.is_missing() {
      format!("missing `{}`", node.kind())
    } else {
      return if node.has_error() { Walk::Descend } else { Walk::Skip };
    };
    error_count = error_count.saturating_add(1);
    if first_errors.len() < MAX_REPORTED_PARSE_ERRORS {
      first_errors.push(ParseErrorFact {
        span: lines.span(node.start_byte(), node.end_byte()),
        message,
      });
    }
    Walk::Skip
  });
  ParseStatus::Partial { error_count, first_errors }
}

fn declared_type_names(doc: Doc<'_>, root: Node<'_>) -> HashSet<String> {
  let mut names = HashSet::new();
  walk_tree(root, |node| {
    if matches!(node.kind(), "struct_item" | "enum_item" | "union_item" | "type_item")
      && let Some((name, _)) = name_of(doc, node)
    {
      names.insert(name);
    }
    Walk::Descend
  });
  names
}
