mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use atlas_core::Entity;
use atlas_store::{EntityFilter, OwnedFacts, Store, StoreError, StoreOptions};
use common::*;

fn versioned_facts(version: usize, count: usize) -> OwnedFacts {
  let entities: Vec<Entity> = (0..count)
    .map(|index| {
      let mut entity = entity(&format!("item{index}"));
      entity.signature = Some(format!("v{version}"));
      entity
    })
    .collect();
  let edges = (1..count).map(|index| calls(&format!("item{}", index - 1), &format!("item{index}"))).collect();
  facts(entities, edges)
}

fn signatures(store: &Store) -> Vec<Option<String>> {
  read(store, |tx| tx.entities_in_file(&repo(), &rel("src/lib.rs"), &EntityFilter::default()))
    .into_iter()
    .map(|entity| entity.signature)
    .collect()
}

#[test]
fn should_be_shareable_across_threads() {
  fn assert_send_sync<T: Send + Sync>() {}
  assert_send_sync::<Store>();
  assert_send_sync::<Arc<Store>>();
}

#[test]
fn should_roll_back_every_write_when_the_closure_fails() {
  let store = Store::open_in_memory().unwrap();
  let owner = file_owner("src/lib.rs");

  let result = store.write(|tx| {
    tx.replace_owner(&owner, &facts(vec![entity("a")], vec![]))?;
    tx.begin_run("full")?;
    Err::<(), _>(StoreError::Reentrant)
  });

  assert!(matches!(result, Err(StoreError::Reentrant)));
  assert_eq!(read(&store, |tx| tx.entity(id("a"))), None);
  assert_eq!(read(&store, |tx| tx.runs(10)), vec![]);
}

#[test]
fn should_roll_back_a_panicking_write_and_keep_serving() {
  let dir = tempfile::tempdir().unwrap();
  let store = Store::open(dir.path().join("atlas.db")).unwrap();
  let owner = file_owner("src/lib.rs");

  let outcome = catch_unwind(AssertUnwindSafe(|| {
    store.write(|tx| -> Result<(), StoreError> {
      tx.replace_owner(&owner, &facts(vec![entity("a")], vec![]))?;
      panic!("analyzer bug");
    })
  }));

  assert!(outcome.is_err());
  assert_eq!(read(&store, |tx| tx.entity(id("a"))), None);
  replace(&store, &owner, &facts(vec![entity("b")], vec![]));
  assert_eq!(read(&store, |tx| tx.entity(id("b"))), Some(entity("b")));
}

#[test]
fn should_refuse_reentrant_access_instead_of_deadlocking() {
  let store = Store::open_in_memory().unwrap();

  let nested_write = store.write(|_| store.write(|_| Ok::<_, StoreError>(())));
  let nested_read = store.write(|_| store.read(|_| Ok::<_, StoreError>(())));

  assert!(matches!(nested_write, Err(StoreError::Reentrant)));
  assert!(matches!(nested_read, Err(StoreError::Reentrant)));
  assert!(store.write(|_| Ok::<_, StoreError>(())).is_ok());
}

#[test]
fn should_refuse_a_nested_read_on_an_exhausted_reader_pool_instead_of_deadlocking() {
  let dir = tempfile::tempdir().unwrap();
  let store = Store::open_with(
    dir.path().join("atlas.db"),
    StoreOptions {
      readers: 1,
      ..StoreOptions::default()
    },
  )
  .unwrap();

  let nested = store.read(|_| store.read(|_| Ok::<_, StoreError>(())));
  let read_inside_write = store.write(|_| store.read(|_| Ok::<_, StoreError>(())));

  assert!(matches!(nested, Err(StoreError::Reentrant)), "{nested:?}");
  assert!(read_inside_write.is_ok());
  assert!(store.read(|_| Ok::<_, StoreError>(())).is_ok());
}

