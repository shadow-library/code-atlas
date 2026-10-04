use std::path::{Path, PathBuf};

use atlas_core::{ContentHash, RelPath};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipInputFile {
  pub path: RelPath,
  pub content_hash: ContentHash,
}

pub fn cache_key(indexer: &str, version: &str, files: &[ScipInputFile]) -> String {
  let mut sorted: Vec<&ScipInputFile> = files.iter().collect();
  sorted.sort_by(|left, right| left.path.cmp(&right.path));
  let mut hasher = blake3::Hasher::new();
  for field in [indexer, version] {
    hasher.update(&(field.len() as u64).to_le_bytes());
    hasher.update(field.as_bytes());
  }
  hasher.update(&(sorted.len() as u64).to_le_bytes());
  for file in sorted {
    let path = file.path.as_str();
    hasher.update(&(path.len() as u64).to_le_bytes());
    hasher.update(path.as_bytes());
    hasher.update(file.content_hash.as_bytes());
  }
  hasher.finalize().to_hex().to_string()
}

pub fn cache_path(cache_dir: &Path, key: &str) -> PathBuf {
  cache_dir.join(format!("{key}.scip"))
}

pub const CACHE_ENTRIES_PER_ROOT: usize = 2;

pub(crate) fn cache_scope(indexer: &str, root: &Path) -> String {
  let mut hasher = blake3::Hasher::new();
  hasher.update(&(indexer.len() as u64).to_le_bytes());
  hasher.update(indexer.as_bytes());
  hasher.update(root.as_os_str().as_encoded_bytes());
  hasher.finalize().to_hex()[..16].to_owned()
}

pub(crate) fn entry_stem(scope: &str, key: &str) -> String {
  format!("{scope}-{key}")
}

pub(crate) fn unverified_path(cache_dir: &Path, scope: &str, run: &str) -> PathBuf {
  cache_dir.join(format!("{scope}-unverified-{run}.scip"))
}

