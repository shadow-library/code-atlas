use std::collections::{HashMap, HashSet};

use atlas_core::{ArgSummary, DeclaredBinding, LocalSymbolId, ReceiverHint, RefKind, RefOrigin, ReferenceFact};
use tree_sitter::Node;

use crate::extract::{Extractor, generic_names};
use crate::macros::{MacroArgs, PieceBody, STD_EXPRESSION_MACROS, macro_token_tree};
use crate::source::{Doc, short_text, signature_text};
use crate::summary::{call_args, expression_children, receiver_chain};
use crate::syntax::{decorated_children, is_comment, name_of, named_children, path_segments};
use crate::tokens::{inner_tokens, token_tree_args, word};

const MAX_NESTING: u32 = 160;
const MAX_MACRO_NESTING: u32 = 4;
const MAX_PATH_WALK: usize = 64;
const MAX_SCAN_TOKENS: usize = 4_096;
const MAX_SCAN_DEPTH: u32 = 8;
const MAX_RECEIVER_TOKENS: usize = 32;
const DECLARATION_KEYWORDS: [&str; 6] = ["impl", "trait", "mod", "type", "macro_rules", "dyn"];
const CLOSURE_TRAITS: [&str; 3] = ["Fn", "FnMut", "FnOnce"];
const PATTERN_FOLLOWERS: [&str; 4] = ["=>", "=", "in", "|"];
const DEREF_WRAPPERS: [&str; 3] = ["Box", "Arc", "Rc"];
const NESTED_SELF_TYPE: &str = "(Self)";
const NEWTYPE_WRAPPERS: [&str; 10] = ["State", "Path", "Json", "Query", "Form", "Extension", "Data", "Some", "Reverse", "Wrapping"];
const PIECEWISE_SYNTAX_MACROS: [&str; 9] = ["info", "debug", "warn", "error", "trace", "event", "anyhow", "bail", "ensure"];
const KEYWORDS: [&str; 36] = [
  "as",
  "async",
  "await",
  "break",
  "const",
  "continue",
  "dyn",
  "else",
  "enum",
  "extern",
  "fn",
  "for",
  "if",
  "impl",
  "in",
  "let",
  "loop",
  "match",
  "mod",
  "move",
  "mut",
  "pub",
  "ref",
  "return",
  "static",
  "struct",
  "trait",
  "type",
  "union",
  "unsafe",
  "use",
  "where",
  "while",
  "macro_rules",
  "yield",
  "box",
];

const TYPE_KINDS: [&str; 18] = [
  "type_identifier",
  "scoped_type_identifier",
  "generic_type",
  "generic_type_with_turbofish",
  "reference_type",
  "pointer_type",
  "array_type",
  "tuple_type",
  "function_type",
  "dynamic_type",
  "abstract_type",
  "bounded_type",
  "qualified_type",
  "bracketed_type",
  "type_arguments",
  "primitive_type",
  "never_type",
  "unit_type",
];

const ITEM_KINDS: [&str; 13] = [
  "function_item",
  "function_signature_item",
  "const_item",
  "static_item",
  "struct_item",
  "union_item",
  "enum_item",
  "trait_item",
  "impl_item",
  "type_item",
  "mod_item",
  "foreign_mod_item",
  "macro_definition",
];

const LEAF_KINDS: [&str; 20] = [
  "attribute_item",
  "inner_attribute_item",
  "line_comment",
  "block_comment",
  "label",
  "lifetime",
  "self",
  "crate",
  "super",
  "metavariable",
  "token_tree",
  "string_literal",
  "raw_string_literal",
  "char_literal",
  "integer_literal",
  "float_literal",
  "boolean_literal",
  "empty_statement",
  "unit_expression",
  "extern_crate_declaration",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueRefs {
  Emit,
  Suppress,
}

#[derive(Clone, Copy)]
pub(crate) struct Cx<'t> {
  doc: Doc<'t>,
  from: Option<LocalSymbolId>,
  values: ValueRefs,
  origin: RefOrigin,
  depth: u32,
  macro_depth: u32,
}

impl<'t> Cx<'t> {
  pub(crate) fn new(doc: Doc<'t>, from: Option<LocalSymbolId>) -> Self {
    Self {
      doc,
      from,
      values: ValueRefs::Emit,
      origin: RefOrigin::Syntax,
      depth: 0,
      macro_depth: 0,
    }
  }
}

#[derive(Default)]
pub(crate) struct Bindings {
  frames: Vec<Vec<String>>,
  scopes: HashMap<String, Vec<Option<ReceiverHint>>>,
}

impl Bindings {
  pub(crate) fn push(&mut self, names: Vec<String>) {
    for name in &names {
      self.scopes.entry(name.clone()).or_default().push(None);
    }
    self.frames.push(names);
  }

  pub(crate) fn pop(&mut self) {
    for name in self.frames.pop().unwrap_or_default() {
      if let Some(scope) = self.scopes.get_mut(&name) {
        scope.pop();
      }
    }
  }

  fn bind(&mut self, name: &str, hint: Option<ReceiverHint>) {
    let Some(frame) = self.frames.last_mut() else { return };
    frame.push(name.to_owned());
    self.scopes.entry(name.to_owned()).or_default().push(hint);
  }

