use crate::text::{LineIndex, bom_len};

const MAX_REGION_LINES: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
  Rust,
  TypeScript,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TriviaSummary {
  pub docs: bool,
  pub comments: bool,
  pub attributes: bool,
}

impl TriviaSummary {
  pub fn is_empty(self) -> bool {
    !self.comments && !self.attributes
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeadingRegion {
  pub start: usize,
  pub end: usize,
  pub attributes_start: Option<usize>,
  pub summary: TriviaSummary,
}

impl LeadingRegion {
  pub fn is_empty(&self) -> bool {
    self.start == self.end
  }
}

pub fn leading_region(content: &str, declaration_start: usize, syntax: Syntax, floor: usize) -> LeadingRegion {
  let bom = bom_len(content);
  let lines = LineIndex::new(content);
  let declaration_line = lines.line_of(declaration_start.min(content.len()));
  let end = lines.start(declaration_line).max(bom);
  let floor_line = lines.line_of(floor.min(content.len()));
  let floor_line = if lines.start(floor_line) < floor { floor_line + 1 } else { floor_line };
  let first_line = declaration_line.saturating_sub(MAX_REGION_LINES).max(floor_line);
  let mut valid: Vec<Option<bool>> = vec![None; declaration_line.saturating_sub(first_line)];
  for line in (first_line..declaration_line).rev() {
    let start = lines.start(line).max(bom);
    let known = |offset: usize| {
      let absolute = start + offset;
      if absolute >= end { None } else { valid[lines.line_of(absolute) - first_line] }
    };
    let scanned = scan(&content[start..end], syntax, &known);
    valid[line - first_line] = Some(matches!(scanned.stop, Stop::End | Stop::Known(true)) && !scanned.tokens.is_empty());
  }
  let Some(earliest) = valid.iter().position(|line| *line == Some(true)) else {
    return LeadingRegion {
      start: end,
      end,
      attributes_start: None,
      summary: TriviaSummary::default(),
    };
  };
  let start = lines.start(first_line + earliest).max(bom);
  let tokens = scan(&content[start..end], syntax, &|_| None).tokens;
  let attributes_start = tokens
    .iter()
    .find(|token| token.kind == TokenKind::Attribute)
    .map(|token| lines.start(lines.line_of(start + token.offset)).max(bom));
  LeadingRegion {
    start,
    end,
    attributes_start,
    summary: summarize(&tokens),
  }
}

pub fn leading_trivia(text: &str, syntax: Syntax) -> TriviaSummary {
  summarize(&scan(text, syntax, &|_| None).tokens)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeadKind {
  Doc,
  Attribute,
  Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lead {
  pub(crate) lines: Vec<LeadKind>,
  pub(crate) body_offset: usize,
  pub(crate) inline: Option<LeadKind>,
}

pub(crate) fn lead(text: &str, syntax: Syntax) -> Lead {
  let scanned = scan(text, syntax, &|_| None);
  let body_offset = if scanned.stop == Stop::End { text.len() } else { scanned.position };
  let line_of = |offset: usize| text[..offset].matches('\n').count();
  let body_line = line_of(body_offset);
  let mut lines: Vec<Option<LeadKind>> = vec![None; body_line];
  let mut inline = None;
  for token in &scanned.tokens {
    let kind = match token.kind {
      TokenKind::DocComment => LeadKind::Doc,
      TokenKind::Attribute => LeadKind::Attribute,
      TokenKind::Comment => LeadKind::Plain,
    };
    let last = line_of(token.end.saturating_sub(1).max(token.offset));
    let first = line_of(token.offset);
    for slot in lines.iter_mut().take(last + 1).skip(first) {
      *slot = Some(strongest(*slot, kind));
    }
    if last >= body_line {
      inline = Some(strongest(inline, kind));
    }
  }
  Lead {
    lines: lines.into_iter().map(|kind| kind.unwrap_or(LeadKind::Plain)).collect(),
    body_offset,
    inline,
  }
}

fn strongest(current: Option<LeadKind>, next: LeadKind) -> LeadKind {
  match (current, next) {
    (Some(LeadKind::Attribute), _) | (_, LeadKind::Attribute) => LeadKind::Attribute,
    (Some(LeadKind::Doc), _) | (_, LeadKind::Doc) => LeadKind::Doc,
    _ => LeadKind::Plain,
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
  DocComment,
  Comment,
  Attribute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Token {
  kind: TokenKind,
  offset: usize,
  end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
  End,
  Known(bool),
  Code,
  BlankLine,
  Unterminated,
}

struct Scan {
  tokens: Vec<Token>,
  stop: Stop,
  position: usize,
}

fn summarize(tokens: &[Token]) -> TriviaSummary {
  TriviaSummary {
    docs: tokens.iter().any(|token| token.kind == TokenKind::DocComment),
    comments: tokens.iter().any(|token| matches!(token.kind, TokenKind::DocComment | TokenKind::Comment)),
    attributes: tokens.iter().any(|token| token.kind == TokenKind::Attribute),
  }
}

fn is_doc_attribute(attribute: &str) -> bool {
  attribute
    .strip_prefix("#[")
    .map(str::trim_start)
    .and_then(|rest| rest.strip_prefix("doc"))
    .is_some_and(|rest| rest.trim_start().starts_with('='))
}

fn comment_kind(rest: &str, syntax: Syntax) -> TokenKind {
  let block_doc = rest.starts_with("/**") && !rest.starts_with("/***") && !rest.starts_with("/**/");
  let line_doc = syntax == Syntax::Rust && rest.starts_with("///") && !rest.starts_with("////");
  if block_doc || line_doc { TokenKind::DocComment } else { TokenKind::Comment }
}

fn scan(text: &str, syntax: Syntax, known: &dyn Fn(usize) -> Option<bool>) -> Scan {
  let bytes = text.as_bytes();
  let mut tokens = Vec::new();
  let mut position = 0;
  let mut line_has_token = false;
  let stop = loop {
    while position < bytes.len() && matches!(bytes[position], b' ' | b'\t' | b'\r') {
      position += 1;
    }
    if position == bytes.len() {
      break Stop::End;
    }
    let rest = &text[position..];
    let (kind, next) = if bytes[position] == b'\n' {
      if !line_has_token {
        break Stop::BlankLine;
      }
      line_has_token = false;
      position += 1;
      if let Some(valid) = known(position) {
        break Stop::Known(valid);
      }
      continue;
    } else if rest.starts_with("//") {
      if syntax == Syntax::Rust && rest.starts_with("//!") {
        break Stop::Code;
      }
      (comment_kind(rest, syntax), Some(rest.find('\n').map_or(bytes.len(), |newline| position + newline)))
    } else if rest.starts_with("/*") {
      if syntax == Syntax::Rust && rest.starts_with("/*!") {
        break Stop::Code;
      }
      (comment_kind(rest, syntax), block_comment_end(text, position, syntax))
    } else if syntax == Syntax::Rust && rest.starts_with("#[") {
      let end = balanced_end(text, position + 1, b'[', b']', syntax);
      let kind = if end.is_some_and(|end| is_doc_attribute(&text[position..end])) {
        TokenKind::DocComment
      } else {
        TokenKind::Attribute
      };
      (kind, end)
    } else if syntax == Syntax::TypeScript && rest.starts_with('@') {
      let Some(end) = decorator_end(text, position) else { break Stop::Code };
      (TokenKind::Attribute, end)
    } else {
      break Stop::Code;
    };
    let Some(next) = next else { break Stop::Unterminated };
    tokens.push(Token {
      kind,
      offset: position,
      end: next,
    });
    position = next;
    line_has_token = true;
  };
  Scan { tokens, stop, position }
}

fn block_comment_end(text: &str, start: usize, syntax: Syntax) -> Option<usize> {
  let bytes = text.as_bytes();
  let mut depth = 0usize;
  let mut position = start;
  while position + 1 < bytes.len() {
    match (bytes[position], bytes[position + 1]) {
      (b'/', b'*') if depth == 0 || syntax == Syntax::Rust => {
        depth += 1;
        position += 2;
      }
      (b'*', b'/') => {
        depth -= 1;
        position += 2;
        if depth == 0 {
          return Some(position);
        }
      }
      _ => position += 1,
    }
  }
  None
}

fn balanced_end(text: &str, open: usize, opener: u8, closer: u8, syntax: Syntax) -> Option<usize> {
  let bytes = text.as_bytes();
  let mut depth = 0usize;
  let mut position = open;
  while position < bytes.len() {
    let byte = bytes[position];
    if byte == opener {
      depth += 1;
    } else if byte == closer {
      depth -= 1;
      if depth == 0 {
        return Some(position + 1);
      }
    } else if let Some(end) = string_end(text, position, syntax) {
      position = end;
      continue;
    }
    position += 1;
  }
  None
}

fn string_end(text: &str, position: usize, syntax: Syntax) -> Option<usize> {
  let bytes = text.as_bytes();
  match (syntax, bytes[position]) {
    (_, b'"') => quoted_end(bytes, position, b'"'),
    (Syntax::TypeScript, quote @ (b'\'' | b'`')) => quoted_end(bytes, position, quote),
    (Syntax::Rust, b'r') if position == 0 || !is_identifier_byte(bytes[position - 1]) => raw_string_end(text, position),
    (Syntax::Rust, b'\'') => char_literal_end(text, position),
    _ => None,
  }
}

fn quoted_end(bytes: &[u8], start: usize, quote: u8) -> Option<usize> {
  let mut position = start + 1;
  while position < bytes.len() {
    match bytes[position] {
      b'\\' => position += 2,
      byte if byte == quote => return Some(position + 1),
      _ => position += 1,
    }
  }
  Some(bytes.len())
}

fn raw_string_end(text: &str, start: usize) -> Option<usize> {
  let rest = &text[start + 1..];
  let hashes = rest.bytes().take_while(|byte| *byte == b'#').count();
  if rest.as_bytes().get(hashes) != Some(&b'"') {
    return None;
  }
  let body_start = start + 1 + hashes + 1;
  let terminator = format!("\"{}", "#".repeat(hashes));
  Some(text[body_start..].find(&terminator).map_or(text.len(), |found| body_start + found + terminator.len()))
}

fn char_literal_end(text: &str, start: usize) -> Option<usize> {
  let rest = &text[start + 1..];
  if rest.starts_with('\\') {
    return text.get(start + 3..)?.find('\'').map(|found| start + 3 + found + 1);
  }
  let mut chars = rest.chars();
  let literal = chars.next()?;
  (chars.next() == Some('\'')).then_some(start + 1 + literal.len_utf8() + 1)
}

fn decorator_end(text: &str, at: usize) -> Option<Option<usize>> {
  let bytes = text.as_bytes();
  let mut position = at + 1;
  loop {
    if !bytes.get(position).is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')) {
      return None;
    }
    while bytes.get(position).is_some_and(|byte| is_identifier_byte(*byte)) {
      position += 1;
    }
    if bytes.get(position) != Some(&b'.') {
      break;
    }
    position += 1;
  }
  if bytes.get(position) == Some(&b'(') {
    return Some(balanced_end(text, position, b'(', b')', Syntax::TypeScript));
  }
  Some(Some(position))
}

fn is_identifier_byte(byte: u8) -> bool {
  byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

#[cfg(test)]
mod tests {
  use super::*;

  fn region_text(content: &str, declaration: &str, syntax: Syntax) -> String {
    let start = content.find(declaration).unwrap();
    let region = leading_region(content, start, syntax, 0);
    content[region.start..region.end].to_owned()
  }

  #[test]
  fn should_include_rust_docs_and_multi_line_attributes_up_to_a_blank_line() {
    let content = "use a;\n\n// unrelated\n\n/// Docs.\n/// More.\n#[derive(\n  Debug,\n  Clone,\n)]\n#[serde(rename = \"x]\")]\npub struct S;\n";
    assert_eq!(
      region_text(content, "pub struct", Syntax::Rust),
      "/// Docs.\n/// More.\n#[derive(\n  Debug,\n  Clone,\n)]\n#[serde(rename = \"x]\")]\n"
    );
  }

  #[test]
  fn should_stop_at_code_directly_above() {
    let content = "fn a() {}\n// about b\nfn b() {}\n";
    assert_eq!(region_text(content, "fn b", Syntax::Rust), "// about b\n");
  }

  #[test]
  fn should_include_block_doc_comments_with_star_continuations() {
    let content = "fn a() {}\n\n  /**\n   * Docs.\n   *\n   */\n  #[inline]\n  fn b() {}\n";
    assert_eq!(region_text(content, "fn b", Syntax::Rust), "  /**\n   * Docs.\n   *\n   */\n  #[inline]\n");
  }

  #[test]
  fn should_keep_blank_lines_inside_block_comments() {
    let content = "/* first\n\n   second */\nfn b() {}\n";
    assert_eq!(region_text(content, "fn b", Syntax::Rust), "/* first\n\n   second */\n");
  }

  #[test]
  fn should_exclude_inner_docs_and_inner_attributes() {
    let content = "//! Crate docs.\n#![allow(dead_code)]\n/// Item docs.\nfn b() {}\n";
    assert_eq!(region_text(content, "fn b", Syntax::Rust), "/// Item docs.\n");
  }

  #[test]
  fn should_include_ts_jsdoc_and_decorators_with_balanced_parens() {
    let content = "import x from 'x';\n\n/**\n * Controller.\n */\n@Controller('users')\n@UseGuards(\n  AuthGuard(')'),\n)\nexport class UsersController {}\n";
    assert_eq!(
      region_text(content, "export class", Syntax::TypeScript),
      "/**\n * Controller.\n */\n@Controller('users')\n@UseGuards(\n  AuthGuard(')'),\n)\n"
    );
  }

  #[test]
  fn should_return_an_empty_region_after_a_blank_line() {
    let content = "/// Orphan.\n\nfn b() {}\n";
    let start = content.find("fn b").unwrap();
    let region = leading_region(content, start, Syntax::Rust, 0);
    assert_eq!((region.start, region.end, region.is_empty()), (start, start, true));
  }

  #[test]
  fn should_locate_the_first_attribute_line() {
    let content = "/// Docs.\n#[inline]\n#[must_use]\nfn b() {}\n";
    let region = leading_region(content, content.find("fn b").unwrap(), Syntax::Rust, 0);
    assert_eq!(region.attributes_start, Some(10));
    assert_eq!(
      region.summary,
      TriviaSummary {
        docs: true,
        comments: true,
        attributes: true
      }
    );
  }

  #[test]
  fn should_not_treat_a_ts_decorator_as_rust_or_rust_attributes_as_ts() {
    assert_eq!(region_text("@Injectable()\nfn b() {}\n", "fn b", Syntax::Rust), "");
    assert_eq!(region_text("#[inline]\nfunction b() {}\n", "function b", Syntax::TypeScript), "");
  }

  #[test]
  fn should_summarize_the_leading_trivia_of_new_text() {
    assert_eq!(
      leading_trivia("/// Docs.\nfn b() {}", Syntax::Rust),
      TriviaSummary {
        docs: true,
        comments: true,
        attributes: false
      }
    );
    assert_eq!(
      leading_trivia("#[inline]\nfn b() {}", Syntax::Rust),
      TriviaSummary {
        docs: false,
        comments: false,
        attributes: true
      }
    );
    assert_eq!(
      leading_trivia("@Get(':id')\nfind() {}", Syntax::TypeScript),
      TriviaSummary {
        docs: false,
        comments: false,
        attributes: true
      }
    );
    assert_eq!(leading_trivia("fn b() {}", Syntax::Rust), TriviaSummary::default());
  }

  #[test]
  fn should_handle_crlf_and_a_byte_order_mark() {
    let content = "\u{feff}/// Docs.\r\n#[inline]\r\nfn b() {}\r\n";
    assert_eq!(region_text(content, "fn b", Syntax::Rust), "/// Docs.\r\n#[inline]\r\n");
  }

  #[test]
  fn should_reject_unterminated_attributes() {
    assert_eq!(region_text("#[derive(Debug\nfn b() {}\n", "fn b", Syntax::Rust), "");
  }

  #[test]
  fn should_tell_doc_comments_from_plain_comments() {
    let summary = |text, syntax| leading_trivia(text, syntax);
    assert_eq!(
      summary("// note\nfn a() {}", Syntax::Rust),
      TriviaSummary {
        docs: false,
        comments: true,
        attributes: false
      }
    );
    assert!(!summary("//// banner\nfn a() {}", Syntax::Rust).docs);
    assert!(!summary("/**/\nfn a() {}", Syntax::Rust).docs);
    assert!(summary("/** Docs. */\nfn a() {}", Syntax::Rust).docs);
    assert!(!summary("/// <reference path=\"x\" />\nexport {}", Syntax::TypeScript).docs);
    assert!(summary("/** Docs. */\nexport {}", Syntax::TypeScript).docs);
  }

  #[test]
  fn should_classify_lead_lines_and_find_the_body() {
    use LeadKind::{Attribute, Doc, Plain};
    let text = "// NOTE\n/// Docs.\n#[doc = \"More.\"]\n#[derive(\n  Debug,\n)]\nfn a() {}";
    assert_eq!(
      lead(text, Syntax::Rust),
      Lead {
        lines: vec![Plain, Doc, Doc, Attribute, Attribute, Attribute],
        body_offset: text.find("fn a").unwrap(),
        inline: None,
      }
    );
    assert_eq!(
      lead("@Input() name: string;", Syntax::TypeScript),
      Lead {
        lines: Vec::new(),
        body_offset: 9,
        inline: Some(Attribute),
      }
    );
    assert_eq!(lead("#[doc(hidden)]\nfn a() {}", Syntax::Rust).lines, [Attribute]);
    assert_eq!(
      lead("fn a() {}", Syntax::Rust),
      Lead {
        lines: Vec::new(),
        body_offset: 0,
        inline: None,
      }
    );
  }

  #[test]
  fn should_not_start_a_region_before_the_floor() {
    let ts = "const t = `\n// end`;\nexport function b() {}\n";
    let start = ts.find("export").unwrap();
    assert_eq!(region_text(ts, "export", Syntax::TypeScript), "// end`;\n");
    let floored = leading_region(ts, start, Syntax::TypeScript, ts.find(";\n").unwrap() + 1);
    assert!(floored.is_empty());
    let rust = "const S: &str = \"abc\n// x\";\n/// Docs.\nfn b() {}\n";
    let start = rust.find("fn b").unwrap();
    let floor = rust.find("\";\n").unwrap() + 2;
    let region = leading_region(rust, start, Syntax::Rust, floor);
    assert_eq!(&rust[region.start..region.end], "/// Docs.\n");
  }

  #[test]
  fn should_balance_attributes_containing_char_literals() {
    let content = "#[cfg_attr(x, doc = \"]\")]\n#[foo(']')]\n#[bar('\\'', 'é', '\\u{5d}')]\nfn b<'a>() {}\n";
    assert_eq!(
      region_text(content, "fn b", Syntax::Rust),
      "#[cfg_attr(x, doc = \"]\")]\n#[foo(']')]\n#[bar('\\'', 'é', '\\u{5d}')]\n"
    );
    assert_eq!(region_text("#[x(for<'a> Fn(&'a str))]\nfn b() {}\n", "fn b", Syntax::Rust), "#[x(for<'a> Fn(&'a str))]\n");
  }

  #[test]
  fn should_scan_a_long_comment_run_once_per_line() {
    let comments = "// line\n".repeat(400);
    let content = format!("fn a() {{}}\n{comments}fn b() {{}}\n");
    let started = std::time::Instant::now();
    assert_eq!(region_text(&content, "fn b", Syntax::Rust), comments);
    assert!(started.elapsed() < std::time::Duration::from_millis(200), "{:?}", started.elapsed());
  }
}
