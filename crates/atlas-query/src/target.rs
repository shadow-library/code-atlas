use atlas_core::{Entity, EntityId, EntityKey, EntityKind, HttpMethod, RelPath, RepoName, UnresolvedReason, normalize_route};
use atlas_store::EntityFilter;

use crate::error::{QueryError, QueryResult};
use crate::graph::Graph;
use crate::model::{EntityRef, Targeted};
use crate::text::counted;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSpec {
  Key(EntityKey),
  Id(EntityId),
  Endpoint { method: Option<HttpMethod>, route: String },
  File { repo: Option<String>, path: String, line: Option<u32> },
  Name { qualifier: Vec<String>, name: String },
}

pub(crate) const SOURCE_EXTENSIONS: [&str; 10] = [".rs", ".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs", ".json"];

impl TargetSpec {
  pub fn parse(text: &str) -> QueryResult<Self> {
    let text = text.trim();
    if text.is_empty() {
      return Err(QueryError::invalid("empty target"));
    }
    if let Ok(key) = text.parse::<EntityKey>() {
      return Ok(Self::Key(key));
    }
    if let Ok(id) = text.parse::<EntityId>() {
      return Ok(Self::Id(id));
    }
    if let Some(endpoint) = parse_endpoint(text) {
      return Ok(endpoint);
    }
    if let Some(file) = parse_file(text) {
      return Ok(file);
    }
    parse_name(text).ok_or_else(|| QueryError::invalid(format!("cannot parse target {text:?}")))
  }
}

fn parse_endpoint(text: &str) -> Option<TargetSpec> {
  if text.starts_with('/') {
    return Some(TargetSpec::Endpoint {
      method: None,
      route: normalize_route(text),
    });
  }
  let (method, route) = text.split_once(' ')?;
  let route = route.trim();
  if !route.starts_with('/') {
    return None;
  }
  Some(TargetSpec::Endpoint {
    method: Some(HttpMethod::parse_lenient(method)?),
    route: normalize_route(route),
  })
}

fn parse_file(text: &str) -> Option<TargetSpec> {
  if text.contains(' ') {
    return None;
  }
  let mut parts: Vec<&str> = text.split(':').collect();
  let line = match parts.last().and_then(|last| last.parse::<u32>().ok()) {
    Some(line) if parts.len() > 1 => {
      parts.pop();
      Some(line)
    }
    _ => None,
  };
  let (repo, path) = match parts.as_slice() {
    [path] => (None, *path),
    [repo, path] => (Some((*repo).to_owned()), *path),
    _ => return None,
  };
  let looks_like_path = path.contains('/') || SOURCE_EXTENSIONS.iter().any(|extension| path.ends_with(extension));
  if !looks_like_path {
    return None;
  }
  Some(TargetSpec::File {
    repo,
    path: path.trim_start_matches("./").to_owned(),
    line,
  })
}

fn parse_name(text: &str) -> Option<TargetSpec> {
  let normalised = rust_impl_path(text).unwrap_or_else(|| text.to_owned());
  let separator = if normalised.contains("::") { "::" } else { "." };
  let mut segments: Vec<String> = normalised.split(separator).filter(|segment| !segment.is_empty()).map(str::to_owned).collect();
  let name = segments.pop()?;
  Some(TargetSpec::Name { qualifier: segments, name })
}

