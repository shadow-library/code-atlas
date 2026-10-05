use std::collections::BTreeSet;

use atlas_core::{AnalysisInput, ContentHash, EntityKind, FileFacts, FileRef, LanguageAnalyzer, LanguageFamily, RelPath, RepoName};
use atlas_lang_rust::RustAnalyzer;
use atlas_lang_ts::TsAnalyzer;
use serde::{Deserialize, Serialize};

use super::atlas_cli::SimilarMatch;
use super::diff::Diff;

pub const DUPLICATION_SCORE_THRESHOLD: f64 = 0.8;
const MAX_JACCARD: f64 = 1.0;
const ANALYSIS_REPO: &str = "tree";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolTarget {
  pub raw: String,
  segments: Vec<String>,
}

impl SymbolTarget {
  pub fn parse(raw: &str) -> Self {
    let segments = split_symbol(raw)
      .into_iter()
      .filter(|segment| segment != "crate" && segment != "self" && segment != "super")
      .collect();
    Self { raw: raw.to_owned(), segments }
  }

  pub fn name(&self) -> &str {
    self.segments.last().map_or(self.raw.as_str(), String::as_str)
  }

  pub fn matches_symbol(&self, candidate: &str) -> bool {
    let candidate = split_symbol(candidate);
    let wanted = &self.segments[self.segments.len().saturating_sub(2)..];
    !wanted.is_empty() && candidate.ends_with(wanted)
  }
}

fn split_symbol(raw: &str) -> Vec<String> {
  let without_kind = raw.split('#').next().unwrap_or(raw);
  without_kind
    .split([':', '.', '/'])
    .map(|segment| segment.split('~').next().unwrap_or(segment))
    .filter(|segment| !segment.is_empty())
    .map(str::to_owned)
    .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReuseReference {
  pub target: String,
  pub referenced: bool,
  pub locations: Vec<String>,
}

pub fn reuse_references(diff: &Diff, targets: &[String]) -> Vec<ReuseReference> {
  targets
    .iter()
    .map(|raw| {
      let target = SymbolTarget::parse(raw);
      let locations: Vec<String> = diff
        .added_lines()
        .filter(|(_, line)| references_identifier(&line.text, target.name()))
        .map(|(file, line)| format!("{}:{}", file.path, line.line))
        .collect();
      ReuseReference {
        target: raw.clone(),
        referenced: !locations.is_empty(),
        locations,
      }
    })
    .collect()
}

pub fn references_identifier(line: &str, name: &str) -> bool {
  let code = line.split("//").next().unwrap_or(line);
  if name.is_empty() || defines(code, name) {
    return false;
  }
  identifier_positions(code, name).next().is_some()
}

fn identifier_positions<'a>(line: &'a str, name: &'a str) -> impl Iterator<Item = usize> + 'a {
  line.match_indices(name).map(|(index, _)| index).filter(move |index| {
    let before = line[..*index].chars().next_back();
    let after = line[index + name.len()..].chars().next();
    !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char)
  })
}

fn defines(line: &str, name: &str) -> bool {
  let trimmed = line.trim_start();
  let declarations = ["fn ", "function ", "const ", "let ", "var ", "class ", "struct ", "enum ", "trait ", "type ", "interface "];
  let stripped = strip_modifiers(trimmed);
  if declarations.iter().any(|keyword| {
    stripped
      .strip_prefix(keyword)
      .is_some_and(|rest| rest.trim_start().starts_with(name) && rest.trim_start()[name.len()..].chars().next().is_none_or(|c| !is_identifier_char(c)))
  }) {
    return true;
  }
  stripped.starts_with(name) && stripped[name.len()..].starts_with('(') && trimmed.trim_end().ends_with('{')
}

fn strip_modifiers(line: &str) -> &str {
  let modifiers = [
    "pub(crate) ",
    "pub ",
    "export ",
    "default ",
    "async ",
    "static ",
    "private ",
    "public ",
    "protected ",
    "readonly ",
    "unsafe ",
  ];
  let mut rest = line;
  while let Some(stripped) = modifiers.iter().find_map(|modifier| rest.strip_prefix(modifier)) {
    rest = stripped;
  }
  rest
}

