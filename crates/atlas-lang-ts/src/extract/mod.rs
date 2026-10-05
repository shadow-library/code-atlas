mod classes;
mod declarations;
mod references;
mod test_symbols;

use std::collections::{HashMap, HashSet};

use atlas_core::{
  ArgSummary, AttributeFact, Deadline, EntityKind, ExportFact, FileFacts, FileRef, ImportBinding, ImportFact, LocalSymbolId, Producer, ReceiverHint, RefKind, RefOrigin,
  ReferenceFact, Span, SymbolFact, Visibility, is_test_path,
};
use tree_sitter::Node;

use self::references::{IGNORED_GLOBALS, MAX_VALUE_SPANS};
use crate::args::summarize_arguments;
use crate::modules::{import_statement, require_call};
use crate::scope::{Namespace, ScopeKind, Scopes};
use crate::syntax::{children, first_child, member_chain, span_of, text};
use crate::testing::{UNKNOWN_FRAMEWORK, framework_of_global, framework_of_import};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
  Module,
  Namespace(LocalSymbolId),
  Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThisBinding {
  EnclosingClass,
  Rebound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Export {
  None,
  Named,
  Default,
}

impl Export {
  fn visibility(self) -> Visibility {
    match self {
      Self::None => Visibility::Module,
      Self::Named => Visibility::Exported,
      Self::Default => Visibility::ExportedDefault,
    }
  }
}

#[derive(Debug, Clone, Copy)]
struct Site<'t> {
  export: Export,
  outer: Node<'t>,
  ambient: bool,
}

impl<'t> Site<'t> {
  fn plain(node: Node<'t>) -> Self {
    Self {
      export: Export::None,
      outer: node,
      ambient: false,
    }
  }
}

#[derive(Debug, Clone, Copy)]
struct Saved {
  owner: Option<LocalSymbolId>,
  container: Container,
  this_binding: ThisBinding,
  scopes: usize,
  suites: usize,
  in_error: bool,
}

#[derive(Debug)]
enum Step<'t> {
  Visit(Node<'t>),
  Member {
    node: Node<'t>,
    parent: LocalSymbolId,
    decorators: Vec<Node<'t>>,
  },
  Owner(Option<LocalSymbolId>),
  Restore(Saved),
}

#[derive(Debug, Default)]
struct ModuleInfo {
  namespaces: Vec<String>,
  foreign_imports: Vec<String>,
  shadowed_globals: Vec<String>,
  framework: Option<&'static str>,
}

impl ModuleInfo {
  fn scan(root: Node<'_>, source: &str) -> Self {
    let mut info = Self::default();
    let statements = children(root);
    let mut declared = Scopes::default();
    declared.push(ScopeKind::Block);
    declared.declare_statements(&statements, source);
    info
      .shadowed_globals
      .extend(IGNORED_GLOBALS.iter().filter(|name| declared.is_local_value(name)).map(|name| (*name).to_owned()));
    for statement in statements {
      match statement.kind() {
        "import_statement" => info.record_import(import_statement(statement, source)),
        "lexical_declaration" | "variable_declaration" => {
          for declarator in children(statement) {
            let (Some(name), Some(value)) = (declarator.child_by_field_name("name"), declarator.child_by_field_name("value")) else {
              continue;
            };
            if name.kind() == "identifier" && require_call(value, source).is_some() {
              info.namespaces.push(text(name, source).to_owned());
            }
          }
        }
        _ => {}
      }
    }
    info
  }

  fn record_import(&mut self, fact: Option<ImportFact>) {
    match fact {
      Some(ImportFact::TsImport { specifier, bindings, .. }) => {
        self
          .shadowed_globals
          .extend(bindings.iter().map(ImportBinding::local).filter(|local| IGNORED_GLOBALS.contains(local)).map(str::to_owned));
        let framework = framework_of_import(&specifier);
        if framework.is_some() && self.framework.is_none_or(|known| known == UNKNOWN_FRAMEWORK) {
          self.framework = framework;
        }
        for binding in bindings {
          match binding {
            ImportBinding::Namespace { local } => self.namespaces.push(local),
            other if framework.is_none() => self.foreign_imports.push(other.local().to_owned()),
            _ => {}
          }
        }
      }
      Some(ImportFact::TsRequire { bindings, .. }) => {
        self
          .shadowed_globals
          .extend(bindings.iter().map(ImportBinding::local).filter(|local| IGNORED_GLOBALS.contains(local)).map(str::to_owned));
        self.namespaces.extend(bindings.iter().map(|binding| binding.local().to_owned()));
      }
      _ => {}
    }
  }
}

type ValueKey = (Option<LocalSymbolId>, Vec<String>, Option<ReceiverHint>);

