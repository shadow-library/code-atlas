use std::collections::BTreeMap;

use atlas_core::{Certainty, ContentHash, Edge, EdgeKind, EntityId, EntityKey, Evidence, PackageEcosystem, Producer, RelPath, RepoName, Span};
use atlas_project::{DependencySection, PackageInfo};
use tracing::debug;

use crate::layout::PackageKey;
use crate::output::{RESOLVER_NAME, RESOLVER_VERSION};
use crate::table::ProjectSymbolTable;

#[derive(Debug, Clone, PartialEq)]
pub struct ManifestEdges {
  pub manifest: RelPath,
  pub package: EntityKey,
  pub edges: Vec<Edge>,
}

fn section_name(section: DependencySection) -> &'static str {
  match section {
    DependencySection::Normal => "normal",
    DependencySection::Dev => "dev",
    DependencySection::Build => "build",
    DependencySection::Peer => "peer",
  }
}

struct Dependency {
  name: String,
  section: DependencySection,
  in_project: Option<PackageKey>,
}

fn dependencies(table: &ProjectSymbolTable, key: &PackageKey, package: &PackageInfo) -> Vec<Dependency> {
  match (package.as_cargo(), package.as_npm()) {
    (Some(cargo), _) => cargo
      .dependencies
      .iter()
      .map(|dependency| Dependency {
        name: dependency.package_name.clone(),
        section: dependency.section,
        in_project: table.layout.in_project_dependency(key, dependency),
      })
      .collect(),
    (_, Some(npm)) => npm
      .dependencies
      .iter()
      .map(|dependency| Dependency {
        name: dependency.name.clone(),
        section: dependency.section,
        in_project: table.layout.npm_package(&key.repo, &dependency.name),
      })
      .collect(),
    _ => Vec::new(),
  }
}

pub fn manifest_edges(table: &ProjectSymbolTable, repo: &RepoName, manifest_hash: impl Fn(&RelPath) -> Option<ContentHash>) -> Vec<ManifestEdges> {
  let Some(graph) = table.layout.graph(repo) else { return Vec::new() };
  let producer = Producer::new(RESOLVER_NAME, RESOLVER_VERSION);
  let mut result = Vec::new();
  for package in &graph.packages {
    let Some(name) = package.name() else { continue };
    let Some(content_hash) = manifest_hash(&package.manifest) else {
      debug!(repo = %repo, manifest = %package.manifest, "skipping manifest without a known content hash");
      continue;
    };
    let ecosystem = match package.as_cargo() {
      Some(_) => PackageEcosystem::Cargo,
      None => PackageEcosystem::Npm,
    };
    let key = PackageKey {
      repo: repo.clone(),
      manifest: package.manifest.clone(),
    };
    let from_key = EntityKey::Package { ecosystem, name: name.to_owned() };
    let from = EntityId::of(&from_key);
    let mut grouped: BTreeMap<String, (Vec<&'static str>, Option<PackageKey>)> = BTreeMap::new();
    for dependency in dependencies(table, &key, package) {
      let entry = grouped.entry(dependency.name).or_default();
      let section = section_name(dependency.section);
      if !entry.0.contains(&section) {
        entry.0.push(section);
      }
      entry.1 = entry.1.take().or(dependency.in_project);
    }
    let edges = grouped
      .into_iter()
      .map(|(dependency, (sections, in_project))| {
        let to = EntityId::of(&EntityKey::Package { ecosystem, name: dependency });
        let mut edge = Edge::new(EdgeKind::DependsOnPackage, from, to, "", Certainty::Syntactic).with_evidence(Evidence {
          repo: repo.clone(),
          path: package.manifest.clone(),
          span: Span::from_zero_based_rows(0, 0, 0, 0, 0, 0),
          content_hash,
          producer: producer.clone(),
          note: None,
        });
        edge.attrs.insert("sections".to_owned(), serde_json::json!(sections));
        if let Some(target) = in_project {
          edge.attrs.insert(
            "in_project".to_owned(),
            serde_json::json!({ "repo": target.repo.as_str(), "manifest": target.manifest.as_str() }),
          );
        }
        edge
      })
      .collect();
    result.push(ManifestEdges {
      manifest: package.manifest.clone(),
      package: from_key,
      edges,
    });
  }
  result
}