  fn contains(&self, name: &str) -> bool {
    self.scopes.get(name).is_some_and(|scope| !scope.is_empty())
  }

  fn hint_of(&self, name: &str) -> Option<&ReceiverHint> {
    self.scopes.get(name)?.last()?.as_ref()
  }
}

fn last_segment(node: Node<'_>) -> Node<'_> {
  let mut current = node;
  for _ in 0..MAX_PATH_WALK {
    let next = match current.kind() {
      "scoped_identifier" | "scoped_type_identifier" => current.child_by_field_name("name"),
      "generic_type" | "generic_type_with_turbofish" => current.child_by_field_name("type"),
      "generic_function" => current.child_by_field_name("function"),
      _ => None,
    };
    let Some(next) = next else { break };
    current = next;
  }
  current
}

impl Extractor<'_> {
  fn enter<'t>(&mut self, cx: Cx<'t>) -> Option<Cx<'t>> {
    if cx.depth >= MAX_NESTING {
      self.depth_exceeded = true;
      return None;
    }
    Some(Cx { depth: cx.depth + 1, ..cx })
  }

  fn push_ref(&mut self, cx: Cx<'_>, kind: RefKind, mut path: Vec<String>, receiver: Option<ReceiverHint>, args: Vec<ArgSummary>, at: Node<'_>) {
    if self.names_nested_self_type(&path) {
      path[0] = NESTED_SELF_TYPE.to_owned();
    }
    let span = self.span(cx.doc, at);
    self.facts.references.push(ReferenceFact {
      from: cx.from,
      kind,
      path,
      receiver,
      args,
      span,
      origin: cx.origin,
    });
  }

  fn names_nested_self_type(&self, path: &[String]) -> bool {
    self.nested_items > 0 && path.first().is_some_and(|head| head == "Self")
  }

  fn skips_value(&self, name: &str) -> bool {
    name.is_empty() || matches!(name, "self" | "Self" | "_") || self.locals.contains(name) || self.generics.contains(name)
  }

  pub(crate) fn function_refs(&mut self, cx: Cx<'_>, node: Node<'_>) {
    self.generics.push(generic_names(cx.doc, node));
    self.locals.push(Vec::new());
    if let Some(parameters) = node.child_by_field_name("type_parameters") {
      self.visit_type(cx, parameters);
    }
    for parameter in node.child_by_field_name("parameters").map(named_children).unwrap_or_default() {
      if parameter.kind() != "parameter" {
        continue;
      }
      if let Some(declared) = parameter.child_by_field_name("type") {
        self.visit_type(cx, declared);
      }
      if let Some(pattern) = parameter.child_by_field_name("pattern") {
        self.bind_declared(cx, pattern, parameter.child_by_field_name("type"), DeclaredBinding::Param);
      }
    }
    if let Some(returned) = node.child_by_field_name("return_type") {
      self.visit_type(cx, returned);
    }
    for clause in named_children(node).into_iter().filter(|child| child.kind() == "where_clause") {
      self.visit_type(cx, clause);
    }
    if let Some(body) = node.child_by_field_name("body") {
      self.visit_expr(cx, body);
    }
    self.locals.pop();
    self.generics.pop();
  }

  pub(crate) fn visit_type<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    let Some(cx) = self.enter(cx) else { return };
    match node.kind() {
      "type_identifier" | "scoped_type_identifier" => self.type_ref(cx, node),
      "generic_type" => {
        if let Some(base) = node.child_by_field_name("type") {
          self.type_ref(cx, base);
        }
        if let Some(arguments) = node.child_by_field_name("type_arguments") {
          self.visit_type(cx, arguments);
        }
      }
      "type_parameter" | "const_parameter" | "type_binding" | "associated_type" => {
        let name = node.child_by_field_name("name").map(|name| name.id());
        for child in named_children(node).into_iter().filter(|child| Some(child.id()) != name) {
          self.visit_type(cx, child);
        }
      }
      "primitive_type"
      | "lifetime"
      | "lifetime_parameter"
      | "for_lifetimes"
      | "attribute_item"
      | "visibility_modifier"
      | "identifier"
      | "field_identifier"
      | "line_comment"
      | "block_comment"
      | "macro_invocation" => {}
      _ => {
        for child in named_children(node) {
          self.visit_type(cx, child);
        }
      }
    }
  }

  fn type_ref(&mut self, cx: Cx<'_>, node: Node<'_>) {
    let Some(path) = path_segments(cx.doc, node) else { return };
    if path[0] == "Self" || self.generics.contains(&path[0]) {
      return;
    }
    self.visit_path_types(cx, node);
    self.push_ref(cx, RefKind::TypeRef, path, None, Vec::new(), last_segment(node));
  }

  fn visit_path_types<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    let mut current = node.child_by_field_name("path");
    for _ in 0..MAX_PATH_WALK {
      let Some(segment) = current else { return };
      current = match segment.kind() {
        "scoped_identifier" | "scoped_type_identifier" => segment.child_by_field_name("path"),
        "generic_type" | "generic_type_with_turbofish" => {
          if let Some(arguments) = segment.child_by_field_name("type_arguments") {
            self.visit_type(cx, arguments);
          }
          segment.child_by_field_name("type")
        }
        "bracketed_type" => {
          self.visit_type(cx, segment);
          None
        }
        _ => None,
      };
    }
  }

  pub(crate) fn visit_expr<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    let Some(cx) = self.enter(cx) else { return };
    match node.kind() {
      "call_expression" => self.call(cx, node),
      "macro_invocation" => self.macro_call(cx, node),
      "struct_expression" => self.struct_literal(cx, node),
      "identifier" => self.value_identifier(cx, node),
      "scoped_identifier" => self.value_path(cx, node),
      "generic_function" => {
        if let Some(arguments) = node.child_by_field_name("type_arguments") {
          self.visit_type(cx, arguments);
        }
        if let Some(function) = node.child_by_field_name("function") {
          self.visit_expr(cx, function);
        }
      }
      "field_expression" => {
        self.field_access(cx, node);
        if let Some(value) = node.child_by_field_name("value") {
          self.visit_expr(cx, value);
        }
      }
      "closure_expression" => self.closure(cx, node),
      "block" | "unsafe_block" | "async_block" | "const_block" | "gen_block" | "try_block" => self.block(cx, node),
      "let_declaration" => self.let_declaration(cx, node),
      "let_condition" => {
        if let Some(value) = node.child_by_field_name("value") {
          self.visit_expr(cx, value);
        }
        if let Some(pattern) = node.child_by_field_name("pattern") {
          self.bind_pattern(cx, pattern);
        }
      }
      "if_expression" | "while_expression" => {
        self.locals.push(Vec::new());
        self.visit_children(cx, node);
        self.locals.pop();
      }
      "match_expression" => {
        if let Some(value) = node.child_by_field_name("value") {
          self.visit_expr(cx, value);
        }
        for arm in node.child_by_field_name("body").map(named_children).unwrap_or_default() {
          if arm.kind() == "match_arm" {
            self.match_arm(cx, arm);
          }
        }
      }
      "for_expression" => {
        if let Some(value) = node.child_by_field_name("value") {
          self.visit_expr(cx, value);
        }
        self.locals.push(Vec::new());
        if let Some(pattern) = node.child_by_field_name("pattern") {
          self.bind_pattern(cx, pattern);
        }
        if let Some(body) = node.child_by_field_name("body") {
          self.visit_expr(cx, body);
        }
        self.locals.pop();
      }
      "type_cast_expression" => {
        if let Some(value) = node.child_by_field_name("value") {
          self.visit_expr(cx, value);
        }
        if let Some(target) = node.child_by_field_name("type") {
          self.visit_type(cx, target);
        }
      }
      "field_initializer" => {
        if let Some(value) = node.child_by_field_name("value") {
          self.visit_expr(cx, value);
        }
      }
      "shorthand_field_initializer" => {
        if let Some(name) = node.named_child(0) {
          self.value_identifier(cx, name);
        }
      }
      "use_declaration" => self.use_declaration(cx.doc, node, cx.from),
      kind if ITEM_KINDS.contains(&kind) => self.nested_item(cx, node),
      kind if LEAF_KINDS.contains(&kind) => {}
      kind if TYPE_KINDS.contains(&kind) => self.visit_type(cx, node),
      _ => self.visit_children(cx, node),
    }
  }

  fn visit_children<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    for child in named_children(node) {
      self.visit_expr(cx, child);
    }
  }

  fn block<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    let items = named_children(node)
      .into_iter()
      .filter(|child| matches!(child.kind(), "function_item" | "const_item" | "static_item"))
      .filter_map(|item| name_of(cx.doc, item).map(|(name, _)| name))
      .collect();
    self.locals.push(items);
    for decorated in decorated_children(node) {
      match decorated.node.kind() {
        "impl_item" => self.nested_impl(cx, decorated.node, &decorated.attributes, decorated.start),
        _ => self.visit_expr(cx, decorated.node),
      }
    }
    self.locals.pop();
  }

  fn let_declaration<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    if let Some(declared) = node.child_by_field_name("type") {
      self.visit_type(cx, declared);
    }
    if let Some(value) = node.child_by_field_name("value") {
      self.visit_expr(cx, value);
    }
    if let Some(alternative) = node.child_by_field_name("alternative") {
      self.visit_expr(cx, alternative);
    }
    if let Some(pattern) = node.child_by_field_name("pattern") {
      let declared = node.child_by_field_name("type");
      match declared {
        Some(declared) => self.bind_declared(cx, pattern, Some(declared), DeclaredBinding::Let),
        None => {
          self.bind_pattern(cx, pattern);
          let literal = node.child_by_field_name("value").and_then(|value| literal_type(cx.doc, value));
          if let (Some(name), Some(path)) = (plain_binding(cx.doc, pattern), literal) {
            let unknown = self.generics.contains(&path[0]) || self.names_nested_self_type(&path);
            self.locals.bind(name, (!unknown).then_some(ReceiverHint::TypeHint(path)));
          }
        }
      }
    }
  }

  fn bind_declared<'t>(&mut self, cx: Cx<'t>, pattern: Node<'t>, declared: Option<Node<'t>>, binding: DeclaredBinding) {
    self.bind_pattern(cx, pattern);
    if let Some((name, declared)) = declared.and_then(|declared| typed_binding(cx.doc, pattern, declared, &self.local_types)) {
      let hint = self.declared_hint(cx.doc, declared, binding);
      self.locals.bind(name, hint);
    }
  }

  fn declared_hint(&self, doc: Doc<'_>, declared: Node<'_>, binding: DeclaredBinding) -> Option<ReceiverHint> {
    let path = type_hint(doc, declared)?;
    if self.names_nested_self_type(&path) {
      return None;
    }
    let generic = self.generics.contains(&path[0]);
    let loses_information = match path.as_slice() {
      [_] if generic => true,
      _ if generic => return None,
      _ => path.last().is_some_and(|last| DEREF_WRAPPERS.contains(&last.as_str())),
    };
    // The resolver only knows the generics of the symbol a reference is attributed to, not those of fn items nested in its body.
    if !loses_information || self.nested_items > 0 {
      return (!generic).then_some(ReceiverHint::TypeHint(path));
    }
    Some(ReceiverHint::Declared {
      type_text: signature_text(doc.text(declared)),
      binding,
    })
  }

  fn head_hint(&self, head: &str) -> ReceiverHint {
    match head {
      "self" if self.nested_items > 0 => ReceiverHint::Expr(head.to_owned()),
      "self" => ReceiverHint::SelfValue,
      name if self.locals.contains(name) => self.locals.hint_of(name).cloned().unwrap_or_else(|| ReceiverHint::Expr(name.to_owned())),
      name => ReceiverHint::Identifier(name.to_owned()),
    }
  }

  fn field_access(&mut self, cx: Cx<'_>, node: Node<'_>) {
    let (Some(path), Some(field)) = (field_chain(cx.doc, node), node.child_by_field_name("field")) else {
      return;
    };
    let hint = self.head_hint(&path[0]);
    self.push_ref(cx, RefKind::ValueRef, path, Some(hint), Vec::new(), field);
  }

  fn match_arm<'t>(&mut self, cx: Cx<'t>, arm: Node<'t>) {
    self.locals.push(Vec::new());
    let pattern = arm.child_by_field_name("pattern");
    let condition = pattern.and_then(|pattern| pattern.child_by_field_name("condition"));
    if let Some(pattern) = pattern {
      self.bind_pattern(cx, pattern);
    }
    if let Some(condition) = condition {
      self.visit_expr(cx, condition);
    }
    if let Some(value) = arm.child_by_field_name("value") {
      self.visit_expr(cx, value);
    }
    self.locals.pop();
  }

  fn closure<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    self.locals.push(Vec::new());
    for parameter in node.child_by_field_name("parameters").map(named_children).unwrap_or_default() {
      if parameter.kind() != "parameter" {
        self.bind_pattern(cx, parameter);
        continue;
      }
      if let Some(declared) = parameter.child_by_field_name("type") {
        self.visit_type(cx, declared);
      }
      if let Some(pattern) = parameter.child_by_field_name("pattern") {
        self.bind_declared(cx, pattern, parameter.child_by_field_name("type"), DeclaredBinding::Param);
      }
    }
    if let Some(returned) = node.child_by_field_name("return_type") {
      self.visit_type(cx, returned);
    }
    if let Some(body) = node.child_by_field_name("body") {
      self.visit_expr(cx, body);
    }
    self.locals.pop();
  }

  fn bind_pattern<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    let Some(cx) = self.enter(cx) else { return };
    match node.kind() {
      "identifier" | "shorthand_field_identifier" => {
        let name = cx.doc.identifier(node);
        if !name.is_empty() && name != "_" {
          self.locals.bind(name, None);
        }
      }
      "scoped_identifier" => self.pattern_path(cx, node),
      "tuple_struct_pattern" | "struct_pattern" => {
        let type_node = node.child_by_field_name("type");
        if let Some(type_node) = type_node {
          self.pattern_path(cx, type_node);
        }
        for child in named_children(node)
          .into_iter()
          .filter(|child| Some(child.id()) != type_node.map(|type_node| type_node.id()))
        {
          self.bind_pattern(cx, child);
        }
      }
      "field_pattern" => match (node.child_by_field_name("pattern"), node.child_by_field_name("name")) {
        (Some(pattern), _) => self.bind_pattern(cx, pattern),
        (None, Some(name)) => self.bind_pattern(cx, name),
        (None, None) => {}
      },
      "match_pattern" => {
        let condition = node.child_by_field_name("condition").map(|condition| condition.id());
        for child in named_children(node).into_iter().filter(|child| Some(child.id()) != condition) {
          self.bind_pattern(cx, child);
        }
      }
      "range_pattern"
      | "remaining_field_pattern"
      | "negative_literal"
      | "mutable_specifier"
      | "field_identifier"
      | "string_literal"
      | "raw_string_literal"
      | "char_literal"
      | "integer_literal"
      | "float_literal"
      | "boolean_literal"
      | "line_comment"
      | "block_comment" => {}
      _ => {
        for child in named_children(node) {
          self.bind_pattern(cx, child);
        }
      }
    }
  }

  fn pattern_path(&mut self, cx: Cx<'_>, node: Node<'_>) {
    if cx.values == ValueRefs::Suppress {
      return;
    }
    let Some(path) = path_segments(cx.doc, node) else { return };
    if path.len() < 2 || self.generics.contains(&path[0]) {
      return;
    }
    self.push_ref(cx, RefKind::ValueRef, path, None, Vec::new(), last_segment(node));
  }

  fn value_identifier(&mut self, cx: Cx<'_>, node: Node<'_>) {
    let name = cx.doc.identifier(node);
    if cx.values == ValueRefs::Suppress || self.skips_value(name) {
      return;
    }
    self.push_ref(cx, RefKind::ValueRef, vec![name.to_owned()], None, Vec::new(), node);
  }

  fn value_path<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    self.visit_path_types(cx, node);
    if cx.values == ValueRefs::Suppress {
      return;
    }
    let Some(path) = path_segments(cx.doc, node) else { return };
    if path.len() < 2 || self.generics.contains(&path[0]) {
      return;
    }
    self.push_ref(cx, RefKind::ValueRef, path, None, Vec::new(), last_segment(node));
  }

  fn call<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    let arguments = node.child_by_field_name("arguments");
    let args = || arguments.map(|arguments| call_args(cx.doc, arguments)).unwrap_or_default();
    let mut target = node.child_by_field_name("function");
    if let Some(generic) = target.filter(|target| target.kind() == "generic_function") {
      if let Some(turbofish) = generic.child_by_field_name("type_arguments") {
        self.visit_type(cx, turbofish);
      }
      target = generic.child_by_field_name("function");
    }
    match target {
      Some(function) if function.kind() == "identifier" => {
        let name = cx.doc.identifier(function);
        if !name.is_empty() && !self.locals.contains(name) {
          self.push_ref(cx, RefKind::Call, vec![name.to_owned()], None, args(), function);
        }
      }
      Some(function) if function.kind() == "scoped_identifier" => {
        self.visit_path_types(cx, function);
        if let Some(path) = path_segments(cx.doc, function) {
          self.push_ref(cx, RefKind::Call, path, None, args(), last_segment(function));
        }
      }
      Some(function) if function.kind() == "field_expression" => self.method_call(cx, function, args()),
      Some(function) => self.visit_expr(cx, function),
      None => {}
    }
    for argument in arguments.map(expression_children).unwrap_or_default() {
      self.visit_expr(cx, argument);
    }
  }

  fn method_call<'t>(&mut self, cx: Cx<'t>, function: Node<'t>, args: Vec<ArgSummary>) {
    let Some(receiver) = function.child_by_field_name("value") else { return };
    self.visit_expr(cx, receiver);
    let Some(method) = function.child_by_field_name("field") else { return };
    let name = cx.doc.identifier(method);
    if name.is_empty() {
      return;
    }
    let (mut path, hint) = match receiver_chain(cx.doc, receiver) {
      Some(chain) => {
        let hint = self.head_hint(&chain[0]);
        (chain, hint)
      }
      None => {
        let text = short_text(cx.doc.text(receiver));
        (vec![format!("({text})")], ReceiverHint::Expr(text))
      }
    };
    path.push(name.to_owned());
    self.push_ref(cx, RefKind::MethodCall, path, Some(hint), args, method);
  }

  fn struct_literal<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    if let Some(name) = node.child_by_field_name("name") {
      self.visit_path_types(cx, name);
      if let Some(path) = path_segments(cx.doc, name) {
        self.push_ref(cx, RefKind::Instantiation, path, None, Vec::new(), last_segment(name));
      }
    }
    for field in node.child_by_field_name("body").map(expression_children).unwrap_or_default() {
      self.visit_expr(cx, field);
    }
  }

  pub(crate) fn macro_call<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    let Some(macro_node) = node.child_by_field_name("macro") else { return };
    let Some(path) = path_segments(cx.doc, macro_node) else { return };
    self.macro_body(cx, path, last_segment(macro_node), macro_token_tree(node));
  }

  fn macro_body<'t>(&mut self, cx: Cx<'t>, path: Vec<String>, name: Node<'t>, token_tree: Option<Node<'t>>) {
    let values = if is_std_expression_macro(&path) { cx.values } else { ValueRefs::Suppress };
    let patterns_after_first = takes_patterns_after_first_argument(&path);
    let piecewise_syntax = is_std_expression_macro(&path) || is_known_expression_family(&path);
    let args = match token_tree {
      Some(token_tree) if cx.macro_depth < MAX_MACRO_NESTING => MacroArgs::parse(cx.doc, token_tree),
      Some(token_tree) => MacroArgs::Opaque(short_text(cx.doc.text(token_tree))),
      None => MacroArgs::Empty,
    };
    self.push_ref(cx, RefKind::MacroCall, path, None, args.summaries(0), name);
    let inner = Cx {
      values,
      macro_depth: cx.macro_depth + 1,
      ..cx
    };
    match &args {
      MacroArgs::Expressions(parsed) => {
        let elements = parsed.elements();
        let visited = if patterns_after_first { &elements[..elements.len().min(1)] } else { &elements[..] };
        let parsed_cx = Cx { doc: parsed.doc(), ..inner };
        for element in visited {
          let named_value = element
            .child_by_field_name("left")
            .filter(|left| element.kind() == "assignment_expression" && left.kind() == "identifier")
            .and_then(|_| element.child_by_field_name("right"));
          self.visit_expr(parsed_cx, named_value.unwrap_or(*element));
        }
      }
      MacroArgs::Pieces(pieces) => {
        let visited = if patterns_after_first { &pieces[..pieces.len().min(1)] } else { &pieces[..] };
        let mut budget = MAX_SCAN_TOKENS;
        for piece in visited {
          match &piece.body {
            PieceBody::Parsed(parsed) => {
              let origin = if piecewise_syntax { RefOrigin::Syntax } else { RefOrigin::MacroTokenScan };
              let parsed_cx = Cx {
                doc: parsed.doc(),
                origin: if cx.origin == RefOrigin::Syntax { origin } else { cx.origin },
                ..inner
              };
              for element in parsed.elements() {
                self.visit_expr(parsed_cx, element);
              }
            }
            PieceBody::Unparsed(tokens) => {
              let scan = Cx {
                values: ValueRefs::Suppress,
                origin: RefOrigin::MacroTokenScan,
                ..inner
              };
              self.scan_tokens(scan, tokens, &mut budget, 0);
            }
          }
        }
      }
      MacroArgs::Empty | MacroArgs::Opaque(_) => {}
    }
  }

  fn scan_tokens<'t>(&mut self, cx: Cx<'t>, tokens: &[Node<'t>], budget: &mut usize, depth: u32) {
    if depth >= MAX_SCAN_DEPTH {
      return;
    }
    let mut index = 0;
    while index < tokens.len() && *budget > 0 {
      *budget -= 1;
      let token = tokens[index];
      if let Some(resume) = skip_non_expression(cx.doc, tokens, index) {
        index = resume;
        continue;
      }
      if token.kind() == "."
        && let (Some(method), Some(arguments)) = (tokens.get(index + 1), tokens.get(index + 2))
        && let Some(name) = call_name(cx.doc, *method)
        && opens_with(*arguments, "(")
      {
        let mut path = receiver_tokens(cx.doc, &tokens[..index]);
        let hint = path.first().map(|head| self.head_hint(head));
        path.push(name.to_owned());
        self.push_ref(cx, RefKind::MethodCall, path, hint, token_tree_args(cx.doc, *arguments, 0), *method);
        self.scan_tokens(cx, &inner_tokens(*arguments), budget, depth + 1);
        index += 3;
        continue;
      }
      let previous = index.checked_sub(1).map(|previous| tokens[previous]);
      let after_path_separator = previous.is_some_and(|previous| matches!(previous.kind(), "." | "::"));
      if !after_path_separator && let Some(resume) = self.scan_path(cx, tokens, index, budget, depth) {
        index = resume;
        continue;
      }
      if token.kind() == "token_tree" {
        self.scan_tokens(cx, &inner_tokens(token), budget, depth + 1);
      }
      index += 1;
    }
  }

  fn scan_path<'t>(&mut self, cx: Cx<'t>, tokens: &[Node<'t>], start: usize, budget: &mut usize, depth: u32) -> Option<usize> {
    let (segments, end) = token_path(cx.doc, tokens, start)?;
    let name = tokens[end - 1];
    let group = tokens.get(end).filter(|group| group.kind() == "token_tree");
    let follower = tokens.get(end + 1).map(|follower| cx.doc.text(*follower));
    if let (Some(bang), Some(tree)) = (tokens.get(end), tokens.get(end + 1))
      && bang.kind() == "!"
      && tree.kind() == "token_tree"
    {
      self.macro_body(cx, segments, name, Some(*tree));
      return Some(end + 2);
    }
    let Some(group) = group else { return Some(end) };
    let previous = start.checked_sub(1).map(|previous| cx.doc.text(tokens[previous]));
    let last = segments.last().map(String::as_str).unwrap_or_default();
    let in_bound = matches!(previous, Some(":" | "+")) && last.starts_with(char::is_uppercase);
    let declared = previous.is_some_and(|previous| DECLARATION_KEYWORDS.contains(&previous));
    let pattern = follower.is_some_and(|follower| PATTERN_FOLLOWERS.contains(&follower));
    if in_bound || pattern || CLOSURE_TRAITS.contains(&last) || (declared && opens_with(*group, "(")) {
      return Some(end + 1);
    }
    if declared {
      return Some(end);
    }
    if opens_with(*group, "(") {
      self.push_ref(cx, RefKind::Call, segments, None, token_tree_args(cx.doc, *group, 0), name);
    } else if opens_with(*group, "{") && last.starts_with(char::is_uppercase) {
      self.push_ref(cx, RefKind::Instantiation, segments, None, Vec::new(), name);
    }
    self.scan_tokens(cx, &inner_tokens(*group), budget, depth + 1);
    Some(end + 1)
  }

  fn nested_impl<'t>(&mut self, cx: Cx<'t>, node: Node<'t>, attributes: &[Node<'t>], start: usize) {
    let module_suffix = cx
      .from
      .and_then(|local| self.facts.symbol(local))
      .map(|symbol| symbol.module_suffix.clone())
      .unwrap_or_default();
    self.push_impl(cx.doc, node, attributes, start, module_suffix, cx.from);
    self.nested_items += 1;
    self.generics.push(generic_names(cx.doc, node));
    self.nested_parts(cx, node);
    self.generics.pop();
    self.nested_items -= 1;
  }

  fn nested_item<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    self.nested_items += 1;
    self.nested_item_parts(cx, node);
    self.nested_items -= 1;
  }

  fn nested_item_parts<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    match node.kind() {
      "function_item" | "function_signature_item" => self.function_refs(cx, node),
      "const_item" | "static_item" => {
        if let Some(declared) = node.child_by_field_name("type") {
          self.visit_type(cx, declared);
        }
        if let Some(value) = node.child_by_field_name("value") {
          self.locals.push(Vec::new());
          self.visit_expr(cx, value);
          self.locals.pop();
        }
      }
      "macro_definition" => {}
      "impl_item" => self.nested_impl(cx, node, &[], node.start_byte()),
      _ => {
        self.generics.push(generic_names(cx.doc, node));
        self.nested_parts(cx, node);
        self.generics.pop();
      }
    }
  }

  fn nested_parts<'t>(&mut self, cx: Cx<'t>, node: Node<'t>) {
    let Some(cx) = self.enter(cx) else { return };
    let name = node.child_by_field_name("name").map(|name| name.id());
    for child in named_children(node).into_iter().filter(|child| Some(child.id()) != name && !is_comment(*child)) {
      match child.kind() {
        kind if ITEM_KINDS.contains(&kind) => self.nested_item(cx, child),
        "declaration_list" | "field_declaration_list" | "enum_variant_list" | "enum_variant" | "field_declaration" | "associated_type" => self.nested_parts(cx, child),
        "type_parameters" | "where_clause" | "trait_bounds" | "ordered_field_declaration_list" => self.visit_type(cx, child),
        "attribute_item" | "visibility_modifier" | "extern_modifier" => {}
        kind if TYPE_KINDS.contains(&kind) => self.visit_type(cx, child),
        _ => self.visit_expr(cx, child),
      }
    }
  }
}

