use atlas_core::{ARG_MAX_DEPTH, ARG_MAX_TEXT, ARG_MAX_WIDTH, ArgSummary, ObjectEntry, ObjectKey, TemplatePart};
use tree_sitter::Node;

use crate::syntax::{children, compact_text, decode_escape, member_chain, property_name, snippet, string_value, text, unwrap_expression};

pub(crate) fn summarize_arguments(arguments: Option<Node<'_>>, source: &str) -> Vec<ArgSummary> {
  let Some(arguments) = arguments else { return Vec::new() };
  if arguments.kind() == "template_string" {
    return vec![summarize(arguments, source, 0)];
  }
  capped(children(arguments).into_iter().map(|argument| summarize(argument, source, 0)))
}

fn capped(items: impl Iterator<Item = ArgSummary>) -> Vec<ArgSummary> {
  let mut summaries = Vec::new();
  for item in items {
    if summaries.len() == ARG_MAX_WIDTH {
      summaries.push(ArgSummary::Truncated);
      break;
    }
    summaries.push(item);
  }
  summaries
}

pub(crate) fn summarize(node: Node<'_>, source: &str, depth: usize) -> ArgSummary {
  let node = unwrap_expression(node);
  match node.kind() {
    "string" => ArgSummary::Str(limit_literal(string_value(node, source))),
    "number" => ArgSummary::Number(text(node, source).to_owned()),
    "true" => ArgSummary::Bool(true),
    "false" => ArgSummary::Bool(false),
    "null" => ArgSummary::Null,
    "undefined" => ArgSummary::Path(vec!["undefined".to_owned()]),
    "identifier" | "this" => ArgSummary::Path(vec![text(node, source).to_owned()]),
    "member_expression" | "subscript_expression" => match member_chain(node, source).static_path(source) {
      Some(path) => ArgSummary::Path(path),
      None => ArgSummary::Other(snippet(text(node, source))),
    },
    "arrow_function" | "function_expression" | "function" | "generator_function" => ArgSummary::Closure {
      params: closure_params(node, source),
    },
    _ if depth >= ARG_MAX_DEPTH && is_composite(node) => ArgSummary::Truncated,
    "template_string" => ArgSummary::Template(template_parts(node, source, depth)),
    "binary_expression" if is_concat(node, source) => ArgSummary::Concat(capped(concat_operands(node, source).into_iter().map(|operand| summarize(operand, source, depth + 1)))),
    "object" => ArgSummary::Object(object_entries(node, source, depth)),
    "array" => ArgSummary::Array(capped(children(node).into_iter().map(|item| summarize(item, source, depth + 1)))),
    "call_expression" => call_summary(node, source, depth),
    _ => ArgSummary::Other(snippet(text(node, source))),
  }
}

fn is_composite(node: Node<'_>) -> bool {
  matches!(node.kind(), "template_string" | "binary_expression" | "object" | "array" | "call_expression")
}

fn limit_literal(value: String) -> String {
  if value.len() <= ARG_MAX_TEXT {
    return value;
  }
  let mut end = ARG_MAX_TEXT - '…'.len_utf8();
  while !value.is_char_boundary(end) {
    end -= 1;
  }
  format!("{}…", &value[..end])
}

fn template_parts(node: Node<'_>, source: &str, depth: usize) -> Vec<TemplatePart> {
  let mut parts: Vec<TemplatePart> = Vec::new();
  for part in children(node) {
    if parts.len() == ARG_MAX_WIDTH {
      parts.push(TemplatePart::Expr(ArgSummary::Truncated));
      break;
    }
    match part.kind() {
      "template_substitution" => {
        let value = children(part).into_iter().next().map_or(ArgSummary::Truncated, |inner| summarize(inner, source, depth + 1));
        parts.push(TemplatePart::Expr(value));
      }
      "escape_sequence" => push_literal(&mut parts, &decode_escape(text(part, source))),
      _ => push_literal(&mut parts, text(part, source)),
    }
  }
  parts
}

fn push_literal(parts: &mut Vec<TemplatePart>, value: &str) {
  if value.is_empty() {
    return;
  }
  if let Some(TemplatePart::Lit(previous)) = parts.last_mut() {
    previous.push_str(value);
    return;
  }
  parts.push(TemplatePart::Lit(value.to_owned()));
}

fn is_concat(node: Node<'_>, source: &str) -> bool {
  node.child_by_field_name("operator").is_some_and(|operator| text(operator, source) == "+")
}

