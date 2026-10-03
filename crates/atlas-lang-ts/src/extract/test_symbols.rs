use atlas_core::{AttributeFact, EntityKind, TestInfo, TestKind, Visibility};
use tree_sitter::Node;

use super::Extractor;
use crate::args::summarize_arguments;
use crate::fingerprint::fingerprint;
use crate::signature::normalize;
use crate::syntax::{children, is_function_value, span_of, text, unwrap_expression};
use crate::testing::{TestCall, UNKNOWN_FRAMEWORK};

const TABLE_MODIFIERS: [&str; 4] = ["each", "skipIf", "runIf", "for"];

impl<'s, 't> Extractor<'s, 't> {
  pub(super) fn is_test_head(&self, head: Node<'_>) -> bool {
    let name = text(head, self.source);
    !self.scopes.is_local_value(name) && !self.module.foreign_imports.iter().any(|imported| imported == name)
  }

  pub(super) fn test_symbol(&mut self, call: Node<'t>, test: TestCall<'t>) {
    let mut walk: Vec<Node<'t>> = test.each_arguments.into_iter().collect();
    walk.push(test.arguments);
    if test.name.is_empty() {
      return self.push_all(walk);
    }
    let kind = match test.kind {
      TestKind::Suite => EntityKind::TestSuite,
      _ => EntityKind::Test,
    };
    let mut item_path = self.suites.clone();
    item_path.push(test.name.clone());
    let local = self.add_symbol(kind, test.name.clone(), item_path, self.owner, (span_of(call), span_of(test.name_node)), Visibility::Module);
    let attributes = test
      .modifiers
      .iter()
      .map(|modifier| AttributeFact {
        path: vec![modifier.clone()],
        args: if TABLE_MODIFIERS.contains(&modifier.as_str()) {
          summarize_arguments(test.each_arguments, self.source)
        } else {
          Vec::new()
        },
        span: span_of(test.callee),
      })
      .collect();
    let callback = children(test.arguments).into_iter().map(unwrap_expression).rfind(|argument| is_function_value(*argument));
    let body = callback.and_then(|callback| callback.child_by_field_name("body")).map(fingerprint);
    let signature = normalize(&format!("{}({})", text(test.callee, self.source), text(test.name_node, self.source)));
    if let Some(symbol) = self.symbol_mut(local) {
      symbol.signature = signature;
      symbol.attributes = attributes;
      symbol.body = body;
      symbol.test = Some(TestInfo {
        framework: UNKNOWN_FRAMEWORK.to_owned(),
        kind: test.kind,
        display_name: Some(test.name.clone()),
      });
    }
    self.test_symbols.push(local);
    self.checkpoint();
    self.owner = Some(local);
    if test.kind == TestKind::Suite {
      self.suites.push(test.name);
    }
    self.push_all(walk);
  }
}
