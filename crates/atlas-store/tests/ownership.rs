mod common;

use atlas_core::{Certainty, CertaintyClass, EdgeKind, EntityKind, Owner};
use atlas_store::{
  Assertion, ChangeCounts, EdgeFilter, EntityFilter, FactDelta, InferredPolicy, OwnedFacts, OwnedUnresolved, OwnerDelta, RunOutcome, RunReport, Store, StoreError, UnresolvedCounts,
};
use common::*;

#[test]
fn should_read_back_exactly_what_an_owner_asserted() {
  let store = Store::open_in_memory().unwrap();
  let owner = file_owner("src/lib.rs");
  let owned = OwnedFacts {
    entities: vec![entity("parse"), entity("render")],
    edges: vec![calls("parse", "render")],
    unresolved: vec![unresolved("parse", &["fmt", "write"], "src/lib.rs")],
  };

  let delta = replace(&store, &owner, &owned);

  assert_eq!(
    delta,
    FactDelta {
      entities: ChangeCounts { added: 2, removed: 0, changed: 0 },
      edges: ChangeCounts { added: 1, removed: 0, changed: 0 },
      unresolved: UnresolvedCounts { added: 1, removed: 0 },
      created_entities: sorted(vec![id("parse"), id("render")]),
      collected_entities: vec![],
    }
  );
  assert_eq!(read(&store, |tx| tx.entity(id("parse"))), Some(entity("parse")));
  assert_eq!(read(&store, |tx| tx.entity_by_key(&symbol_key("render"))), Some(entity("render")));
  assert_eq!(read(&store, |tx| tx.edges_from(id("parse"), &EdgeFilter::default())), vec![calls("parse", "render")]);
  assert_eq!(read(&store, |tx| tx.edges_to(id("render"), &EdgeFilter::default())), vec![calls("parse", "render")]);
  let stored = read(&store, |tx| tx.owned_facts(&owner));
  let mut expected_entities = owned.entities.clone();
  expected_entities.sort_by_key(|entity| entity.id.as_i64());
  assert_eq!(stored.entities, expected_entities);
  assert_eq!(stored.edges, owned.edges);
  assert_eq!(stored.unresolved, owned.unresolved);
}

#[test]
fn should_replace_previous_assertions_and_collect_orphans() {
  let store = Store::open_in_memory().unwrap();
  let owner = file_owner("src/lib.rs");
  replace(&store, &owner, &facts(vec![entity("a"), entity("b"), entity("c")], vec![calls("a", "b"), calls("b", "c")]));

  let delta = replace(&store, &owner, &facts(vec![entity("a"), entity("c")], vec![calls("a", "c")]));

  assert_eq!(delta.entities, ChangeCounts { added: 0, removed: 1, changed: 0 });
  assert_eq!(delta.edges, ChangeCounts { added: 1, removed: 2, changed: 0 });
  assert_eq!(delta.collected_entities, vec![id("b")]);
  assert_eq!(read(&store, |tx| tx.entity(id("b"))), None);
  assert_eq!(read(&store, |tx| tx.edges_from(id("b"), &EdgeFilter::default())), vec![]);
  assert_eq!(read(&store, |tx| tx.edges_from(id("a"), &EdgeFilter::default())), vec![calls("a", "c")]);
}

#[test]
fn should_remove_every_fact_of_an_owner() {
  let store = Store::open_in_memory().unwrap();
  let owner = file_owner("src/lib.rs");
  replace(
    &store,
    &owner,
    &OwnedFacts {
      entities: vec![entity("a"), entity("b")],
      edges: vec![calls("a", "b")],
      unresolved: vec![unresolved("a", &["missing"], "src/lib.rs")],
    },
  );

  let delta = store.write(|tx| tx.remove_owner(&owner)).unwrap();

  assert_eq!(delta.entities.removed, 2);
  assert_eq!(delta.edges.removed, 1);
  assert_eq!(delta.unresolved, UnresolvedCounts { added: 0, removed: 1 });
  assert_eq!(delta.collected_entities, sorted(vec![id("a"), id("b")]));
  assert_eq!(read(&store, |tx| tx.owned_facts(&owner)), OwnedFacts::default());
  assert_eq!(read(&store, |tx| tx.entities_by_name("a", &EntityFilter::default())), vec![]);
  assert_eq!(read(&store, |tx| tx.unresolved_by_names(&["missing"])), vec![]);
  assert_eq!(store.write(|tx| tx.remove_owner(&owner)).unwrap(), FactDelta::default());
}

