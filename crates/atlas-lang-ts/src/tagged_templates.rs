use std::borrow::Cow;
use std::ops::{ControlFlow, Range};

use atlas_core::{AnalysisError, Deadline};
use tree_sitter::{Node, Tree};

use crate::grammar::{Grammar, parse};
use crate::type_arguments::mask_tagged_template_type_arguments;

pub(crate) struct Parsed<'s> {
  pub(crate) tree: Tree,
  pub(crate) source: Cow<'s, str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reading {
  Comparison,
  Error,
  Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ErrorFootprint {
  count: u32,
  bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Walk {
  Completed,
  Stopped,
  Cancelled,
}

// tree-sitter-typescript 0.23 has no type arguments on a tagged template: `` sql<T[]>`…` `` is an error and `` sql<T>`…` `` a comparison, while TypeScript
// reads both as type arguments. Masking them with spaces and reparsing restores the tagged call without moving any offset.
pub(crate) fn parse_tagged_templates<'s>(grammar: Grammar, source: &'s str, deadline: &Deadline) -> Result<Option<Parsed<'s>>, AnalysisError> {
  let Some(tree) = parse(grammar, source, deadline)? else { return Ok(None) };
  let original = |tree| {
    Ok(Some(Parsed {
      tree,
      source: Cow::Borrowed(source),
    }))
  };
  if grammar == Grammar::JavaScript {
    return original(tree);
  }
  let Some(masked) = mask_tagged_template_type_arguments(source) else {
    return original(tree);
  };
  let Some(reading) = reading(tree.root_node(), deadline) else { return Ok(None) };
  if reading == Reading::Plain {
    return original(tree);
  }
  let Some(retried) = parse(grammar, &masked.source, deadline)? else { return Ok(None) };
  let before = ErrorFootprint::of(tree.root_node());
  let after = ErrorFootprint::of(retried.root_node());
  let repairs = after.count < before.count || reading == Reading::Comparison;
  if !repairs || after.count > before.count || after.bytes > before.bytes {
    tracing::debug!(?reading, ?before, ?after, "kept the unmasked parse of tagged templates");
    return original(tree);
  }
  match masks_fill_tag_gaps(retried.root_node(), &masked.ranges, deadline) {
    None => Ok(None),
    Some(false) => {
      tracing::debug!(ranges = masked.ranges.len(), "a masked range is not between a tag and its template");
      original(tree)
    }
    Some(true) => Ok(Some(Parsed {
      tree: retried,
      source: Cow::Owned(masked.source),
    })),
  }
}

fn reading(root: Node<'_>, deadline: &Deadline) -> Option<Reading> {
  let walk = walk(root, deadline, |node| match is_comparison_tag(node) {
    true => ControlFlow::Break(()),
    false => ControlFlow::Continue(()),
  });
  match walk {
    Walk::Cancelled => None,
    Walk::Stopped => Some(Reading::Comparison),
    Walk::Completed if root.has_error() => Some(Reading::Error),
    Walk::Completed => Some(Reading::Plain),
  }
}

fn is_comparison_tag(node: Node<'_>) -> bool {
  node.kind() == "binary_expression"
    && node.child_by_field_name("operator").is_some_and(|operator| operator.kind() == ">")
    && node.child_by_field_name("right").is_some_and(|right| right.kind() == "template_string")
}

fn masks_fill_tag_gaps(root: Node<'_>, ranges: &[Range<usize>], deadline: &Deadline) -> Option<bool> {
  let mut gaps: Vec<Range<usize>> = Vec::new();
  let walk = walk(root, deadline, |node| {
    if node.kind() == "call_expression"
      && let (Some(function), Some(arguments)) = (node.child_by_field_name("function"), node.child_by_field_name("arguments"))
      && arguments.kind() == "template_string"
    {
      gaps.push(function.end_byte()..arguments.start_byte());
    }
    ControlFlow::Continue(())
  });
  if walk == Walk::Cancelled {
    return None;
  }
  gaps.sort_unstable_by_key(|gap| gap.start);
  let fills = |range: &Range<usize>| {
    let after = gaps.partition_point(|gap| gap.start <= range.start);
    after.checked_sub(1).and_then(|index| gaps.get(index)).is_some_and(|gap| range.end <= gap.end)
  };
  Some(ranges.iter().all(fills))
}

fn walk<'t>(root: Node<'t>, deadline: &Deadline, mut visit: impl FnMut(Node<'t>) -> ControlFlow<()>) -> Walk {
  let mut cursor = root.walk();
  loop {
    if deadline.tick().is_break() {
      return Walk::Cancelled;
    }
    if visit(cursor.node()).is_break() {
      return Walk::Stopped;
    }
    if cursor.goto_first_child() {
      continue;
    }
    loop {
      if cursor.goto_next_sibling() {
        break;
      }
      if !cursor.goto_parent() {
        return Walk::Completed;
      }
    }
  }
}

impl ErrorFootprint {
  fn of(root: Node<'_>) -> Self {
    let mut footprint = Self { count: 0, bytes: 0 };
    visit_errors(root, |node| {
      footprint.count = footprint.count.saturating_add(1);
      footprint.bytes = footprint.bytes.saturating_add(node.end_byte() - node.start_byte());
    });
    footprint
  }
}

pub(crate) fn visit_errors<'t>(root: Node<'t>, mut visit: impl FnMut(Node<'t>)) {
  if !root.has_error() {
    return;
  }
  let mut cursor = root.walk();
  loop {
    let node = cursor.node();
    let is_error = node.is_error() || node.is_missing();
    if is_error {
      visit(node);
    }
    if !is_error && node.has_error() && cursor.goto_first_child() {
      continue;
    }
    loop {
      if cursor.goto_next_sibling() {
        break;
      }
      if !cursor.goto_parent() {
        return;
      }
    }
  }
}