fn is_std_expression_macro(path: &[String]) -> bool {
  let (Some(name), qualifier) = (path.last(), &path[..path.len().saturating_sub(1)]) else {
    return false;
  };
  let std_qualified = matches!(qualifier, [] | [_]) && qualifier.iter().all(|crate_name| matches!(crate_name.as_str(), "std" | "core" | "alloc"));
  std_qualified && STD_EXPRESSION_MACROS.contains(&name.as_str())
}

fn takes_patterns_after_first_argument(path: &[String]) -> bool {
  path
    .last()
    .is_some_and(|name| matches!(name.as_str(), "matches" | "assert_matches" | "debug_assert_matches"))
}

fn opens_with(tree: Node<'_>, delimiter: &str) -> bool {
  tree.kind() == "token_tree" && tree.child(0).is_some_and(|open| open.kind() == delimiter)
}

fn call_name<'t>(doc: Doc<'t>, token: Node<'_>) -> Option<&'t str> {
  word(doc, token).filter(|name| !KEYWORDS.contains(name) && !name.starts_with('$'))
}

fn token_path(doc: Doc<'_>, tokens: &[Node<'_>], start: usize) -> Option<(Vec<String>, usize)> {
  let mut segments = vec![call_name(doc, *tokens.get(start)?)?.to_owned()];
  let mut end = start + 1;
  while let (Some(separator), Some(segment)) = (tokens.get(end), tokens.get(end + 1)) {
    let Some(segment) = call_name(doc, *segment).filter(|_| separator.kind() == "::") else {
      break;
    };
    segments.push(segment.to_owned());
    end += 2;
  }
  Some((segments, end))
}

