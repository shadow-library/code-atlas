use std::collections::HashMap;
use std::sync::LazyLock;

use crate::text::stem;

const GROUPS: &[&[&str]] = &[
  &["auth", "authenticate", "authentication", "authorize", "authorization", "login", "credential", "token"],
  &["money", "currency", "cents", "price", "amount"],
  &["remove", "delete", "destroy", "erase"],
  &["fetch", "get", "retrieve", "load", "find"],
  &["create", "add", "insert", "new"],
  &["user", "account", "member"],
  &["config", "configuration", "setting", "option", "env", "environment"],
  &["parse", "decode", "deserialize"],
  &["format", "render", "serialize", "encode"],
  &["update", "modify", "edit", "change", "patch"],
  &["validate", "verify", "check", "sanitize"],
  &["send", "emit", "publish", "dispatch", "notify"],
  &["receive", "subscribe", "listen", "consume", "handle"],
  &["error", "exception", "failure", "fault"],
  &["log", "logger", "trace", "audit"],
  &["cache", "memoize", "memo"],
  &["endpoint", "route", "handler"],
  &["database", "db", "persistence", "storage"],
  &["list", "enumerate", "scan"],
  &["hash", "digest", "checksum", "fingerprint"],
  &["encrypt", "encryption", "cipher", "crypto"],
  &["email", "mail", "smtp"],
  &["retry", "backoff", "reattempt"],
  &["permission", "role", "policy", "acl", "access"],
  &["queue", "job", "task", "worker"],
];

struct Concepts {
  groups: Vec<Vec<String>>,
  membership: HashMap<String, Vec<usize>>,
}

static CONCEPTS: LazyLock<Concepts> = LazyLock::new(|| {
  let mut groups: Vec<Vec<String>> = Vec::new();
  let mut membership: HashMap<String, Vec<usize>> = HashMap::new();
  for (index, group) in GROUPS.iter().enumerate() {
    let mut stems: Vec<String> = Vec::new();
    for word in *group {
      let stemmed = stem(word);
      if !stems.contains(&stemmed) {
        stems.push(stemmed);
      }
    }
    for stemmed in &stems {
      membership.entry(stemmed.clone()).or_default().push(index);
    }
    groups.push(stems);
  }
  Concepts { groups, membership }
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Expansion {
  pub term: String,
  pub from: Vec<String>,
}

pub(crate) fn expand(direct: &[String], all: &[String]) -> Vec<Expansion> {
  let concepts = &*CONCEPTS;
  let mut expansions: Vec<Expansion> = Vec::new();
  for source in all {
    let Some(groups) = concepts.membership.get(source) else { continue };
    for &group in groups {
      for member in &concepts.groups[group] {
        if member == source || direct.contains(member) {
          continue;
        }
        match expansions.iter_mut().find(|expansion| &expansion.term == member) {
          Some(existing) => {
            if !existing.from.contains(source) {
              existing.from.push(source.clone());
            }
          }
          None => expansions.push(Expansion {
            term: member.clone(),
            from: vec![source.clone()],
          }),
        }
      }
    }
  }
  expansions
}

#[cfg(test)]
mod tests {
  use super::*;

  fn terms(expansions: &[Expansion]) -> Vec<&str> {
    expansions.iter().map(|expansion| expansion.term.as_str()).collect()
  }

  #[test]
  fn should_expand_a_concept_to_its_group_excluding_direct_terms() {
    let direct = vec!["remov".to_owned(), "user".to_owned()];
    let expansions = expand(&direct, &direct);
    assert_eq!(terms(&expansions), ["delet", "destroy", "eras", "account", "memb"]);
    assert_eq!(expansions[0].from, ["remov"]);
  }

  #[test]
  fn should_expand_through_stopword_members_of_the_query() {
    let direct = vec!["user".to_owned()];
    let all = vec!["get".to_owned(), "user".to_owned()];
    let expansions = expand(&direct, &all);
    assert!(expansions.iter().any(|expansion| expansion.term == "fetch" && expansion.from == ["get"]));
  }

  #[test]
  fn should_not_expand_unknown_terms() {
    let direct = vec!["frobnicate".to_owned()];
    assert!(expand(&direct, &direct).is_empty());
  }

  #[test]
  fn should_define_about_twenty_five_groups() {
    assert!(GROUPS.len() >= 20);
  }
}
