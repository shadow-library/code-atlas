use atlas_core::LanguageFamily;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenKind {
  Ident,
  Str,
  TemplateChunk,
  Number,
  Punct,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
  pub kind: TokenKind,
  pub text: String,
  pub start: u32,
  pub end: u32,
}

impl Token {
  pub fn is_punct(&self, punct: &str) -> bool {
    self.kind == TokenKind::Punct && self.text == punct
  }

  pub fn is_ident(&self, ident: &str) -> bool {
    self.kind == TokenKind::Ident && self.text == ident
  }
}

const MULTI_PUNCT: [&str; 16] = ["...", "===", "!==", "::", "?.", "=>", "==", "!=", "&&", "||", "??", "<=", ">=", "->", "+=", "-="];
const REGEX_PRECEDING_KEYWORDS: [&str; 12] = ["return", "typeof", "case", "in", "of", "new", "delete", "void", "throw", "yield", "await", "else"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
  Code { braces: u32 },
  Template,
}

pub(crate) fn lex(source: &str, family: LanguageFamily) -> Vec<Token> {
  Lexer {
    source,
    bytes: source.as_bytes(),
    position: 0,
    family,
    tokens: Vec::new(),
    stack: vec![Context::Code { braces: 0 }],
  }
  .run()
}

struct Lexer<'s> {
  source: &'s str,
  bytes: &'s [u8],
  position: usize,
  family: LanguageFamily,
  tokens: Vec<Token>,
  stack: Vec<Context>,
}

