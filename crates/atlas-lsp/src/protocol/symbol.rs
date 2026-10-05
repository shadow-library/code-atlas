use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::document::{Location, Range};
use super::null_as_default;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SymbolKind(pub u32);

const SYMBOL_KIND_NAMES: [&str; 26] = [
  "file",
  "module",
  "namespace",
  "package",
  "class",
  "method",
  "property",
  "field",
  "constructor",
  "enum",
  "interface",
  "function",
  "variable",
  "constant",
  "string",
  "number",
  "boolean",
  "array",
  "object",
  "key",
  "null",
  "enum_member",
  "struct",
  "event",
  "operator",
  "type_parameter",
];

impl SymbolKind {
  pub fn name(self) -> &'static str {
    let index = usize::try_from(self.0).unwrap_or(0).wrapping_sub(1);
    SYMBOL_KIND_NAMES.get(index).copied().unwrap_or("unknown")
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbol {
  pub name: String,
  pub detail: Option<String>,
  pub kind: SymbolKind,
  #[serde(default, deserialize_with = "null_as_default")]
  pub tags: Vec<u32>,
  pub deprecated: Option<bool>,
  pub range: Range,
  pub selection_range: Range,
  #[serde(default, deserialize_with = "null_as_default")]
  pub children: Vec<DocumentSymbol>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolInformation {
  pub name: String,
  pub kind: SymbolKind,
  #[serde(default, deserialize_with = "null_as_default")]
  pub tags: Vec<u32>,
  pub deprecated: Option<bool>,
  pub location: Location,
  pub container_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentSymbolResponse {
  Nested(Vec<DocumentSymbol>),
  Flat(Vec<SymbolInformation>),
}

impl<'de> Deserialize<'de> for DocumentSymbolResponse {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    let items = match Value::deserialize(deserializer)? {
      Value::Null => return Ok(Self::Nested(Vec::new())),
      Value::Array(items) => items,
      other => return Err(D::Error::custom(format!("expected an array of symbols, got {other}"))),
    };
    let flat = items.iter().filter(|item| item.get("location").is_some()).count();
    match flat {
      0 => serde_json::from_value(Value::Array(items)).map(Self::Nested).map_err(D::Error::custom),
      count if count == items.len() => serde_json::from_value(Value::Array(items)).map(Self::Flat).map_err(D::Error::custom),
      _ => Err(D::Error::custom("document symbols mix DocumentSymbol and SymbolInformation")),
    }
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::super::document::Position;
  use super::*;

  fn json_range(line: u32, start: u32, end: u32) -> Value {
    json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
  }

  fn range(line: u32, start: u32, end: u32) -> Range {
    Range {
      start: Position { line, character: start },
      end: Position { line, character: end },
    }
  }

  #[test]
  fn should_read_hierarchical_document_symbols() {
    let response: DocumentSymbolResponse = serde_json::from_value(json!([{
      "name": "Store", "kind": 23, "range": json_range(0, 0, 30), "selectionRange": json_range(0, 7, 12),
      "children": [{"name": "get", "kind": 6, "detail": "fn(&self)", "range": json_range(1, 2, 20), "selectionRange": json_range(1, 5, 8), "children": null}]
    }]))
    .unwrap();

    let DocumentSymbolResponse::Nested(symbols) = response else {
      panic!("expected nested symbols, got {response:?}");
    };
    assert_eq!(symbols[0].name, "Store");
    assert_eq!(symbols[0].kind.name(), "struct");
    assert_eq!(symbols[0].selection_range, range(0, 7, 12));
    assert_eq!(symbols[0].children[0].name, "get");
    assert_eq!(symbols[0].children[0].detail.as_deref(), Some("fn(&self)"));
    assert_eq!(symbols[0].children[0].kind.name(), "method");
    assert_eq!(symbols[0].children[0].children, vec![]);
  }

  #[test]
  fn should_read_flat_symbol_information() {
    let response: DocumentSymbolResponse = serde_json::from_value(json!([{
      "name": "findOne", "kind": 6, "containerName": "UsersService",
      "location": {"uri": "file:///u.ts", "range": json_range(4, 2, 30)}
    }]))
    .unwrap();

    assert_eq!(
      response,
      DocumentSymbolResponse::Flat(vec![SymbolInformation {
        name: "findOne".to_owned(),
        kind: SymbolKind(6),
        location: Location {
          uri: "file:///u.ts".to_owned(),
          range: range(4, 2, 30)
        },
        container_name: Some("UsersService".to_owned()),
        ..SymbolInformation::default()
      }])
    );
  }

  #[test]
  fn should_read_null_or_empty_symbols_as_an_empty_outline_and_refuse_a_mix() {
    assert_eq!(
      serde_json::from_value::<DocumentSymbolResponse>(Value::Null).unwrap(),
      DocumentSymbolResponse::Nested(vec![])
    );
    assert_eq!(serde_json::from_value::<DocumentSymbolResponse>(json!([])).unwrap(), DocumentSymbolResponse::Nested(vec![]));
    let mixed = json!([
      {"name": "a", "kind": 12, "range": json_range(0, 0, 1), "selectionRange": json_range(0, 0, 1)},
      {"name": "b", "kind": 12, "location": {"uri": "file:///x", "range": json_range(1, 0, 1)}}
    ]);
    assert!(serde_json::from_value::<DocumentSymbolResponse>(mixed).is_err());
  }

  #[test]
  fn should_name_unknown_symbol_kinds_unknown() {
    assert_eq!(SymbolKind(0).name(), "unknown");
    assert_eq!(SymbolKind(26).name(), "type_parameter");
    assert_eq!(SymbolKind(27).name(), "unknown");
  }

  #[test]
  fn should_refuse_symbols_without_their_ranges() {
    let nested = serde_json::from_value::<DocumentSymbolResponse>(json!([{"name": "a", "kind": 12, "range": json_range(0, 0, 1)}]));
    let flat = serde_json::from_value::<DocumentSymbolResponse>(json!([{"name": "a", "kind": 12, "location": {"uri": "file:///x"}}]));

    assert!(nested.unwrap_err().to_string().contains("missing field `selectionRange`"));
    assert!(flat.unwrap_err().to_string().contains("missing field `range`"));
  }
}