#[test]
fn should_serve_the_previous_snapshot_while_a_write_is_open() {
  let dir = tempfile::tempdir().unwrap();
  let store = Arc::new(Store::open(dir.path().join("atlas.db")).unwrap());
  let owner = file_owner("src/lib.rs");
  replace(&store, &owner, &versioned_facts(1, 3));
  let (started_tx, started_rx) = mpsc::channel();
  let (release_tx, release_rx) = mpsc::channel::<()>();

  let writer = {
    let store = Arc::clone(&store);
    let owner = owner.clone();
    thread::spawn(move || {
      store
        .write(|tx| {
          tx.replace_owner(&owner, &versioned_facts(2, 3))?;
          started_tx.send(()).unwrap();
          release_rx.recv().unwrap();
          Ok::<_, StoreError>(())
        })
        .unwrap();
    })
  };
  started_rx.recv().unwrap();

  assert_eq!(signatures(&store), vec![Some("v1".to_owned()); 3]);
  release_tx.send(()).unwrap();
  writer.join().unwrap();
  assert_eq!(signatures(&store), vec![Some("v2".to_owned()); 3]);
}

#[test]
fn should_never_expose_torn_reads_to_concurrent_readers() {
  let dir = tempfile::tempdir().unwrap();
  let store = Arc::new(Store::open(dir.path().join("atlas.db")).unwrap());
  let owner = file_owner("src/lib.rs");
  let size = 25;
  replace(&store, &owner, &versioned_facts(0, size));
  let done = Arc::new(AtomicBool::new(false));
  let reads = Arc::new(AtomicUsize::new(0));

  let readers: Vec<_> = (0..3)
    .map(|_| {
      let (store, done, reads) = (Arc::clone(&store), Arc::clone(&done), Arc::clone(&reads));
      thread::spawn(move || {
        while !done.load(Ordering::Acquire) {
          let (entity_versions, edge_count) = store
            .read(|tx| {
              let entities = tx.entities_in_file(&repo(), &rel("src/lib.rs"), &EntityFilter::default())?;
              let edges = entities
                .iter()
                .map(|entity| tx.edges_from(entity.id, &Default::default()).map(|edges| edges.len()))
                .sum::<Result<usize, _>>()?;
              Ok::<_, StoreError>((entities.into_iter().map(|entity| entity.signature).collect::<Vec<_>>(), edges))
            })
            .unwrap();
          assert_eq!(entity_versions.len(), size);
          assert!(entity_versions.iter().all(|version| *version == entity_versions[0]), "torn read: {entity_versions:?}");
          assert_eq!(edge_count, size - 1);
          reads.fetch_add(1, Ordering::Relaxed);
        }
      })
    })
    .collect();

  for version in 1..=60 {
    replace(&store, &owner, &versioned_facts(version, size));
  }
  done.store(true, Ordering::Release);
  for reader in readers {
    reader.join().unwrap();
  }

  assert!(reads.load(Ordering::Relaxed) > 0);
  assert_eq!(signatures(&store), vec![Some("v60".to_owned()); size]);
}

#[test]
fn should_make_a_second_store_on_the_same_file_wait_for_the_writer_lock() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("atlas.db");
  let first = Arc::new(Store::open(&path).unwrap());
  let impatient = Store::open_with(
    &path,
    StoreOptions {
      lock_timeout: Duration::from_millis(100),
      ..StoreOptions::default()
    },
  )
  .unwrap();
  let patient = Store::open(&path).unwrap();
  let (started_tx, started_rx) = mpsc::channel();
  let hold = Duration::from_millis(300);

  let holder = {
    let first = Arc::clone(&first);
    thread::spawn(move || {
      first
        .write(|tx| {
          tx.replace_owner(&file_owner("src/a.rs"), &facts(vec![entity("from_first")], vec![]))?;
          started_tx.send(Instant::now()).unwrap();
          thread::sleep(hold);
          Ok::<_, StoreError>(())
        })
        .unwrap();
    })
  };
  let held_since = started_rx.recv().unwrap();

  let timed_out = impatient.write(|_| Ok::<_, StoreError>(()));
  assert!(matches!(timed_out, Err(StoreError::LockTimeout { .. })), "{timed_out:?}");

  let seen_by_second_writer = patient.write(|tx| tx.entity(id("from_first"))).unwrap();
  assert!(held_since.elapsed() >= hold);
  assert_eq!(seen_by_second_writer, Some(entity("from_first")));
  holder.join().unwrap();
  assert!(impatient.write(|_| Ok::<_, StoreError>(())).is_ok());
}
