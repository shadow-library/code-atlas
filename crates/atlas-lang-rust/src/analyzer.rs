use std::cell::RefCell;
use std::ops::ControlFlow;

use atlas_core::{AnalysisError, AnalysisInput, FileFacts, LanguageAnalyzer, LanguageFamily, Producer};
use tree_sitter::{ParseOptions, ParseState, Parser, Tree};

use crate::extract::Extractor;

pub const ANALYZER_NAME: &str = "atlas-lang-rust";
pub const ANALYZER_VERSION: u32 = 1;
pub const DEFAULT_MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;

thread_local! {
  static PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustAnalyzer {
  max_source_bytes: usize,
}

impl Default for RustAnalyzer {
  fn default() -> Self {
    Self::new()
  }
}

impl RustAnalyzer {
  pub fn new() -> Self {
    Self {
      max_source_bytes: DEFAULT_MAX_SOURCE_BYTES,
    }
  }

  pub fn with_max_source_bytes(self, max_source_bytes: usize) -> Self {
    Self { max_source_bytes }
  }
}

impl LanguageAnalyzer for RustAnalyzer {
  fn language_family(&self) -> LanguageFamily {
    LanguageFamily::Rust
  }

  fn producer(&self) -> Producer {
    Producer::new(ANALYZER_NAME, ANALYZER_VERSION)
  }

  fn analyze(&self, input: &AnalysisInput<'_>) -> Result<FileFacts, AnalysisError> {
    if !self.supports(input.file.language) {
      return Err(AnalysisError::UnsupportedLanguage(input.file.language));
    }
    if input.source.len() > self.max_source_bytes {
      return Err(AnalysisError::SourceTooLarge {
        bytes: input.source.len(),
        limit: self.max_source_bytes,
      });
    }
    let Some(tree) = parse_rust(input.source, ParseMode::Recover)? else {
      tracing::warn!(path = %input.file.path, "tree-sitter produced no tree for a rust file");
      return Ok(FileFacts::failed(input.file.clone(), self.producer(), "tree-sitter produced no syntax tree"));
    };
    Ok(Extractor::new(input.file, input.source, self.producer()).run(&tree))
  }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseMode {
  Recover,
  StopAtFirstError,
}

pub(crate) fn parse_rust(text: &str, mode: ParseMode) -> Result<Option<Tree>, AnalysisError> {
  PARSER.with(|slot| match slot.try_borrow_mut() {
    Ok(mut slot) => {
      let parser = match slot.as_mut() {
        Some(parser) => parser,
        None => slot.insert(new_parser()?),
      };
      Ok(parse_with(parser, text, mode))
    }
    Err(_) => Ok(parse_with(&mut new_parser()?, text, mode)),
  })
}

fn parse_with(parser: &mut Parser, text: &str, mode: ParseMode) -> Option<Tree> {
  if mode == ParseMode::Recover {
    return parser.parse(text, None);
  }
  let bytes = text.as_bytes();
  let mut stop_at_error = |state: &ParseState| if state.has_error() { ControlFlow::Break(()) } else { ControlFlow::Continue(()) };
  let options = ParseOptions::new().progress_callback(&mut stop_at_error);
  let tree = parser.parse_with_options(&mut |offset, _| bytes.get(offset..).unwrap_or_default(), None, Some(options));
  if tree.is_none() {
    // A halted tree-sitter parse resumes on the next call unless the parser is reset.
    parser.reset();
  }
  tree
}

fn new_parser() -> Result<Parser, AnalysisError> {
  let mut parser = Parser::new();
  parser
    .set_language(&tree_sitter_rust::LANGUAGE.into())
    .map_err(|error| AnalysisError::Parser(error.to_string()))?;
  Ok(parser)
}
