use atlas_core::{ContentHash, EdgeKind, EntityKind, LanguageFamily, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
  Import,
  ImportModule,
  Call,
  Member,
  Type,
  Construct,
  Jsx,
  Value,
}

impl Site {
  pub fn of(edge: &EdgeKind, span_text: &str, target: &EntityKind) -> Self {
    match edge {
      EdgeKind::Imports | EdgeKind::ReExports if matches!(target, EntityKind::Module) => Self::ImportModule,
      EdgeKind::Imports | EdgeKind::ReExports => Self::Import,
      EdgeKind::Instantiates => Self::Construct,
      EdgeKind::Extends | EdgeKind::Implements | EdgeKind::TypeOf => Self::Type,
      _ if span_text.starts_with('<') => Self::Jsx,
      EdgeKind::Calls if has_member_access(span_text) => Self::Member,
      EdgeKind::Calls => Self::Call,
      _ => Self::Value,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnUnit {
  Utf8,
  Utf16,
  Utf32,
}

pub fn column_in(source: &str, byte_offset: usize, unit: ColumnUnit) -> Option<u32> {
  let line_start = source.get(..byte_offset)?.rfind('\n').map_or(0, |index| index + 1);
  let prefix = &source[line_start..byte_offset];
  let count = match unit {
    ColumnUnit::Utf8 => prefix.len(),
    ColumnUnit::Utf16 => prefix.chars().map(char::len_utf16).sum(),
    ColumnUnit::Utf32 => prefix.chars().count(),
  };
  u32::try_from(count).ok()
}

pub fn display_column(token: &Span) -> u32 {
  token.start_col.saturating_add(1)
}

pub struct VerifiedSource<'a> {
  text: &'a str,
  family: LanguageFamily,
}

impl<'a> VerifiedSource<'a> {
  pub fn verify(content: &'a [u8], indexed: &ContentHash, family: LanguageFamily) -> Option<Self> {
    if ContentHash::of(content) != *indexed {
      return None;
    }
    Some(Self {
      text: std::str::from_utf8(content).ok()?,
      family,
    })
  }

  pub fn text(&self) -> &'a str {
    self.text
  }

  pub fn name_token(&self, span: &Span, name: &str, site: Site) -> Option<Span> {
    name_token(self.text, self.family, span, name, site)
  }
}

/// Picks one occurrence of `name` among the identifier-bounded ones at bracket depth 0 of the span (outside literals and comments).
/// A span that is exactly the token wins. Otherwise `Member` prefers member-access occurrences, then the last one is the callee (`a.f(1).f(2)` is the second `f`);
/// `Jsx` takes the opening tag, `Type` and `Construct` require a single occurrence, and `Import` takes the single binding (the last path segment in Rust, else the single module prefix, as in `use a::*`); `ImportModule` prefers the single `::`-prefixed occurrence.
/// Anything ambiguous, stale or malformed is `None`.
pub fn name_token(source: &str, family: LanguageFamily, span: &Span, name: &str, site: Site) -> Option<Span> {
  if name.is_empty() || Span::of_text(source, span.start_byte as usize, span.end_byte as usize)? != *span {
    return None;
  }
  let from = span.start_byte as usize;
  let text = span.text(source)?;
  let (start, end) = match text.strip_prefix("r#").unwrap_or(text) == name {
    true => (0, text.len()),
    false => pick(site, family, text, name)?,
  };
  Span::of_text(source, from + start, from + end)
}

fn pick(site: Site, family: LanguageFamily, text: &str, name: &str) -> Option<(usize, usize)> {
  let (found, certain) = occurrences(text, family, name);
  if !certain {
    return None;
  }
  let range = |occurrence: &Occurrence| (occurrence.start, occurrence.end);
  match site {
    Site::Import | Site::ImportModule => {
      let prefixes = found.iter().filter(|occurrence| family == LanguageFamily::Rust && occurrence.path_prefix);
      let bindings = found.iter().filter(|occurrence| family != LanguageFamily::Rust || !occurrence.path_prefix);
      let (preferred, other): (Vec<_>, Vec<_>) = if site == Site::ImportModule {
        (prefixes.collect(), bindings.collect())
      } else {
        (bindings.collect(), prefixes.collect())
      };
      match (preferred.as_slice(), other.as_slice()) {
        ([only], _) | ([], [only]) => Some(range(only)),
        _ => None,
      }
    }
    _ => {
      let top: Vec<_> = found.iter().filter(|occurrence| occurrence.depth == 0).collect();
      match (site, top.as_slice()) {
        (_, [only]) => Some(range(only)),
        (Site::Jsx, [first, ..]) => Some(range(first)),
        (Site::Member, _) => top.iter().rfind(|occurrence| occurrence.member).or(top.last()).map(|occurrence| range(occurrence)),
        (Site::Call | Site::Value, [first, ..]) => Some(range(first)),
        _ => None,
      }
    }
  }
}

struct Occurrence {
  start: usize,
  end: usize,
  depth: i32,
  member: bool,
  path_prefix: bool,
}

fn occurrences(text: &str, family: LanguageFamily, name: &str) -> (Vec<Occurrence>, bool) {
  let mut found = Vec::new();
  let mut certain = true;
  let mut significant: Option<char> = None;
  let mut depth = 0i32;
  let mut index = 0;
  let mut previous: Option<char> = None;
  while let Some(current) = text.get(index..).and_then(|rest| rest.chars().next()) {
    if let Some(skip) = literal_len(&text[index..], previous, family) {
      index += skip.max(1);
      previous = None;
      significant = Some('"');
      continue;
    }
    match current {
      '(' | '[' | '{' => depth += 1,
      ')' | ']' | '}' => depth -= 1,
      '/' if family == LanguageFamily::Ts && may_start_regex(&text[index..], significant) => certain = false,
      _ => {}
    }
    let at = index;
    index += current.len_utf8();
    previous = Some(current);
    if !current.is_whitespace() {
      significant = Some(current);
    }
    if !text[at..].starts_with(name) || text[at + name.len()..].chars().next().is_some_and(is_ident_char) {
      continue;
    }
    let end = at + name.len();
    let (start, before) = match text[..at].strip_suffix("r#") {
      Some(rest) if !rest.chars().next_back().is_some_and(is_ident_char) => (at - 2, rest),
      _ => (at, &text[..at]),
    };
    if before.chars().next_back().is_some_and(is_ident_char) {
      continue;
    }
    found.push(Occurrence {
      start,
      end,
      depth,
      member: is_member_access(before),
      path_prefix: text[end..].trim_start().starts_with("::"),
    });
    index = end;
    previous = text[..end].chars().next_back();
    significant = previous;
  }
  (found, certain)
}

fn may_start_regex(from_slash: &str, significant: Option<char>) -> bool {
  let rest = &from_slash[1..];
  if rest.starts_with(['/', '*', '>']) {
    return false;
  }
  let after_operand = significant.is_some_and(|c| is_ident_char(c) || matches!(c, ')' | ']' | '"' | '\'' | '`' | '<' | '>'));
  let line = rest.split('\n').next().unwrap_or(rest);
  !after_operand && line.contains(['\'', '"', '`'])
}

fn is_ident_char(c: char) -> bool {
  c == '_' || c == '$' || c.is_alphanumeric()
}

fn is_member_access(before: &str) -> bool {
  let trimmed = before.trim_end();
  !trimmed.ends_with("..") && (trimmed.ends_with('.') || trimmed.ends_with("::"))
}

fn has_member_access(text: &str) -> bool {
  let mut depth = 0i32;
  let bytes = text.as_bytes();
  for (index, byte) in bytes.iter().enumerate() {
    match byte {
      b'(' | b'[' | b'{' => depth += 1,
      b')' | b']' | b'}' => depth -= 1,
      b'.' if depth == 0 && bytes.get(index + 1) != Some(&b'.') && index.checked_sub(1).and_then(|before| bytes.get(before)) != Some(&b'.') => return true,
      b':' if depth == 0 && bytes.get(index + 1) == Some(&b':') => return true,
      _ => {}
    }
  }
  false
}

fn literal_len(rest: &str, previous: Option<char>, family: LanguageFamily) -> Option<usize> {
  if let Some(comment) = rest.strip_prefix("//") {
    return Some(2 + comment.find('\n').unwrap_or(comment.len()));
  }
  if let Some(comment) = rest.strip_prefix("/*") {
    return Some(2 + block_comment_len(comment, family == LanguageFamily::Rust));
  }
  let first = rest.chars().next()?;
  if family == LanguageFamily::Rust
    && first == 'r'
    && !previous.is_some_and(is_ident_char)
    && let Some(len) = raw_string_len(rest)
  {
    return Some(len);
  }
  if family == LanguageFamily::Ts && first == '`' {
    return Some(template_len(rest));
  }
  if !matches!(first, '"' | '\'' | '`') {
    return None;
  }
  let body = &rest[1..];
  if first == '\'' && family == LanguageFamily::Rust && !is_char_literal(body) {
    return None;
  }
  let ends_at_newline = family == LanguageFamily::Ts && first != '`';
  let mut escaped = false;
  for (offset, c) in body.char_indices() {
    match c {
      _ if escaped => escaped = false,
      '\\' => escaped = true,
      '\n' if ends_at_newline => return Some(1 + offset),
      c if c == first => return Some(1 + offset + 1),
      _ => {}
    }
  }
  Some(rest.len())
}

fn template_len(rest: &str) -> usize {
  let body = &rest[1..];
  let mut index = 0;
  while index < body.len() {
    let tail = &body[index..];
    if let Some(escaped) = tail.strip_prefix('\\') {
      index += 1 + escaped.chars().next().map_or(0, char::len_utf8);
    } else if tail.starts_with('`') {
      return 1 + index + 1;
    } else if let Some(code) = tail.strip_prefix("${") {
      index += 2 + expression_len(code);
    } else {
      index += tail.chars().next().map_or(1, char::len_utf8);
    }
  }
  rest.len()
}

fn expression_len(code: &str) -> usize {
  let mut depth = 1;
  let mut index = 0;
  while index < code.len() {
    let tail = &code[index..];
    if let Some(skip) = literal_len(tail, None, LanguageFamily::Ts) {
      index += skip.max(1);
      continue;
    }
    let Some(c) = tail.chars().next() else { break };
    index += c.len_utf8();
    match c {
      '{' => depth += 1,
      '}' => {
        depth -= 1;
        if depth == 0 {
          return index;
        }
      }
      _ => {}
    }
  }
  code.len()
}

fn block_comment_len(body: &str, nested: bool) -> usize {
  let mut depth = 1;
  let mut index = 0;
  while index < body.len() {
    let rest = &body[index..];
    if rest.starts_with("*/") {
      depth -= 1;
      index += 2;
      if depth == 0 || !nested {
        return index;
      }
    } else if nested && rest.starts_with("/*") {
      depth += 1;
      index += 2;
    } else {
      index += rest.chars().next().map_or(1, char::len_utf8);
    }
  }
  body.len()
}

fn raw_string_len(rest: &str) -> Option<usize> {
  let hashes = rest[1..].chars().take_while(|c| *c == '#').count();
  rest[1 + hashes..].strip_prefix('"')?;
  let closing = format!("\"{}", "#".repeat(hashes));
  let body = &rest[2 + hashes..];
  Some(2 + hashes + body.find(&closing).map_or(body.len(), |end| end + closing.len()))
}

fn is_char_literal(body: &str) -> bool {
  let Some(first) = body.chars().next() else { return false };
  if first == '\\' || !is_ident_char(first) {
    return true;
  }
  let run = body.find(|c: char| !is_ident_char(c)).unwrap_or(body.len());
  body[run..].starts_with('\'')
}

#[cfg(test)]
mod tests {
  use super::*;

