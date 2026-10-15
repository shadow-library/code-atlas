use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{EntityId, EntityKey, EntityKind, RepoName};
use serde_json::Value;

use crate::error::LinkResult;
use crate::facts::Facts;

const ATTR_MANIFEST: &str = "manifest";

#[derive(Debug, Default)]
pub(crate) struct PackageIndex {
  manifests: BTreeMap<RepoName, Vec<(String, String)>>,
  names: BTreeSet<String>,
}

impl PackageIndex {
  pub fn load(facts: &Facts<'_, '_>) -> LinkResult<Self> {
    let mut index = Self::default();
    for package in facts.entities(&EntityKind::Package)? {
      let EntityKey::Package { name, .. } = &package.key else { continue };
      for (repo, path, asserted) in facts.file_assertions(package.id)? {
        if asserted.attrs.get(ATTR_MANIFEST).and_then(Value::as_str) != Some(path.as_str()) {
          continue;
        }
        let dir = path.as_str().rsplit_once('/').map_or("", |(dir, _)| dir).to_owned();
        index.manifests.entry(repo).or_default().push((dir, name.clone()));
        index.names.insert(name.clone());
      }
    }
    for manifests in index.manifests.values_mut() {
      manifests.sort_by(|left, right| right.0.len().cmp(&left.0.len()).then_with(|| left.cmp(right)));
      manifests.dedup_by(|later, earlier| later.0 == earlier.0);
    }
    Ok(index)
  }

  pub fn nearest(&self, repo: &RepoName, path: &str) -> Option<&str> {
    self
      .manifests
      .get(repo)?
      .iter()
      .find(|(dir, _)| dir.is_empty() || path.strip_prefix(dir.as_str()).is_some_and(|rest| rest.starts_with('/')))
      .map(|(_, name)| name.as_str())
  }

  pub fn has_several(&self, repo: &RepoName) -> bool {
    self.manifests.get(repo).is_some_and(|manifests| manifests.len() > 1)
  }

  pub fn knows(&self, name: &str) -> bool {
    self.names.contains(name)
  }

  pub fn of_entity(&self, facts: &Facts<'_, '_>, id: EntityId) -> LinkResult<Vec<String>> {
    let mut packages: Vec<String> = facts
      .file_assertions(id)?
      .iter()
      .filter_map(|(repo, path, _)| self.nearest(repo, path.as_str()).map(str::to_owned))
      .collect();
    packages.sort();
    packages.dedup();
    Ok(packages)
  }
}

pub(crate) fn package_of_specifier(specifier: &str) -> Option<&str> {
  if specifier.starts_with('.') || specifier.starts_with('/') {
    return None;
  }
  let mut parts = specifier.splitn(3, '/');
  let first = parts.next()?;
  match first.starts_with('@') {
    true => parts.next().map(|second| &specifier[..first.len() + 1 + second.len()]),
    false => Some(first),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_read_the_package_name_of_bare_and_scoped_specifiers() {
    assert_eq!(package_of_specifier("@acme/books-api/openapi"), Some("@acme/books-api"));
    assert_eq!(package_of_specifier("@acme/books-api"), Some("@acme/books-api"));
    assert_eq!(package_of_specifier("books-api/openapi"), Some("books-api"));
    assert_eq!(package_of_specifier("@acme"), None);
    assert_eq!(package_of_specifier("../books-api/openapi"), None);
  }

  #[test]
  fn should_pick_the_deepest_manifest_directory_containing_a_path() {
    let repo = RepoName::new("mono").unwrap();
    let mut index = PackageIndex::default();
    index.manifests.insert(
      repo.clone(),
      vec![
        ("apps/books-api".to_owned(), "@acme/books-api".to_owned()),
        ("apps/books".to_owned(), "@acme/books".to_owned()),
        (String::new(), "mono".to_owned()),
      ],
    );
    assert_eq!(index.nearest(&repo, "apps/books-api/src/books.controller.ts"), Some("@acme/books-api"));
    assert_eq!(index.nearest(&repo, "apps/books/src/main.ts"), Some("@acme/books"));
    assert_eq!(index.nearest(&repo, "apps/books-apix/src/main.ts"), Some("mono"));
    assert_eq!(index.nearest(&RepoName::new("other").unwrap(), "src/main.ts"), None);
  }
}