#[test]
fn should_keep_a_multi_owner_entity_until_its_last_owner_retracts_it() {
  let store = Store::open_in_memory().unwrap();
  let first = file_owner("src/a.rs");
  let second = file_owner("src/b.rs");
  replace(&store, &first, &facts(vec![entity_in("shared", "src/a.rs")], vec![]));
  let second_delta = replace(&store, &second, &facts(vec![entity_in("shared", "src/b.rs")], vec![]));
  assert_eq!(second_delta.created_entities, vec![]);

  let owners = |store: &Store| {
    read(store, |tx| tx.entity_assertions(id("shared")))
      .into_iter()
      .map(|assertion| assertion.owner)
      .collect::<Vec<_>>()
  };
  assert_eq!(owners(&store), vec![first.clone(), second.clone()]);
  assert_eq!(read(&store, |tx| tx.entity(id("shared"))), Some(entity_in("shared", "src/a.rs")));
  assert_eq!(
    read(&store, |tx| tx.entities_in_file(&repo(), &rel("src/b.rs"), &EntityFilter::default())),
    vec![entity_in("shared", "src/a.rs")]
  );

  let delta = store.write(|tx| tx.remove_owner(&first)).unwrap();
  assert_eq!(delta.collected_entities, vec![]);
  assert_eq!(owners(&store), vec![second.clone()]);
  assert_eq!(read(&store, |tx| tx.entity(id("shared"))), Some(entity_in("shared", "src/b.rs")));
  assert_eq!(read(&store, |tx| tx.entities_in_file(&repo(), &rel("src/a.rs"), &EntityFilter::default())), vec![]);

  let delta = store.write(|tx| tx.remove_owner(&second)).unwrap();
  assert_eq!(delta.collected_entities, vec![id("shared")]);
  assert_eq!(read(&store, |tx| tx.entity(id("shared"))), None);
}

#[test]
fn should_merge_certainty_and_evidence_of_an_edge_asserted_by_two_owners() {
  let store = Store::open_in_memory().unwrap();
  let file = file_owner("src/a.rs");
  let pass = Owner::Project { pass: "link".into() };
  let weak = edge(EdgeKind::Calls, "a", "b", heuristic(0.6), "src/z.rs");
  let strong = edge(EdgeKind::Calls, "a", "b", Certainty::Resolved, "src/a.rs");
  replace(&store, &file, &facts(vec![entity("a")], vec![weak.clone()]));
  replace(&store, &pass, &facts(vec![], vec![strong.clone()]));

  let merged = read(&store, |tx| tx.edge(weak.id)).unwrap();
  assert_eq!(merged.certainty, Certainty::Resolved);
  assert_eq!(merged.evidence, vec![evidence("src/a.rs", 5), evidence("src/z.rs", 5)]);
  assert_eq!(
    read(&store, |tx| tx.edge_assertions(weak.id)),
    vec![
      Assertion {
        owner: file.clone(),
        fact: weak.clone()
      },
      Assertion {
        owner: pass.clone(),
        fact: strong.clone()
      },
    ]
  );
  let authoritative = EdgeFilter {
    min_certainty: Some(CertaintyClass::Resolved),
    ..EdgeFilter::default()
  };
  assert_eq!(read(&store, |tx| tx.edges_from(id("a"), &authoritative)).len(), 1);

  store.write(|tx| tx.remove_owner(&pass)).unwrap();
  assert_eq!(read(&store, |tx| tx.edge(weak.id)), Some(weak.clone()));
  assert_eq!(read(&store, |tx| tx.edges_from(id("a"), &authoritative)), vec![]);
}