impl Lexer<'_> {
  fn run(mut self) -> Vec<Token> {
    while self.position < self.bytes.len() {
      let before = self.position;
      match self.stack.last().copied() {
        Some(Context::Template) => self.template_chunk(),
        _ => self.code(),
      }
      if self.position == before {
        self.position += 1;
      }
    }
    self.tokens
  }

  fn peek(&self, offset: usize) -> u8 {
    self.bytes.get(self.position + offset).copied().unwrap_or(0)
  }

  fn push(&mut self, kind: TokenKind, text: String, start: usize, end: usize) {
    self.tokens.push(Token {
      kind,
      text,
      start: to_u32(start),
      end: to_u32(end),
    });
  }

  fn slice(&self, start: usize, end: usize) -> &str {
    self.source.get(start..end).unwrap_or_default()
  }

  fn code(&mut self) {
    let byte = self.peek(0);
    match byte {
      b' ' | b'\t' | b'\n' | b'\r' => self.position += 1,
      b'/' if self.peek(1) == b'/' => self.skip_line(),
      b'/' if self.peek(1) == b'*' => self.skip_block_comment(),
      b'/' if self.family == LanguageFamily::Ts && self.regex_allowed() => self.skip_regex(),
      b'"' => self.string(b'"'),
      b'\'' if self.family == LanguageFamily::Ts => self.string(b'\''),
      b'\'' => self.rust_quote(),
      b'`' if self.family == LanguageFamily::Ts => {
        self.push(TokenKind::Punct, "`".to_owned(), self.position, self.position + 1);
        self.position += 1;
        self.stack.push(Context::Template);
      }
      b'0'..=b'9' => self.number(),
      byte if is_ident_start(byte) => self.ident_or_prefixed_string(),
      _ => self.punct(),
    }
  }

  fn regex_allowed(&self) -> bool {
    let Some(previous) = self.tokens.last() else { return true };
    match previous.kind {
      TokenKind::Punct => !matches!(previous.text.as_str(), ")" | "]" | "}" | "`"),
      TokenKind::Ident => REGEX_PRECEDING_KEYWORDS.contains(&previous.text.as_str()),
      TokenKind::Str | TokenKind::TemplateChunk | TokenKind::Number => false,
    }
  }

  fn skip_line(&mut self) {
    while self.position < self.bytes.len() && self.peek(0) != b'\n' {
      self.position += 1;
    }
  }

  fn skip_block_comment(&mut self) {
    self.position += 2;
    let mut depth = 1u32;
    while self.position < self.bytes.len() && depth > 0 {
      if self.peek(0) == b'*' && self.peek(1) == b'/' {
        depth -= 1;
        self.position += 2;
      } else if self.family == LanguageFamily::Rust && self.peek(0) == b'/' && self.peek(1) == b'*' {
        depth += 1;
        self.position += 2;
      } else {
        self.position += 1;
      }
    }
  }

  fn skip_regex(&mut self) {
    self.position += 1;
    let mut in_class = false;
    while self.position < self.bytes.len() {
      match self.peek(0) {
        b'\n' => return,
        b'\\' => self.position += 2,
        b'[' => {
          in_class = true;
          self.position += 1;
        }
        b']' => {
          in_class = false;
          self.position += 1;
        }
        b'/' if !in_class => {
          self.position += 1;
          while is_ident_continue(self.peek(0)) {
            self.position += 1;
          }
          return;
        }
        _ => self.position += 1,
      }
    }
  }

  fn string(&mut self, quote: u8) {
    let start = self.position;
    self.position += 1;
    let mut value = String::new();
    let mut chunk_start = self.position;
    while self.position < self.bytes.len() {
      let byte = self.peek(0);
      if byte == quote {
        value.push_str(self.slice(chunk_start, self.position));
        self.position += 1;
        self.push(TokenKind::Str, value, start, self.position);
        return;
      }
      if byte == b'\n' && self.family == LanguageFamily::Ts {
        break;
      }
      if byte == b'\\' {
        value.push_str(self.slice(chunk_start, self.position));
        self.push_escape(&mut value);
        chunk_start = self.position;
        continue;
      }
      self.position += 1;
    }
    value.push_str(self.slice(chunk_start, self.position.min(self.bytes.len())));
    self.push(TokenKind::Str, value, start, self.position.min(self.bytes.len()));
  }

  fn push_escape(&mut self, value: &mut String) {
    let escaped = self.peek(1);
    self.position += 2;
    match escaped {
      b'n' => value.push('\n'),
      b't' => value.push('\t'),
      b'r' => value.push('\r'),
      b'0' => value.push('\0'),
      b'\n' => {}
      _ => {
        let start = self.position.saturating_sub(1);
        let mut end = self.position;
        while end < self.bytes.len() && !self.source.is_char_boundary(end) {
          end += 1;
        }
        self.position = end;
        value.push_str(self.slice(start, end));
      }
    }
  }

  fn rust_quote(&mut self) {
    let start = self.position;
    let next_char_width = self.source.get(start + 1..).and_then(|rest| rest.chars().next()).map_or(0, char::len_utf8);
    let is_char = self.peek(1) == b'\\' || (next_char_width > 0 && self.peek(1 + next_char_width) == b'\'');
    if !is_char {
      self.position += 1;
      self.push(TokenKind::Punct, "'".to_owned(), start, self.position);
      return;
    }
    self.position += 1;
    while self.position < self.bytes.len() {
      match self.peek(0) {
        b'\\' => self.position += 2,
        b'\'' => {
          self.position += 1;
          break;
        }
        b'\n' => break,
        _ => self.position += 1,
      }
    }
    let end = self.position.min(self.bytes.len());
    self.push(TokenKind::Str, self.slice(start, end).trim_matches('\'').to_owned(), start, end);
  }

  fn raw_string(&mut self, start: usize) {
    let mut hashes = 0usize;
    while self.peek(0) == b'#' {
      hashes += 1;
      self.position += 1;
    }
    if self.peek(0) != b'"' {
      return;
    }
    self.position += 1;
    let content_start = self.position;
    while self.position < self.bytes.len() {
      if self.peek(0) == b'"' && (1..=hashes).all(|offset| self.peek(offset) == b'#') {
        let value = self.slice(content_start, self.position).to_owned();
        self.position += 1 + hashes;
        self.push(TokenKind::Str, value, start, self.position);
        return;
      }
      self.position += 1;
    }
    let value = self.slice(content_start, self.bytes.len()).to_owned();
    self.push(TokenKind::Str, value, start, self.bytes.len());
  }

  fn template_chunk(&mut self) {
    let start = self.position;
    let mut value = String::new();
    let mut chunk_start = self.position;
    while self.position < self.bytes.len() {
      match self.peek(0) {
        b'`' => {
          value.push_str(self.slice(chunk_start, self.position));
          if !value.is_empty() {
            self.push(TokenKind::TemplateChunk, value, start, self.position);
          }
          self.push(TokenKind::Punct, "`".to_owned(), self.position, self.position + 1);
          self.position += 1;
          self.stack.pop();
          return;
        }
        b'$' if self.peek(1) == b'{' => {
          value.push_str(self.slice(chunk_start, self.position));
          if !value.is_empty() {
            self.push(TokenKind::TemplateChunk, value, start, self.position);
          }
          self.push(TokenKind::Punct, "${".to_owned(), self.position, self.position + 2);
          self.position += 2;
          self.stack.push(Context::Code { braces: 0 });
          return;
        }
        b'\\' => {
          value.push_str(self.slice(chunk_start, self.position));
          self.push_escape(&mut value);
          chunk_start = self.position;
        }
        _ => self.position += 1,
      }
    }
    value.push_str(self.slice(chunk_start, self.bytes.len()));
    if !value.is_empty() {
      self.push(TokenKind::TemplateChunk, value, start, self.bytes.len());
    }
    self.position = self.bytes.len();
  }

  fn number(&mut self) {
    let start = self.position;
    while is_ident_continue(self.peek(0)) || (self.peek(0) == b'.' && self.peek(1).is_ascii_digit()) {
      self.position += 1;
    }
    let text = self.slice(start, self.position).to_owned();
    self.push(TokenKind::Number, text, start, self.position);
  }

  fn ident_or_prefixed_string(&mut self) {
    let start = self.position;
    while is_ident_continue(self.peek(0)) {
      self.position += 1;
    }
    let text = self.slice(start, self.position).to_owned();
    if self.family == LanguageFamily::Rust {
      let next = self.peek(0);
      match (text.as_str(), next) {
        ("r" | "br" | "cr", b'"' | b'#') => return self.raw_string(start),
        ("b" | "c", b'"') => {
          self.string(b'"');
          if let Some(token) = self.tokens.last_mut() {
            token.start = to_u32(start);
          }
          return;
        }
        _ => {}
      }
    }
    self.push(TokenKind::Ident, text, start, self.position);
  }

  fn punct(&mut self) {
    let start = self.position;
    let rest = self.source.get(start..).unwrap_or_default();
    if let Some(punct) = MULTI_PUNCT.iter().find(|punct| rest.starts_with(**punct)) {
      self.position += punct.len();
      self.push(TokenKind::Punct, (*punct).to_owned(), start, self.position);
      return;
    }
    let width = rest.chars().next().map_or(1, char::len_utf8);
    self.position += width;
    let text = self.slice(start, self.position).to_owned();
    match text.as_str() {
      "{" => self.adjust_braces(1),
      "}" if self.closes_interpolation() => {
        self.stack.pop();
        self.push(TokenKind::Punct, "}".to_owned(), start, self.position);
        return;
      }
      "}" => self.adjust_braces(-1),
      _ => {}
    }
    self.push(TokenKind::Punct, text, start, self.position);
  }

  fn closes_interpolation(&self) -> bool {
    self.stack.len() > 1 && matches!(self.stack.last(), Some(Context::Code { braces: 0 }))
  }

  fn adjust_braces(&mut self, delta: i32) {
    if let Some(Context::Code { braces }) = self.stack.last_mut() {
      *braces = if delta > 0 { braces.saturating_add(1) } else { braces.saturating_sub(1) };
    }
  }
}

