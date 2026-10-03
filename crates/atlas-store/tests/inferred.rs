mod common;

use atlas_core::{Certainty, ContentHash, Evidence, Freshness};
use atlas_store::{FreshnessPolicy, InferredFactId, InferredQuery, NewInferredFact, Store, StoreError};
use common::*;

fn evidence_with(path: &str, content: &[u8]) -> Evidence {
  Evidence {
    content_hash: ContentHash::of(content),
    ..evidence(path, 0)
  }
}

fn equivalence(left: &str, right: &str, evidence: Vec<Evidence>) -> NewInferredFact {
  NewInferredFact {
    subjects: vec![id(left), id(right)],
    relation: "equivalent_to".into(),
    payload: serde_json::json!({ "label": "equivalent" }),
    certainty: inferred(0.81),
    evidence,
    created_run: None,
    llm_call: None,
  }
}

fn insert(store: &Store, fact: &NewInferredFact) -> InferredFactId {
  store.write(|tx| tx.insert_inferred_fact(fact)).unwrap()
}

fn including_stale() -> InferredQuery {
  InferredQuery {
    freshness: FreshnessPolicy::IncludeStale,
    ..InferredQuery::default()
  }
}

#[test]
fn should_store_inferred_facts_with_their_subjects_and_evidence() {
  let store = Store::open_in_memory().unwrap();
  let fact = equivalence("parse_money", "to_cents", vec![evidence_with("src/a.rs", b"v1"), evidence_with("src/b.rs", b"v1")]);

  let stored_id = insert(&store, &fact);

  let stored = read(&store, |tx| tx.inferred_fact(stored_id)).unwrap();
  assert_eq!(stored.subjects, fact.subjects);
  assert_eq!(stored.relation, "equivalent_to");
  assert_eq!(stored.payload, fact.payload);
  assert_eq!(stored.certainty, fact.certainty);
  assert_eq!(stored.evidence, fact.evidence);
  assert_eq!(stored.freshness, Freshness::Current);
  let by_subject = InferredQuery {
    subject: Some(id("to_cents")),
    ..InferredQuery::default()
  };
  assert_eq!(read(&store, |tx| tx.inferred_facts(&by_subject)), vec![stored]);
  let other_subject = InferredQuery {
    subject: Some(id("unrelated")),
    ..InferredQuery::default()
  };
  assert_eq!(read(&store, |tx| tx.inferred_facts(&other_subject)), vec![]);
}

#[test]
fn should_reject_facts_that_are_not_llm_derived() {
  let store = Store::open_in_memory().unwrap();
  let mut fact = equivalence("a", "b", vec![]);
  fact.certainty = Certainty::Resolved;

  let result = store.write(|tx| tx.insert_inferred_fact(&fact));

  assert!(matches!(result, Err(StoreError::NotInferred(tag)) if tag == "[resolved]"));
}

#[test]
fn should_mark_facts_stale_only_when_their_evidence_changed() {
  let store = Store::open_in_memory().unwrap();
  let on_a = insert(&store, &equivalence("a", "b", vec![evidence_with("src/a.rs", b"v1")]));
  let on_b = insert(&store, &equivalence("c", "d", vec![evidence_with("src/b.rs", b"v1")]));

  let unchanged = store
    .write(|tx| tx.mark_stale_where_evidence_changed(&repo(), &rel("src/a.rs"), &ContentHash::of(b"v1")))
    .unwrap();
  let changed = store
    .write(|tx| tx.mark_stale_where_evidence_changed(&repo(), &rel("src/a.rs"), &ContentHash::of(b"v2")))
    .unwrap();
  let again = store
    .write(|tx| tx.mark_stale_where_evidence_changed(&repo(), &rel("src/a.rs"), &ContentHash::of(b"v3")))
    .unwrap();

  assert_eq!((unchanged, changed, again), (0, 1, 0));
  let current: Vec<_> = read(&store, |tx| tx.inferred_facts(&InferredQuery::default())).into_iter().map(|fact| fact.id).collect();
  assert_eq!(current, vec![on_b]);
  let all = read(&store, |tx| tx.inferred_facts(&including_stale()));
  assert_eq!(all.iter().map(|fact| fact.id).collect::<Vec<_>>(), vec![on_a, on_b]);
  assert_eq!(
    all[0].freshness,
    Freshness::Stale {
      reason: "acme:src/a.rs changed".into()
    }
  );
  assert_eq!(all[1].freshness, Freshness::Current);
}

#[test]
fn should_mark_facts_stale_when_their_evidence_file_disappears() {
  let store = Store::open_in_memory().unwrap();
  let fact = insert(&store, &equivalence("a", "b", vec![evidence_with("src/a.rs", b"v1")]));

  let marked = store.write(|tx| tx.mark_stale_where_evidence_removed(&repo(), &rel("src/a.rs"))).unwrap();

  assert_eq!(marked, 1);
  assert_eq!(
    read(&store, |tx| tx.inferred_fact(fact)).unwrap().freshness,
    Freshness::Stale {
      reason: "acme:src/a.rs no longer exists".into()
    }
  );
}

#[test]
fn should_flag_revalidation_without_reviving_stale_facts() {
  let store = Store::open_in_memory().unwrap();
  let current = insert(&store, &equivalence("a", "b", vec![evidence_with("src/a.rs", b"v1")]));
  let stale = insert(&store, &equivalence("c", "d", vec![evidence_with("src/b.rs", b"v1")]));
  store.write(|tx| tx.mark_stale_where_evidence_removed(&repo(), &rel("src/b.rs"))).unwrap();

  let flagged = store.write(|tx| tx.mark_needs_revalidation(current, "renamed")).unwrap();
  let revived = store.write(|tx| tx.mark_needs_revalidation(stale, "renamed")).unwrap();

  assert_eq!((flagged, revived), (true, false));
  assert_eq!(read(&store, |tx| tx.inferred_facts(&InferredQuery::default())), vec![]);
  let freshness: Vec<_> = read(&store, |tx| tx.inferred_facts(&including_stale())).into_iter().map(|fact| fact.freshness).collect();
  assert_eq!(
    freshness,
    vec![
      Freshness::NeedsRevalidation { reason: "renamed".into() },
      Freshness::Stale {
        reason: "acme:src/b.rs no longer exists".into()
      },
    ]
  );
  assert!(store.write(|tx| tx.delete_inferred_fact(stale)).unwrap());
  assert_eq!(read(&store, |tx| tx.inferred_fact(stale)), None);
}