#[test]
fn should_merge_duplicate_evidence_once() {
  let store = Store::open_in_memory().unwrap();
  let shared = edge(EdgeKind::Calls, "a", "b", Certainty::Resolved, "src/a.rs");
  replace(&store, &file_owner("src/a.rs"), &facts(vec![], vec![shared.clone()]));
  replace(&store, &file_owner("src/b.rs"), &facts(vec![], vec![shared.clone()]));

  assert_eq!(read(&store, |tx| tx.edge(shared.id)), Some(shared));
}

#[test]
fn should_count_changed_assertions_and_update_the_merged_view() {
  let store = Store::open_in_memory().unwrap();
  let owner = file_owner("src/lib.rs");
  replace(&store, &owner, &facts(vec![entity("a")], vec![calls("a", "b")]));
  let mut renamed = entity("a");
  renamed.signature = Some("fn a(x: u8)".into());
  let upgraded = edge(EdgeKind::Calls, "a", "b", Certainty::CompilerVerified, "src/lib.rs");

  let delta = replace(&store, &owner, &facts(vec![renamed.clone()], vec![upgraded.clone()]));

  assert_eq!(delta.entities, ChangeCounts { added: 0, removed: 0, changed: 1 });
  assert_eq!(delta.edges, ChangeCounts { added: 0, removed: 0, changed: 1 });
  assert_eq!(read(&store, |tx| tx.entity(id("a"))), Some(renamed));
  let compiler_only = EdgeFilter {
    min_certainty: Some(CertaintyClass::CompilerVerified),
    ..EdgeFilter::default()
  };
  assert_eq!(read(&store, |tx| tx.edges_from(id("a"), &compiler_only)), vec![upgraded]);
}

#[test]
fn should_make_an_identical_reindex_a_no_op() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let store = Store::open(&path).unwrap();
  let owner = file_owner("src/lib.rs");
  let owned = OwnedFacts {
    entities: vec![entity("a"), entity("b")],
    edges: vec![calls("a", "b")],
    unresolved: vec![unresolved("a", &["x"], "src/lib.rs"), unresolved("b", &["y"], "src/lib.rs")],
  };
  let first_run = store
    .write(|tx| {
      let run = tx.begin_run("full")?;
      tx.replace_owner(&owner, &owned)?;
      tx.finish_run(run, RunOutcome::Ok, &RunReport::default())?;
      Ok::<_, StoreError>(run)
    })
    .unwrap();
  let observer = rusqlite::Connection::open(&path).unwrap();
  let data_version = || observer.query_row("PRAGMA data_version", [], |row| row.get::<_, i64>(0)).unwrap();
  let before = data_version();

  let mut reordered = owned.clone();
  reordered.entities.reverse();
  reordered.unresolved.reverse();
  let (second_run, delta) = store
    .write(|tx| {
      let run = tx.begin_run("incremental")?;
      let delta = tx.replace_owner(&owner, &reordered)?;
      Ok::<_, StoreError>((run, delta))
    })
    .unwrap();

  assert_eq!(delta, FactDelta::default());
  assert_eq!(read(&store, |tx| tx.run_deltas(second_run)), vec![]);
  assert_eq!(read(&store, |tx| tx.run_deltas(first_run)).len(), 1);
  let assertion_runs: Vec<Option<i64>> = observer
    .prepare("SELECT run_id FROM entity_owners UNION ALL SELECT run_id FROM edge_owners UNION ALL SELECT run_id FROM unresolved_refs")
    .unwrap()
    .query_map([], |row| row.get(0))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap();
  assert_eq!(assertion_runs, vec![Some(first_run.0); 5]);
  let after_run = data_version();
  assert_ne!(after_run, before, "the run row is a committed change");

  store.write(|tx| tx.replace_owner(&owner, &reordered)).unwrap();
  assert_eq!(data_version(), after_run, "a run-less identical reindex must not write");
}

