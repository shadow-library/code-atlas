use std::collections::BTreeMap;

use atlas_core::{Edge, EntityId, RelPath, RepoName};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScipStats {
  pub documents: u64,
  pub documents_outside_project: u64,
  pub definitions_mapped: u64,
  pub definitions_unmapped: u64,
  pub definitions_ambiguous: u64,
  pub references_mapped: u64,
  pub references_external: u64,
  pub references_unmapped: u64,
  pub relationships_mapped: u64,
  pub relationships_unmapped: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnmappedDefinition {
  pub repo: RepoName,
  pub path: RelPath,
  pub entity: EntityId,
  pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScipFacts {
  pub per_file: BTreeMap<(RepoName, RelPath), Vec<Edge>>,
  pub stats: ScipStats,
  pub unmapped_definitions: Vec<UnmappedDefinition>,
  pub external_references: BTreeMap<String, u64>,
  pub warnings: Vec<String>,
}

impl ScipFacts {
  pub fn edges(&self) -> impl Iterator<Item = &Edge> {
    self.per_file.values().flatten()
  }

  pub fn owner_pass(repo: &RepoName, path: &RelPath) -> String {
    format!("scip:{repo}:{path}")
  }
}
