use std::fmt;
use std::path::Path;

use atlas_core::CertaintyClass;
use serde::Deserialize;

use super::error::{QualityError, QualityResult};

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct Selector {
  pub kind: Option<String>,
  pub repo: Option<String>,
  pub path: Option<String>,
  pub symbol: Option<String>,
  pub suite: Option<String>,
  pub method: Option<String>,
  pub route: Option<String>,
  pub name: Option<String>,
  pub scheme: Option<String>,
}

impl fmt::Display for Selector {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let mut parts = Vec::new();
    let mut push = |label: &str, value: &Option<String>| {
      if let Some(value) = value {
        parts.push(format!("{label}={value}"));
      }
    };
    push("kind", &self.kind);
    push("repo", &self.repo);
    push("path", &self.path);
    push("symbol", &self.symbol);
    push("suite", &self.suite);
    push("method", &self.method);
    push("route", &self.route);
    push("name", &self.name);
    push("scheme", &self.scheme);
    write!(f, "{{ {} }}", parts.join(", "))
  }
}

impl Selector {
  pub fn is_endpoint(&self) -> bool {
    self.kind.as_deref() == Some("api_endpoint")
  }

  pub fn is_wildcard(&self) -> bool {
    if self.path.is_some() || self.symbol.is_some() || self.suite.is_some() {
      return false;
    }
    match self.kind.as_deref() {
      Some("api_endpoint") => self.method.is_none() || self.route.is_none(),
      Some(_) => self.name.is_none() && self.scheme.is_none(),
      None => true,
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Certainties {
  #[serde(default)]
  pub certainty: Vec<String>,
  pub min_confidence: Option<f32>,
}

impl Certainties {
  pub fn classes(&self) -> Vec<CertaintyClass> {
    self.certainty.iter().filter_map(|class| class.parse().ok()).collect()
  }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectTruth {
  pub name: String,
  pub repos: Vec<String>,
  #[serde(default)]
  pub unused_exhaustive: bool,
  #[serde(default)]
  pub exhaustive_edge_kinds: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntityTruth {
  pub kind: String,
  pub sym: Option<Selector>,
  pub handler: Option<Selector>,
  pub repo: Option<String>,
  pub name: Option<String>,
  pub scheme: Option<String>,
  pub method: Option<String>,
  pub route: Option<String>,
  #[serde(default)]
  pub certainty: Vec<String>,
  pub min_confidence: Option<f32>,
  #[serde(default)]
  pub best_effort: bool,
}

impl EntityTruth {
  pub fn selector(&self) -> Selector {
    match &self.sym {
      Some(sym) if self.name.is_none() => sym.clone(),
      _ => Selector {
        kind: Some(self.kind.clone()),
        repo: self.repo.clone(),
        name: self.name.clone(),
        scheme: self.scheme.clone(),
        method: self.method.clone(),
        route: self.route.clone(),
        ..Selector::default()
      },
    }
  }

  pub fn model_selector(&self) -> Option<&Selector> {
    self.sym.as_ref().filter(|_| self.name.is_some())
  }

  pub fn certainties(&self) -> Certainties {
    Certainties {
      certainty: self.certainty.clone(),
      min_confidence: self.min_confidence,
    }
  }
}

#[derive(Debug, Clone, Deserialize)]
pub struct EdgeTruth {
  pub kind: String,
  pub from: Selector,
  pub to: Selector,
  #[serde(default)]
  pub certainty: Vec<String>,
  pub min_confidence: Option<f32>,
  #[serde(default = "default_required")]
  pub required: bool,
  pub occurrences: Option<usize>,
}

fn default_required() -> bool {
  true
}

impl EdgeTruth {
  pub fn certainties(&self) -> Certainties {
    Certainties {
      certainty: self.certainty.clone(),
      min_confidence: self.min_confidence,
    }
  }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ForbiddenTruth {
  pub kind: String,
  pub from: Selector,
  pub to: Selector,
  #[serde(default)]
  pub certainty: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UnresolvedTruth {
  pub from: Selector,
  pub ref_kind: String,
  pub reason: String,
  pub name: Option<String>,
  #[serde(default)]
  pub candidates: Vec<Selector>,
  #[serde(default)]
  pub unless_certainty: Vec<String>,
  pub max_certainty: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UnmatchedCallTruth {
  pub from: Selector,
  pub method: String,
  pub route: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthTruth {
  pub api_repo: String,
  #[serde(default)]
  pub supported: Vec<String>,
  #[serde(default)]
  pub used: std::collections::BTreeMap<String, Vec<String>>,
  #[serde(default)]
  pub unused: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EndpointAuthTruth {
  pub method: String,
  pub route: String,
  pub schemes: Vec<String>,
  pub mode: String,
  pub permission: Option<String>,
  #[serde(default)]
  pub certainty: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UnusedTruth {
  pub sym: Option<Selector>,
  pub endpoint: Option<Selector>,
  pub reason: Option<String>,
}

impl UnusedTruth {
  pub fn selector(&self) -> Option<Selector> {
    match (&self.sym, &self.endpoint) {
      (Some(sym), _) => Some(sym.clone()),
      (None, Some(endpoint)) => Some(Selector {
        kind: Some("api_endpoint".to_owned()),
        method: endpoint.method.clone(),
        route: endpoint.route.clone(),
        ..Selector::default()
      }),
      (None, None) => None,
    }
  }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SimilarTruth {
  pub a: Selector,
  pub b: Selector,
  pub expected: String,
  #[serde(default)]
  pub acceptable: Vec<String>,
  #[serde(default = "default_required")]
  pub deterministic_claim_allowed: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TestOfTruth {
  pub test: Selector,
  pub subject: Selector,
  #[serde(default)]
  pub certainty: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileFlagTruth {
  pub repo: String,
  pub path: String,
  pub flag: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlastRadiusTruth {
  pub change: Selector,
  #[serde(default)]
  pub must_include: Vec<Selector>,
  #[serde(default)]
  pub may_include: Vec<Selector>,
  #[serde(default)]
  pub must_not_include: Vec<Selector>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IsolationTruth {
  pub project: String,
  #[serde(default)]
  pub must_not_return: Vec<Selector>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Truth {
  pub project: ProjectTruth,
  #[serde(default, rename = "entity")]
  pub entities: Vec<EntityTruth>,
  #[serde(default, rename = "edge")]
  pub edges: Vec<EdgeTruth>,
  #[serde(default, rename = "forbidden_edge")]
  pub forbidden: Vec<ForbiddenTruth>,
  #[serde(default)]
  pub unresolved: Vec<UnresolvedTruth>,
  #[serde(default)]
  pub unmatched_call: Vec<UnmatchedCallTruth>,
  #[serde(default)]
  pub auth: Vec<AuthTruth>,
  #[serde(default)]
  pub endpoint_auth: Vec<EndpointAuthTruth>,
  #[serde(default)]
  pub unused: Vec<UnusedTruth>,
  #[serde(default)]
  pub ambiguous_unused: Vec<UnusedTruth>,
  #[serde(default)]
  pub not_unused: Vec<UnusedTruth>,
  #[serde(default)]
  pub similar: Vec<SimilarTruth>,
  #[serde(default)]
  pub test_of: Vec<TestOfTruth>,
  #[serde(default)]
  pub file_flag: Vec<FileFlagTruth>,
  #[serde(default)]
  pub blast_radius: Vec<BlastRadiusTruth>,
  #[serde(default)]
  pub isolation: Vec<IsolationTruth>,
}

pub fn parse_truth(text: &str, file: &str) -> QualityResult<Truth> {
  let truth: Truth = toml::from_str(text).map_err(|error| QualityError::truth(file, error.to_string()))?;
  validate(&truth, file)?;
  Ok(truth)
}

pub fn load_truth(path: &Path) -> QualityResult<Truth> {
  let text = std::fs::read_to_string(path).map_err(|error| QualityError::io(path, error))?;
  parse_truth(&text, &path.display().to_string())
}

fn validate(truth: &Truth, file: &str) -> QualityResult<()> {
  let classes = truth
    .entities
    .iter()
    .flat_map(|entity| entity.certainty.iter())
    .chain(truth.edges.iter().flat_map(|edge| edge.certainty.iter()))
    .chain(truth.forbidden.iter().flat_map(|edge| edge.certainty.iter()))
    .chain(truth.test_of.iter().flat_map(|test| test.certainty.iter()))
    .chain(truth.endpoint_auth.iter().flat_map(|auth| auth.certainty.iter()))
    .chain(truth.unresolved.iter().flat_map(|unresolved| unresolved.unless_certainty.iter()));
  for class in classes {
    class
      .parse::<CertaintyClass>()
      .map_err(|_| QualityError::truth(file, format!("unknown certainty class `{class}`")))?;
  }
  for similar in &truth.similar {
    for verdict in std::iter::once(&similar.expected).chain(&similar.acceptable) {
      if !matches!(verdict.as_str(), "equivalent" | "similar_not_equivalent" | "unrelated") {
        return Err(QualityError::truth(file, format!("unknown similarity verdict `{verdict}`")));
      }
    }
  }
  for flag in &truth.file_flag {
    if !matches!(flag.flag.as_str(), "generated" | "partial_syntax") {
      return Err(QualityError::truth(file, format!("unknown file flag `{}`", flag.flag)));
    }
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  const SAMPLE: &str = r#"
[project]
name = "demo"
repos = ["a"]

[[entity]]
kind = "function"
sym = { repo = "a", path = "src/x.ts", symbol = "f" }

[[entity]]
kind = "api_endpoint"
repo = "a"
method = "GET"
route = "/users/:id"
certainty = ["resolved", "heuristic"]
min_confidence = 0.8
future_key = "ignored"

[[edge]]
kind = "calls"
from = { repo = "a", path = "src/x.ts", symbol = "f" }
to = { repo = "a", path = "src/y.ts", symbol = "g" }
certainty = ["resolved"]
required = false
occurrences = 2

[[forbidden_edge]]
kind = "calls"
from = { repo = "a", path = "src/x.ts", symbol = "f" }
to = { kind = "function" }
"#;

  #[test]
  fn should_parse_selectors_and_ignore_unknown_keys() {
    let truth = parse_truth(SAMPLE, "sample.toml").unwrap();
    assert_eq!(truth.entities.len(), 2);
    let endpoint = &truth.entities[1];
    assert_eq!(endpoint.selector().method.as_deref(), Some("GET"));
    assert_eq!(endpoint.certainties().min_confidence, Some(0.8));
    assert_eq!(truth.entities[0].selector().symbol.as_deref(), Some("f"));
    assert!(!truth.edges[0].required);
    assert_eq!(truth.edges[0].occurrences, Some(2));
    assert!(truth.forbidden[0].to.is_wildcard());
  }

  #[test]
  fn should_reject_unknown_certainty_classes() {
    let broken = SAMPLE.replace("[\"resolved\"]", "[\"certain\"]");
    let error = parse_truth(&broken, "sample.toml").unwrap_err();
    assert!(error.to_string().contains("unknown certainty class `certain`"), "{error}");
  }

  #[test]
  fn should_treat_endpoint_selectors_without_method_or_route_as_wildcards() {
    let selector = Selector {
      kind: Some("api_endpoint".into()),
      repo: Some("a".into()),
      method: Some("GET".into()),
      ..Selector::default()
    };
    assert!(selector.is_wildcard());
    let full = Selector {
      route: Some("/x".into()),
      ..selector
    };
    assert!(!full.is_wildcard());
  }

  #[test]
  fn should_parse_every_committed_expected_toml() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut parsed = 0;
    for group in ["acme", "globex"] {
      load_truth(&root.join(group).join("expected.toml")).unwrap();
      parsed += 1;
    }
    for entry in std::fs::read_dir(root.join("adversarial")).unwrap() {
      load_truth(&entry.unwrap().path().join("expected.toml")).unwrap();
      parsed += 1;
    }
    assert_eq!(parsed, 13);
  }
}
