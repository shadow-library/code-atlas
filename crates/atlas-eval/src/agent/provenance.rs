use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

const MIN_TERM_CHARS: usize = 3;
const MIN_WORD_CHARS: usize = 4;
const MAX_QUOTED_CHARS: usize = 80;
const MOST_SELECTIVE_TERMS: usize = 5;
const STOP_WORDS: [&str; 34] = [
  "async",
  "await",
  "class",
  "const",
  "crates",
  "enum",
  "export",
  "false",
  "file",
  "files",
  "from",
  "function",
  "impl",
  "import",
  "index",
  "interface",
  "main",
  "name",
  "none",
  "null",
  "path",
  "return",
  "self",
  "some",
  "static",
  "struct",
  "test",
  "tests",
  "this",
  "trait",
  "true",
  "type",
  "types",
  "with",
];

fn word_char(c: char) -> bool {
  c.is_ascii_alphanumeric() || c == '_'
}

fn joined_tokens(text: &str) -> Vec<&str> {
  let chars: Vec<(usize, char)> = text.char_indices().collect();
  let is_word = |index: usize| chars.get(index).is_some_and(|(_, c)| word_char(*c));
  let joins = |index: usize| {
    let (Some((_, c)), true) = (chars.get(index), index > 0 && is_word(index - 1)) else {
      return 0;
    };
    match c {
      ':' if chars.get(index + 1).is_some_and(|(_, next)| *next == ':') && is_word(index + 2) => 2,
      '-' | '.' | '/' if is_word(index + 1) => 1,
      _ => 0,
    }
  };
  let mut tokens = Vec::new();
  let mut index = 0;
  while index < chars.len() {
    if !is_word(index) {
      index += 1;
      continue;
    }
    let start = index;
    while is_word(index) || joins(index) > 0 {
      index += joins(index).max(1);
    }
    let end = chars.get(index).map_or(text.len(), |(offset, _)| *offset);
    tokens.push(&text[chars[start].0..end]);
  }
  tokens
}

fn has_inner_capital(word: &str) -> bool {
  let mut lower_seen = false;
  word.chars().any(|c| {
    let capital_after_lower = lower_seen && c.is_ascii_uppercase();
    lower_seen |= c.is_ascii_lowercase();
    capital_after_lower
  })
}

fn identifier_shaped(token: &str) -> bool {
  let trimmed = token.trim_matches('_');
  token.chars().any(|c| c.is_ascii_alphabetic()) && (trimmed.contains('_') || trimmed.contains(['-', '/']) || token.contains("::") || has_inner_capital(token))
}

fn quoted_strings(text: &str) -> Vec<&str> {
  text
    .lines()
    .flat_map(|line| line.split('"').skip(1).step_by(2).collect::<Vec<_>>())
    .filter(|quoted| quoted.chars().count() <= MAX_QUOTED_CHARS)
    .collect()
}

