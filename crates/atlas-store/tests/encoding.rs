mod common;

use atlas_core::{
  Certainty, ContentHash, DeclaredBinding, Edge, EdgeKind, Entity, EntityKey, Evidence, Owner, Producer, ReceiverHint, RefKind, RepoName, UnresolvedReason, UnresolvedRef,
};
use atlas_store::{ChangeCounts, OwnedFacts, SCHEMA_VERSION, Store};
use common::*;
use rusqlite::{Connection, params};

const PINNED_DIGEST: &str = "40b9e7c8da7ab075b8769b0391ca95bc841621e301ccebdf2b3a68e5cf7634e6";

fn hash(text: &str) -> ContentHash {
  ContentHash::of(text.as_bytes())
}

fn evidence_with(path: &str, start: u32, content_hash: ContentHash, note: Option<&str>) -> Evidence {
  Evidence {
    content_hash,
    note: note.map(str::to_owned),
    ..evidence(path, start)
  }
}

fn other_repo() -> RepoName {
  RepoName::new("globex").unwrap()
}

fn reference(from: &str, path: &[&str], kind: RefKind, receiver: Option<ReceiverHint>, reason: UnresolvedReason, file: &str) -> UnresolvedRef {
  UnresolvedRef {
    kind,
    receiver,
    reason,
    ..unresolved(from, path, file)
  }
}

fn owner_a() -> (Owner, OwnedFacts) {
  let current = hash("a v2");
  let mut documented = entity_in("a", "src/a.rs");
  documented.attrs.insert("meta".into(), serde_json::json!({ "z": 1, "a": [true, null] }));
  documented.doc = Some("does a".into());
  documented.signature = Some("fn a()".into());
  let mut repo_entity = Entity::new(EntityKey::Repository { repo: other_repo() }, Certainty::Declared);
  repo_entity.attrs.insert("origin".into(), serde_json::json!("a"));
  let mut many = Edge::new(EdgeKind::Calls, id("a"), id("b"), "", Certainty::Resolved)
    .with_evidence(evidence_with("src/a.rs", 5, current, None))
    .with_evidence(evidence_with("src/a.rs", 9, current, Some("second call")))
    .with_evidence(evidence_with("src/a.rs", 12, hash("a v1"), None));
  many.attrs.insert("site".into(), serde_json::json!([1, 2]));
  let elsewhere = Edge::new(EdgeKind::References, id("a"), id("c"), "", heuristic(0.4))
    .with_evidence(evidence_with("src/b.rs", 7, hash("b v1"), None))
    .with_evidence(Evidence {
      repo: other_repo(),
      ..evidence_with("src/a.rs", 3, current, None)
    })
    .with_evidence(Evidence {
      producer: Producer::new("detector", 3),
      ..evidence_with("src/a.rs", 4, current, Some("route"))
    });
  let bare = Edge::new(EdgeKind::Contains, id("b"), id("a"), "", Certainty::Syntactic);
  (
    file_owner("src/a.rs"),
    OwnedFacts {
      entities: vec![documented, entity_in("b", "src/a.rs"), repo_entity],
      edges: vec![many, elsewhere, bare],
      unresolved: vec![
        unresolved("a", &["x"], "src/a.rs"),
        reference(
          "b",
          &["(self.items.iter())", "map"],
          RefKind::MethodCall,
          Some(ReceiverHint::Expr("self.items.iter()".into())),
          UnresolvedReason::Dynamic,
          "src/a.rs",
        ),
        reference(
          "b",
          &["HashMap"],
          RefKind::TypeRef,
          Some(ReceiverHint::Declared {
            type_text: "T".into(),
            binding: DeclaredBinding::Param,
          }),
          UnresolvedReason::External { package: "std".into() },
          "src/a.rs",
        ),
        reference(
          "a",
          &["y", "z"],
          RefKind::Call,
          Some(ReceiverHint::TypeHint(vec!["Y".into()])),
          UnresolvedReason::Ambiguous {
            candidates: vec![id("b"), id("c")],
          },
          "src/other.rs",
        ),
        UnresolvedRef {
          repo: other_repo(),
          ..unresolved("a", &["q"], "src/a.rs")
        },
        unresolved("a", &["x"], "src/a.rs"),
      ],
    },
  )
}

