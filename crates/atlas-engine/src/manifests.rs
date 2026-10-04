use std::collections::BTreeMap;

use atlas_core::{Certainty, ContentHash, Edge, EdgeKind, Entity, EntityId, EntityKey, RelPath, RepoName};
use atlas_project::{PackageGraph, PackageInfo};
use atlas_resolve::{ProjectSymbolTable, manifest_edges};
use atlas_store::OwnedFacts;
use serde_json::json;

use crate::owner::{engine_evidence, merge_edge};

fn package_entity(key: EntityKey, repo: &RepoName, package: Option<&PackageInfo>, manifest: &RelPath) -> Entity {
  let mut entity = Entity::new(key, Certainty::Syntactic);
  entity.path = Some(manifest.clone());
  entity.attrs.insert("repo".to_owned(), json!(repo.as_str()));
  entity.attrs.insert("manifest".to_owned(), json!(manifest.as_str()));
  let version = package.and_then(|package| match (package.as_cargo(), package.as_npm()) {
    (Some(cargo), _) => cargo.version.clone(),
    (_, Some(npm)) => npm.version.clone(),
    _ => None,
  });
  if let Some(version) = version {
    entity.attrs.insert("version".to_owned(), json!(version));
  }
  entity
}

pub(crate) fn manifest_owner_facts(
  table: &ProjectSymbolTable,
  repo: &RepoName,
  graph: &PackageGraph,
  hash_of: impl Fn(&RelPath) -> Option<ContentHash>,
) -> BTreeMap<RelPath, OwnedFacts> {
  let repo_id = EntityId::of(&EntityKey::Repository { repo: repo.clone() });
  let mut owners = BTreeMap::new();
  for manifest in manifest_edges(table, repo, &hash_of) {
    let Some(content_hash) = hash_of(&manifest.manifest) else { continue };
    let package = graph.packages.iter().find(|package| package.manifest == manifest.manifest);
    let entity = package_entity(manifest.package.clone(), repo, package, &manifest.manifest);
    let mut edges = BTreeMap::new();
    let contains = Edge::new(EdgeKind::Contains, repo_id, entity.id, "", Certainty::Syntactic).with_evidence(engine_evidence(repo, &manifest.manifest, content_hash));
    merge_edge(&mut edges, contains);
    for edge in manifest.edges {
      merge_edge(&mut edges, edge);
    }
    owners.insert(
      manifest.manifest,
      OwnedFacts {
        entities: vec![entity],
        edges: edges.into_values().collect(),
        unresolved: Vec::new(),
      },
    );
  }
  owners
}
