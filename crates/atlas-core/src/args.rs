use std::borrow::Cow;

use serde::{Deserialize, Serialize};

pub const ARG_MAX_DEPTH: usize = 4;
pub const ARG_MAX_WIDTH: usize = 16;
pub const ARG_MAX_TEXT: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ArgSummary {
  Str(String),
  Template(Vec<TemplatePart>),
  Number(String),
  Bool(bool),
  Null,
  Path(Vec<String>),
  Call { callee: Vec<String>, args: Vec<ArgSummary> },
  Object(Vec<ObjectEntry>),
  Array(Vec<ArgSummary>),
  Closure { params: Vec<String> },
  Concat(Vec<ArgSummary>),
  Named { name: String, value: Box<ArgSummary> },
  Other(String),
  Truncated,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum TemplatePart {
  Lit(String),
  Expr(ArgSummary),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectEntry {
  pub key: ObjectKey,
  pub value: ArgSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ObjectKey {
  Ident(String),
  Str(String),
  Computed(String),
  Spread,
}

impl ObjectKey {
  pub fn static_name(&self) -> Option<&str> {
    match self {
      Self::Ident(name) | Self::Str(name) => Some(name),
      Self::Computed(_) | Self::Spread => None,
    }
  }
}

pub fn truncate_text(text: &str) -> String {
  let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
  if collapsed.len() <= ARG_MAX_TEXT {
    return collapsed;
  }
  let mut end = ARG_MAX_TEXT - '…'.len_utf8();
  while !collapsed.is_char_boundary(end) {
    end -= 1;
  }
  format!("{}…", &collapsed[..end])
}

impl ArgSummary {
  pub fn other(text: &str) -> Self {
    Self::Other(truncate_text(text))
  }

  pub fn as_static_str(&self) -> Option<Cow<'_, str>> {
    if let Self::Str(value) = self {
      return Some(Cow::Borrowed(value));
    }
    let parts = self.string_parts()?;
    match parts.as_slice() {
      [] => Some(Cow::Borrowed("")),
      [TemplatePart::Lit(value)] => Some(Cow::Owned(value.clone())),
      _ => None,
    }
  }

  pub fn static_prefix(&self) -> Option<String> {
    let parts = self.string_parts()?;
    match parts.first() {
      Some(TemplatePart::Lit(value)) => Some(value.clone()),
      Some(TemplatePart::Expr(_)) | None => Some(String::new()),
    }
  }

  pub fn to_pattern(&self, placeholder: &str) -> Option<String> {
    let parts = self.string_parts()?;
    Some(
      parts
        .iter()
        .map(|part| match part {
          TemplatePart::Lit(value) => value.as_str(),
          TemplatePart::Expr(_) => placeholder,
        })
        .collect(),
    )
  }

  pub fn string_parts(&self) -> Option<Vec<TemplatePart>> {
    let mut parts = Vec::new();
    if !self.collect_string_parts(&mut parts) {
      return None;
    }
    Some(parts)
  }

  fn collect_string_parts(&self, parts: &mut Vec<TemplatePart>) -> bool {
    match self {
      Self::Str(value) => push_literal(parts, value),
      Self::Template(template) => {
        for part in template {
          match part {
            TemplatePart::Lit(value) => push_literal(parts, value),
            TemplatePart::Expr(inner) => match inner.as_static_str() {
              Some(value) => push_literal(parts, &value),
              None => parts.push(TemplatePart::Expr(inner.clone())),
            },
          }
        }
      }
      Self::Concat(items) => {
        let mut saw_string = false;
        let mut collected = Vec::new();
        for item in items {
          if item.collect_string_parts(&mut collected) {
            saw_string = true;
          } else {
            collected.push(TemplatePart::Expr(item.clone()));
          }
        }
        if !saw_string {
          return false;
        }
        for part in collected {
          match part {
            TemplatePart::Lit(value) => push_literal(parts, &value),
            expr => parts.push(expr),
          }
        }
      }
      _ => return false,
    }
    true
  }
}

fn push_literal(parts: &mut Vec<TemplatePart>, value: &str) {
  if value.is_empty() {
    return;
  }
  if let Some(TemplatePart::Lit(previous)) = parts.last_mut() {
    previous.push_str(value);
    return;
  }
  parts.push(TemplatePart::Lit(value.to_owned()));
}

#[cfg(test)]
mod tests {
  use super::*;

  fn lit(value: &str) -> TemplatePart {
    TemplatePart::Lit(value.to_owned())
  }

  fn path(segments: &[&str]) -> ArgSummary {
    ArgSummary::Path(segments.iter().map(|segment| (*segment).to_owned()).collect())
  }

  #[test]
  fn should_return_static_strings_only_for_fully_literal_values() {
    assert_eq!(ArgSummary::Str("/users".into()).as_static_str().as_deref(), Some("/users"));
    assert_eq!(ArgSummary::Template(vec![lit("/a"), lit("/b")]).as_static_str().as_deref(), Some("/a/b"));
    assert_eq!(ArgSummary::Template(vec![]).as_static_str().as_deref(), Some(""));
    assert_eq!(
      ArgSummary::Concat(vec![ArgSummary::Str("/a".into()), ArgSummary::Str("/b".into())])
        .as_static_str()
        .as_deref(),
      Some("/a/b")
    );
    assert_eq!(ArgSummary::Template(vec![lit("/users/"), TemplatePart::Expr(path(&["id"]))]).as_static_str(), None);
    assert_eq!(ArgSummary::Number("1".into()).as_static_str(), None);
    assert_eq!(path(&["ROUTE"]).as_static_str(), None);
  }

  #[test]
  fn should_fold_static_expressions_inside_templates() {
    let template = ArgSummary::Template(vec![lit("/a"), TemplatePart::Expr(ArgSummary::Str("/b".into()))]);
    assert_eq!(template.as_static_str().as_deref(), Some("/a/b"));
  }

  #[test]
  fn should_compute_static_prefixes_for_templates_and_concatenation() {
    let template = ArgSummary::Template(vec![lit("/users/"), TemplatePart::Expr(path(&["id"])), lit("/posts")]);
    assert_eq!(template.static_prefix().as_deref(), Some("/users/"));
    let concat = ArgSummary::Concat(vec![ArgSummary::Str("/users/".into()), path(&["id"]), ArgSummary::Str("/x".into())]);
    assert_eq!(concat.static_prefix().as_deref(), Some("/users/"));
    let leading_expr = ArgSummary::Template(vec![TemplatePart::Expr(path(&["process", "env", "API_URL"])), lit("/users")]);
    assert_eq!(leading_expr.static_prefix().as_deref(), Some(""));
    assert_eq!(ArgSummary::Concat(vec![path(&["a"]), path(&["b"])]).static_prefix(), None);
    assert_eq!(ArgSummary::Bool(true).static_prefix(), None);
  }

  #[test]
  fn should_flatten_nested_concatenation_into_merged_parts() {
    let nested = ArgSummary::Concat(vec![
      ArgSummary::Concat(vec![ArgSummary::Str("/a".into()), ArgSummary::Str("/b".into())]),
      ArgSummary::Template(vec![lit("/c/"), TemplatePart::Expr(path(&["id"]))]),
      ArgSummary::Str("".into()),
    ]);
    assert_eq!(nested.string_parts().unwrap(), vec![lit("/a/b/c/"), TemplatePart::Expr(path(&["id"]))]);
    assert_eq!(nested.to_pattern("{}").as_deref(), Some("/a/b/c/{}"));
  }

  #[test]
  fn should_render_patterns_with_placeholders() {
    let url = ArgSummary::Template(vec![TemplatePart::Expr(path(&["BASE"])), lit("/users/"), TemplatePart::Expr(path(&["id"]))]);
    assert_eq!(url.to_pattern("{}").as_deref(), Some("{}/users/{}"));
  }

  #[test]
  fn should_truncate_long_text_on_char_boundaries_and_collapse_whitespace() {
    assert_eq!(truncate_text("a  \n b"), "a b");
    let long = "é".repeat(200);
    let truncated = truncate_text(&long);
    assert!(truncated.len() <= ARG_MAX_TEXT);
    assert!(truncated.ends_with('…'));
    assert_eq!(ArgSummary::other("x   y"), ArgSummary::Other("x y".into()));
  }

  #[test]
  fn should_serialise_arguments_as_tagged_values() {
    let arg = ArgSummary::Call {
      callee: vec!["AuthGuard".into()],
      args: vec![ArgSummary::Str("jwt".into())],
    };
    let json = serde_json::to_value(&arg).unwrap();
    assert_eq!(
      json,
      serde_json::json!({ "type": "call", "value": { "callee": ["AuthGuard"], "args": [{ "type": "str", "value": "jwt" }] } })
    );
    assert_eq!(serde_json::from_value::<ArgSummary>(json).unwrap(), arg);
    assert_eq!(serde_json::to_value(ArgSummary::Null).unwrap(), serde_json::json!({ "type": "null" }));
  }

  #[test]
  fn should_expose_static_object_key_names() {
    assert_eq!(ObjectKey::Ident("headers".into()).static_name(), Some("headers"));
    assert_eq!(ObjectKey::Str("x-api-key".into()).static_name(), Some("x-api-key"));
    assert_eq!(ObjectKey::Computed("k".into()).static_name(), None);
    assert_eq!(ObjectKey::Spread.static_name(), None);
  }
}
