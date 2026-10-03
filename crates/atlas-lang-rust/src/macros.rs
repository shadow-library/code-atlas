use atlas_core::ArgSummary;
use tree_sitter::{Node, Tree};

use crate::analyzer::{ParseMode, parse_rust};
use crate::source::{Doc, short_text};
use crate::summary::{capped, expression_children, summarize};
use crate::tokens::{inner_tokens, split_top_level, word};

const PREFIX: &str = "fn __atlas_macro() { [";
const SUFFIX: &str = "]; }";
const MAX_MACRO_SOURCE_BYTES: usize = 64 * 1024;

pub(crate) const STD_EXPRESSION_MACROS: [&str; 22] = [
  "assert",
  "assert_eq",
  "assert_ne",
  "dbg",
  "debug_assert",
  "debug_assert_eq",
  "debug_assert_ne",
  "eprint",
  "eprintln",
  "format",
  "format_args",
  "panic",
  "print",
  "println",
  "todo",
  "unimplemented",
  "unreachable",
  "vec",
  "write",
  "writeln",
  "matches",
  "concat",
];

const MAX_PIECE_PARSES: usize = 32;

pub(crate) enum MacroArgs<'t> {
  Empty,
  Expressions(ParsedMacro),
  Pieces(Vec<Piece<'t>>),
  Opaque(String),
}

pub(crate) struct Piece<'t> {
  name: Option<String>,
  text: &'t str,
  pub(crate) body: PieceBody<'t>,
}

pub(crate) enum PieceBody<'t> {
  Parsed(ParsedMacro),
  Unparsed(Vec<Node<'t>>),
}

pub(crate) struct ParsedMacro {
  text: String,
  base: usize,
  tree: Tree,
}

impl<'t> MacroArgs<'t> {
  pub(crate) fn of_invocation(doc: Doc<'t>, invocation: Node<'t>) -> Self {
    match macro_token_tree(invocation) {
      Some(token_tree) => Self::parse(doc, token_tree),
      None => Self::Empty,
    }
  }

  pub(crate) fn parse(doc: Doc<'t>, token_tree: Node<'t>) -> Self {
    let tokens = inner_tokens(token_tree);
    let (Some(first), Some(last)) = (tokens.first(), tokens.last()) else {
      return Self::Empty;
    };
    let (start, end) = (first.start_byte(), last.end_byte());
    let content = doc.slice(start, end);
    if content.trim().is_empty() {
      return Self::Empty;
    }
    if content.len() > MAX_MACRO_SOURCE_BYTES {
      return Self::Opaque(short_text(content));
    }
    let repeat_form = token_tree.child(0).is_some_and(|open| open.kind() == "[");
    if looks_like_expression_list(doc, &tokens, repeat_form)
      && let Some(parsed) = ParsedMacro::parse(doc, start, end)
    {
      return Self::Expressions(parsed);
    }
    let pieces = split_top_level(&tokens)
      .into_iter()
      .enumerate()
      .map(|(index, group)| Piece::parse(doc, group, index < MAX_PIECE_PARSES))
      .collect();
    Self::Pieces(pieces)
  }

  pub(crate) fn summaries(&self, depth: usize) -> Vec<ArgSummary> {
    match self {
      Self::Empty => Vec::new(),
      Self::Opaque(text) => vec![ArgSummary::Other(text.clone())],
      Self::Expressions(parsed) => {
        let doc = parsed.doc();
        capped(&parsed.elements(), |element| summarize(doc, *element, depth))
      }
      Self::Pieces(pieces) => capped(pieces, |piece| piece.summary(depth)),
    }
  }
}

impl<'t> Piece<'t> {
  fn parse(doc: Doc<'t>, group: Vec<Node<'t>>, may_parse: bool) -> Self {
    let text = match (group.first(), group.last()) {
      (Some(first), Some(last)) => doc.slice(first.start_byte(), last.end_byte()),
      _ => "",
    };
    let (name, value) = split_argument_prefix(doc, &group);
    let parsed = match (value.first(), value.last()) {
      (Some(first), Some(last)) if may_parse && looks_like_expression_list(doc, value, false) => {
        ParsedMacro::parse(doc, first.start_byte(), last.end_byte()).filter(|parsed| parsed.elements().len() == 1)
      }
      _ => None,
    };
    match parsed {
      Some(parsed) => Self {
        name,
        text,
        body: PieceBody::Parsed(parsed),
      },
      None => Self {
        name: None,
        text,
        body: PieceBody::Unparsed(group),
      },
    }
  }

