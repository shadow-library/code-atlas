use std::cell::RefCell;

use atlas_core::{AnalysisError, Language};
use tree_sitter::{Parser, Tree};

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

pub(crate) fn parse(grammar: Grammar, source: &str) -> Result<Tree, AnalysisError> {
  PARSERS.with(|parsers| match parsers.try_borrow_mut() {
    Ok(mut parsers) => parse_with(&mut parsers[grammar.slot()], grammar, source),
    Err(_) => parse_with(&mut None, grammar, source),
  })
}

fn parse_with(slot: &mut Option<Parser>, grammar: Grammar, source: &str) -> Result<Tree, AnalysisError> {
  let parser = match slot {
    Some(parser) => parser,
    None => {
      let mut parser = Parser::new();
      parser.set_language(&grammar.language()).map_err(|error| AnalysisError::Parser(error.to_string()))?;
      slot.insert(parser)
    }
  };
  parser.reset();
  parser
    .parse(source, None)
    .ok_or_else(|| AnalysisError::Parser(format!("tree-sitter produced no tree for the {grammar:?} grammar")))
}