fn receiver_tokens(doc: Doc<'_>, before: &[Node<'_>]) -> Vec<String> {
  let mut chain = Vec::new();
  for (offset, token) in before.iter().rev().take(MAX_RECEIVER_TOKENS).enumerate() {
    let is_segment = offset % 2 == 0;
    match (is_segment, word(doc, *token)) {
      (true, Some(segment)) if !KEYWORDS.contains(&segment) => chain.push(segment.to_owned()),
      (false, _) if token.kind() == "." => {}
      _ => break,
    }
  }
  chain.reverse();
  chain
}

fn plain_binding<'t>(doc: Doc<'t>, pattern: Node<'_>) -> Option<&'t str> {
  let pattern = match pattern.kind() {
    "mut_pattern" | "ref_pattern" => pattern.named_children(&mut pattern.walk()).find(|child| child.kind() == "identifier")?,
    _ => pattern,
  };
  (pattern.kind() == "identifier")
    .then(|| doc.identifier(pattern))
    .filter(|name| !name.is_empty() && *name != "_")
}

fn field_chain(doc: Doc<'_>, node: Node<'_>) -> Option<Vec<String>> {
  let mut segments = Vec::new();
  let mut current = node;
  while current.kind() == "field_expression" {
    if segments.len() >= MAX_PATH_WALK {
      return None;
    }
    let field = doc.identifier(current.child_by_field_name("field")?);
    if field.is_empty() {
      return None;
    }
    segments.push(field.to_owned());
    current = current.child_by_field_name("value")?;
  }
  let accessed = segments.first()?;
  let head = doc.identifier(current);
  if !matches!(current.kind(), "identifier" | "self") || head.is_empty() || accessed.starts_with(|c: char| c.is_ascii_digit()) {
    return None;
  }
  segments.push(head.to_owned());
  segments.reverse();
  Some(segments)
}