  const TS: LanguageFamily = LanguageFamily::Ts;
  const RS: LanguageFamily = LanguageFamily::Rust;

  fn span_of(source: &str, needle: &str) -> Span {
    let start = source.find(needle).unwrap();
    Span::of_text(source, start, start + needle.len()).unwrap()
  }

  fn token(family: LanguageFamily, source: &str, expression: &str, name: &str, site: Site) -> Option<(u32, u32, u32, u32)> {
    let found = name_token(source, family, &span_of(source, expression), name, site)?;
    Some((found.start_line, found.start_col, found.end_line, found.end_col))
  }

  fn text_of(family: LanguageFamily, source: &str, expression: &str, name: &str, site: Site) -> Option<String> {
    let found = name_token(source, family, &span_of(source, expression), name, site)?;
    found.text(source).map(str::to_owned)
  }

  #[test]
  fn should_locate_the_member_name_of_a_method_call() {
    let source = "    return this.usersService.findOne(id);\n";

    assert_eq!(token(TS, source, "this.usersService.findOne(id)", "findOne", Site::Member), Some((1, 29, 1, 36)));
  }

  #[test]
  fn should_locate_a_bare_call_and_ignore_the_same_name_in_arguments() {
    let source = "x = findOne(findOne(a), b);";

    assert_eq!(token(TS, source, "findOne(findOne(a), b)", "findOne", Site::Call), Some((1, 4, 1, 11)));
  }

