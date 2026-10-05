pub type StoreResult<T> = Result<T, ()>;

pub struct Entry;

impl Entry {
  pub fn label(&self) -> &str {
    "entry"
  }
}

pub struct Store;

impl Store {
  pub fn open() -> StoreResult<Self> {
    Ok(Store)
  }

  pub fn entry(&self, key: &str) -> StoreResult<Option<Entry>> {
    Ok(None)
  }
}
