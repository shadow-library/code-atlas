use std::collections::{BTreeSet, HashMap, HashSet};

use atlas_similarity::analyze_terms;

use super::markdown::RawSection;

const K1: f64 = 1.2;
const B: f64 = 0.75;
const HEADING_WEIGHT: usize = 3;
pub(crate) const SNIPPET_CHARS: usize = 240;

pub(crate) struct SectionTerms {
  counts: HashMap<String, usize>,
  heading: HashSet<String>,
  length: usize,
}

impl SectionTerms {
  pub fn of(section: &RawSection) -> Self {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut length = 0;
    for term in analyze_terms(section.title()) {
      *counts.entry(term).or_default() += HEADING_WEIGHT;
      length += HEADING_WEIGHT;
    }
    for term in analyze_terms(&section.body) {
      *counts.entry(term).or_default() += 1;
      length += 1;
    }
    Self {
      counts,
      heading: analyze_terms(&section.heading_path()).into_iter().collect(),
      length,
    }
  }

  pub fn admits_fused(&self, terms: &BTreeSet<String>) -> bool {
    let all = !terms.is_empty() && terms.iter().all(|term| self.counts.contains_key(term));
    all || terms.iter().any(|term| self.heading.contains(term))
  }
}

pub(crate) fn query_terms(query: &str) -> BTreeSet<String> {
  analyze_terms(query).into_iter().collect()
}

pub(crate) fn bm25(sections: &[&SectionTerms], terms: &BTreeSet<String>) -> Vec<f64> {
  let total = sections.len() as f64;
  if sections.is_empty() || terms.is_empty() {
    return vec![0.0; sections.len()];
  }
  let average = sections.iter().map(|section| section.length).sum::<usize>() as f64 / total;
  let idf: Vec<(&String, f64)> = terms
    .iter()
    .map(|term| {
      let holding = sections.iter().filter(|section| section.counts.contains_key(term)).count() as f64;
      (term, ((total - holding + 0.5) / (holding + 0.5) + 1.0).ln())
    })
    .collect();
  sections
    .iter()
    .map(|section| {
      let norm = K1 * (1.0 - B + B * section.length as f64 / average.max(1.0));
      idf
        .iter()
        .map(|(term, idf)| {
          let frequency = section.counts.get(*term).copied().unwrap_or(0) as f64;
          match frequency > 0.0 {
            true => idf * frequency * (K1 + 1.0) / (frequency + norm),
            false => 0.0,
          }
        })
        .sum()
    })
    .collect()
}

pub(crate) fn clip(text: &str, limit: usize) -> String {
  match text.char_indices().nth(limit) {
    Some((end, _)) => format!("{}…", text[..end].trim_end()),
    None => text.to_owned(),
  }
}

fn collapsed(lines: &[&str]) -> String {
  lines.iter().flat_map(|line| line.split_whitespace()).collect::<Vec<_>>().join(" ")
}

pub(crate) fn snippet(body: &str, terms: &BTreeSet<String>, limit: usize) -> String {
  let lines: Vec<&str> = body.lines().filter(|line| !line.trim().is_empty()).collect();
  let start = lines.iter().position(|line| analyze_terms(line).iter().any(|term| terms.contains(term))).unwrap_or(0);
  clip(&collapsed(&lines[start.min(lines.len())..]), limit)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::queries::docs::markdown::parse;

  #[test]
  fn should_rank_sections_by_bm25_with_heading_terms_weighted() {
    let doc = parse(
      "# Retries\nThe client waits between attempts.\n# Paging\nPages hold retries counters in a footnote and other words to pad the body out.\n# Auth\nTokens.\n",
      "a.md",
    );
    let sections: Vec<SectionTerms> = doc.sections.iter().map(SectionTerms::of).collect();
    let scores = bm25(&sections.iter().collect::<Vec<_>>(), &query_terms("retries"));
    assert!(scores[0] > scores[1] && scores[1] > 0.0, "{scores:?}");
    assert_eq!(scores[2], 0.0);
  }

  #[test]
  fn should_fuse_a_section_only_when_it_holds_every_query_term_or_its_heading_holds_one() {
    let doc = parse("# Retries\nThe client waits.\n# Paging\nPages hold cursors and retries.\n", "a.md");
    let retries = SectionTerms::of(&doc.sections[0]);
    let paging = SectionTerms::of(&doc.sections[1]);
    assert!(retries.admits_fused(&query_terms("retries budget")));
    assert!(paging.admits_fused(&query_terms("retries cursors")));
    assert!(!paging.admits_fused(&query_terms("retries budget")));
    assert!(!paging.admits_fused(&query_terms("the")));
  }

  #[test]
  fn should_start_the_snippet_at_the_first_line_with_a_query_term_and_clip_it() {
    let body = "Intro.\n\nThe   backoff doubles\nafter each attempt.";
    assert_eq!(snippet(body, &query_terms("backoff"), 240), "The backoff doubles after each attempt.");
    assert_eq!(snippet(body, &query_terms("nothing"), 10), "Intro. The…");
    assert_eq!(clip("ééé", 2), "éé…");
  }
}