#[test]
fn should_record_per_owner_fact_deltas_for_the_active_run() {
  let store = Store::open_in_memory().unwrap();
  let a = file_owner("src/a.rs");
  let b = file_owner("src/b.rs");
  replace(&store, &a, &facts(vec![entity("x")], vec![]));

  let run = store
    .write(|tx| {
      let run = tx.begin_run("incremental")?;
      tx.replace_owner(&a, &facts(vec![entity("y")], vec![calls("y", "x")]))?;
      tx.replace_owner(&b, &facts(vec![entity("z")], vec![]))?;
      Ok::<_, StoreError>(run)
    })
    .unwrap();
  store
    .write(|tx| {
      tx.resume_run(run)?;
      tx.replace_owner(&b, &facts(vec![entity("z"), entity("w")], vec![]))?;
      tx.finish_run(run, RunOutcome::Ok, &RunReport::default())
    })
    .unwrap();

  assert_eq!(
    read(&store, |tx| tx.run_deltas(run)),
    vec![
      OwnerDelta {
        owner: a,
        entities: ChangeCounts { added: 1, removed: 1, changed: 0 },
        edges: ChangeCounts { added: 1, removed: 0, changed: 0 },
        unresolved: UnresolvedCounts::default(),
      },
      OwnerDelta {
        owner: b,
        entities: ChangeCounts { added: 2, removed: 0, changed: 0 },
        edges: ChangeCounts::default(),
        unresolved: UnresolvedCounts::default(),
      },
    ]
  );
}

#[test]
fn should_reject_inconsistent_or_duplicate_facts_without_writing() {
  let store = Store::open_in_memory().unwrap();
  let owner = file_owner("src/lib.rs");
  let mut tampered = entity("a");
  tampered.kind = EntityKind::Struct;

  let inconsistent = store.write(|tx| tx.replace_owner(&owner, &facts(vec![tampered], vec![])));
  let duplicate_entity = store.write(|tx| tx.replace_owner(&owner, &facts(vec![entity("a"), entity("a")], vec![])));
  let duplicate_edge = store.write(|tx| tx.replace_owner(&owner, &facts(vec![], vec![calls("a", "b"), calls("a", "b")])));

  for result in [inconsistent, duplicate_entity, duplicate_edge] {
    assert!(matches!(result, Err(StoreError::InvalidFacts { .. })), "{result:?}");
  }
  assert_eq!(read(&store, |tx| tx.owned_facts(&owner)), OwnedFacts::default());
}

#[test]
fn should_filter_entities_by_kind_certainty_and_inference() {
  let store = Store::open_in_memory().unwrap();
  let mut guessed = entity("guessed");
  guessed.certainty = inferred(0.9);
  let mut fuzzy = entity("fuzzy");
  fuzzy.certainty = heuristic(0.5);
  replace(&store, &file_owner("src/lib.rs"), &facts(vec![entity("exact"), guessed.clone(), fuzzy.clone()], vec![]));

  let functions = |filter: EntityFilter| read(&store, |tx| tx.entities_by_kind(&EntityKind::Function, &filter)).len();
  assert_eq!(functions(EntityFilter::default()), 3);
  assert_eq!(
    functions(EntityFilter {
      inferred: InferredPolicy::Exclude,
      ..EntityFilter::default()
    }),
    2
  );
  assert_eq!(
    functions(EntityFilter {
      min_certainty: Some(CertaintyClass::Syntactic),
      ..EntityFilter::default()
    }),
    1
  );
  assert_eq!(
    functions(EntityFilter {
      kinds: vec![EntityKind::Struct],
      ..EntityFilter::default()
    }),
    0
  );
  assert_eq!(read(&store, |tx| tx.entities_by_name("guessed", &EntityFilter::default())), vec![guessed]);
}

