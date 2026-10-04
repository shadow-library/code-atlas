use std::collections::BTreeSet;
use std::rc::Rc;

use atlas_core::{EdgeKind, Entity, EntityId, EntityKey, Owner, RefKind, RelPath, RepoName, UnresolvedReason};

use crate::error::QueryResult;
use crate::graph::Graph;
use crate::model::{Caveat, Location, UnresolvedSite};
use crate::queries::common::file_entity;

const SITES_SHOWN: usize = 5;

pub(crate) fn is_computed_segment(segment: &str) -> bool {
  segment.starts_with('[') && segment.ends_with(']')
}

pub(crate) fn dynamic_member_calls(graph: &Graph<'_, '_>, target: &Entity) -> QueryResult<Option<Caveat>> {
  if target.key.as_symbol().is_none() || !target.kind.is_code_symbol() {
    return Ok(None);
  }
  let mut files: BTreeSet<(RepoName, RelPath)> = BTreeSet::new();
  if let (Some(repo), Some(path)) = (&target.repo, &target.path) {
    files.insert((repo.clone(), path.clone()));
  }
  let mut anchors = vec![target.id];
  anchors.extend(file_entity(target).map(|file| EntityId::of(&file)));
  if let Some(container) = graph.parent(target.id)? {
    anchors.push(container.id);
    anchors.extend(file_entity(&container).map(|file| EntityId::of(&file)));
  }
  for anchor in anchors {
    for edge in graph.edges_in(anchor, &[EdgeKind::Imports])? {
      let Some(importer) = graph.entity(edge.from)? else { continue };
      if let EntityKey::File { repo, path } = importer.key {
        files.insert((repo, path));
      }
    }
  }
  let mut sites = Vec::new();
  for (repo, path) in files {
    sites.extend(graph.dynamic_sites(&repo, &path)?.iter().cloned());
  }
  if sites.is_empty() {
    return Ok(None);
  }
  sites.sort_by(|left, right| left.location.cmp(&right.location).then_with(|| left.path.cmp(&right.path)));
  sites.dedup();
  let total = sites.len();
  sites.truncate(SITES_SHOWN);
  Ok(Some(Caveat::DynamicMemberCalls { total, sites }))
}

pub(crate) fn load_dynamic_sites(graph: &Graph<'_, '_>, repo: &RepoName, path: &RelPath) -> QueryResult<Rc<Vec<UnresolvedSite>>> {
  let facts = graph.tx().owned_facts(&Owner::File {
    repo: repo.clone(),
    path: path.clone(),
  })?;
  let sites = facts
    .unresolved
    .iter()
    .filter(|reference| matches!(reference.kind, RefKind::Call | RefKind::MethodCall))
    .filter(|reference| !matches!(reference.reason, UnresolvedReason::External { .. }))
    .filter(|reference| reference.path.last().is_some_and(|segment| is_computed_segment(segment)))
    .map(|reference| UnresolvedSite {
      location: Location {
        repo: repo.to_string(),
        path: path.to_string(),
        line: Some(reference.span.start_line),
      },
      path: reference.path.join("."),
      reason: "computed property access".to_owned(),
    })
    .collect();
  Ok(Rc::new(sites))
}
