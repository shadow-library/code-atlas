use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;

use atlas_core::{ContentHash, RelPath};
use rayon::prelude::*;

use crate::discovery::DiscoveredFile;

const RACY_WINDOW_NS: i64 = 2_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
  pub size: u64,
  pub mtime_ns: i64,
  pub inode: u64,
  pub hash: ContentHash,
}

impl FileStamp {
  fn matches_stat(&self, file: &DiscoveredFile) -> bool {
    self.size == file.size && self.mtime_ns == file.mtime_ns && self.inode == file.inode
  }

  fn is_racy(&self, previous_scan_ns: i64) -> bool {
    previous_scan_ns.saturating_sub(self.mtime_ns) < RACY_WINDOW_NS
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeError {
  pub path: RelPath,
  pub message: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
  pub added: Vec<RelPath>,
  pub modified: Vec<RelPath>,
  pub deleted: Vec<RelPath>,
  pub unchanged: Vec<RelPath>,
  pub restamped: Vec<RelPath>,
  pub renamed_files: Vec<(RelPath, RelPath)>,
  pub stamps: BTreeMap<RelPath, FileStamp>,
  pub errors: Vec<ChangeError>,
}

impl ChangeSet {
  pub fn needs_reindex(&self) -> bool {
    !(self.added.is_empty() && self.modified.is_empty() && self.deleted.is_empty())
  }
}

enum HashOutcome {
  Hashed(ContentHash),
  Vanished,
  Failed(String),
}

fn hash_file(path: &Path) -> HashOutcome {
  match fs::read(path) {
    Ok(content) => HashOutcome::Hashed(ContentHash::of(&content)),
    Err(error) if error.kind() == io::ErrorKind::NotFound => HashOutcome::Vanished,
    Err(error) => HashOutcome::Failed(error.to_string()),
  }
}

pub fn detect_changes(repo_root: &Path, discovered: &[DiscoveredFile], previous: &HashMap<RelPath, FileStamp>, previous_scan_ns: i64) -> ChangeSet {
  let mut changes = ChangeSet::default();
  let mut to_hash: Vec<&DiscoveredFile> = Vec::new();

  for file in discovered {
    match previous.get(&file.path) {
      Some(stamp) if stamp.matches_stat(file) && !stamp.is_racy(previous_scan_ns) => {
        changes.unchanged.push(file.path.clone());
        changes.stamps.insert(file.path.clone(), stamp.clone());
      }
      _ => to_hash.push(file),
    }
  }

  let outcomes: Vec<HashOutcome> = to_hash.par_iter().map(|file| hash_file(&repo_root.join(file.path.as_str()))).collect();

  for (file, outcome) in to_hash.into_iter().zip(outcomes) {
    let before = previous.get(&file.path);
    match outcome {
      HashOutcome::Hashed(hash) => {
        let stamp = FileStamp {
          size: file.size,
          mtime_ns: file.mtime_ns,
          inode: file.inode,
          hash,
        };
        match before {
          None => changes.added.push(file.path.clone()),
          Some(old) if old.hash != hash => changes.modified.push(file.path.clone()),
          Some(old) if old.matches_stat(file) => changes.unchanged.push(file.path.clone()),
          Some(_) => changes.restamped.push(file.path.clone()),
        }
        changes.stamps.insert(file.path.clone(), stamp);
      }
      HashOutcome::Vanished => {
        if before.is_some() {
          changes.deleted.push(file.path.clone());
        }
      }
      HashOutcome::Failed(message) => {
        if before.is_some() {
          changes.deleted.push(file.path.clone());
        }
        changes.errors.push(ChangeError { path: file.path.clone(), message });
      }
    }
  }

  let seen: HashSet<&RelPath> = discovered.iter().map(|file| &file.path).collect();
  changes.deleted.extend(previous.keys().filter(|path| !seen.contains(path)).cloned());

  changes.renamed_files = pair_renames(&changes, previous);
  for list in [
    &mut changes.added,
    &mut changes.modified,
    &mut changes.deleted,
    &mut changes.unchanged,
    &mut changes.restamped,
  ] {
    list.sort();
  }
  changes.errors.sort_by(|a, b| a.path.cmp(&b.path));
  changes
}

fn pair_renames(changes: &ChangeSet, previous: &HashMap<RelPath, FileStamp>) -> Vec<(RelPath, RelPath)> {
  let mut removed: HashMap<ContentHash, Vec<&RelPath>> = HashMap::new();
  for path in &changes.deleted {
    if let Some(stamp) = previous.get(path) {
      removed.entry(stamp.hash).or_default().push(path);
    }
  }
  let mut created: HashMap<ContentHash, Vec<&RelPath>> = HashMap::new();
  for path in &changes.added {
    if let Some(stamp) = changes.stamps.get(path) {
      created.entry(stamp.hash).or_default().push(path);
    }
  }
  let mut renames: Vec<(RelPath, RelPath)> = removed
    .into_iter()
    .filter_map(|(hash, from)| match (from.as_slice(), created.get(&hash).map(Vec::as_slice)) {
      ([from], Some([to])) => Some(((*from).clone(), (*to).clone())),
      _ => None,
    })
    .collect();
  renames.sort();
  renames
}

#[cfg(test)]
mod tests {
  use tempfile::TempDir;

  use super::*;
  use crate::discovery::{inode, mtime_ns};

  struct Tree {
    dir: TempDir,
  }

  impl Tree {
    fn new() -> Self {
      Self {
        dir: tempfile::tempdir().unwrap(),
      }
    }

    fn write(&self, name: &str, content: &str) {
      let path = self.dir.path().join(name);
      fs::create_dir_all(path.parent().unwrap()).unwrap();
      fs::write(path, content).unwrap();
    }

    fn file(&self, name: &str) -> DiscoveredFile {
      let metadata = fs::metadata(self.dir.path().join(name)).unwrap();
      DiscoveredFile {
        path: RelPath::new(name).unwrap(),
        language: None,
        size: metadata.len(),
        mtime_ns: mtime_ns(&metadata),
        inode: inode(&metadata),
      }
    }

    fn detect(&self, names: &[&str], previous: &HashMap<RelPath, FileStamp>, scan_ns: i64) -> ChangeSet {
      let files: Vec<_> = names.iter().map(|name| self.file(name)).collect();
      detect_changes(self.dir.path(), &files, previous, scan_ns)
    }
  }

  fn rel(path: &str) -> RelPath {
    RelPath::new(path).unwrap()
  }

  fn stamps(changes: &ChangeSet) -> HashMap<RelPath, FileStamp> {
    changes.stamps.iter().map(|(path, stamp)| (path.clone(), stamp.clone())).collect()
  }

  const LONG_AFTER: i64 = i64::MAX / 2;

  #[test]
  fn should_report_every_new_file_as_added_with_a_stamp() {
    let tree = Tree::new();
    tree.write("a.ts", "one");
    tree.write("b.ts", "two");
    let changes = tree.detect(&["a.ts", "b.ts"], &HashMap::new(), LONG_AFTER);
    assert_eq!(changes.added, [rel("a.ts"), rel("b.ts")]);
    assert_eq!(changes.stamps[&rel("a.ts")].hash, ContentHash::of(b"one"));
    assert!(changes.errors.is_empty());
  }

  #[test]
  fn should_treat_a_stat_identical_file_as_unchanged_without_hashing() {
    let tree = Tree::new();
    tree.write("a.ts", "one");
    let first = tree.detect(&["a.ts"], &HashMap::new(), LONG_AFTER);
    let mut previous = stamps(&first);
    previous.get_mut(&rel("a.ts")).unwrap().hash = ContentHash::of(b"lies about content");
    let second = tree.detect(&["a.ts"], &previous, LONG_AFTER);
    assert_eq!(second.unchanged, [rel("a.ts")]);
    assert!(second.modified.is_empty());
    assert_eq!(second.stamps[&rel("a.ts")].hash, ContentHash::of(b"lies about content"));
  }

  #[test]
  fn should_report_modified_when_content_changes_with_the_stat() {
    let tree = Tree::new();
    tree.write("a.ts", "one");
    let previous = stamps(&tree.detect(&["a.ts"], &HashMap::new(), LONG_AFTER));
    tree.write("a.ts", "three");
    let changes = tree.detect(&["a.ts"], &previous, LONG_AFTER);
    assert_eq!(changes.modified, [rel("a.ts")]);
    assert_eq!(changes.stamps[&rel("a.ts")].hash, ContentHash::of(b"three"));
  }

  #[test]
  fn should_report_restamped_when_only_the_stat_changes() {
    let tree = Tree::new();
    tree.write("a.ts", "same");
    let previous = stamps(&tree.detect(&["a.ts"], &HashMap::new(), LONG_AFTER));
    let mut aged = previous.clone();
    aged.get_mut(&rel("a.ts")).unwrap().mtime_ns -= 10_000_000_000;
    let changes = tree.detect(&["a.ts"], &aged, LONG_AFTER);
    assert_eq!(changes.restamped, [rel("a.ts")]);
    assert!(changes.modified.is_empty());
    assert_eq!(changes.stamps[&rel("a.ts")].mtime_ns, previous[&rel("a.ts")].mtime_ns);
    assert!(!changes.needs_reindex());
  }

  #[test]
  fn should_rehash_racily_clean_files_and_catch_same_stat_edits() {
    let tree = Tree::new();
    tree.write("a.ts", "aaaa");
    let mut previous = stamps(&tree.detect(&["a.ts"], &HashMap::new(), LONG_AFTER));
    let stamp = previous.get_mut(&rel("a.ts")).unwrap();
    stamp.hash = ContentHash::of(b"older content with the same size and mtime");
    let scan_ns = stamp.mtime_ns + 1_000_000_000;
    let racy = tree.detect(&["a.ts"], &previous, scan_ns);
    assert_eq!(racy.modified, [rel("a.ts")]);

    let settled = tree.detect(&["a.ts"], &previous, stamp_mtime(&previous) + 2_000_000_000);
    assert_eq!(settled.unchanged, [rel("a.ts")]);
  }

  fn stamp_mtime(previous: &HashMap<RelPath, FileStamp>) -> i64 {
    previous[&rel("a.ts")].mtime_ns
  }

  #[test]
  fn should_keep_a_racily_clean_file_unchanged_when_its_hash_still_matches() {
    let tree = Tree::new();
    tree.write("a.ts", "aaaa");
    let first = tree.detect(&["a.ts"], &HashMap::new(), LONG_AFTER);
    let previous = stamps(&first);
    let scan_ns = previous[&rel("a.ts")].mtime_ns;
    let again = tree.detect(&["a.ts"], &previous, scan_ns);
    assert_eq!(again.unchanged, [rel("a.ts")]);
    assert!(again.restamped.is_empty());
  }

  #[test]
  fn should_report_previous_files_missing_from_discovery_as_deleted() {
    let tree = Tree::new();
    tree.write("a.ts", "one");
    tree.write("b.ts", "two");
    let previous = stamps(&tree.detect(&["a.ts", "b.ts"], &HashMap::new(), LONG_AFTER));
    fs::remove_file(tree.dir.path().join("b.ts")).unwrap();
    let changes = tree.detect(&["a.ts"], &previous, LONG_AFTER);
    assert_eq!(changes.deleted, [rel("b.ts")]);
    assert_eq!(changes.unchanged, [rel("a.ts")]);
    assert!(!changes.stamps.contains_key(&rel("b.ts")));
  }

  #[test]
  fn should_treat_a_file_that_vanishes_between_discovery_and_hashing_as_deleted() {
    let tree = Tree::new();
    tree.write("a.ts", "one");
    tree.write("new.ts", "fresh");
    let previous = stamps(&tree.detect(&["a.ts"], &HashMap::new(), LONG_AFTER));
    let discovered = [tree.file("a.ts"), tree.file("new.ts")];
    fs::remove_file(tree.dir.path().join("a.ts")).unwrap();
    fs::remove_file(tree.dir.path().join("new.ts")).unwrap();
    let mut forced = previous.clone();
    forced.get_mut(&rel("a.ts")).unwrap().size += 1;
    let changes = detect_changes(tree.dir.path(), &discovered, &forced, LONG_AFTER);
    assert_eq!(changes.deleted, [rel("a.ts")]);
    assert!(changes.added.is_empty());
    assert!(changes.errors.is_empty());
    assert!(changes.stamps.is_empty());
  }

  #[test]
  #[cfg(unix)]
  fn should_surface_unreadable_files_as_errors_and_treat_known_ones_as_deleted() {
    use std::os::unix::fs::PermissionsExt;

    let tree = Tree::new();
    tree.write("known.ts", "one");
    tree.write("unknown.ts", "two");
    let previous = stamps(&tree.detect(&["known.ts"], &HashMap::new(), LONG_AFTER));
    tree.write("known.ts", "changed");
    for name in ["known.ts", "unknown.ts"] {
      fs::set_permissions(tree.dir.path().join(name), fs::Permissions::from_mode(0o000)).unwrap();
    }
    if fs::read(tree.dir.path().join("known.ts")).is_ok() {
      return;
    }
    let changes = tree.detect(&["known.ts", "unknown.ts"], &previous, LONG_AFTER);
    assert_eq!(changes.deleted, [rel("known.ts")]);
    assert!(changes.added.is_empty());
    assert_eq!(changes.errors.iter().map(|error| error.path.as_str()).collect::<Vec<_>>(), ["known.ts", "unknown.ts"]);
    assert!(changes.stamps.is_empty());
  }

  #[test]
  fn should_hint_a_rename_for_a_unique_deleted_and_added_pair() {
    let tree = Tree::new();
    tree.write("old.ts", "moved content");
    tree.write("stays.ts", "stays");
    let previous = stamps(&tree.detect(&["old.ts", "stays.ts"], &HashMap::new(), LONG_AFTER));
    fs::rename(tree.dir.path().join("old.ts"), tree.dir.path().join("renamed.ts")).unwrap();
    let changes = tree.detect(&["renamed.ts", "stays.ts"], &previous, LONG_AFTER);
    assert_eq!(changes.renamed_files, [(rel("old.ts"), rel("renamed.ts"))]);
    assert_eq!(changes.deleted, [rel("old.ts")]);
    assert_eq!(changes.added, [rel("renamed.ts")]);
  }

  #[test]
  fn should_not_guess_a_rename_when_identical_content_is_ambiguous() {
    let tree = Tree::new();
    tree.write("a.ts", "same");
    tree.write("b.ts", "same");
    let previous = stamps(&tree.detect(&["a.ts", "b.ts"], &HashMap::new(), LONG_AFTER));
    fs::remove_file(tree.dir.path().join("a.ts")).unwrap();
    fs::remove_file(tree.dir.path().join("b.ts")).unwrap();
    tree.write("c.ts", "same");
    tree.write("d.ts", "same");
    let changes = tree.detect(&["c.ts", "d.ts"], &previous, LONG_AFTER);
    assert!(changes.renamed_files.is_empty());
    assert_eq!(changes.deleted.len(), 2);
    assert_eq!(changes.added.len(), 2);
  }

  #[test]
  fn should_not_hint_a_rename_when_the_content_differs() {
    let tree = Tree::new();
    tree.write("a.ts", "one");
    let previous = stamps(&tree.detect(&["a.ts"], &HashMap::new(), LONG_AFTER));
    fs::remove_file(tree.dir.path().join("a.ts")).unwrap();
    tree.write("b.ts", "two");
    let changes = tree.detect(&["b.ts"], &previous, LONG_AFTER);
    assert!(changes.renamed_files.is_empty());
  }
}
