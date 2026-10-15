use atlas_core::{ArgSummary, EntityKind, LocalSymbolId, ObjectEntry, ObjectKey};

use crate::lex::{Token, TokenKind};
use crate::view::{FileView, initializer_start};

const TYPE_SUFFIXES: [&str; 2] = ["as", "satisfies"];
const OPENERS: [&str; 4] = ["(", "[", "{", "${"];
const CLOSERS: [&str; 3] = [")", "]", "}"];
const MAX_DEPTH: usize = 32;

pub(crate) fn const_object(view: &FileView<'_>, scope: Option<LocalSymbolId>, name: &str) -> Option<Vec<ObjectEntry>> {
  if view.enclosing_scope_contains_binding(scope, name) {
    return None;
  }
  let symbol = view.top_level_named(name).filter(|symbol| symbol.kind == EntityKind::Constant)?;
  let signature = symbol.signature.as_deref().unwrap_or_default();
  if !signature.starts_with("const ") && !signature.starts_with("export const ") {
    return None;
  }
  let tokens = view.span_tokens(symbol.span);
  tokens.first().filter(|token| token.is_ident(name))?;
  let start = initializer_start(tokens, 1)?;
  let (entries, consumed) = object_literal(tokens.get(start..)?)?;
  match tokens.get(start + consumed..)? {
    [] => Some(entries),
    [semicolon] if semicolon.is_punct(";") => Some(entries),
    [suffix, ..] if TYPE_SUFFIXES.iter().any(|word| suffix.is_ident(word)) => Some(entries),
    _ => None,
  }
}

fn object_literal(tokens: &[Token]) -> Option<(Vec<ObjectEntry>, usize)> {
  let literal = Literal {
    tokens,
    closers: closing_brackets(tokens),
  };
  literal.object(0, 0)
}

fn closing_brackets(tokens: &[Token]) -> Vec<Option<usize>> {
  let mut closers = vec![None; tokens.len()];
  let mut open = Vec::new();
  for (index, token) in tokens.iter().enumerate() {
    if token.kind != TokenKind::Punct {
      continue;
    }
    if OPENERS.contains(&token.text.as_str()) {
      open.push(index);
    } else if CLOSERS.contains(&token.text.as_str())
      && let Some(opener) = open.pop()
    {
      closers[opener] = Some(index);
    }
  }
  closers
}

struct Literal<'t> {
  tokens: &'t [Token],
  closers: Vec<Option<usize>>,
}

impl Literal<'_> {
  fn object(&self, at: usize, depth: usize) -> Option<(Vec<ObjectEntry>, usize)> {
    if !self.tokens.get(at)?.is_punct("{") {
      return None;
    }
    let close = self.closers[at]?;
    let mut entries = Vec::new();
    let mut cursor = at + 1;
    while cursor < close {
      let end = self.entry_end(cursor, close)?;
      entries.push(self.entry(cursor, end, depth)?);
      cursor = end + usize::from(end < close);
    }
    Some((entries, close + 1))
  }

  fn entry_end(&self, from: usize, limit: usize) -> Option<usize> {
    let mut index = from;
    while index < limit {
      let token = &self.tokens[index];
      if token.kind == TokenKind::Punct && OPENERS.contains(&token.text.as_str()) {
        index = self.closers[index]? + 1;
        continue;
      }
      if token.is_punct(",") {
        return Some(index);
      }
      index += 1;
    }
    (index == limit).then_some(limit)
  }

  fn entry(&self, start: usize, end: usize, depth: usize) -> Option<ObjectEntry> {
    let tokens = &self.tokens[start..end];
    match tokens {
      [spread, ..] if spread.is_punct("...") => Some(ObjectEntry {
        key: ObjectKey::Spread,
        value: self.value(start + 1, end, depth),
      }),
      [name] if name.kind == TokenKind::Ident => Some(ObjectEntry {
        key: ObjectKey::Ident(name.text.clone()),
        value: ArgSummary::Path(vec![name.text.clone()]),
      }),
      [key, colon, _, ..] if colon.is_punct(":") => {
        let key = match key.kind {
          TokenKind::Ident => ObjectKey::Ident(key.text.clone()),
          TokenKind::Str | TokenKind::Number => ObjectKey::Str(key.text.clone()),
          TokenKind::TemplateChunk | TokenKind::Punct => return None,
        };
        Some(ObjectEntry {
          key,
          value: self.value(start + 2, end, depth),
        })
      }
      [open, ..] if open.is_punct("[") => {
        let close = self.closers[start].filter(|close| *close + 1 < end)?;
        self.tokens[close + 1].is_punct(":").then_some(())?;
        Some(ObjectEntry {
          key: ObjectKey::Computed(text(&self.tokens[start + 1..close], "")),
          value: self.value(close + 2, end, depth),
        })
      }
      _ => None,
    }
  }

  fn value(&self, start: usize, end: usize, depth: usize) -> ArgSummary {
    let end = self.type_suffix(start, end).unwrap_or(end);
    let tokens = &self.tokens[start..end];
    match tokens {
      [literal] if literal.kind == TokenKind::Str => return ArgSummary::Str(literal.text.clone()),
      [open, chunk, close] if open.is_punct("`") && chunk.kind == TokenKind::TemplateChunk && close.is_punct("`") => return ArgSummary::Str(chunk.text.clone()),
      [open, close] if open.is_punct("`") && close.is_punct("`") => return ArgSummary::Str(String::new()),
      [open, ..] if open.is_punct("{") && depth < MAX_DEPTH && self.closers[start] == Some(end - 1) => {
        if let Some((entries, _)) = self.object(start, depth + 1) {
          return ArgSummary::Object(entries);
        }
      }
      _ => {}
    }
    match member_path(tokens) {
      Some(path) => ArgSummary::Path(path),
      None => ArgSummary::Other(text(tokens, " ")),
    }
  }

  fn type_suffix(&self, start: usize, end: usize) -> Option<usize> {
    let mut index = start;
    while index < end {
      let token = &self.tokens[index];
      if token.kind == TokenKind::Punct && OPENERS.contains(&token.text.as_str()) {
        index = self.closers[index].map_or(end, |close| close + 1);
        continue;
      }
      if TYPE_SUFFIXES.iter().any(|word| token.is_ident(word)) {
        return Some(index);
      }
      index += 1;
    }
    None
  }
}