fn owner_b() -> (Owner, OwnedFacts) {
  let current = hash("b v1");
  (
    file_owner("src/b.rs"),
    OwnedFacts {
      entities: vec![entity_in("c", "src/b.rs"), entity_in("b", "src/b.rs")],
      edges: vec![
        Edge::new(EdgeKind::References, id("a"), id("c"), "", Certainty::Resolved).with_evidence(evidence_with("src/b.rs", 2, current, None)),
        Edge::new(EdgeKind::Calls, id("c"), id("b"), "", Certainty::Syntactic).with_evidence(evidence_with("src/b.rs", 8, current, None)),
      ],
      unresolved: vec![reference("c", &["zz"], RefKind::ValueRef, Some(ReceiverHint::This), UnresolvedReason::NotFound, "src/b.rs")],
    },
  )
}

fn owner_link() -> (Owner, OwnedFacts) {
  let mut linked = Edge::new(EdgeKind::Calls, id("a"), id("b"), "", heuristic(0.7)).with_evidence(evidence_with("src/b.rs", 1, hash("b v1"), Some("link")));
  linked.attrs.insert("site".into(), serde_json::json!("link"));
  (
    Owner::Project { pass: "link".into() },
    OwnedFacts {
      entities: vec![entity_in("c", "src/c.rs")],
      edges: vec![linked],
      unresolved: vec![],
    },
  )
}

fn populated() -> Store {
  let store = Store::open_in_memory().unwrap();
  for (owner, owned) in [owner_a(), owner_b(), owner_link()] {
    replace(&store, &owner, &owned);
  }
  store
}

fn by_id(mut owned: OwnedFacts) -> OwnedFacts {
  owned.entities.sort_by_key(|entity| entity.id.as_i64());
  owned.edges.sort_by_key(|edge| edge.id.as_i64());
  owned.unresolved.sort_by_key(|reference| serde_json::to_string(reference).unwrap());
  owned
}

#[test]
fn should_keep_the_fact_set_digest_pinned_across_storage_encodings() {
  let store = populated();

  let digest = read(&store, |tx| tx.fact_set_digest());

  assert_eq!(digest.to_string(), PINNED_DIGEST);
}

#[test]
fn should_return_every_owned_fact_exactly_as_written() {
  let store = populated();

  for (owner, owned) in [owner_a(), owner_b(), owner_link()] {
    assert_eq!(read(&store, |tx| tx.owned_facts(&owner)), by_id(owned), "{owner:?}");
  }
}

#[test]
fn should_hydrate_merged_edges_with_each_owner_content_hash() {
  let store = populated();
  let (_, a_facts) = owner_a();
  let (_, b_facts) = owner_b();
  let shared = a_facts.edges[1].id;

  let merged = read(&store, |tx| tx.edge(shared)).unwrap();

  let mut expected: Vec<Evidence> = a_facts.edges[1].evidence.iter().chain(&b_facts.edges[0].evidence).cloned().collect();
  expected.sort_by(|left, right| {
    (&left.repo, &left.path, &left.span, &left.content_hash, &left.producer, &left.note).cmp(&(
      &right.repo,
      &right.path,
      &right.span,
      &right.content_hash,
      &right.producer,
      &right.note,
    ))
  });
  assert_eq!(merged.certainty, Certainty::Resolved);
  assert_eq!(merged.evidence, expected);
}

fn rehashed(mut owned: OwnedFacts, path: &str, from: ContentHash, to: ContentHash) -> OwnedFacts {
  for evidence in owned.edges.iter_mut().flat_map(|edge| edge.evidence.iter_mut()) {
    if evidence.path == rel(path) && evidence.content_hash == from {
      evidence.content_hash = to;
    }
  }
  owned
}

#[test]
fn should_rewrite_no_edge_when_only_the_owner_file_hash_changes() {
  let store = Store::open_in_memory().unwrap();
  let (owner, before) = owner_b();
  let after = rehashed(before.clone(), "src/b.rs", hash("b v1"), hash("b v2"));
  replace(&store, &owner, &before);

  let delta = replace(&store, &owner, &after);

  let fresh = Store::open_in_memory().unwrap();
  replace(&fresh, &owner, &after);
  assert_eq!(delta.edges, ChangeCounts::default());
  assert_eq!(read(&store, |tx| tx.owned_facts(&owner)), by_id(after.clone()));
  assert_eq!(read(&store, |tx| tx.edge(after.edges[1].id)), Some(after.edges[1].clone()));
  assert_eq!(read(&store, |tx| tx.fact_set_digest()), read(&fresh, |tx| tx.fact_set_digest()));
}

#[test]
fn should_rewrite_only_the_moved_edge_when_a_body_edit_shifts_one_span() {
  let store = Store::open_in_memory().unwrap();
  let (owner, before) = owner_b();
  let mut after = rehashed(before.clone(), "src/b.rs", hash("b v1"), hash("b v2"));
  after.edges[1].evidence[0].span = span(40);
  replace(&store, &owner, &before);

  let delta = replace(&store, &owner, &after);

  assert_eq!(delta.edges, ChangeCounts { added: 0, removed: 0, changed: 1 });
  assert_eq!(read(&store, |tx| tx.owned_facts(&owner)), by_id(after));
}