  #[test]
  fn should_pick_the_last_member_occurrence_as_the_callee_of_a_chain() {
    assert_eq!(token(TS, "a.f(1).f(2)", "a.f(1).f(2)", "f", Site::Member), Some((1, 7, 1, 8)));
    assert_eq!(token(TS, "findOne.findOne(1)", "findOne.findOne(1)", "findOne", Site::Member), Some((1, 8, 1, 15)));
    assert_eq!(token(TS, "obj.f<f>()", "obj.f<f>()", "f", Site::Member), Some((1, 4, 1, 5)));
  }

  #[test]
  fn should_accept_a_span_that_is_exactly_the_token_even_for_a_qualified_path() {
    assert_eq!(token(RS, "self.bar()", "bar", "bar", Site::Member), Some((1, 5, 1, 8)));
    assert_eq!(token(RS, "let a = r#type;", "r#type", "type", Site::Value), Some((1, 8, 1, 14)));
  }

  #[test]
  fn should_include_the_raw_prefix_of_a_raw_identifier() {
    let source = "let a = self.r#type(1);";

    assert_eq!(token(RS, source, "self.r#type(1)", "type", Site::Member), Some((1, 13, 1, 19)));
  }

  #[test]
  fn should_require_a_single_occurrence_for_type_and_construct_sites() {
    assert_eq!(token(TS, "Foo<Foo>", "Foo<Foo>", "Foo", Site::Type), None);
    assert_eq!(token(TS, "new Foo(new Foo())", "new Foo(new Foo())", "Foo", Site::Construct), Some((1, 4, 1, 7)));
  }