fn text(tokens: &[Token], separator: &str) -> String {
  tokens.iter().map(|token| token.text.as_str()).collect::<Vec<_>>().join(separator)
}

fn member_path(tokens: &[Token]) -> Option<Vec<String>> {
  let mut path = Vec::new();
  for (index, token) in tokens.iter().enumerate() {
    match index % 2 {
      0 if token.kind == TokenKind::Ident => path.push(token.text.clone()),
      1 if token.is_punct(".") => {}
      _ => return None,
    }
  }
  (tokens.len() % 2 == 1).then_some(path)
}

#[cfg(test)]
mod tests {
  use atlas_core::LanguageFamily;

  use super::*;
  use crate::lex::lex;

  fn parse(source: &str) -> Option<Vec<ObjectEntry>> {
    let tokens = lex(source, LanguageFamily::Ts);
    let (entries, consumed) = object_literal(&tokens)?;
    (consumed == tokens.len()).then_some(entries)
  }

  fn ident(name: &str, value: ArgSummary) -> ObjectEntry {
    ObjectEntry {
      key: ObjectKey::Ident(name.to_owned()),
      value,
    }
  }

  fn on_small_stack<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(work).unwrap().join().unwrap()
  }

  fn nesting(entries: &[ObjectEntry]) -> usize {
    match entries.first().map(|entry| &entry.value) {
      Some(ArgSummary::Object(inner)) => 1 + nesting(inner),
      _ => 0,
    }
  }

  #[test]
  fn should_read_literal_member_shorthand_and_nested_values() {
    assert_eq!(
      parse("{ method: 'POST', path: `/import`, kind: Method.PUT, ready, 'v-1': { base: '/v1' }, }"),
      Some(vec![
        ident("method", ArgSummary::Str("POST".to_owned())),
        ident("path", ArgSummary::Str("/import".to_owned())),
        ident("kind", ArgSummary::Path(vec!["Method".to_owned(), "PUT".to_owned()])),
        ident("ready", ArgSummary::Path(vec!["ready".to_owned()])),
        ObjectEntry {
          key: ObjectKey::Str("v-1".to_owned()),
          value: ArgSummary::Object(vec![ident("base", ArgSummary::Str("/v1".to_owned()))]),
        },
      ])
    );
  }

  #[test]
  fn should_keep_unreadable_values_opaque_and_reject_unreadable_entries() {
    assert_eq!(
      parse("{ path: `/x/${id}`, method: pick('GET'), ...defaults, [KEY]: 'k', name: 'n' as const }"),
      Some(vec![
        ident("path", ArgSummary::Other("` /x/ ${ id } `".to_owned())),
        ident("method", ArgSummary::Other("pick ( GET )".to_owned())),
        ObjectEntry {
          key: ObjectKey::Spread,
          value: ArgSummary::Path(vec!["defaults".to_owned()]),
        },
        ObjectEntry {
          key: ObjectKey::Computed("KEY".to_owned()),
          value: ArgSummary::Str("k".to_owned()),
        },
        ident("name", ArgSummary::Str("n".to_owned())),
      ])
    );
    assert_eq!(parse("{ run() { return 1; } }"), None);
    assert_eq!(parse("{ path: '/x'"), None);
    assert_eq!(parse("{ path: ('/x' }"), None);
  }

  #[test]
  fn should_cap_nesting_and_keep_deeper_values_opaque_on_a_two_mebibyte_stack() {
    let depth = 50_000;
    let source = format!("{}'leaf'{}", "{ a: ".repeat(depth), " }".repeat(depth));
    let entries = on_small_stack(move || parse(&source)).unwrap();
    assert_eq!(nesting(&entries), MAX_DEPTH);
  }

  #[test]
  fn should_read_a_long_flat_object_on_a_two_mebibyte_stack() {
    let count = 50_000;
    let body: Vec<String> = (0..count).map(|index| format!("k{index}: {{ v: '{index}' }}")).collect();
    let source = format!("{{ {} }}", body.join(", "));
    let entries = on_small_stack(move || parse(&source)).unwrap();
    assert_eq!(entries.len(), count);
    assert_eq!(
      entries[count - 1],
      ident("k49999", ArgSummary::Object(vec![ident("v", ArgSummary::Str("49999".to_owned()))]))
    );
  }
}