#[test]
fn should_index_unresolved_references_and_dependents_by_target() {
  let store = Store::open_in_memory().unwrap();
  let a = file_owner("src/a.rs");
  let b = file_owner("src/b.rs");
  let c = file_owner("src/c.rs");
  let pending = unresolved("caller", &["util", "helper"], "src/a.rs");
  replace(
    &store,
    &a,
    &OwnedFacts {
      entities: vec![entity_in("caller", "src/a.rs")],
      edges: vec![calls("caller", "target")],
      unresolved: vec![pending.clone()],
    },
  );
  replace(&store, &b, &facts(vec![], vec![calls("other", "target")]));
  replace(&store, &c, &facts(vec![], vec![calls("target", "other")]));

  assert_eq!(
    read(&store, |tx| tx.unresolved_by_names(&["helper", "helper", "absent"])),
    vec![OwnedUnresolved {
      owner: a.clone(),
      reference: pending
    }]
  );
  assert_eq!(read(&store, |tx| tx.unresolved_by_names(&["util"])), vec![]);
  assert_eq!(read(&store, |tx| tx.owners_referencing(&[id("target")])), vec![a.clone(), b.clone()]);
  assert_eq!(read(&store, |tx| tx.owners_referencing(&[id("target"), id("other")])), vec![a, b, c]);
  assert_eq!(read(&store, |tx| tx.owners_referencing(&[id("caller")])), vec![]);
}

#[test]
fn should_refuse_an_edge_whose_id_collides_with_a_different_edge() {
  let store = Store::open_in_memory().unwrap();
  let genuine = calls("a", "b");
  replace(&store, &file_owner("src/a.rs"), &facts(vec![], vec![genuine.clone()]));
  let mut forged = edge(EdgeKind::References, "x", "y", Certainty::Resolved, "src/b.rs");
  forged.id = genuine.id;

  let result = store.write(|tx| tx.replace_owner(&file_owner("src/b.rs"), &facts(vec![], vec![forged.clone()])));

  assert!(matches!(result, Err(StoreError::IdCollision { what: "edge", .. })), "{result:?}");
  assert_eq!(read(&store, |tx| tx.edge(genuine.id)), Some(genuine));
}

#[test]
fn should_leave_no_partial_state_when_a_failed_replacement_error_is_swallowed() {
  let store = Store::open_in_memory().unwrap();
  let genuine = calls("a", "b");
  replace(&store, &file_owner("src/a.rs"), &facts(vec![entity("a")], vec![genuine.clone()]));
  let before = read(&store, |tx| tx.fact_set_digest());
  let intruder = file_owner("src/b.rs");
  let mut forged = edge(EdgeKind::References, "x", "y", Certainty::Resolved, "src/b.rs");
  forged.id = genuine.id;

  let swallowed = store
    .write(|tx| {
      let run = tx.begin_run("incremental")?;
      let failed = tx.replace_owner(&intruder, &facts(vec![entity("z")], vec![forged.clone()]));
      tx.finish_run(run, RunOutcome::Ok, &RunReport::default())?;
      Ok::<_, StoreError>((run, failed))
    })
    .unwrap();

  let (run, failed) = swallowed;
  assert!(matches!(failed, Err(StoreError::IdCollision { .. })), "{failed:?}");
  assert_eq!(read(&store, |tx| tx.entity(id("z"))), None);
  assert_eq!(read(&store, |tx| tx.owned_facts(&intruder)), OwnedFacts::default());
  assert_eq!(read(&store, |tx| tx.run_deltas(run)), vec![]);
  assert_eq!(read(&store, |tx| tx.fact_set_digest()), before);
}

#[test]
fn should_keep_edges_to_a_collected_target_until_their_owner_re_resolves() {
  let store = Store::open_in_memory().unwrap();
  let caller = file_owner("src/a.rs");
  let callee = file_owner("src/b.rs");
  let dangling = calls("a", "b");
  replace(&store, &caller, &facts(vec![entity_in("a", "src/a.rs")], vec![dangling.clone()]));
  replace(&store, &callee, &facts(vec![entity_in("b", "src/b.rs")], vec![]));

  let delta = store.write(|tx| tx.remove_owner(&callee)).unwrap();

  assert_eq!(delta.collected_entities, vec![id("b")]);
  assert_eq!(read(&store, |tx| tx.entity(id("b"))), None);
  assert_eq!(read(&store, |tx| tx.edges_from(id("a"), &EdgeFilter::default())), vec![dangling.clone()]);
  assert_eq!(read(&store, |tx| tx.edges_to(id("b"), &EdgeFilter::default())), vec![dangling.clone()]);
  assert_eq!(read(&store, |tx| tx.owners_referencing(&[id("b")])), vec![caller]);
  let traversal = read(&store, |tx| {
    tx.traverse(&atlas_store::TraverseSpec {
      seeds: vec![id("a")],
      direction: atlas_store::Direction::Out,
      filter: EdgeFilter::default(),
      max_depth: 2,
      max_nodes: 10,
    })
  });
  let reached_b = traversal.nodes.iter().find(|node| node.id == id("b")).map(|node| node.path.clone());
  assert_eq!(reached_b, Some(vec![dangling.id]));
}

