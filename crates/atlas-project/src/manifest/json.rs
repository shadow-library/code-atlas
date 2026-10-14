use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
  Null,
  Bool(bool),
  Number(f64),
  String(String),
  Array(Vec<Json>),
  Object(Vec<(String, Json)>),
}

impl Json {
  pub fn get(&self, key: &str) -> Option<&Json> {
    let Self::Object(entries) = self else { return None };
    entries.iter().rev().find(|(name, _)| name == key).map(|(_, value)| value)
  }

  pub fn as_str(&self) -> Option<&str> {
    match self {
      Self::String(text) => Some(text),
      _ => None,
    }
  }

  pub fn as_bool(&self) -> Option<bool> {
    match self {
      Self::Bool(value) => Some(*value),
      _ => None,
    }
  }

  pub fn as_array(&self) -> Option<&[Json]> {
    match self {
      Self::Array(items) => Some(items),
      _ => None,
    }
  }

  pub fn as_object(&self) -> Option<&[(String, Json)]> {
    match self {
      Self::Object(entries) => Some(entries),
      _ => None,
    }
  }

  pub fn string_at(&self, key: &str) -> Option<String> {
    self.get(key)?.as_str().map(str::to_owned)
  }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
  type Value = Json;

  fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str("any JSON value")
  }

  fn visit_unit<E>(self) -> Result<Json, E> {
    Ok(Json::Null)
  }

  fn visit_none<E>(self) -> Result<Json, E> {
    Ok(Json::Null)
  }

  fn visit_bool<E>(self, value: bool) -> Result<Json, E> {
    Ok(Json::Bool(value))
  }

  fn visit_i64<E>(self, value: i64) -> Result<Json, E> {
    Ok(Json::Number(value as f64))
  }

  fn visit_u64<E>(self, value: u64) -> Result<Json, E> {
    Ok(Json::Number(value as f64))
  }

  fn visit_f64<E>(self, value: f64) -> Result<Json, E> {
    Ok(Json::Number(value))
  }

  fn visit_str<E>(self, value: &str) -> Result<Json, E> {
    Ok(Json::String(value.to_owned()))
  }

  fn visit_string<E>(self, value: String) -> Result<Json, E> {
    Ok(Json::String(value))
  }

  fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
    let mut items = Vec::new();
    while let Some(item) = seq.next_element()? {
      items.push(item);
    }
    Ok(Json::Array(items))
  }

  fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
    let mut entries = Vec::new();
    while let Some((key, value)) = map.next_entry()? {
      entries.push((key, value));
    }
    Ok(Json::Object(entries))
  }
}

impl<'de> Deserialize<'de> for Json {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    deserializer.deserialize_any(JsonVisitor)
  }
}

pub fn parse_json(text: &str) -> Result<Json, serde_json::Error> {
  serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(text))
}

pub fn parse_jsonc(text: &str) -> Result<Json, String> {
  let stripped = jsonc_to_json(text)?;
  serde_json::from_str(&stripped).map_err(|error| error.to_string())
}

pub fn jsonc_to_json(text: &str) -> Result<String, String> {
  let without_comments = strip_comments(text.strip_prefix('\u{feff}').unwrap_or(text))?;
  Ok(strip_trailing_commas(&without_comments))
}

fn strip_comments(text: &str) -> Result<String, String> {
  let mut output = String::with_capacity(text.len());
  let mut chars = text.chars().peekable();
  let mut in_string = false;
  while let Some(c) = chars.next() {
    if in_string {
      output.push(c);
      match c {
        '\\' => {
          if let Some(escaped) = chars.next() {
            output.push(escaped);
          }
        }
        '"' => in_string = false,
        _ => {}
      }
      continue;
    }
    match (c, chars.peek()) {
      ('"', _) => {
        in_string = true;
        output.push(c);
      }
      ('/', Some('/')) => {
        for next in chars.by_ref() {
          if next == '\n' {
            output.push('\n');
            break;
          }
        }
      }
      ('/', Some('*')) => {
        chars.next();
        let mut previous = '\0';
        let mut closed = false;
        for next in chars.by_ref() {
          if previous == '*' && next == '/' {
            closed = true;
            break;
          }
          if next == '\n' {
            output.push('\n');
          }
          previous = next;
        }
        if !closed {
          return Err("unterminated block comment".to_owned());
        }
        output.push(' ');
      }
      _ => output.push(c),
    }
  }
  Ok(output)
}

