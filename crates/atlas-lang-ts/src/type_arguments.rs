use std::ops::{Range, RangeInclusive};

const REGEX_PRECEDING_KEYWORDS: [&str; 14] = [
  "return",
  "typeof",
  "instanceof",
  "in",
  "of",
  "new",
  "delete",
  "void",
  "throw",
  "case",
  "do",
  "else",
  "yield",
  "await",
];

const TYPE_OPERATOR_KEYWORDS: [&str; 5] = ["extends", "import", "is", "keyof", "readonly"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frame {
  Paren(Contents),
  Bracket(Contents),
  Brace,
  Substitution,
  Angle { open: usize, after: Previous, contents: Contents },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Closer {
  Paren,
  Bracket,
  Brace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Contents {
  Type,
  Expression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
  Code,
  Template,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Previous {
  Tag,
  // Tags like `Tag`, but a parenthesis after it may still be a type, as in `keyof (A | B)`.
  TypeOperator,
  Operand,
  Operator,
}

pub(crate) struct MaskedTypeArguments {
  pub(crate) source: String,
  pub(crate) ranges: Vec<Range<usize>>,
}

pub(crate) fn mask_tagged_template_type_arguments(source: &str) -> Option<MaskedTypeArguments> {
  let ranges = disjoint(Scanner::new(source.as_bytes()).run());
  if ranges.is_empty() {
    return None;
  }
  let mut bytes = source.as_bytes().to_vec();
  for range in &ranges {
    for byte in bytes.get_mut(range.clone()).unwrap_or_default() {
      if !matches!(byte, b'\n' | b'\r') {
        *byte = b' ';
      }
    }
  }
  let source = String::from_utf8(bytes).ok()?;
  Some(MaskedTypeArguments { source, ranges })
}

fn disjoint(mut candidates: Vec<RangeInclusive<usize>>) -> Vec<Range<usize>> {
  candidates.sort_unstable_by_key(|candidate| *candidate.start());
  let mut ranges: Vec<Range<usize>> = Vec::new();
  for candidate in candidates {
    let range = *candidate.start()..*candidate.end() + 1;
    match ranges.last_mut() {
      Some(last) if range.start < last.end => last.end = last.end.max(range.end),
      _ => ranges.push(range),
    }
  }
  ranges
}

struct Scanner<'a> {
  bytes: &'a [u8],
  at: usize,
  frames: Vec<Frame>,
  previous: Previous,
  regex_free_until: usize,
  closed: Option<RangeInclusive<usize>>,
  candidates: Vec<RangeInclusive<usize>>,
}

impl<'a> Scanner<'a> {
  fn new(bytes: &'a [u8]) -> Self {
    Self {
      bytes,
      at: 0,
      frames: Vec::new(),
      previous: Previous::Operator,
      regex_free_until: 0,
      closed: None,
      candidates: Vec::new(),
    }
  }

  fn run(mut self) -> Vec<RangeInclusive<usize>> {
    let mut mode = Mode::Code;
    while self.at < self.bytes.len() {
      mode = match mode {
        Mode::Code => self.code(),
        Mode::Template => self.template(),
      };
    }
    self.candidates
  }

  fn byte(&self, offset: usize) -> Option<u8> {
    self.bytes.get(self.at + offset).copied()
  }

  fn code(&mut self) -> Mode {
    let Some(current) = self.byte(0) else { return Mode::Code };
    let next = self.byte(1);
    match (current, next) {
      (b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c, _) => self.at += 1,
      (b'/', Some(b'/')) => self.skip_line_comment(),
      (b'/', Some(b'*')) => self.skip_block_comment(),
      _ => return self.token(current, next),
    }
    Mode::Code
  }

  fn token(&mut self, current: u8, next: Option<u8>) -> Mode {
    let closed = self.closed.take();
    match (current, next) {
      (b'`', _) => {
        self.candidates.extend(closed);
        self.at += 1;
        Mode::Template
      }
      (b')', _) => self.close(Closer::Paren),
      (b']', _) => self.close(Closer::Bracket),
      (b'}', _) => self.close(Closer::Brace),
      _ => {
        self.code_token(current, next);
        Mode::Code
      }
    }
  }

  fn code_token(&mut self, current: u8, next: Option<u8>) {
    match (current, next) {
      (b'\'' | b'"', _) => self.skip_string(current),
      (b'/', _) => self.slash(),
      (b'(', _) => self.open_paren(),
      (b'[', _) => self.open(Frame::Bracket(Contents::Type)),
      (b'{', _) => self.open(Frame::Brace),
      (b'<', Some(b'=')) => self.terminate(2),
      (b'<', _) => self.open_angle(),
      (b'>', _) => self.close_angle(),
      (b'=', Some(b'>')) => self.operator(2),
      (b'=' | b';', _) | (b'!', Some(b'=')) => self.terminate(1),
      (b'&', Some(b'&')) | (b'|', Some(b'|')) | (b'?', Some(b'?')) => self.terminate(2),
      (b'+' | b'*' | b'%', _) => self.arithmetic(),
      (b'-', _) if matches!(self.previous, Previous::Tag | Previous::Operand) => self.arithmetic(),
      (byte, _) if is_identifier_byte(byte) => self.word(),
      _ => self.operator(1),
    }
  }

  fn template(&mut self) -> Mode {
    while let Some(current) = self.byte(0) {
      match (current, self.byte(1)) {
        (b'\\', _) => self.at += 2,
        (b'`', _) => {
          self.at += 1;
          self.previous = Previous::Operand;
          return Mode::Code;
        }
        (b'$', Some(b'{')) => {
          self.at += 2;
          self.frames.push(Frame::Substitution);
          self.previous = Previous::Operator;
          return Mode::Code;
        }
        _ => self.at += 1,
      }
    }
    Mode::Code
  }

  fn skip_line_comment(&mut self) {
    self.at = find_byte(b'\n', self.bytes, self.at).unwrap_or(self.bytes.len());
  }

  fn skip_block_comment(&mut self) {
    let body = self.at + 2;
    self.at = self
      .bytes
      .get(body..)
      .and_then(|rest| rest.windows(2).position(|pair| pair == b"*/"))
      .map_or(self.bytes.len(), |offset| body + offset + 2);
  }

  fn skip_string(&mut self, quote: u8) {
    self.at += 1;
    while let Some(current) = self.byte(0) {
      match current {
        b'\\' => self.at += 2,
        b'\n' => break,
        _ if current == quote => {
          self.at += 1;
          break;
        }
        _ => self.at += 1,
      }
    }
    self.previous = Previous::Operand;
  }

  fn slash(&mut self) {
    if self.previous == Previous::Operator && self.at >= self.regex_free_until {
      match self.regex_end() {
        Ok(end) => {
          self.at = end;
          self.previous = Previous::Operand;
          return;
        }
        Err(line_end) => self.regex_free_until = line_end,
      }
    }
    self.arithmetic();
  }

  // A failed attempt rules out regexes for the rest of its line, so a line of `[/[/[/…` is scanned once instead of once per slash.
  fn regex_end(&self) -> Result<usize, usize> {
    let mut at = self.at + 1;
    let mut in_class = false;
    loop {
      let Some(current) = self.bytes.get(at).copied() else { return Err(at) };
      match (current, in_class) {
        (b'\n', _) => return Err(at),
        (b'\\', _) => at += 1,
        (b'[', false) => in_class = true,
        (b']', true) => in_class = false,
        (b'/', false) => return Ok(at + 1),
        _ => {}
      }
      at += 1;
    }
  }

  fn open_paren(&mut self) {
    if self.previous == Previous::Tag {
      self.mark_angles_as_expressions();
    }
    self.open(Frame::Paren(Contents::Type));
  }

  fn arithmetic(&mut self) {
    self.mark_angles_as_expressions();
    self.operator(1);
  }

  // Only an object type in braces may hold what looks like a call or arithmetic (method signatures, `+readonly`); a type has no binary minus.
  // Frames below a marked frame were marked with it, so each frame is marked once.
  fn mark_angles_as_expressions(&mut self) {
    for frame in self.frames.iter_mut().rev() {
      match frame {
        Frame::Brace | Frame::Substitution => return,
        Frame::Angle { contents, .. } | Frame::Paren(contents) | Frame::Bracket(contents) => {
          if *contents == Contents::Expression {
            return;
          }
          *contents = Contents::Expression;
        }
      }
    }
  }

  fn open(&mut self, frame: Frame) {
    self.frames.push(frame);
    self.operator(1);
  }

  fn close(&mut self, closer: Closer) -> Mode {
    self.drop_angles();
    self.at += 1;
    self.previous = match closer {
      Closer::Brace => Previous::Operand,
      Closer::Paren | Closer::Bracket => Previous::Tag,
    };
    match (self.frames.last().copied(), closer) {
      (Some(Frame::Substitution), Closer::Brace) => {
        self.frames.pop();
        Mode::Template
      }
      (Some(Frame::Paren(_)), Closer::Paren) | (Some(Frame::Bracket(_)), Closer::Bracket) | (Some(Frame::Brace), Closer::Brace) => {
        self.frames.pop();
        Mode::Code
      }
      _ => Mode::Code,
    }
  }

  fn open_angle(&mut self) {
    self.frames.push(Frame::Angle {
      open: self.at,
      after: self.previous,
      contents: Contents::Type,
    });
    self.operator(1);
  }

  fn close_angle(&mut self) {
    if let Some(Frame::Angle { open, after, contents }) = self.frames.last().copied() {
      self.frames.pop();
      if matches!(after, Previous::Tag | Previous::TypeOperator) && contents == Contents::Type {
        self.closed = Some(open..=self.at);
      }
    }
    self.at += 1;
    self.previous = Previous::Operator;
  }

  fn terminate(&mut self, width: usize) {
    self.drop_angles();
    self.operator(width);
  }

  fn drop_angles(&mut self) {
    while matches!(self.frames.last(), Some(Frame::Angle { .. })) {
      self.frames.pop();
    }
  }

  fn operator(&mut self, width: usize) {
    self.at += width;
    self.previous = Previous::Operator;
  }

  fn word(&mut self) {
    let start = self.at;
    while self.byte(0).is_some_and(is_identifier_byte) {
      self.at += 1;
    }
    let word = self.bytes.get(start..self.at).unwrap_or_default();
    let is_in = |keywords: &[&str]| keywords.iter().any(|keyword| keyword.as_bytes() == word);
    self.previous = if is_in(&REGEX_PRECEDING_KEYWORDS) {
      Previous::Operator
    } else if is_in(&TYPE_OPERATOR_KEYWORDS) {
      Previous::TypeOperator
    } else {
      Previous::Tag
    };
  }
}

fn is_identifier_byte(byte: u8) -> bool {
  byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'\\') || !byte.is_ascii()
}

fn find_byte(needle: u8, haystack: &[u8], from: usize) -> Option<usize> {
  haystack.get(from..)?.iter().position(|byte| *byte == needle).map(|offset| from + offset)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn masked(source: &str) -> Option<String> {
    let masked = mask_tagged_template_type_arguments(source)?;
    assert_eq!(masked.source.len(), source.len());
    let changed: Vec<usize> = source
      .bytes()
      .zip(masked.source.bytes())
      .enumerate()
      .filter(|(_, (before, after))| before != after)
      .map(|(at, _)| at)
      .collect();
    assert!(changed.iter().all(|at| masked.ranges.iter().any(|range| range.contains(at))));
    assert!(masked.ranges.windows(2).all(|pair| pair[0].end < pair[1].start));
    Some(masked.source)
  }

  #[test]
  fn should_mask_the_type_arguments_of_an_identifier_tag() {
    assert_eq!(masked("const rows = sql<string[]>`select 1`;").as_deref(), Some("const rows = sql          `select 1`;"));
  }

  #[test]
  fn should_mask_the_type_arguments_of_a_call_tag() {
    assert_eq!(masked("db()<Row[]>`select ${id}`").as_deref(), Some("db()       `select ${id}`"));
  }

  #[test]
  fn should_mask_nested_type_arguments_with_arrows_objects_and_literal_types() {
    let source = "q<Map<string, (a: 'x' | \"y\") => { b: number; c: Array<`k${string}`> }>> \n`t`";
    let expected = format!("q{} \n`t`", " ".repeat(source.len() - 6));
    assert_eq!(masked(source), Some(expected));
  }

  #[test]
  fn should_keep_newlines_inside_masked_type_arguments() {
    assert_eq!(masked("sql<{\r\n  id: string\n}>`x`").as_deref(), Some("sql  \r\n            \n  `x`"));
  }

  #[test]
  fn should_replace_every_byte_of_a_multibyte_type_argument() {
    assert_eq!(masked("sql<'é'>`x`"), Some(format!("sql{}`x`", " ".repeat(6))));
  }

  #[test]
  fn should_mask_a_tag_inside_a_template_substitution() {
    assert_eq!(masked("`a ${sql<T>`b`} c`").as_deref(), Some("`a ${sql   `b`} c`"));
  }

  #[test]
  fn should_read_spaced_angles_before_a_template_as_type_arguments_as_typescript_does() {
    assert_eq!(masked("const x = a < b > `c`;").as_deref(), Some("const x = a       `c`;"));
    assert_eq!(masked("const y = f() <T>\n  `c`;").as_deref(), Some("const y = f()    \n  `c`;"));
  }

  #[test]
  fn should_leave_comparisons_generic_calls_and_keyword_angles_untouched() {
    assert_eq!(masked("const y = f<T>(x);"), None);
    assert_eq!(masked("const v = a < b > c;"), None);
    assert_eq!(masked("return <T>`x`;"), None);
    assert_eq!(masked("const w = {} <T>`x`;"), None);
    assert_eq!(masked("if (a<b) { run(`x`); } else if (c > d) {}"), None);
    assert_eq!(masked("const z = a<b; const w = c>`d`;"), None);
    assert_eq!(masked("type A<T = string> = T; `x`;"), None);
    assert_eq!(masked("const v = a<b && c>`d`;"), None);
  }

  #[test]
  fn should_leave_angles_holding_a_call_or_arithmetic_untouched() {
    assert_eq!(masked("const a = a < b() > `c`;"), None);
    assert_eq!(masked("const a = a < b + 1 > `c`;"), None);
    assert_eq!(masked("const a = a < b * 2 > `c`;"), None);
    assert_eq!(masked("const a = a < b % 2 > `c`;"), None);
    assert_eq!(masked("const a = a < b / 2 > `c`;"), None);
    assert_eq!(masked("const a = a < b - 1 > `c`;"), None);
    assert_eq!(masked("const a = a < (b)(c) > `d`;"), None);
    assert_eq!(masked("const a = a < x[0](1) > `d`;"), None);
    assert_eq!(masked("const a = a < b < c() > > `d`;"), None);
    assert_eq!(masked("const a = a < [b + 1] > `d`;"), None);
    assert_eq!(masked("const a = sql<{ m(): void } & f()>`d`;"), None);
  }

  #[test]
  fn should_still_read_type_operator_words_as_tags() {
    assert_eq!(masked("const a = db.readonly<Row>`x`;").as_deref(), Some("const a = db.readonly     `x`;"));
  }

  #[test]
  fn should_mask_type_arguments_whose_syntax_resembles_calls_or_arithmetic() {
    let types = [
      "{ m(): void; new (): T; (x: A): B }",
      "{ +readonly [K in T]-?: K }",
      "typeof import('./x')",
      "import('./x').Row",
      "keyof (A | B)",
      "T extends (infer U)[] ? U : never",
      "readonly (A | B)[]",
      "new () => T",
      "abstract new (...args: any[]) => T",
      "(x: unknown) => x is (A | B)",
      "-1 | 2",
      "[a: string, ...rest: number[]]",
      "Record<string, () => void>",
    ];
    for type_text in types {
      let source = format!("const a = sql<{type_text}>`x`;");
      let expected = format!("const a = sql{}`x`;", " ".repeat(type_text.len() + 2));
      assert_eq!(masked(&source), Some(expected), "{type_text}");
    }
  }

  #[test]
  fn should_mark_angles_once_in_linear_time() {
    let depth = 200_000;
    assert_eq!(masked(&format!("a<{}{}`x`", "(".repeat(depth), "x+".repeat(depth))), None);
  }

  #[test]
  fn should_ignore_angles_in_strings_comments_templates_and_regexes() {
    assert_eq!(masked("const s = 'sql<T>`x`'; // sql<T>`x`\n/* sql<T>`x` */"), None);
    assert_eq!(masked("const t = `sql<T>` + `x`;"), None);
    assert_eq!(masked("const r = /a<b>`/; const u = `c`;"), None);
    assert_eq!(masked("const d = x / y<T>`z`;").as_deref(), Some("const d = x / y   `z`;"));
  }

  #[test]
  fn should_stop_a_string_at_the_end_of_its_line() {
    assert_eq!(masked("<p>don't</p>\nsql<T>`x`").as_deref(), Some("<p>don't</p>\nsql   `x`"));
  }

  #[test]
  fn should_leave_unbalanced_and_unterminated_input_untouched() {
    assert_eq!(masked("sql<T`x`"), None);
    assert_eq!(masked(&format!("{}`x`", "a<".repeat(10_000))), None);
    assert_eq!(masked("sql<T>/* never closed"), None);
    assert_eq!(masked("sql<T>`never closed"), Some("sql   `never closed".to_owned()));
    assert_eq!(masked(""), None);
  }

  #[test]
  fn should_scan_a_line_of_failed_regex_starts_once() {
    let line = format!("x = {}", "[/".repeat(200_000));
    assert_eq!(masked(&format!("{line}\nsql<T>`x`")), Some(format!("{line}\nsql   `x`")));
  }

  #[test]
  fn should_find_nothing_in_a_long_chain_of_type_arguments_without_a_template() {
    assert_eq!(masked(&format!("const x = sql{};", "<a>".repeat(30_000))), None);
  }

  #[test]
  fn should_mask_deeply_nested_tags_once_in_linear_time() {
    let depth = 50_000;
    let source = format!("{}T{}", "a<".repeat(depth), ">`x`".repeat(depth));
    assert_eq!(masked(&source), Some(format!("a{}`x`", " ".repeat(source.len() - 4))));
  }
}