fn with_attrs(mut entity: atlas_core::Entity, attrs: &[(&str, serde_json::Value)]) -> atlas_core::Entity {
  entity.attrs.extend(attrs.iter().map(|(name, value)| ((*name).to_owned(), value.clone())));
  entity
}

#[test]
fn should_union_entity_attrs_across_assertions_without_changing_certainty() {
  let store = Store::open_in_memory().unwrap();
  let file = file_owner("src/a.rs");
  let pass = Owner::Project { pass: "link".into() };
  let mut declared = with_attrs(entity("shared"), &[("language", serde_json::json!("rust"))]);
  declared.certainty = Certainty::CompilerVerified;
  replace(&store, &file, &facts(vec![declared.clone()], vec![]));
  replace(&store, &pass, &facts(vec![with_attrs(entity("shared"), &[("consumers", serde_json::json!(2))])], vec![]));

  let merged = read(&store, |tx| tx.entity(id("shared"))).unwrap();

  assert_eq!(merged.certainty, Certainty::CompilerVerified);
  assert_eq!(merged.attrs.get("language"), Some(&serde_json::json!("rust")));
  assert_eq!(merged.attrs.get("consumers"), Some(&serde_json::json!(2)));
  assert_eq!(read(&store, |tx| tx.entity_assertions(id("shared"))).len(), 2);
  assert_eq!(read(&store, |tx| tx.entity_assertions(id("shared")))[0].fact, declared);
}

#[test]
fn should_resolve_entity_attr_conflicts_by_winner_then_owner_order() {
  let store = Store::open_in_memory().unwrap();
  let first = file_owner("src/a.rs");
  let second = file_owner("src/b.rs");
  let pass = Owner::Project { pass: "link".into() };
  let mut strongest = with_attrs(entity("shared"), &[("count", serde_json::json!("pass"))]);
  strongest.certainty = Certainty::CompilerVerified;
  replace(
    &store,
    &second,
    &facts(
      vec![with_attrs(
        entity("shared"),
        &[("count", serde_json::json!("second")), ("only_second", serde_json::json!(true))],
      )],
      vec![],
    ),
  );
  replace(
    &store,
    &first,
    &facts(
      vec![with_attrs(entity("shared"), &[("count", serde_json::json!("first")), ("tag", serde_json::json!("first"))])],
      vec![],
    ),
  );
  replace(
    &store,
    &second,
    &facts(
      vec![with_attrs(
        entity("shared"),
        &[
          ("count", serde_json::json!("second")),
          ("tag", serde_json::json!("second")),
          ("only_second", serde_json::json!(true)),
        ],
      )],
      vec![],
    ),
  );

  let before = read(&store, |tx| tx.entity(id("shared"))).unwrap();
  assert_eq!(before.attrs.get("count"), Some(&serde_json::json!("first")));
  assert_eq!(before.attrs.get("tag"), Some(&serde_json::json!("first")));
  assert_eq!(before.attrs.get("only_second"), Some(&serde_json::json!(true)));

  replace(&store, &pass, &facts(vec![strongest], vec![]));
  let after = read(&store, |tx| tx.entity(id("shared"))).unwrap();
  assert_eq!(after.attrs.get("count"), Some(&serde_json::json!("pass")));
  assert_eq!(after.attrs.get("tag"), Some(&serde_json::json!("first")));
  assert_eq!(after.attrs.get("only_second"), Some(&serde_json::json!(true)));
}

