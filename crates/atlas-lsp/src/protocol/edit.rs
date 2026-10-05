use std::collections::BTreeMap;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::document::{OptionalVersionedTextDocumentIdentifier, Range};
use super::null_as_default;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "RawTextEdit")]
pub struct TextEdit {
  pub range: Range,
  pub new_text: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub annotation_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawTextEdit {
  range: Range,
  new_text: Option<String>,
  annotation_id: Option<String>,
  insert_text_format: Option<u8>,
  snippet: Option<Value>,
}

const PLAIN_TEXT_FORMAT: u8 = 1;

impl TryFrom<RawTextEdit> for TextEdit {
  type Error = String;

  fn try_from(raw: RawTextEdit) -> Result<Self, Self::Error> {
    if raw.snippet.is_some() || raw.insert_text_format.is_some_and(|format| format != PLAIN_TEXT_FORMAT) {
      return Err("snippet text edits are not supported".to_owned());
    }
    let Some(new_text) = raw.new_text else {
      return Err("text edit has no newText".to_owned());
    };
    Ok(Self {
      range: raw.range,
      new_text,
      annotation_id: raw.annotation_id,
    })
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentEdit {
  pub text_document: OptionalVersionedTextDocumentIdentifier,
  #[serde(default, deserialize_with = "null_as_default")]
  pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CreateFileOptions {
  pub overwrite: bool,
  pub ignore_if_exists: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DeleteFileOptions {
  pub recursive: bool,
  pub ignore_if_not_exists: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFile {
  pub uri: String,
  #[serde(default, deserialize_with = "null_as_default")]
  pub options: CreateFileOptions,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub annotation_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameFile {
  pub old_uri: String,
  pub new_uri: String,
  #[serde(default, deserialize_with = "null_as_default")]
  pub options: CreateFileOptions,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub annotation_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteFile {
  pub uri: String,
  #[serde(default, deserialize_with = "null_as_default")]
  pub options: DeleteFileOptions,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub annotation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentChange {
  Edit(TextDocumentEdit),
  Create(CreateFile),
  Rename(RenameFile),
  Delete(DeleteFile),
}

impl<'de> Deserialize<'de> for DocumentChange {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    let value = Value::deserialize(deserializer)?;
    let kind = value.get("kind").cloned();
    let decoded = match kind.as_ref().map(|kind| kind.as_str()) {
      None if value.get("textDocument").is_some() => serde_json::from_value(value).map(Self::Edit),
      Some(Some("create")) => serde_json::from_value(value).map(Self::Create),
      Some(Some("rename")) => serde_json::from_value(value).map(Self::Rename),
      Some(Some("delete")) => serde_json::from_value(value).map(Self::Delete),
      _ => return Err(D::Error::custom(format!("unknown document change {}", kind.unwrap_or(value)))),
    };
    decoded.map_err(D::Error::custom)
  }
}

impl DocumentChange {
  fn annotation_ids(&self) -> Vec<Option<&str>> {
    match self {
      Self::Edit(edit) => edit.edits.iter().map(|text| text.annotation_id.as_deref()).collect(),
      Self::Create(create) => vec![create.annotation_id.as_deref()],
      Self::Rename(rename) => vec![rename.annotation_id.as_deref()],
      Self::Delete(delete) => vec![delete.annotation_id.as_deref()],
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeAnnotation {
  #[serde(default)]
  pub label: String,
  #[serde(default)]
  pub needs_confirmation: bool,
  pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEdit {
  #[serde(default, deserialize_with = "null_as_default")]
  pub changes: BTreeMap<String, Vec<TextEdit>>,
  #[serde(default, deserialize_with = "null_as_default")]
  pub document_changes: Vec<DocumentChange>,
  #[serde(default, deserialize_with = "null_as_default")]
  pub change_annotations: BTreeMap<String, ChangeAnnotation>,
}

impl WorkspaceEdit {
  pub fn needs_confirmation(&self, annotation_id: Option<&str>) -> bool {
    let Some(id) = annotation_id else {
      return false;
    };
    self.change_annotations.get(id).is_none_or(|annotation| annotation.needs_confirmation)
  }

  pub fn requires_confirmation(&self) -> bool {
    let plain = self.changes.values().flatten().map(|edit| edit.annotation_id.as_deref());
    let documents = self.document_changes.iter().flat_map(DocumentChange::annotation_ids);
    plain.chain(documents).any(|id| self.needs_confirmation(id))
  }

  pub fn into_document_changes(self) -> Vec<DocumentChange> {
    if !self.document_changes.is_empty() {
      return self.document_changes;
    }
    self
      .changes
      .into_iter()
      .map(|(uri, edits)| {
        DocumentChange::Edit(TextDocumentEdit {
          text_document: OptionalVersionedTextDocumentIdentifier { uri, version: None },
          edits,
        })
      })
      .collect()
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareRenameResult {
  Range(Range),
  RangeWithPlaceholder { range: Range, placeholder: String },
  DefaultBehavior,
}

impl<'de> Deserialize<'de> for PrepareRenameResult {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    let value = Value::deserialize(deserializer)?;
    if value.get("defaultBehavior").is_some() {
      return Ok(Self::DefaultBehavior);
    }
    if let Some(range) = value.get("range") {
      let range = Range::deserialize(range).map_err(D::Error::custom)?;
      let placeholder = value.get("placeholder").and_then(Value::as_str).unwrap_or_default().to_owned();
      return Ok(Self::RangeWithPlaceholder { range, placeholder });
    }
    if value.get("start").is_some() && value.get("end").is_some() {
      return Range::deserialize(&value).map(Self::Range).map_err(D::Error::custom);
    }
    Err(D::Error::custom(format!("unrecognised prepareRename result {value}")))
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::super::document::Position;
  use super::*;

  fn range(line: u32, start: u32, end: u32) -> Range {
    Range {
      start: Position { line, character: start },
      end: Position { line, character: end },
    }
  }

  fn json_range(line: u32, start: u32, end: u32) -> Value {
    json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
  }

  #[test]
  fn should_read_document_changes_with_resource_operations() {
    let edit: WorkspaceEdit = serde_json::from_value(json!({
      "documentChanges": [
        {"textDocument": {"uri": "file:///a.ts", "version": 3}, "edits": [{"range": json_range(0, 4, 7), "newText": "b"}]},
        {"kind": "rename", "oldUri": "file:///a.ts", "newUri": "file:///b.ts"},
        {"kind": "create", "uri": "file:///c.ts", "options": {"ignoreIfExists": true}},
        {"kind": "delete", "uri": "file:///d.ts", "options": null}
      ],
      "changeAnnotations": {"x": {"label": "rename"}}
    }))
    .unwrap();

    assert_eq!(
      edit.into_document_changes(),
      vec![
        DocumentChange::Edit(TextDocumentEdit {
          text_document: OptionalVersionedTextDocumentIdentifier {
            uri: "file:///a.ts".to_owned(),
            version: Some(3)
          },
          edits: vec![TextEdit {
            range: range(0, 4, 7),
            new_text: "b".to_owned(),
            annotation_id: None
          }],
        }),
        DocumentChange::Rename(RenameFile {
          old_uri: "file:///a.ts".to_owned(),
          new_uri: "file:///b.ts".to_owned(),
          options: CreateFileOptions::default(),
          annotation_id: None
        }),
        DocumentChange::Create(CreateFile {
          uri: "file:///c.ts".to_owned(),
          options: CreateFileOptions {
            overwrite: false,
            ignore_if_exists: true
          },
          annotation_id: None
        }),
        DocumentChange::Delete(DeleteFile {
          uri: "file:///d.ts".to_owned(),
          options: DeleteFileOptions::default(),
          annotation_id: None
        }),
      ]
    );
  }

  #[test]
  fn should_turn_plain_changes_into_unversioned_document_edits_in_uri_order() {
    let edit: WorkspaceEdit = serde_json::from_value(json!({
      "changes": {
        "file:///z.rs": [{"range": json_range(1, 0, 3), "newText": "new"}],
        "file:///a.rs": [{"range": json_range(2, 5, 8), "newText": "new", "annotationId": "n"}]
      },
      "documentChanges": null
    }))
    .unwrap();
    let uris: Vec<_> = edit
      .into_document_changes()
      .into_iter()
      .map(|change| match change {
        DocumentChange::Edit(edit) => (edit.text_document.uri, edit.text_document.version, edit.edits.len()),
        other => panic!("unexpected {other:?}"),
      })
      .collect();

    assert_eq!(uris, vec![("file:///a.rs".to_owned(), None, 1), ("file:///z.rs".to_owned(), None, 1)]);
  }

  #[test]
  fn should_prefer_document_changes_over_changes() {
    let edit: WorkspaceEdit = serde_json::from_value(json!({
      "changes": {"file:///a.rs": []},
      "documentChanges": [{"kind": "delete", "uri": "file:///b.rs"}]
    }))
    .unwrap();

    assert_eq!(
      edit.into_document_changes(),
      vec![DocumentChange::Delete(DeleteFile {
        uri: "file:///b.rs".to_owned(),
        ..DeleteFile::default()
      })]
    );
  }

  #[test]
  fn should_refuse_an_unknown_document_change_instead_of_dropping_it() {
    let result = serde_json::from_value::<WorkspaceEdit>(json!({"documentChanges": [{"kind": "move", "uri": "file:///a"}]}));

    assert!(result.unwrap_err().to_string().contains("unknown document change \"move\""));
  }

  #[test]
  fn should_read_every_prepare_rename_shape() {
    let read = |value: Value| serde_json::from_value::<Option<PrepareRenameResult>>(value).unwrap();

    assert_eq!(read(Value::Null), None);
    assert_eq!(read(json_range(2, 4, 9)), Some(PrepareRenameResult::Range(range(2, 4, 9))));
    assert_eq!(
      read(json!({"range": json_range(1, 0, 3), "placeholder": "foo"})),
      Some(PrepareRenameResult::RangeWithPlaceholder {
        range: range(1, 0, 3),
        placeholder: "foo".to_owned()
      })
    );
    assert_eq!(read(json!({"defaultBehavior": true})), Some(PrepareRenameResult::DefaultBehavior));
    assert!(serde_json::from_value::<PrepareRenameResult>(json!({"name": "x"})).is_err());
  }

  fn edit_error(edit: Value) -> String {
    serde_json::from_value::<TextEdit>(edit).unwrap_err().to_string()
  }

  #[test]
  fn should_refuse_text_edits_that_would_invent_text_or_positions() {
    assert_eq!(edit_error(json!({"range": json_range(0, 1, 2)})), "text edit has no newText");
    assert!(edit_error(json!({"newText": "x"})).contains("missing field `range`"));
    assert!(edit_error(json!({"range": {"start": {"line": 0}, "end": {"line": 0, "character": 1}}, "newText": "x"})).contains("missing field `character`"));
    assert!(edit_error(json!({"range": {"start": {"line": 0, "character": 0}}, "newText": "x"})).contains("missing field `end`"));
  }

  #[test]
  fn should_refuse_snippet_and_unknown_text_edit_kinds() {
    assert_eq!(
      edit_error(json!({"range": json_range(0, 1, 2), "snippet": {"kind": "snippet", "value": "${1:x}"}})),
      "snippet text edits are not supported"
    );
    assert_eq!(
      edit_error(json!({"range": json_range(0, 1, 2), "newText": "$0x", "insertTextFormat": 2})),
      "snippet text edits are not supported"
    );
    assert!(edit_error(json!({"range": json_range(0, 1, 2), "newText": "x", "replace": true})).contains("unknown field `replace`"));
    let plain: TextEdit = serde_json::from_value(json!({"range": json_range(0, 1, 2), "newText": "x", "insertTextFormat": 1})).unwrap();
    assert_eq!(plain.new_text, "x");
  }

  #[test]
  fn should_refuse_a_workspace_edit_containing_a_snippet_edit() {
    let result = serde_json::from_value::<WorkspaceEdit>(json!({
      "documentChanges": [{"textDocument": {"uri": "file:///a.rs", "version": 1}, "edits": [{"range": json_range(0, 0, 1), "snippet": {"value": "x"}}]}]
    }));

    assert!(result.unwrap_err().to_string().contains("snippet text edits are not supported"));
  }

  #[test]
  fn should_flag_edits_whose_annotation_needs_confirmation() {
    let edit: WorkspaceEdit = serde_json::from_value(json!({
      "changes": {"file:///a.ts": [{"range": json_range(0, 0, 1), "newText": "b", "annotationId": "safe"}]},
      "documentChanges": [{"kind": "rename", "oldUri": "file:///a.ts", "newUri": "file:///b.ts", "annotationId": "ask"}],
      "changeAnnotations": {"safe": {"label": "rename"}, "ask": {"label": "rename file", "needsConfirmation": true}}
    }))
    .unwrap();
    let unconfirmed: WorkspaceEdit = serde_json::from_value(json!({
      "changes": {"file:///a.ts": [{"range": json_range(0, 0, 1), "newText": "b", "annotationId": "safe"}]},
      "changeAnnotations": {"safe": {"label": "rename", "needsConfirmation": false}}
    }))
    .unwrap();
    let dangling: WorkspaceEdit = serde_json::from_value(json!({
      "changes": {"file:///a.ts": [{"range": json_range(0, 0, 1), "newText": "b", "annotationId": "missing"}]}
    }))
    .unwrap();

    assert!(!edit.needs_confirmation(None));
    assert!(!edit.needs_confirmation(Some("safe")));
    assert!(edit.needs_confirmation(Some("ask")));
    assert!(edit.needs_confirmation(Some("unknown")));
    assert!(edit.requires_confirmation());
    assert!(!unconfirmed.requires_confirmation());
    assert!(dangling.requires_confirmation());
  }
}