fn is_ident_start(byte: u8) -> bool {
  byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$' || byte >= 0x80
}

fn is_ident_continue(byte: u8) -> bool {
  byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte >= 0x80
}

fn to_u32(value: usize) -> u32 {
  u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn texts(source: &str, family: LanguageFamily) -> Vec<String> {
    lex(source, family).into_iter().map(|token| token.text).collect()
  }

  #[test]
  fn should_split_member_access_and_computed_strings() {
    assert_eq!(
      texts("request.headers['x-api-key'] ?? process.env.KEY", LanguageFamily::Ts),
      ["request", ".", "headers", "[", "x-api-key", "]", "??", "process", ".", "env", ".", "KEY"]
    );
  }

  #[test]
  fn should_emit_template_chunks_and_interpolated_tokens() {
    let tokens = lex("`Bearer ${this.auth.token}/x${`n${a}`}`", LanguageFamily::Ts);
    let kinds: Vec<(TokenKind, &str)> = tokens.iter().map(|token| (token.kind, token.text.as_str())).collect();
    assert_eq!(
      kinds,
      [
        (TokenKind::Punct, "`"),
        (TokenKind::TemplateChunk, "Bearer "),
        (TokenKind::Punct, "${"),
        (TokenKind::Ident, "this"),
        (TokenKind::Punct, "."),
        (TokenKind::Ident, "auth"),
        (TokenKind::Punct, "."),
        (TokenKind::Ident, "token"),
        (TokenKind::Punct, "}"),
        (TokenKind::TemplateChunk, "/x"),
        (TokenKind::Punct, "${"),
        (TokenKind::Punct, "`"),
        (TokenKind::TemplateChunk, "n"),
        (TokenKind::Punct, "${"),
        (TokenKind::Ident, "a"),
        (TokenKind::Punct, "}"),
        (TokenKind::Punct, "`"),
        (TokenKind::Punct, "}"),
        (TokenKind::Punct, "`"),
      ]
    );
  }

  #[test]
  fn should_skip_comments_and_regex_literals() {
    assert_eq!(
      texts("a = /x'y/g; // 'z'\n/* \"w\" */ b = c / d;", LanguageFamily::Ts),
      ["a", "=", ";", "b", "=", "c", "/", "d", ";"]
    );
  }

  #[test]
  fn should_lex_rust_strings_chars_and_lifetimes() {
    let tokens = lex(
      "fn f<'a>(x: &'a str) { let c = 'q'; r#\"raw \"q\"\"#; b\"bytes\"; /* /* nested */ */ }",
      LanguageFamily::Rust,
    );
    let strings: Vec<&str> = tokens.iter().filter(|token| token.kind == TokenKind::Str).map(|token| token.text.as_str()).collect();
    assert_eq!(strings, ["q", "raw \"q\"", "bytes"]);
    assert!(tokens.iter().any(|token| token.is_ident("a")));
  }

  #[test]
  fn should_decode_escapes_in_strings() {
    let tokens = lex(r#"'it\'s' "a\"b""#, LanguageFamily::Ts);
    assert_eq!(tokens[0].text, "it's");
    assert_eq!(tokens[1].text, "a\"b");
  }

  #[test]
  fn should_survive_unterminated_input() {
    for source in ["'abc", "`abc ${", "/* open", "r#\"open", "\"x\\", "'", "é'", "`${`${"] {
      let _ = lex(source, LanguageFamily::Ts);
      let _ = lex(source, LanguageFamily::Rust);
    }
  }
}