#[test]
fn should_union_edge_attrs_across_assertions_deterministically() {
  let forward = Store::open_in_memory().unwrap();
  let backward = Store::open_in_memory().unwrap();
  let file = file_owner("src/a.rs");
  let pass = Owner::Project { pass: "link".into() };
  let mut from_file = edge(EdgeKind::Calls, "a", "b", Certainty::CompilerVerified, "src/a.rs");
  from_file.attrs.insert("via".into(), serde_json::json!("file"));
  from_file.attrs.insert("file_only".into(), serde_json::json!(1));
  let mut from_pass = edge(EdgeKind::Calls, "a", "b", Certainty::Syntactic, "src/z.rs");
  from_pass.attrs.insert("via".into(), serde_json::json!("pass"));
  from_pass.attrs.insert("pass_only".into(), serde_json::json!(2));

  replace(&forward, &file, &facts(vec![entity("a")], vec![from_file.clone()]));
  replace(&forward, &pass, &facts(vec![], vec![from_pass.clone()]));
  replace(&backward, &pass, &facts(vec![], vec![from_pass.clone()]));
  replace(&backward, &file, &facts(vec![entity("a")], vec![from_file.clone()]));

  let merged = read(&forward, |tx| tx.edge(from_file.id)).unwrap();
  assert_eq!(merged.certainty, Certainty::CompilerVerified);
  assert_eq!(merged.attrs.get("via"), Some(&serde_json::json!("file")));
  assert_eq!(merged.attrs.get("file_only"), Some(&serde_json::json!(1)));
  assert_eq!(merged.attrs.get("pass_only"), Some(&serde_json::json!(2)));
  assert_eq!(read(&backward, |tx| tx.edge(from_file.id)), Some(merged));
  assert_eq!(read(&forward, |tx| tx.fact_set_digest()), read(&backward, |tx| tx.fact_set_digest()));
}

#[test]
fn should_report_the_owner_and_certainty_that_supplied_each_attr() {
  let store = Store::open_in_memory().unwrap();
  let file = file_owner("src/a.rs");
  let pass = Owner::Project { pass: "link".into() };
  let mut weak = with_attrs(entity("shared"), &[("used", serde_json::json!(true)), ("language", serde_json::json!("pass"))]);
  weak.certainty = heuristic(0.6);
  replace(
    &store,
    &file,
    &facts(vec![with_attrs(entity("shared"), &[("language", serde_json::json!("rust"))])], vec![]),
  );
  replace(&store, &pass, &facts(vec![weak], vec![]));

  let merged = read(&store, |tx| tx.entity_with_sources(id("shared"))).unwrap();

  assert_eq!(merged.entity.certainty, Certainty::Syntactic);
  assert_eq!(
    merged.attr_sources["language"],
    atlas_store::AttrSource {
      owner: file,
      certainty: Certainty::Syntactic
    }
  );
  assert_eq!(
    merged.attr_sources["used"],
    atlas_store::AttrSource {
      owner: pass,
      certainty: heuristic(0.6)
    }
  );
  assert_eq!(merged.attr_sources.len(), 2);
}

#[test]
fn should_produce_the_same_digest_for_entity_attr_unions_in_any_write_order() {
  let file = file_owner("src/a.rs");
  let pass = Owner::Project { pass: "link".into() };
  let from_file = with_attrs(entity("shared"), &[("count", serde_json::json!("file")), ("a", serde_json::json!(1))]);
  let from_pass = with_attrs(entity("shared"), &[("count", serde_json::json!("pass")), ("b", serde_json::json!(2))]);
  let forward = Store::open_in_memory().unwrap();
  let backward = Store::open_in_memory().unwrap();
  replace(&forward, &file, &facts(vec![from_file.clone()], vec![]));
  replace(&forward, &pass, &facts(vec![from_pass.clone()], vec![]));
  replace(&backward, &pass, &facts(vec![from_pass], vec![]));
  replace(&backward, &file, &facts(vec![from_file], vec![]));

  assert_eq!(read(&forward, |tx| tx.fact_set_digest()), read(&backward, |tx| tx.fact_set_digest()));
  assert_eq!(read(&forward, |tx| tx.entity(id("shared"))), read(&backward, |tx| tx.entity(id("shared"))));
}
