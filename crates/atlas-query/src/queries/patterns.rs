use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{ATTR_EXTENDS, ATTR_TEST, EdgeKind, Entity, EntityId, EntityKind, TestInfo};
use serde::Serialize;

use crate::QueryEngine;
use crate::engine::{Snippet, SnippetMode};
use crate::error::QueryResult;
use crate::graph::Graph;
use crate::model::{EntityRef, Response};
use crate::policy::Policy;
use crate::queries::common::{attr_str, attribute_names, is_test, language};
use crate::queries::search::lexical_hits;

const EXEMPLARS_PER_SHAPE: usize = 3;
const EXCERPT_LINES: usize = 25;
const DEFAULT_SHAPES: usize = 8;
const FREE_TEXT_HITS: usize = 40;

#[derive(Debug, Clone, PartialEq)]
pub struct PatternsRequest {
  pub topic: String,
  pub limit: usize,
  pub excerpts: bool,
}

impl PatternsRequest {
  pub fn new(topic: impl Into<String>) -> Self {
    Self {
      topic: topic.into(),
      limit: DEFAULT_SHAPES,
      excerpts: true,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternTopic {
  Endpoint,
  Auth,
  ErrorHandling,
  Config,
  Test,
  Db,
  Event,
  FreeText,
}

impl PatternTopic {
  pub fn parse(topic: &str) -> Self {
    match topic.trim().to_ascii_lowercase().as_str() {
      "endpoint" | "endpoints" | "route" | "routes" | "api" => Self::Endpoint,
      "auth" | "authentication" | "authorization" | "guard" | "guards" => Self::Auth,
      "error-handling" | "error" | "errors" | "error_handling" => Self::ErrorHandling,
      "config" | "configuration" | "env" => Self::Config,
      "test" | "tests" | "testing" => Self::Test,
      "db" | "database" | "orm" => Self::Db,
      "event" | "events" | "messaging" => Self::Event,
      _ => Self::FreeText,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Exemplar {
  pub entity: EntityRef,
  pub signature: Option<String>,
  pub excerpt: Option<Snippet>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PatternShape {
  pub shape: String,
  pub count: usize,
  pub exemplars: Vec<Exemplar>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PatternsAnswer {
  pub topic: PatternTopic,
  pub query: String,
  pub instances: usize,
  pub shapes: Vec<PatternShape>,
  pub omitted_shapes: usize,
}

impl QueryEngine {
  pub fn patterns(&self, request: &PatternsRequest) -> QueryResult<Response<PatternsAnswer>> {
    let mut freshness = self.barrier()?;
    let topic = PatternTopic::parse(&request.topic);
    let index = self.engine().similarity();
    let mut answer = self.read(Policy::with_floor(0.0), |graph| -> QueryResult<PatternsAnswer> {
      let instances = match topic {
        PatternTopic::FreeText => free_text(graph, &index, &request.topic)?,
        known => instances(graph, known)?,
      };
      Ok(group(topic, &request.topic, instances, request.limit))
    })?;
    if request.excerpts {
      let mut reader = self.sources(&mut freshness);
      for shape in &mut answer.shapes {
        for exemplar in &mut shape.exemplars {
          exemplar.excerpt = Some(reader.snippet(exemplar.entity.entity_id()?, SnippetMode::Body { max_lines: Some(EXCERPT_LINES) }, 0)?);
        }
      }
    }
    Ok(Response { freshness, answer })
  }
}

pub(crate) struct Instance {
  pub entity: Entity,
  pub shape: String,
}

fn group(topic: PatternTopic, query: &str, instances: Vec<Instance>, limit: usize) -> PatternsAnswer {
  let total = instances.len();
  let mut shapes: BTreeMap<String, Vec<Entity>> = BTreeMap::new();
  let mut seen: BTreeSet<(String, EntityId)> = BTreeSet::new();
  for instance in instances {
    if seen.insert((instance.shape.clone(), instance.entity.id)) {
      shapes.entry(instance.shape).or_default().push(instance.entity);
    }
  }
  let mut ranked: Vec<PatternShape> = shapes
    .into_iter()
    .map(|(shape, mut entities)| {
      entities.sort_by_key(|entity| EntityRef::of(entity).sort_key());
      PatternShape {
        shape,
        count: entities.len(),
        exemplars: entities
          .iter()
          .take(EXEMPLARS_PER_SHAPE)
          .map(|entity| Exemplar {
            entity: EntityRef::of(entity),
            signature: entity.signature.clone(),
            excerpt: None,
          })
          .collect(),
      }
    })
    .collect();
  ranked.sort_by(|left, right| right.count.cmp(&left.count).then_with(|| left.shape.cmp(&right.shape)));
  let omitted_shapes = ranked.len().saturating_sub(limit);
  ranked.truncate(limit);
  PatternsAnswer {
    topic,
    query: query.to_owned(),
    instances: total,
    shapes: ranked,
    omitted_shapes,
  }
}

const HTTP_VERB_DECORATORS: [&str; 9] = ["Get", "Post", "Put", "Patch", "Delete", "Head", "Options", "All", "Route"];

fn decorators(entity: &Entity) -> String {
  let mut names: Vec<String> = attribute_names(entity)
    .into_iter()
    .map(|name| match HTTP_VERB_DECORATORS.contains(&name.as_str()) {
      true => "@<Verb>".to_owned(),
      false => format!("@{name}"),
    })
    .collect();
  names.sort();
  names.dedup();
  match names.is_empty() {
    true => "registered without decorators".to_owned(),
    false => names.join(" "),
  }
}

fn file_convention(entity: &Entity) -> String {
  let Some(path) = &entity.path else { return "unknown file".to_owned() };
  let file = path.file_name();
  if path.segments().next() == Some("tests") {
    return "tests/ directory".to_owned();
  }
  for marker in [".spec.", ".test."] {
    if let Some((_, extension)) = file.split_once(marker) {
      return format!("*{marker}{extension}");
    }
  }
  if path.segments().any(|segment| segment == "__tests__") {
    return "__tests__ directory".to_owned();
  }
  match language(entity) {
    Some(atlas_core::LanguageFamily::Rust) => "inline #[cfg(test)] module".to_owned(),
    _ => "colocated in source file".to_owned(),
  }
}

pub(crate) fn instances(graph: &Graph<'_, '_>, topic: PatternTopic) -> QueryResult<Vec<Instance>> {
  let mut found = Vec::new();
  match topic {
    PatternTopic::Endpoint => {
      for endpoint in graph.entities_of_kind(&EntityKind::ApiEndpoint)? {
        let framework = attr_str(&endpoint, "framework").unwrap_or("unknown framework").to_owned();
        for edge in graph.edges_in(endpoint.id, &[EdgeKind::ExposesEndpoint])? {
          let Some(handler) = graph.entity(edge.from)? else { continue };
          let shape = format!("{framework} handler, {}", decorators(&handler));
          found.push(Instance { entity: handler, shape });
        }
      }
    }
    PatternTopic::Auth => {
      for scheme in graph.entities_of_kind(&EntityKind::AuthScheme)? {
        for edge in graph.edges_in(scheme.id, &[EdgeKind::ImplementsAuth])? {
          let Some(guard) = graph.entity(edge.from)? else { continue };
          let kind = edge.attrs.get("guard_kind").and_then(serde_json::Value::as_str).unwrap_or("auth implementation");
          found.push(Instance {
            entity: guard,
            shape: format!("{kind} implementing {}", scheme.name),
          });
        }
      }
    }
    PatternTopic::ErrorHandling => {
      for kind in [EntityKind::Class, EntityKind::Struct, EntityKind::Enum] {
        for entity in graph.entities_of_kind(&kind)? {
          let extends: Vec<String> = entity
            .attrs
            .get(ATTR_EXTENDS)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
          let named = entity.name.ends_with("Error") || entity.name.ends_with("Exception");
          let extends_error = extends.iter().any(|base| base.ends_with("Error") || base.ends_with("Exception"));
          if !named && !extends_error {
            continue;
          }
          let shape = match extends.first() {
            Some(base) => format!("{} extending {base}", entity.kind),
            None => format!("{} error type", entity.kind),
          };
          found.push(Instance { entity, shape });
        }
      }
    }
    PatternTopic::Config => readers(
      graph,
      &mut found,
      &[EntityKind::EnvVar, EntityKind::ConfigKey],
      &[EdgeKind::ReadsEnv, EdgeKind::ReadsConfig],
    )?,
    PatternTopic::Db => readers(
      graph,
      &mut found,
      &[EntityKind::DbTable],
      &[EdgeKind::MapsToTable, EdgeKind::ReadsTable, EdgeKind::WritesTable],
    )?,
    PatternTopic::Event => readers(graph, &mut found, &[EntityKind::Event, EntityKind::Queue], &[EdgeKind::Publishes, EdgeKind::Subscribes])?,
    PatternTopic::Test => {
      for kind in [EntityKind::TestSuite, EntityKind::Test] {
        for entity in graph.entities_of_kind(&kind)? {
          if graph.parent(entity.id)?.is_some_and(|parent| is_test(&parent)) {
            continue;
          }
          let framework = entity
            .attrs
            .get(ATTR_TEST)
            .and_then(|value| serde_json::from_value::<TestInfo>(value.clone()).ok())
            .map_or_else(|| "unknown framework".to_owned(), |info| info.framework);
          let shape = format!("{framework}, {}", file_convention(&entity));
          found.push(Instance { entity, shape });
        }
      }
    }
    PatternTopic::FreeText => {}
  }
  Ok(found)
}

fn readers(graph: &Graph<'_, '_>, found: &mut Vec<Instance>, kinds: &[EntityKind], edges: &[EdgeKind]) -> QueryResult<()> {
  for kind in kinds {
    for target in graph.entities_of_kind(kind)? {
      for edge in graph.edges_in(target.id, edges)? {
        let Some(reader) = graph.entity(edge.from)? else { continue };
        let detail = ["receiver", "orm", "library"]
          .iter()
          .find_map(|name| edge.attrs.get(*name).and_then(serde_json::Value::as_str).map(|value| format!(" via {value}")))
          .unwrap_or_default();
        found.push(Instance {
          entity: reader,
          shape: format!("{}{detail}", edge.kind.forward_label()),
        });
      }
    }
  }
  Ok(())
}

fn free_text(graph: &Graph<'_, '_>, index: &atlas_similarity::SimilarityIndex, text: &str) -> QueryResult<Vec<Instance>> {
  let mut found = Vec::new();
  for hit in lexical_hits(index, text, &[], None, FREE_TEXT_HITS) {
    let Some(entity) = graph.entity(hit.entity)? else { continue };
    if is_test(&entity) || entity.kind == EntityKind::Field {
      continue;
    }
    let module = entity
      .path
      .as_ref()
      .and_then(|path| path.parent())
      .map_or_else(|| ".".to_owned(), |parent| parent.to_string());
    let repo = entity.repo.as_ref().map(ToString::to_string).unwrap_or_default();
    found.push(Instance {
      shape: format!("{repo}:{module}/ ({})", entity.kind),
      entity,
    });
  }
  Ok(found)
}
