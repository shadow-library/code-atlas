use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

const UNTRACKED_DIRS: [&str; 3] = [".git", "target", "node_modules"];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
  Dir,
  File { len: u64, hash: blake3::Hash },
  Link { target: PathBuf },
  Unreadable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkedSnapshot {
  entries: BTreeMap<String, Entry>,
}

impl LinkedSnapshot {
  pub fn take(roots: &[(&str, &Path)]) -> Self {
    let mut snapshot = Self::default();
    for (label, root) in roots {
      if fs::symlink_metadata(root).is_ok() {
        snapshot.walk(label, root);
      }
    }
    snapshot
  }

  pub fn changes(&self, after: &Self) -> Vec<String> {
    let changed: BTreeSet<&str> = self
      .entries
      .keys()
      .chain(after.entries.keys())
      .filter(|key| self.entries.get(*key) != after.entries.get(*key))
      .map(String::as_str)
      .collect();
    changed
      .iter()
      .filter(|key| !ancestors(key).any(|ancestor| changed.contains(ancestor)))
      .map(|key| (*key).to_owned())
      .collect()
  }

  fn walk(&mut self, label: &str, root: &Path) {
    let mut pending = vec![(root.to_owned(), label.to_owned())];
    while let Some((dir, key)) = pending.pop() {
      let Ok(entries) = fs::read_dir(&dir) else {
        self.entries.insert(key, Entry::Unreadable);
        continue;
      };
      self.entries.insert(key.clone(), Entry::Dir);
      for entry in entries {
        let Ok(entry) = entry else {
          self.entries.insert(key.clone(), Entry::Unreadable);
          continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = format!("{key}/{name}");
        let path = entry.path();
        match entry.file_type() {
          Ok(kind) if kind.is_dir() && UNTRACKED_DIRS.contains(&name.as_str()) => {}
          Ok(kind) if kind.is_dir() => pending.push((path, child)),
          Ok(kind) if kind.is_symlink() => {
            let entry = fs::read_link(&path).map_or(Entry::Unreadable, |target| Entry::Link { target });
            self.entries.insert(child, entry);
          }
          Ok(_) => {
            self.entries.insert(child, file_entry(&path));
          }
          Err(_) => {
            self.entries.insert(child, Entry::Unreadable);
          }
        }
      }
    }
  }
}

fn file_entry(path: &Path) -> Entry {
  let Ok(mut file) = File::open(path) else {
    return Entry::Unreadable;
  };
  let mut hasher = blake3::Hasher::new();
  match hasher.update_reader(&mut file) {
    Ok(_) => Entry::File {
      len: hasher.count(),
      hash: hasher.finalize(),
    },
    Err(_) => Entry::Unreadable,
  }
}

fn ancestors(key: &str) -> impl Iterator<Item = &str> {
  key.char_indices().filter(|(_, c)| *c == '/').map(move |(index, _)| &key[..index])
}

#[cfg(test)]
mod tests {
  use super::*;

  fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
  }

  fn snapshot(root: &Path) -> LinkedSnapshot {
    LinkedSnapshot::take(&[("linked-hold", &root.join("hold")), ("linked-view", &root.join("view"))])
  }

  #[test]
  fn should_report_every_added_removed_or_rewritten_entry_once_at_its_highest_changed_path() {
    let root = tempfile::tempdir().unwrap();
    let hold = root.path().join("hold");
    let view = root.path().join("view");
    write(&hold.join("apps/acme-web/src/client.ts"), "export const base = '/api';\n");
    write(&hold.join("apps/acme-web/src/orders.ts"), "export const orders = 1;\n");
    write(&hold.join("apps/acme-web/node_modules/acme-ui/index.js"), "x\n");
    write(&view.join("apps/acme-web/src/client.ts"), "export const base = '/api';\n");
    write(&view.join("package.json"), "{}\n");
    let before = snapshot(root.path());
    assert_eq!(before.changes(&snapshot(root.path())), Vec::<String>::new());

    write(&hold.join("apps/acme-web/src/client.ts"), "export const base = '/api/v2';\n");
    fs::remove_dir_all(hold.join("apps/acme-web/src")).unwrap();
    write(&view.join("apps/acme-web/src/client.ts"), "export const base = '/apX';\n");
    write(&view.join("apps/acme-web/src/new/a.ts"), "x\n");
    write(&view.join("apps/acme-web/node_modules/acme-ui/index.js"), "y\n");
    write(&view.join("target/debug/out"), "z\n");

    assert_eq!(
      before.changes(&snapshot(root.path())),
      vec![
        "linked-hold/apps/acme-web/src",
        "linked-view/apps/acme-web/src/client.ts",
        "linked-view/apps/acme-web/src/new"
      ]
    );
  }

  #[test]
  fn should_report_a_root_that_vanished_and_ignore_one_that_never_existed() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("hold/apps/acme-web/a.ts"), "a\n");
    let before = snapshot(root.path());
    fs::rename(root.path().join("hold"), root.path().join("moved")).unwrap();
    assert_eq!(before.changes(&snapshot(root.path())), vec!["linked-hold"]);
  }
}
