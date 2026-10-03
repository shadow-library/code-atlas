use atlas_core::{AnalysisError, AnalysisInput, FileFacts, LanguageAnalyzer, LanguageFamily, MAX_REPORTED_PARSE_ERRORS, ParseErrorFact, ParseStatus, Producer};
use tree_sitter::Node;

use crate::extract::Extractor;
use crate::grammar::{Grammar, parse};
use crate::syntax::{snippet, span_of, text};

pub const ANALYZER_NAME: &str = "atlas-lang-ts";
pub const ANALYZER_VERSION: u32 = 1;
pub const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct TsAnalyzer;

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
    let tree = parse(grammar, input.source)?;
    let root = tree.root_node();
    let mut facts = Extractor::run(input.file.clone(), self.producer(), input.source, root);
    facts.parse_status = parse_status(root, input.source);
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
  let mut cursor = root.walk();
  loop {
    let node = cursor.node();
    let message = match (node.is_error(), node.is_missing()) {
      (true, _) => Some(format!("unexpected syntax `{}`", snippet(text(node, source)))),
      (_, true) => Some(format!("missing `{}`", node.kind())),
      _ => None,
    };
    if let Some(message) = &message {
      error_count = error_count.saturating_add(1);
      if first_errors.len() < MAX_REPORTED_PARSE_ERRORS {
        first_errors.push(ParseErrorFact {
          span: span_of(node),
          message: message.clone(),
        });
      }
    }
    if message.is_none() && node.has_error() && cursor.goto_first_child() {
      continue;
    }
    loop {
      if cursor.goto_next_sibling() {
        break;
      }
      if !cursor.goto_parent() {
        return ParseStatus::Partial { error_count, first_errors };
      }
    }
  }
}
