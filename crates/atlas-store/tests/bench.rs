mod common;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use atlas_core::{Certainty, Edge, EdgeKind, Entity, EntityId};
use atlas_store::{Completeness, Direction, EdgeFilter, OwnedFacts, Store, StoreError, TraverseSpec};
use common::*;

const OWNERS: usize = 5_000;
const ENTITIES_PER_OWNER: usize = 20;
const EDGES_PER_OWNER: usize = 80;

struct XorShift(u64);

impl XorShift {
  fn next(&mut self, bound: usize) -> usize {
    self.0 ^= self.0 << 13;
    self.0 ^= self.0 >> 7;
    self.0 ^= self.0 << 17;
    (self.0 % bound as u64) as usize
  }
}

fn name(owner: usize, index: usize) -> String {
  format!("f{owner}_{index}")
}

fn owner_facts(owner: usize, rng: &mut XorShift, signature: &str) -> OwnedFacts {
  let path = format!("src/m{owner}.rs");
  let entities: Vec<Entity> = (0..ENTITIES_PER_OWNER)
    .map(|index| {
      let mut entity = entity_in(&name(owner, index), &path);
      entity.signature = Some(format!("fn {}() {signature}", name(owner, index)));
      entity
    })
    .collect();
  let hub = id(&name(0, 0));
  let mut seen = HashSet::new();
  let mut edges: Vec<Edge> = Vec::with_capacity(EDGES_PER_OWNER);
  if owner != 0 {
    let to_hub = Edge::new(EdgeKind::Calls, entities[0].id, hub, "", Certainty::Resolved).with_evidence(evidence(&path, 0));
    seen.insert(to_hub.id);
    edges.push(to_hub);
  }
  while edges.len() < EDGES_PER_OWNER {
    let from = entities[rng.next(ENTITIES_PER_OWNER)].id;
    let to = id(&name(rng.next(OWNERS), rng.next(ENTITIES_PER_OWNER)));
    let certainty = if rng.next(4) == 0 { heuristic(0.6) } else { Certainty::Resolved };
    let edge = Edge::new(EdgeKind::Calls, from, to, "", certainty).with_evidence(evidence(&path, rng.next(4_000) as u32));
    if from != to && seen.insert(edge.id) {
      edges.push(edge);
    }
  }
  OwnedFacts {
    entities,
    edges,
    unresolved: vec![unresolved(&name(owner, 0), &["missing", "thing"], &path)],
  }
}

fn timed<T>(label: &str, f: impl FnOnce() -> T) -> (T, Duration) {
  let started = Instant::now();
  let value = f();
  let elapsed = started.elapsed();
  eprintln!("{label}: {elapsed:?}");
  (value, elapsed)
}

#[test]
#[ignore = "performance benchmark; run with --release -- --ignored --nocapture"]
fn should_write_and_query_a_hundred_thousand_entities_within_budget() {
  let dir = tempfile::tempdir().unwrap();
  let store = Store::open(dir.path().join("atlas.db")).unwrap();
  let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
  let all: Vec<OwnedFacts> = (0..OWNERS).map(|owner| owner_facts(owner, &mut rng, "v1")).collect();

  let (_, full_write) = timed("full write (100k entities, 400k edges, 5k owners)", || {
    store
      .write(|tx| {
        for (owner, owned) in all.iter().enumerate() {
          tx.replace_owner(&file_owner(&format!("src/m{owner}.rs")), owned)?;
        }
        Ok::<_, StoreError>(())
      })
      .unwrap()
  });
  let counts = read(&store, |tx| {
    Ok((
      tx.entities_in_file(&repo(), &rel("src/m42.rs"), &Default::default())?.len(),
      tx.edges_to(id(&name(0, 0)), &Default::default())?.len(),
    ))
  });
  assert_eq!(counts.0, ENTITIES_PER_OWNER);
  assert!(counts.1 >= OWNERS - 1);

  let target = 1_234;
  let owner = file_owner(&format!("src/m{target}.rs"));
  let (noop, noop_time) = timed("replace_owner, identical 101 facts", || replace(&store, &owner, &all[target]));
  assert!(noop.is_empty());

  let mut typical = all[target].clone();
  typical.edges.truncate(30);
  for entity in typical.entities.iter_mut().take(5) {
    entity.signature = Some("fn changed()".into());
  }
  let mut edit_rng = XorShift(7);
  for edge in typical.edges.iter_mut().take(10) {
    let to = id(&name(edit_rng.next(OWNERS), edit_rng.next(ENTITIES_PER_OWNER)));
    *edge = Edge::new(EdgeKind::Calls, edge.from, to, "", Certainty::Resolved).with_evidence(evidence("src/x.rs", 1));
  }
  typical.edges.sort_by_key(|edge| edge.id);
  typical.edges.dedup_by_key(|edge| edge.id);
  let (delta, typical_time) = timed("replace_owner, typical edit (20 entities + 30 edges)", || replace(&store, &owner, &typical));
  assert_eq!(delta.entities.changed, 5);

  let spec = TraverseSpec {
    seeds: vec![id(&name(0, 0))],
    direction: Direction::Both,
    filter: EdgeFilter::default(),
    max_depth: 3,
    max_nodes: 20_000,
  };
  let (traversal, traverse_time) = timed("depth-3 traversal from the hub (Both, cap 20k)", || read(&store, |tx| tx.traverse(&spec)));
  eprintln!("traversal reached {} nodes, {:?}", traversal.nodes.len(), traversal.completeness);
  assert!(traversal.nodes.len() > OWNERS);
  let out_spec = TraverseSpec {
    direction: Direction::Out,
    seeds: vec![EntityId::of(&symbol_key(&name(17, 3)))],
    ..spec.clone()
  };
  let (outward, out_time) = timed("depth-3 traversal outward from a leaf function", || read(&store, |tx| tx.traverse(&out_spec)));
  eprintln!("outward traversal reached {} nodes, {:?}", outward.nodes.len(), outward.completeness);
  assert_eq!(outward.completeness, Completeness::Complete);
  let (_, digest_time) = timed("fact_set_digest", || read(&store, |tx| tx.fact_set_digest()));

  eprintln!("summary: full={full_write:?} noop={noop_time:?} typical={typical_time:?} traverse_hub={traverse_time:?} traverse_out={out_time:?} digest={digest_time:?}");
}
