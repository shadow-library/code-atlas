use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{EdgeKind, EntityKey, EntityKind, RepoName};

use crate::error::LinkResult;
use crate::facts::Facts;
use crate::output::attr_str;

const GENERIC_TOKENS: [&str; 44] = [
  "api",
  "app",
  "web",
  "www",
  "service",
  "svc",
  "server",
  "backend",
  "frontend",
  "gateway",
  "core",
  "client",
  "sdk",
  "common",
  "shared",
  "http",
  "https",
  "rest",
  "url",
  "uri",
  "base",
  "host",
  "hostname",
  "endpoint",
  "addr",
  "address",
  "origin",
  "port",
  "localhost",
  "com",
  "org",
  "net",
  "io",
  "dev",
  "key",
  "token",
  "secret",
  "public",
  "private",
  "vite",
  "next",
  "react",
  "env",
  "internal",
];
const NAMED_STRENGTH: f32 = 1.0;
const PARTIAL_NAME_STRENGTH: f32 = 0.6;
const SHARED_VARIABLE_STRENGTH: f32 = 0.6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BaseSource {
  None,
  EnvVar(String),
  ConfigKey(String),
  Identifier(String),
  Host(String),
  Opaque(String),
}

impl BaseSource {
  pub fn parse(raw: Option<&str>) -> Self {
    let Some(raw) = raw else { return Self::None };
    match raw.split_once(':') {
      Some(("env", name)) => Self::EnvVar(name.to_owned()),
      Some(("config", key)) => Self::ConfigKey(key.to_owned()),
      Some(("ident", name)) => Self::Identifier(name.to_owned()),
      Some(("literal", origin)) => {
        let host = origin.split("://").nth(1).unwrap_or(origin);
        Self::Host(host.to_ascii_lowercase())
      }
      _ => Self::Opaque(raw.to_owned()),
    }
  }
}

pub(crate) struct RepoNames {
  names: BTreeMap<RepoName, Vec<BTreeSet<String>>>,
  env_reads: BTreeMap<RepoName, BTreeSet<String>>,
}

pub(crate) fn tokens(text: &str) -> BTreeSet<String> {
  let mut tokens = BTreeSet::new();
  let mut current = String::new();
  let mut previous_lower = false;
  for c in text.chars() {
    if !c.is_ascii_alphanumeric() {
      flush(&mut tokens, &mut current);
      previous_lower = false;
      continue;
    }
    if c.is_ascii_uppercase() && previous_lower {
      flush(&mut tokens, &mut current);
    }
    previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
    current.push(c.to_ascii_lowercase());
  }
  flush(&mut tokens, &mut current);
  tokens
}

fn flush(tokens: &mut BTreeSet<String>, current: &mut String) {
  if !current.is_empty() {
    tokens.insert(std::mem::take(current));
  }
}

fn is_generic(token: &str) -> bool {
  let version = token.strip_prefix('v').is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()));
  version || token.chars().all(|c| c.is_ascii_digit()) || GENERIC_TOKENS.contains(&token)
}

