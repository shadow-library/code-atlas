use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImplHeader {
  pub trait_path: Range<usize>,
  pub self_path: Range<usize>,
}

fn is_word_boundary(bytes: &[u8], index: usize) -> bool {
  bytes.get(index).is_none_or(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
}

fn is_keyword_at(bytes: &[u8], at: usize, keyword: &str) -> bool {
  bytes[at..].starts_with(keyword.as_bytes()) && (at == 0 || is_word_boundary(bytes, at - 1)) && is_word_boundary(bytes, at + keyword.len())
}

fn skip_whitespace(bytes: &[u8], mut at: usize) -> usize {
  while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
    at += 1;
  }
  at
}

fn skip_generics(bytes: &[u8], mut at: usize) -> Option<usize> {
  let mut depth = 0usize;
  while let Some(&byte) = bytes.get(at) {
    match byte {
      b'<' => depth += 1,
      b'>' if at > 0 && bytes[at - 1] == b'-' => {}
      b'>' => depth = depth.checked_sub(1)?,
      b'{' | b';' => return None,
      _ => {}
    }
    at += 1;
    if depth == 0 {
      return Some(at);
    }
  }
  None
}

fn path_before_generics(source: &str, range: Range<usize>) -> Option<Range<usize>> {
  let text = source.get(range.clone())?;
  let path = text.split('<').next()?;
  let trimmed = path.trim();
  let valid = !trimmed.is_empty() && trimmed.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b':');
  if !valid {
    return None;
  }
  let start = range.start + (path.len() - path.trim_start().len());
  Some(start..start + trimmed.len())
}

pub(crate) fn parse_impl_header(source: &str, impl_start: usize) -> Option<ImplHeader> {
  let bytes = source.as_bytes();
  let keyword = (impl_start..bytes.len()).find(|at| is_keyword_at(bytes, *at, "impl"))?;
  let mut at = skip_whitespace(bytes, keyword + "impl".len());
  if bytes.get(at) == Some(&b'<') {
    at = skip_generics(bytes, at)?;
  }
  let trait_start = at;
  let mut depth = 0usize;
  let mut for_at = None;
  let mut end = None;
  while at < bytes.len() {
    let byte = bytes[at];
    match byte {
      b'<' | b'(' | b'[' => depth += 1,
      b'>' if at > 0 && bytes[at - 1] == b'-' => {}
      b'>' | b')' | b']' => depth = depth.checked_sub(1)?,
      b'{' | b';' if depth != 0 => return None,
      b'{' => {
        end = Some(at);
        break;
      }
      b';' => return None,
      b'f' if depth == 0 && for_at.is_none() && is_keyword_at(bytes, at, "for") && bytes.get(skip_whitespace(bytes, at + 3)) != Some(&b'<') => for_at = Some(at),
      b'w' if depth == 0 && is_keyword_at(bytes, at, "where") => {
        end = Some(at);
        break;
      }
      _ => {}
    }
    at += 1;
  }
  let (for_at, end) = (for_at?, end?);
  Some(ImplHeader {
    trait_path: path_before_generics(source, trait_start..for_at)?,
    self_path: path_before_generics(source, for_at + "for".len()..end)?,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  fn parsed(source: &str) -> Option<(&str, &str)> {
    let header = parse_impl_header(source, 0)?;
    Some((&source[header.trait_path], &source[header.self_path]))
  }

  #[test]
  fn should_split_a_plain_trait_impl() {
    assert_eq!(parsed("impl Store for MemStore {}"), Some(("Store", "MemStore")));
    assert_eq!(parsed("unsafe impl a::Tr for b::Foo {}"), Some(("a::Tr", "b::Foo")));
  }

  #[test]
  fn should_cut_the_header_at_a_top_level_where_clause() {
    assert_eq!(parsed("impl Tr for a::Foo where b::Foo: Other {}"), Some(("Tr", "a::Foo")));
  }

  #[test]
  fn should_ignore_generic_arguments_and_impl_generics() {
    assert_eq!(parsed("impl<T: Tr> Tr<other::Tr> for Foo<T> {}"), Some(("Tr", "Foo")));
    assert_eq!(parsed("impl<F: Fn() -> u8> Tr for Foo<F> {}"), Some(("Tr", "Foo")));
  }

  #[test]
  fn should_reject_headers_that_are_not_plain_paths() {
    assert_eq!(parsed("impl Tr for &Foo {}"), None);
    assert_eq!(parsed("impl Tr for (Foo, Bar) {}"), None);
    assert_eq!(parsed("impl !Tr for Foo {}"), None);
    assert_eq!(parsed("impl Foo {}"), None);
    assert_eq!(parsed("impl Tr for dyn Other {}"), None);
  }
}
