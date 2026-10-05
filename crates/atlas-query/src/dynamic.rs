use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use atlas_core::{EdgeKind, Entity, EntityId, EntityKey, Owner, RefKind, RelPath, RepoName, UnresolvedReason, UnresolvedRef};

use crate::error::QueryResult;
use crate::graph::{FileKey, FileSites, Graph};
use crate::model::{Caveat, Location, UnresolvedSite};
use crate::queries::common::file_entity;

const SITES_SHOWN: usize = 5;
pub(crate) const COMPUTED_PREFIX: &str = "[";

pub(crate) fn is_computed_segment(segment: &str) -> bool {
  segment.starts_with('[') && segment.ends_with(']')
}

pub(crate) fn dynamic_member_files(graph: &Graph<'_, '_>, target: &Entity) -> QueryResult<BTreeSet<(RepoName, RelPath)>> {
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
    for edge in graph.links_in(anchor, &[EdgeKind::Imports])? {
      let Some(importer) = graph.entity(edge.from)? else { continue };
      if let EntityKey::File { repo, path } = importer.key {
        files.insert((repo, path));
      }
    }
  }
  Ok(files)
}

pub(crate) fn dynamic_member_calls(graph: &Graph<'_, '_>, target: &Entity) -> QueryResult<Option<Caveat>> {
  if target.key.as_symbol().is_none() || !target.kind.is_code_symbol() || !graph.may_have_dynamic_sites()? {
    return Ok(None);
  }
  let files = dynamic_member_files(graph, target)?;
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
  let references = graph.tx().unresolved_in_file_with_name_prefix(repo, path, COMPUTED_PREFIX)?;
  Ok(Rc::new(references.iter().filter_map(|reference| computed_site(repo, path, reference)).collect()))
}

pub(crate) fn all_dynamic_sites(graph: &Graph<'_, '_>) -> QueryResult<FileSites> {
  let mut files: HashMap<FileKey, Vec<UnresolvedSite>> = HashMap::new();
  for found in graph.tx().unresolved_with_name_prefix(COMPUTED_PREFIX)? {
    let Owner::File { repo, path } = found.owner else { continue };
    if let Some(site) = computed_site(&repo, &path, &found.reference) {
      files.entry((repo, path)).or_default().push(site);
    }
  }
  Ok(files.into_iter().map(|(file, sites)| (file, Rc::new(sites))).collect())
}

fn computed_site(repo: &RepoName, path: &RelPath, reference: &UnresolvedRef) -> Option<UnresolvedSite> {
  let computed = matches!(reference.kind, RefKind::Call | RefKind::MethodCall)
    && !matches!(reference.reason, UnresolvedReason::External { .. })
    && reference.path.last().is_some_and(|segment| is_computed_segment(segment));
  computed.then(|| UnresolvedSite {
    location: Location {
      repo: repo.to_string(),
      path: path.to_string(),
      line: Some(reference.span.start_line),
      column: None,
    },
    path: reference.path.join("."),
    reason: "computed property access".to_owned(),
  })
}