fn is_key(text: &str) -> bool {
  text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn remove(path: &Path) {
  if let Err(error) = std::fs::remove_file(path)
    && error.kind() != std::io::ErrorKind::NotFound
  {
    tracing::debug!(path = %path.display(), %error, "could not prune a scip cache entry");
  }
}

fn is_unverified(name: &str, scope: &str) -> bool {
  name.strip_prefix(scope).is_some_and(|rest| rest.starts_with("-unverified")) && name.ends_with(".scip")
}

pub(crate) fn prune_unverified(cache_dir: &Path, scope: &str, older_than: std::time::SystemTime) {
  let Ok(entries) = std::fs::read_dir(cache_dir) else { return };
  for entry in entries.flatten() {
    let name = entry.file_name().to_string_lossy().into_owned();
    let stale = entry.metadata().and_then(|metadata| metadata.modified()).is_ok_and(|modified| modified < older_than);
    if is_unverified(&name, scope) && stale {
      remove(&entry.path());
    }
  }
}

pub(crate) fn prune(cache_dir: &Path, scope: &str, keep: usize, published: &str) {
  let Ok(entries) = std::fs::read_dir(cache_dir) else { return };
  let prefix = format!("{scope}-");
  let mut older: Vec<(std::time::SystemTime, String)> = Vec::new();
  for entry in entries.flatten() {
    let name = entry.file_name().to_string_lossy().into_owned();
    let legacy = [".unverified.scip", ".scip", ".json"].iter().any(|suffix| name.strip_suffix(suffix).is_some_and(is_key));
    if legacy || is_unverified(&name, scope) {
      remove(&entry.path());
      continue;
    }
    let Some(stem) = name.strip_prefix(&prefix).and_then(|rest| rest.strip_suffix(".scip")).filter(|key| is_key(key)) else {
      continue;
    };
    let stem = entry_stem(scope, stem);
    if stem == published {
      continue;
    }
    let modified = entry.metadata().and_then(|metadata| metadata.modified()).unwrap_or(std::time::UNIX_EPOCH);
    older.push((modified, stem));
  }
  older.sort_by(|left, right| right.cmp(left));
  for (_, stem) in older.into_iter().skip(keep.saturating_sub(1)) {
    remove(&cache_path(cache_dir, &stem));
    remove(&sidecar_path(cache_dir, &stem));
  }
}

pub(crate) fn touch(path: &Path) {
  let touched = std::fs::File::options()
    .write(true)
    .open(path)
    .and_then(|file| file.set_modified(std::time::SystemTime::now()));
  if let Err(error) = touched {
    tracing::debug!(path = %path.display(), %error, "could not refresh a scip cache entry");
  }
}

pub(crate) fn sidecar_path(cache_dir: &Path, key: &str) -> PathBuf {
  cache_dir.join(format!("{key}.json"))
}

const SKIPPED_DIRECTORIES: [&str; 3] = ["target", "node_modules", ".git"];
const MAX_VISITED_ENTRIES: usize = 50_000;

pub(crate) fn collect_inputs(root: &Path, wanted: fn(&str) -> bool) -> Vec<ScipInputFile> {
  let mut found = Vec::new();
  let mut pending = vec![root.to_path_buf()];
  let mut visited = 0usize;
  while let Some(directory) = pending.pop() {
    let Ok(entries) = std::fs::read_dir(&directory) else {
      continue;
    };
    for entry in entries.flatten() {
      visited += 1;
      if visited > MAX_VISITED_ENTRIES {
        tracing::warn!(root = %root.display(), "stopped collecting cache inputs at the entry limit");
        return found;
      }
      let path = entry.path();
      let name = entry.file_name().to_string_lossy().into_owned();
      let Ok(file_type) = entry.file_type() else {
        continue;
      };
      if file_type.is_dir() {
        if !SKIPPED_DIRECTORIES.contains(&name.as_str()) && (name == ".cargo" || !name.starts_with('.')) {
          pending.push(path);
        }
        continue;
      }
      let Some(relative) = path.strip_prefix(root).ok().and_then(Path::to_str).and_then(|relative| RelPath::new(relative).ok()) else {
        continue;
      };
      if !wanted(relative.as_str()) {
        continue;
      }
      if let Ok(bytes) = std::fs::read(&path) {
        found.push(ScipInputFile {
          path: relative,
          content_hash: ContentHash::of(&bytes),
        });
      }
    }
  }
  found
}

#[cfg(test)]
mod tests {
  use super::*;

  fn file(path: &str, content: &str) -> ScipInputFile {
    ScipInputFile {
      path: RelPath::new(path).unwrap(),
      content_hash: ContentHash::of(content.as_bytes()),
    }
  }

  #[test]
  fn should_ignore_input_order() {
    let forward = [file("a.rs", "1"), file("b.rs", "2")];
    let backward = [file("b.rs", "2"), file("a.rs", "1")];
    assert_eq!(cache_key("rust-analyzer", "1", &forward), cache_key("rust-analyzer", "1", &backward));
  }

  #[test]
  fn should_change_when_content_path_indexer_or_version_changes() {
    let base = cache_key("rust-analyzer", "1", &[file("a.rs", "1")]);
    assert_ne!(base, cache_key("rust-analyzer", "1", &[file("a.rs", "2")]));
    assert_ne!(base, cache_key("rust-analyzer", "1", &[file("b.rs", "1")]));
    assert_ne!(base, cache_key("scip-typescript", "1", &[file("a.rs", "1")]));
    assert_ne!(base, cache_key("rust-analyzer", "2", &[file("a.rs", "1")]));
  }

  #[test]
  fn should_not_confuse_field_boundaries() {
    assert_ne!(cache_key("ab", "c", &[]), cache_key("a", "bc", &[]));
  }

  #[test]
  fn should_collect_only_wanted_files_and_skip_build_directories() {
    let dir = tempfile::tempdir().unwrap();
    for path in [
      "Cargo.toml",
      "member/Cargo.toml",
      "target/Cargo.toml",
      "node_modules/x/Cargo.toml",
      ".cargo/config.toml",
      "src/lib.rs",
    ] {
      let full = dir.path().join(path);
      std::fs::create_dir_all(full.parent().unwrap()).unwrap();
      std::fs::write(full, path).unwrap();
    }
    let mut found: Vec<String> = collect_inputs(dir.path(), |path| path.ends_with("Cargo.toml") || path == ".cargo/config.toml")
      .into_iter()
      .map(|file| file.path.to_string())
      .collect();
    found.sort();
    assert_eq!(found, vec![".cargo/config.toml", "Cargo.toml", "member/Cargo.toml"]);
  }

  #[test]
  fn should_name_the_cache_file_after_the_key() {
    assert_eq!(cache_path(Path::new("/c"), "k"), Path::new("/c/k.scip"));
  }
}
