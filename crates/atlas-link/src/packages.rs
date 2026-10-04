use atlas_core::{Certainty, Edge, EdgeKind, Entity, EntityId, EntityKey, EntityKind, RepoName};
use serde_json::{Value, json};

use crate::error::LinkResult;
use crate::facts::Facts;
use crate::output::{Output, attr_str, relabel, weaker};

const ATTR_REPO: &str = "repo";
const ATTR_IN_PROJECT: &str = "in_project";
const RULE: &str = "manifest-dependency";

pub(crate) fn link_packages(facts: &Facts<'_, '_>, output: &mut Output) -> LinkResult<()> {
  for package in facts.entities(&EntityKind::Package)? {
    let Some(source_repo) = package_repo(&package) else { continue };
    if !facts.in_project(&source_repo) {
      continue;
    }
    for dependency in facts.edges_from(package.id, &EdgeKind::DependsOnPackage)? {
      let Some(target) = facts.entity(dependency.to)? else { continue };
      let Some(target_repo) = dependency_repo(&dependency, &target) else { continue };
      if target_repo == source_repo || !facts.in_project(&target_repo) {
        continue;
      }
      output.edge(cross_repo_package_edge(&dependency, &target_repo));
      output.edge(repo_edge(&dependency, &package, &target, &source_repo, &target_repo));
    }
  }
  Ok(())
}

fn package_repo(package: &Entity) -> Option<RepoName> {
  RepoName::new(attr_str(&package.attrs, ATTR_REPO)?).ok()
}

fn dependency_repo(dependency: &Edge, target: &Entity) -> Option<RepoName> {
  let declared = dependency.attrs.get(ATTR_IN_PROJECT).and_then(|value| value.get(ATTR_REPO)).and_then(Value::as_str);
  match declared {
    Some(repo) => RepoName::new(repo).ok(),
    None => package_repo(target),
  }
}

fn cross_repo_package_edge(dependency: &Edge, target_repo: &RepoName) -> Edge {
  let mut edge = dependency.clone();
  edge.attrs.insert("cross_repo".to_owned(), json!(true));
  edge.attrs.insert("target_repo".to_owned(), json!(target_repo.as_str()));
  edge.certainty = weaker(&dependency.certainty, &Certainty::Resolved);
  edge.evidence = dependency.evidence.iter().map(|evidence| relabel(evidence, None)).collect();
  edge
}

fn repo_edge(dependency: &Edge, package: &Entity, target: &Entity, source_repo: &RepoName, target_repo: &RepoName) -> Edge {
  let from = EntityId::of(&EntityKey::Repository { repo: source_repo.clone() });
  let to = EntityId::of(&EntityKey::Repository { repo: target_repo.clone() });
  let discriminator = format!("{} -> {}", package.key, target.key);
  let note = format!("{} depends on {}", package.name, target.name);
  let mut edge = Edge::new(EdgeKind::DependsOnPackage, from, to, &discriminator, Certainty::Resolved);
  edge.attrs.insert("rule".to_owned(), json!(RULE));
  edge.attrs.insert("from_package".to_owned(), json!(package.key.to_string()));
  edge.attrs.insert("to_package".to_owned(), json!(target.key.to_string()));
  edge.attrs.insert("sections".to_owned(), dependency.attrs.get("sections").cloned().unwrap_or(Value::Null));
  edge.evidence = dependency.evidence.iter().map(|evidence| relabel(evidence, Some(note.clone()))).collect();
  edge
}
