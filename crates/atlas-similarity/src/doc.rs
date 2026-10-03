use atlas_core::{BodyFingerprintInput, EntityId, EntityKind, RelPath, RepoName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimilarityDoc {
  pub entity: EntityId,
  pub kind: EntityKind,
  pub repo: RepoName,
  pub name: String,
  pub path: RelPath,
  pub signature: Option<String>,
  pub doc: Option<String>,
  pub referenced_names: Vec<String>,
  pub body: Option<BodyFingerprintInput>,
}
