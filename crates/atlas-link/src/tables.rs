use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{Edge, EdgeId, EdgeKind, EntityKind};
use serde_json::json;

use crate::error::LinkResult;
use crate::facts::Facts;
use crate::output::{Output, attr_str, relabel, weaker};

const ACCESS_KINDS: [EdgeKind; 2] = [EdgeKind::ReadsTable, EdgeKind::WritesTable];
const ATTR_ORM: &str = "orm";
const ATTR_VIA: &str = "via";
const ATTR_EXPERIMENTAL: &str = "experimental";

struct Composed {
  edge: Edge,
  via: BTreeSet<String>,
  orms: BTreeSet<String>,
}

pub(crate) fn link_table_accesses(facts: &Facts<'_, '_>, output: &mut Output) -> LinkResult<usize> {
  let mut composed: BTreeMap<EdgeId, Composed> = BTreeMap::new();
  for table in facts.entities(&EntityKind::DbTable)? {
    for mapping in facts.edges_to(table.id, &EdgeKind::MapsToTable)? {
      let Some(mapped) = facts.entity(mapping.from)? else { continue };
      for kind in &ACCESS_KINDS {
        for access in facts.edges_to(mapped.id, kind)? {
          let certainty = weaker(&access.certainty, &mapping.certainty);
          let orm = attr_str(&access.attrs, ATTR_ORM).or_else(|| attr_str(&mapping.attrs, ATTR_ORM));
          let mut edge = Edge::new(kind.clone(), access.from, table.id, "", certainty);
          edge.evidence = access.evidence.iter().map(|evidence| relabel(evidence, None)).collect();
          merge(&mut composed, edge, mapped.key.to_string(), orm);
        }
      }
    }
  }
  let count = composed.len();
  for Composed { mut edge, via, orms } in composed.into_values() {
    edge.attrs.insert(ATTR_EXPERIMENTAL.to_owned(), json!(true));
    edge.attrs.insert(ATTR_VIA.to_owned(), json!(via));
    if let [orm] = orms.iter().collect::<Vec<_>>().as_slice() {
      edge.attrs.insert(ATTR_ORM.to_owned(), json!(orm));
    }
    output.edge(edge);
  }
  Ok(count)
}

fn merge(composed: &mut BTreeMap<EdgeId, Composed>, edge: Edge, via: String, orm: Option<&str>) {
  let entry = composed.entry(edge.id).or_insert_with(|| Composed {
    edge: Edge::new(edge.kind.clone(), edge.from, edge.to, "", edge.certainty.clone()),
    via: BTreeSet::new(),
    orms: BTreeSet::new(),
  });
  if edge.certainty.cmp_strength(&entry.edge.certainty) == Ordering::Greater {
    entry.edge.certainty = edge.certainty;
  }
  entry.edge.evidence.extend(edge.evidence);
  entry.via.insert(via);
  entry.orms.extend(orm.map(str::to_owned));
}

#[cfg(test)]
mod tests {
  use atlas_core::{Certainty, Confidence, ContentHash, EntityId, EntityKey, Evidence, Producer, RelPath, RepoName, Span};

  use super::*;

  fn access(confidence: f32, line: u32) -> Edge {
    let from = EntityId::of(&EntityKey::DbTable { name: "reader".to_owned() });
    let to = EntityId::of(&EntityKey::DbTable { name: "users".to_owned() });
    let mut edge = Edge::new(EdgeKind::ReadsTable, from, to, "", Certainty::heuristic(Confidence::new(confidence).unwrap(), "rule"));
    edge.evidence.push(Evidence {
      repo: RepoName::new("shop").unwrap(),
      path: RelPath::new("src/repo.ts").unwrap(),
      span: Span::from_zero_based_rows(line, 0, line, 1, 0, 1),
      content_hash: ContentHash::of(b"x"),
      producer: Producer::new("atlas-link", crate::LINK_VERSION),
      note: None,
    });
    edge
  }

  #[test]
  fn should_keep_the_strongest_certainty_and_every_constant_whatever_the_merge_order() {
    let mut forward = BTreeMap::new();
    merge(&mut forward, access(0.4, 1), "b".to_owned(), Some("drizzle"));
    merge(&mut forward, access(0.6, 2), "a".to_owned(), Some("drizzle"));
    let mut backward = BTreeMap::new();
    merge(&mut backward, access(0.6, 2), "a".to_owned(), Some("drizzle"));
    merge(&mut backward, access(0.4, 1), "b".to_owned(), Some("drizzle"));
    for merged in [forward, backward] {
      let composed = merged.into_values().next().unwrap();
      assert_eq!(composed.edge.certainty.tag(), "[heuristic 0.60]");
      assert_eq!(composed.via.into_iter().collect::<Vec<_>>(), ["a", "b"]);
      assert_eq!(composed.orms.into_iter().collect::<Vec<_>>(), ["drizzle"]);
      assert_eq!(composed.edge.evidence.len(), 2);
    }
  }
}
