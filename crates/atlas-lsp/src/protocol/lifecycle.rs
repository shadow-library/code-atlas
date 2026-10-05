use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use super::diagnostic::NumberOrString;
use super::jsonrpc::RequestId;
use super::null_as_default;
use crate::position::PositionEncoding;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InitializeParams {
  pub process_id: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub client_info: Option<ClientInfo>,
  pub root_uri: Option<String>,
  pub capabilities: ClientCapabilities,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub initialization_options: Option<Value>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub workspace_folders: Option<Vec<WorkspaceFolder>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientInfo {
  pub name: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClientCapabilities {
  pub general: GeneralClientCapabilities,
  #[serde(skip_serializing_if = "Value::is_null")]
  pub workspace: Value,
  #[serde(skip_serializing_if = "Value::is_null")]
  pub text_document: Value,
  #[serde(skip_serializing_if = "Value::is_null")]
  pub window: Value,
  #[serde(skip_serializing_if = "Value::is_null")]
  pub experimental: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GeneralClientCapabilities {
  pub position_encodings: Vec<PositionEncoding>,
}

impl ClientCapabilities {
  pub fn standard(position_encodings: Vec<PositionEncoding>) -> Self {
    Self {
      general: GeneralClientCapabilities { position_encodings },
      workspace: json!({
        "workspaceFolders": true,
        "configuration": true,
        "didChangeWatchedFiles": {"dynamicRegistration": false},
        "workspaceEdit": {"documentChanges": true, "resourceOperations": ["create", "rename", "delete"], "failureHandling": "abort"}
      }),
      text_document: json!({
        "synchronization": {"dynamicRegistration": false, "didSave": true},
        "rename": {"prepareSupport": true},
        "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
        "publishDiagnostics": {"versionSupport": true},
        "diagnostic": {"dynamicRegistration": false},
        "references": {},
        "definition": {"linkSupport": true},
        "declaration": {"linkSupport": true},
        "implementation": {"linkSupport": true}
      }),
      window: json!({"workDoneProgress": true}),
      experimental: json!({"serverStatusNotification": true}),
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceFolder {
  pub uri: String,
  pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InitializeResult {
  pub capabilities: ServerCapabilities,
  pub server_info: Option<ServerInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ServerInfo {
  pub name: String,
  pub version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerCapabilities {
  pub position_encoding: Option<String>,
  pub text_document_sync: TextDocumentSync,
  pub rename_provider: Capability,
  pub references_provider: Capability,
  pub definition_provider: Capability,
  pub declaration_provider: Capability,
  pub implementation_provider: Capability,
  pub document_symbol_provider: Capability,
  pub diagnostic_provider: Option<DiagnosticOptions>,
  pub experimental: Option<Value>,
}

impl ServerCapabilities {
  pub fn supports_prepare_rename(&self) -> bool {
    self.rename_provider.flag("prepareProvider")
  }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum Capability {
  #[default]
  Unsupported,
  Supported(Value),
}

impl Capability {
  pub fn is_supported(&self) -> bool {
    matches!(self, Self::Supported(_))
  }

  pub fn flag(&self, option: &str) -> bool {
    match self {
      Self::Supported(options) => options.get(option).and_then(Value::as_bool).unwrap_or(false),
      Self::Unsupported => false,
    }
  }
}

impl<'de> Deserialize<'de> for Capability {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
      Value::Bool(true) => Self::Supported(Value::Null),
      options @ Value::Object(_) => Self::Supported(options),
      _ => Self::Unsupported,
    })
  }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TextDocumentSyncKind {
  #[default]
  None,
  Full,
  Incremental,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextDocumentSync {
  pub open_close: bool,
  pub change: TextDocumentSyncKind,
  pub save: Option<SaveOptions>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SaveOptions {
  pub include_text: bool,
}

fn sync_kind(number: Option<u64>) -> TextDocumentSyncKind {
  match number {
    Some(1) => TextDocumentSyncKind::Full,
    Some(2) => TextDocumentSyncKind::Incremental,
    _ => TextDocumentSyncKind::None,
  }
}

impl<'de> Deserialize<'de> for TextDocumentSync {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    let value = Value::deserialize(deserializer)?;
    if let Some(number) = value.as_u64() {
      return Ok(Self {
        open_close: true,
        change: sync_kind(Some(number)),
        save: Some(SaveOptions::default()),
      });
    }
    let save = match value.get("save") {
      Some(Value::Bool(true)) => Some(SaveOptions::default()),
      Some(options @ Value::Object(_)) => Some(SaveOptions::deserialize(options).unwrap_or_default()),
      _ => None,
    };
    Ok(Self {
      open_close: value.get("openClose").and_then(Value::as_bool).unwrap_or(false),
      change: sync_kind(value.get("change").and_then(Value::as_u64)),
      save,
    })
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiagnosticOptions {
  pub identifier: Option<String>,
  pub inter_file_dependencies: bool,
  pub workspace_diagnostics: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ConfigurationParams {
  #[serde(deserialize_with = "null_as_default")]
  pub items: Vec<ConfigurationItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ConfigurationItem {
  pub scope_uri: Option<String>,
  pub section: Option<String>,
}

pub type ProgressToken = NumberOrString;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorkDoneProgressCreateParams {
  pub token: ProgressToken,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ProgressParams {
  pub token: ProgressToken,
  #[serde(default)]
  pub value: Value,
}

impl ProgressParams {
  pub fn work_done(&self) -> Option<WorkDoneProgress> {
    WorkDoneProgress::deserialize(&self.value).ok()
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WorkDoneProgress {
  Begin(WorkDoneProgressBegin),
  Report(WorkDoneProgressReport),
  End(WorkDoneProgressEnd),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct WorkDoneProgressBegin {
  pub title: String,
  pub cancellable: Option<bool>,
  pub message: Option<String>,
  pub percentage: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct WorkDoneProgressReport {
  pub cancellable: Option<bool>,
  pub message: Option<String>,
  pub percentage: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct WorkDoneProgressEnd {
  pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
  Ok,
  Warning,
  Error,
  #[default]
  #[serde(other)]
  Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ServerStatusParams {
  pub health: Health,
  pub quiescent: bool,
  pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MessageType {
  Error,
  Warning,
  Info,
  #[default]
  Log,
  Debug,
}

impl<'de> Deserialize<'de> for MessageType {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    Ok(match Value::deserialize(deserializer)?.as_u64() {
      Some(1) => Self::Error,
      Some(2) => Self::Warning,
      Some(3) => Self::Info,
      Some(5) => Self::Debug,
      _ => Self::Log,
    })
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ShowMessageParams {
  #[serde(rename = "type")]
  pub kind: MessageType,
  pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyWorkspaceEditResult {
  pub applied: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelParams {
  pub id: RequestId,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_read_server_capabilities_in_their_boolean_and_option_forms() {
    let result: InitializeResult = serde_json::from_value(json!({
      "capabilities": {
        "positionEncoding": "utf-8",
        "textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": false}},
        "renameProvider": {"prepareProvider": true},
        "referencesProvider": true,
        "definitionProvider": false,
        "implementationProvider": null,
        "documentSymbolProvider": {"label": "x"},
        "diagnosticProvider": {"identifier": "rust-analyzer", "interFileDependencies": true, "workspaceDiagnostics": false},
        "unknownProvider": 42
      },
      "serverInfo": {"name": "rust-analyzer", "version": "1.97.1"}
    }))
    .unwrap();
    let capabilities = &result.capabilities;

    assert_eq!(capabilities.position_encoding.as_deref(), Some("utf-8"));
    assert_eq!(
      capabilities.text_document_sync,
      TextDocumentSync {
        open_close: true,
        change: TextDocumentSyncKind::Incremental,
        save: Some(SaveOptions { include_text: false })
      }
    );
    assert!(capabilities.supports_prepare_rename());
    assert!(capabilities.references_provider.is_supported());
    assert!(!capabilities.definition_provider.is_supported());
    assert!(!capabilities.implementation_provider.is_supported());
    assert!(!capabilities.declaration_provider.is_supported());
    assert!(capabilities.document_symbol_provider.is_supported());
    assert_eq!(
      capabilities.diagnostic_provider.as_ref().and_then(|options| options.identifier.as_deref()),
      Some("rust-analyzer")
    );
    assert_eq!(
      result.server_info,
      Some(ServerInfo {
        name: "rust-analyzer".to_owned(),
        version: Some("1.97.1".to_owned())
      })
    );
  }

  #[test]
  fn should_read_a_numeric_text_document_sync_kind_and_an_empty_result() {
    let result: InitializeResult = serde_json::from_value(json!({"capabilities": {"textDocumentSync": 1, "renameProvider": true}})).unwrap();

    assert_eq!(
      result.capabilities.text_document_sync,
      TextDocumentSync {
        open_close: true,
        change: TextDocumentSyncKind::Full,
        save: Some(SaveOptions::default())
      }
    );
    assert!(result.capabilities.rename_provider.is_supported());
    assert!(!result.capabilities.supports_prepare_rename());
    assert_eq!(serde_json::from_value::<InitializeResult>(json!({})).unwrap(), InitializeResult::default());
  }

  #[test]
  fn should_offer_the_standard_client_capabilities() {
    let params = InitializeParams {
      process_id: Some(7),
      capabilities: ClientCapabilities::standard(vec![PositionEncoding::Utf8, PositionEncoding::Utf16]),
      ..InitializeParams::default()
    };
    let value = serde_json::to_value(params).unwrap();

    assert_eq!(value["processId"], json!(7));
    assert_eq!(value["rootUri"], Value::Null);
    assert_eq!(value["capabilities"]["general"]["positionEncodings"], json!(["utf-8", "utf-16"]));
    assert_eq!(
      value["capabilities"]["workspace"]["workspaceEdit"]["resourceOperations"],
      json!(["create", "rename", "delete"])
    );
    assert_eq!(value["capabilities"]["textDocument"]["rename"]["prepareSupport"], json!(true));
    assert_eq!(value["capabilities"]["window"]["workDoneProgress"], json!(true));
    assert_eq!(value["capabilities"]["experimental"]["serverStatusNotification"], json!(true));
  }

  #[test]
  fn should_read_work_done_progress_and_server_status() {
    let progress: ProgressParams = serde_json::from_value(json!({"token": "rustAnalyzer/Indexing", "value": {"kind": "report", "message": "3/10", "percentage": 30}})).unwrap();
    let end: ProgressParams = serde_json::from_value(json!({"token": 4, "value": {"kind": "end"}})).unwrap();
    let partial: ProgressParams = serde_json::from_value(json!({"token": 4, "value": [{"uri": "file:///a"}]})).unwrap();
    let status: ServerStatusParams = serde_json::from_value(json!({"health": "warning", "quiescent": true, "message": "fetch failed"})).unwrap();
    let odd: ServerStatusParams = serde_json::from_value(json!({"health": "degraded"})).unwrap();

    assert_eq!(progress.token, NumberOrString::String("rustAnalyzer/Indexing".to_owned()));
    assert_eq!(
      progress.work_done(),
      Some(WorkDoneProgress::Report(WorkDoneProgressReport {
        cancellable: None,
        message: Some("3/10".to_owned()),
        percentage: Some(30)
      }))
    );
    assert_eq!(end.work_done(), Some(WorkDoneProgress::End(WorkDoneProgressEnd::default())));
    assert_eq!(partial.work_done(), None);
    assert_eq!(
      status,
      ServerStatusParams {
        health: Health::Warning,
        quiescent: true,
        message: Some("fetch failed".to_owned())
      }
    );
    assert_eq!(odd.health, Health::Unknown);
  }
}
