use std::collections::{BTreeMap, BTreeSet, HashMap};

use atlas_core::{Edge, EdgeKind, Entity, EntityId, EntityKind, Owner, RelPath, RepoName, UnresolvedRef};
use atlas_engine::{Engine, PASS_LINEAGE, PASS_REPOS, PASS_STRUCTURE};
use atlas_project::Project;
use atlas_store::{FileRecord, FreshnessPolicy, InferredQuery, StoredInferredFact};

use super::error::QualityResult;

pub const PASS_LINK: &str = atlas_link::PASS_NAME;

pub struct Snapshot {
  pub repos: BTreeSet<String>,
  pub entities: BTreeMap<EntityId, Entity>,
  pub edges: Vec<Edge>,
  pub unresolved: Vec<UnresolvedRef>,
  pub files: BTreeMap<(RepoName, RelPath), FileRecord>,
  pub inferred: Vec<StoredInferredFact>,
  outgoing: HashMap<EntityId, Vec<usize>>,
  incoming: HashMap<EntityId, Vec<usize>>,
}

impl Snapshot {
  pub fn load(engine: &Engine, project: &Project) -> QualityResult<Self> {
    engine.store().read(|tx| -> QualityResult<Self> {
      let mut owners = Vec::new();
      let mut files = BTreeMap::new();
      for repo in &project.repos {
        for record in tx.files_in_repo(&repo.name)? {
          owners.push(Owner::File {
            repo: record.repo.clone(),
            path: record.path.clone(),
          });
          files.insert((record.repo.clone(), record.path.clone()), record);
        }
      }
      for pass in [PASS_REPOS, PASS_STRUCTURE, PASS_LINK, PASS_LINEAGE] {
        owners.push(Owner::Project { pass: pass.to_owned() });
      }
      let mut entity_ids = BTreeSet::new();
      let mut edge_ids = BTreeSet::new();
      let mut unresolved = Vec::new();
      for owner in &owners {
        let facts = tx.owned_facts(owner)?;
        entity_ids.extend(facts.entities.iter().map(|entity| entity.id));
        edge_ids.extend(facts.edges.iter().map(|edge| edge.id));
        unresolved.extend(facts.unresolved);
      }
      let mut entities = BTreeMap::new();
      for id in entity_ids {
        if let Some(entity) = tx.entity(id)? {
          entities.insert(id, entity);
        }
      }
      let mut edges = Vec::new();
      for id in edge_ids {
        if let Some(edge) = tx.edge(id)? {
          edges.push(edge);
        }
      }
      let inferred = tx.inferred_facts(&InferredQuery {
        subject: None,
        relation: None,
        freshness: FreshnessPolicy::IncludeStale,
      })?;
      Ok(Self::assemble(
        project.repos.iter().map(|repo| repo.name.to_string()).collect(),
        entities,
        edges,
        unresolved,
        files,
        inferred,
      ))
    })
  }

  pub fn assemble(
    repos: BTreeSet<String>,
    entities: BTreeMap<EntityId, Entity>,
    edges: Vec<Edge>,
    unresolved: Vec<UnresolvedRef>,
    files: BTreeMap<(RepoName, RelPath), FileRecord>,
    inferred: Vec<StoredInferredFact>,
  ) -> Self {
    let mut outgoing: HashMap<EntityId, Vec<usize>> = HashMap::new();
    let mut incoming: HashMap<EntityId, Vec<usize>> = HashMap::new();
    for (index, edge) in edges.iter().enumerate() {
      outgoing.entry(edge.from).or_default().push(index);
      incoming.entry(edge.to).or_default().push(index);
    }
    Self {
      repos,
      entities,
      edges,
      unresolved,
      files,
      inferred,
      outgoing,
      incoming,
    }
  }

  pub fn entity(&self, id: EntityId) -> Option<&Entity> {
    self.entities.get(&id)
  }

  pub fn edges_from(&self, id: EntityId) -> impl Iterator<Item = &Edge> {
    self.outgoing.get(&id).into_iter().flatten().map(|index| &self.edges[*index])
  }

  pub fn edges_to(&self, id: EntityId) -> impl Iterator<Item = &Edge> {
    self.incoming.get(&id).into_iter().flatten().map(|index| &self.edges[*index])
  }

  pub fn edges_between<'s>(&'s self, from: EntityId, to: EntityId, kind: &'s EdgeKind) -> impl Iterator<Item = &'s Edge> {
    self.edges_from(from).filter(move |edge| edge.to == to && &edge.kind == kind)
  }

  pub fn entities_of_kind<'s>(&'s self, kind: &'s EntityKind) -> impl Iterator<Item = &'s Entity> {
    self.entities.values().filter(move |entity| &entity.kind == kind)
  }

  pub fn unresolved_from(&self, id: EntityId) -> impl Iterator<Item = &UnresolvedRef> {
    self.unresolved.iter().filter(move |reference| reference.from == id)
  }

  pub fn describe(&self, id: EntityId) -> String {
    self.entity(id).map_or_else(|| format!("<dangling {id}>"), |entity| entity.key.to_string())
  }
}
