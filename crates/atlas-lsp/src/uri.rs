use std::io;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum UriError {
  #[error("path {0} is not absolute")]
  NotAbsolute(String),
  #[error("path {0} contains `..`; canonicalise it first")]
  NotNormalized(String),
  #[error("cannot canonicalise {path}: {reason}")]
  Canonicalize { path: String, reason: String },
  #[error("{0} is not a valid URI")]
  Invalid(String),
  #[error("{0} is not a local file URI")]
  NotAFile(String),
}

/// Pass canonical paths and compare server URIs as paths, never as strings: tsserver answers with realpaths and servers percent-encode differently.
pub fn uri_from_path(path: &Path) -> Result<String, UriError> {
  if !path.is_absolute() {
    return Err(UriError::NotAbsolute(path.display().to_string()));
  }
  if path.components().any(|component| component == Component::ParentDir) {
    return Err(UriError::NotNormalized(path.display().to_string()));
  }
  Url::from_file_path(path).map(String::from).map_err(|()| UriError::NotAbsolute(path.display().to_string()))
}

pub fn path_from_uri(uri: &str) -> Result<PathBuf, UriError> {
  let parsed = Url::parse(uri).map_err(|_| UriError::Invalid(uri.to_owned()))?;
  if parsed.scheme() != "file" {
    return Err(UriError::NotAFile(uri.to_owned()));
  }
  let path = parsed.to_file_path().map_err(|()| UriError::NotAFile(uri.to_owned()))?;
  if path.components().any(|component| component == Component::ParentDir) {
    return Err(UriError::NotNormalized(path.display().to_string()));
  }
  if path.as_os_str().as_encoded_bytes().contains(&0) {
    return Err(UriError::Invalid(uri.to_owned()));
  }
  Ok(path)
}

/// Canonicalises the longest existing prefix, so a not-yet-created rename target maps to the directory the server sees.
pub fn canonical_path(path: &Path) -> Result<PathBuf, UriError> {
  if !path.is_absolute() {
    return Err(UriError::NotAbsolute(path.display().to_string()));
  }
  let mut existing = path;
  let mut missing = Vec::new();
  loop {
    match existing.canonicalize() {
      Ok(canonical) => return Ok(missing.iter().rev().fold(canonical, |joined, name| joined.join(name))),
      Err(error) if error.kind() == io::ErrorKind::NotFound => {
        let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
          return Err(canonicalize_error(path, &error));
        };
        missing.push(name.to_owned());
        existing = parent;
      }
      Err(error) => return Err(canonicalize_error(path, &error)),
    }
  }
}

pub fn canonical_uri(path: &Path) -> Result<String, UriError> {
  uri_from_path(&canonical_path(path)?)
}

fn canonicalize_error(path: &Path, error: &io::Error) -> UriError {
  UriError::Canonicalize {
    path: path.display().to_string(),
    reason: error.to_string(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_round_trip_file_uris_with_spaces_and_unicode() {
    let path = Path::new("/tmp/my project/src/ünïcödé 😀/a#b%c?d.rs");
    let uri = uri_from_path(path).unwrap();

    assert_eq!(uri, "file:///tmp/my%20project/src/%C3%BCn%C3%AFc%C3%B6d%C3%A9%20%F0%9F%98%80/a%23b%25c%3Fd.rs");
    assert_eq!(path_from_uri(&uri).unwrap(), path);
  }

  #[test]
  fn should_decode_uris_encoded_differently_by_servers() {
    assert_eq!(path_from_uri("file:///tmp/a%20b/c.ts").unwrap(), Path::new("/tmp/a b/c.ts"));
    assert_eq!(path_from_uri("file:///tmp/%c3%bc.ts").unwrap(), Path::new("/tmp/ü.ts"));
    assert_eq!(path_from_uri("file:///tmp/ü.ts").unwrap(), Path::new("/tmp/ü.ts"));
    assert_eq!(path_from_uri("file://localhost/tmp/a.rs").unwrap(), Path::new("/tmp/a.rs"));
  }

  #[test]
  fn should_reject_relative_and_non_normalized_paths() {
    assert_eq!(uri_from_path(Path::new("src/a.rs")), Err(UriError::NotAbsolute("src/a.rs".to_owned())));
    assert_eq!(uri_from_path(Path::new("/tmp/x/../a.rs")), Err(UriError::NotNormalized("/tmp/x/../a.rs".to_owned())));
  }

  #[test]
  fn should_reject_non_file_and_remote_uris() {
    assert_eq!(path_from_uri("untitled:Untitled-1"), Err(UriError::NotAFile("untitled:Untitled-1".to_owned())));
    assert_eq!(path_from_uri("file://server/share/a.rs"), Err(UriError::NotAFile("file://server/share/a.rs".to_owned())));
    assert_eq!(path_from_uri("not a uri"), Err(UriError::Invalid("not a uri".to_owned())));
  }

  #[cfg(unix)]
  #[test]
  fn should_canonicalise_symlinks_and_keep_missing_leaves() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real dir");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("a.rs"), "").unwrap();
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let real = real.canonicalize().unwrap();

    assert_eq!(canonical_path(&link.join("a.rs")).unwrap(), real.join("a.rs"));
    assert_eq!(canonical_path(&link.join("new/b.rs")).unwrap(), real.join("new/b.rs"));
    assert_eq!(canonical_uri(&link.join("a.rs")).unwrap(), uri_from_path(&real.join("a.rs")).unwrap());
    assert!(canonical_uri(&link.join("a.rs")).unwrap().contains("real%20dir/a.rs"));
  }

  #[test]
  fn should_reject_uris_that_decode_to_parent_components_or_nul() {
    assert_eq!(path_from_uri("file:///tmp/a/%2E%2E%2Fb.rs"), Err(UriError::NotNormalized("/tmp/a/../b.rs".to_owned())));
    assert_eq!(path_from_uri("file:///tmp/a%2F..%2Fb.rs"), Err(UriError::NotNormalized("/tmp/a/../b.rs".to_owned())));
    assert_eq!(path_from_uri("file:///tmp/a%00b.rs"), Err(UriError::Invalid("file:///tmp/a%00b.rs".to_owned())));
  }
}
