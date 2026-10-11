use std::ops::Range;

use serde_json::Value;

#[derive(Debug, PartialEq, Eq)]
pub enum JsonEditError {
  Unsupported(&'static str),
}

struct Member {
  key: String,
  line_indent: Option<String>,
  value: Range<usize>,
}

struct Object {
  close: usize,
  members: Vec<Member>,
}

struct Layout {
  eol: &'static str,
  multiline: bool,
  unit: String,
}

pub fn upsert_server(text: &str, path: &[&str], key: &str, value: &Value) -> Result<Option<String>, JsonEditError> {
  let layout = layout(text);
  let open = skip_ws(text, 0);
  upsert(text, open, "", path, key, value, &layout)
}

fn upsert(text: &str, open: usize, base: &str, path: &[&str], key: &str, value: &Value, layout: &Layout) -> Result<Option<String>, JsonEditError> {
  let object = scan_object(text, open)?;
  let (name, rest) = match path.split_first() {
    Some((name, rest)) => (*name, Some(rest)),
    None => (key, None),
  };
  let mut matching = object.members.iter().filter(|member| member.key == name);
  let existing = matching.next();
  if matching.next().is_some() {
    return Err(JsonEditError::Unsupported("duplicate key"));
  }
  let child_base = format!("{base}{}", if layout.multiline { layout.unit.as_str() } else { "" });
  match (existing, rest) {
    (Some(member), None) => {
      let current: Value = serde_json::from_str(&text[member.value.clone()]).map_err(|_| JsonEditError::Unsupported("existing entry is not valid JSON"))?;
      if &current == value {
        return Ok(None);
      }
      let member_base = member.line_indent.clone().unwrap_or_else(|| child_base.clone());
      Ok(Some(splice(text, member.value.clone(), &render(value, &member_base, layout))))
    }
    (Some(member), Some(rest)) => {
      if !text[member.value.clone()].starts_with('{') {
        return Err(JsonEditError::Unsupported("`mcpServers` is not an object"));
      }
      let member_base = member.line_indent.clone().unwrap_or_else(|| child_base.clone());
      upsert(text, member.value.start, &member_base, rest, key, value, layout)
    }
    (None, rest) => {
      let inserted = match rest {
        Some(rest) => nest(rest, key, value),
        None => value.clone(),
      };
      Ok(Some(insert_member(text, &object, name, &inserted, &child_base, base, layout)))
    }
  }
}

fn nest(path: &[&str], key: &str, value: &Value) -> Value {
  let mut inner = serde_json::Map::new();
  match path.split_first() {
    Some((name, rest)) => inner.insert((*name).to_owned(), nest(rest, key, value)),
    None => inner.insert(key.to_owned(), value.clone()),
  };
  Value::Object(inner)
}

fn insert_member(text: &str, object: &Object, name: &str, value: &Value, child_base: &str, base: &str, layout: &Layout) -> String {
  let name_json = Value::String(name.to_owned()).to_string();
  match (object.members.last(), layout.multiline) {
    (Some(last), true) => {
      let indent = last.line_indent.clone().unwrap_or_else(|| child_base.to_owned());
      let member = format!(",{}{indent}{name_json}: {}", layout.eol, render(value, &indent, layout));
      splice(text, last.value.end..last.value.end, &member)
    }
    (Some(last), false) => splice(text, last.value.end..last.value.end, &format!(",{name_json}:{}", render(value, "", layout))),
    (None, true) => {
      let member = format!("{eol}{child_base}{name_json}: {}{eol}{base}", render(value, child_base, layout), eol = layout.eol);
      splice(text, object.close_range_start(text)..object.close, &member)
    }
    (None, false) => splice(text, object.close_range_start(text)..object.close, &format!("{name_json}:{}", render(value, "", layout))),
  }
}

impl Object {
  fn close_range_start(&self, text: &str) -> usize {
    let open = text[..self.close].rfind('{').unwrap_or(0);
    open + 1
  }
}

fn splice(text: &str, range: Range<usize>, replacement: &str) -> String {
  format!("{}{replacement}{}", &text[..range.start], &text[range.end..])
}

fn render(value: &Value, base: &str, layout: &Layout) -> String {
  if !layout.multiline {
    return value.to_string();
  }
  let mut buffer = Vec::new();
  let formatter = serde_json::ser::PrettyFormatter::with_indent(layout.unit.as_bytes());
  let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
  if serde::Serialize::serialize(value, &mut serializer).is_err() {
    return value.to_string();
  }
  String::from_utf8_lossy(&buffer).replace('\n', &format!("{}{base}", layout.eol))
}

fn layout(text: &str) -> Layout {
  let multiline = text.trim_end().contains('\n');
  let unit = text
    .lines()
    .find_map(|line| {
      let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
      (!indent.is_empty() && indent.len() < line.len()).then_some(indent)
    })
    .unwrap_or_else(|| "  ".to_owned());
  let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
  Layout { eol, multiline, unit }
}

fn skip_ws(text: &str, from: usize) -> usize {
  from + text[from..].len() - text[from..].trim_start().len()
}

fn line_indent(text: &str, at: usize) -> Option<String> {
  let before = &text[..at];
  let line_start = before.rfind('\n')? + 1;
  let indent = &before[line_start..];
  indent.chars().all(|c| c == ' ' || c == '\t').then(|| indent.to_owned())
}

fn scan_string(text: &str, start: usize) -> usize {
  let bytes = text.as_bytes();
  let mut index = start + 1;
  while index < bytes.len() {
    match bytes[index] {
      b'\\' => index += 2,
      b'"' => return index + 1,
      _ => index += 1,
    }
  }
  index
}

fn scan_value(text: &str, start: usize) -> usize {
  let bytes = text.as_bytes();
  match bytes[start] {
    b'"' => scan_string(text, start),
    b'{' | b'[' => {
      let mut depth = 0usize;
      let mut index = start;
      while index < bytes.len() {
        match bytes[index] {
          b'"' => {
            index = scan_string(text, index);
            continue;
          }
          b'{' | b'[' => depth += 1,
          b'}' | b']' => {
            depth -= 1;
            if depth == 0 {
              return index + 1;
            }
          }
          _ => {}
        }
        index += 1;
      }
      index
    }
    _ => {
      start
        + text[start..]
          .find(|c: char| matches!(c, ',' | '}' | ']') || c.is_whitespace())
          .unwrap_or(text.len() - start)
    }
  }
}

fn scan_object(text: &str, open: usize) -> Result<Object, JsonEditError> {
  let bytes = text.as_bytes();
  if bytes.get(open) != Some(&b'{') {
    return Err(JsonEditError::Unsupported("expected a JSON object"));
  }
  let mut members = Vec::new();
  let mut index = skip_ws(text, open + 1);
  while bytes.get(index) == Some(&b'"') {
    let key_end = scan_string(text, index);
    let key: String = serde_json::from_str(&text[index..key_end]).map_err(|_| JsonEditError::Unsupported("unreadable key"))?;
    let colon = skip_ws(text, key_end);
    let value_start = skip_ws(text, colon + 1);
    let value_end = scan_value(text, value_start);
    members.push(Member {
      key,
      line_indent: line_indent(text, index),
      value: value_start..value_end,
    });
    index = skip_ws(text, value_end);
    if bytes.get(index) == Some(&b',') {
      index = skip_ws(text, index + 1);
    }
  }
  match bytes.get(index) {
    Some(b'}') => Ok(Object { close: index, members }),
    _ => Err(JsonEditError::Unsupported("unexpected JSON structure")),
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  fn server() -> Value {
    json!({"type": "stdio", "command": "/bin/catl", "args": ["mcp", "--project", "p"]})
  }

  #[test]
  fn should_keep_every_other_byte_when_adding_to_existing_servers() {
    let text = "{\n  \"zeta\": 1.10,\n  \"mcpServers\": {\n    \"other\": { \"command\": \"x\", \"v\": 2.50 }\n  },\n  \"alpha\": [1, 2]\n}\n";
    let edited = upsert_server(text, &["mcpServers"], "code-atlas", &server()).unwrap().unwrap();
    assert!(edited.starts_with("{\n  \"zeta\": 1.10,\n  \"mcpServers\": {\n    \"other\": { \"command\": \"x\", \"v\": 2.50 },\n    \"code-atlas\": {\n      \"args\": ["));
    assert!(edited.ends_with("\n    }\n  },\n  \"alpha\": [1, 2]\n}\n"));
    let parsed: Value = serde_json::from_str(&edited).unwrap();
    assert_eq!(parsed["mcpServers"]["code-atlas"], server());
  }

  #[test]
  fn should_add_the_servers_object_when_missing() {
    let text = "{\n  \"a\": 1\n}\n";
    let edited = upsert_server(text, &["mcpServers"], "code-atlas", &server()).unwrap().unwrap();
    assert!(edited.starts_with("{\n  \"a\": 1,\n  \"mcpServers\": {\n    \"code-atlas\": {"));
    assert_eq!(serde_json::from_str::<Value>(&edited).unwrap()["mcpServers"]["code-atlas"], server());
  }

  #[test]
  fn should_fill_empty_objects() {
    for text in ["{}", "{\n}\n", "{\"mcpServers\":{}}"] {
      let edited = upsert_server(text, &["mcpServers"], "code-atlas", &server()).unwrap().unwrap();
      assert_eq!(serde_json::from_str::<Value>(&edited).unwrap()["mcpServers"]["code-atlas"], server(), "{edited}");
    }
  }

  #[test]
  fn should_replace_only_our_entry_and_report_no_change_when_equal() {
    let text = "{\"mcpServers\":{\"code-atlas\":{\"command\":\"old\"},\"n\":1.0}}";
    let edited = upsert_server(text, &["mcpServers"], "code-atlas", &server()).unwrap().unwrap();
    assert!(edited.ends_with(",\"n\":1.0}}"));
    assert_eq!(upsert_server(&edited, &["mcpServers"], "code-atlas", &server()).unwrap(), None);
  }

  #[test]
  fn should_refuse_duplicate_keys_on_the_path() {
    let duplicate_entry = "{\"mcpServers\":{\"code-atlas\":{\"a\":1},\"code-atlas\":{\"a\":2}}}";
    let duplicate_servers = "{\"mcpServers\":{},\"mcpServers\":{}}";
    for text in [duplicate_entry, duplicate_servers] {
      assert_eq!(
        upsert_server(text, &["mcpServers"], "code-atlas", &server()),
        Err(JsonEditError::Unsupported("duplicate key"))
      );
    }
  }

  #[test]
  fn should_keep_crlf_line_endings_for_everything_it_inserts() {
    let text = "{\r\n  \"mcpServers\": {\r\n    \"other\": {}\r\n  }\r\n}\r\n";
    let edited = upsert_server(text, &["mcpServers"], "code-atlas", &server()).unwrap().unwrap();
    assert!(!edited.replace("\r\n", "").contains('\n'), "{edited:?}");
    let empty = upsert_server("{\r\n  \"a\": 1\r\n}\r\n", &["mcpServers"], "code-atlas", &server()).unwrap().unwrap();
    assert!(!empty.replace("\r\n", "").contains('\n'), "{empty:?}");
    assert_eq!(serde_json::from_str::<Value>(&edited).unwrap()["mcpServers"]["code-atlas"], server());
  }

  #[test]
  fn should_refuse_a_servers_member_that_is_not_an_object() {
    assert_eq!(
      upsert_server("{\"mcpServers\": []}", &["mcpServers"], "code-atlas", &server()),
      Err(JsonEditError::Unsupported("`mcpServers` is not an object"))
    );
  }
}