  fn summary(&self, depth: usize) -> ArgSummary {
    let value = match &self.body {
      PieceBody::Parsed(parsed) => match parsed.elements().first() {
        Some(element) => summarize(parsed.doc(), *element, depth),
        None => ArgSummary::Truncated,
      },
      PieceBody::Unparsed(_) => ArgSummary::Other(short_text(self.text)),
    };
    match &self.name {
      Some(name) => ArgSummary::Named {
        name: name.clone(),
        value: Box::new(value),
      },
      None => value,
    }
  }
}

pub(crate) fn macro_token_tree(invocation: Node<'_>) -> Option<Node<'_>> {
  invocation.named_children(&mut invocation.walk()).filter(|child| child.kind() == "token_tree").last()
}

fn split_argument_prefix<'a, 't>(doc: Doc<'_>, group: &'a [Node<'t>]) -> (Option<String>, &'a [Node<'t>]) {
  if let [name, colon, rest @ ..] = group
    && colon.kind() == ":"
    && let Some(name) = word(doc, *name)
  {
    return (Some(name.to_owned()), strip_sigil(rest));
  }
  let equals = group.iter().position(|token| token.kind() == "=");
  if let Some(equals) = equals
    && equals % 2 == 1
    && group[..equals]
      .iter()
      .enumerate()
      .all(|(index, token)| if index % 2 == 0 { word(doc, *token).is_some() } else { token.kind() == "." })
  {
    let name = group[..equals].iter().map(|token| doc.identifier(*token)).collect::<Vec<_>>().concat();
    return (Some(name), strip_sigil(&group[equals + 1..]));
  }
  (None, strip_sigil(group))
}

fn strip_sigil<'a, 't>(tokens: &'a [Node<'t>]) -> &'a [Node<'t>] {
  match tokens {
    [sigil, rest @ ..] if matches!(sigil.kind(), "%" | "?") && !rest.is_empty() => rest,
    _ => tokens,
  }
}

impl ParsedMacro {
  fn parse(doc: Doc<'_>, start: usize, end: usize) -> Option<Self> {
    let text = format!("{PREFIX}{}{SUFFIX}", doc.slice(start, end));
    let tree = parse_rust(&text, ParseMode::StopAtFirstError).ok().flatten()?;
    let parsed = Self {
      base: doc.absolute(start),
      tree,
      text,
    };
    parsed.array().is_some().then_some(parsed)
  }

  pub(crate) fn doc(&self) -> Doc<'_> {
    Doc::embedded(&self.text, self.base, PREFIX.len())
  }

  pub(crate) fn elements(&self) -> Vec<Node<'_>> {
    self.array().map(expression_children).unwrap_or_default()
  }

  fn array(&self) -> Option<Node<'_>> {
    let root = self.tree.root_node();
    if root.has_error() {
      return None;
    }
    let body = root.named_child(0)?.child_by_field_name("body")?;
    let statement = body.named_child(0)?;
    let array = match statement.kind() {
      "expression_statement" => statement.named_child(0)?,
      _ => statement,
    };
    let spans_content = array.start_byte() + 1 == PREFIX.len() && array.end_byte() + SUFFIX.len() - 1 == self.text.len();
    (array.kind() == "array_expression" && spans_content).then_some(array)
  }
}

const NON_EXPRESSION_TOKENS: [&str; 17] = [
  "=>", ":", "->", "fn", "struct", "enum", "impl", "trait", "static", "let", "use", "mod", "type", "pub", "#", "$", "@",
];
const OPERATOR_KEYWORDS: [&str; 20] = [
  "as", "mut", "move", "ref", "if", "else", "match", "in", "async", "await", "loop", "while", "for", "return", "break", "continue", "dyn", "unsafe", "where", "box",
];

// Tree-sitter error recovery makes re-parsing non-expression macro bodies ~100x slower than valid ones, so screen them by token first.
fn looks_like_expression_list(doc: Doc<'_>, tokens: &[Node<'_>], repeat_form: bool) -> bool {
  let mut semicolons = 0;
  let mut previous_is_operand = false;
  for token in tokens {
    let kind = token.kind();
    if NON_EXPRESSION_TOKENS.contains(&kind) {
      return false;
    }
    if kind == ";" {
      semicolons += 1;
      if !repeat_form || semicolons > 1 {
        return false;
      }
    }
    let operand = is_operand(doc, *token);
    if operand && previous_is_operand {
      return false;
    }
    previous_is_operand = operand || (kind == "token_tree" && token.child(0).is_some_and(|open| open.kind() == "{"));
  }
  true
}

fn is_operand(doc: Doc<'_>, token: Node<'_>) -> bool {
  match token.kind() {
    "identifier" => !OPERATOR_KEYWORDS.contains(&doc.text(token)),
    kind => matches!(
      kind,
      "string_literal" | "raw_string_literal" | "integer_literal" | "float_literal" | "char_literal" | "boolean_literal" | "self" | "crate" | "super" | "primitive_type"
    ),
  }
}
