use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{Edge, EdgeKind, EntityId, RelPath, RepoName, Span};

const COMPARABLE_KINDS: [EdgeKind; 5] = [EdgeKind::Calls, EdgeKind::References, EdgeKind::Instantiates, EdgeKind::Implements, EdgeKind::Extends];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReconcileKey {
  Site { from: EntityId, repo: RepoName, path: RelPath, span: Span },
  Relation { kind: EdgeKind, from: EntityId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agreement {
  pub key: ReconcileKey,
  pub target: EntityId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disagreement {
  pub key: ReconcileKey,
  pub scip_targets: Vec<EntityId>,
  pub our_targets: Vec<EntityId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciliation {
  pub agreed: Vec<Agreement>,
  pub ours_only: Vec<Agreement>,
  pub scip_only: Vec<Agreement>,
  pub conflicting: Vec<Disagreement>,
}

type Targets = BTreeMap<ReconcileKey, BTreeSet<EntityId>>;

fn keyed<'a>(edges: impl IntoIterator<Item = &'a Edge>) -> Targets {
  let mut targets = Targets::new();
  for edge in edges.into_iter().filter(|edge| COMPARABLE_KINDS.contains(&edge.kind)) {
    for key in keys_of(edge) {
      targets.entry(key).or_default().insert(edge.to);
    }
  }
  targets
}

fn keys_of(edge: &Edge) -> Vec<ReconcileKey> {
  let relational = matches!(edge.kind, EdgeKind::Implements | EdgeKind::Extends) || edge.evidence.is_empty();
  if relational {
    return vec![ReconcileKey::Relation {
      kind: edge.kind.clone(),
      from: edge.from,
    }];
  }
  edge
    .evidence
    .iter()
    .map(|evidence| ReconcileKey::Site {
      from: edge.from,
      repo: evidence.repo.clone(),
      path: evidence.path.clone(),
      span: evidence.span,
    })
    .collect()
}

pub fn reconcile<'a>(scip_edges: impl IntoIterator<Item = &'a Edge>, our_edges: impl IntoIterator<Item = &'a Edge>) -> Reconciliation {
  let scip = keyed(scip_edges);
  let ours = keyed(our_edges);
  let mut result = Reconciliation::default();
  let keys: BTreeSet<&ReconcileKey> = scip.keys().chain(ours.keys()).collect();
  for key in keys {
    let empty = BTreeSet::new();
    let (scip_targets, our_targets) = (scip.get(key).unwrap_or(&empty), ours.get(key).unwrap_or(&empty));
    let record = |target: &EntityId| Agreement {
      key: key.clone(),
      target: *target,
    };
    result.agreed.extend(scip_targets.intersection(our_targets).map(record));
    let scip_rest: Vec<EntityId> = scip_targets.difference(our_targets).copied().collect();
    let our_rest: Vec<EntityId> = our_targets.difference(scip_targets).copied().collect();
    let shares_target = scip_targets.intersection(our_targets).next().is_some();
    let site_conflict = matches!(key, ReconcileKey::Site { .. }) && !shares_target && !scip_rest.is_empty() && !our_rest.is_empty();
    if site_conflict {
      result.conflicting.push(Disagreement {
        key: key.clone(),
        scip_targets: scip_rest,
        our_targets: our_rest,
      });
      continue;
    }
    result.scip_only.extend(scip_rest.iter().map(record));
    result.ours_only.extend(our_rest.iter().map(record));
  }
  result
}

#[cfg(test)]
mod tests {
  use atlas_core::{Certainty, ContentHash, Evidence, Producer};

  use super::*;

  fn id(raw: u64) -> EntityId {
    EntityId::from_raw(raw)
  }

  fn span(start: u32) -> Span {
    Span::from_zero_based_rows(start, start + 3, 0, start, 0, start + 3)
  }

  fn site(kind: EdgeKind, from: u64, to: u64, start: u32, certainty: Certainty) -> Edge {
    Edge::new(kind, id(from), id(to), "", certainty).with_evidence(Evidence {
      repo: RepoName::new("r").unwrap(),
      path: RelPath::new("src/lib.rs").unwrap(),
      span: span(start),
      content_hash: ContentHash::of(b"x"),
      producer: Producer::new("t", 1),
      note: None,
    })
  }

  fn site_key(from: u64, start: u32) -> ReconcileKey {
    ReconcileKey::Site {
      from: id(from),
      repo: RepoName::new("r").unwrap(),
      path: RelPath::new("src/lib.rs").unwrap(),
      span: span(start),
    }
  }

  fn pair(key: ReconcileKey, target: u64) -> Agreement {
    Agreement { key, target: id(target) }
  }

  #[test]
  fn should_agree_when_site_and_target_match_regardless_of_edge_kind_or_certainty() {
    let scip = [site(EdgeKind::Calls, 1, 9, 10, Certainty::CompilerVerified)];
    let ours = [site(EdgeKind::References, 1, 9, 10, Certainty::Resolved)];
    let result = reconcile(&scip, &ours);
    assert_eq!(result.agreed, vec![pair(site_key(1, 10), 9)]);
    assert!(result.ours_only.is_empty() && result.scip_only.is_empty() && result.conflicting.is_empty());
  }

  #[test]
  fn should_report_a_conflict_when_the_same_site_has_a_different_target() {
    let scip = [site(EdgeKind::Calls, 1, 9, 10, Certainty::CompilerVerified)];
    let ours = [site(EdgeKind::Calls, 1, 8, 10, Certainty::Resolved)];
    let result = reconcile(&scip, &ours);
    assert_eq!(
      result.conflicting,
      vec![Disagreement {
        key: site_key(1, 10),
        scip_targets: vec![id(9)],
        our_targets: vec![id(8)]
      }]
    );
    assert!(result.agreed.is_empty() && result.ours_only.is_empty() && result.scip_only.is_empty());
  }

  #[test]
  fn should_split_one_sided_sites_into_ours_only_and_scip_only() {
    let scip = [site(EdgeKind::Calls, 1, 9, 10, Certainty::CompilerVerified)];
    let ours = [site(EdgeKind::Calls, 1, 9, 20, Certainty::Resolved)];
    let result = reconcile(&scip, &ours);
    assert_eq!(result.scip_only, vec![pair(site_key(1, 10), 9)]);
    assert_eq!(result.ours_only, vec![pair(site_key(1, 20), 9)]);
    assert!(result.agreed.is_empty() && result.conflicting.is_empty());
  }

  #[test]
  fn should_treat_extra_heuristic_candidates_beside_an_agreed_target_as_ours_only() {
    let scip = [site(EdgeKind::Calls, 1, 9, 10, Certainty::CompilerVerified)];
    let ours = [
      site(EdgeKind::Calls, 1, 9, 10, Certainty::Resolved),
      site(EdgeKind::Calls, 1, 7, 10, Certainty::heuristic(atlas_core::Confidence::new(0.4).unwrap(), "candidate")),
    ];
    let result = reconcile(&scip, &ours);
    assert_eq!(result.agreed, vec![pair(site_key(1, 10), 9)]);
    assert_eq!(result.ours_only, vec![pair(site_key(1, 10), 7)]);
    assert!(result.conflicting.is_empty() && result.scip_only.is_empty());
  }

  #[test]
  fn should_key_sites_by_enclosing_symbol_as_well_as_span() {
    let scip = [site(EdgeKind::Calls, 1, 9, 10, Certainty::CompilerVerified)];
    let ours = [site(EdgeKind::Calls, 2, 9, 10, Certainty::Resolved)];
    let result = reconcile(&scip, &ours);
    assert_eq!(result.scip_only.len(), 1);
    assert_eq!(result.ours_only.len(), 1);
    assert!(result.conflicting.is_empty());
  }

  #[test]
  fn should_compare_implements_edges_by_pair_without_spans() {
    let scip = [
      site(EdgeKind::Implements, 1, 5, 30, Certainty::CompilerVerified),
      site(EdgeKind::Implements, 1, 6, 30, Certainty::CompilerVerified),
    ];
    let ours = [
      site(EdgeKind::Implements, 1, 5, 99, Certainty::Resolved),
      site(EdgeKind::Implements, 1, 7, 99, Certainty::Resolved),
    ];
    let result = reconcile(&scip, &ours);
    let key = ReconcileKey::Relation {
      kind: EdgeKind::Implements,
      from: id(1),
    };
    assert_eq!(result.agreed, vec![pair(key.clone(), 5)]);
    assert_eq!(result.scip_only, vec![pair(key.clone(), 6)]);
    assert_eq!(result.ours_only, vec![pair(key, 7)]);
    assert!(result.conflicting.is_empty());
  }

  #[test]
  fn should_ignore_edge_kinds_the_oracle_cannot_produce() {
    let ours = [
      site(EdgeKind::Imports, 1, 9, 10, Certainty::Resolved),
      site(EdgeKind::Contains, 1, 9, 10, Certainty::Syntactic),
    ];
    assert_eq!(reconcile(&[], &ours), Reconciliation::default());
  }

  #[test]
  fn should_expand_merged_evidence_into_one_site_per_span() {
    let mut merged = site(EdgeKind::Calls, 1, 9, 10, Certainty::CompilerVerified);
    merged.evidence.push(site(EdgeKind::Calls, 1, 9, 20, Certainty::CompilerVerified).evidence.remove(0));
    let ours = [site(EdgeKind::Calls, 1, 9, 10, Certainty::Resolved)];
    let result = reconcile(&[merged], &ours);
    assert_eq!(result.agreed, vec![pair(site_key(1, 10), 9)]);
    assert_eq!(result.scip_only, vec![pair(site_key(1, 20), 9)]);
  }

  #[test]
  fn should_be_deterministic_in_input_order() {
    let a = site(EdgeKind::Calls, 1, 9, 10, Certainty::CompilerVerified);
    let b = site(EdgeKind::Calls, 1, 8, 20, Certainty::CompilerVerified);
    let first = reconcile([&a, &b], std::iter::empty());
    let second = reconcile([&b, &a], std::iter::empty());
    assert_eq!(first, second);
  }
}
