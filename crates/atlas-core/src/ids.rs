use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::keys::EntityKey;
use crate::kinds::EdgeKind;

const MAX_NAME_LEN: usize = 128;
const ENTITY_ID_CONTEXT: &str = "code-atlas 2026-10-04 entity-id v1";
const EDGE_ID_CONTEXT: &str = "code-atlas 2026-10-04 edge-id v1";

fn validate_slug(what: &'static str, value: &str) -> Result<(), CoreError> {
  let invalid = |reason| CoreError::InvalidName {
    what,
    value: value.to_owned(),
    reason,
  };
  let Some(first) = value.chars().next() else { return Err(invalid("must not be empty")) };
  if value.len() > MAX_NAME_LEN {
    return Err(invalid("must be at most 128 bytes"));
  }
  if !first.is_ascii_alphanumeric() {
    return Err(invalid("must start with an ASCII letter or digit"));
  }
  if !value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
    return Err(invalid("may only contain ASCII letters, digits, '-', '_' and '.'"));
  }
  Ok(())
}

macro_rules! slug_type {
  ($name:ident, $what:literal) => {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(try_from = "String", into = "String")]
    pub struct $name(String);

    impl $name {
      pub fn new(value: impl Into<String>) -> Result<Self, CoreError> {
        let value = value.into();
        validate_slug($what, &value)?;
        Ok(Self(value))
      }

      pub fn as_str(&self) -> &str {
        &self.0
      }
    }

    impl fmt::Display for $name {
      fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
      }
    }

    impl FromStr for $name {
      type Err = CoreError;

      fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
      }
    }

    impl TryFrom<String> for $name {
      type Error = CoreError;

      fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
      }
    }

    impl From<$name> for String {
      fn from(value: $name) -> Self {
        value.0
      }
    }

    impl AsRef<str> for $name {
      fn as_ref(&self) -> &str {
        &self.0
      }
    }
  };
}

slug_type!(ProjectName, "project name");
slug_type!(RepoName, "repository name");

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RelPath(String);

impl RelPath {
  pub fn new(input: &str) -> Result<Self, CoreError> {
    let invalid = |reason| CoreError::InvalidRelPath { path: input.to_owned(), reason };
    if input.contains('\0') {
      return Err(invalid("contains a NUL byte"));
    }
    let unified = input.replace('\\', "/");
    if unified.starts_with('/') || has_drive_prefix(&unified) {
      return Err(invalid("must be relative to the repository root"));
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in unified.split('/') {
      match segment {
        "" | "." => {}
        ".." => {
          if segments.pop().is_none() {
            return Err(invalid("escapes the repository root"));
          }
        }
        other => segments.push(other),
      }
    }
    if segments.is_empty() {
      return Err(invalid("must name a path below the repository root"));
    }
    Ok(Self(segments.join("/")))
  }

  pub fn as_str(&self) -> &str {
    &self.0
  }

  pub fn segments(&self) -> impl Iterator<Item = &str> {
    self.0.split('/')
  }

  pub fn file_name(&self) -> &str {
    self.0.rsplit('/').next().unwrap_or(&self.0)
  }

  pub fn extension(&self) -> Option<&str> {
    let name = self.file_name();
    let dot = name.rfind('.')?;
    if dot == 0 {
      return None;
    }
    Some(&name[dot + 1..])
  }

  pub fn parent(&self) -> Option<RelPath> {
    let slash = self.0.rfind('/')?;
    Some(Self(self.0[..slash].to_owned()))
  }

  pub fn join(&self, relative: &str) -> Result<RelPath, CoreError> {
    Self::new(&format!("{}/{relative}", self.0))
  }

  pub fn resolve_relative(&self, specifier: &str) -> Result<RelPath, CoreError> {
    match self.parent() {
      Some(dir) => dir.join(specifier),
      None => Self::new(specifier),
    }
  }

  pub fn strip_dir_prefix(&self, dir: &RelPath) -> Option<&str> {
    self.0.strip_prefix(dir.as_str())?.strip_prefix('/')
  }
}

fn has_drive_prefix(path: &str) -> bool {
  let bytes = path.as_bytes();
  bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes.get(2), None | Some(b'/'))
}