fn concat_operands<'t>(node: Node<'t>, source: &str) -> Vec<Node<'t>> {
  let mut operands = Vec::new();
  let mut current = node;
  while current.kind() == "binary_expression" && is_concat(current, source) {
    let (Some(left), Some(right)) = (current.child_by_field_name("left"), current.child_by_field_name("right")) else {
      break;
    };
    operands.push(right);
    current = left;
  }
  operands.push(current);
  operands.reverse();
  operands
}

fn object_entries(node: Node<'_>, source: &str, depth: usize) -> Vec<ObjectEntry> {
  let mut entries = Vec::new();
  for member in children(node) {
    if entries.len() == ARG_MAX_WIDTH {
      entries.push(ObjectEntry {
        key: ObjectKey::Spread,
        value: ArgSummary::Truncated,
      });
      break;
    }
    let entry = match member.kind() {
      "pair" => {
        let (Some(key), Some(value)) = (member.child_by_field_name("key"), member.child_by_field_name("value")) else {
          continue;
        };
        ObjectEntry {
          key: object_key(key, source),
          value: summarize(value, source, depth + 1),
        }
      }
      "shorthand_property_identifier" => ObjectEntry {
        key: ObjectKey::Ident(text(member, source).to_owned()),
        value: ArgSummary::Path(vec![text(member, source).to_owned()]),
      },
      "spread_element" => ObjectEntry {
        key: ObjectKey::Spread,
        value: children(member)
          .into_iter()
          .next()
          .map_or(ArgSummary::Truncated, |inner| summarize(inner, source, depth + 1)),
      },
      "method_definition" => {
        let Some(name) = member.child_by_field_name("name") else { continue };
        ObjectEntry {
          key: object_key(name, source),
          value: ArgSummary::Closure {
            params: closure_params(member, source),
          },
        }
      }
      _ => continue,
    };
    entries.push(entry);
  }
  entries
}

fn object_key(key: Node<'_>, source: &str) -> ObjectKey {
  match key.kind() {
    "string" => ObjectKey::Str(string_value(key, source)),
    "number" => ObjectKey::Str(text(key, source).to_owned()),
    "computed_property_name" => ObjectKey::Computed(compact_text(key, source)),
    _ => ObjectKey::Ident(property_name(key, source)),
  }
}

fn call_summary(node: Node<'_>, source: &str, depth: usize) -> ArgSummary {
  let callee = node.child_by_field_name("function").and_then(|callee| match callee.kind() {
    "identifier" => Some(vec![text(callee, source).to_owned()]),
    _ => member_chain(callee, source).static_path(source),
  });
  let Some(callee) = callee else { return ArgSummary::Other(snippet(text(node, source))) };
  let arguments = node.child_by_field_name("arguments");
  let args = match arguments {
    Some(arguments) if arguments.kind() == "template_string" => vec![summarize(arguments, source, depth + 1)],
    Some(arguments) => capped(children(arguments).into_iter().map(|argument| summarize(argument, source, depth + 1))),
    None => Vec::new(),
  };
  ArgSummary::Call { callee, args }
}

pub(crate) fn closure_params(function: Node<'_>, source: &str) -> Vec<String> {
  if let Some(parameter) = function.child_by_field_name("parameter") {
    return vec![text(parameter, source).to_owned()];
  }
  let Some(parameters) = function.child_by_field_name("parameters") else {
    return Vec::new();
  };
  children(parameters).into_iter().filter_map(|parameter| parameter_name(parameter, source)).collect()
}

pub(crate) fn parameter_name(parameter: Node<'_>, source: &str) -> Option<String> {
  let pattern = match parameter.kind() {
    "required_parameter" | "optional_parameter" => parameter.child_by_field_name("pattern")?,
    "assignment_pattern" => parameter.child_by_field_name("left")?,
    _ => parameter,
  };
  match pattern.kind() {
    "this" => None,
    "rest_pattern" => children(pattern).into_iter().next().map(|inner| compact_text(inner, source)),
    _ => Some(compact_text(pattern, source)),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_cap_literal_strings_on_char_boundaries() {
    let long = "é".repeat(200);
    let limited = limit_literal(long);
    assert!(limited.len() <= ARG_MAX_TEXT);
    assert!(limited.ends_with('…'));
    assert_eq!(limit_literal("/users".to_owned()), "/users");
  }
}
