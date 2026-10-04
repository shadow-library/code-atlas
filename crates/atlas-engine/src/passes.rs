use std::collections::BTreeMap;
use std::fmt;

use atlas_core::{Certainty, Confidence, Edge, EdgeKind, Entity, EntityId, EntityKey, RelPath, RepoName, UnresolvedReason};
use atlas_project::{PackageGraph, PackageInfo, Project, parse_jsonc};
use atlas_store::{OwnedFacts, ReadTx, StoreError};
use serde_json::json;
use thiserror::Error;

use crate::owner::{engine_evidence, merge_edge};
use crate::packages::package_key;

pub const PASS_REPOS: &str = "repos";
pub const PASS_STRUCTURE: &str = "structure";
pub const PASS_LINEAGE: &str = "lineage";
const RESERVED_PREFIX: &str = "scip:";

pub(crate) fn is_reserved_pass_name(name: &str) -> bool {
  matches!(name, PASS_REPOS | PASS_STRUCTURE | PASS_LINEAGE) || name.starts_with(RESERVED_PREFIX)
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PassError {
  #[error(transparent)]
  Store(#[from] StoreError),
  #[error("{0}")]
  Failed(String),
}

pub struct PassContext<'a> {
  pub project: &'a Project,
  pub graphs: &'a BTreeMap<RepoName, PackageGraph>,
  pub manifest_hashes: &'a BTreeMap<(RepoName, RelPath), atlas_core::ContentHash>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PassOutput {
  pub entities: Vec<Entity>,
  pub edges: Vec<Edge>,
}

pub trait ProjectPass: Send + Sync {
  fn name(&self) -> &'static str;

  fn run(&self, tx: &ReadTx<'_>, context: &PassContext<'_>) -> Result<PassOutput, PassError>;
}

impl fmt::Debug for dyn ProjectPass {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "ProjectPass({})", self.name())
  }
}

impl PassOutput {
  pub(crate) fn into_owned(self) -> OwnedFacts {
    let mut entities: BTreeMap<EntityId, Entity> = BTreeMap::new();
    for entity in self.entities {
      entities.entry(entity.id).or_insert(entity);
    }
    let mut edges = BTreeMap::new();
    for edge in self.edges {
      merge_edge(&mut edges, edge);
    }
    OwnedFacts {
      entities: entities.into_values().collect(),
      edges: edges.into_values().collect(),
      unresolved: Vec::new(),
    }
  }
}

pub(crate) fn repos_pass(project: &Project) -> OwnedFacts {
  let entities = project
    .repos
    .iter()
    .map(|repo| Entity::new(EntityKey::Repository { repo: repo.name.clone() }, Certainty::Declared))
    .collect();
  PassOutput { entities, edges: Vec::new() }.into_owned()
}

const RULE_SERVER_BOOTSTRAP: &str = "server-bootstrap";
const BOOTSTRAP_CONFIDENCE: f32 = 0.7;

enum AppEvidence {
  Manifest { reason: &'static str },
  Bootstrap { file: RelPath, call: String },
}

pub(crate) struct StructurePass;

impl ProjectPass for StructurePass {
  fn name(&self) -> &'static str {
    PASS_STRUCTURE
  }

