use std::collections::BTreeSet;

use atlas_core::{ArgSummary, ObjectEntry, ObjectKey};

use crate::lex::{Token, TokenKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HeaderValue {
  Literal(String),
  Dynamic,
}

impl HeaderValue {
  pub fn of_arg(arg: &ArgSummary) -> Self {
    match arg {
      ArgSummary::Str(value) => Self::Literal(value.clone()),
      ArgSummary::Template(_) | ArgSummary::Concat(_) => match arg.static_prefix() {
        Some(prefix) if !prefix.is_empty() => Self::Literal(prefix),
        _ => Self::Dynamic,
      },
      ArgSummary::Call { callee, args } if callee.last().is_some_and(|name| name == "format!") => match args.first() {
        Some(ArgSummary::Str(template)) => Self::Literal(template.split('{').next().unwrap_or_default().to_owned()),
        _ => Self::Dynamic,
      },
      _ => Self::Dynamic,
    }
  }
}

pub(crate) fn is_api_key_header(name: &str) -> bool {
  let lower = name.to_ascii_lowercase();
  if lower.contains("api-key") || lower.contains("api_key") || lower.contains("apikey") {
    return true;
  }
  lower.starts_with("x-") && (lower.ends_with("-key") || lower.ends_with("-token")) && !lower.contains("csrf") && !lower.contains("xsrf")
}

pub(crate) fn classify_header(name: &str, value: &HeaderValue) -> Option<String> {
  let lower = name.to_ascii_lowercase();
  if lower == "authorization" {
    let hint = match value {
      HeaderValue::Literal(text) if text.to_ascii_lowercase().starts_with("bearer") => "bearer",
      HeaderValue::Literal(text) if text.to_ascii_lowercase().starts_with("basic") => "basic",
      _ => "unknown",
    };
    return Some(hint.to_owned());
  }
  if is_api_key_header(&lower) {
    return Some(format!("api_key:{lower}"));
  }
  if lower == "cookie" {
    let cookie = match value {
      HeaderValue::Literal(text) => text.split('=').next().map(str::trim).filter(|name| !name.is_empty()).unwrap_or("*").to_owned(),
      HeaderValue::Dynamic => "*".to_owned(),
    };
    return Some(format!("cookie:{cookie}"));
  }
  None
}

pub(crate) fn hints_from_object(entries: &[ObjectEntry]) -> (BTreeSet<String>, bool) {
  let mut hints = BTreeSet::new();
  let mut dynamic = false;
  for entry in entries {
    match entry.key.static_name() {
      Some(name) => hints.extend(classify_header(name, &HeaderValue::of_arg(&entry.value))),
      None if matches!(entry.key, ObjectKey::Spread | ObjectKey::Computed(_)) => dynamic = true,
      None => {}
    }
  }
  (hints, dynamic)
}

pub(crate) fn hints_from_tokens(tokens: &[Token]) -> BTreeSet<String> {
  let mut hints = BTreeSet::new();
  for (index, token) in tokens.iter().enumerate() {
    let Some((name, value_index)) = header_assignment(tokens, index, token) else { continue };
    let value = match tokens.get(value_index) {
      Some(value) if value.kind == TokenKind::Str => HeaderValue::Literal(value.text.clone()),
      Some(open) if open.is_punct("`") => match tokens.get(value_index + 1) {
        Some(chunk) if chunk.kind == TokenKind::TemplateChunk => HeaderValue::Literal(chunk.text.clone()),
        _ => HeaderValue::Dynamic,
      },
      _ => HeaderValue::Dynamic,
    };
    hints.extend(classify_header(name, &value));
  }
  hints
}

fn header_assignment<'t>(tokens: &'t [Token], index: usize, token: &'t Token) -> Option<(&'t str, usize)> {
  let previous = index.checked_sub(1).and_then(|previous| tokens.get(previous));
  let next = tokens.get(index + 1);
  if token.kind == TokenKind::Str {
    if previous.is_some_and(|previous| previous.is_punct("[")) && next.is_some_and(|next| next.is_punct("]")) && tokens.get(index + 2).is_some_and(|eq| eq.is_punct("=")) {
      return Some((token.text.as_str(), index + 3));
    }
    if next.is_some_and(|next| next.is_punct(":")) && previous.is_some_and(|previous| previous.is_punct("{") || previous.is_punct(",")) {
      return Some((token.text.as_str(), index + 2));
    }
    let call = index.checked_sub(2).and_then(|call| tokens.get(call));
    if previous.is_some_and(|previous| previous.is_punct("("))
      && call.is_some_and(|call| call.kind == TokenKind::Ident && matches!(call.text.as_str(), "set" | "append" | "header" | "setHeader"))
      && next.is_some_and(|next| next.is_punct(","))
    {
      return Some((token.text.as_str(), index + 2));
    }
    return None;
  }
  if token.kind == TokenKind::Ident
    && next.is_some_and(|next| next.is_punct(":"))
    && previous.is_some_and(|previous| previous.is_punct("{") || previous.is_punct(","))
    && !tokens.get(index + 2).is_some_and(|after| after.is_punct(":"))
  {
    return Some((token.text.as_str(), index + 2));
  }
  None
}

#[cfg(test)]
mod tests {
  use atlas_core::LanguageFamily;

  use super::*;
  use crate::lex::lex;

  #[test]
  fn should_classify_auth_headers() {
    assert_eq!(classify_header("Authorization", &HeaderValue::Literal("Bearer ".into())).as_deref(), Some("bearer"));
    assert_eq!(classify_header("authorization", &HeaderValue::Literal("Basic abc".into())).as_deref(), Some("basic"));
    assert_eq!(classify_header("Authorization", &HeaderValue::Dynamic).as_deref(), Some("unknown"));
    assert_eq!(classify_header("X-Api-Key", &HeaderValue::Dynamic).as_deref(), Some("api_key:x-api-key"));
    assert_eq!(classify_header("x-globex-key", &HeaderValue::Dynamic).as_deref(), Some("api_key:x-globex-key"));
    assert_eq!(classify_header("Cookie", &HeaderValue::Literal("sid=1".into())).as_deref(), Some("cookie:sid"));
    assert_eq!(classify_header("Content-Type", &HeaderValue::Literal("application/json".into())), None);
    assert_eq!(classify_header("x-csrf-token", &HeaderValue::Dynamic), None);
  }

  #[test]
  fn should_read_header_assignments_from_helper_bodies() {
    let tokens = lex(
      "const headers = { 'Content-Type': 'application/json' }; if (a) headers['Authorization'] = `Bearer ${t}`; else headers['x-api-key'] = k;",
      LanguageFamily::Ts,
    );
    let hints: Vec<String> = hints_from_tokens(&tokens).into_iter().collect();
    assert_eq!(hints, ["api_key:x-api-key", "bearer"]);
  }
}
