use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
  pub line: u32,
  pub character: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Range {
  pub start: Position,
  pub end: Position,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
  pub uri: String,
  pub range: Range,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationLink {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub origin_selection_range: Option<Range>,
  pub target_uri: String,
  pub target_range: Range,
  pub target_selection_range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocationOrLink {
  Location(Location),
  Link(LocationLink),
}

impl LocationOrLink {
  pub fn into_location(self) -> Location {
    match self {
      Self::Location(location) => location,
      Self::Link(link) => Location {
        uri: link.target_uri,
        range: link.target_selection_range,
      },
    }
  }
}

impl<'de> Deserialize<'de> for LocationOrLink {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    let value = Value::deserialize(deserializer)?;
    let is_link = value.get("targetUri").is_some();
    match is_link {
      true => serde_json::from_value(value).map(Self::Link).map_err(D::Error::custom),
      false if value.get("uri").is_some() => serde_json::from_value(value).map(Self::Location).map_err(D::Error::custom),
      false => Err(D::Error::custom("expected a Location or a LocationLink")),
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GotoResponse(pub Vec<LocationOrLink>);

impl GotoResponse {
  pub fn into_locations(self) -> Vec<Location> {
    self.0.into_iter().map(LocationOrLink::into_location).collect()
  }
}

impl<'de> Deserialize<'de> for GotoResponse {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    match Value::deserialize(deserializer)? {
      Value::Null => Ok(Self::default()),
      Value::Array(items) => items
        .into_iter()
        .map(|item| serde_json::from_value(item).map_err(D::Error::custom))
        .collect::<Result<_, _>>()
        .map(Self),
      single => serde_json::from_value(single).map(|one| Self(vec![one])).map_err(D::Error::custom),
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
  pub uri: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedTextDocumentIdentifier {
  pub uri: String,
  pub version: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionalVersionedTextDocumentIdentifier {
  pub uri: String,
  pub version: Option<i32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentItem {
  pub uri: String,
  pub language_id: String,
  pub version: i32,
  pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentPositionParams {
  pub text_document: TextDocumentIdentifier,
  pub position: Position,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceContext {
  pub include_declaration: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceParams {
  #[serde(flatten)]
  pub position: TextDocumentPositionParams,
  pub context: ReferenceContext,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameParams {
  #[serde(flatten)]
  pub position: TextDocumentPositionParams,
  pub new_name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbolParams {
  pub text_document: TextDocumentIdentifier,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidOpenTextDocumentParams {
  pub text_document: TextDocumentItem,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentContentChangeEvent {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub range: Option<Range>,
  pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeTextDocumentParams {
  pub text_document: VersionedTextDocumentIdentifier,
  pub content_changes: Vec<TextDocumentContentChangeEvent>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidCloseTextDocumentParams {
  pub text_document: TextDocumentIdentifier,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidSaveTextDocumentParams {
  pub text_document: TextDocumentIdentifier,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeType {
  Created,
  Changed,
  Deleted,
}

impl Serialize for FileChangeType {
  fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_u8(match self {
      Self::Created => 1,
      Self::Changed => 2,
      Self::Deleted => 3,
    })
  }
}

impl<'de> Deserialize<'de> for FileChangeType {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    match u8::deserialize(deserializer)? {
      1 => Ok(Self::Created),
      2 => Ok(Self::Changed),
      3 => Ok(Self::Deleted),
      other => Err(D::Error::custom(format!("unknown file change type {other}"))),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEvent {
  pub uri: String,
  #[serde(rename = "type")]
  pub change: FileChangeType,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DidChangeWatchedFilesParams {
  pub changes: Vec<FileEvent>,
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  fn range(start: (u32, u32), end: (u32, u32)) -> Range {
    Range {
      start: Position {
        line: start.0,
        character: start.1,
      },
      end: Position { line: end.0, character: end.1 },
    }
  }

  #[test]
  fn should_read_goto_results_in_every_shape() {
    let location = json!({"uri": "file:///a.rs", "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}}});
    let link = json!({
      "targetUri": "file:///b.rs",
      "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 1}},
      "targetSelectionRange": {"start": {"line": 3, "character": 4}, "end": {"line": 3, "character": 7}}
    });
    let expected_a = Location {
      uri: "file:///a.rs".to_owned(),
      range: range((1, 2), (1, 5)),
    };
    let expected_b = Location {
      uri: "file:///b.rs".to_owned(),
      range: range((3, 4), (3, 7)),
    };
    let read = |value: Value| serde_json::from_value::<GotoResponse>(value).unwrap().into_locations();

    assert_eq!(read(Value::Null), vec![]);
    assert_eq!(read(location.clone()), vec![expected_a.clone()]);
    assert_eq!(read(json!([location, link])), vec![expected_a, expected_b]);
    assert!(serde_json::from_value::<GotoResponse>(json!([{"name": "x"}])).is_err());
  }

  #[test]
  fn should_flatten_position_params_into_reference_and_rename_params() {
    let position = TextDocumentPositionParams {
      text_document: TextDocumentIdentifier { uri: "file:///a.ts".to_owned() },
      position: Position { line: 2, character: 3 },
    };
    let references = ReferenceParams {
      position: position.clone(),
      context: ReferenceContext { include_declaration: false },
    };
    let rename = RenameParams {
      position,
      new_name: "b".to_owned(),
    };

    assert_eq!(
      serde_json::to_value(references).unwrap(),
      json!({"textDocument": {"uri": "file:///a.ts"}, "position": {"line": 2, "character": 3}, "context": {"includeDeclaration": false}})
    );
    assert_eq!(
      serde_json::to_value(rename).unwrap(),
      json!({"textDocument": {"uri": "file:///a.ts"}, "position": {"line": 2, "character": 3}, "newName": "b"})
    );
  }

  #[test]
  fn should_serialize_watched_file_events_as_numbers() {
    let params = DidChangeWatchedFilesParams {
      changes: vec![FileEvent {
        uri: "file:///a".to_owned(),
        change: FileChangeType::Deleted,
      }],
    };

    assert_eq!(serde_json::to_value(params).unwrap(), json!({"changes": [{"uri": "file:///a", "type": 3}]}));
  }

  #[test]
  fn should_refuse_locations_without_a_range_or_uri() {
    let missing_range = serde_json::from_value::<GotoResponse>(json!([{"uri": "file:///a.rs"}]));
    let missing_target =
      serde_json::from_value::<GotoResponse>(json!([{"targetUri": "file:///a.rs", "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}}]));
    let missing_uri = serde_json::from_value::<Location>(json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}}));

    assert!(missing_range.unwrap_err().to_string().contains("missing field `range`"));
    assert!(missing_target.unwrap_err().to_string().contains("missing field `targetSelectionRange`"));
    assert!(missing_uri.unwrap_err().to_string().contains("missing field `uri`"));
  }
}