impl fmt::Display for RelPath {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.0)
  }
}

impl FromStr for RelPath {
  type Err = CoreError;

  fn from_str(value: &str) -> Result<Self, Self::Err> {
    Self::new(value)
  }
}

impl TryFrom<String> for RelPath {
  type Error = CoreError;

  fn try_from(value: String) -> Result<Self, Self::Error> {
    Self::new(&value)
  }
}

impl From<RelPath> for String {
  fn from(value: RelPath) -> Self {
    value.0
  }
}

impl AsRef<str> for RelPath {
  fn as_ref(&self) -> &str {
    &self.0
  }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContentHash([u8; 32]);

impl ContentHash {
  pub fn of(content: &[u8]) -> Self {
    Self(*blake3::hash(content).as_bytes())
  }

  pub fn from_bytes(bytes: [u8; 32]) -> Self {
    Self(bytes)
  }

  pub fn as_bytes(&self) -> &[u8; 32] {
    &self.0
  }
}

impl fmt::Display for ContentHash {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
  }
}

impl fmt::Debug for ContentHash {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "ContentHash({self})")
  }
}

impl FromStr for ContentHash {
  type Err = CoreError;

  fn from_str(value: &str) -> Result<Self, Self::Err> {
    let invalid = || CoreError::InvalidContentHash(value.to_owned());
    if value.len() != 64 {
      return Err(invalid());
    }
    let mut bytes = [0u8; 32];
    for (byte, pair) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
      *byte = (lower_hex_digit(pair[0]).ok_or_else(invalid)? << 4) | lower_hex_digit(pair[1]).ok_or_else(invalid)?;
    }
    Ok(Self(bytes))
  }
}

fn lower_hex_digit(byte: u8) -> Option<u8> {
  match byte {
    b'0'..=b'9' => Some(byte - b'0'),
    b'a'..=b'f' => Some(byte - b'a' + 10),
    _ => None,
  }
}

impl TryFrom<String> for ContentHash {
  type Error = CoreError;

  fn try_from(value: String) -> Result<Self, Self::Error> {
    value.parse()
  }
}

impl From<ContentHash> for String {
  fn from(value: ContentHash) -> Self {
    value.to_string()
  }
}

fn truncated_hash(hasher: &blake3::Hasher) -> u64 {
  let digest = hasher.finalize();
  let mut head = [0u8; 8];
  head.copy_from_slice(&digest.as_bytes()[..8]);
  u64::from_le_bytes(head)
}

fn write_hex_u64(f: &mut fmt::Formatter<'_>, value: u64) -> fmt::Result {
  write!(f, "{value:016x}")
}

fn parse_hex_u64(value: &str) -> Option<u64> {
  if value.len() != 16 || !value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
    return None;
  }
  u64::from_str_radix(value, 16).ok()
}

macro_rules! hashed_id {
  ($name:ident) => {
    impl $name {
      pub fn from_raw(raw: u64) -> Self {
        Self(raw)
      }

      pub fn raw(self) -> u64 {
        self.0
      }

      pub fn as_i64(self) -> i64 {
        i64::from_le_bytes(self.0.to_le_bytes())
      }

      pub fn from_i64(value: i64) -> Self {
        Self(u64::from_le_bytes(value.to_le_bytes()))
      }
    }

    impl fmt::Display for $name {
      fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex_u64(f, self.0)
      }
    }

    impl fmt::Debug for $name {
      fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({self})", stringify!($name))
      }
    }

    impl FromStr for $name {
      type Err = CoreError;

      fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_hex_u64(value).map(Self).ok_or_else(|| CoreError::InvalidEntityId(value.to_owned()))
      }
    }

    impl TryFrom<String> for $name {
      type Error = CoreError;

      fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
      }
    }

    impl From<$name> for String {
      fn from(value: $name) -> Self {
        value.to_string()
      }
    }
  };
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EntityId(u64);

impl EntityId {
  pub fn of(key: &EntityKey) -> Self {
    let mut hasher = blake3::Hasher::new_derive_key(ENTITY_ID_CONTEXT);
    hasher.update(key.to_string().as_bytes());
    Self(truncated_hash(&hasher))
  }
}

hashed_id!(EntityId);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EdgeId(u64);

