use atlas_core::{ARG_MAX_DEPTH, ARG_MAX_WIDTH, ArgSummary, ObjectEntry, ObjectKey};
use tree_sitter::Node;

use crate::literals::{decode_raw_string_literal, decode_string_literal};
use crate::macros::MacroArgs;
use crate::source::{Doc, normalize_whitespace, short_text};
use crate::syntax::{children, is_comment, named_children, path_segments};

const MAX_RECEIVER_SEGMENTS: usize = 32;
const STRING_PRESERVING_METHODS: [&str; 6] = ["to_string", "to_owned", "into", "as_str", "as_ref", "into_owned"];

pub(crate) fn capped<T>(items: &[T], summarize: impl FnMut(&T) -> ArgSummary) -> Vec<ArgSummary> {
  if items.len() <= ARG_MAX_WIDTH {
    return items.iter().map(summarize).collect();
  }
  let mut summaries: Vec<ArgSummary> = items[..ARG_MAX_WIDTH - 1].iter().map(summarize).collect();
  summaries.push(ArgSummary::Truncated);
  summaries
}

pub(crate) fn expression_children(node: Node<'_>) -> Vec<Node<'_>> {
  named_children(node)
    .into_iter()
    .filter(|child| !is_comment(*child) && !matches!(child.kind(), "attribute_item" | "inner_attribute_item"))
    .collect()
}

pub(crate) fn call_args(doc: Doc<'_>, arguments: Node<'_>) -> Vec<ArgSummary> {
  capped(&expression_children(arguments), |argument| summarize(doc, *argument, 0))
}

pub(crate) fn summarize(doc: Doc<'_>, node: Node<'_>, depth: usize) -> ArgSummary {
  if depth >= ARG_MAX_DEPTH {
    return ArgSummary::Truncated;
  }
  let text = doc.text(node);
  match node.kind() {
    "string_literal" => ArgSummary::Str(decode_string_literal(text)),
    "raw_string_literal" => ArgSummary::Str(decode_raw_string_literal(text)),
    "integer_literal" | "float_literal" => ArgSummary::Number(text.to_owned()),
    "boolean_literal" => ArgSummary::Bool(text == "true"),
    "unary_expression" if text.starts_with('-') && node.named_child(0).is_some_and(|operand| matches!(operand.kind(), "integer_literal" | "float_literal")) => {
      ArgSummary::Number(normalize_whitespace(text).replace(' ', ""))
    }
    "identifier" | "self" | "scoped_identifier" | "field_expression" => match receiver_chain(doc, node) {
      Some(path) => ArgSummary::Path(path),
      None => ArgSummary::other(&short_text(text)),
    },
    "reference_expression" | "parenthesized_expression" | "try_expression" | "await_expression" | "type_cast_expression" => {
      let inner = node.child_by_field_name("value").or_else(|| expression_children(node).into_iter().next());
      match inner {
        Some(inner) => summarize(doc, inner, depth + 1),
        None => ArgSummary::other(&short_text(text)),
      }
    }
    "call_expression" => summarize_call(doc, node, depth),
    "macro_invocation" => summarize_macro(doc, node, depth),
    "closure_expression" => ArgSummary::Closure {
      params: node
        .child_by_field_name("parameters")
        .map(|parameters| {
          children(parameters)
            .into_iter()
            .filter(|parameter| !matches!(parameter.kind(), "|" | "," | "line_comment" | "block_comment"))
            .map(|parameter| normalize_whitespace(doc.text(parameter.child_by_field_name("pattern").unwrap_or(parameter))))
            .collect()
        })
        .unwrap_or_default(),
    },
    "struct_expression" => summarize_struct(doc, node, depth),
    "array_expression" | "tuple_expression" => ArgSummary::Array(capped(&expression_children(node), |element| summarize(doc, *element, depth + 1))),
    "binary_expression" if operator(doc, node) == Some("+") => {
      let operands = concat_operands(doc, node);
      ArgSummary::Concat(capped(&operands, |operand| summarize(doc, *operand, depth + 1)))
    }
    "assignment_expression" => match (node.child_by_field_name("left"), node.child_by_field_name("right")) {
      (Some(left), Some(right)) if left.kind() == "identifier" => ArgSummary::Named {
        name: doc.identifier(left).to_owned(),
        value: Box::new(summarize(doc, right, depth + 1)),
      },
      _ => ArgSummary::other(&short_text(text)),
    },
    _ => ArgSummary::other(&short_text(text)),
  }
}

fn operator<'t>(doc: Doc<'t>, node: Node<'_>) -> Option<&'t str> {
  node.child_by_field_name("operator").map(|operator| doc.text(operator))
}

fn concat_operands<'t>(doc: Doc<'_>, node: Node<'t>) -> Vec<Node<'t>> {
  let mut operands = Vec::new();
  let mut current = node;
  while current.kind() == "binary_expression" && operator(doc, current) == Some("+") {
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

fn summarize_call(doc: Doc<'_>, node: Node<'_>, depth: usize) -> ArgSummary {
  let text = doc.text(node);
  let Some(function) = node.child_by_field_name("function") else {
    return ArgSummary::other(&short_text(text));
  };
  let args = || {
    node
      .child_by_field_name("arguments")
      .map(|arguments| capped(&expression_children(arguments), |argument| summarize(doc, *argument, depth + 1)))
  };
  let callee = match function.kind() {
    "field_expression" => {
      let receiver = function.child_by_field_name("value");
      let method = function.child_by_field_name("field").map(|field| doc.identifier(field));
      if let (Some(receiver), Some(method)) = (receiver, method)
        && STRING_PRESERVING_METHODS.contains(&method)
        && matches!(receiver.kind(), "string_literal" | "raw_string_literal")
      {
        return summarize(doc, receiver, depth);
      }
      receiver_chain(doc, function)
    }
    _ => path_segments(doc, function),
  };
  match callee {
    Some(callee) => ArgSummary::Call {
      callee,
      args: args().unwrap_or_default(),
    },
    None => ArgSummary::other(&short_text(text)),
  }
}

fn summarize_macro(doc: Doc<'_>, node: Node<'_>, depth: usize) -> ArgSummary {
  let Some(callee) = macro_callee(doc, node) else {
    return ArgSummary::other(&short_text(doc.text(node)));
  };
  let args = MacroArgs::of_invocation(doc, node);
  ArgSummary::Call {
    callee,
    args: args.summaries(depth + 1),
  }
}

pub(crate) fn macro_callee(doc: Doc<'_>, node: Node<'_>) -> Option<Vec<String>> {
  let mut callee = path_segments(doc, node.child_by_field_name("macro")?)?;
  if let Some(last) = callee.last_mut() {
    last.push('!');
  }
  Some(callee)
}

fn summarize_struct(doc: Doc<'_>, node: Node<'_>, depth: usize) -> ArgSummary {
  let fields = node.child_by_field_name("body").map(expression_children).unwrap_or_default();
  let entries = fields
    .iter()
    .take(ARG_MAX_WIDTH)
    .filter_map(|field| {
      let (key, value) = match field.kind() {
        "field_initializer" => (
          ObjectKey::Ident(doc.identifier(field.child_by_field_name("field")?).to_owned()),
          summarize(doc, field.child_by_field_name("value")?, depth + 1),
        ),
        "shorthand_field_initializer" => {
          let name = doc.identifier(field.named_child(0)?).to_owned();
          (ObjectKey::Ident(name.clone()), ArgSummary::Path(vec![name]))
        }
        "base_field_initializer" => (ObjectKey::Spread, summarize(doc, field.named_child(0)?, depth + 1)),
        _ => return None,
      };
      Some(ObjectEntry { key, value })
    })
    .collect();
  ArgSummary::Object(entries)
}

pub(crate) fn receiver_chain(doc: Doc<'_>, node: Node<'_>) -> Option<Vec<String>> {
  let mut segments = Vec::new();
  let mut current = node;
  loop {
    if segments.len() >= MAX_RECEIVER_SEGMENTS {
      return None;
    }
    match current.kind() {
      "field_expression" => {
        let field = doc.identifier(current.child_by_field_name("field")?);
        if field.is_empty() {
          return None;
        }
        segments.push(field.to_owned());
        current = current.child_by_field_name("value")?;
      }
      "identifier" | "self" | "scoped_identifier" => {
        let mut path = path_segments(doc, current)?;
        segments.reverse();
        path.extend(segments);
        return Some(path);
      }
      _ => return None,
    }
  }
}
