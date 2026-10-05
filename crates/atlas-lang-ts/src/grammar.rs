use std::cell::RefCell;

use atlas_core::{AnalysisError, Deadline, Language};
use tree_sitter::{ParseOptions, ParseState, Parser, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Grammar {
  TypeScript,
  Tsx,
  JavaScript,
}

impl Grammar {
  pub(crate) fn for_language(language: Language) -> Option<Self> {
    match language {
      Language::TypeScript => Some(Self::TypeScript),
      Language::Tsx => Some(Self::Tsx),
      Language::JavaScript | Language::Jsx => Some(Self::JavaScript),
      _ => None,
    }
  }

  fn language(self) -> tree_sitter::Language {
    match self {
      Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
      Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
      Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
    }
  }

  fn slot(self) -> usize {
    match self {
      Self::TypeScript => 0,
      Self::Tsx => 1,
      Self::JavaScript => 2,
    }
  }
}

thread_local! {
  static PARSERS: RefCell<[Option<Parser>; 3]> = const { RefCell::new([None, None, None]) };
}

pub(crate) fn parse(grammar: Grammar, source: &str, deadline: &Deadline) -> Result<Option<Tree>, AnalysisError> {
  PARSERS.with(|parsers| match parsers.try_borrow_mut() {
    Ok(mut parsers) => parse_with(&mut parsers[grammar.slot()], grammar, source, deadline),
    Err(_) => parse_with(&mut None, grammar, source, deadline),
  })
}

fn parse_with(slot: &mut Option<Parser>, grammar: Grammar, source: &str, deadline: &Deadline) -> Result<Option<Tree>, AnalysisError> {
  let parser = match slot {
    Some(parser) => parser,
    None => {
      let mut parser = Parser::new();
      parser.set_language(&grammar.language()).map_err(|error| AnalysisError::Parser(error.to_string()))?;
      slot.insert(parser)
    }
  };
  parser.reset();
  let bytes = source.as_bytes();
  let mut watch = deadline.parse_progress();
  let mut progress = |state: &ParseState| watch.observe(state.current_byte_offset());
  let options = ParseOptions::new().progress_callback(&mut progress);
  let tree = parser.parse_with_options(&mut |offset, _| bytes.get(offset..).unwrap_or_default(), None, Some(options));
  if tree.is_none() {
    // Drop the halted parse stack now: it can hold hundreds of MB until this grammar's next parse.
    parser.reset();
  }
  match tree {
    Some(tree) => Ok(Some(tree)),
    None if deadline.exceeded() => Ok(None),
    None => Err(AnalysisError::Parser(format!("tree-sitter produced no tree for the {grammar:?} grammar"))),
  }
}