  #[test]
  fn should_return_none_when_the_name_is_absent() {
    assert_eq!(token(TS, "this.service.run(id)", "this.service.run(id)", "findOne", Site::Member), None);
  }

  #[test]
  fn should_return_none_for_a_rust_qualifier_whose_span_is_the_last_segment() {
    assert_eq!(token(RS, "Type::new()", "new", "Type", Site::Call), None);
  }

  #[test]
  fn should_locate_a_token_on_a_later_line_of_a_multi_line_span() {
    let source = "const r = this.users\n  .repository\n  .findOne(id);\n";

    assert_eq!(token(TS, source, "this.users\n  .repository\n  .findOne(id)", "findOne", Site::Member), Some((3, 3, 3, 10)));
  }

  #[test]
  fn should_count_columns_from_the_last_newline_under_crlf() {
    let source = "a();\r\nthis.svc\r\n  .findOne(1);\r\n";

    assert_eq!(token(TS, source, "this.svc\r\n  .findOne(1)", "findOne", Site::Member), Some((3, 3, 3, 10)));
  }

  #[test]
  fn should_ignore_occurrences_in_strings_comments_and_nested_brackets() {
    let source = "x.log(\"findOne\") /* findOne */; // findOne\nfindOne(a[findOne])";

    assert_eq!(token(TS, source, "findOne(a[findOne])", "findOne", Site::Call), Some((2, 0, 2, 7)));
    assert_eq!(token(TS, source, "x.log(\"findOne\")", "findOne", Site::Member), None);
  }