fn typed_binding<'t, 'n>(doc: Doc<'t>, pattern: Node<'n>, declared: Node<'n>, local_types: &HashSet<String>) -> Option<(&'t str, Node<'n>)> {
  if let Some(name) = plain_binding(doc, pattern) {
    return Some((name, declared));
  }
  if pattern.kind() != "tuple_struct_pattern" {
    return None;
  }
  let wrapper = pattern.child_by_field_name("type")?;
  let wrapper_id = wrapper.id();
  let inner: Vec<Node<'_>> = pattern.named_children(&mut pattern.walk()).filter(|child| child.id() != wrapper_id).collect();
  let [binding] = inner.as_slice() else { return None };
  let name = plain_binding(doc, *binding)?;
  let declared = strip_reference(declared);
  let base = declared.child_by_field_name("type").filter(|_| declared.kind() == "generic_type")?;
  let wrapper_name = path_segments(doc, wrapper)?.pop()?;
  let base_name = path_segments(doc, base)?.pop()?;
  let wraps_declared_type = match wrapper_name.as_str() {
    "Some" => base_name == "Option",
    name => NEWTYPE_WRAPPERS.contains(&name) && base_name == name,
  };
  if !wraps_declared_type || local_types.contains(&wrapper_name) || local_types.contains(&base_name) {
    return None;
  }
  let arguments = declared.child_by_field_name("type_arguments")?;
  let arguments = named_children(arguments);
  let [argument] = arguments.as_slice() else { return None };
  Some((name, *argument))
}