pub(crate) struct Extractor<'s, 't> {
  source: &'s str,
  facts: FileFacts,
  scopes: Scopes<'s>,
  owner: Option<LocalSymbolId>,
  container: Container,
  this_binding: ThisBinding,
  suites: Vec<String>,
  in_error: bool,
  stack: Vec<Step<'t>>,
  module: ModuleInfo,
  tests_enabled: bool,
  test_symbols: Vec<LocalSymbolId>,
  callees: HashSet<usize>,
  seen_values: HashMap<ValueKey, Vec<u32>>,
}

impl<'s, 't> Extractor<'s, 't> {
  pub(crate) fn run(file: FileRef, producer: Producer, source: &'s str, root: Node<'t>, deadline: &Deadline) -> Option<FileFacts> {
    let module = ModuleInfo::scan(root, source);
    let tests_enabled = is_test_path(file.path.as_str()) || module.framework.is_some();
    let mut extractor = Self {
      source,
      facts: FileFacts::new(file, producer),
      scopes: Scopes::default(),
      owner: None,
      container: Container::Module,
      this_binding: ThisBinding::EnclosingClass,
      suites: Vec::new(),
      in_error: false,
      stack: vec![Step::Visit(root)],
      module,
      tests_enabled,
      test_symbols: Vec::new(),
      callees: HashSet::new(),
      seen_values: HashMap::new(),
    };
    while let Some(step) = extractor.stack.pop() {
      if deadline.tick().is_break() {
        return None;
      }
      match step {
        Step::Visit(node) => extractor.visit(node),
        Step::Member { node, parent, decorators } => extractor.member(node, parent, decorators),
        Step::Owner(owner) => extractor.owner = owner,
        Step::Restore(saved) => extractor.restore(saved),
      }
    }
    Some(extractor.finish())
  }

  fn visit(&mut self, node: Node<'t>) {
    let kind = node.kind();
    if self.in_error && is_declaration(kind) && !node.has_error() {
      self.checkpoint();
      self.in_error = false;
    }
    match kind {
      "comment" | "jsx_closing_element" | "hash_bang_line" | "type_query" => {}
      "ERROR" => {
        self.checkpoint();
        self.in_error = true;
        self.push_all(children(node));
      }
      "import_statement" => self.facts.imports.extend(import_statement(node, self.source)),
      "export_statement" => self.export_statement(node),
      _ if self.declares_symbols() && is_declaration(kind) => self.declaration(node, Site::plain(node), Vec::new()),
      "function_declaration"
      | "generator_function_declaration"
      | "function_signature"
      | "arrow_function"
      | "function_expression"
      | "function"
      | "generator_function"
      | "method_definition"
      | "method_signature" => self.function_scope(node, FunctionScope::default()),
      "call_signature" | "construct_signature" | "function_type" | "constructor_type" => self.type_scope(node),
      "class_declaration" | "abstract_class_declaration" | "class" => self.local_class(node),
      "interface_declaration" | "type_alias_declaration" => self.type_scope(node),
      "lexical_declaration" | "variable_declaration" => self.local_variables(node),
      "statement_block" => self.block(children(node)),
      "switch_body" => self.switch_body(node),
      "for_statement" | "for_in_statement" => self.for_loop(node),
      "catch_clause" => self.catch_clause(node),
      "call_expression" => self.call(node),
      "new_expression" => self.instantiation(node),
      "assignment_expression" => self.assignment(node),
      "decorator" => self.decorator(node),
      "jsx_opening_element" | "jsx_self_closing_element" => self.jsx_element(node),
      "jsx_expression" => self.jsx_expression(node),
      "identifier" | "shorthand_property_identifier" => self.value_read(node),
      "member_expression" | "subscript_expression" => self.member_read(node),
      "type_identifier" => self.type_identifier(node),
      "nested_type_identifier" => self.nested_type(node),
      "type_parameter" => self.push_all(children(node).into_iter().filter(|child| Some(*child) != node.child_by_field_name("name"))),
      "infer_type" => self.infer_type(node),
      "mapped_type_clause" => self.mapped_type(node),
      "enum_declaration" | "internal_module" | "module" | "ambient_declaration" => self.push_except_name(node),
      _ => self.push_all(children(node)),
    }
  }

  fn declares_symbols(&self) -> bool {
    self.container != Container::Local
  }

  fn container_parent(&self) -> Option<LocalSymbolId> {
    match self.container {
      Container::Namespace(namespace) => Some(namespace),
      Container::Module | Container::Local => None,
    }
  }

  fn save(&self) -> Saved {
    Saved {
      owner: self.owner,
      container: self.container,
      this_binding: self.this_binding,
      scopes: self.scopes.len(),
      suites: self.suites.len(),
      in_error: self.in_error,
    }
  }

