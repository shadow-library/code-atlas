mod common;

use atlas_core::{Certainty, EdgeKind, Owner};
use atlas_store::{OwnedFacts, RunOutcome, RunReport, Store};
use common::*;

fn owner_a() -> (Owner, OwnedFacts) {
  let mut documented = entity_in("a", "src/a.rs");
  documented.attrs.insert("meta".into(), serde_json::json!({ "z": 1, "a": [true, null] }));
  (
    file_owner("src/a.rs"),
    OwnedFacts {
      entities: vec![documented, entity_in("b", "src/a.rs")],
      edges: vec![calls("a", "b"), edge(EdgeKind::Calls, "a", "c", heuristic(0.4), "src/a.rs")],
      unresolved: vec![unresolved("a", &["x"], "src/a.rs"), unresolved("b", &["y", "z"], "src/a.rs")],
    },
  )
}

fn owner_b() -> (Owner, OwnedFacts) {
  (
    file_owner("src/b.rs"),
    OwnedFacts {
      entities: vec![entity_in("c", "src/b.rs"), entity_in("b", "src/b.rs")],
      edges: vec![edge(EdgeKind::Calls, "a", "c", Certainty::Resolved, "src/b.rs")],
      unresolved: vec![],
    },
  )
}

fn reversed(mut owned: OwnedFacts) -> OwnedFacts {
  owned.entities.reverse();
  owned.edges.reverse();
  owned.unresolved.reverse();
  owned
}

fn digest(store: &Store) -> atlas_core::ContentHash {
  read(store, |tx| tx.fact_set_digest())
}

#[test]
fn should_produce_the_same_digest_for_the_same_facts_in_any_order() {
  let forward = Store::open_in_memory().unwrap();
  let backward = Store::open_in_memory().unwrap();
  let (a, a_facts) = owner_a();
  let (b, b_facts) = owner_b();

  forward
    .write(|tx| {
      let run = tx.begin_run("full")?;
      tx.replace_owner(&a, &a_facts)?;
      tx.replace_owner(&b, &b_facts)?;
      tx.finish_run(run, RunOutcome::Ok, &RunReport::default())
    })
    .unwrap();
  replace(&backward, &b, &reversed(b_facts.clone()));
  replace(&backward, &a, &facts(vec![entity("scratch")], vec![]));
  replace(&backward, &a, &reversed(a_facts.clone()));

  assert_eq!(digest(&forward), digest(&backward));
}

#[test]
fn should_change_the_digest_when_any_fact_changes() {
  let store = Store::open_in_memory().unwrap();
  let (a, a_facts) = owner_a();
  let (b, b_facts) = owner_b();
  replace(&store, &a, &a_facts);
  replace(&store, &b, &b_facts);
  let baseline = digest(&store);

  let mut weaker = b_facts.clone();
  weaker.edges[0].certainty = heuristic(0.9);
  replace(&store, &b, &weaker);
  let after_certainty = digest(&store);
  replace(&store, &b, &b_facts);
  let restored = digest(&store);
  let mut fewer_refs = a_facts.clone();
  fewer_refs.unresolved.pop();
  replace(&store, &a, &fewer_refs);
  let after_unresolved = digest(&store);
  store.write(|tx| tx.remove_owner(&b)).unwrap();
  let after_removal = digest(&store);

  assert_ne!(after_certainty, baseline);
  assert_eq!(restored, baseline);
  assert_ne!(after_unresolved, baseline);
  assert_ne!(after_removal, after_unresolved);
}

#[test]
fn should_digest_an_empty_store_identically_after_everything_is_removed() {
  let empty = digest(&Store::open_in_memory().unwrap());
  let store = Store::open_in_memory().unwrap();
  let (a, a_facts) = owner_a();
  replace(&store, &a, &a_facts);

  store.write(|tx| tx.remove_owner(&a)).unwrap();

  assert_eq!(digest(&store), empty);
  assert_eq!(store.write(|tx| tx.fact_set_digest()).unwrap(), empty);
}
