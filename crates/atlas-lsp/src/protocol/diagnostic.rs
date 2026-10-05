use std::collections::BTreeMap;
use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::document::{Location, Range, TextDocumentIdentifier};
use super::null_as_default;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticSeverity {
  Error,
  Warning,
  Information,
  Hint,
}

impl DiagnosticSeverity {
  fn from_number(number: u64) -> Option<Self> {
    match number {
      1 => Some(Self::Error),
      2 => Some(Self::Warning),
      3 => Some(Self::Information),
      4 => Some(Self::Hint),
      _ => None,
    }
  }
}

fn lenient_severity<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<DiagnosticSeverity>, D::Error> {
  Ok(
    Option::<Value>::deserialize(deserializer)?
      .as_ref()
      .and_then(Value::as_u64)
      .and_then(DiagnosticSeverity::from_number),
  )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NumberOrString {
  Number(i64),
  String(String),
}

impl fmt::Display for NumberOrString {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Number(number) => write!(formatter, "{number}"),
      Self::String(text) => formatter.write_str(text),
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct DiagnosticRelatedInformation {
  pub location: Location,
  pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
  pub range: Range,
  #[serde(default, deserialize_with = "lenient_severity")]
  pub severity: Option<DiagnosticSeverity>,
  pub code: Option<NumberOrString>,
  pub source: Option<String>,
  pub message: String,
  #[serde(default, deserialize_with = "null_as_default")]
  pub tags: Vec<u32>,
  #[serde(default, deserialize_with = "null_as_default")]
  pub related_information: Vec<DiagnosticRelatedInformation>,
  pub data: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct PublishDiagnosticsParams {
  pub uri: String,
  pub version: Option<i32>,
  #[serde(default, deserialize_with = "null_as_default")]
  pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentDiagnosticParams {
  pub text_document: TextDocumentIdentifier,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub identifier: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub previous_result_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DocumentDiagnosticReport {
  Full {
    result_id: Option<String>,
    items: Vec<Diagnostic>,
    related_documents: BTreeMap<String, DocumentDiagnosticReport>,
  },
  Unchanged {
    result_id: String,
    related_documents: BTreeMap<String, DocumentDiagnosticReport>,
  },
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RawReport {
  kind: String,
  result_id: Option<String>,
  #[serde(default, deserialize_with = "null_as_default")]
  items: Vec<Diagnostic>,
  #[serde(default, deserialize_with = "null_as_default")]
  related_documents: BTreeMap<String, DocumentDiagnosticReport>,
}

impl<'de> Deserialize<'de> for DocumentDiagnosticReport {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    let raw = RawReport::deserialize(deserializer)?;
    match raw.kind.as_str() {
      "full" => Ok(Self::Full {
        result_id: raw.result_id,
        items: raw.items,
        related_documents: raw.related_documents,
      }),
      "unchanged" => Ok(Self::Unchanged {
        result_id: raw.result_id.unwrap_or_default(),
        related_documents: raw.related_documents,
      }),
      other => Err(D::Error::custom(format!("unknown diagnostic report kind {other:?}"))),
    }
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  #[test]
  fn should_read_published_diagnostics_leniently() {
    let params: PublishDiagnosticsParams = serde_json::from_value(json!({
      "uri": "file:///a.ts",
      "version": 4,
      "diagnostics": [
        {"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 9}}, "severity": 1, "code": 2307, "source": "ts", "message": "Cannot find module"},
        {"range": {"start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 1}}, "severity": 9, "code": "E0425", "message": "x", "relatedInformation": null}
      ]
    }))
    .unwrap();

    assert_eq!(params.version, Some(4));
    assert_eq!(params.diagnostics[0].severity, Some(DiagnosticSeverity::Error));
    assert_eq!(params.diagnostics[0].code.as_ref().map(ToString::to_string).as_deref(), Some("2307"));
    assert_eq!(params.diagnostics[1].severity, None);
    assert_eq!(params.diagnostics[1].code, Some(NumberOrString::String("E0425".to_owned())));
    assert_eq!(params.diagnostics[1].related_information, vec![]);
  }

  #[test]
  fn should_read_full_and_unchanged_document_diagnostic_reports() {
    let report: DocumentDiagnosticReport = serde_json::from_value(json!({
      "kind": "full",
      "resultId": "7",
      "items": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "severity": 2, "message": "unused"}],
      "relatedDocuments": {"file:///b.rs": {"kind": "unchanged", "resultId": "3"}}
    }))
    .unwrap();

    let DocumentDiagnosticReport::Full {
      result_id,
      items,
      related_documents,
    } = report
    else {
      panic!("expected a full report");
    };
    assert_eq!(result_id.as_deref(), Some("7"));
    assert_eq!(items[0].message, "unused");
    assert_eq!(items[0].severity, Some(DiagnosticSeverity::Warning));
    assert_eq!(
      related_documents.get("file:///b.rs"),
      Some(&DocumentDiagnosticReport::Unchanged {
        result_id: "3".to_owned(),
        related_documents: BTreeMap::new()
      })
    );
    assert!(serde_json::from_value::<DocumentDiagnosticReport>(json!({"kind": "partial"})).is_err());
  }

  #[test]
  fn should_refuse_diagnostics_without_a_range_or_message() {
    let missing_range = serde_json::from_value::<PublishDiagnosticsParams>(json!({"uri": "file:///a.rs", "diagnostics": [{"message": "x"}]}));
    let missing_message = serde_json::from_value::<Diagnostic>(json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}}));

    assert!(missing_range.unwrap_err().to_string().contains("missing field `range`"));
    assert!(missing_message.unwrap_err().to_string().contains("missing field `message`"));
  }
}