  fn checkpoint(&mut self) {
    let saved = self.save();
    self.stack.push(Step::Restore(saved));
  }

  fn restore(&mut self, saved: Saved) {
    self.owner = saved.owner;
    self.container = saved.container;
    self.this_binding = saved.this_binding;
    self.scopes.truncate(saved.scopes);
    self.suites.truncate(saved.suites);
    self.in_error = saved.in_error;
  }

  fn push_all(&mut self, nodes: impl IntoIterator<Item = Node<'t>>) {
    let nodes: Vec<Node<'t>> = nodes.into_iter().collect();
    self.stack.extend(nodes.into_iter().rev().map(Step::Visit));
  }

  fn push_steps(&mut self, steps: Vec<Step<'t>>) {
    self.stack.extend(steps.into_iter().rev());
  }

  fn push_except_name(&mut self, node: Node<'t>) {
    let name = node.child_by_field_name("name");
    self.push_all(children(node).into_iter().filter(|child| Some(*child) != name));
  }

  fn add_symbol(&mut self, kind: EntityKind, name: String, item_path: Vec<String>, parent: Option<LocalSymbolId>, spans: (Span, Span), visibility: Visibility) -> LocalSymbolId {
    let local = self.facts.next_local_id();
    let mut symbol = SymbolFact::new(local, kind, name, item_path, spans.0, spans.1, visibility);
    symbol.parent = parent;
    self.facts.symbols.push(symbol);
    local
  }

  fn child_path(&self, parent: Option<LocalSymbolId>, name: &str) -> Vec<String> {
    let mut path = parent
      .and_then(|parent| self.facts.symbol(parent))
      .map(|parent| parent.item_path.clone())
      .unwrap_or_default();
    path.push(name.to_owned());
    path
  }

  fn symbol_mut(&mut self, local: LocalSymbolId) -> Option<&mut SymbolFact> {
    self.facts.symbols.get_mut(local.index())
  }

  fn attribute(&self, decorator: Node<'_>) -> Option<AttributeFact> {
    let expression = first_child(decorator)?;
    let (target, arguments) = match expression.kind() {
      "call_expression" => (expression.child_by_field_name("function")?, expression.child_by_field_name("arguments")),
      _ => (expression, None),
    };
    let path = member_chain(target, self.source).static_path(self.source)?;
    Some(AttributeFact {
      path,
      args: summarize_arguments(arguments, self.source),
      span: span_of(decorator),
    })
  }

  fn reference(&mut self, kind: RefKind, path: Vec<String>, receiver: Option<ReceiverHint>, args: Vec<ArgSummary>, span: Span) {
    if self.in_error || path.is_empty() || path.iter().any(String::is_empty) {
      return;
    }
    if kind == RefKind::ValueRef && !self.admit_value(&path, receiver.as_ref(), span.start_byte) {
      return;
    }
    self.facts.references.push(ReferenceFact {
      from: self.owner,
      kind,
      path,
      receiver,
      args,
      span,
      origin: RefOrigin::Syntax,
    });
  }

  fn admit_value(&mut self, path: &[String], receiver: Option<&ReceiverHint>, start: u32) -> bool {
    let spans = self.seen_values.entry((self.owner, path.to_vec(), receiver.cloned())).or_default();
    if spans.len() >= MAX_VALUE_SPANS || spans.contains(&start) {
      return false;
    }
    spans.push(start);
    true
  }

  fn block(&mut self, statements: Vec<Node<'t>>) {
    self.checkpoint();
    self.container = Container::Local;
    self.scopes.push(ScopeKind::Block);
    self.scopes.declare_statements(&statements, self.source);
    self.push_all(statements);
  }

  fn switch_body(&mut self, node: Node<'t>) {
    let cases = children(node);
    let statements: Vec<Node<'t>> = cases.iter().flat_map(|case| children(*case)).collect();
    self.checkpoint();
    self.container = Container::Local;
    self.scopes.push(ScopeKind::Block);
    self.scopes.declare_statements(&statements, self.source);
    self.push_all(cases);
  }

  fn for_loop(&mut self, node: Node<'t>) {
    self.checkpoint();
    self.container = Container::Local;
    self.scopes.push(ScopeKind::Block);
    if let Some(initializer) = node.child_by_field_name("initializer") {
      self.scopes.declare_statements(&[initializer], self.source);
    }
    if let (Some(left), Some(keyword)) = (node.child_by_field_name("left"), node.child_by_field_name("kind")) {
      match text(keyword, self.source) {
        "var" => self.bind_var(left),
        _ => self.scopes.bind_pattern(left, self.source, None),
      }
    }
    self.push_all(children(node));
  }