fn rust_impl_path(text: &str) -> Option<String> {
  let rest = text.strip_prefix('<')?;
  let (inner, tail) = rest.split_once(">::")?;
  let (self_type, trait_name) = inner.split_once(" as ")?;
  Some(format!("{}::<{}>::{tail}", self_type.trim(), trait_name.trim()))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetFilter {
  pub kinds: Vec<EntityKind>,
  pub repo: Option<String>,
}

pub(crate) enum Resolution {
  Found(Box<Entity>),
  Ambiguous(Vec<Entity>),
  NotFound(String),
}

impl Resolution {
  pub fn into_targeted<T>(self, spec: &str) -> Result<Entity, Targeted<T>> {
    match self {
      Self::Found(entity) => Ok(*entity),
      Self::Ambiguous(candidates) => Err(Targeted::Ambiguous {
        spec: spec.to_owned(),
        candidates: candidates.iter().map(EntityRef::of).collect(),
      }),
      Self::NotFound(hint) => Err(Targeted::NotFound { spec: spec.to_owned(), hint }),
    }
  }
}

pub(crate) fn resolve(graph: &Graph<'_, '_>, repos: &[RepoName], text: &str, filter: &TargetFilter) -> QueryResult<Resolution> {
  let spec = TargetSpec::parse(text)?;
  let mut outside = None;
  let candidates = match &spec {
    TargetSpec::Key(key) => graph.tx().entity_by_key(key)?.into_iter().collect(),
    TargetSpec::Id(id) => graph.entity(*id)?.into_iter().collect(),
    TargetSpec::Endpoint { method, route } => endpoints(graph, *method, route)?,
    TargetSpec::File { repo, path, line } => file_targets(graph, repos, repo.as_deref(), path, *line, &mut outside)?,
    TargetSpec::Name { qualifier, name } => named(graph, qualifier, name)?,
  };
  let candidates: Vec<Entity> = candidates.into_iter().filter(|entity| admits(filter, entity)).collect();
  let candidates = prefer_definitions(candidates);
  Ok(match candidates.len() {
    0 => Resolution::NotFound(match outside {
      Some(hint) => hint,
      None => not_found_hint(graph, &spec, text)?,
    }),
    1 => Resolution::Found(Box::new(candidates.into_iter().next().ok_or_else(|| QueryError::invalid("empty candidate list"))?)),
    _ => Resolution::Ambiguous(sorted(candidates)),
  })
}

fn admits(filter: &TargetFilter, entity: &Entity) -> bool {
  let kind_ok = filter.kinds.is_empty() || filter.kinds.contains(&entity.kind);
  let repo_ok = filter
    .repo
    .as_ref()
    .is_none_or(|repo| entity.repo.as_ref().is_some_and(|candidate| candidate.as_str() == repo));
  kind_ok && repo_ok
}

pub(crate) fn is_test_block(entity: &Entity) -> bool {
  matches!(entity.kind, EntityKind::Test | EntityKind::TestSuite)
}

pub(crate) fn prefer_definitions(candidates: Vec<Entity>) -> Vec<Entity> {
  if candidates.iter().all(is_test_block) {
    return candidates;
  }
  candidates.into_iter().filter(|entity| !is_test_block(entity)).collect()
}

fn sorted(mut candidates: Vec<Entity>) -> Vec<Entity> {
  candidates.sort_by(|left, right| left.key.cmp(&right.key));
  candidates
}

pub(crate) fn not_found_hint(graph: &Graph<'_, '_>, spec: &TargetSpec, text: &str) -> QueryResult<String> {
  Ok(match spec {
    TargetSpec::Endpoint { .. } => format!("no endpoint matches {text}; list endpoints with the api query"),
    TargetSpec::File { .. } => format!("no indexed file or symbol at {text}"),
    TargetSpec::Name { name, .. } => match macro_call_sites(graph, name)? {
      Some(macros) => format!("no entity matches {text}; {macros}"),
      None => format!("no entity matches {text}; try a search for {text}"),
    },
    _ => format!("no entity matches {text}; try a search for {text}"),
  })
}

fn macro_call_sites(graph: &Graph<'_, '_>, name: &str) -> QueryResult<Option<String>> {
  let mut sites: Vec<String> = graph
    .tx()
    .unresolved_by_names(&[name])?
    .into_iter()
    .map(|found| found.reference)
    .filter(|reference| matches!(&reference.reason, UnresolvedReason::Unsupported { detail } if detail == "macro_generated"))
    .map(|reference| format!("{}:{}:{}", reference.repo, reference.file, reference.span.start_line))
    .collect();
  sites.sort();
  let Some(first) = sites.first() else {
    return Ok(None);
  };
  Ok(Some(format!(
    "`{name}` looks macro-generated ({}, e.g. {first}); macro-generated items have no symbol: search for the macro definition",
    counted(sites.len(), "macro call site")
  )))
}

fn anonymous_route(route: &str) -> String {
  route
    .split('/')
    .map(|segment| if segment.starts_with('{') && segment.ends_with('}') { "{}" } else { segment })
    .collect::<Vec<_>>()
    .join("/")
}

fn endpoints(graph: &Graph<'_, '_>, method: Option<HttpMethod>, route: &str) -> QueryResult<Vec<Entity>> {
  let wanted = anonymous_route(route);
  let found = graph
    .entities_of_kind(&EntityKind::ApiEndpoint)?
    .into_iter()
    .filter(|entity| match &entity.key {
      EntityKey::Endpoint { method: candidate, route, .. } => anonymous_route(&normalize_route(route)) == wanted && method.is_none_or(|method| method == *candidate),
      _ => false,
    })
    .collect();
  Ok(found)
}

fn file_targets(graph: &Graph<'_, '_>, repos: &[RepoName], repo: Option<&str>, path: &str, line: Option<u32>, outside: &mut Option<String>) -> QueryResult<Vec<Entity>> {
  let Ok(path) = RelPath::new(path) else {
    return Ok(Vec::new());
  };
  let mut found = Vec::new();
  for candidate in repos.iter().filter(|candidate| repo.is_none_or(|repo| candidate.as_str() == repo)) {
    let Some(file) = graph.tx().entity_by_key(&EntityKey::File {
      repo: candidate.clone(),
      path: path.clone(),
    })?
    else {
      continue;
    };
    let Some(line) = line else {
      found.push(file);
      continue;
    };
    let symbols: Vec<Entity> = graph
      .tx()
      .entities_in_file(candidate, &path, &EntityFilter::default())?
      .into_iter()
      .filter(|entity| entity.kind.is_code_symbol())
      .collect();
    let total = symbols.len();
    let innermost = symbols
      .into_iter()
      .filter(|entity| entity.span.is_some_and(|span| span.start_line <= line && line <= span.end_line))
      .min_by_key(|entity| entity.span.map(|span| (span.end_byte - span.start_byte, span.start_byte)));
    match innermost {
      Some(symbol) => found.push(symbol),
      None => {
        *outside = Some(format!(
          "line {line} is outside any symbol in {candidate}:{path}; the file has {total} symbols (target the file itself to aggregate them)"
        ))
      }
    }
  }
  Ok(found)
}

fn named(graph: &Graph<'_, '_>, qualifier: &[String], name: &str) -> QueryResult<Vec<Entity>> {
  let found = graph
    .tx()
    .entities_by_name(name, &EntityFilter::default())?
    .into_iter()
    .filter(|entity| entity.kind != EntityKind::File && qualifier_matches(entity, qualifier))
    .collect();
  Ok(found)
}

fn qualifier_matches(entity: &Entity, qualifier: &[String]) -> bool {
  if qualifier.is_empty() {
    return true;
  }
  let EntityKey::Symbol(symbol) = &entity.key else {
    return false;
  };
  let path: Vec<&str> = symbol.module_path().iter().chain(symbol.item_path()).map(String::as_str).collect();
  let Some((_, owners)) = path.split_last() else { return false };
  let names_trait = qualifier.iter().any(|segment| is_trait_segment(segment));
  let owners: Vec<&str> = owners.iter().copied().filter(|owner| names_trait || !is_trait_segment(owner)).collect();
  owners.len() >= qualifier.len() && owners[owners.len() - qualifier.len()..].iter().zip(qualifier).all(|(owner, wanted)| *owner == wanted)
}

fn is_trait_segment(segment: &str) -> bool {
  segment.starts_with('<') && segment.ends_with('>')
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_parse_every_target_form() {
    assert_eq!(
      TargetSpec::parse("UsersService.findOne").unwrap(),
      TargetSpec::Name {
        qualifier: vec!["UsersService".into()],
        name: "findOne".into()
      }
    );
    assert_eq!(
      TargetSpec::parse("AcmeApiClient::get_order").unwrap(),
      TargetSpec::Name {
        qualifier: vec!["AcmeApiClient".into()],
        name: "get_order".into()
      }
    );
    assert_eq!(
      TargetSpec::parse("<PgInvoiceStore as InvoiceStore>::insert").unwrap(),
      TargetSpec::Name {
        qualifier: vec!["PgInvoiceStore".into(), "<InvoiceStore>".into()],
        name: "insert".into()
      }
    );
    assert_eq!(
      TargetSpec::parse("acme-api:src/users/users.service.ts:18").unwrap(),
      TargetSpec::File {
        repo: Some("acme-api".into()),
        path: "src/users/users.service.ts".into(),
        line: Some(18)
      }
    );
    assert_eq!(
      TargetSpec::parse("src/money.rs").unwrap(),
      TargetSpec::File {
        repo: None,
        path: "src/money.rs".into(),
        line: None
      }
    );
    assert_eq!(
      TargetSpec::parse("GET /users/:id").unwrap(),
      TargetSpec::Endpoint {
        method: Some(HttpMethod::Get),
        route: "/users/{id}".into()
      }
    );
    assert_eq!(
      TargetSpec::parse("endpoint:acme-api:GET /users/{id}").unwrap(),
      TargetSpec::Key("endpoint:acme-api:GET /users/{id}".parse().unwrap())
    );
    assert_eq!(
      TargetSpec::parse("toCents").unwrap(),
      TargetSpec::Name {
        qualifier: vec![],
        name: "toCents".into()
      }
    );
    assert!(TargetSpec::parse("  ").is_err());
  }

  #[test]
  fn should_ignore_parameter_names_when_matching_routes() {
    assert_eq!(anonymous_route("/users/{id}"), anonymous_route("/users/{userId}"));
    assert_ne!(anonymous_route("/users/{id}"), anonymous_route("/users/me"));
  }
}
