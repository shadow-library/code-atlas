use std::collections::BTreeMap;

const MAX_TOKENS: usize = 4_000;
const MAX_LINES_PER_TOKEN: usize = 64;
const MIN_TOKEN_BYTES: usize = 2;
const MAX_TOKEN_BYTES: usize = 80;
const MAX_TEMPLATE_NESTING: usize = 8;
const MAX_CHAR_LITERAL_BYTES: usize = 12;
const MAX_BRACKET_DEPTH: usize = 256;
const IMPORT_WORDS: [&[u8]; 3] = [b"from", b"import", b"module"];
const LOADER_WORDS: [&[u8]; 2] = [b"import", b"require"];
const BINDING_WORDS: [&[u8]; 4] = [b"case", b"const", b"let", b"var"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Syntax {
  Rust,
  Script,
}

pub(crate) type Tokens = BTreeMap<String, Vec<u32>>;

pub(crate) fn contract_tokens(text: &str, syntax: Syntax) -> Tokens {
  let mut scanner = Scanner {
    bytes: text.as_bytes(),
    at: 0,
    line: 1,
    found: Tokens::new(),
    recent: [Recent::Nothing, Recent::Nothing],
    brackets: Vec::new(),
    overflow: 0,
  };
  match syntax {
    Syntax::Rust => scanner.rust(),
    Syntax::Script => scanner.script(),
  }
  scanner.found
}

fn is_token(bytes: &[u8]) -> bool {
  let Some((first, rest)) = bytes.split_first() else { return false };
  (MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&bytes.len())
    && (first.is_ascii_alphabetic() || *first == b'_')
    && rest.iter().all(|byte| byte.is_ascii_alphanumeric() || b"_.:/-".contains(byte))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recent<'t> {
  Nothing,
  Word(&'t [u8]),
  Punct(u8),
  Literal,
}

struct Scanner<'t> {
  bytes: &'t [u8],
  at: usize,
  line: u32,
  found: Tokens,
  recent: [Recent<'t>; 2],
  brackets: Vec<u8>,
  overflow: usize,
}

impl<'t> Scanner<'t> {
  fn peek(&self, offset: usize) -> Option<u8> {
    self.bytes.get(self.at + offset).copied()
  }

  fn push(&mut self, recent: Recent<'t>) {
    self.recent = [self.recent[1], recent];
  }

  fn record(&mut self, token: &[u8], line: u32) {
    if !is_token(token) {
      return;
    }
    let Ok(token) = std::str::from_utf8(token) else { return };
    if !self.found.contains_key(token) && self.found.len() >= MAX_TOKENS {
      return;
    }
    let lines = self.found.entry(token.to_owned()).or_default();
    if lines.last() != Some(&line) && lines.len() < MAX_LINES_PER_TOKEN {
      lines.push(line);
    }
  }

  fn newline_or_space(&mut self, byte: u8) -> bool {
    match byte {
      b'\n' => self.line += 1,
      b' ' | b'\t' | b'\r' | 0x0b | 0x0c => {}
      _ => return false,
    }
    self.at += 1;
    true
  }

  fn comment(&mut self, nested: bool) -> bool {
    match (self.peek(0), self.peek(1)) {
      (Some(b'/'), Some(b'/')) => {
        while self.peek(0).is_some_and(|byte| byte != b'\n') {
          self.at += 1;
        }
        true
      }
      (Some(b'/'), Some(b'*')) => {
        self.at += 2;
        let mut depth = 1usize;
        while depth > 0 && self.at < self.bytes.len() {
          match (self.peek(0), self.peek(1)) {
            (Some(b'*'), Some(b'/')) => {
              depth -= 1;
              self.at += 2;
            }
            (Some(b'/'), Some(b'*')) if nested => {
              depth += 1;
              self.at += 2;
            }
            (Some(b'\n'), _) => {
              self.line += 1;
              self.at += 1;
            }
            _ => self.at += 1,
          }
        }
        true
      }
      _ => false,
    }
  }

  fn word(&mut self) -> &'t [u8] {
    let start = self.at;
    while self.peek(0).is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$') {
      self.at += 1;
    }
    &self.bytes[start..self.at]
  }

  fn script(&mut self) {
    while let Some(byte) = self.peek(0) {
      if self.newline_or_space(byte) || self.comment(false) {
        continue;
      }
      match byte {
        b'\'' | b'"' => {
          let line = self.line;
          let content = self.quoted(byte, false);
          if !self.imports_module() {
            self.record(content, line);
          }
          self.push(Recent::Literal);
        }
        b'`' => {
          self.template(0);
          self.push(Recent::Literal);
        }
        _ if byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$' => {
          let line = self.line;
          let word = self.word();
          if word != b"default" && self.is_key() {
            self.record(word, line);
          }
          self.push(Recent::Word(word));
        }
        _ => {
          self.at += 1;
          self.bracket(byte);
          self.push(Recent::Punct(byte));
        }
      }
    }
  }

  fn bracket(&mut self, byte: u8) {
    let opener = match byte {
      b')' => b'(',
      b']' => b'[',
      b'}' => b'{',
      b'(' | b'[' | b'{' if self.brackets.len() < MAX_BRACKET_DEPTH => {
        self.brackets.push(byte);
        return;
      }
      b'(' | b'[' | b'{' => {
        self.overflow += 1;
        return;
      }
      _ => return,
    };
    if self.overflow > 0 {
      self.overflow -= 1;
      return;
    }
    if let Some(index) = self.brackets.iter().rposition(|open| *open == opener) {
      self.brackets.truncate(index);
    }
  }

  fn imports_module(&self) -> bool {
    match self.recent {
      [_, Recent::Word(word)] => IMPORT_WORDS.contains(&word),
      [Recent::Word(word), Recent::Punct(b'(')] => LOADER_WORDS.contains(&word),
      _ => false,
    }
  }

  fn is_key(&self) -> bool {
    let colon = match (self.peek(0), self.peek(1)) {
      (Some(b':'), next) => next != Some(b':'),
      (Some(b'?'), Some(b':')) => self.peek(2) != Some(b':'),
      _ => false,
    };
    colon
      && self.brackets.last().is_none_or(|open| *open == b'{')
      && match self.recent[1] {
        Recent::Punct(punct) => !b"?.(".contains(&punct),
        Recent::Word(word) => !BINDING_WORDS.contains(&word),
        Recent::Nothing | Recent::Literal => true,
      }
  }

  fn quoted(&mut self, quote: u8, multiline: bool) -> &'t [u8] {
    self.at += 1;
    let start = self.at;
    while let Some(byte) = self.peek(0) {
      match byte {
        _ if byte == quote => {
          let content = &self.bytes[start..self.at];
          self.at += 1;
          return content;
        }
        b'\\' => {
          if self.peek(1) == Some(b'\n') {
            self.line += 1;
          }
          self.at = (self.at + 2).min(self.bytes.len());
        }
        b'\n' if !multiline => return &self.bytes[start..self.at],
        b'\n' => {
          self.line += 1;
          self.at += 1;
        }
        _ => self.at += 1,
      }
    }
    &self.bytes[start..self.at]
  }

  fn template(&mut self, nesting: usize) {
    let line = self.line;
    self.at += 1;
    let start = self.at;
    let mut interpolated = false;
    while let Some(byte) = self.peek(0) {
      match byte {
        b'`' => {
          if !interpolated {
            self.record(&self.bytes[start..self.at], line);
          }
          self.at += 1;
          return;
        }
        b'\\' => {
          if self.peek(1) == Some(b'\n') {
            self.line += 1;
          }
          self.at = (self.at + 2).min(self.bytes.len());
        }
        b'$' if self.peek(1) == Some(b'{') => {
          interpolated = true;
          self.interpolation(nesting);
        }
        b'\n' => {
          self.line += 1;
          self.at += 1;
        }
        _ => self.at += 1,
      }
    }
  }

  fn interpolation(&mut self, nesting: usize) {
    self.at += 2;
    let mut depth = 1usize;
    while let Some(byte) = self.peek(0) {
      if self.newline_or_space(byte) || self.comment(false) {
        continue;
      }
      match byte {
        b'{' => {
          depth += 1;
          self.at += 1;
        }
        b'}' => {
          depth -= 1;
          self.at += 1;
          if depth == 0 {
            return;
          }
        }
        b'\'' | b'"' => {
          let line = self.line;
          let content = self.quoted(byte, false);
          self.record(content, line);
        }
        b'`' if nesting < MAX_TEMPLATE_NESTING => self.template(nesting + 1),
        _ => self.at += 1,
      }
    }
  }

  fn rust(&mut self) {
    while let Some(byte) = self.peek(0) {
      if self.newline_or_space(byte) || self.comment(true) {
        continue;
      }
      match byte {
        b'"' => {
          let line = self.line;
          let content = self.quoted(b'"', true);
          self.record(content, line);
        }
        b'\'' => self.char_or_lifetime(),
        _ if byte.is_ascii_alphabetic() || byte == b'_' => {
          let word = self.word();
          self.prefixed_string(word);
        }
        _ => self.at += 1,
      }
    }
  }

  fn prefixed_string(&mut self, prefix: &[u8]) {
    let line = self.line;
    match prefix {
      b"r" | b"br" | b"cr" => {
        let hashes = self.bytes[self.at..].iter().take_while(|byte| **byte == b'#').count();
        if self.peek(hashes) == Some(b'"') {
          self.at += hashes;
          let content = self.raw(hashes);
          self.record(content, line);
        }
      }
      b"b" | b"c" if self.peek(0) == Some(b'"') => {
        let content = self.quoted(b'"', true);
        self.record(content, line);
      }
      _ => {}
    }
  }

  fn raw(&mut self, hashes: usize) -> &'t [u8] {
    self.at += 1;
    let start = self.at;
    while let Some(byte) = self.peek(0) {
      if byte == b'"' && self.bytes[self.at + 1..].iter().take(hashes).filter(|byte| **byte == b'#').count() == hashes {
        let content = &self.bytes[start..self.at];
        self.at += 1 + hashes;
        return content;
      }
      if byte == b'\n' {
        self.line += 1;
      }
      self.at += 1;
    }
    &self.bytes[start..self.at]
  }

  fn char_or_lifetime(&mut self) {
    match self.peek(1) {
      Some(b'\\') => {
        let close = self.bytes[self.at + 2..]
          .iter()
          .take(MAX_CHAR_LITERAL_BYTES)
          .position(|byte| *byte == b'\'' || *byte == b'\n');
        self.at += match close {
          Some(offset) if self.bytes[self.at + 2 + offset] == b'\'' => offset + 3,
          _ => 1,
        };
      }
      Some(lead) => {
        let width = utf8_width(lead);
        self.at += match self.peek(1 + width) {
          Some(b'\'') => width + 2,
          _ => 1,
        };
      }
      None => self.at += 1,
    }
  }
}

