use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::Language;
use atlas_project::{Project, discover};

use crate::error::{BenchError, BenchResult};

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MIN_NUMBER_DIGITS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
  pub abs: PathBuf,
  pub rel: String,
  pub language: Language,
}

pub fn list_sources(project: &Project) -> BenchResult<Vec<SourceFile>> {
  let mut files = Vec::new();
  for repo in &project.repos {
    let found = discover(repo, MAX_FILE_BYTES)?;
    for file in found.files {
      let Some(language) = file.language else { continue };
      files.push(SourceFile {
        abs: repo.path.join(file.path.as_str()),
        rel: format!("{}/{}", repo.name, file.path),
        language,
      });
    }
  }
  files.sort_by(|left, right| left.rel.cmp(&right.rel));
  Ok(files)
}

pub fn read(path: &Path) -> BenchResult<String> {
  fs::read_to_string(path).map_err(|source| BenchError::io("read", path, source))
}

pub fn write(path: &Path, content: &str) -> BenchResult<()> {
  fs::write(path, content).map_err(|source| BenchError::io("write", path, source))
}

pub fn count_loc(files: &[SourceFile]) -> BenchResult<u64> {
  let mut total = 0;
  for file in files {
    let bytes = fs::read(&file.abs).map_err(|source| BenchError::io("read", &file.abs, source))?;
    total += bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
  }
  Ok(total)
}

fn is_word_byte(byte: u8) -> bool {
  byte.is_ascii_alphanumeric() || byte == b'_'
}

fn number_spans(text: &str) -> Vec<(usize, usize)> {
  let bytes = text.as_bytes();
  let mut spans = Vec::new();
  let mut index = 0;
  while index < bytes.len() {
    if !bytes[index].is_ascii_digit() {
      index += 1;
      continue;
    }
    let start = index;
    while index < bytes.len() && is_word_byte(bytes[index]) {
      index += 1;
    }
    let only_digits = bytes[start..index].iter().all(u8::is_ascii_digit);
    let bounded = start == 0 || (!is_word_byte(bytes[start - 1]) && bytes[start - 1] != b'.');
    if only_digits && bounded && index - start >= MIN_NUMBER_DIGITS {
      spans.push((start, index));
    }
  }
  spans
}

pub fn bump_numbers(text: &str, count: usize) -> Option<String> {
  let spans = number_spans(text);
  if spans.is_empty() {
    return None;
  }
  let mut out = String::with_capacity(text.len() + 4);
  let mut cursor = 0;
  for (start, end) in spans.into_iter().take(count) {
    out.push_str(&text[cursor..start]);
    let value: u64 = text[start..end].parse().ok()?;
    out.push_str(&(value + 1).to_string());
    cursor = end;
  }
  out.push_str(&text[cursor..]);
  Some(out)
}

pub fn replace_word(text: &str, old: &str, new: &str) -> Option<(String, usize)> {
  if old.is_empty() {
    return None;
  }
  let bytes = text.as_bytes();
  let mut out = String::with_capacity(text.len());
  let mut cursor = 0;
  let mut replaced = 0;
  while let Some(found) = text[cursor..].find(old) {
    let start = cursor + found;
    let end = start + old.len();
    let before_ok = start == 0 || !is_word_byte(bytes[start - 1]);
    let after_ok = end >= bytes.len() || !is_word_byte(bytes[end]);
    out.push_str(&text[cursor..start]);
    if before_ok && after_ok {
      out.push_str(new);
      replaced += 1;
    } else {
      out.push_str(old);
    }
    cursor = end;
  }
  out.push_str(&text[cursor..]);
  (replaced > 0).then_some((out, replaced))
}

pub fn appended_function(language: Language, ordinal: usize) -> String {
  match language {
    Language::Rust => format!("\npub fn branch_switch_probe_{ordinal}() -> u32 {{\n  {ordinal}\n}}\n"),
    _ => format!("\nexport function branchSwitchProbe{ordinal}() {{\n  return {ordinal};\n}}\n"),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_bump_the_first_standalone_number_with_three_digits() {
    let text = "let a = compute_12(7) + 250;\nlet b = 999;\n";
    assert_eq!(bump_numbers(text, 1).as_deref(), Some("let a = compute_12(7) + 251;\nlet b = 999;\n"));
  }

  #[test]
  fn should_bump_several_numbers_in_order() {
    assert_eq!(bump_numbers("x = 100; y = 200; z = 300;", 2).as_deref(), Some("x = 101; y = 201; z = 300;"));
  }

  #[test]
  fn should_ignore_digits_inside_identifiers_and_decimals() {
    assert_eq!(bump_numbers("name_1234 v1000 1.500 0x100", 1), None);
  }

  #[test]
  fn should_return_none_when_no_number_qualifies() {
    assert_eq!(bump_numbers("fn a() {}", 1), None);
  }

  #[test]
  fn should_replace_whole_words_only() {
    let (text, count) = replace_word("fn fetch_1() {} fetch_1(); xfetch_1(); fetch_10();", "fetch_1", "fetch_9").unwrap();
    assert_eq!(text, "fn fetch_9() {} fetch_9(); xfetch_1(); fetch_10();");
    assert_eq!(count, 2);
  }

  #[test]
  fn should_report_no_replacement_when_the_word_is_absent() {
    assert_eq!(replace_word("fn other() {}", "fetch_1", "fetch_9"), None);
  }

  #[test]
  fn should_append_a_language_appropriate_function() {
    assert!(appended_function(Language::Rust, 3).contains("pub fn branch_switch_probe_3"));
    assert!(appended_function(Language::TypeScript, 3).contains("export function branchSwitchProbe3"));
  }
}
