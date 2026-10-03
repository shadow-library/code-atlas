use std::collections::HashSet;
use std::sync::LazyLock;

const STOPWORDS: &[&str] = &[
  "fn",
  "func",
  "function",
  "def",
  "let",
  "const",
  "var",
  "mut",
  "pub",
  "public",
  "private",
  "protected",
  "static",
  "async",
  "await",
  "return",
  "if",
  "else",
  "for",
  "while",
  "loop",
  "match",
  "switch",
  "case",
  "class",
  "struct",
  "enum",
  "interface",
  "impl",
  "trait",
  "type",
  "use",
  "import",
  "export",
  "from",
  "mod",
  "crate",
  "self",
  "this",
  "super",
  "get",
  "set",
  "void",
  "null",
  "undefined",
  "none",
  "some",
  "true",
  "false",
  "the",
  "an",
  "of",
  "in",
  "is",
  "are",
  "and",
  "or",
  "to",
  "as",
  "with",
  "on",
  "by",
  "it",
  "be",
  "at",
  "that",
  "its",
  "src",
  "rs",
  "ts",
  "tsx",
  "js",
  "jsx",
  "mjs",
];

static STOPWORD_SET: LazyLock<HashSet<&'static str>> = LazyLock::new(|| STOPWORDS.iter().copied().collect());

pub(crate) fn split_words(input: &str) -> Vec<String> {
  let mut words = Vec::new();
  for segment in input.split(|c: char| !c.is_alphanumeric()).filter(|segment| !segment.is_empty()) {
    let chars: Vec<char> = segment.chars().collect();
    let mut start = 0;
    for index in 1..chars.len() {
      if is_boundary(&chars, index) {
        words.push(lowercase(&chars[start..index]));
        start = index;
      }
    }
    words.push(lowercase(&chars[start..]));
  }
  words
}

fn lowercase(chars: &[char]) -> String {
  chars.iter().flat_map(|c| c.to_lowercase()).collect()
}

fn is_boundary(chars: &[char], index: usize) -> bool {
  let (previous, current) = (chars[index - 1], chars[index]);
  if previous.is_lowercase() && current.is_uppercase() {
    return true;
  }
  if previous.is_numeric() != current.is_numeric() {
    return true;
  }
  previous.is_uppercase() && current.is_uppercase() && chars.get(index + 1).is_some_and(|next| next.is_lowercase())
}

fn is_stopword(word: &str) -> bool {
  let mut chars = word.chars();
  let single_letter = matches!((chars.next(), chars.next()), (Some(c), None) if c.is_alphabetic());
  single_letter || STOPWORD_SET.contains(word)
}

pub(crate) fn analyze(input: &str) -> Vec<String> {
  let words = split_words(input);
  let kept: Vec<&String> = words.iter().filter(|word| !is_stopword(word)).collect();
  if kept.is_empty() {
    return words.iter().map(|word| stem(word)).collect();
  }
  kept.into_iter().map(|word| stem(word)).collect()
}

pub(crate) struct QueryTerms {
  pub direct: Vec<String>,
  pub all: Vec<String>,
}

pub(crate) fn analyze_query(input: &str) -> QueryTerms {
  let words = split_words(input);
  let mut all: Vec<String> = Vec::new();
  for word in &words {
    let stemmed = stem(word);
    if !all.contains(&stemmed) {
      all.push(stemmed);
    }
  }
  let mut direct: Vec<String> = Vec::new();
  for term in analyze(input) {
    if !direct.contains(&term) {
      direct.push(term);
    }
  }
  QueryTerms { direct, all }
}

fn has_vowel(text: &str) -> bool {
  text.bytes().any(|byte| matches!(byte, b'a' | b'e' | b'i' | b'o' | b'u' | b'y'))
}

fn strip_plural(word: &str) -> String {
  if word.len() > 4 && word.ends_with("ies") {
    return format!("{}y", &word[..word.len() - 3]);
  }
  if ["sses", "xes", "zes", "ches", "shes"].iter().any(|suffix| word.ends_with(suffix)) {
    return word[..word.len() - 2].to_owned();
  }
  if word.ends_with('s') && !["ss", "us", "is"].iter().any(|suffix| word.ends_with(suffix)) {
    return word[..word.len() - 1].to_owned();
  }
  word.to_owned()
}

