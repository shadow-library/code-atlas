use std::ops::Range;

pub fn toml_error_summary(text: &str, error: &toml::de::Error) -> String {
  describe_toml_error(text, error.message(), error.span())
}

pub fn describe_toml_error(text: &str, message: &str, span: Option<Range<usize>>) -> String {
  let message = redact_echoed_string(message);
  let Some(span) = span else { return message };
  let before = text.get(..span.start.min(text.len())).unwrap_or_default();
  let line = before.matches('\n').count() + 1;
  let column = before.rsplit('\n').next().map_or(0, |tail| tail.chars().count()) + 1;
  format!("line {line}, column {column}: {message}")
}

pub fn describe_value(value: &toml::Value) -> String {
  match value {
    toml::Value::Boolean(_) | toml::Value::Integer(_) | toml::Value::Float(_) => value.to_string(),
    toml::Value::String(_) => "a string".to_owned(),
    toml::Value::Datetime(_) => "a datetime".to_owned(),
    toml::Value::Array(_) => "an array".to_owned(),
    toml::Value::Table(_) => "a table".to_owned(),
  }
}

fn redact_echoed_string(message: &str) -> String {
  let message = redact_quoted(message, &["unknown variant `", "unknown field `"]);
  const MARKER: &str = "string \"";
  let Some(start) = message.find(MARKER) else { return message.to_owned() };
  let value_start = start + MARKER.len();
  let value_end = message.rfind("\", expected").unwrap_or(message.len());
  match value_end >= value_start {
    true => format!("{}...{}", &message[..value_start], &message[value_end..]),
    false => message.to_owned(),
  }
}

fn redact_quoted(message: &str, markers: &[&str]) -> String {
  let mut redacted = message.to_owned();
  for marker in markers {
    let Some(start) = redacted.find(marker) else { continue };
    let value_start = start + marker.len();
    let Some(length) = redacted[value_start..].find('`') else { continue };
    redacted.replace_range(value_start..value_start + length, "...");
  }
  redacted
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_report_line_and_column_of_a_syntax_error_without_the_source_line() {
    let text = "a = 1\nb = é\nkey = sk-FAKEKEY123\n";
    let error = toml::from_str::<toml::Table>(text).unwrap_err();

    let summary = toml_error_summary(text, &error);

    assert!(summary.starts_with("line 2, column 5: "), "{summary}");
    assert!(!summary.contains("FAKEKEY") && !summary.contains('|'), "{summary}");
  }

  #[test]
  fn should_redact_a_string_value_that_serde_echoes_in_its_message() {
    assert_eq!(
      redact_echoed_string("invalid type: string \"sk-FAKEKEY123\", expected a sequence"),
      "invalid type: string \"...\", expected a sequence"
    );
    assert_eq!(redact_echoed_string("missing field `name`"), "missing field `name`");
  }

  #[test]
  fn should_describe_strings_arrays_and_tables_by_type_and_scalars_by_value() {
    let table: toml::Table = toml::from_str("s = \"sk-FAKEKEY123\"\na = [\"sk-FAKEKEY123\"]\nt = { k = \"sk-FAKEKEY123\" }\nn = 7\nb = true\nf = 1.5\n").unwrap();
    let described: Vec<String> = ["s", "a", "t", "n", "b", "f"].iter().map(|key| describe_value(&table[*key])).collect();
    assert_eq!(described, ["a string", "an array", "a table", "7", "true", "1.5"]);
  }

  #[test]
  fn should_redact_the_name_serde_echoes_for_an_unknown_field_or_variant() {
    assert_eq!(
      redact_echoed_string("unknown variant `sk-FAKEKEY123`, expected `project` or `repo`"),
      "unknown variant `...`, expected `project` or `repo`"
    );
    assert_eq!(
      redact_echoed_string("unknown field `sk-FAKEKEY123`, expected one of `a`, `b`"),
      "unknown field `...`, expected one of `a`, `b`"
    );
  }

  #[test]
  fn should_omit_the_position_when_the_error_has_no_span() {
    assert_eq!(describe_toml_error("a = 1", "duplicate key", None), "duplicate key");
  }
}
