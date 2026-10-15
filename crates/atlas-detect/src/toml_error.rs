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

fn redact_echoed_string(message: &str) -> String {
  const MARKER: &str = "string \"";
  let Some(start) = message.find(MARKER) else { return message.to_owned() };
  let value_start = start + MARKER.len();
  let value_end = message.rfind("\", expected").unwrap_or(message.len());
  match value_end >= value_start {
    true => format!("{}...{}", &message[..value_start], &message[value_end..]),
    false => message.to_owned(),
  }
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
  fn should_omit_the_position_when_the_error_has_no_span() {
    assert_eq!(describe_toml_error("a = 1", "duplicate key", None), "duplicate key");
  }
}
