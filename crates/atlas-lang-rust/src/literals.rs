pub(crate) fn decode_string_literal(text: &str) -> String {
  let Some(open) = text.find('"') else { return String::new() };
  let body = &text[open + 1..];
  let body = body.strip_suffix('"').unwrap_or(body);
  unescape(body)
}

pub(crate) fn decode_raw_string_literal(text: &str) -> String {
  let Some(open) = text.find(['#', '"']) else { return String::new() };
  let rest = &text[open..];
  let hashes = rest.bytes().take_while(|byte| *byte == b'#').count();
  let rest = rest[hashes..].strip_prefix('"').unwrap_or(&rest[hashes..]);
  let closing = format!("\"{}", "#".repeat(hashes));
  rest.strip_suffix(closing.as_str()).unwrap_or(rest).to_owned()
}

fn unescape(body: &str) -> String {
  let mut decoded = String::with_capacity(body.len());
  let mut chars = body.chars().peekable();
  while let Some(current) = chars.next() {
    if current != '\\' {
      decoded.push(current);
      continue;
    }
    match chars.next() {
      Some('n') => decoded.push('\n'),
      Some('r') => decoded.push('\r'),
      Some('t') => decoded.push('\t'),
      Some('0') => decoded.push('\0'),
      Some(quoted @ ('\\' | '\'' | '"')) => decoded.push(quoted),
      Some('\n') => while chars.next_if(|next| next.is_whitespace()).is_some() {},
      Some('x') => {
        let digits: String = (0..2).filter_map(|_| chars.next_if(char::is_ascii_hexdigit)).collect();
        match u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32) {
          Some(byte) => decoded.push(byte),
          None => {
            decoded.push_str("\\x");
            decoded.push_str(&digits);
          }
        }
      }
      Some('u') => decoded.push_str(&unicode_escape(&mut chars)),
      Some(other) => {
        decoded.push('\\');
        decoded.push(other);
      }
      None => decoded.push('\\'),
    }
  }
  decoded
}

fn unicode_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
  if chars.next_if_eq(&'{').is_none() {
    return "\\u".to_owned();
  }
  let mut digits = String::new();
  while let Some(digit) = chars.next_if(|next| *next != '}' && digits.len() < 8) {
    digits.push(digit);
  }
  let closed = chars.next_if_eq(&'}').is_some();
  let decoded = u32::from_str_radix(&digits.replace('_', ""), 16).ok().and_then(char::from_u32);
  match (decoded, closed) {
    (Some(character), true) => character.to_string(),
    _ => format!("\\u{{{digits}{}", if closed { "}" } else { "" }),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_decode_common_escapes() {
    assert_eq!(decode_string_literal(r#""a\n\t\"b\\c\'\0""#), "a\n\t\"b\\c'\0");
    assert_eq!(decode_string_literal(r#""\x41\u{1F600}\u{e9}""#), "A😀é");
  }

  #[test]
  fn should_skip_line_continuations() {
    assert_eq!(decode_string_literal("\"one \\\n     two\""), "one two");
  }

  #[test]
  fn should_strip_byte_and_c_string_prefixes() {
    assert_eq!(decode_string_literal(r#"b"ab""#), "ab");
    assert_eq!(decode_string_literal(r#"c"ab""#), "ab");
  }

  #[test]
  fn should_keep_invalid_escapes_verbatim() {
    assert_eq!(decode_string_literal(r#""\q\u{zz}\xZZ""#), r"\q\u{zz}\xZZ");
  }

  #[test]
  fn should_tolerate_unterminated_literals() {
    assert_eq!(decode_string_literal("\"abc"), "abc");
    assert_eq!(decode_string_literal("\"abc\\"), "abc\\");
  }

  #[test]
  fn should_decode_raw_strings_without_processing_escapes() {
    assert_eq!(decode_raw_string_literal(r#"r"a\n""#), r"a\n");
    assert_eq!(decode_raw_string_literal(r###"r##"say "hi"#"##"###), r##"say "hi"#"##);
    assert_eq!(decode_raw_string_literal(r#"br"x""#), "x");
  }
}