  fn run(&self, tx: &ReadTx<'_>, context: &PassContext<'_>) -> Result<PassOutput, PassError> {
    let bootstraps = bootstrap_calls(tx)?;
    let mut output = PassOutput::default();
    for repo in &context.project.repos {
      let Some(graph) = context.graphs.get(&repo.name) else { continue };
      for package in &graph.packages {
        let (Some(name), Some(package_key)) = (package.name(), package_key(package)) else {
          continue;
        };
        let Some(evidence) = application_evidence(&repo.path, package, &repo.name, &bootstraps) else {
          continue;
        };
        let Some(manifest_hash) = context.manifest_hashes.get(&(repo.name.clone(), package.manifest.clone())) else {
          continue;
        };
        let app_key = EntityKey::Application {
          repo: repo.name.clone(),
          name: name.to_owned(),
        };
        let (certainty, evidence_path, basis) = match &evidence {
          AppEvidence::Manifest { reason } => (Certainty::Syntactic, package.manifest.clone(), json!({ "basis": "manifest", "reason": reason })),
          AppEvidence::Bootstrap { file, call } => {
            let Ok(confidence) = Confidence::new(BOOTSTRAP_CONFIDENCE) else { continue };
            (
              Certainty::heuristic(confidence, RULE_SERVER_BOOTSTRAP),
              file.clone(),
              json!({ "basis": "bootstrap", "call": call, "file": file.as_str() }),
            )
          }
        };
        let mut app = Entity::new(app_key, certainty.clone());
        app.path = Some(package.manifest.clone());
        app.attrs.insert("evidence".to_owned(), basis);
        let file_hash = match &evidence {
          AppEvidence::Manifest { .. } => *manifest_hash,
          AppEvidence::Bootstrap { .. } => tx.file(&repo.name, &evidence_path)?.map_or(*manifest_hash, |record| record.content_hash),
        };
        let proof = engine_evidence(&repo.name, &evidence_path, file_hash);
        output
          .edges
          .push(Edge::new(EdgeKind::PartOfApp, EntityId::of(&package_key), app.id, "", certainty.clone()).with_evidence(proof.clone()));
        output
          .edges
          .push(Edge::new(EdgeKind::Contains, EntityId::of(&EntityKey::Repository { repo: repo.name.clone() }), app.id, "", certainty).with_evidence(proof));
        output.entities.push(app);
      }
    }
    Ok(output)
  }
}

struct BootstrapCall {
  repo: RepoName,
  file: RelPath,
  call: String,
}

fn bootstrap_calls(tx: &ReadTx<'_>) -> Result<Vec<BootstrapCall>, PassError> {
  let mut calls = Vec::new();
  for found in tx.unresolved_by_names(&["create", "serve", "listen"])? {
    let reference = found.reference;
    let path: Vec<&str> = reference.path.iter().map(String::as_str).collect();
    let call = match (path.as_slice(), &reference.reason) {
      ([.., "NestFactory", "create"], _) => "NestFactory.create",
      ([.., "axum", "serve"], _) => "axum::serve",
      ([.., "serve"], UnresolvedReason::External { package }) if package == "axum" => "axum::serve",
      ([.., "listen"], UnresolvedReason::External { package }) if package == "express" => "listen",
      _ => continue,
    };
    calls.push(BootstrapCall {
      repo: reference.repo,
      file: reference.file,
      call: call.to_owned(),
    });
  }
  calls.sort_by(|left, right| (&left.repo, &left.file, &left.call).cmp(&(&right.repo, &right.file, &right.call)));
  calls.dedup_by(|left, right| (&left.repo, &left.file, &left.call) == (&right.repo, &right.file, &right.call));
  Ok(calls)
}

fn npm_start_script(repo_root: &std::path::Path, manifest: &RelPath) -> bool {
  let Ok(text) = std::fs::read_to_string(repo_root.join(manifest.as_str())) else {
    return false;
  };
  let Ok(value) = parse_jsonc(&text) else { return false };
  value.get("scripts").and_then(|scripts| scripts.get("start")).is_some_and(|start| start.as_str().is_some())
}

fn application_evidence(repo_root: &std::path::Path, package: &PackageInfo, repo: &RepoName, bootstraps: &[BootstrapCall]) -> Option<AppEvidence> {
  if let Some(cargo) = package.as_cargo()
    && cargo.targets.iter().any(|target| matches!(target.target, atlas_core::RustTarget::Bin(_)))
  {
    return Some(AppEvidence::Manifest { reason: "cargo bin target" });
  }
  if package.as_npm().is_some() && npm_start_script(repo_root, &package.manifest) {
    return Some(AppEvidence::Manifest {
      reason: "package.json start script",
    });
  }
  bootstraps
    .iter()
    .find(|call| call.repo == *repo && package.root.contains(&call.file))
    .map(|call| AppEvidence::Bootstrap {
      file: call.file.clone(),
      call: call.call.clone(),
    })
}
