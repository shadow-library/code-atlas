use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::facts::{FileFacts, FileRef, LocalSymbolId};
use crate::graph::Attrs;
use crate::keys::{EntityKey, PackageEcosystem};
use crate::kinds::EdgeKind;
use crate::lang::{Language, LanguageFamily};
use crate::provenance::{Certainty, Producer};
use crate::span::Span;

#[derive(Debug, Clone, Copy)]
pub struct AnalysisInput<'a> {
  pub file: &'a FileRef,
  pub source: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum AnalysisError {
  #[error("analyzer does not support {0}")]
  UnsupportedLanguage(Language),
  #[error("source is {bytes} bytes, above the {limit} byte limit")]
  SourceTooLarge { bytes: usize, limit: usize },
  #[error("parser failure: {0}")]
  Parser(String),
}

pub trait LanguageAnalyzer: Send + Sync {
  fn language_family(&self) -> LanguageFamily;

  fn supports(&self, language: Language) -> bool {
    language.family() == self.language_family()
  }

  fn producer(&self) -> Producer;

  fn analyze(&self, input: &AnalysisInput<'_>) -> Result<FileFacts, AnalysisError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageContext {
  pub ecosystem: PackageEcosystem,
  pub name: String,
  pub dependencies: Vec<String>,
}

impl PackageContext {
  pub fn depends_on(&self, package: &str) -> bool {
    self.dependencies.iter().any(|dependency| dependency == package)
  }
}

#[derive(Debug, Clone, Copy)]
pub struct DetectInput<'a> {
  pub facts: &'a FileFacts,
  pub source: &'a str,
  pub package: Option<&'a PackageContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum NodeRef {
  Local(LocalSymbolId),
  Key(EntityKey),
  Unresolved { path: Vec<String>, from: Option<LocalSymbolId> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedEntity {
  pub key: EntityKey,
  pub name: String,
  pub certainty: Certainty,
  pub span: Span,
  pub attrs: Attrs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedEdge {
  pub kind: EdgeKind,
  pub from: NodeRef,
  pub to: NodeRef,
  pub certainty: Certainty,
  pub span: Span,
  pub note: Option<String>,
  pub attrs: Attrs,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detection {
  pub entities: Vec<DetectedEntity>,
  pub edges: Vec<DetectedEdge>,
}

impl Detection {
  pub fn is_empty(&self) -> bool {
    self.entities.is_empty() && self.edges.is_empty()
  }

  pub fn merge(&mut self, other: Detection) {
    self.entities.extend(other.entities);
    self.edges.extend(other.edges);
  }
}

pub trait Detector: Send + Sync {
  fn id(&self) -> &'static str;

  fn producer(&self) -> Producer;

  fn applies_to(&self, facts: &FileFacts) -> bool;

  fn detect(&self, input: &DetectInput<'_>) -> Detection;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Availability {
  Available,
  Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmRequest {
  pub system: String,
  pub prompt: String,
  pub max_output_tokens: u32,
  pub json_schema: Option<serde_json::Value>,
  pub timeout: Duration,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
  pub input: u64,
  pub output: u64,
  pub cache_read: u64,
  pub cache_write: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmResponse {
  pub text: String,
  pub usage: TokenUsage,
  pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum LlmError {
  #[error("LLM provider unavailable: {0}")]
  Unavailable(String),
  #[error("LLM call timed out after {0:?}")]
  Timeout(Duration),
  #[error("LLM provider rate limited the call")]
  RateLimited { retry_after: Option<Duration> },
  #[error("LLM returned an invalid response: {0}")]
  InvalidResponse(String),
  #[error("LLM provider error: {0}")]
  Provider(String),
}

pub trait LlmProvider: Send + Sync {
  fn id(&self) -> &str;

  fn model(&self) -> &str;

  fn availability(&self) -> Availability;

  fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::ids::{ContentHash, RelPath, RepoName};

  struct FixedAnalyzer;

  impl LanguageAnalyzer for FixedAnalyzer {
    fn language_family(&self) -> LanguageFamily {
      LanguageFamily::Rust
    }

    fn producer(&self) -> Producer {
      Producer::new("fixed", 1)
    }

    fn analyze(&self, input: &AnalysisInput<'_>) -> Result<FileFacts, AnalysisError> {
      if !self.supports(input.file.language) {
        return Err(AnalysisError::UnsupportedLanguage(input.file.language));
      }
      Ok(FileFacts::new(input.file.clone(), self.producer()))
    }
  }

  struct EnvDetector;

  impl Detector for EnvDetector {
    fn id(&self) -> &'static str {
      "env"
    }

    fn producer(&self) -> Producer {
      Producer::new("detect.env", 1)
    }

    fn applies_to(&self, facts: &FileFacts) -> bool {
      facts.file.language == Language::Rust
    }

    fn detect(&self, input: &DetectInput<'_>) -> Detection {
      let key = EntityKey::EnvVar {
        repo: input.facts.file.repo.clone(),
        name: "PORT".into(),
      };
      let span = Span::of_text(input.source, 0, input.source.len()).unwrap_or(Span::from_zero_based_rows(0, 0, 0, 0, 0, 0));
      Detection {
        entities: vec![DetectedEntity {
          key: key.clone(),
          name: "PORT".into(),
          certainty: Certainty::Syntactic,
          span,
          attrs: Attrs::new(),
        }],
        edges: vec![DetectedEdge {
          kind: EdgeKind::ReadsEnv,
          from: NodeRef::Local(LocalSymbolId(0)),
          to: NodeRef::Key(key),
          certainty: Certainty::Syntactic,
          span,
          note: None,
          attrs: Attrs::new(),
        }],
      }
    }
  }

  fn file(path: &str) -> FileRef {
    FileRef::detect(RepoName::new("acme").unwrap(), RelPath::new(path).unwrap(), ContentHash::of(b"")).unwrap()
  }

  #[test]
  fn should_gate_analysis_on_the_language_family() {
    let analyzer: Box<dyn LanguageAnalyzer> = Box::new(FixedAnalyzer);
    let rust = file("src/main.rs");
    assert!(
      analyzer
        .analyze(&AnalysisInput {
          file: &rust,
          source: "fn main() {}"
        })
        .is_ok()
    );
    let ts = file("src/main.ts");
    assert_eq!(
      analyzer.analyze(&AnalysisInput { file: &ts, source: "" }),
      Err(AnalysisError::UnsupportedLanguage(Language::TypeScript))
    );
  }

  #[test]
  fn should_emit_storable_detections_expressed_with_keys() {
    let facts = FileFacts::new(file("src/config.rs"), Producer::new("fixed", 1));
    let detector: Box<dyn Detector> = Box::new(EnvDetector);
    assert!(detector.applies_to(&facts));
    let mut detection = detector.detect(&DetectInput {
      facts: &facts,
      source: "env::var(\"PORT\")",
      package: None,
    });
    assert_eq!(detection.entities[0].key.to_string(), "env:acme:PORT");
    assert_eq!(
      detection.edges[0].to,
      NodeRef::Key(EntityKey::EnvVar {
        repo: RepoName::new("acme").unwrap(),
        name: "PORT".into()
      })
    );
    detection.merge(Detection::default());
    assert_eq!(detection.edges.len(), 1);
    assert!(Detection::default().is_empty());
    let json = serde_json::to_string(&detection).unwrap();
    assert_eq!(serde_json::from_str::<Detection>(&json).unwrap(), detection);
  }

  #[test]
  fn should_serialise_unresolved_node_refs_as_tagged_paths() {
    let node = NodeRef::Unresolved {
      path: vec!["JwtAuthGuard".into()],
      from: Some(LocalSymbolId(3)),
    };
    let json = serde_json::to_value(&node).unwrap();
    assert_eq!(json, serde_json::json!({ "type": "unresolved", "value": { "path": ["JwtAuthGuard"], "from": 3 } }));
    assert_eq!(serde_json::from_value::<NodeRef>(json).unwrap(), node);
    let local = serde_json::to_value(NodeRef::Local(LocalSymbolId(1))).unwrap();
    assert_eq!(local, serde_json::json!({ "type": "local", "value": 1 }));
  }

  #[test]
  fn should_check_package_dependencies_by_exact_name() {
    let package = PackageContext {
      ecosystem: PackageEcosystem::Npm,
      name: "@acme/api".into(),
      dependencies: vec!["@nestjs/common".into()],
    };
    assert!(package.depends_on("@nestjs/common"));
    assert!(!package.depends_on("@nestjs/core"));
  }
}
