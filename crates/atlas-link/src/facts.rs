use std::collections::BTreeSet;

use atlas_core::{Edge, EdgeKind, Entity, EntityId, EntityKind, Owner, RelPath, RepoName};
use atlas_store::{Assertion, EdgeFilter, EntityFilter, ReadTx};

use crate::error::LinkResult;
use crate::pass::PASS_NAME;

const LINEAGE_PASS: &str = "lineage";
const SCIP_PASS_PREFIX: &str = "scip:";

pub(crate) struct Facts<'t, 'c> {
  tx: &'t ReadTx<'c>,
  repos: BTreeSet<RepoName>,
}

fn is_readable(owner: &Owner) -> bool {
  match owner {
    Owner::File { .. } => true,
    Owner::Project { pass } => pass != PASS_NAME && pass != LINEAGE_PASS && !pass.starts_with(SCIP_PASS_PREFIX),
  }
}

fn strongest<T>(assertions: Vec<Assertion<T>>, certainty: impl Fn(&T) -> &atlas_core::Certainty) -> Option<(T, Vec<Assertion<T>>)> {
  let readable: Vec<Assertion<T>> = assertions.into_iter().filter(|assertion| is_readable(&assertion.owner)).collect();
  let index = readable
    .iter()
    .enumerate()
    .max_by(|(_, left), (_, right)| certainty(&left.fact).cmp_strength(certainty(&right.fact)).then_with(|| right.owner.cmp(&left.owner)))
    .map(|(index, _)| index)?;
  let mut rest = readable;
  let winner = rest.remove(index);
  Some((winner.fact, rest))
}

impl<'t, 'c> Facts<'t, 'c> {
  pub fn new(tx: &'t ReadTx<'c>, repos: BTreeSet<RepoName>) -> Self {
    Self { tx, repos }
  }

  pub fn in_project(&self, repo: &RepoName) -> bool {
    self.repos.contains(repo)
  }

  pub fn entity(&self, id: EntityId) -> LinkResult<Option<Entity>> {
    let assertions = self.tx.entity_assertions(id)?;
    let in_project = assertions.iter().any(|assertion| self.owned_in_project(&assertion.owner));
    let Some((entity, _)) = strongest(assertions, |entity: &Entity| &entity.certainty) else {
      return Ok(None);
    };
    let scoped = match &entity.repo {
      Some(repo) => self.in_project(repo),
      None => in_project,
    };
    Ok(scoped.then_some(entity))
  }

  fn owned_in_project(&self, owner: &Owner) -> bool {
    match owner {
      Owner::File { repo, .. } => self.in_project(repo),
      Owner::Project { .. } => true,
    }
  }

  pub fn entities(&self, kind: &EntityKind) -> LinkResult<Vec<Entity>> {
    let mut found = Vec::new();
    for listed in self.tx.entities_by_kind(kind, &EntityFilter::default())? {
      found.extend(self.entity(listed.id)?);
    }
    Ok(found)
  }

  pub fn entities_named(&self, name: &str) -> LinkResult<Vec<Entity>> {
    let mut found = Vec::new();
    for listed in self.tx.entities_by_name(name, &EntityFilter::default())? {
      found.extend(self.entity(listed.id)?);
    }
    Ok(found)
  }

  pub fn entities_in_file(&self, repo: &RepoName, path: &RelPath) -> LinkResult<Vec<Entity>> {
    let mut found = Vec::new();
    for listed in self.tx.entities_in_file(repo, path, &EntityFilter::default())? {
      found.extend(self.entity(listed.id)?);
    }
    Ok(found)
  }

  pub fn edges_from(&self, id: EntityId, kind: &EdgeKind) -> LinkResult<Vec<Edge>> {
    let listed = self.tx.edges_from(id, &filter(kind))?;
    self.readable_edges(listed)
  }

  pub fn edges_to(&self, id: EntityId, kind: &EdgeKind) -> LinkResult<Vec<Edge>> {
    let listed = self.tx.edges_to(id, &filter(kind))?;
    self.readable_edges(listed)
  }

  fn readable_edges(&self, listed: Vec<Edge>) -> LinkResult<Vec<Edge>> {
    let mut edges = Vec::with_capacity(listed.len());
    for edge in listed {
      let Some((mut merged, others)) = strongest(self.tx.edge_assertions(edge.id)?, |edge: &Edge| &edge.certainty) else {
        continue;
      };
      merged.evidence.extend(others.into_iter().flat_map(|assertion| assertion.fact.evidence));
      sort_evidence(&mut merged.evidence);
      edges.push(merged);
    }
    Ok(edges)
  }

  pub fn file_hash(&self, repo: &RepoName, path: &RelPath) -> LinkResult<Option<atlas_core::ContentHash>> {
    Ok(self.tx.file(repo, path)?.map(|record| record.content_hash))
  }
}

fn filter(kind: &EdgeKind) -> EdgeFilter {
  EdgeFilter {
    kinds: vec![kind.clone()],
    ..EdgeFilter::default()
  }
}

pub(crate) fn evidence_key(evidence: &atlas_core::Evidence) -> impl Ord + '_ {
  (&evidence.repo, &evidence.path, evidence.span, &evidence.content_hash, &evidence.producer, &evidence.note)
}

pub(crate) fn sort_evidence(evidence: &mut Vec<atlas_core::Evidence>) {
  evidence.sort_by(|left, right| evidence_key(left).cmp(&evidence_key(right)));
  evidence.dedup();
}
