mod common;

use atlas_core::{CertaintyClass, EdgeKind, EntityKind};
use atlas_store::{EdgeEnd, EdgeFilter, EdgeLink, OwnedFacts, OwnedUnresolved, Store};
use common::*;

fn seeded() -> Store {
  let store = Store::open_in_memory().unwrap();
  replace(
    &store,
    &file_owner("src/lib.rs"),
    &OwnedFacts {
      entities: vec![entity("parse"), entity("render"), entity("emit")],
      edges: vec![calls("parse", "render"), edge(EdgeKind::References, "emit", "render", heuristic(0.25), "src/lib.rs")],
      unresolved: vec![
        unresolved("parse", &["registry", "[key]"], "src/lib.rs"),
        unresolved("parse", &["fmt", "write"], "src/lib.rs"),
        unresolved("parse", &["handlers", "[name]", "run"], "src/lib.rs"),
      ],
    },
  );
  replace(
    &store,
    &file_owner("src/other.rs"),
    &OwnedFacts {
      entities: vec![entity_in("other", "src/other.rs")],
      edges: Vec::new(),
      unresolved: vec![unresolved("other", &["table", "[0]"], "src/other.rs"), unresolved("other", &["\\escaped"], "src/other.rs")],
    },
  );
  store
}

#[test]
fn should_list_edge_rows_with_their_certainty_without_decoding_assertions() {
  let store = seeded();

  let links = read(&store, |tx| tx.edge_links_to(id("render"), &EdgeFilter::default()));

  let mut expected = vec![
    EdgeLink {
      id: calls("parse", "render").id,
      kind: EdgeKind::Calls,
      from: id("parse"),
      to: id("render"),
      certainty_class: CertaintyClass::Resolved,
      confidence: None,
    },
    EdgeLink {
      id: edge(EdgeKind::References, "emit", "render", heuristic(0.25), "src/lib.rs").id,
      kind: EdgeKind::References,
      from: id("emit"),
      to: id("render"),
      certainty_class: CertaintyClass::Heuristic,
      confidence: Some(0.25),
    },
  ];
  expected.sort_by_key(|link| link.id.as_i64());
  assert_eq!(links, expected);
  let calls_only = EdgeFilter {
    kinds: vec![EdgeKind::Calls],
    ..EdgeFilter::default()
  };
  assert_eq!(read(&store, |tx| tx.edge_links_from(id("parse"), &calls_only)).len(), 1);
  assert_eq!(read(&store, |tx| tx.edge_links_from(id("render"), &EdgeFilter::default())), vec![]);
}

#[test]
fn should_tell_whether_an_edge_kind_touches_an_entity_kind() {
  let store = seeded();

  let meets = |kind: EdgeKind, end: EdgeEnd, entity: EntityKind| read(&store, |tx| tx.edge_kind_meets_entity_kind(&kind, end, &entity));

  assert!(meets(EdgeKind::Calls, EdgeEnd::Target, EntityKind::Function));
  assert!(meets(EdgeKind::Calls, EdgeEnd::Source, EntityKind::Function));
  assert!(!meets(EdgeKind::Calls, EdgeEnd::Target, EntityKind::ApiEndpoint));
  assert!(!meets(EdgeKind::CallsEndpoint, EdgeEnd::Target, EntityKind::Function));
}

#[test]
fn should_find_unresolved_references_whose_last_segment_starts_with_a_prefix() {
  let store = seeded();

  let found = read(&store, |tx| tx.unresolved_with_name_prefix("["));

  assert_eq!(
    found,
    vec![
      OwnedUnresolved {
        owner: file_owner("src/lib.rs"),
        reference: unresolved("parse", &["registry", "[key]"], "src/lib.rs"),
      },
      OwnedUnresolved {
        owner: file_owner("src/other.rs"),
        reference: unresolved("other", &["table", "[0]"], "src/other.rs"),
      },
    ]
  );
  assert!(read(&store, |tx| tx.has_unresolved_with_name_prefix("[")));
  assert!(!read(&store, |tx| tx.has_unresolved_with_name_prefix("[[")));
  assert_eq!(read(&store, |tx| tx.unresolved_with_name_prefix("")).len(), 5);
  assert_eq!(
    read(&store, |tx| tx.unresolved_in_file_with_name_prefix(&repo(), &rel("src/other.rs"), "[")),
    vec![unresolved("other", &["table", "[0]"], "src/other.rs")]
  );
  assert_eq!(read(&store, |tx| tx.unresolved_in_file_with_name_prefix(&repo(), &rel("src/missing.rs"), "[")), vec![]);
}

#[test]
fn should_list_a_repos_unresolved_references_in_path_then_owned_facts_order() {
  let store = seeded();

  let found = read(&store, |tx| tx.unresolved_in_repo(&repo()));

  let mut expected = Vec::new();
  for path in ["src/lib.rs", "src/other.rs"] {
    let owner = file_owner(path);
    for reference in read(&store, |tx| tx.owned_facts(&owner)).unresolved {
      expected.push(OwnedUnresolved { owner: owner.clone(), reference });
    }
  }
  assert_eq!(found.len(), 5);
  assert_eq!(found, expected);
}

#[test]
fn should_list_edges_of_one_kind_and_count_rows_without_decoding() {
  let store = seeded();

  let references = read(&store, |tx| tx.edge_links_of_kind(&EdgeKind::References, &EdgeFilter::default()));

  assert_eq!(references.iter().map(|link| (link.from, link.to)).collect::<Vec<_>>(), vec![(id("emit"), id("render"))]);
  assert_eq!(read(&store, |tx| tx.edge_count_to(id("render"))), 2);
  assert_eq!(read(&store, |tx| tx.edge_count_to(id("parse"))), 0);
  assert_eq!(read(&store, |tx| tx.edge_count_of_kind(&EdgeKind::Calls)), 1);
  assert_eq!(read(&store, |tx| tx.edge_count_of_kind(&EdgeKind::CoversFile)), 0);
}
