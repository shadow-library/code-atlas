use std::collections::HashMap;

pub trait Store {
  fn get(&self, key: &str) -> Option<String>;
  fn put(&mut self, key: &str, value: String);
}

#[derive(Default)]
pub struct MemStore {
  entries: HashMap<String, String>,
}

pub struct DiskStore {
  root: String,
}

impl DiskStore {
  pub fn new(root: &str) -> Self {
    Self { root: root.to_string() }
  }

  pub fn path_for(&self, key: &str) -> String {
    format!("{}/{}", self.root, key)
  }
}

impl Store for MemStore {
  fn get(&self, key: &str) -> Option<String> {
    self.entries.get(key).cloned()
  }

  fn put(&mut self, key: &str, value: String) {
    self.entries.insert(key.to_string(), value);
  }
}

impl Store for DiskStore {
  fn get(&self, key: &str) -> Option<String> {
    std::fs::read_to_string(self.path_for(key)).ok()
  }

  fn put(&mut self, key: &str, value: String) {
    let _ = std::fs::write(self.path_for(key), value);
  }
}

pub fn read_generic<T: Store>(store: &T, key: &str) -> Option<String> {
  store.get(key)
}

pub fn read_dyn(store: &dyn Store, key: &str) -> Option<String> {
  store.get(key)
}

pub fn read_impl(store: impl Store, key: &str) -> Option<String> {
  store.get(key)
}

pub fn read_mem(store: &MemStore, key: &str) -> Option<String> {
  store.get(key)
}

pub fn write_boxed(mut store: Box<dyn Store>, key: &str) {
  store.put(key, "v".to_string());
}