  fn catch_clause(&mut self, node: Node<'t>) {
    self.checkpoint();
    self.container = Container::Local;
    self.scopes.push(ScopeKind::Block);
    if let Some(parameter) = node.child_by_field_name("parameter") {
      self.scopes.bind_pattern(parameter, self.source, None);
    }
    self.push_all(children(node));
  }

  fn bind_var(&mut self, pattern: Node<'_>) {
    for identifier in crate::scope::pattern_identifiers(pattern) {
      self.scopes.bind_in_function(text(identifier, self.source), None);
    }
  }

  fn type_scope(&mut self, node: Node<'t>) {
    self.checkpoint();
    self.scopes.push(ScopeKind::Block);
    self.scopes.bind_type_parameters(node, self.source);
    self.push_except_name(node);
  }

  fn infer_type(&mut self, node: Node<'t>) {
    let mut parts = children(node).into_iter();
    if let Some(name) = parts.next() {
      self.scopes.bind(text(name, self.source), Namespace::Type, None);
    }
    self.push_all(parts);
  }

  fn mapped_type(&mut self, node: Node<'t>) {
    if let Some(name) = node.child_by_field_name("name") {
      self.scopes.bind(text(name, self.source), Namespace::Type, None);
    }
    self.push_except_name(node);
  }

  fn finish(mut self) -> FileFacts {
    self.upgrade_exported_visibility();
    self.assign_test_framework();
    self.facts
  }

  fn upgrade_exported_visibility(&mut self) {
    let mut exported: HashMap<String, Visibility> = HashMap::new();
    for export in &self.facts.exports {
      let names: Vec<(String, Visibility)> = match export {
        ExportFact::Named { local, exported: name, .. } => {
          let visibility = if name == "default" { Visibility::ExportedDefault } else { Visibility::Exported };
          vec![(local.clone(), visibility)]
        }
        ExportFact::Default {
          target: atlas_core::DefaultExportTarget::Identifier(name),
          ..
        } => vec![(name.clone(), Visibility::ExportedDefault)],
        ExportFact::CommonJs { value, .. } => commonjs_locals(value).into_iter().map(|name| (name, Visibility::Exported)).collect(),
        _ => Vec::new(),
      };
      for (name, visibility) in names {
        exported.entry(name).or_insert(visibility);
      }
    }
    for symbol in &mut self.facts.symbols {
      let upgradable = symbol.parent.is_none() && symbol.visibility == Visibility::Module && !matches!(symbol.kind, EntityKind::Test | EntityKind::TestSuite);
      if let Some(visibility) = exported.get(&symbol.name).filter(|_| upgradable) {
        symbol.visibility = visibility.clone();
      }
    }
  }

  fn assign_test_framework(&mut self) {
    if self.test_symbols.is_empty() {
      return;
    }
    let from_globals = || {
      self.facts.references.iter().find_map(|reference| match &reference.receiver {
        Some(atlas_core::ReceiverHint::Identifier(receiver)) if reference.kind == RefKind::MethodCall => framework_of_global(receiver),
        _ => None,
      })
    };
    let framework = self
      .module
      .framework
      .filter(|framework| *framework != UNKNOWN_FRAMEWORK)
      .or_else(from_globals)
      .unwrap_or(UNKNOWN_FRAMEWORK);
    for local in std::mem::take(&mut self.test_symbols) {
      if let Some(test) = self.symbol_mut(local).and_then(|symbol| symbol.test.as_mut()) {
        test.framework = framework.to_owned();
      }
    }
  }
}

fn commonjs_locals(value: &ArgSummary) -> Vec<String> {
  match value {
    ArgSummary::Path(path) if path.len() == 1 => path.clone(),
    ArgSummary::Object(entries) => entries
      .iter()
      .filter_map(|entry| match &entry.value {
        ArgSummary::Path(path) if path.len() == 1 => path.first().cloned(),
        _ => None,
      })
      .collect(),
    _ => Vec::new(),
  }
}

fn is_declaration(kind: &str) -> bool {
  matches!(
    kind,
    "function_declaration"
      | "generator_function_declaration"
      | "function_signature"
      | "class_declaration"
      | "abstract_class_declaration"
      | "interface_declaration"
      | "type_alias_declaration"
      | "enum_declaration"
      | "internal_module"
      | "module"
      | "lexical_declaration"
      | "variable_declaration"
      | "ambient_declaration"
  )
}

#[derive(Debug, Default)]
struct FunctionScope<'t> {
  owner: Option<LocalSymbolId>,
  decorators: Vec<Node<'t>>,
  property_fields: Vec<(Node<'t>, LocalSymbolId)>,
}
