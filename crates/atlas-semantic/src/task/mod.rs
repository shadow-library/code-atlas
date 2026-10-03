#[cfg(test)]
mod golden;
mod link;
mod similarity;
mod summary;

use atlas_core::{Confidence, ContentHash, EntityId, RelPath, RepoName, Span};
use atlas_store::StoredInferredFact;
use serde_json::{Map, Value};

pub use link::*;
pub use similarity::*;
pub use summary::*;

pub const MAX_SNIPPET_LINES: usize = 120;
pub const TRUNCATION_MARKER: &str = "…truncated";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
  pub repo: RepoName,
  pub path: RelPath,
  pub span: Span,
  pub file_hash: ContentHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaskKind {
  LinkUnresolvedClientCall,
  ClassifySimilarityPair,
  SummarizeCapability,
}

impl TaskKind {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::LinkUnresolvedClientCall => "link_unresolved_client_call",
      Self::ClassifySimilarityPair => "classify_similarity_pair",
      Self::SummarizeCapability => "summarize_capability",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticTask {
  ClassifySimilarityPair(SimilarityPair),
  SummarizeCapability(CapabilityInput),
  LinkUnresolvedClientCall(ClientCallLink),
}

pub(crate) struct RenderedPrompt {
  pub kind: TaskKind,
  pub template_version: u32,
  pub system: String,
  pub prompt: String,
  pub schema: Value,
}

pub(crate) struct EvidenceSource {
  pub source: SourceRef,
  pub note: String,
}

pub(crate) struct FactDraft {
  pub relation: &'static str,
  pub subjects: Vec<EntityId>,
  pub confidence: Confidence,
  pub rationale: String,
  pub payload: Map<String, Value>,
  pub evidence: Vec<EvidenceSource>,
}

pub(crate) enum Interpretation {
  NoRelation,
  Fact(FactDraft),
}

impl SemanticTask {
  pub fn kind(&self) -> TaskKind {
    match self {
      Self::ClassifySimilarityPair(_) => TaskKind::ClassifySimilarityPair,
      Self::SummarizeCapability(_) => TaskKind::SummarizeCapability,
      Self::LinkUnresolvedClientCall(_) => TaskKind::LinkUnresolvedClientCall,
    }
  }

  pub fn subjects(&self) -> Vec<EntityId> {
    match self {
      Self::ClassifySimilarityPair(pair) => pair.subjects(),
      Self::SummarizeCapability(input) => vec![input.entity],
      Self::LinkUnresolvedClientCall(link) => vec![link.call.call_site],
    }
  }

  pub fn sources(&self) -> Vec<&SourceRef> {
    match self {
      Self::ClassifySimilarityPair(pair) => pair.sources(),
      Self::SummarizeCapability(input) => input.sources.iter().collect(),
      Self::LinkUnresolvedClientCall(link) => link.sources(),
    }
  }

  pub(crate) fn render(&self) -> RenderedPrompt {
    match self {
      Self::ClassifySimilarityPair(pair) => pair.render(),
      Self::SummarizeCapability(input) => input.render(),
      Self::LinkUnresolvedClientCall(link) => link.render(),
    }
  }

  pub(crate) fn interpret(&self, output: &Value) -> Result<Interpretation, String> {
    match self {
      Self::ClassifySimilarityPair(pair) => pair.interpret(output),
      Self::SummarizeCapability(input) => input.interpret(output),
      Self::LinkUnresolvedClientCall(link) => link.interpret(output),
    }
  }

  pub(crate) fn supersedes(&self, existing: &StoredInferredFact) -> bool {
    match self {
      Self::ClassifySimilarityPair(pair) => pair.supersedes(existing),
      Self::SummarizeCapability(input) => input.supersedes(existing),
      Self::LinkUnresolvedClientCall(link) => link.supersedes(existing),
    }
  }
}

pub(crate) fn bound_snippet(text: &str, max_lines: usize) -> String {
  let total = text.lines().count();
  if total <= max_lines {
    return text.trim_end().to_owned();
  }
  let kept = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
  format!("{kept}\n{TRUNCATION_MARKER} ({} more lines omitted)", total - max_lines)
}

const FENCE_TAGS: [&str; 4] = ["symbol", "module", "call", "candidates"];

pub(crate) fn fence_safe(text: &str) -> String {
  let mut escaped = String::with_capacity(text.len());
  for (index, ch) in text.char_indices() {
    if ch != '<' {
      escaped.push(ch);
      continue;
    }
    let rest = text[index + 1..].trim_start().to_ascii_lowercase();
    let tag_like = rest.starts_with('/') || FENCE_TAGS.iter().any(|tag| rest.starts_with(tag));
    escaped.push_str(if tag_like { "&lt;" } else { "<" });
  }
  escaped
}

pub(crate) fn snippet_note(shown: &str) -> String {
  format!("snippet blake3:{}", ContentHash::of(shown.as_bytes()))
}

pub(crate) fn json_only_instruction(schema_text: &str) -> String {
  format!("Respond with a single JSON object and nothing else: no prose, no code fences. It must match this JSON Schema:\n{schema_text}")
}

pub(crate) fn parse_schema(schema_text: &str) -> Value {
  serde_json::from_str(schema_text).unwrap_or(Value::Null)
}

pub(crate) fn to_confidence(value: f32) -> Result<Confidence, String> {
  Confidence::new(value).map_err(|error| error.to_string())
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(output: &Value) -> Result<T, String> {
  serde_json::from_value(output.clone()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_keep_short_snippets_untouched() {
    assert_eq!(bound_snippet("a\nb\n", 3), "a\nb");
  }

  #[test]
  fn should_truncate_long_snippets_with_an_explicit_marker() {
    let text = (1..=5).map(|n| format!("l{n}")).collect::<Vec<_>>().join("\n");
    assert_eq!(bound_snippet(&text, 3), "l1\nl2\nl3\n…truncated (2 more lines omitted)");
  }

  #[test]
  fn should_neutralise_tag_lookalikes_case_and_whitespace_insensitively() {
    assert_eq!(
      fence_safe("a </SYMBOL > b < /call> <Module x> Vec<String>"),
      "a &lt;/SYMBOL > b &lt; /call> &lt;Module x> Vec<String>"
    );
  }
}
