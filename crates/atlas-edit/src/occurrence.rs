use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use atlas_core::ContentHash;
use serde::{Deserialize, Serialize};

use crate::error::{EditError, EditResult};
use crate::plan::FileTarget;

const PREFIX: &str = "o-";
const HEX_LEN: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OccurrenceId(String);

impl OccurrenceId {
  pub fn derive(file: &FileTarget, content_hash: &ContentHash, start: usize, end: usize) -> Self {
    let mut hasher = blake3::Hasher::new();
    hasher.update(file.repo.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(file.path.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(content_hash.as_bytes());
    hasher.update(&(start as u64).to_le_bytes());
    hasher.update(&(end as u64).to_le_bytes());
    let hex = hasher.finalize().to_hex();
    Self(format!("{PREFIX}{}", &hex.as_str()[..HEX_LEN]))
  }

  pub fn as_str(&self) -> &str {
    &self.0
  }
}

impl fmt::Display for OccurrenceId {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.0)
  }
}

impl FromStr for OccurrenceId {
  type Err = EditError;

  fn from_str(value: &str) -> Result<Self, Self::Err> {
    let valid = value
      .strip_prefix(PREFIX)
      .is_some_and(|hex| hex.len() == HEX_LEN && hex.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')));
    if valid {
      Ok(Self(value.to_owned()))
    } else {
      Err(EditError::InvalidOccurrenceId(value.to_owned()))
    }
  }
}

impl TryFrom<String> for OccurrenceId {
  type Error = EditError;

  fn try_from(value: String) -> Result<Self, Self::Error> {
    value.parse()
  }
}

impl From<OccurrenceId> for String {
  fn from(value: OccurrenceId) -> Self {
    value.0
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
  pub id: OccurrenceId,
  pub file: FileTarget,
  pub start: usize,
  pub end: usize,
  pub line: usize,
  pub column: usize,
}

pub(crate) fn ensure_unique(occurrences: &[Occurrence]) -> EditResult<()> {
  let mut seen = HashSet::with_capacity(occurrences.len());
  match occurrences.iter().find(|occurrence| !seen.insert(&occurrence.id)) {
    Some(duplicate) => Err(EditError::OccurrenceCollision { id: duplicate.id.clone() }),
    None => Ok(()),
  }
}