fn utf8_width(lead: u8) -> usize {
  match lead {
    0xf0..=0xf7 => 4,
    0xe0..=0xef => 3,
    0xc0..=0xdf => 2,
    _ => 1,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn tokens(text: &str, syntax: Syntax) -> Vec<(String, Vec<u32>)> {
    contract_tokens(text, syntax).into_iter().collect()
  }

  fn names(text: &str, syntax: Syntax) -> Vec<String> {
    contract_tokens(text, syntax).into_keys().collect()
  }

  fn owned(lines: &[(&str, &[u32])]) -> Vec<(String, Vec<u32>)> {
    lines.iter().map(|(token, lines)| ((*token).to_owned(), lines.to_vec())).collect()
  }

  #[test]
  fn should_read_string_contents_and_property_keys_with_their_lines() {
    let text = "const SPECS = {\n  'op.merge_pages': { required: { target: 'string' } },\n  \"op.trim\": { optional?: 1 },\n};\nsend('op.merge_pages');\n";
    assert_eq!(
      tokens(text, Syntax::Script),
      owned(&[
        ("op.merge_pages", &[2, 5]),
        ("op.trim", &[3]),
        ("optional", &[3]),
        ("required", &[2]),
        ("string", &[2]),
        ("target", &[2]),
      ])
    );
  }

  #[test]
  fn should_take_a_template_only_without_interpolation() {
    let text = "a(`plain/topic`);\nb(`page/${id}/x`);\nc(`x ${`nested.topic`} y`);\n";
    assert_eq!(tokens(text, Syntax::Script), owned(&[("nested.topic", &[3]), ("plain/topic", &[1])]));
  }

  #[test]
  fn should_skip_import_specifiers_relative_and_alias_paths_and_non_tokens() {
    let text = "import { a } from 'pkg-name';\nimport 'side-effect';\nexport * from \"barrel\";\nconst m = await import('lazy-mod');\nconst r = require('legacy-mod');\ndeclare module 'ambient-mod' {}\nconst paths = ['./local', '../up', '@/alias', '~/home', 'kept/path'];\nconst text = 'two words';\nconst one = 'x';\nconst long = 'a';\n";
    assert_eq!(names(text, Syntax::Script), ["kept/path"]);
  }

  #[test]
  fn should_not_take_ternaries_case_labels_members_comments_or_parameters_as_keys() {
    let text = "const v = flag ? left: right;\nswitch (k) { case member_kind: break; default: break; }\n// note: 'commented'\n/* block: 'also' */\nconst o = { kept_key: 1 };\nlet typed_binding: Spec = {};\nfunction f(first_param: number, second_param: string) {}\nconst g = (arrow_param: T, other_param?: U) => ({ body_key: 2 });\nsend({ arg_key: 1 }, [tuple_label: 3]);\nclass C { field_key: string; method(method_param: T) {} }\n";
    assert_eq!(names(text, Syntax::Script), ["arg_key", "body_key", "field_key", "kept_key"]);
  }

  #[test]
  fn should_recover_key_detection_after_unbalanced_and_deeply_nested_brackets() {
    let unbalanced = "const re = /\\(/;\nconst o = { after_regex: 1 };\n";
    assert_eq!(names(unbalanced, Syntax::Script), ["after_regex"]);
    let deep = format!(
      "{}{}\nconst o = {{ after_depth: 1 }};\n",
      "(".repeat(MAX_BRACKET_DEPTH + 10),
      ")".repeat(MAX_BRACKET_DEPTH + 10)
    );
    assert_eq!(names(&deep, Syntax::Script), ["after_depth"]);
  }

  #[test]
  fn should_read_only_string_literals_in_rust() {
    let text = "// 'comment.topic'\nfn f<'a>(x: &'a str) -> char {\n  let key = Key { field_name: 1 };\n  let q = '\"';\n  let e = '\\n';\n  let u = 'é';\n  let s = \"job.retry_kind\";\n  let raw = r#\"raw.topic\"#;\n  let b = b\"byte.topic\";\n  let r#type = 1;\n  'x'\n}\n/* outer /* \"nested.comment\" */ \"still.comment\" */\nconst LAST: &str = \"last.topic\";\n";
    assert_eq!(
      tokens(text, Syntax::Rust),
      owned(&[("byte.topic", &[9]), ("job.retry_kind", &[7]), ("last.topic", &[14]), ("raw.topic", &[8])])
    );
  }

  #[test]
  fn should_bound_tokens_per_file_and_lines_per_token() {
    let many: String = (0..5_000).map(|index| format!("'t{index:05}'\n")).collect();
    assert_eq!(contract_tokens(&many, Syntax::Script).len(), MAX_TOKENS);
    let repeated = "'same.topic'\n".repeat(200);
    assert_eq!(contract_tokens(&repeated, Syntax::Script)["same.topic"].len(), MAX_LINES_PER_TOKEN);
    let long = format!("'{}'", "a".repeat(MAX_TOKEN_BYTES + 1));
    assert!(contract_tokens(&long, Syntax::Script).is_empty());
  }

  #[test]
  fn should_survive_unterminated_and_malformed_input() {
    for text in [
      "'open",
      "`open ${ 'x",
      "\"a\\",
      "r##\"open",
      "'\\",
      "'",
      "/* open",
      "`${`${`${`${`${`${`${`${`${`${",
      "b'",
      "\u{e9}'\u{e9}",
    ] {
      contract_tokens(text, Syntax::Script);
      contract_tokens(text, Syntax::Rust);
    }
    assert_eq!(names("'open.topic\nnext", Syntax::Script), ["open.topic"]);
  }
}