fn strip_trailing_commas(text: &str) -> String {
  let chars: Vec<char> = text.chars().collect();
  let mut output = String::with_capacity(text.len());
  let mut in_string = false;
  let mut index = 0;
  while index < chars.len() {
    let c = chars[index];
    index += 1;
    if in_string {
      output.push(c);
      match c {
        '\\' => {
          if let Some(escaped) = chars.get(index) {
            output.push(*escaped);
            index += 1;
          }
        }
        '"' => in_string = false,
        _ => {}
      }
      continue;
    }
    if c == '"' {
      in_string = true;
      output.push(c);
      continue;
    }
    if c == ',' {
      let next_significant = chars[index..].iter().find(|next| !next.is_whitespace());
      if matches!(next_significant, Some('}' | ']')) {
        continue;
      }
    }
    output.push(c);
  }
  output
}

#[cfg(test)]
mod tests {
  use super::*;

  fn parsed(text: &str) -> Json {
    parse_jsonc(text).unwrap()
  }

  #[test]
  fn should_strip_line_and_block_comments() {
    let json = parsed("{\n  // line\n  \"a\": 1, /* block\n spanning */ \"b\": 2\n}");
    assert_eq!(json.get("a"), Some(&Json::Number(1.0)));
    assert_eq!(json.get("b"), Some(&Json::Number(2.0)));
  }

  #[test]
  fn should_keep_comment_markers_inside_strings() {
    let json = parsed(r#"{"url": "https://example.com/a//b", "glob": "src/**/*.ts", "c": "/* not a comment */"}"#);
    assert_eq!(json.string_at("url").as_deref(), Some("https://example.com/a//b"));
    assert_eq!(json.string_at("glob").as_deref(), Some("src/**/*.ts"));
    assert_eq!(json.string_at("c").as_deref(), Some("/* not a comment */"));
  }

  #[test]
  fn should_respect_escaped_quotes_and_backslashes_in_strings() {
    let json = parsed(
      r#"{"a": "say \"// hi\"", "b": "ends with backslash\\", // real comment
      "c": "x"}"#,
    );
    assert_eq!(json.string_at("a").as_deref(), Some("say \"// hi\""));
    assert_eq!(json.string_at("b").as_deref(), Some("ends with backslash\\"));
    assert_eq!(json.string_at("c").as_deref(), Some("x"));
  }

  #[test]
  fn should_remove_trailing_commas_in_objects_and_arrays() {
    let json = parsed("{\"a\": [1, 2, ], \"b\": {\"c\": 3,}, }");
    assert_eq!(json.get("a"), Some(&Json::Array(vec![Json::Number(1.0), Json::Number(2.0)])));
    assert_eq!(json.get("b").and_then(|b| b.get("c")), Some(&Json::Number(3.0)));
  }

  #[test]
  fn should_remove_trailing_commas_followed_by_comments() {
    let json = parsed("{\"a\": [1, // one\n 2, /* two */ ], }");
    assert_eq!(json.get("a").and_then(Json::as_array).map(<[Json]>::len), Some(2));
  }

  #[test]
  fn should_keep_commas_inside_strings_before_closing_brackets() {
    let json = parsed(r#"{"a": "x,]", "b": ",}"}"#);
    assert_eq!(json.string_at("a").as_deref(), Some("x,]"));
    assert_eq!(json.string_at("b").as_deref(), Some(",}"));
  }

  #[test]
  fn should_accept_a_byte_order_mark() {
    assert_eq!(parsed("\u{feff}{\"a\": true}").get("a"), Some(&Json::Bool(true)));
  }

  #[test]
  fn should_reject_an_unterminated_block_comment() {
    assert_eq!(parse_jsonc("{ /* oops").unwrap_err(), "unterminated block comment");
  }

  #[test]
  fn should_still_reject_genuinely_invalid_json() {
    assert!(parse_jsonc("{\"a\": }").is_err());
    assert!(parse_jsonc("{\"a\" 1}").is_err());
  }

  #[test]
  fn should_keep_object_key_order() {
    let json = parse_json(r#"{"z": 1, "a": 2, "m": 3}"#).unwrap();
    let keys: Vec<_> = json.as_object().unwrap().iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys, ["z", "a", "m"]);
  }
}
