use std::collections::BTreeSet;

use atlas_core::{EdgeKind, EntityKind, RepoName};
use serde_json::{Value, json};

use crate::error::LinkResult;
use crate::facts::Facts;
use crate::output::{Output, annotated};

struct Side {
  count: usize,
  repos: BTreeSet<RepoName>,
}

fn side(facts: &Facts<'_, '_>, event: atlas_core::EntityId, kind: &EdgeKind) -> LinkResult<Side> {
  let edges = facts.edges_to(event, kind)?;
  let mut repos = BTreeSet::new();
  for edge in &edges {
    if let Some(repo) = facts.entity(edge.from)?.and_then(|entity| entity.repo) {
      repos.insert(repo);
    }
  }
  Ok(Side { count: edges.len(), repos })
}

fn repo_names(repos: &BTreeSet<RepoName>) -> Value {
  json!(repos.iter().map(RepoName::as_str).collect::<Vec<_>>())
}

pub(crate) fn annotate_events(facts: &Facts<'_, '_>, output: &mut Output) -> LinkResult<()> {
  for event in facts.entities(&EntityKind::Event)? {
    let producers = side(facts, event.id, &EdgeKind::Publishes)?;
    let consumers = side(facts, event.id, &EdgeKind::Subscribes)?;
    let cross_repo = producers.repos.iter().any(|producer| consumers.repos.iter().any(|consumer| consumer != producer));
    let mut attrs = vec![
      ("producers", json!(producers.count)),
      ("consumers", json!(consumers.count)),
      ("producer_repos", repo_names(&producers.repos)),
      ("consumer_repos", repo_names(&consumers.repos)),
      ("cross_repo", json!(cross_repo)),
    ];
    match (producers.count, consumers.count) {
      (0, 0) => {}
      (_, 0) => attrs.push(("orphan", json!("no_consumer"))),
      (0, _) => attrs.push(("orphan", json!("no_producer"))),
      _ => {}
    }
    output.entity(annotated(&event, attrs));
  }
  Ok(())
}