fn strip_derivational(word: &str) -> Option<String> {
  ["ing", "ed", "er"].iter().find_map(|suffix| {
    let stem = word.strip_suffix(suffix)?;
    (stem.len() >= 3 && has_vowel(stem)).then(|| stem.to_owned())
  })
}

fn collapse_doubled_consonant(mut word: String) -> String {
  let bytes = word.as_bytes();
  if let [.., a, b] = bytes
    && a == b
    && !matches!(a, b'a' | b'e' | b'i' | b'o' | b'u' | b'l' | b's' | b'z')
  {
    word.pop();
  }
  word
}

pub(crate) fn stem(word: &str) -> String {
  if word.len() <= 3 || !word.is_ascii() || word.bytes().any(|byte| byte.is_ascii_digit()) {
    return word.to_owned();
  }
  let mut stem = strip_plural(word);
  let mut stripped = false;
  for _ in 0..2 {
    let Some(next) = strip_derivational(&stem) else { break };
    stem = next;
    stripped = true;
  }
  if stripped {
    stem = collapse_doubled_consonant(stem);
  }
  if stem.len() > 3 && stem.ends_with('e') {
    stem.pop();
  }
  stem
}

#[cfg(test)]
mod tests {
  use super::*;

  fn words(input: &str) -> Vec<String> {
    split_words(input)
  }

  #[test]
  fn should_split_mixed_case_acronyms_and_digits() {
    assert_eq!(words("parseHTTPResponse2"), ["parse", "http", "response", "2"]);
  }

  #[test]
  fn should_split_snake_case() {
    assert_eq!(words("to_cents"), ["to", "cents"]);
  }

  #[test]
  fn should_split_leading_acronym_pascal_case() {
    assert_eq!(words("XMLHttpRequest"), ["xml", "http", "request"]);
  }

  #[test]
  fn should_split_paths_dotted_and_kebab_forms() {
    assert_eq!(words("crates/atlas-core/src/user_service.rs"), ["crates", "atlas", "core", "src", "user", "service", "rs"]);
    assert_eq!(words("a.b::c-d"), ["a", "b", "c", "d"]);
  }

  #[test]
  fn should_drop_stopwords_and_single_letters_but_keep_a_sole_get() {
    assert_eq!(analyze("getUser"), ["user"]);
    assert_eq!(analyze("get"), ["get"]);
    assert_eq!(analyze("self.this"), ["self", "this"]);
    assert_eq!(analyze("src/user_service.rs"), ["user", "servic"]);
  }

  #[test]
  fn should_stem_inflections_to_a_shared_form() {
    for variant in ["parse", "parses", "parsed", "parsing", "parser", "parsers"] {
      assert_eq!(stem(variant), "pars", "{variant}");
    }
    assert_eq!(stem("running"), "run");
    assert_eq!(stem("queries"), "query");
    assert_eq!(stem("boxes"), "box");
    assert_eq!(stem("orders"), stem("ordering"));
  }

  #[test]
  fn should_not_strip_protected_endings_or_short_words() {
    assert_eq!(stem("status"), "status");
    assert_eq!(stem("address"), "address");
    assert_eq!(stem("user"), "user");
    assert_eq!(stem("users"), "user");
    assert_eq!(stem("string"), "string");
    assert_eq!(stem("utf8s"), "utf8s");
  }

  #[test]
  fn should_document_stemmer_limits() {
    assert_ne!(stem("children"), stem("child"));
    assert_ne!(stem("data"), stem("datum"));
    assert_eq!(stem("rate"), stem("rat"));
    assert_eq!(stem("reader"), stem("read"));
  }

  #[test]
  fn should_analyze_query_keeping_stopwords_only_in_the_full_list() {
    let query = analyze_query("get the users");
    assert_eq!(query.direct, ["user"]);
    assert_eq!(query.all, ["get", "the", "user"]);
  }
}