pub fn candidate_terms(text: &str) -> Vec<String> {
  let mut terms: Vec<String> = Vec::new();
  let mut add = |term: &str| {
    let term = term.trim();
    if term.chars().count() >= MIN_TERM_CHARS && !terms.iter().any(|known| known == term) {
      terms.push(term.to_owned());
    }
  };
  for (index, piece) in text.split('`').enumerate() {
    let code = index % 2 == 1;
    if code && !piece.contains('\n') {
      add(piece);
    }
    for token in joined_tokens(piece) {
      if identifier_shaped(token) || code && token.chars().count() >= MIN_WORD_CHARS {
        add(token);
      }
      for part in token.split(['.', '/', ':']) {
        if identifier_shaped(part) || code && part.chars().count() >= MIN_WORD_CHARS {
          add(part);
        }
      }
    }
    if !code {
      quoted_strings(piece).into_iter().for_each(&mut add);
    }
  }
  terms
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermReach {
  pub term: String,
  pub files: usize,
  pub must_change: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReach {
  pub terms: usize,
  pub reaching: usize,
  pub most_selective: Vec<TermReach>,
}

pub fn source_reach(text: &str, contents: &BTreeMap<String, String>, must_change: &[String]) -> SourceReach {
  let terms = candidate_terms(text);
  let mut reaching: Vec<TermReach> = terms
    .iter()
    .map(|term| {
      let files: Vec<&String> = contents.iter().filter(|(_, content)| content.contains(term.as_str())).map(|(path, _)| path).collect();
      TermReach {
        term: term.clone(),
        files: files.len(),
        must_change: files.iter().filter(|path| must_change.contains(path)).count(),
      }
    })
    .filter(|reach| reach.must_change > 0)
    .collect();
  reaching.sort_by(|a, b| (a.files, std::cmp::Reverse(a.must_change), &a.term).cmp(&(b.files, std::cmp::Reverse(b.must_change), &b.term)));
  SourceReach {
    terms: terms.len(),
    reaching: reaching.len(),
    most_selective: reaching.into_iter().take(MOST_SELECTIVE_TERMS).collect(),
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskReach {
  pub prompt: SourceReach,
  pub instructions: Option<SourceReach>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wording {
  tokens: BTreeSet<String>,
  parts: BTreeSet<String>,
}

impl Wording {
  pub fn of(text: &str) -> Self {
    let kept = |word: &str| word.chars().count() >= MIN_WORD_CHARS && !STOP_WORDS.contains(&word);
    let tokens: Vec<&str> = text.split(|c: char| !word_char(c)).filter(|token| !token.is_empty()).collect();
    Self {
      tokens: tokens.iter().map(|token| token.to_ascii_lowercase()).filter(|token| kept(token)).collect(),
      parts: tokens.iter().flat_map(|token| word_parts(token)).filter(|part| kept(part)).collect(),
    }
  }

  fn shared(ours: &BTreeSet<String>, theirs: &BTreeSet<String>) -> Vec<String> {
    ours.intersection(theirs).cloned().collect()
  }
}

fn word_parts(token: &str) -> Vec<String> {
  let mut parts = vec![String::new()];
  let mut previous_lower = false;
  for c in token.chars() {
    if c == '_' || previous_lower && c.is_ascii_uppercase() {
      parts.push(String::new());
    }
    if c != '_' {
      parts.last_mut().into_iter().for_each(|part| part.push(c.to_ascii_lowercase()));
    }
    previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
  }
  parts
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntrySource {
  PromptIdentifier,
  InstructionsIdentifier,
  PromptWords,
  InstructionsWords,
  Neither,
}

impl EntrySource {
  pub fn label(self) -> &'static str {
    match self {
      Self::PromptIdentifier => "an identifier or word written in the prompt",
      Self::InstructionsIdentifier => "an identifier or word written in the injected instructions",
      Self::PromptWords => "parts of words in the prompt",
      Self::InstructionsWords => "parts of words in the injected instructions",
      Self::Neither => "neither",
    }
  }
}

pub fn provenance(query: &str, prompt: &Wording, instructions: &Wording) -> (EntrySource, Vec<String>) {
  let query = Wording::of(query);
  let candidates = [
    (EntrySource::PromptIdentifier, Wording::shared(&query.tokens, &prompt.tokens)),
    (EntrySource::InstructionsIdentifier, Wording::shared(&query.tokens, &instructions.tokens)),
    (EntrySource::PromptWords, Wording::shared(&query.parts, &prompt.parts)),
    (EntrySource::InstructionsWords, Wording::shared(&query.parts, &instructions.parts)),
  ];
  candidates.into_iter().find(|(_, shared)| !shared.is_empty()).unwrap_or((EntrySource::Neither, Vec::new()))
}

#[cfg(test)]
mod tests {
  use super::*;

  const PROMPT: &str = "`serveStatic({ precompressed: true })` only serves a variant when the client lists the coding.\n\
`FreshnessReport::Degraded` should carry `cause: DegradedCause`, exported from `atlas_engine`.\n\
Routers mounted directly on the app still link. The phrase \"project configuration changed\" is matched in crates/atlas-cli/src.\n\
```ts\nexport const apiRouter = Router();\n```\n";

  #[test]
  fn should_extract_code_spans_identifiers_paths_and_quoted_phrases() {
    assert_eq!(
      candidate_terms(PROMPT),
      vec![
        "serveStatic({ precompressed: true })",
        "serveStatic",
        "precompressed",
        "true",
        "FreshnessReport::Degraded",
        "FreshnessReport",
        "Degraded",
        "cause: DegradedCause",
        "cause",
        "DegradedCause",
        "atlas_engine",
        "crates/atlas-cli/src",
        "atlas-cli",
        "project configuration changed",
        "export",
        "const",
        "apiRouter",
        "Router"
      ]
    );
    assert_eq!(candidate_terms("Keep the change minimal and do not touch the other packages."), Vec::<String>::new());
  }

  fn contents(files: &[(&str, &str)]) -> BTreeMap<String, String> {
    files.iter().map(|(path, content)| ((*path).to_owned(), (*content).to_owned())).collect()
  }

  #[test]
  fn should_count_the_files_a_term_returns_and_the_must_change_files_among_them() {
    let tree = contents(&[
      ("src/report.rs", "pub enum FreshnessReport { Degraded { cause: DegradedCause } }"),
      ("src/watch.rs", "match report { FreshnessReport::Degraded { .. } => reload() }"),
      ("src/lib.rs", "pub use atlas_engine::FreshnessReport;"),
      ("README.md", "serve static files"),
    ]);
    let must_change = ["src/report.rs".to_owned(), "src/watch.rs".to_owned()];
    let reach = source_reach(PROMPT, &tree, &must_change);
    assert_eq!((reach.terms, reach.reaching), (18, 6));
    assert_eq!(
      reach.most_selective,
      vec![
        TermReach {
          term: "DegradedCause".to_owned(),
          files: 1,
          must_change: 1
        },
        TermReach {
          term: "FreshnessReport::Degraded".to_owned(),
          files: 1,
          must_change: 1
        },
        TermReach {
          term: "cause".to_owned(),
          files: 1,
          must_change: 1
        },
        TermReach {
          term: "cause: DegradedCause".to_owned(),
          files: 1,
          must_change: 1
        },
        TermReach {
          term: "Degraded".to_owned(),
          files: 2,
          must_change: 2
        }
      ]
    );
    assert_eq!(source_reach("no code here", &tree, &must_change), SourceReach::default());
  }

  #[test]
  fn should_attribute_a_query_to_the_prompt_before_the_instructions_and_whole_words_before_parts() {
    let prompt = Wording::of(PROMPT);
    let instructions = Wording::of("Cross-file mounts are edges with attrs `mount_prefix`, `mounted_on`. Use the lambda handler.");
    assert_eq!(
      provenance("FreshnessReport|reason", &prompt, &instructions),
      (EntrySource::PromptIdentifier, vec!["freshnessreport".to_owned()])
    );
    assert_eq!(
      provenance("mounted_on|mount_prefix", &prompt, &instructions),
      (EntrySource::InstructionsIdentifier, vec!["mount_prefix".to_owned(), "mounted_on".to_owned()])
    );
    assert_eq!(
      provenance("src/middleware/serve-static/index.ts", &prompt, &instructions),
      (EntrySource::PromptWords, vec!["serve".to_owned()])
    );
    assert_eq!(
      provenance("isLambdaEvent", &prompt, &instructions),
      (EntrySource::InstructionsWords, vec!["lambda".to_owned()])
    );
    assert_eq!(provenance("class|export const", &prompt, &instructions), (EntrySource::Neither, Vec::new()));
  }
}