  #[test]
  fn should_scan_typescript_single_quotes_as_strings() {
    assert_eq!(
      text_of(TS, "x.get('user name').findOne();", "x.get('user name').findOne()", "findOne", Site::Member).as_deref(),
      Some("findOne")
    );
    assert_eq!(
      token(TS, "x.get('user name').findOne();", "x.get('user name').findOne()", "findOne", Site::Member),
      Some((1, 19, 1, 26))
    );
    assert_eq!(
      token(
        TS,
        "q.where('user.id = :id', { id }).getOne();",
        "q.where('user.id = :id', { id }).getOne()",
        "getOne",
        Site::Member
      ),
      Some((1, 33, 1, 39))
    );
    assert_eq!(token(TS, "'x findOne' + findOne()", "'x findOne' + findOne()", "findOne", Site::Call), Some((1, 14, 1, 21)));
    assert_eq!(token(TS, "'http://x'.concat(f())", "'http://x'.concat(f())", "concat", Site::Member), Some((1, 11, 1, 17)));
  }

  #[test]
  fn should_not_treat_a_rust_lifetime_as_a_string() {
    let source = "fn f<'a>(x: &'a str) { svc.find(x) }";

    assert_eq!(token(RS, source, "svc.find(x)", "find", Site::Member), Some((1, 27, 1, 31)));
  }

  #[test]
  fn should_skip_rust_char_raw_string_and_nested_comment_literals() {
    assert_eq!(token(RS, "f('{').bar()", "f('{').bar()", "bar", Site::Member), Some((1, 7, 1, 10)));
    assert_eq!(
      token(RS, "r#\"a \"q\" b\"#.find(x)", "r#\"a \"q\" b\"#.find(x)", "find", Site::Member),
      Some((1, 13, 1, 17))
    );
    assert_eq!(
      token(RS, "/* a /* b */ bar */ x.bar()", "/* a /* b */ bar */ x.bar()", "bar", Site::Member),
      Some((1, 22, 1, 25))
    );
  }

  #[test]
  fn should_pick_the_opening_tag_of_a_jsx_element() {
    assert_eq!(token(TS, "<Card id={1}>text</Card>", "<Card id={1}>text</Card>", "Card", Site::Jsx), Some((1, 1, 1, 5)));
  }

  #[test]
  fn should_find_import_bindings_in_typescript() {
    assert_eq!(
      text_of(TS, "import { a as b } from './m';", "import { a as b } from './m';", "b", Site::Import).as_deref(),
      Some("b")
    );
    assert_eq!(
      token(TS, "import { a as b } from './m';", "import { a as b } from './m';", "b", Site::Import),
      Some((1, 14, 1, 15))
    );
    assert_eq!(
      token(TS, "import { a as b } from './m';", "import { a as b } from './m';", "a", Site::Import),
      Some((1, 9, 1, 10))
    );
    assert_eq!(token(TS, "export { x } from './x';", "export { x } from './x';", "x", Site::Import), Some((1, 9, 1, 10)));
    assert_eq!(
      token(TS, "import Default from './d';", "import Default from './d';", "Default", Site::Import),
      Some((1, 7, 1, 14))
    );
    assert_eq!(
      token(TS, "import * as ns from './n';", "import * as ns from './n';", "ns", Site::Import),
      Some((1, 12, 1, 14))
    );
    assert_eq!(
      token(TS, "import { toCents } from '@acme/sdk';", "import { toCents } from '@acme/sdk';", "toCents", Site::Import),
      Some((1, 9, 1, 16))
    );
  }

  #[test]
  fn should_not_find_an_import_binding_inside_the_module_string() {
    assert_eq!(token(TS, "import { a } from './b';", "import { a } from './b';", "b", Site::Import), None);
  }

