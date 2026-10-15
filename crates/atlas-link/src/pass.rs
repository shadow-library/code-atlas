use std::collections::BTreeSet;

use atlas_core::{Certainty, Edge, Entity, EntityKey, EntityKind};
use atlas_project::Project;
use atlas_store::ReadTx;
use serde_json::json;

use crate::error::LinkResult;
use crate::facts::Facts;
use crate::output::Output;
use crate::scopes::PackageIndex;
use crate::{auth, compose, events, http, packages};

pub const PASS_NAME: &str = "link";
const STATUS_KIND: &str = "link_status";
const UNSUPPORTED: [&str; 1] = ["env definition files"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkOutput {
  pub entities: Vec<Entity>,
  pub edges: Vec<Edge>,
}

pub fn link(tx: &ReadTx<'_>, project: &Project) -> LinkResult<LinkOutput> {
  let repos: BTreeSet<_> = project.repos.iter().map(|repo| repo.name.clone()).collect();
  let facts = Facts::new(tx, repos);
  let mut output = Output::default();
  packages::link_packages(&facts, &mut output)?;
  let package_index = PackageIndex::load(&facts)?;
  let routes = compose::compose_routes(&facts, &package_index, &mut output)?;
  let endpoint_auth = auth::link_endpoint_auth(&facts, &routes, &mut output)?;
  let clients = http::link_clients(&facts, &routes, &package_index, &mut output)?;
  auth::link_client_auth(&facts, &clients, &endpoint_auth, &mut output)?;
  events::annotate_events(&facts, &mut output)?;
  output.entity(status_entity(&routes, &clients)?);
  tracing::debug!(
    pass = PASS_NAME,
    composed_endpoints = routes.composed_count(),
    client_calls = clients.client_count,
    unmatched_client_calls = clients.unmatched_count,
    entities = output.entity_count(),
    "linked project"
  );
  Ok(output.finish())
}

fn status_entity(routes: &compose::Routes, clients: &http::ClientLinks) -> LinkResult<Entity> {
  let key = EntityKey::Other {
    kind: EntityKind::custom(STATUS_KIND)?,
    repo: None,
    name: PASS_NAME.to_owned(),
  };
  let mut entity = Entity::new(key, Certainty::Declared);
  entity.attrs.insert("unsupported".to_owned(), json!(UNSUPPORTED));
  entity.attrs.insert("client_calls".to_owned(), json!(clients.client_count));
  entity.attrs.insert("client_calls_unmatched".to_owned(), json!(clients.unmatched_count));
  entity.attrs.insert("composed_endpoints".to_owned(), json!(routes.composed_count()));
  Ok(entity)
}