#[test]
fn should_keep_explicit_hashes_for_evidence_that_disagrees_with_its_owner() {
  let store = Store::open_in_memory().unwrap();
  let (owner, owned) = owner_a();
  replace(&store, &owner, &owned);

  let stale = read(&store, |tx| tx.edge(owned.edges[0].id)).unwrap();

  assert_eq!(
    stale.evidence.iter().map(|item| item.content_hash).collect::<Vec<_>>(),
    vec![hash("a v2"), hash("a v2"), hash("a v1")]
  );
}

fn legacy_v1_database(path: &std::path::Path) {
  let conn = Connection::open(path).unwrap();
  conn.execute_batch(include_str!("../src/schema_v1.sql")).unwrap();
  conn.pragma_update(None, "user_version", 1).unwrap();
  for (owner, owned) in [owner_a(), owner_b(), owner_link()] {
    let (kind, repo, path, pass) = match &owner {
      Owner::File { repo, path } => ("file", Some(repo.to_string()), Some(path.to_string()), None),
      Owner::Project { pass } => ("project", None, None, Some(pass.clone())),
    };
    conn
      .execute(
        "INSERT INTO owners (key, kind, repo, path, pass) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![serde_json::to_string(&owner).unwrap(), kind, repo, path, pass],
      )
      .unwrap();
    let owner_id = conn.last_insert_rowid();
    for entity in &owned.entities {
      conn
        .execute(
          "INSERT OR IGNORE INTO entities (id, key, kind, name, repo, path, certainty_class, certainty_tier) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
          params![
            entity.id.as_i64(),
            entity.key.to_string(),
            entity.kind.as_str(),
            entity.name,
            entity.repo.as_ref().map(ToString::to_string),
            entity.path.as_ref().map(ToString::to_string),
            entity.certainty.class().as_str()
          ],
        )
        .unwrap();
      conn
        .execute(
          "INSERT INTO entity_owners (entity_id, owner_id, run_id, data) VALUES (?1, ?2, NULL, ?3)",
          params![entity.id.as_i64(), owner_id, serde_json::to_string(entity).unwrap()],
        )
        .unwrap();
    }
    for edge in &owned.edges {
      conn
        .execute(
          "INSERT OR IGNORE INTO edges (id, kind, src, dst, certainty_class, certainty_tier) VALUES (?1, ?2, ?3, ?4, ?5, 0)",
          params![edge.id.as_i64(), edge.kind.as_str(), edge.from.as_i64(), edge.to.as_i64(), edge.certainty.class().as_str()],
        )
        .unwrap();
      conn
        .execute(
          "INSERT INTO edge_owners (edge_id, owner_id, run_id, data) VALUES (?1, ?2, NULL, ?3)",
          params![edge.id.as_i64(), owner_id, serde_json::to_string(edge).unwrap()],
        )
        .unwrap();
    }
    let mut references: Vec<(String, &UnresolvedRef)> = owned.unresolved.iter().map(|reference| (serde_json::to_string(reference).unwrap(), reference)).collect();
    references.sort_by(|left, right| left.0.cmp(&right.0));
    for (ordinal, (data, reference)) in references.into_iter().enumerate() {
      conn
        .execute(
          "INSERT INTO unresolved_refs (owner_id, ordinal, name, src, run_id, data) VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
          params![owner_id, ordinal as i64, reference.path.last().unwrap(), reference.from.as_i64(), data],
        )
        .unwrap();
    }
  }
}

#[test]
fn should_migrate_a_version_one_database_without_changing_any_fact() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  legacy_v1_database(&path);

  let store = Store::open(&path).unwrap();

  assert_eq!(read(&store, |tx| tx.fact_set_digest()).to_string(), PINNED_DIGEST);
  for (owner, owned) in [owner_a(), owner_b(), owner_link()] {
    assert_eq!(read(&store, |tx| tx.owned_facts(&owner)), by_id(owned), "{owner:?}");
  }
  let raw = Connection::open(&path).unwrap();
  let version: i64 = raw.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap();
  let verbose: i64 = raw
    .query_row(
      "SELECT (SELECT COUNT(*) FROM edge_owners WHERE data LIKE '%content_hash%') + (SELECT COUNT(*) FROM unresolved_refs WHERE data LIKE '%\"file\"%')",
      [],
      |row| row.get(0),
    )
    .unwrap();
  let owner_hash: Option<String> = raw.query_row("SELECT content_hash FROM owners WHERE path = 'src/a.rs'", [], |row| row.get(0)).unwrap();
  assert_eq!(version, SCHEMA_VERSION);
  assert_eq!(verbose, 0);
  assert_eq!(owner_hash, Some(hash("a v2").to_string()));
}

