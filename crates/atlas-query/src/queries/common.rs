use atlas_core::{ATTR_ATTRIBUTES, ATTR_TEST, AttributeFact, EdgeKind, Entity, EntityId, EntityKey, EntityKind, LanguageFamily, PackageEcosystem, RelPath};
use atlas_store::EdgeEnd;

use crate::QueryEngine;
use crate::error::QueryResult;
use crate::graph::Graph;
use crate::model::{Caveat, Targeted};
use crate::target::{TargetFilter, resolve};

pub(crate) const DEFAULT_LIMIT: usize = 30;
pub(crate) const MAX_NODES: usize = 2_000;

pub(crate) fn resolve_one<T>(query: &QueryEngine, graph: &Graph<'_, '_>, spec: &str, filter: &TargetFilter) -> QueryResult<Result<Entity, Targeted<T>>> {
  Ok(resolve(graph, &query.repos(), spec, filter)?.into_targeted(spec))
}

pub(crate) fn is_test(entity: &Entity) -> bool {
  matches!(entity.kind, EntityKind::Test | EntityKind::TestSuite) || entity.attrs.contains_key(ATTR_TEST)
}

pub(crate) fn attributes(entity: &Entity) -> Vec<AttributeFact> {
  entity
    .attrs
    .get(ATTR_ATTRIBUTES)
    .and_then(|value| serde_json::from_value::<Vec<AttributeFact>>(value.clone()).ok())
    .unwrap_or_default()
}

pub(crate) fn attribute_names(entity: &Entity) -> Vec<String> {
  let mut names: Vec<String> = attributes(entity).into_iter().map(|attribute| attribute.path.join("::")).collect();
  names.sort();
  names.dedup();
  names
}

pub(crate) fn attr_str<'e>(entity: &'e Entity, name: &str) -> Option<&'e str> {
  entity.attrs.get(name).and_then(serde_json::Value::as_str)
}

pub(crate) fn is_router_relative(endpoint: &Entity) -> bool {
  attr_str(endpoint, "route_scope") == Some("router_relative")
}

pub(crate) fn superseded_fragments(graph: &Graph<'_, '_>) -> QueryResult<std::collections::BTreeSet<String>> {
  Ok(atlas_engine::superseded_fragment_keys(graph.tx())?)
}

pub(crate) fn file_entity(entity: &Entity) -> Option<EntityKey> {
  Some(EntityKey::File {
    repo: entity.repo.clone()?,
    path: entity.path.clone()?,
  })
}

pub(crate) fn package_key(entity: &Entity) -> Option<EntityKey> {
  let EntityKey::Symbol(symbol) = &entity.key else {
    return None;
  };
  let (ecosystem, name) = match symbol.family() {
    LanguageFamily::Rust => (PackageEcosystem::Cargo, symbol.package().split('/').next().unwrap_or_default().to_owned()),
    LanguageFamily::Ts => (PackageEcosystem::Npm, symbol.package().to_owned()),
    _ => return None,
  };
  Some(EntityKey::Package { ecosystem, name })
}

pub(crate) fn language(entity: &Entity) -> Option<LanguageFamily> {
  entity.key.as_symbol().map(|symbol| symbol.family())
}

pub(crate) fn first_line(text: Option<&str>) -> Option<String> {
  let line = text?.lines().map(str::trim).find(|line| !line.is_empty())?;
  Some(line.to_owned())
}

pub(crate) fn link_caveat(graph: &Graph<'_, '_>, kind: EdgeKind, detail: &str) -> QueryResult<Option<Caveat>> {
  let end = match kind {
    EdgeKind::UsesAuth | EdgeKind::CallsEndpoint => EdgeEnd::Target,
    _ => EdgeEnd::Source,
  };
  if graph.tx().edge_kind_meets_entity_kind(&kind, end, &link_anchor(&kind))? {
    return Ok(None);
  }
  Ok(Some(Caveat::MissingFacts { detail: detail.to_owned() }))
}

fn link_anchor(kind: &EdgeKind) -> EntityKind {
  match kind {
    EdgeKind::UsesAuth => EntityKind::AuthScheme,
    _ => EntityKind::ApiEndpoint,
  }
}

pub(crate) fn sibling_test_paths(path: &RelPath) -> Vec<RelPath> {
  let file = path.file_name();
  let Some((stem, extension)) = file.rsplit_once('.') else {
    return Vec::new();
  };
  let directory = path.parent();
  let join = |name: String| match &directory {
    Some(directory) => directory.join(&name).ok(),
    None => RelPath::new(&name).ok(),
  };
  let mut candidates: Vec<RelPath> = ["spec", "test"].iter().filter_map(|marker| join(format!("{stem}.{marker}.{extension}"))).collect();
  candidates.extend(["spec", "test"].iter().filter_map(|marker| join(format!("__tests__/{stem}.{marker}.{extension}"))));
  candidates.extend(join(format!("__tests__/{stem}.{extension}")));
  candidates
}

pub(crate) fn file_symbols(graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<Option<Vec<EntityId>>> {
  let EntityKey::File { repo, path } = &entity.key else { return Ok(None) };
  let symbols = graph
    .tx()
    .entities_in_file(repo, path, &atlas_store::EntityFilter::default())?
    .into_iter()
    .filter(|symbol| symbol.kind.is_code_symbol())
    .map(|symbol| symbol.id)
    .collect();
  Ok(Some(symbols))
}
