use serde_json::Value;

pub(crate) fn parse_json_object(text: &str) -> Result<Value, String> {
  let candidate = strip_fence(text.trim());
  let parsed = serde_json::from_str::<Value>(candidate).or_else(|error| {
    let (Some(start), Some(end)) = (candidate.find('{'), candidate.rfind('}')) else {
      return Err(error);
    };
    if start >= end {
      return Err(error);
    }
    serde_json::from_str::<Value>(&candidate[start..=end])
  });
  match parsed {
    Ok(value) if value.is_object() => Ok(value),
    Ok(_) => Err("response must be a JSON object".to_owned()),
    Err(error) => Err(format!("response is not valid JSON: {error}")),
  }
}

fn strip_fence(text: &str) -> &str {
  let Some(rest) = text.strip_prefix("```") else { return text };
  let rest = rest.split_once('\n').map_or(rest, |(_, body)| body);
  rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
}

pub(crate) fn validate(schema: &Value, value: &Value) -> Result<(), String> {
  check(schema, value, "$")
}

fn check(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
  if let Some(types) = schema.get("type") {
    check_type(types, value, path)?;
  }
  if let Some(options) = schema.get("enum").and_then(Value::as_array)
    && !options.contains(value)
  {
    let allowed = options.iter().map(Value::to_string).collect::<Vec<_>>().join(", ");
    return Err(format!("{path} must be one of {allowed}"));
  }
  match value {
    Value::Number(number) => check_range(schema, number.as_f64().unwrap_or(f64::NAN), path),
    Value::String(text) => check_string(schema, text, path),
    Value::Array(items) => check_array(schema, items, path),
    Value::Object(fields) => check_object(schema, fields, path),
    _ => Ok(()),
  }
}

fn check_type(types: &Value, value: &Value, path: &str) -> Result<(), String> {
  let names: Vec<&str> = match types {
    Value::String(name) => vec![name.as_str()],
    Value::Array(names) => names.iter().filter_map(Value::as_str).collect(),
    _ => return Ok(()),
  };
  if names.iter().any(|name| matches_type(name, value)) {
    return Ok(());
  }
  Err(format!("{path} must be of type {}, got {}", names.join(" or "), type_name(value)))
}

fn matches_type(name: &str, value: &Value) -> bool {
  match name {
    "object" => value.is_object(),
    "array" => value.is_array(),
    "string" => value.is_string(),
    "number" => value.is_number(),
    "integer" => value.is_i64() || value.is_u64(),
    "boolean" => value.is_boolean(),
    "null" => value.is_null(),
    _ => false,
  }
}

fn type_name(value: &Value) -> &'static str {
  match value {
    Value::Null => "null",
    Value::Bool(_) => "boolean",
    Value::Number(_) => "number",
    Value::String(_) => "string",
    Value::Array(_) => "array",
    Value::Object(_) => "object",
  }
}

fn check_range(schema: &Value, number: f64, path: &str) -> Result<(), String> {
  if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
    && number < minimum
  {
    return Err(format!("{path} must be at least {minimum}, got {number}"));
  }
  if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64)
    && number > maximum
  {
    return Err(format!("{path} must be at most {maximum}, got {number}"));
  }
  Ok(())
}

fn check_string(schema: &Value, text: &str, path: &str) -> Result<(), String> {
  let Some(min) = schema.get("minLength").and_then(Value::as_u64) else { return Ok(()) };
  if (text.trim().chars().count() as u64) < min {
    return Err(format!("{path} must not be empty"));
  }
  Ok(())
}

fn check_array(schema: &Value, items: &[Value], path: &str) -> Result<(), String> {
  if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
    && (items.len() as u64) < min
  {
    return Err(format!("{path} must have at least {min} items"));
  }
  if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
    && (items.len() as u64) > max
  {
    return Err(format!("{path} must have at most {max} items"));
  }
  let Some(item_schema) = schema.get("items") else { return Ok(()) };
  items.iter().enumerate().try_for_each(|(index, item)| check(item_schema, item, &format!("{path}[{index}]")))
}

fn check_object(schema: &Value, fields: &serde_json::Map<String, Value>, path: &str) -> Result<(), String> {
  let properties = schema.get("properties").and_then(Value::as_object);
  if let Some(required) = schema.get("required").and_then(Value::as_array) {
    for name in required.iter().filter_map(Value::as_str) {
      if !fields.contains_key(name) {
        return Err(format!("{path} is missing required property {name:?}"));
      }
    }
  }
  if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
    for name in fields.keys() {
      if !properties.is_some_and(|known| known.contains_key(name)) {
        return Err(format!("{path} has unexpected property {name:?}"));
      }
    }
  }
  let Some(properties) = properties else { return Ok(()) };
  for (name, property_schema) in properties {
    if let Some(field) = fields.get(name) {
      check(property_schema, field, &format!("{path}.{name}"))?;
    }
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn schema() -> Value {
    serde_json::from_str(
      r#"{"type":"object","additionalProperties":false,"required":["a","b"],
      "properties":{"a":{"type":"string","enum":["x","y"]},"b":{"type":["integer","null"],"minimum":0,"maximum":3},"c":{"type":"array","maxItems":2,"items":{"type":"string","minLength":1}}}}"#,
    )
    .unwrap()
  }

  fn check_json(text: &str) -> Result<(), String> {
    validate(&schema(), &serde_json::from_str(text).unwrap())
  }

  #[test]
  fn should_accept_a_conforming_object() {
    assert_eq!(check_json(r#"{"a":"x","b":null,"c":["k"]}"#), Ok(()));
  }

  #[test]
  fn should_report_the_path_of_the_first_violation() {
    assert_eq!(check_json(r#"{"a":"z","b":1}"#), Err(r#"$.a must be one of "x", "y""#.to_owned()));
    assert_eq!(check_json(r#"{"a":"x","b":4}"#), Err("$.b must be at most 3, got 4".to_owned()));
    assert_eq!(check_json(r#"{"a":"x","b":1.5}"#), Err("$.b must be of type integer or null, got number".to_owned()));
    assert_eq!(check_json(r#"{"a":"x","b":1,"c":["ok",""]}"#), Err("$.c[1] must not be empty".to_owned()));
    assert_eq!(check_json(r#"{"a":"x","b":1,"c":["1","2","3"]}"#), Err("$.c must have at most 2 items".to_owned()));
  }

  #[test]
  fn should_reject_missing_and_unexpected_properties() {
    assert_eq!(check_json(r#"{"a":"x"}"#), Err(r#"$ is missing required property "b""#.to_owned()));
    assert_eq!(check_json(r#"{"a":"x","b":1,"d":1}"#), Err(r#"$ has unexpected property "d""#.to_owned()));
  }

  #[test]
  fn should_extract_json_from_fenced_and_chatty_responses() {
    assert_eq!(parse_json_object("```json\n{\"a\":1}\n```").unwrap()["a"], 1);
    assert_eq!(parse_json_object("Sure! Here you go: {\"a\":2} hope it helps").unwrap()["a"], 2);
    assert!(parse_json_object("no json here").unwrap_err().starts_with("response is not valid JSON"));
    assert_eq!(parse_json_object("[1]").unwrap_err(), "response must be a JSON object");
  }
}