  #[test]
  fn should_find_import_bindings_in_rust() {
    let grouped = "use a::{b, c as d};";

    assert_eq!(token(RS, grouped, grouped, "c", Site::Import), Some((1, 11, 1, 12)));
    assert_eq!(token(RS, grouped, grouped, "d", Site::Import), Some((1, 16, 1, 17)));
    assert_eq!(token(RS, grouped, grouped, "b", Site::Import), Some((1, 8, 1, 9)));
    assert_eq!(token(RS, "use crate::lex::lex;", "use crate::lex::lex;", "lex", Site::Import), Some((1, 16, 1, 19)));
    assert_eq!(token(RS, "use a::*;", "use a::*;", "b", Site::Import), None);
    assert_eq!(token(RS, "use crate::a::*;", "use crate::a::*;", "a", Site::Import), Some((1, 11, 1, 12)));
    assert_eq!(
      token(RS, "use crate::source::{self, f};", "use crate::source::{self, f};", "source", Site::Import),
      Some((1, 11, 1, 17))
    );
    assert_eq!(token(RS, "use crate::a::b::c;", "use crate::a::b::c;", "a", Site::Import), Some((1, 11, 1, 12)));
    assert_eq!(token(RS, "use super::*;", "use super::*;", "compare", Site::Import), None);
  }

  #[test]
  fn should_derive_the_site_from_the_edge_kind_and_span_text() {
    assert_eq!(Site::of(&EdgeKind::Imports, "use a::b;", &EntityKind::Function), Site::Import);
    assert_eq!(Site::of(&EdgeKind::Imports, "use a::b;", &EntityKind::Module), Site::ImportModule);
    assert_eq!(Site::of(&EdgeKind::ReExports, "export { a } from 'b'", &EntityKind::Function), Site::Import);
    assert_eq!(Site::of(&EdgeKind::Instantiates, "new Foo()", &EntityKind::Function), Site::Construct);
    assert_eq!(Site::of(&EdgeKind::Extends, "Base", &EntityKind::Function), Site::Type);
    assert_eq!(Site::of(&EdgeKind::References, "<Card />", &EntityKind::Function), Site::Jsx);
    assert_eq!(Site::of(&EdgeKind::Calls, "this.svc.find(id)", &EntityKind::Function), Site::Member);
    assert_eq!(Site::of(&EdgeKind::Calls, "find(a.b)", &EntityKind::Function), Site::Call);
    assert_eq!(Site::of(&EdgeKind::Calls, "Type::new()", &EntityKind::Function), Site::Member);
    assert_eq!(Site::of(&EdgeKind::References, "...rest", &EntityKind::Function), Site::Value);
  }

  #[test]
  fn should_not_panic_when_the_span_splits_a_character() {
    let source = "é";
    let span = Span::of_text(source, 1, 2).unwrap();

    assert_eq!(name_token(source, TS, &span, "x", Site::Call), None);
  }

  #[test]
  fn should_reject_a_span_that_disagrees_with_the_source() {
    let source = "this.svc.findOne(1)";
    let mut span = span_of(source, source);
    span.start_col = 3;

    assert_eq!(name_token(source, TS, &span, "findOne", Site::Member), None);
  }

  #[test]
  fn should_reject_content_whose_hash_differs_from_the_indexed_hash() {
    let indexed = "this.svc.findOne(1)";
    let span = span_of(indexed, indexed);
    let hash = ContentHash::of(indexed.as_bytes());

    let verified = VerifiedSource::verify(indexed.as_bytes(), &hash, TS).unwrap();

    assert_eq!(verified.text(), indexed);
    assert_eq!(
      verified.name_token(&span, "findOne", Site::Member).map(|found| (found.start_col, found.end_col)),
      Some((9, 16))
    );
    assert!(VerifiedSource::verify(b"this.svc.findOne(1) ", &hash, TS).is_none());
  }

  #[test]
  fn should_reject_content_that_is_not_utf8() {
    let content = [0xffu8, 0xfe];

    assert!(VerifiedSource::verify(&content, &ContentHash::of(&content), TS).is_none());
  }

