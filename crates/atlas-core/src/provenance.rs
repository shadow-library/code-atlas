use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::{ContentHash, RelPath, RepoName};
use crate::span::Span;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct Confidence(f32);

impl Confidence {
  pub const CERTAIN: Confidence = Confidence(1.0);

  pub fn new(value: f32) -> Result<Self, CoreError> {
    if !(0.0..=1.0).contains(&value) {
      return Err(CoreError::InvalidConfidence(value));
    }
    Ok(Self(if value == 0.0 { 0.0 } else { value }))
  }

  pub fn value(self) -> f32 {
    self.0
  }

  fn basis_points(self) -> u16 {
    let scaled = (self.0 * 10_000.0).round();
    if scaled >= 10_000.0 { 10_000 } else { scaled as u16 }
  }
}

impl PartialEq for Confidence {
  fn eq(&self, other: &Self) -> bool {
    self.0.to_bits() == other.0.to_bits()
  }
}

impl Eq for Confidence {}

impl Hash for Confidence {
  fn hash<H: Hasher>(&self, state: &mut H) {
    self.0.to_bits().hash(state);
  }
}

impl PartialOrd for Confidence {
  fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
    Some(self.cmp(other))
  }
}

impl Ord for Confidence {
  fn cmp(&self, other: &Self) -> Ordering {
    self.0.total_cmp(&other.0)
  }
}

impl fmt::Display for Confidence {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{:.2}", self.0)
  }
}

impl TryFrom<f32> for Confidence {
  type Error = CoreError;

  fn try_from(value: f32) -> Result<Self, Self::Error> {
    Self::new(value)
  }
}

impl From<Confidence> for f32 {
  fn from(value: Confidence) -> Self {
    value.0
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertaintyClass {
  CompilerVerified,
  Syntactic,
  Resolved,
  Heuristic,
  Inferred,
  Declared,
}

impl CertaintyClass {
  pub const ALL: [CertaintyClass; 6] = [Self::CompilerVerified, Self::Syntactic, Self::Resolved, Self::Heuristic, Self::Inferred, Self::Declared];

  pub fn as_str(self) -> &'static str {
    match self {
      Self::CompilerVerified => "compiler_verified",
      Self::Syntactic => "syntactic",
      Self::Resolved => "resolved",
      Self::Heuristic => "heuristic",
      Self::Inferred => "inferred",
      Self::Declared => "declared",
    }
  }
}

impl fmt::Display for CertaintyClass {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

impl std::str::FromStr for CertaintyClass {
  type Err = CoreError;