impl EdgeId {
  pub fn of(kind: &EdgeKind, from: EntityId, to: EntityId, discriminator: &str) -> Self {
    let mut hasher = blake3::Hasher::new_derive_key(EDGE_ID_CONTEXT);
    let kind = kind.as_str();
    hasher.update(&(kind.len() as u64).to_le_bytes());
    hasher.update(kind.as_bytes());
    hasher.update(&from.0.to_le_bytes());
    hasher.update(&to.0.to_le_bytes());
    hasher.update(&(discriminator.len() as u64).to_le_bytes());
    hasher.update(discriminator.as_bytes());
    Self(truncated_hash(&hasher))
  }
}

hashed_id!(EdgeId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(pub i64);

impl fmt::Display for RunId {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}", self.0)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::keys::{EntityKey, SymbolKey};
  use crate::kinds::EntityKind;
  use crate::lang::LanguageFamily;

  fn repo(name: &str) -> RepoName {
    RepoName::new(name).unwrap()
  }

  #[test]
  fn should_accept_slug_names() {
    for name in ["acme", "acme-web", "Acme_API", "v2.service", "9lives"] {
      assert_eq!(RepoName::new(name).unwrap().as_str(), name);
      assert_eq!(ProjectName::new(name).unwrap().to_string(), name);
    }
  }

  #[test]
  fn should_reject_names_with_separators_or_bad_starts() {
    for name in ["", "-acme", ".hidden", "acme web", "acme:web", "acme/web", "acme#1", "ünicode", &"a".repeat(129)] {
      assert!(RepoName::new(name).is_err(), "{name:?} must be rejected");
    }
  }

  #[test]
  fn should_normalise_relative_paths() {
    let cases = [
      ("src/lib.rs", "src/lib.rs"),
      ("./src/lib.rs", "src/lib.rs"),
      ("src//nested///mod.rs", "src/nested/mod.rs"),
      ("src/./a/../b.ts", "src/b.ts"),
      ("src\\windows\\path.ts", "src/windows/path.ts"),
      (".\\src\\x.rs", "src/x.rs"),
      ("src/dir/", "src/dir"),
      ("src/ünï cödé/файл.ts", "src/ünï cödé/файл.ts"),
      ("a/b/../../c", "c"),
      ("a:b.txt", "a:b.txt"),
    ];
    for (input, expected) in cases {
      assert_eq!(RelPath::new(input).unwrap().as_str(), expected, "input {input:?}");
    }
  }

  #[test]
  fn should_reject_paths_outside_the_repository() {
    let cases = [
      "",
      ".",
      "./",
      "/etc/passwd",
      "\\\\server\\share",
      "C:\\repo\\x.rs",
      "c:/x.rs",
      "D:",
      "../x.rs",
      "a/../../x.rs",
      "nul\0byte",
    ];
    for input in cases {
      assert!(matches!(RelPath::new(input), Err(CoreError::InvalidRelPath { .. })), "{input:?} must be rejected");
    }
  }

  #[test]
  fn should_expose_path_components() {
    let path = RelPath::new("src/users/users.controller.ts").unwrap();
    assert_eq!(path.file_name(), "users.controller.ts");
    assert_eq!(path.extension(), Some("ts"));
    assert_eq!(path.parent().unwrap().as_str(), "src/users");
    assert_eq!(path.segments().collect::<Vec<_>>(), ["src", "users", "users.controller.ts"]);
    assert_eq!(RelPath::new(".gitignore").unwrap().extension(), None);
    assert_eq!(RelPath::new("Makefile").unwrap().extension(), None);
    assert_eq!(RelPath::new("main.rs").unwrap().parent(), None);
  }

  #[test]
  fn should_resolve_relative_specifiers_against_the_file_directory() {
    let file = RelPath::new("src/users/users.service.ts").unwrap();
    assert_eq!(file.resolve_relative("./dto").unwrap().as_str(), "src/users/dto");
    assert_eq!(file.resolve_relative("../auth/guard").unwrap().as_str(), "src/auth/guard");
    assert!(file.resolve_relative("../../../outside").is_err());
    let top = RelPath::new("index.ts").unwrap();
    assert_eq!(top.resolve_relative("./lib").unwrap().as_str(), "lib");
  }

  #[test]
  fn should_strip_directory_prefix_only_at_segment_boundaries() {
    let file = RelPath::new("packages/api/src/main.ts").unwrap();
    assert_eq!(file.strip_dir_prefix(&RelPath::new("packages/api").unwrap()), Some("src/main.ts"));
    assert_eq!(file.strip_dir_prefix(&RelPath::new("packages/ap").unwrap()), None);
    assert_eq!(file.strip_dir_prefix(&RelPath::new("packages/api/src/main.ts").unwrap()), None);
  }

  #[test]
  fn should_serialise_rel_path_as_string_and_validate_on_deserialise() {
    let path = RelPath::new("src/a.rs").unwrap();
    assert_eq!(serde_json::to_string(&path).unwrap(), "\"src/a.rs\"");
    assert_eq!(serde_json::from_str::<RelPath>("\"src/a.rs\"").unwrap(), path);
    assert!(serde_json::from_str::<RelPath>("\"../a.rs\"").is_err());
  }

  #[test]
  fn should_hash_content_with_blake3_and_round_trip_hex() {
    let hash = ContentHash::of(b"fn main() {}");
    let hex = hash.to_string();
    assert_eq!(hex, blake3::hash(b"fn main() {}").to_hex().as_str());
    assert_eq!(hex.parse::<ContentHash>().unwrap(), hash);
    assert_eq!(serde_json::from_str::<ContentHash>(&serde_json::to_string(&hash).unwrap()).unwrap(), hash);
  }

  #[test]
  fn should_reject_non_canonical_content_hashes() {
    let upper = ContentHash::of(b"x").to_string().to_uppercase();
    let multibyte = format!("a{}{}", "é", "b".repeat(61));
    assert_eq!(multibyte.len(), 64);
    for input in ["", "abc", &upper, &"g".repeat(64), &multibyte, &"é".repeat(32)] {
      assert!(input.parse::<ContentHash>().is_err(), "{input:?}");
    }
  }

  #[test]
  fn should_derive_entity_ids_deterministically_across_platforms() {
    let key = EntityKey::Symbol(
      SymbolKey::new(
        LanguageFamily::Rust,
        repo("atlas"),
        "atlas-store",
        vec!["graph".into()],
        vec!["GraphStore".into(), "neighbors".into()],
        EntityKind::Method,
        0,
      )
      .unwrap(),
    );
    let id = EntityId::of(&key);
    assert_eq!(id, EntityId::of(&key.to_string().parse().unwrap()));
    assert_eq!(id.to_string(), "55de6d55b54a7d53");
    assert_eq!(
      EntityId::of(&EntityKey::EnvVar {
        repo: repo("acme-api"),
        name: "DATABASE_URL".into()
      })
      .to_string(),
      "70dae9f4de8f432f"
    );
  }

  #[test]
  fn should_round_trip_entity_ids_through_i64_and_hex() {
    for raw in [0, 1, u64::MAX, 1 << 63, 0x0123_4567_89ab_cdef] {
      let id = EntityId::from_raw(raw);
      assert_eq!(EntityId::from_i64(id.as_i64()), id);
      assert_eq!(id.to_string().parse::<EntityId>().unwrap(), id);
    }
    assert_eq!(EntityId::from_raw(u64::MAX).as_i64(), -1);
    assert!("123".parse::<EntityId>().is_err());
    assert!("FFFFFFFFFFFFFFFF".parse::<EntityId>().is_err());
  }

  #[test]
  fn should_distinguish_edge_ids_by_every_component() {
    let a = EntityId::from_raw(1);
    let b = EntityId::from_raw(2);
    let base = EdgeId::of(&EdgeKind::Calls, a, b, "");
    assert_eq!(base, EdgeId::of(&EdgeKind::Calls, a, b, ""));
    assert_ne!(base, EdgeId::of(&EdgeKind::References, a, b, ""));
    assert_ne!(base, EdgeId::of(&EdgeKind::Calls, b, a, ""));
    assert_ne!(base, EdgeId::of(&EdgeKind::Calls, a, b, "1"));
    assert_eq!(base.to_string(), "10da65b780b8b0b3");
  }
}