#[test]
fn should_answer_identically_after_reopening_a_migrated_database() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  legacy_v1_database(&path);
  drop(Store::open(&path).unwrap());

  let reopened = Store::open(&path).unwrap();
  let (owner, owned) = owner_a();
  let delta = replace(&reopened, &owner, &owned);

  assert_eq!(delta, atlas_store::FactDelta::default());
  assert_eq!(read(&reopened, |tx| tx.fact_set_digest()).to_string(), PINNED_DIGEST);
}

#[test]
fn should_hydrate_unresolved_lookups_like_the_owned_facts_of_a_migrated_database() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  legacy_v1_database(&path);
  let store = Store::open(&path).unwrap();
  let owned = |owner: &Owner| read(&store, |tx| tx.owned_facts(owner)).unresolved;
  let (a, _) = owner_a();
  let (b, _) = owner_b();
  let tagged = |owner: &Owner| {
    owned(owner)
      .into_iter()
      .map(|reference| atlas_store::OwnedUnresolved { owner: owner.clone(), reference })
      .collect::<Vec<_>>()
  };
  let all: Vec<_> = tagged(&a).into_iter().chain(tagged(&b)).collect();

  let in_repo = read(&store, |tx| tx.unresolved_in_repo(&repo()));
  let any_name = read(&store, |tx| tx.unresolved_with_name_prefix(""));
  let in_file = read(&store, |tx| tx.unresolved_in_file_with_name_prefix(&repo(), &rel("src/a.rs"), "Hash"));

  assert_eq!(in_repo, all);
  assert_eq!(any_name, all);
  assert_eq!(
    in_file,
    owned(&a)
      .into_iter()
      .filter(|reference| reference.path.last().is_some_and(|name| name.starts_with("Hash")))
      .collect::<Vec<_>>()
  );
  assert_eq!(in_file.len(), 1);
  assert!(all.iter().any(|found| found.reference.repo == other_repo()));
  assert!(all.iter().any(|found| found.reference.file == rel("src/other.rs")));
}

fn schema_version(path: &std::path::Path) -> i64 {
  Connection::open(path).unwrap().query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap()
}

fn index_lock(path: &std::path::Path) -> fd_lock::RwLock<std::fs::File> {
  let mut name = path.as_os_str().to_owned();
  name.push(".index.lock");
  let file = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(name).unwrap();
  fd_lock::RwLock::new(file)
}

#[test]
fn should_wait_for_a_running_indexer_before_migrating() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  legacy_v1_database(&path);
  let mut lock = index_lock(&path);
  let guard = lock.try_write().unwrap();
  let options = |timeout_ms| atlas_store::StoreOptions {
    lock_timeout: std::time::Duration::from_millis(timeout_ms),
    ..atlas_store::StoreOptions::default()
  };

  let refused = Store::open_with(&path, options(100));
  let version_while_held = schema_version(&path);
  drop(guard);
  let store = Store::open_with(&path, options(100)).unwrap();

  let Err(
    refused @ atlas_store::StoreError::LockTimeout {
      lock: atlas_store::StoreLock::Index,
      ..
    },
  ) = refused
  else {
    panic!("expected an index lock timeout, got {:?}", refused.err());
  };
  assert!(refused.to_string().contains("waiting for the index lock"), "{refused}");
  assert_eq!(version_while_held, 1);
  assert_eq!(schema_version(&path), SCHEMA_VERSION);
  assert_eq!(read(&store, |tx| tx.fact_set_digest()).to_string(), PINNED_DIGEST);
}

#[test]
fn should_name_the_database_and_the_fix_when_a_legacy_row_cannot_be_migrated() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  legacy_v1_database(&path);
  Connection::open(&path)
    .unwrap()
    .execute(
      "UPDATE edge_owners SET data = '{\"not\":\"an edge\"}' WHERE rowid = (SELECT MIN(rowid) FROM edge_owners)",
      [],
    )
    .unwrap();

  let opened = Store::open(&path);

  let Err(error @ atlas_store::StoreError::Migration { from: 1, .. }) = opened else {
    panic!("expected a migration error, got {:?}", opened.err());
  };
  let message = error.to_string();
  assert!(message.contains(&path.display().to_string()), "{message}");
  assert!(message.contains("delete the database with its -wal and -shm files and re-index"), "{message}");
  assert_eq!(schema_version(&path), 1);
}

