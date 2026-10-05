use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub const MAX_CODE_SPANS: usize = 8;
pub const MAX_PROSE_PHRASES: usize = 4;
const MIN_PHRASE_CHARS: usize = 3;
const MIN_PROSE_WORDS: usize = 2;
const MAX_PROSE_WORDS: usize = 4;
const MIN_WORD_CHARS: usize = 2;
const FENCE: &str = "```";
const STOPWORDS: [&str; 152] = [
  "a", "about", "above", "added", "adds", "add", "after", "again", "all", "already", "also", "an", "and", "another", "any", "are", "as", "at", "be", "because", "been", "before",
  "being", "below", "both", "but", "by", "can", "cannot", "could", "did", "do", "does", "done", "down", "each", "either", "else", "even", "every", "exactly", "for", "from", "get",
  "gets", "got", "had", "has", "have", "here", "how", "if", "in", "instead", "into", "is", "it", "its", "just", "keep", "keeps", "kept", "least", "less", "let", "lets", "like",
  "make", "makes", "made", "may", "me", "might", "more", "most", "must", "my", "need", "needs", "needed", "neither", "new", "no", "nor", "not", "now", "of", "off", "on", "one",
  "only", "onto", "or", "other", "our", "out", "over", "per", "please", "same", "see", "set", "sets", "shall", "should", "so", "some", "still", "such", "than", "that", "the",
  "their", "them", "then", "there", "these", "they", "this", "those", "to", "today", "too", "two", "under", "up", "us", "use", "used", "uses", "using", "very", "via", "was", "we",
  "were", "what", "when", "where", "whether", "which", "while", "who", "why", "will", "with", "within", "without", "would", "you", "your", "yet",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhraseSource {
  CodeSpan,
  Prose,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phrase {
  pub text: String,
  pub source: PhraseSource,
}

pub fn significant_phrases(prompt: &str) -> Vec<Phrase> {
  let text = without_fenced_blocks(prompt);
  let (code_spans, prose) = split_code_spans(&text);
  let mut seen = BTreeSet::new();
  let code = code_spans
    .into_iter()
    .map(|span| span.trim().to_owned())
    .filter(|span| span.chars().count() >= MIN_PHRASE_CHARS)
    .filter(|span| seen.insert(span.to_lowercase()))
    .take(MAX_CODE_SPANS)
    .map(|text| Phrase {
      text,
      source: PhraseSource::CodeSpan,
    });
  let mut phrases: Vec<Phrase> = code.collect();
  let prose = prose_chunks(&prose)
    .into_iter()
    .filter(|chunk| seen.insert(chunk.to_lowercase()))
    .take(MAX_PROSE_PHRASES)
    .map(|text| Phrase {
      text,
      source: PhraseSource::Prose,
    });
  phrases.extend(prose);
  phrases
}

fn without_fenced_blocks(prompt: &str) -> String {
  let mut kept = Vec::new();
  let mut fenced = false;
  for line in prompt.lines() {
    if line.trim_start().starts_with(FENCE) {
      fenced = !fenced;
      continue;
    }
    if !fenced {
      kept.push(line);
    }
  }
  kept.join("\n")
}

fn split_code_spans(text: &str) -> (Vec<String>, Vec<String>) {
  let segments: Vec<&str> = text.split('`').collect();
  let closed = segments.len() % 2 == 1;
  let mut code = Vec::new();
  let mut prose = Vec::new();
  for (index, segment) in segments.iter().enumerate() {
    let unclosed_tail = !closed && index == segments.len() - 1;
    match index % 2 == 1 && !unclosed_tail {
      true => code.push((*segment).to_owned()),
      false => prose.push((*segment).to_owned()),
    }
  }
  (code, prose)
}

fn prose_chunks(segments: &[String]) -> Vec<String> {
  let mut chunks = Vec::new();
  for segment in segments {
    let separated: String = segment
      .chars()
      .map(|c| if c.is_alphanumeric() || c == '-' || c.is_whitespace() { c } else { '|' })
      .collect();
    for fragment in separated.split('|') {
      let mut run: Vec<&str> = Vec::new();
      for word in fragment.split_whitespace().map(|word| word.trim_matches('-')) {
        match is_content_word(word) {
          true => run.push(word),
          false => flush(&mut run, &mut chunks),
        }
      }
      flush(&mut run, &mut chunks);
    }
  }
  chunks
}

fn is_content_word(word: &str) -> bool {
  word.chars().count() >= MIN_WORD_CHARS && word.chars().any(char::is_alphabetic) && !STOPWORDS.contains(&word.to_lowercase().as_str())
}

fn flush(run: &mut Vec<&str>, chunks: &mut Vec<String>) {
  if run.len() >= MIN_PROSE_WORDS {
    let tail = &run[run.len().saturating_sub(MAX_PROSE_WORDS)..];
    chunks.push(tail.join(" "));
  }
  run.clear();
}

#[cfg(test)]
mod tests {
  use super::*;

  fn texts(phrases: &[Phrase], source: PhraseSource) -> Vec<&str> {
    phrases.iter().filter(|phrase| phrase.source == source).map(|phrase| phrase.text.as_str()).collect()
  }

  #[test]
  fn should_take_code_spans_first_then_stopword_delimited_prose_chunks() {
    let prompt = "In acme-api, add a nullable `deletedAt` timestamp to the `User` entity, stored in a database column named `deleted_at` \
(use TypeORM's soft-delete column decorator). Make sure `UsersService.findOne` returns it.";
    let phrases = significant_phrases(prompt);
    assert_eq!(texts(&phrases, PhraseSource::CodeSpan), vec!["deletedAt", "User", "deleted_at", "UsersService.findOne"]);
    assert_eq!(texts(&phrases, PhraseSource::Prose), vec!["database column named", "soft-delete column decorator"]);
  }

  #[test]
  fn should_skip_fenced_code_blocks_short_spans_and_duplicates() {
    let prompt = "Bug: `Router` and `router` and `ab` break.\n```ts\nconst `inside` = 1;\n```\nNested routers mounted across files lose prefixes.";
    let phrases = significant_phrases(prompt);
    assert_eq!(
      phrases,
      vec![
        Phrase {
          text: "Router".to_owned(),
          source: PhraseSource::CodeSpan
        },
        Phrase {
          text: "across files lose prefixes".to_owned(),
          source: PhraseSource::Prose
        },
      ]
    );
  }

  #[test]
  fn should_keep_the_last_words_of_a_long_chunk_and_cap_each_source() {
    let spans: String = (0..10).map(|index| format!("`span{index}` ")).collect();
    let prose = "alpha beta gamma delta epsilon. one-two three. aa bb. cc dd. ee ff. gg hh.";
    let phrases = significant_phrases(&format!("{spans}{prose}"));
    assert_eq!(texts(&phrases, PhraseSource::CodeSpan).len(), MAX_CODE_SPANS);
    assert_eq!(texts(&phrases, PhraseSource::Prose), vec!["beta gamma delta epsilon", "one-two three", "aa bb", "cc dd"]);
  }

  #[test]
  fn should_treat_an_unclosed_backtick_as_prose() {
    let phrases = significant_phrases("Closed `FreshnessReport` then an unclosed `typed degraded cause");
    assert_eq!(texts(&phrases, PhraseSource::CodeSpan), vec!["FreshnessReport"]);
    assert_eq!(texts(&phrases, PhraseSource::Prose), vec!["typed degraded cause"]);
  }
}
