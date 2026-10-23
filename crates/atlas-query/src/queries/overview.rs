use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{EdgeKind, Entity, EntityId, EntityKey, EntityKind, ParseStatus};
use atlas_store::EntityFilter;
use serde::Serialize;

use crate::QueryEngine;
use crate::error::QueryResult;
use crate::graph::Graph;
use crate::model::{AttrView, CertaintyTag, EntityRef, Response, non_authoritative};
use crate::policy::Policy;
use crate::queries::RepoDocs;
use crate::queries::common::{attr_str, attribute_names, is_router_relative, superseded_fragments};
use crate::queries::patterns::{PatternTopic, instances};

const TOP_MODULES: usize = 8;
const PATTERN_SHAPES: usize = 3;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RepoSummary {
  pub repo: String,
  pub files: usize,
  pub languages: BTreeMap<String, usize>,
  pub problem_files: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PackageSummary {
  pub package: EntityRef,
  pub repo: String,
  pub ecosystem: String,
  pub in_project_dependencies: Vec<String>,
  pub external_dependencies: usize,
  pub dependents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApplicationSummary {
  pub application: EntityRef,
  pub certainty: CertaintyTag,
  pub basis: String,
  pub basis_source: Option<AttrView>,
  pub entrypoints: Vec<EntityRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModuleFanIn {
  pub file: String,
  pub importers: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DomainSummary {
  pub repo: String,
  pub endpoints: usize,
  #[serde(skip_serializing_if = "is_zero")]
  pub router_fragments: usize,
  pub auth_schemes: Vec<String>,
  #[serde(skip_serializing_if = "BTreeMap::is_empty")]
  pub auth_scheme_certainty: BTreeMap<String, CertaintyTag>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventSummary {
  pub event: String,
  pub publishers: usize,
  pub subscribers: usize,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub certainty: Option<Box<CertaintyTag>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OverviewAnswer {
  pub project: String,
  pub repos: Vec<RepoSummary>,
  pub packages: Vec<PackageSummary>,
  pub applications: Vec<ApplicationSummary>,
  pub top_modules: Vec<ModuleFanIn>,
  pub apis: Vec<DomainSummary>,
  pub events: Vec<EventSummary>,
  pub patterns: Vec<String>,
  pub health: Vec<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub docs: Option<Vec<RepoDocs>>,
}

impl QueryEngine {
  pub fn overview(&self) -> QueryResult<Response<OverviewAnswer>> {
    self.overview_with(false)
  }

  pub fn overview_with(&self, docs: bool) -> QueryResult<Response<OverviewAnswer>> {
    let freshness = self.barrier()?;
    let status = self.engine().status()?;
    let project = self.engine().project().name.to_string();
    let repos = self.repos();
    let mut answer = self.read(Policy::with_floor(0.0), |graph| -> QueryResult<OverviewAnswer> {
      let mut summaries = Vec::new();
      for repo in &repos {
        let files = graph.tx().files_in_repo(repo)?;
        let mut languages: BTreeMap<String, usize> = BTreeMap::new();
        for file in &files {
          if let Some(language) = file.language {
            *languages.entry(language.as_str().to_owned()).or_default() += 1;
          }
        }
        summaries.push(RepoSummary {
          repo: repo.to_string(),
          files: files.len(),
          languages,
          problem_files: files.iter().filter(|file| !matches!(file.parse_status, ParseStatus::Ok)).count(),
        });
      }
      Ok(OverviewAnswer {
        project: project.clone(),
        repos: summaries,
        packages: packages(graph)?,
        applications: applications(graph)?,
        top_modules: top_modules(graph)?,
        apis: apis(graph)?,
        events: events(graph)?,
        patterns: patterns(graph)?,
        health: Vec::new(),
        docs: docs.then(|| self.doc_map(graph)).transpose()?,
      })
    })?;
    let mut health = vec![format!(
      "{} unresolved references: {}",
      status.unresolved_by_reason.values().sum::<u64>(),
      status
        .unresolved_by_reason
        .iter()
        .map(|(reason, count)| format!("{reason} {count}"))
        .collect::<Vec<_>>()
        .join(", ")
    )];
    health.push(format!("scip coverage {}/{} files", status.scip.files_with_scip, status.scip.files_total));
    if !status.failed_files.is_empty() || !status.repos_missing.is_empty() {
      health.push(format!("{} failed files, {} missing repos", status.failed_files.len(), status.repos_missing.len()));
    }
    health.push(format!(
      "inferred facts current {} stale {} needs revalidation {}",
      status.inferred.current, status.inferred.stale, status.inferred.needs_revalidation
    ));
    answer.health = health;
    Ok(Response { freshness, answer })
  }
}

fn packages(graph: &Graph<'_, '_>) -> QueryResult<Vec<PackageSummary>> {
  let mut summaries = Vec::new();
  for package in graph.entities_of_kind(&EntityKind::Package)? {
    let EntityKey::Package { ecosystem, .. } = &package.key else { continue };
    let mut in_project = Vec::new();
    let mut external = 0;
    for edge in graph.edges_out(package.id, &[EdgeKind::DependsOnPackage])? {
      match graph.entity(edge.to)? {
        Some(dependency) if dependency.kind == EntityKind::Package => in_project.push(dependency.name),
        _ => external += 1,
      }
    }
    let mut dependents = Vec::new();
    for edge in graph.edges_in(package.id, &[EdgeKind::DependsOnPackage])? {
      if let Some(dependent) = graph.entity(edge.from)?.filter(|dependent| dependent.kind == EntityKind::Package) {
        dependents.push(dependent.name);
      }
    }
    in_project.sort();
    dependents.sort();
    summaries.push(PackageSummary {
      repo: attr_str(&package, "repo").unwrap_or_default().to_owned(),
      package: EntityRef::of(&package),
      ecosystem: ecosystem.as_str().to_owned(),
      in_project_dependencies: in_project,
      external_dependencies: external,
      dependents,
    });
  }
  summaries.sort_by(|left, right| (&left.repo, &left.package.key).cmp(&(&right.repo, &right.package.key)));
  Ok(summaries)
}

fn applications(graph: &Graph<'_, '_>) -> QueryResult<Vec<ApplicationSummary>> {
  let mains: Vec<Entity> = graph
    .tx()
    .entities_by_name("main", &EntityFilter::default())?
    .into_iter()
    .filter(|entity| entity.kind == EntityKind::Function)
    .collect();
  let mut summaries = Vec::new();
  for application in graph.entities_of_kind(&EntityKind::Application)? {
    let evidence = application.attrs.get("evidence");
    let field = |name: &str| evidence.and_then(|evidence| evidence.get(name)).and_then(serde_json::Value::as_str).map(str::to_owned);
    let basis = match (field("basis"), field("reason"), field("call"), field("file")) {
      (Some(basis), Some(reason), _, _) => format!("{basis}: {reason}"),
      (Some(basis), None, Some(call), Some(file)) => format!("{basis}: {call} in {file}"),
      (Some(basis), _, _, _) => basis,
      _ => "unknown".to_owned(),
    };
    let mut entrypoints: Vec<EntityRef> = mains.iter().filter(|main| main.repo == application.repo).map(EntityRef::of).collect();
    if let (Some(repo), Some(file)) = (&application.repo, field("file"))
      && let Ok(path) = atlas_core::RelPath::new(&file)
      && let Some(file) = graph.tx().entity_by_key(&EntityKey::File { repo: repo.clone(), path })?
    {
      entrypoints.push(EntityRef::of(&file));
    }
    entrypoints.sort_by_key(EntityRef::sort_key);
    entrypoints.dedup();
    summaries.push(ApplicationSummary {
      application: EntityRef::of(&application),
      certainty: CertaintyTag::of(&application.certainty),
      basis,
      basis_source: graph.entity_attrs(application.id, &["evidence"])?.into_iter().next(),
      entrypoints,
    });
  }
  summaries.sort_by(|left, right| left.application.key.cmp(&right.application.key));
  Ok(summaries)
}

fn top_modules(graph: &Graph<'_, '_>) -> QueryResult<Vec<ModuleFanIn>> {
  let mut fan_in = Vec::new();
  for file in graph.entities_of_kind(&EntityKind::File)? {
    let EntityKey::File { repo, path } = &file.key else { continue };
    let mut importers: BTreeSet<EntityId> = BTreeSet::new();
    let mut targets = vec![file.id];
    targets.extend(graph.tx().entities_in_file(repo, path, &EntityFilter::default())?.into_iter().map(|entity| entity.id));
    for target in targets {
      for edge in graph.edges_in(target, &[EdgeKind::Imports])? {
        if edge.from != file.id {
          importers.insert(edge.from);
        }
      }
    }
    if !importers.is_empty() {
      fan_in.push(ModuleFanIn {
        file: format!("{repo}:{path}"),
        importers: importers.len(),
      });
    }
  }
  fan_in.sort_by(|left, right| right.importers.cmp(&left.importers).then_with(|| left.file.cmp(&right.file)));
  fan_in.truncate(TOP_MODULES);
  Ok(fan_in)
}

fn apis(graph: &Graph<'_, '_>) -> QueryResult<Vec<DomainSummary>> {
  let mut by_repo: BTreeMap<String, DomainSummary> = BTreeMap::new();
  let superseded = superseded_fragments(graph)?;
  for endpoint in graph.entities_of_kind(&EntityKind::ApiEndpoint)? {
    let fragment = is_router_relative(&endpoint);
    if fragment && superseded.contains(&endpoint.key.to_string()) {
      continue;
    }
    let repo = endpoint.repo.as_ref().map(ToString::to_string).unwrap_or_default();
    let summary = by_repo.entry(repo.clone()).or_insert_with(|| DomainSummary {
      repo,
      endpoints: 0,
      router_fragments: 0,
      auth_schemes: Vec::new(),
      auth_scheme_certainty: BTreeMap::new(),
    });
    match fragment {
      true => summary.router_fragments += 1,
      false => summary.endpoints += 1,
    }
  }
  for scheme in graph.entities_of_kind(&EntityKind::AuthScheme)? {
    let repo = scheme.repo.as_ref().map(ToString::to_string).unwrap_or_default();
    let summary = by_repo.entry(repo.clone()).or_insert_with(|| DomainSummary {
      repo,
      endpoints: 0,
      router_fragments: 0,
      auth_schemes: Vec::new(),
      auth_scheme_certainty: BTreeMap::new(),
    });
    if let Some(tag) = non_authoritative(&scheme.certainty) {
      summary.auth_scheme_certainty.insert(scheme.name.clone(), *tag);
    }
    summary.auth_schemes.push(scheme.name);
  }
  let mut summaries: Vec<DomainSummary> = by_repo.into_values().collect();
  for summary in &mut summaries {
    summary.auth_schemes.sort();
  }
  Ok(summaries)
}

fn events(graph: &Graph<'_, '_>) -> QueryResult<Vec<EventSummary>> {
  let mut summaries = Vec::new();
  for event in graph.entities_of_kind(&EntityKind::Event)? {
    summaries.push(EventSummary {
      publishers: graph.edges_in(event.id, &[EdgeKind::Publishes])?.len(),
      subscribers: graph.edges_in(event.id, &[EdgeKind::Subscribes])?.len(),
      certainty: non_authoritative(&event.certainty),
      event: event.name,
    });
  }
  summaries.sort_by(|left, right| left.event.cmp(&right.event));
  Ok(summaries)
}

fn shape_summary(graph: &Graph<'_, '_>, topic: PatternTopic, label: &str) -> QueryResult<Option<String>> {
  let mut counts: BTreeMap<String, usize> = BTreeMap::new();
  for instance in instances(graph, topic)? {
    let shape = match topic {
      PatternTopic::Endpoint => instance.shape.split(',').next().unwrap_or_default().to_owned(),
      PatternTopic::Auth => instance.shape.split(" implementing").next().unwrap_or_default().to_owned(),
      _ => instance.shape,
    };
    *counts.entry(shape).or_default() += 1;
  }
  if counts.is_empty() {
    return Ok(None);
  }
  let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
  ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
  let shapes: Vec<String> = ranked.iter().take(PATTERN_SHAPES).map(|(shape, count)| format!("{shape} ({count})")).collect();
  let more = ranked.len().saturating_sub(PATTERN_SHAPES);
  let suffix = if more > 0 { format!(", {more} more") } else { String::new() };
  Ok(Some(format!("{label}: {}{suffix}", shapes.join(", "))))
}

fn patterns(graph: &Graph<'_, '_>) -> QueryResult<Vec<String>> {
  let mut lines = Vec::new();
  for (topic, label) in [
    (PatternTopic::Endpoint, "HTTP endpoints"),
    (PatternTopic::Auth, "auth"),
    (PatternTopic::Test, "tests"),
    (PatternTopic::Db, "data access"),
    (PatternTopic::Event, "events"),
    (PatternTopic::Config, "config"),
  ] {
    lines.extend(shape_summary(graph, topic, label)?);
  }
  let mut injected = 0;
  for class in graph.entities_of_kind(&EntityKind::Class)? {
    let decorated = attribute_names(&class).iter().any(|name| matches!(name.as_str(), "Injectable" | "Controller"));
    if !decorated {
      continue;
    }
    let constructor = graph.children(class.id)?.into_iter().find(|child| child.name == "constructor");
    if constructor.is_some_and(|constructor| {
      constructor
        .attrs
        .get(atlas_core::ATTR_PARAMS)
        .and_then(serde_json::Value::as_array)
        .is_some_and(|params| !params.is_empty())
    }) {
      injected += 1;
    }
  }
  if injected > 0 {
    lines.push(format!("DI via constructor injection ({injected} decorated classes)"));
  }
  let frameworks: BTreeSet<String> = graph
    .entities_of_kind(&EntityKind::ApiEndpoint)?
    .iter()
    .filter_map(|endpoint| attr_str(endpoint, "framework").map(str::to_owned))
    .collect();
  if frameworks.len() > 1 {
    lines.push(format!("multiple HTTP frameworks: {}", frameworks.into_iter().collect::<Vec<_>>().join(", ")));
  }
  Ok(lines)
}

fn is_zero(count: &usize) -> bool {
  *count == 0
}