fn every_key_with_its_display_name() -> Vec<(EntityKey, &'static str)> {
  use atlas_core::{EntityKind, HttpMethod, PackageEcosystem};
  let repo = || RepoName::new("acme").unwrap();
  vec![
    (symbol_key("parse"), "parse"),
    (EntityKey::Repository { repo: repo() }, "acme"),
    (
      EntityKey::File {
        repo: repo(),
        path: rel("src/a.rs"),
      },
      "src/a.rs",
    ),
    (
      EntityKey::Package {
        ecosystem: PackageEcosystem::Npm,
        name: "@acme/sdk".into(),
      },
      "@acme/sdk",
    ),
    (EntityKey::Application { repo: repo(), name: "web".into() }, "web"),
    (
      EntityKey::Service {
        repo: repo(),
        name: "billing".into(),
      },
      "billing",
    ),
    (
      EntityKey::Endpoint {
        repo: repo(),
        method: HttpMethod::Get,
        route: "/orders/:id".into(),
      },
      "GET /orders/:id",
    ),
    (
      EntityKey::ClientCall {
        repo: repo(),
        method: HttpMethod::Post,
        target: "/orders".into(),
      },
      "POST /orders",
    ),
    (
      EntityKey::AuthScheme {
        repo: repo(),
        scheme: "bearer".into(),
      },
      "bearer",
    ),
    (
      EntityKey::Permission {
        repo: repo(),
        name: "orders:read".into(),
      },
      "orders:read",
    ),
    (
      EntityKey::ConfigKey {
        repo: repo(),
        key: "db.url".into(),
      },
      "db.url",
    ),
    (
      EntityKey::EnvVar {
        repo: repo(),
        name: "DATABASE_URL".into(),
      },
      "DATABASE_URL",
    ),
    (EntityKey::Event { name: "order.placed".into() }, "order.placed"),
    (EntityKey::Queue { name: "emails".into() }, "emails"),
    (
      EntityKey::Job {
        repo: repo(),
        name: "nightly".into(),
      },
      "nightly",
    ),
    (EntityKey::DbTable { name: "orders".into() }, "orders"),
    (EntityKey::ExternalService { name: "stripe".into() }, "stripe"),
    (EntityKey::Capability { name: "payments".into() }, "payments"),
    (
      EntityKey::Other {
        kind: EntityKind::Custom("widget".into()),
        repo: Some(repo()),
        name: "gear".into(),
      },
      "gear",
    ),
  ]
}

#[test]
fn should_pin_the_display_name_that_elided_entity_names_rehydrate_from() {
  let store = Store::open_in_memory().unwrap();
  let keys = every_key_with_its_display_name();
  let entities: Vec<Entity> = keys.iter().map(|(key, _)| Entity::new(key.clone(), Certainty::Declared)).collect();
  let owner = file_owner("src/a.rs");
  replace(&store, &owner, &facts(entities.clone(), vec![]));

  let names: Vec<(String, &str)> = keys.iter().map(|(key, expected)| (key.display_name(), *expected)).collect();

  for (actual, expected) in &names {
    assert_eq!(actual, expected);
  }
  assert_eq!(keys.len(), 19);
  assert_eq!(read(&store, |tx| tx.owned_facts(&owner)).entities, by_id(facts(entities, vec![])).entities);
}

fn fingerprint_blob(store: &Store) -> Option<(u32, Vec<u8>)> {
  read(store, |tx| tx.blob(atlas_store::PROJECT_FINGERPRINT_BLOB)).map(|blob| (blob.version, blob.bytes))
}

#[test]
fn should_supersede_the_adopted_project_fingerprint_when_upgrading_the_schema() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  legacy_v1_database(&path);
  Connection::open(&path)
    .unwrap()
    .execute(
      "INSERT INTO derived_blobs (name, version, bytes) VALUES (?1, 1, ?2)",
      params![atlas_store::PROJECT_FINGERPRINT_BLOB, b"0123abcd".to_vec()],
    )
    .unwrap();

  let migrated = Store::open(&path).unwrap();
  let fresh = Store::open(dir.path().join("fresh.db")).unwrap();

  let (version, bytes) = fingerprint_blob(&migrated).unwrap();
  assert_eq!(version, atlas_store::PROJECT_FINGERPRINT_VERSION);
  assert_eq!(String::from_utf8(bytes).unwrap(), "superseded by a store schema upgrade");
  assert_eq!(fingerprint_blob(&fresh), None);
}
