use std::collections::{HashMap, HashSet};
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use atlas_core::ContentHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
  pub len: u64,
  pub modified: Option<SystemTime>,
}

impl FileStamp {
  pub fn of(metadata: &Metadata) -> Self {
    Self {
      len: metadata.len(),
      modified: metadata.modified().ok(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDoc {
  pub version: i32,
  pub hash: ContentHash,
  pub bytes: usize,
  pub stamp: FileStamp,
  used: u64,
}

#[derive(Debug, Default)]
pub struct OpenDocs {
  docs: HashMap<PathBuf, OpenDoc>,
  last_version: i32,
  uses: u64,
  bytes: usize,
}

impl OpenDocs {
  pub fn get(&self, path: &Path) -> Option<&OpenDoc> {
    self.docs.get(path)
  }

  pub fn len(&self) -> usize {
    self.docs.len()
  }

  pub fn is_empty(&self) -> bool {
    self.docs.is_empty()
  }

  pub fn total_bytes(&self) -> usize {
    self.bytes
  }

  pub fn paths(&self) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = self.docs.keys().cloned().collect();
    paths.sort();
    paths
  }

  /// Versions come from one counter per server, so a document reopened after `didClose` still gets a higher version.
  pub fn next_version(&self) -> i32 {
    self.last_version.saturating_add(1)
  }

  pub fn reserve_version(&mut self) -> i32 {
    self.last_version = self.next_version();
    self.last_version
  }

  pub fn record(&mut self, path: &Path, version: i32, hash: ContentHash, bytes: usize, stamp: FileStamp) {
    self.uses += 1;
    self.last_version = self.last_version.max(version);
    let doc = OpenDoc {
      version,
      hash,
      bytes,
      stamp,
      used: self.uses,
    };
    if let Some(previous) = self.docs.insert(path.to_path_buf(), doc) {
      self.bytes -= previous.bytes;
    }
    self.bytes += bytes;
  }

  pub fn touch(&mut self, path: &Path, stamp: FileStamp) {
    self.uses += 1;
    if let Some(doc) = self.docs.get_mut(path) {
      doc.used = self.uses;
      doc.stamp = stamp;
    }
  }

  pub fn remove(&mut self, path: &Path) -> Option<OpenDoc> {
    let removed = self.docs.remove(path)?;
    self.bytes -= removed.bytes;
    Some(removed)
  }

  pub fn excess(&self, max_documents: usize, max_bytes: usize, keep: &HashSet<PathBuf>) -> Vec<PathBuf> {
    let mut candidates: Vec<(&PathBuf, &OpenDoc)> = self.docs.iter().filter(|(path, _)| !keep.contains(*path)).collect();
    candidates.sort_by_key(|(_, doc)| doc.used);
    let mut documents = self.docs.len();
    let mut bytes = self.bytes;
    let mut victims = Vec::new();
    for (path, doc) in candidates {
      if documents <= max_documents && bytes <= max_bytes {
        break;
      }
      documents -= 1;
      bytes -= doc.bytes;
      victims.push(path.clone());
    }
    victims
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn stamp(len: u64) -> FileStamp {
    FileStamp { len, modified: None }
  }

  #[test]
  fn should_keep_versions_increasing_across_close_and_reopen() {
    let mut docs = OpenDocs::default();
    let path = Path::new("/w/a.rs");

    let first = docs.next_version();
    docs.record(path, first, ContentHash::of(b"a"), 1, stamp(1));
    let second = docs.next_version();
    docs.record(path, second, ContentHash::of(b"ab"), 2, stamp(2));
    docs.remove(path);
    let reopened = docs.next_version();

    assert_eq!((first, second, reopened), (1, 2, 3));
    assert_eq!(docs.total_bytes(), 0);
    assert!(docs.is_empty());
  }

  #[test]
  fn should_track_the_bytes_of_replaced_documents() {
    let mut docs = OpenDocs::default();
    docs.record(Path::new("/w/a.rs"), 1, ContentHash::of(b"a"), 10, stamp(10));
    docs.record(Path::new("/w/b.rs"), 2, ContentHash::of(b"b"), 5, stamp(5));
    docs.record(Path::new("/w/a.rs"), 3, ContentHash::of(b"c"), 7, stamp(7));

    assert_eq!(docs.total_bytes(), 12);
    assert_eq!(docs.len(), 2);
    assert_eq!(docs.get(Path::new("/w/a.rs")).map(|doc| doc.version), Some(3));
  }

  #[test]
  fn should_close_the_least_recently_used_documents_over_the_limits_except_kept_ones() {
    let mut docs = OpenDocs::default();
    for (version, name) in ["a", "b", "c", "d"].iter().enumerate() {
      docs.record(
        &PathBuf::from(format!("/w/{name}.rs")),
        version as i32 + 1,
        ContentHash::of(name.as_bytes()),
        100,
        stamp(100),
      );
    }
    docs.touch(Path::new("/w/a.rs"), stamp(100));
    let keep: HashSet<PathBuf> = [PathBuf::from("/w/b.rs")].into();

    assert_eq!(docs.excess(2, usize::MAX, &keep), vec![PathBuf::from("/w/c.rs"), PathBuf::from("/w/d.rs")]);
    assert_eq!(docs.excess(10, 250, &HashSet::new()), vec![PathBuf::from("/w/b.rs"), PathBuf::from("/w/c.rs")]);
    assert_eq!(docs.excess(4, 400, &HashSet::new()), Vec::<PathBuf>::new());
  }
}