fn is_identifier_char(c: char) -> bool {
  c.is_alphanumeric() || c == '_' || c == '$'
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewFunction {
  pub path: String,
  pub name: String,
  pub item_path: String,
  pub start_line: u32,
  pub end_line: u32,
}

pub fn analyze_source(path: &str, source: &str) -> Option<FileFacts> {
  let repo = RepoName::new(ANALYSIS_REPO).ok()?;
  let rel = RelPath::new(path).ok()?;
  let file = FileRef::detect(repo, rel, ContentHash::of(source.as_bytes())).ok()?;
  let input = AnalysisInput { file: &file, source };
  let result = match file.language.family() {
    LanguageFamily::Rust => RustAnalyzer::new().analyze(&input),
    LanguageFamily::Ts => TsAnalyzer.analyze(&input),
    _ => return None,
  };
  match result {
    Ok(facts) => Some(facts),
    Err(error) => {
      tracing::warn!(path, %error, "cannot analyze a changed file for new functions");
      None
    }
  }
}

pub fn new_functions(path: &str, base: Option<&FileFacts>, post: &FileFacts) -> Vec<NewFunction> {
  let existing: BTreeSet<String> = base.map(|facts| function_identities(facts).collect()).unwrap_or_default();
  post
    .symbols
    .iter()
    .filter(|symbol| matches!(symbol.kind, EntityKind::Function | EntityKind::Method))
    .filter(|symbol| !existing.contains(&symbol.item_path.join("::")))
    .map(|symbol| NewFunction {
      path: path.to_owned(),
      name: symbol.name.clone(),
      item_path: symbol.item_path.join("::"),
      start_line: symbol.span.start_line,
      end_line: symbol.span.end_line,
    })
    .collect()
}

fn function_identities(facts: &FileFacts) -> impl Iterator<Item = String> + '_ {
  facts
    .symbols
    .iter()
    .filter(|symbol| matches!(symbol.kind, EntityKind::Function | EntityKind::Method))
    .map(|symbol| symbol.item_path.join("::"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetRole {
  ReuseTarget,
  ForbiddenDuplicate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DuplicationEvidence {
  StructuralJaccard {
    jaccard: f64,
  },
  InferredEquivalent {
    verdict: String,
  },
  // Run files written before the structural/lexical split used this tag for either a Jaccard or a BM25 score.
  #[serde(rename = "structural")]
  UnclassifiedScore {
    score: f64,
  },
}

impl DuplicationEvidence {
  pub fn is_conclusive(&self) -> bool {
    !matches!(self, Self::UnclassifiedScore { .. })
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DuplicationFinding {
  pub new_function: String,
  pub location: String,
  pub matched_symbol: String,
  pub target: String,
  pub role: TargetRole,
  pub evidence: DuplicationEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LexicalCandidate {
  pub new_function: String,
  pub location: String,
  pub matched_symbol: String,
  pub target: String,
  pub role: TargetRole,
  pub score: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DuplicationMatches {
  pub findings: Vec<DuplicationFinding>,
  pub lexical_candidates: Vec<LexicalCandidate>,
}

enum MatchEvidence {
  Duplication(DuplicationEvidence),
  Lexical(f64),
}

pub fn duplication_findings(function: &NewFunction, matches: &[SimilarMatch], reuse_targets: &[String], forbidden: &[String]) -> DuplicationMatches {
  let roles = reuse_targets
    .iter()
    .map(|target| (target, TargetRole::ReuseTarget))
    .chain(forbidden.iter().map(|target| (target, TargetRole::ForbiddenDuplicate)));
  let roles: Vec<(SymbolTarget, TargetRole)> = roles.map(|(target, role)| (SymbolTarget::parse(target), role)).collect();
  let location = format!("{}:{}-{}", function.path, function.start_line, function.end_line);
  let mut scored = DuplicationMatches::default();
  for candidate in matches.iter().filter(|candidate| !is_self_match(function, candidate)) {
    let Some(evidence) = evidence(candidate) else {
      continue;
    };
    for (target, role) in roles.iter().filter(|(target, _)| target.matches_symbol(&candidate.symbol)) {
      match &evidence {
        MatchEvidence::Duplication(evidence) => scored.findings.push(DuplicationFinding {
          new_function: function.item_path.clone(),
          location: location.clone(),
          matched_symbol: candidate.symbol.clone(),
          target: target.raw.clone(),
          role: *role,
          evidence: evidence.clone(),
        }),
        MatchEvidence::Lexical(score) => scored.lexical_candidates.push(LexicalCandidate {
          new_function: function.item_path.clone(),
          location: location.clone(),
          matched_symbol: candidate.symbol.clone(),
          target: target.raw.clone(),
          role: *role,
          score: *score,
        }),
      }
    }
  }
  scored
}

fn evidence(candidate: &SimilarMatch) -> Option<MatchEvidence> {
  if candidate.inferred_equivalent() {
    return candidate
      .verdict
      .clone()
      .map(|verdict| MatchEvidence::Duplication(DuplicationEvidence::InferredEquivalent { verdict }));
  }
  if let Some(jaccard) = candidate.structural.filter(|jaccard| *jaccard >= DUPLICATION_SCORE_THRESHOLD) {
    return Some(MatchEvidence::Duplication(DuplicationEvidence::StructuralJaccard { jaccard }));
  }
  candidate.lexical.map(MatchEvidence::Lexical)
}

pub fn reclassify_legacy(findings: Vec<DuplicationFinding>) -> DuplicationMatches {
  let mut reclassified = DuplicationMatches::default();
  for finding in findings {
    match finding.evidence {
      DuplicationEvidence::UnclassifiedScore { score } if score > MAX_JACCARD => reclassified.lexical_candidates.push(LexicalCandidate {
        new_function: finding.new_function,
        location: finding.location,
        matched_symbol: finding.matched_symbol,
        target: finding.target,
        role: finding.role,
        score,
      }),
      _ => reclassified.findings.push(finding),
    }
  }
  reclassified
}

fn is_self_match(function: &NewFunction, candidate: &SimilarMatch) -> bool {
  let in_same_file = candidate.location.as_deref().is_some_and(|location| location.contains(&function.path));
  in_same_file && SymbolTarget::parse(&function.item_path).matches_symbol(&candidate.symbol)
}

pub fn same_name_as_targets(functions: &[NewFunction], targets: &[String]) -> Vec<String> {
  let names: Vec<SymbolTarget> = targets.iter().map(|target| SymbolTarget::parse(target)).collect();
  functions
    .iter()
    .filter(|function| names.iter().any(|target| target.name() == function.name))
    .map(|function| format!("{}:{} {}", function.path, function.start_line, function.item_path))
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_match_symbol_keys_by_their_trailing_segments() {
    let rust = SymbolTarget::parse("crate::money::to_cents");
    assert!(rust.matches_symbol("rust:acme-billing:acme-billing:money::to_cents#function"));
    assert!(!rust.matches_symbol("rust:globex:globex:convert::to_cents#function"));
    let method = SymbolTarget::parse("UsersService.findOne");
    assert!(method.matches_symbol("ts:acme-api:acme-api:src/users/users.service::UsersService::findOne#method"));
    assert!(!method.matches_symbol("ts:acme-api:acme-api:src/orders/orders.service::OrdersService::findOne#method"));
    assert!(SymbolTarget::parse("formatMoney").matches_symbol("ts:web:web:src/common/money::formatMoney#function~2"));
  }

  #[test]
  fn should_detect_references_but_not_definitions_or_comments() {
    assert!(references_identifier("  const total = to_cents(amount);", "to_cents"));
    assert!(references_identifier("import { formatMoney } from '../common/money';", "formatMoney"));
    assert!(!references_identifier("pub fn to_cents(amount: f64) -> i64 {", "to_cents"));
    assert!(!references_identifier("export function formatMoney(value: number) {", "formatMoney"));
    assert!(!references_identifier("  formatMoney(value: number): string {", "formatMoney"));
    assert!(!references_identifier("let x = to_cents_v2(1); // to_cents", "to_cents"));
    assert!(!references_identifier("my_to_cents(1)", "to_cents"));
  }

  #[test]
  fn should_report_reuse_target_references_in_added_lines_only() {
    let diff = Diff::parse(
      "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n fn keep() { to_cents(1); }\n+fn add() -> i64 { money::to_cents(2.0) }\n-fn old() { parse_money(); }\n",
    );
    let references = reuse_references(&diff, &["crate::money::to_cents".to_owned(), "crate::money::parse_money".to_owned()]);
    assert_eq!(
      references,
      vec![
        ReuseReference {
          target: "crate::money::to_cents".to_owned(),
          referenced: true,
          locations: vec!["src/a.rs:2".to_owned()]
        },
        ReuseReference {
          target: "crate::money::parse_money".to_owned(),
          referenced: false,
          locations: vec![]
        },
      ]
    );
  }

  #[test]
  fn should_find_functions_added_relative_to_the_base_file() {
    let base = analyze_source("src/money.rs", "pub fn to_cents(x: f64) -> i64 { (x * 100.0) as i64 }\n").unwrap();
    let post = analyze_source(
      "src/money.rs",
      "pub fn to_cents(x: f64) -> i64 { (x * 100.0) as i64 }\n\npub fn cents_of(x: f64) -> i64 {\n  (x * 100.0).round() as i64\n}\n",
    )
    .unwrap();
    let added = new_functions("src/money.rs", Some(&base), &post);
    assert_eq!(
      added,
      vec![NewFunction {
        path: "src/money.rs".to_owned(),
        name: "cents_of".to_owned(),
        item_path: "cents_of".to_owned(),
        start_line: 3,
        end_line: 5
      }]
    );
    let ts = analyze_source("web/src/a.ts", "export class A {\n  run(): number {\n    return 1;\n  }\n}\n").unwrap();
    let names: Vec<String> = new_functions("web/src/a.ts", None, &ts).into_iter().map(|function| function.item_path).collect();
    assert_eq!(names, vec!["A::run"]);
  }

  fn function() -> NewFunction {
    NewFunction {
      path: "src/money.rs".to_owned(),
      name: "cents_of".to_owned(),
      item_path: "cents_of".to_owned(),
      start_line: 3,
      end_line: 5,
    }
  }

  fn candidate(symbol: &str, structural: Option<f64>, lexical: Option<f64>, verdict: Option<&str>, location: &str) -> SimilarMatch {
    SimilarMatch {
      symbol: symbol.to_owned(),
      structural,
      lexical,
      verdict: verdict.map(str::to_owned),
      location: Some(location.to_owned()),
    }
  }

  #[test]
  fn should_flag_duplication_above_the_jaccard_threshold_or_on_inferred_equivalence() {
    let matches = vec![
      candidate("rust:b:b:money::to_cents#function", Some(0.85), Some(6.2), None, "b:src/money.rs:1"),
      candidate("rust:b:b:money::round_cents#function", None, None, Some("inferred equivalent (0.9)"), "b:src/money.rs:9"),
      candidate("rust:b:b:money::parse_money#function", Some(0.79), None, None, "b:src/money.rs:20"),
      candidate("rust:b:b:money::cents_of#function", Some(1.0), None, None, "b:src/money.rs:3"),
    ];
    let scored = duplication_findings(
      &function(),
      &matches,
      &["crate::money::to_cents".to_owned()],
      &["money::round_cents".to_owned(), "money::parse_money".to_owned()],
    );
    let summary: Vec<(&str, TargetRole, &DuplicationEvidence)> = scored.findings.iter().map(|finding| (finding.target.as_str(), finding.role, &finding.evidence)).collect();
    assert_eq!(
      summary,
      vec![
        ("crate::money::to_cents", TargetRole::ReuseTarget, &DuplicationEvidence::StructuralJaccard { jaccard: 0.85 }),
        (
          "money::round_cents",
          TargetRole::ForbiddenDuplicate,
          &DuplicationEvidence::InferredEquivalent {
            verdict: "inferred equivalent (0.9)".to_owned()
          }
        ),
      ]
    );
    assert_eq!(scored.lexical_candidates, Vec::new());
  }

  #[test]
  fn should_report_lexical_only_matches_as_candidates_not_duplication() {
    let matches = vec![
      candidate(
        "ts:tree:hono:src/utils/accept::parseAccept#function",
        None,
        Some(5.644),
        None,
        "tree:src/utils/accept.ts:12",
      ),
      candidate(
        "ts:tree:hono:src/middleware/compress/index::selectEncoding#function",
        Some(0.42),
        Some(8.362),
        None,
        "tree:src/middleware/compress/index.ts:30",
      ),
    ];
    let scored = duplication_findings(
      &function(),
      &matches,
      &["src/utils/accept::parseAccept".to_owned()],
      &["src/middleware/compress/index::selectEncoding".to_owned()],
    );
    assert_eq!(scored.findings, Vec::new());
    let summary: Vec<(&str, TargetRole, f64)> = scored
      .lexical_candidates
      .iter()
      .map(|candidate| (candidate.target.as_str(), candidate.role, candidate.score))
      .collect();
    assert_eq!(
      summary,
      vec![
        ("src/utils/accept::parseAccept", TargetRole::ReuseTarget, 5.644),
        ("src/middleware/compress/index::selectEncoding", TargetRole::ForbiddenDuplicate, 8.362),
      ]
    );
  }

  fn legacy_finding(score: f64) -> DuplicationFinding {
    DuplicationFinding {
      new_function: "negotiateEncodings".to_owned(),
      location: "src/middleware/serve-static/index.ts:34-51".to_owned(),
      matched_symbol: "ts:tree:hono:src/middleware/compress/index::selectEncoding#function".to_owned(),
      target: "src/middleware/compress/index::selectEncoding".to_owned(),
      role: TargetRole::ForbiddenDuplicate,
      evidence: DuplicationEvidence::UnclassifiedScore { score },
    }
  }

  #[test]
  fn should_read_legacy_structural_evidence_as_an_unclassified_score() {
    let stored = r#"{"new_function": "negotiateEncodings", "location": "src/middleware/serve-static/index.ts:34-51", "matched_symbol": "ts:tree:hono:src/middleware/compress/index::selectEncoding#function", "target": "src/middleware/compress/index::selectEncoding", "role": "forbidden_duplicate", "evidence": {"kind": "structural", "score": 5.24}}"#;
    assert_eq!(serde_json::from_str::<DuplicationFinding>(stored).unwrap(), legacy_finding(5.24));
  }

  #[test]
  fn should_reclassify_legacy_scores_above_one_as_lexical_and_keep_the_ambiguous_rest() {
    let reclassified = reclassify_legacy(vec![legacy_finding(5.24), legacy_finding(0.9)]);
    assert_eq!(reclassified.findings, vec![legacy_finding(0.9)]);
    assert_eq!(
      reclassified.lexical_candidates,
      vec![LexicalCandidate {
        new_function: "negotiateEncodings".to_owned(),
        location: "src/middleware/serve-static/index.ts:34-51".to_owned(),
        matched_symbol: "ts:tree:hono:src/middleware/compress/index::selectEncoding#function".to_owned(),
        target: "src/middleware/compress/index::selectEncoding".to_owned(),
        role: TargetRole::ForbiddenDuplicate,
        score: 5.24,
      }]
    );
    assert!(!reclassified.findings[0].evidence.is_conclusive());
  }

  #[test]
  fn should_list_new_functions_that_reuse_a_target_name() {
    assert_eq!(same_name_as_targets(&[function()], &["helpers::cents_of".to_owned()]), vec!["src/money.rs:3 cents_of"]);
  }
}