  #[test]
  fn should_report_byte_utf16_and_char_columns_after_multi_byte_characters() {
    let source = "é😀 = svc.find(1);";
    let found = name_token(source, TS, &span_of(source, "svc.find(1)"), "find", Site::Member).unwrap();
    let offset = found.start_byte as usize;

    assert_eq!(found.start_col, 13);
    assert_eq!(display_column(&found), 14);
    assert_eq!(column_in(source, offset, ColumnUnit::Utf8), Some(13));
    assert_eq!(column_in(source, offset, ColumnUnit::Utf16), Some(10));
    assert_eq!(column_in(source, offset, ColumnUnit::Utf32), Some(9));
  }

  #[test]
  fn should_count_columns_from_the_last_line_feed_only_like_stored_spans() {
    assert_eq!(column_in("ab\rcd", 4, ColumnUnit::Utf8), Some(4));
    assert_eq!(column_in("ab\r\ncd", 5, ColumnUnit::Utf8), Some(1));
  }

  #[test]
  fn should_reject_an_offset_inside_a_character() {
    assert_eq!(column_in("é", 1, ColumnUnit::Utf8), None);
  }

  #[test]
  fn should_take_the_first_occurrence_for_calls_and_values_with_type_arguments() {
    assert_eq!(token(TS, "f(f)", "f(f)", "f", Site::Call), Some((1, 0, 1, 1)));
    assert_eq!(token(TS, "f<typeof f>(x)", "f<typeof f>(x)", "f", Site::Call), Some((1, 0, 1, 1)));
    assert_eq!(token(TS, "f<f>()", "f<f>()", "f", Site::Call), Some((1, 0, 1, 1)));
    assert_eq!(token(TS, "f!", "f!", "f", Site::Value), Some((1, 0, 1, 1)));
  }

  #[test]
  fn should_prefer_the_module_prefix_for_module_imports_and_the_item_otherwise() {
    let plain = "use crate::lex::{self, lex};";
    let aliased = "use crate::lex::{lex as tokenize, self as lexmod};";
    let nested = "use crate::outer::{inner::{self, inner as f}};";

    assert_eq!(token(RS, plain, plain, "lex", Site::ImportModule), Some((1, 11, 1, 14)));
    assert_eq!(token(RS, plain, plain, "lex", Site::Import), Some((1, 23, 1, 26)));
    assert_eq!(token(RS, aliased, aliased, "lex", Site::ImportModule), Some((1, 11, 1, 14)));
    assert_eq!(token(RS, aliased, aliased, "lex", Site::Import), Some((1, 17, 1, 20)));
    assert_eq!(token(RS, nested, nested, "inner", Site::ImportModule), Some((1, 19, 1, 24)));
    assert_eq!(token(RS, nested, nested, "inner", Site::Import), Some((1, 33, 1, 38)));
    assert_eq!(token(RS, "use crate::lex;", "use crate::lex;", "lex", Site::ImportModule), Some((1, 11, 1, 14)));
  }

  #[test]
  fn should_return_none_when_a_regex_literal_may_hide_a_quote() {
    let source = "/'/.test(s) ? 'findOne' : x";

    assert_eq!(token(TS, source, source, "findOne", Site::Value), None);
    assert_eq!(token(TS, "a / b + 'x' + f()", "a / b + 'x' + f()", "f", Site::Call), Some((1, 14, 1, 15)));
    assert_eq!(token(TS, "<Foo a={1} />", "<Foo a={1} />", "Foo", Site::Jsx), Some((1, 1, 1, 4)));
  }

  #[test]
  fn should_keep_nested_template_literals_as_strings() {
    let source = "`${`x f`}` + f()";

    assert_eq!(token(TS, source, source, "f", Site::Call), Some((1, 13, 1, 14)));
    assert_eq!(
      token(TS, "`a ${b + `c ${f}`} d` + g(f)", "`a ${b + `c ${f}`} d` + g(f)", "g", Site::Call),
      Some((1, 24, 1, 25))
    );
  }
}
