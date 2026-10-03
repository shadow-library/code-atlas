use atlas_core::TestKind;
use tree_sitter::Node;

use crate::syntax::{ChainHead, children, member_chain, string_value, template_raw, text};

pub(crate) const UNKNOWN_FRAMEWORK: &str = "unknown";

const MODIFIERS: [&str; 13] = [
  "only",
  "skip",
  "todo",
  "each",
  "concurrent",
  "sequential",
  "shuffle",
  "failing",
  "fails",
  "skipIf",
  "runIf",
  "for",
  "serial",
];

pub(crate) fn is_test_path(path: &str) -> bool {
  let mut segments: Vec<&str> = path.split('/').collect();
  let file_name = segments.pop().unwrap_or_default();
  if segments.iter().any(|segment| matches!(*segment, "__tests__" | "test" | "tests")) {
    return true;
  }
  let stem = file_name.rsplit_once('.').map_or(file_name, |(stem, _)| stem);
  stem.split('.').skip(1).any(|part| {
    let marker = part.rsplit(['-', '_']).next().unwrap_or(part);
    matches!(marker, "spec" | "test")
  })
}

pub(crate) fn framework_of_import(specifier: &str) -> Option<&'static str> {
  match specifier {
    "vitest" => Some("vitest"),
    "@jest/globals" => Some("jest"),
    "mocha" => Some("mocha"),
    "node:test" | "bun:test" => Some(UNKNOWN_FRAMEWORK),
    _ => None,
  }
}

pub(crate) fn framework_of_global(receiver: &str) -> Option<&'static str> {
  match receiver {
    "jest" => Some("jest"),
    "vi" => Some("vitest"),
    _ => None,
  }
}

#[derive(Debug)]
pub(crate) struct TestCall<'t> {
  pub(crate) kind: TestKind,
  pub(crate) head: Node<'t>,
  pub(crate) modifiers: Vec<String>,
  pub(crate) callee: Node<'t>,
  pub(crate) each_arguments: Option<Node<'t>>,
  pub(crate) name: String,
  pub(crate) name_node: Node<'t>,
  pub(crate) arguments: Node<'t>,
}

pub(crate) fn recognize<'t>(call: Node<'t>, source: &str) -> Option<TestCall<'t>> {
  let callee = call.child_by_field_name("function")?;
  let (chain_node, each_arguments) = match callee.kind() {
    "call_expression" => (callee.child_by_field_name("function")?, callee.child_by_field_name("arguments")),
    _ => (callee, None),
  };
  let chain = member_chain(chain_node, source);
  let ChainHead::Identifier(head) = chain.head else { return None };
  if each_arguments.is_some() && chain.segments.last().is_none_or(|last| !matches!(last.as_str(), "each" | "skipIf" | "runIf" | "for")) {
    return None;
  }
  if !chain.segments.iter().all(|segment| MODIFIERS.contains(&segment.as_str())) {
    return None;
  }
  let (kind, implied) = match text(head, source) {
    "describe" | "suite" | "context" => (TestKind::Suite, None),
    "fdescribe" => (TestKind::Suite, Some("only")),
    "xdescribe" | "xcontext" => (TestKind::Suite, Some("skip")),
    "it" | "test" | "specify" => (TestKind::Case, None),
    "fit" => (TestKind::Case, Some("only")),
    "xit" | "xtest" | "xspecify" => (TestKind::Case, Some("skip")),
    _ => return None,
  };
  let arguments = call.child_by_field_name("arguments").filter(|arguments| arguments.kind() == "arguments")?;
  let name_node = children(arguments).into_iter().next()?;
  let name = match name_node.kind() {
    "string" => string_value(name_node, source),
    "template_string" => template_raw(name_node, source),
    _ => return None,
  };
  let modifiers = implied.map(str::to_owned).into_iter().chain(chain.segments).collect();
  Some(TestCall {
    kind,
    head,
    modifiers,
    callee,
    each_arguments,
    name,
    name_node,
    arguments,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_recognise_test_file_conventions() {
    for path in [
      "src/users.spec.ts",
      "src/money.test.tsx",
      "test/app.e2e-spec.ts",
      "src/__tests__/x.ts",
      "tests/a.js",
      "a.int_test.mjs",
    ] {
      assert!(is_test_path(path), "{path}");
    }
    for path in ["src/users.ts", "src/testing/helpers.ts", "src/spec.ts", "src/contest.ts", "src/latest.ts"] {
      assert!(!is_test_path(path), "{path}");
    }
  }

  #[test]
  fn should_map_framework_imports() {
    assert_eq!(framework_of_import("vitest"), Some("vitest"));
    assert_eq!(framework_of_import("@jest/globals"), Some("jest"));
    assert_eq!(framework_of_import("mocha"), Some("mocha"));
    assert_eq!(framework_of_import("node:test"), Some(UNKNOWN_FRAMEWORK));
    assert_eq!(framework_of_import("chai"), None);
  }
}