fn specific(words: &BTreeSet<String>) -> BTreeSet<&str> {
  words.iter().map(String::as_str).filter(|token| !is_generic(token)).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum NameMatch {
  Partial { overlap: usize },
  Full { overlap: usize },
}

impl NameMatch {
  fn of(name: &BTreeSet<String>, words: &BTreeSet<String>) -> Option<Self> {
    let distinguishing = specific(name);
    if distinguishing.is_empty() || !distinguishing.iter().all(|token| words.contains(*token)) {
      return None;
    }
    let overlap = name.intersection(words).count();
    Some(match name.is_subset(words) {
      true => Self::Full { overlap },
      false => Self::Partial { overlap },
    })
  }

  fn overlap(self) -> usize {
    match self {
      Self::Partial { overlap } | Self::Full { overlap } => overlap,
    }
  }

  fn strength(self) -> f32 {
    match self {
      Self::Full { .. } => NAMED_STRENGTH,
      Self::Partial { .. } => PARTIAL_NAME_STRENGTH,
    }
  }
}

impl RepoNames {
  pub fn load(facts: &Facts<'_, '_>, repos: impl IntoIterator<Item = RepoName>) -> LinkResult<Self> {
    let mut names: BTreeMap<RepoName, Vec<BTreeSet<String>>> = repos.into_iter().map(|repo| (repo.clone(), vec![tokens(repo.as_str())])).collect();
    for package in facts.entities(&EntityKind::Package)? {
      let Some(repo) = attr_str(&package.attrs, "repo").and_then(|repo| RepoName::new(repo).ok()) else {
        continue;
      };
      if let Some(sets) = names.get_mut(&repo) {
        sets.push(tokens(&package.name));
      }
    }
    for application in facts.entities(&EntityKind::Application)? {
      let Some(repo) = &application.repo else { continue };
      if let Some(sets) = names.get_mut(repo) {
        sets.push(tokens(&application.name));
      }
    }
    for sets in names.values_mut() {
      sets.retain(|set| !set.is_empty());
      sets.sort();
      sets.dedup();
    }
    let mut env_reads: BTreeMap<RepoName, BTreeSet<String>> = BTreeMap::new();
    for variable in facts.entities(&EntityKind::EnvVar)? {
      if let EntityKey::EnvVar { repo, name } = &variable.key
        && names.contains_key(repo)
      {
        env_reads.entry(repo.clone()).or_default().insert(name.clone());
      }
    }
    Ok(Self { names, env_reads })
  }

  pub fn evidence(&self, facts: &Facts<'_, '_>, client_repo: &RepoName, source: &BaseSource) -> LinkResult<BTreeMap<RepoName, f32>> {
    let (variables, words) = match source {
      BaseSource::EnvVar(name) => (vec![name.clone()], tokens(name)),
      BaseSource::ConfigKey(key) => (Vec::new(), tokens(key)),
      BaseSource::Identifier(name) => {
        let variables = constant_env_reads(facts, client_repo, name)?;
        let mut words = tokens(name);
        for variable in &variables {
          words.extend(tokens(variable));
        }
        (variables, words)
      }
      BaseSource::Host(host) => (Vec::new(), tokens(host)),
      BaseSource::None | BaseSource::Opaque(_) => return Ok(BTreeMap::new()),
    };
    let mut evidence = self.named(&words);
    if evidence.is_empty() {
      for (repo, reads) in &self.env_reads {
        let shared = variables.iter().any(|variable| reads.contains(variable) && !specific(&tokens(variable)).is_empty());
        if repo != client_repo && shared {
          evidence.insert(repo.clone(), SHARED_VARIABLE_STRENGTH);
        }
      }
    }
    Ok(evidence)
  }

  fn named(&self, words: &BTreeSet<String>) -> BTreeMap<RepoName, f32> {
    let matches: Vec<(&RepoName, NameMatch)> = self
      .names
      .iter()
      .filter_map(|(repo, sets)| {
        sets
          .iter()
          .filter_map(|set| NameMatch::of(set, words))
          .max_by_key(|found| (found.overlap(), *found))
          .map(|found| (repo, found))
      })
      .collect();
    let Some(best) = matches.iter().map(|(_, found)| found.overlap()).max() else {
      return BTreeMap::new();
    };
    matches
      .into_iter()
      .filter(|(_, found)| found.overlap() == best)
      .map(|(repo, found)| (repo.clone(), found.strength()))
      .collect()
  }
}

fn constant_env_reads(facts: &Facts<'_, '_>, repo: &RepoName, name: &str) -> LinkResult<Vec<String>> {
  let mut variables = BTreeSet::new();
  for constant in facts.entities_named(name)? {
    if constant.kind != EntityKind::Constant || constant.repo.as_ref() != Some(repo) {
      continue;
    }
    for read in facts.edges_from(constant.id, &EdgeKind::ReadsEnv)? {
      if let Some(variable) = facts.entity(read.to)?
        && let EntityKey::EnvVar { name, .. } = variable.key
      {
        variables.insert(name);
      }
    }
  }
  Ok(variables.into_iter().collect())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_split_snake_kebab_and_camel_case_names() {
    assert_eq!(tokens("ACME_API_URL"), BTreeSet::from(["acme".into(), "api".into(), "url".into()]));
    assert_eq!(tokens("acme-api"), BTreeSet::from(["acme".into(), "api".into()]));
    assert_eq!(tokens("acmeApiBaseUrl"), BTreeSet::from(["acme".into(), "api".into(), "base".into(), "url".into()]));
    assert_eq!(tokens("@acme/sdk"), BTreeSet::from(["acme".into(), "sdk".into()]));
  }

  #[test]
  fn should_classify_detector_base_sources() {
    assert_eq!(BaseSource::parse(Some("env:ACME_API_URL")), BaseSource::EnvVar("ACME_API_URL".into()));
    assert_eq!(BaseSource::parse(Some("config:api.url")), BaseSource::ConfigKey("api.url".into()));
    assert_eq!(BaseSource::parse(Some("ident:API_URL")), BaseSource::Identifier("API_URL".into()));
    assert_eq!(BaseSource::parse(Some("literal:https://Api.Acme.dev")), BaseSource::Host("api.acme.dev".into()));
    assert_eq!(BaseSource::parse(Some("field")), BaseSource::Opaque("field".into()));
    assert_eq!(BaseSource::parse(None), BaseSource::None);
  }

  fn repo(name: &str) -> RepoName {
    RepoName::new(name).unwrap()
  }

  fn names(repos: &[&str]) -> RepoNames {
    RepoNames {
      names: repos.iter().map(|name| (repo(name), vec![tokens(name)])).collect(),
      env_reads: BTreeMap::new(),
    }
  }

  #[test]
  fn should_name_the_most_specific_repo_covered_by_the_variable() {
    let project = names(&["acme-api", "acme-web", "acme-billing"]);
    assert_eq!(project.named(&tokens("ACME_API_URL")), BTreeMap::from([(repo("acme-api"), NAMED_STRENGTH)]));
    assert_eq!(project.named(&tokens("ACME_BILLING_URL")), BTreeMap::from([(repo("acme-billing"), NAMED_STRENGTH)]));
    assert_eq!(project.named(&tokens("api.acme.dev")), BTreeMap::from([(repo("acme-api"), NAMED_STRENGTH)]));
  }

  #[test]
  fn should_never_name_a_repo_whose_tokens_are_all_generic() {
    let project = names(&["api", "web-app", "billing-api"]);
    for noise in ["VITE_API_URL", "API_URL", "PAYMENTS_API_KEY_URL", "api.example.com", "SERVICE_URL", "APP_BASE_URL"] {
      assert!(project.named(&tokens(noise)).is_empty(), "{noise}");
    }
    assert_eq!(project.named(&tokens("BILLING_API_URL")), BTreeMap::from([(repo("billing-api"), NAMED_STRENGTH)]));
  }

  #[test]
  fn should_ignore_third_party_hosts() {
    let project = names(&["acme-api", "orders-service"]);
    assert!(project.named(&tokens("api.stripe.com")).is_empty());
    assert!(project.named(&tokens("hooks.slack.com")).is_empty());
    assert_eq!(project.named(&tokens("orders.internal")), BTreeMap::from([(repo("orders-service"), PARTIAL_NAME_STRENGTH)]));
  }

  #[test]
  fn should_use_generic_tokens_only_to_disambiguate_specific_matches() {
    let project = names(&["acme-api", "acme-sdk"]);
    assert_eq!(project.named(&tokens("ACME_API_URL")), BTreeMap::from([(repo("acme-api"), NAMED_STRENGTH)]));
    assert_eq!(
      project.named(&tokens("ACME_URL")),
      BTreeMap::from([(repo("acme-api"), PARTIAL_NAME_STRENGTH), (repo("acme-sdk"), PARTIAL_NAME_STRENGTH)])
    );
  }

  #[test]
  fn should_treat_version_and_numeric_tokens_as_generic() {
    assert!(is_generic("v1") && is_generic("v12") && is_generic("8080"));
    assert!(!is_generic("v") && !is_generic("vault"));
  }
}