  fn from_str(value: &str) -> Result<Self, Self::Err> {
    Self::ALL.into_iter().find(|class| class.as_str() == value).ok_or_else(|| CoreError::UnknownVariant {
      what: "certainty class",
      value: value.to_owned(),
    })
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum Certainty {
  CompilerVerified,
  Syntactic,
  Resolved,
  Heuristic {
    confidence: Confidence,
    rule: String,
  },
  Inferred {
    confidence: Confidence,
    provider: String,
    model: String,
    rationale: String,
  },
  Declared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CertaintyRank {
  tier: u8,
  basis_points: u16,
  class_order: u8,
}

impl Certainty {
  pub fn heuristic(confidence: Confidence, rule: impl Into<String>) -> Self {
    Self::Heuristic { confidence, rule: rule.into() }
  }

  pub fn class(&self) -> CertaintyClass {
    match self {
      Self::CompilerVerified => CertaintyClass::CompilerVerified,
      Self::Syntactic => CertaintyClass::Syntactic,
      Self::Resolved => CertaintyClass::Resolved,
      Self::Heuristic { .. } => CertaintyClass::Heuristic,
      Self::Inferred { .. } => CertaintyClass::Inferred,
      Self::Declared => CertaintyClass::Declared,
    }
  }

  pub fn confidence(&self) -> Option<Confidence> {
    match self {
      Self::Heuristic { confidence, .. } | Self::Inferred { confidence, .. } => Some(*confidence),
      _ => None,
    }
  }

  pub fn is_llm_derived(&self) -> bool {
    matches!(self, Self::Inferred { .. })
  }

  pub fn is_authoritative(&self) -> bool {
    matches!(self, Self::CompilerVerified | Self::Syntactic | Self::Resolved | Self::Declared)
  }

  pub fn rank(&self) -> CertaintyRank {
    let (tier, class_order) = match self {
      Self::CompilerVerified => (5, 0),
      Self::Syntactic => (4, 1),
      Self::Declared => (4, 0),
      Self::Resolved => (3, 0),
      Self::Heuristic { .. } => (2, 0),
      Self::Inferred { .. } => (1, 0),
    };
    CertaintyRank {
      tier,
      basis_points: self.confidence().map_or(0, Confidence::basis_points),
      class_order,
    }
  }

  pub fn cmp_strength(&self, other: &Certainty) -> Ordering {
    self.rank().cmp(&other.rank()).then_with(|| self.tiebreak_fields().cmp(&other.tiebreak_fields()))
  }

  fn tiebreak_fields(&self) -> [&str; 3] {
    match self {
      Self::Heuristic { rule, .. } => [rule, "", ""],
      Self::Inferred { provider, model, rationale, .. } => [provider, model, rationale],
      Self::CompilerVerified | Self::Syntactic | Self::Resolved | Self::Declared => ["", "", ""],
    }
  }

  pub fn strongest<'a>(candidates: impl IntoIterator<Item = &'a Certainty>) -> Option<&'a Certainty> {
    candidates.into_iter().max_by(|left, right| left.cmp_strength(right))
  }

  pub fn tag(&self) -> String {
    match self {
      Self::CompilerVerified => "[compiler]".to_owned(),
      Self::Syntactic => "[syntactic]".to_owned(),
      Self::Resolved => "[resolved]".to_owned(),
      Self::Heuristic { confidence, .. } => format!("[heuristic {confidence}]"),
      Self::Inferred { confidence, .. } => format!("[inferred {confidence}]"),
      Self::Declared => "[declared]".to_owned(),
    }
  }

  pub fn tag_with_freshness(&self, freshness: &Freshness) -> String {
    let marker = match freshness {
      Freshness::Current => return self.tag(),
      Freshness::Stale { .. } => "stale",
      Freshness::NeedsRevalidation { .. } => "needs-revalidation",
    };
    let tag = self.tag();
    format!("{} {marker}]", &tag[..tag.len() - 1])
  }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Producer {
  pub name: String,
  pub version: u32,
}

impl Producer {
  pub fn new(name: impl Into<String>, version: u32) -> Self {
    Self { name: name.into(), version }
  }
}

impl fmt::Display for Producer {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}@{}", self.name, self.version)
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Evidence {
  pub repo: RepoName,
  pub path: RelPath,
  pub span: Span,
  pub content_hash: ContentHash,
  pub producer: Producer,
  pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Freshness {
  Current,
  Stale { reason: String },
  NeedsRevalidation { reason: String },
}

impl Freshness {
  pub fn of_evidence(evidence: &[Evidence], current_hash: impl Fn(&RepoName, &RelPath) -> Option<ContentHash>) -> Freshness {
    for item in evidence {
      match current_hash(&item.repo, &item.path) {
        None => {
          return Freshness::Stale {
            reason: format!("{}:{} no longer exists", item.repo, item.path),
          };
        }
        Some(hash) if hash != item.content_hash => {
          return Freshness::Stale {
            reason: format!("{}:{} changed", item.repo, item.path),
          };
        }
        Some(_) => {}
      }
    }
    Freshness::Current
  }

  pub fn is_current(&self) -> bool {
    matches!(self, Self::Current)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn confidence(value: f32) -> Confidence {
    Confidence::new(value).unwrap()
  }

  fn inferred(value: f32) -> Certainty {
    Certainty::Inferred {
      confidence: confidence(value),
      provider: "claude-cli".into(),
      model: "m".into(),
      rationale: "r".into(),
    }
  }

  #[test]
  fn should_validate_confidence_range() {
    assert_eq!(confidence(0.0).value(), 0.0);
    assert_eq!(confidence(1.0).value(), 1.0);
    assert_eq!(confidence(-0.0), confidence(0.0));
    for invalid in [-0.01, 1.01, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
      assert!(Confidence::new(invalid).is_err(), "{invalid}");
    }
    assert!(serde_json::from_str::<Confidence>("1.5").is_err());
    assert_eq!(serde_json::from_str::<Confidence>("0.5").unwrap(), confidence(0.5));
  }

  #[test]
  fn should_format_compact_certainty_tags() {
    assert_eq!(Certainty::CompilerVerified.tag(), "[compiler]");
    assert_eq!(Certainty::Syntactic.tag(), "[syntactic]");
    assert_eq!(Certainty::Resolved.tag(), "[resolved]");
    assert_eq!(Certainty::heuristic(confidence(0.72), "route-prefix").tag(), "[heuristic 0.72]");
    assert_eq!(inferred(0.805).tag(), "[inferred 0.81]");
    assert_eq!(Certainty::Declared.tag(), "[declared]");
  }

  #[test]
  fn should_append_freshness_markers_to_tags() {
    let stale = Freshness::Stale { reason: "changed".into() };
    let revalidate = Freshness::NeedsRevalidation { reason: "renamed".into() };
    assert_eq!(inferred(0.81).tag_with_freshness(&stale), "[inferred 0.81 stale]");
    assert_eq!(inferred(0.81).tag_with_freshness(&revalidate), "[inferred 0.81 needs-revalidation]");
    assert_eq!(Certainty::Resolved.tag_with_freshness(&Freshness::Current), "[resolved]");
  }

  #[test]
  fn should_rank_certainty_by_strength() {
    let ordered = [
      inferred(0.2),
      inferred(0.9),
      Certainty::heuristic(confidence(0.1), "a"),
      Certainty::heuristic(confidence(0.95), "a"),
      Certainty::Resolved,
      Certainty::Syntactic,
      Certainty::CompilerVerified,
    ];
    for pair in ordered.windows(2) {
      assert!(pair[0].rank() < pair[1].rank(), "{:?} must rank below {:?}", pair[0], pair[1]);
    }
    assert!(Certainty::Declared.rank() < Certainty::Syntactic.rank());
    assert!(Certainty::Resolved.rank() < Certainty::Declared.rank());
    assert!(inferred(1.0).rank() < Certainty::heuristic(confidence(0.0), "a").rank());
  }

  #[test]
  fn should_pick_the_same_strongest_certainty_regardless_of_input_order() {
    let candidates = vec![
      Certainty::heuristic(confidence(0.6), "b"),
      Certainty::Declared,
      Certainty::Syntactic,
      Certainty::Resolved,
      Certainty::heuristic(confidence(0.6), "a"),
    ];
    let mut reversed = candidates.clone();
    reversed.reverse();
    assert_eq!(Certainty::strongest(&candidates), Some(&Certainty::Syntactic));
    assert_eq!(Certainty::strongest(&reversed), Some(&Certainty::Syntactic));
    let heuristics = [Certainty::heuristic(confidence(0.6), "a"), Certainty::heuristic(confidence(0.6), "b")];
    let expected = Some(&heuristics[1]);
    assert_eq!(Certainty::strongest(&heuristics), expected);
    assert_eq!(Certainty::strongest([&heuristics[1], &heuristics[0]]), expected);
    assert_eq!(Certainty::strongest(&[]), None);
  }

  #[test]
  fn should_order_every_distinct_certainty_totally() {
    let all = [
      Certainty::CompilerVerified,
      Certainty::Syntactic,
      Certainty::Declared,
      Certainty::Resolved,
      Certainty::heuristic(confidence(0.6), "a"),
      Certainty::heuristic(confidence(0.6), "b"),
      inferred(0.6),
      Certainty::Inferred {
        confidence: confidence(0.6),
        provider: "http".into(),
        model: "m".into(),
        rationale: "r".into(),
      },
    ];
    for (i, left) in all.iter().enumerate() {
      for (j, right) in all.iter().enumerate() {
        assert_eq!(left.cmp_strength(right) == Ordering::Equal, i == j, "{left:?} vs {right:?}");
        assert_eq!(left.cmp_strength(right), right.cmp_strength(left).reverse());
      }
    }
  }

  #[test]
  fn should_flag_only_inferred_certainty_as_llm_derived() {
    assert!(!Certainty::CompilerVerified.is_llm_derived());
    assert!(!Certainty::Syntactic.is_llm_derived());
    assert!(!Certainty::Resolved.is_llm_derived());
    assert!(!Certainty::heuristic(confidence(0.5), "a").is_llm_derived());
    assert!(!Certainty::Declared.is_llm_derived());
    assert!(inferred(0.99).is_llm_derived());
  }

  #[test]
  fn should_treat_only_exact_derivations_as_authoritative() {
    assert!(Certainty::CompilerVerified.is_authoritative());
    assert!(Certainty::Syntactic.is_authoritative());
    assert!(Certainty::Resolved.is_authoritative());
    assert!(Certainty::Declared.is_authoritative());
    assert!(!Certainty::heuristic(confidence(1.0), "a").is_authoritative());
    assert!(!inferred(1.0).is_authoritative());
  }

  #[test]
  fn should_serialise_certainty_as_a_tagged_union() {
    let heuristic = Certainty::heuristic(confidence(0.5), "name-match");
    let json = serde_json::to_value(&heuristic).unwrap();
    assert_eq!(json, serde_json::json!({ "class": "heuristic", "confidence": 0.5, "rule": "name-match" }));
    assert_eq!(serde_json::from_value::<Certainty>(json).unwrap(), heuristic);
    assert_eq!(serde_json::to_value(Certainty::Resolved).unwrap(), serde_json::json!({ "class": "resolved" }));
    assert_eq!(heuristic.class().as_str(), "heuristic");
    assert_eq!("compiler_verified".parse::<CertaintyClass>().unwrap(), CertaintyClass::CompilerVerified);
  }

  #[test]
  fn should_compute_freshness_from_evidence_hashes() {
    let repo = RepoName::new("acme").unwrap();
    let path = RelPath::new("src/a.ts").unwrap();
    let original = ContentHash::of(b"v1");
    let evidence = vec![Evidence {
      repo: repo.clone(),
      path: path.clone(),
      span: Span::from_zero_based_rows(0, 1, 0, 0, 0, 1),
      content_hash: original,
      producer: Producer::new("semantic.equivalence", 1),
      note: None,
    }];
    assert_eq!(Freshness::of_evidence(&evidence, |_, _| Some(original)), Freshness::Current);
    assert_eq!(
      Freshness::of_evidence(&evidence, |_, _| Some(ContentHash::of(b"v2"))),
      Freshness::Stale {
        reason: "acme:src/a.ts changed".into()
      }
    );
    assert_eq!(
      Freshness::of_evidence(&evidence, |_, _| None),
      Freshness::Stale {
        reason: "acme:src/a.ts no longer exists".into()
      }
    );
  }
}
