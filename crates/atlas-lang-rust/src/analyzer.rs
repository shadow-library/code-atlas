use std::cell::RefCell;
use std::ops::ControlFlow;

use atlas_core::{AnalysisError, AnalysisInput, Deadline, FileFacts, LanguageAnalyzer, LanguageFamily, ParseBudget, Producer};
use tree_sitter::{ParseOptions, ParseState, Parser, Tree};

use crate::extract::Extractor;

pub const ANALYZER_NAME: &str = "atlas-lang-rust";
pub const ANALYZER_VERSION: u32 = 3;
pub const DEFAULT_MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;

thread_local! {
  static PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustAnalyzer {
  max_source_bytes: usize,
  parse_budget: ParseBudget,
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
      parse_budget: ParseBudget::default(),
    }
  }

  pub fn with_max_source_bytes(self, max_source_bytes: usize) -> Self {
    Self { max_source_bytes, ..self }
  }

  pub fn with_parse_budget(self, parse_budget: ParseBudget) -> Self {
    Self { parse_budget, ..self }
  }

  fn cancelled(&self, input: &AnalysisInput<'_>, deadline: &Deadline, phase: &'static str) -> FileFacts {
    let reason = self.parse_budget.failure_reason(input.source.len());
    tracing::warn!(path = %input.file.path, phase, overrun = ?deadline.overrun(), factor = self.parse_budget.factor(), "rust analysis cancelled");
    FileFacts::failed(input.file.clone(), self.producer(), reason)
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
    let deadline = self.parse_budget.start(input.source.len());
    let Some(tree) = parse_rust(input.source, ParseMode::Recover, &deadline)? else {
      if deadline.exceeded() {
        return Ok(self.cancelled(input, &deadline, "parse"));
      }
      tracing::warn!(path = %input.file.path, "tree-sitter produced no tree for a rust file");
      return Ok(FileFacts::failed(input.file.clone(), self.producer(), "tree-sitter produced no syntax tree"));
    };
    match Extractor::new(input.file, input.source, self.producer()).run(&tree, &deadline) {
      Some(facts) => Ok(facts),
      None => Ok(self.cancelled(input, &deadline, "extraction")),
    }
  }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseMode {
  Recover,
  StopAtFirstError,
}

pub(crate) fn parse_rust(text: &str, mode: ParseMode, deadline: &Deadline) -> Result<Option<Tree>, AnalysisError> {
  PARSER.with(|slot| match slot.try_borrow_mut() {
    Ok(mut slot) => {
      let parser = match slot.as_mut() {
        Some(parser) => parser,
        None => slot.insert(new_parser()?),
      };
      Ok(parse_with(parser, text, mode, deadline))
    }
    Err(_) => Ok(parse_with(&mut new_parser()?, text, mode, deadline)),
  })
}

fn parse_with(parser: &mut Parser, text: &str, mode: ParseMode, deadline: &Deadline) -> Option<Tree> {
  let bytes = text.as_bytes();
  let mut watch = deadline.parse_progress();
  let mut progress = |state: &ParseState| match mode {
    ParseMode::StopAtFirstError if state.has_error() => ControlFlow::Break(()),
    ParseMode::Recover | ParseMode::StopAtFirstError => watch.observe(state.current_byte_offset()),
  };
  let options = ParseOptions::new().progress_callback(&mut progress);
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
