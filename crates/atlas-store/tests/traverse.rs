mod common;

use atlas_core::{Certainty, CertaintyClass, EdgeId, EdgeKind, EntityId};
use atlas_store::{Completeness, Direction, EdgeFilter, InferredPolicy, ReachedNode, Store, TraverseSpec};
use common::*;

fn graph() -> Store {
  let store = Store::open_in_memory().unwrap();
  let names = ["a", "b", "c", "d", "e", "x"];
  let edges = vec![
    calls("a", "b"),
    calls("a", "c"),
    calls("b", "d"),
    edge(EdgeKind::References, "c", "d", heuristic(0.5), "src/lib.rs"),
    calls("d", "e"),
    edge(EdgeKind::Imports, "x", "a", Certainty::Syntactic, "src/lib.rs"),
    edge(EdgeKind::SimilarTo, "e", "a", inferred(0.7), "src/lib.rs"),
  ];
  replace(&store, &file_owner("src/lib.rs"), &facts(names.iter().map(|name| entity(name)).collect(), edges));
  store
}

fn spec(seeds: &[&str], direction: Direction, max_depth: u32, max_nodes: usize) -> TraverseSpec {
  TraverseSpec {
    seeds: seeds.iter().map(|name| id(name)).collect(),
    direction,
    filter: EdgeFilter {
      inferred: InferredPolicy::Exclude,
      ..EdgeFilter::default()
    },
    max_depth,
    max_nodes,
  }
}

fn depths(nodes: &[ReachedNode]) -> Vec<(EntityId, u32)> {
  let mut depths: Vec<_> = nodes.iter().map(|node| (node.id, node.depth)).collect();
  depths.sort();
  depths
}

fn expected(pairs: &[(&str, u32)]) -> Vec<(EntityId, u32)> {
  sorted(pairs.iter().map(|(name, depth)| (id(name), *depth)).collect())
}

fn edge_id(kind: EdgeKind, from: &str, to: &str) -> EdgeId {
  EdgeId::of(&kind, id(from), id(to), "")
}

fn assert_explained(store: &Store, spec: &TraverseSpec, node: &ReachedNode) {
  assert_eq!(node.path.len() as u32, node.depth, "path length must equal depth for {:?}", node.id);
  if node.path.is_empty() {
    assert!(spec.seeds.contains(&node.id));
    return;
  }
  let edges: Vec<_> = node.path.iter().map(|edge| read(store, |tx| tx.edge(*edge)).unwrap()).collect();
  let mut candidates: Vec<EntityId> = spec.seeds.clone();
  for edge in &edges {
    let next: Vec<EntityId> = candidates
      .iter()
      .filter_map(|current| match spec.direction {
        Direction::Out => (edge.from == *current).then_some(edge.to),
        Direction::In => (edge.to == *current).then_some(edge.from),
        Direction::Both if edge.from == *current => Some(edge.to),
        Direction::Both => (edge.to == *current).then_some(edge.from),
      })
      .collect();
    assert!(!next.is_empty(), "edge {:?} does not continue the path to {:?}", edge.id, node.id);
    candidates = next;
  }
  assert!(candidates.contains(&node.id));
}

#[test]
fn should_reach_nodes_with_their_shortest_explaining_paths() {
  let store = graph();
  let spec = spec(&["a"], Direction::Out, 3, 100);

  let traversal = read(&store, |tx| tx.traverse(&spec));

  assert_eq!(traversal.completeness, Completeness::Complete);
  assert_eq!(depths(&traversal.nodes), expected(&[("a", 0), ("b", 1), ("c", 1), ("d", 2), ("e", 3)]));
  assert_eq!(traversal.nodes[0].id, id("a"));
  for node in &traversal.nodes {
    assert_explained(&store, &spec, node);
  }
}

#[test]
fn should_only_follow_edges_admitted_by_the_filter() {
  let store = graph();
  let mut calls_only = spec(&["a"], Direction::Out, 5, 100);
  calls_only.filter.kinds = vec![EdgeKind::Calls];
  let mut resolved_only = spec(&["c"], Direction::Out, 5, 100);
  resolved_only.filter.min_certainty = Some(CertaintyClass::Resolved);
  let mut with_inferred = spec(&["e"], Direction::Out, 1, 100);
  with_inferred.filter.inferred = InferredPolicy::Include;

  let traversal = read(&store, |tx| tx.traverse(&calls_only));
  let path_to_d = traversal.nodes.iter().find(|node| node.id == id("d")).map(|node| node.path.clone());
  assert_eq!(path_to_d, Some(vec![edge_id(EdgeKind::Calls, "a", "b"), edge_id(EdgeKind::Calls, "b", "d")]));
  assert_eq!(depths(&read(&store, |tx| tx.traverse(&resolved_only)).nodes), expected(&[("c", 0)]));
  assert_eq!(depths(&read(&store, |tx| tx.traverse(&spec(&["e"], Direction::Out, 1, 100))).nodes), expected(&[("e", 0)]));
  assert_eq!(depths(&read(&store, |tx| tx.traverse(&with_inferred)).nodes), expected(&[("e", 0), ("a", 1)]));
}

#[test]
fn should_walk_incoming_and_both_directions() {
  let store = graph();
  let incoming = spec(&["e"], Direction::In, 10, 100);
  let both = spec(&["d"], Direction::Both, 1, 100);

  let upstream = read(&store, |tx| tx.traverse(&incoming));
  let around = read(&store, |tx| tx.traverse(&both));

  assert_eq!(depths(&upstream.nodes), expected(&[("e", 0), ("d", 1), ("b", 2), ("c", 2), ("a", 3), ("x", 4)]));
  assert_eq!(depths(&around.nodes), expected(&[("d", 0), ("b", 1), ("c", 1), ("e", 1)]));
  for node in &upstream.nodes {
    assert_explained(&store, &incoming, node);
  }
  for node in &around.nodes {
    assert_explained(&store, &both, node);
  }
}

#[test]
fn should_report_truncation_only_when_the_node_cap_cut_the_result() {
  let store = graph();

  let capped = read(&store, |tx| tx.traverse(&spec(&["a"], Direction::Out, 10, 3)));
  let exact_fit = read(&store, |tx| tx.traverse(&spec(&["a"], Direction::Out, 10, 5)));
  let seeds_over_cap = read(&store, |tx| tx.traverse(&spec(&["a", "b", "c"], Direction::Out, 10, 2)));
  let depth_limited = read(&store, |tx| tx.traverse(&spec(&["a"], Direction::Out, 1, 100)));

  assert_eq!(capped.completeness, Completeness::TruncatedAtNodeCap { max_nodes: 3 });
  assert_eq!(depths(&capped.nodes), expected(&[("a", 0), ("b", 1), ("c", 1)]));
  assert_eq!(exact_fit.completeness, Completeness::Complete);
  assert_eq!(exact_fit.nodes.len(), 5);
  assert_eq!(seeds_over_cap.completeness, Completeness::TruncatedAtNodeCap { max_nodes: 2 });
  assert_eq!(seeds_over_cap.nodes.len(), 2);
  assert_eq!(depth_limited.completeness, Completeness::Complete);
  assert_eq!(depths(&depth_limited.nodes), expected(&[("a", 0), ("b", 1), ("c", 1)]));
}

#[test]
fn should_traverse_deterministically() {
  let store = graph();
  let both = spec(&["a", "e"], Direction::Both, 10, 100);

  let first = read(&store, |tx| tx.traverse(&both));
  let mut reversed = both.clone();
  reversed.seeds.reverse();
  let second = read(&store, |tx| tx.traverse(&reversed));

  assert_eq!(first, second);
}