fn strip_reference(mut node: Node<'_>) -> Node<'_> {
  for _ in 0..MAX_PATH_WALK {
    let inner = match node.kind() {
      "reference_type" | "pointer_type" => node.child_by_field_name("type"),
      "dynamic_type" | "abstract_type" => node.child_by_field_name("trait"),
      "bounded_type" => node.named_child(0),
      _ => None,
    };
    let Some(inner) = inner else { break };
    node = inner;
  }
  node
}

fn type_hint(doc: Doc<'_>, declared: Node<'_>) -> Option<Vec<String>> {
  let base = strip_reference(declared);
  if !matches!(base.kind(), "type_identifier" | "scoped_type_identifier" | "generic_type" | "primitive_type") {
    return None;
  }
  path_segments(doc, base)
}

fn literal_type(doc: Doc<'_>, value: Node<'_>) -> Option<Vec<String>> {
  (value.kind() == "struct_expression").then(|| path_segments(doc, value.child_by_field_name("name")?))?
}

fn is_known_expression_family(path: &[String]) -> bool {
  let (Some(name), qualifier) = (path.last(), &path[..path.len().saturating_sub(1)]) else {
    return false;
  };
  let known_crate = matches!(qualifier, [] | [_]) && qualifier.iter().all(|crate_name| matches!(crate_name.as_str(), "tracing" | "log" | "anyhow"));
  known_crate && PIECEWISE_SYNTAX_MACROS.contains(&name.as_str())
}

// Patterns, declarations and attributes inside unparsed macro bodies look like calls; skip them wholesale.
fn skip_non_expression(doc: Doc<'_>, tokens: &[Node<'_>], index: usize) -> Option<usize> {
  let token = tokens[index];
  let next_group = |from: usize, delimiters: &[&str]| {
    tokens[from..]
      .iter()
      .position(|token| delimiters.iter().any(|delimiter| opens_with(*token, delimiter)) || token.kind() == ";")
      .map_or(tokens.len(), |offset| from + offset + 1)
  };
  let until = |from: usize, stops: &[&str]| {
    tokens[from..]
      .iter()
      .position(|token| stops.contains(&doc.text(*token)))
      .map_or(tokens.len(), |offset| from + offset)
  };
  match doc.text(token) {
    "struct" | "enum" | "union" => Some(next_group(index + 1, &["{", "("])),
    "fn" => Some(next_group(index + 1, &["("])),
    "let" => Some(until(index + 1, &["=", ";", "else"])),
    "for" => Some(until(index + 1, &["in"])),
    "#" => Some(next_group(index + 1, &["["])),
    "$" => Some(index + 2),
    _ if token.kind() == "metavariable" => Some(index + 1),
    _ => None,
  }
}
