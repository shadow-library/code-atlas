pub mod model;

use std::sync::{Mutex, MutexGuard, RwLock};

use rayon::prelude::*;

use crate::model::{Entry, Store, StoreResult};

pub struct Graph<'a> {
  store: &'a Store,
  cache: RwLock<Entry>,
}

impl<'a> Graph<'a> {
  pub fn store(&self) -> &'a Store {
    self.store
  }

  pub async fn load(&self) -> Entry {
    Entry
  }

  pub fn query(&self) -> StoreResult<()> {
    self.store().entry("k")?.unwrap().label();
    self.cache.read().unwrap().label();
    Store::open().unwrap().entry("k");
    Ok(())
  }

  pub async fn awaited(&self) {
    self.load().await.label();
    self.load().label();
  }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
  mutex.lock().unwrap()
}

pub fn spend(budget: &Mutex<Entry>, items: Vec<Entry>) {
  lock(budget).label();
  items.par_iter().for_each(|entry| entry.label());
  missing().inner.label();
}
