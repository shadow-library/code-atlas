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
