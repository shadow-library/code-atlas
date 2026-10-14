use atlas_core::{
  AnalysisError, AnalysisInput, Deadline, FileFacts, LanguageAnalyzer, LanguageFamily, MAX_REPORTED_PARSE_ERRORS, ParseBudget, ParseErrorFact, ParseStatus, Producer,
};
use tree_sitter::Node;

use crate::extract::Extractor;
use crate::grammar::Grammar;
use crate::syntax::{snippet, span_of, text};
use crate::tagged_templates::{Parsed, parse_tagged_templates, visit_errors};

pub const ANALYZER_NAME: &str = "atlas-lang-ts";
pub const ANALYZER_VERSION: u32 = 4;
pub const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TsAnalyzer {
  parse_budget: ParseBudget,
}

impl TsAnalyzer {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn with_parse_budget(self, parse_budget: ParseBudget) -> Self {
    Self { parse_budget }
  }

  fn cancelled(&self, input: &AnalysisInput<'_>, deadline: &Deadline, phase: &'static str) -> FileFacts {
    let reason = self.parse_budget.failure_reason(input.source.len());
    tracing::warn!(path = %input.file.path, phase, overrun = ?deadline.overrun(), factor = self.parse_budget.factor(), "ts analysis cancelled");
    FileFacts::failed(input.file.clone(), self.producer(), reason)
  }
}

impl LanguageAnalyzer for TsAnalyzer {
  fn language_family(&self) -> LanguageFamily {
    LanguageFamily::Ts
  }

  fn producer(&self) -> Producer {
    Producer::new(ANALYZER_NAME, ANALYZER_VERSION)
  }

  fn analyze(&self, input: &AnalysisInput<'_>) -> Result<FileFacts, AnalysisError> {
    let language = input.file.language;
    let grammar = Grammar::for_language(language).ok_or(AnalysisError::UnsupportedLanguage(language))?;
    if input.source.len() > MAX_SOURCE_BYTES {
      return Err(AnalysisError::SourceTooLarge {
        bytes: input.source.len(),
        limit: MAX_SOURCE_BYTES,
      });
    }
    let deadline = self.parse_budget.start(input.source.len());
    let Some(Parsed { tree, source }) = parse_tagged_templates(grammar, input.source, &deadline)? else {
      return Ok(self.cancelled(input, &deadline, "parse"));
    };
    let root = tree.root_node();
    let Some(mut facts) = Extractor::run(input.file.clone(), self.producer(), &source, root, &deadline) else {
      return Ok(self.cancelled(input, &deadline, "extraction"));
    };
    facts.parse_status = parse_status(root, &source);
    if let Err(violation) = facts.check_invariants(input.source.len()) {
      tracing::error!(path = %input.file.path, %violation, "ts analyzer produced facts that violate invariants");
      return Ok(FileFacts::failed(input.file.clone(), self.producer(), violation.to_string()));
    }
    tracing::debug!(
      path = %input.file.path,
      symbols = facts.symbols.len(),
      references = facts.references.len(),
      imports = facts.imports.len(),
      "analyzed ts file"
    );
    Ok(facts)
  }
}

fn parse_status(root: Node<'_>, source: &str) -> ParseStatus {
  if !root.has_error() {
    return ParseStatus::Ok;
  }
  let mut error_count: u32 = 0;
  let mut first_errors = Vec::new();
  visit_errors(root, |node| {
    error_count = error_count.saturating_add(1);
    if first_errors.len() < MAX_REPORTED_PARSE_ERRORS {
      first_errors.push(ParseErrorFact {
        span: span_of(node),
        message: error_message(node, source),
      });
    }
  });
  ParseStatus::Partial { error_count, first_errors }
}

fn error_message(node: Node<'_>, source: &str) -> String {
  match node.is_error() {
    true => format!("unexpected syntax `{}`", snippet(text(node, source))),
    false => format!("missing `{}`", node.kind()),
  }
}
