use atlas_core::{ATTR_PARAMS, EdgeKind, Entity, EntityId, EntityKey, EntityKind, LanguageFamily, PackageEcosystem, ParamFact, Visibility};
use atlas_similarity::{Field, SimilarityIndex};

use crate::error::QueryResult;
use crate::graph::Graph;
use crate::model::short_name;
use crate::queries::common::{language, package_key};
use crate::queries::search::lexical_hits;

const LEXICAL_FRACTION: f64 = 0.35;
const SITE: &str = "likely change site";

pub(crate) struct CandidateQuery<'q> {
  pub(crate) text: &'q str,
  pub(crate) shape: Option<&'q str>,
  pub(crate) exclude: Option<EntityId>,
  pub(crate) fetch: usize,
}

pub(crate) struct LexicalCandidate {
  pub(crate) entity: Entity,
  pub(crate) score: f64,
  pub(crate) terms: Vec<String>,
  pub(crate) same_shape: bool,
}

pub(crate) fn lexical_and_signature_candidates(graph: &Graph<'_, '_>, index: &SimilarityIndex, query: &CandidateQuery<'_>) -> QueryResult<Vec<LexicalCandidate>> {
  let hits: Vec<_> = lexical_hits(index, query.text, &[], None, query.fetch)
    .into_iter()
    .filter(|hit| Some(hit.entity) != query.exclude)
    .collect();
  let top = hits.iter().map(|hit| hit.score).fold(0.0_f64, f64::max);
  let mut candidates = Vec::new();
  for hit in hits {
    let name_matches = hit.matched_fields.iter().any(|matched| matched.field == Field::Name);
    if !name_matches || hit.score < LEXICAL_FRACTION * top {
      continue;
    }
    let Some(entity) = graph.entity(hit.entity)? else { continue };
    let same_shape = query.shape.is_some_and(|shape| signature_shape(&entity).as_deref() == Some(shape));
    candidates.push(LexicalCandidate {
      entity,
      score: hit.score,
      terms: hit.direct_terms,
      same_shape,
    });
  }
  Ok(candidates)
}

pub(crate) fn signature_shape(entity: &Entity) -> Option<String> {
  let signature = entity.signature.as_deref()?;
  let params: Vec<ParamFact> = entity
    .attrs
    .get(ATTR_PARAMS)
    .and_then(|value| serde_json::from_value(value.clone()).ok())
    .unwrap_or_default();
  let types: Vec<String> = params
    .iter()
    .filter(|param| !matches!(param.name.as_str(), "self" | "&self" | "&mut self" | "this"))
    .map(|param| param.type_text.as_deref().map_or_else(|| "?".to_owned(), compact))
    .collect();
  let returns = match language(entity) {
    Some(LanguageFamily::Rust) => signature.rsplit_once("->").map(|(_, rest)| rest.split(" where ").next().unwrap_or(rest).to_owned()),
    _ => signature.rsplit_once("):").map(|(_, rest)| rest.to_owned()),
  };
  Some(format!("({}) -> {}", types.join(", "), returns.as_deref().map_or_else(|| "?".to_owned(), compact)))
}

fn compact(text: &str) -> String {
  text.split_whitespace().collect::<Vec<_>>().join("")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Exposure {
  Exported,
  Scoped(String),
  Hidden(&'static str),
  Unknown,
}

fn own_exposure(entity: &Entity) -> Exposure {
  match &entity.visibility {
    Some(Visibility::Public | Visibility::Exported | Visibility::ExportedDefault) => Exposure::Exported,
    Some(Visibility::Crate) => Exposure::Scoped("pub(crate)".to_owned()),
    Some(Visibility::Restricted(path)) => Exposure::Scoped(format!("pub({path})")),
    Some(Visibility::Private) => Exposure::Hidden("private"),
    Some(Visibility::Module) => Exposure::Hidden("not exported"),
    Some(Visibility::Protected) => Exposure::Hidden("protected"),
    None => Exposure::Unknown,
  }
}

fn exposure(graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<Exposure> {
  let own = own_exposure(entity);
  if matches!(own, Exposure::Hidden(_)) || entity.kind != EntityKind::Method {
    return Ok(own);
  }
  Ok(match graph.parent(entity.id)? {
    Some(owner) => own_exposure(&owner),
    None => own,
  })
}

fn ecosystem(key: &EntityKey) -> Option<&PackageEcosystem> {
  match key {
    EntityKey::Package { ecosystem, .. } => Some(ecosystem),
    _ => None,
  }
}

pub(crate) fn access(graph: &Graph<'_, '_>, entity: &Entity, site: Option<&Entity>) -> QueryResult<String> {
  let exposure = exposure(graph, entity)?;
  let location = match (&entity.repo, &entity.path) {
    (Some(repo), Some(path)) => format!("{repo}:{path}"),
    _ => short_name(&entity.key),
  };
  let package = package_key(entity);
  let site = site.and_then(package_key);
  if let Exposure::Hidden(word) = exposure {
    return Ok(match package.is_some() && package == site {
      true => format!("{word} in {location}, same package as the {SITE}"),
      false => format!("{word} in {location}"),
    });
  }
  let Some(package) = package else {
    return Ok(match exposure {
      Exposure::Exported => format!("exported from {location}"),
      Exposure::Scoped(scope) => format!("{scope} in {location}"),
      _ => format!("in {location} (visibility unknown)"),
    });
  };
  let base = match &exposure {
    Exposure::Exported if ecosystem(&package) == Some(&PackageEcosystem::Cargo) => format!("pub in {}", package.display_name()),
    Exposure::Exported => format!("exported from {}", package.display_name()),
    Exposure::Scoped(scope) => format!("{scope} in {}", package.display_name()),
    _ => format!("in {} (visibility unknown)", package.display_name()),
  };
  let Some(site) = site.filter(|site| ecosystem(site) == ecosystem(&package)) else {
    return Ok(base);
  };
  if site == package {
    return Ok(format!("{base} ({SITE})"));
  }
  if let Exposure::Scoped(_) = exposure {
    return Ok(format!("{base}, not importable from {} ({SITE})", site.display_name()));
  }
  let target = EntityId::of(&package);
  let depends = graph.edges_out(EntityId::of(&site), &[EdgeKind::DependsOnPackage])?.iter().any(|edge| edge.to == target);
  Ok(match depends {
    true => format!("{base}, a dependency of {} ({SITE})", site.display_name()),
    false => format!("{base}, not a dependency of {} ({SITE})", site.display_name()),
  })
}
