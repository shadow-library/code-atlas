use std::borrow::Cow;
use std::io::{Read, Write};

use atlas_core::{Detection, Diagnostic, FileFacts, PackageContext, ParseStatus, Producer, RelPath, RepoName};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use serde::{Deserialize, Serialize};

use crate::error::{EngineError, EngineResult};

pub(crate) const ARTIFACT_VERSION: u32 = 1;
const JSON_OBJECT_START: u8 = b'{';

pub(crate) type FileKey = (RepoName, RelPath);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DetectorOutput {
  pub detector: String,
  pub producer: Producer,
  pub detection: Detection,
  pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FileArtifact {
  pub facts: FileFacts,
  pub detections: Vec<DetectorOutput>,
  pub detectors: Vec<Producer>,
  pub package: Option<PackageContext>,
  pub generated: bool,
  pub size: u64,
}

impl FileArtifact {
  pub fn blob_name(repo: &RepoName, path: &RelPath) -> String {
    format!("artifact:{repo}:{path}")
  }

  pub fn failure(&self) -> Option<&str> {
    match &self.facts.parse_status {
      ParseStatus::Failed { reason } => Some(reason),
      ParseStatus::Ok | ParseStatus::Partial { .. } => None,
    }
  }

  pub fn encode(&self) -> EngineResult<Vec<u8>> {
    let json = serde_json::to_vec(self).map_err(|error| EngineError::codec("file artifact", error))?;
    let mut encoder = ZlibEncoder::new(Vec::with_capacity(json.len() / 4), Compression::fast());
    encoder.write_all(&json).map_err(|error| EngineError::codec("file artifact", error))?;
    encoder.finish().map_err(|error| EngineError::codec("file artifact", error))
  }

  pub fn decode(bytes: &[u8]) -> EngineResult<Self> {
    serde_json::from_slice(&artifact_json(bytes)?).map_err(|error| EngineError::codec("file artifact", error))
  }
}

pub fn artifact_file_facts(bytes: &[u8]) -> EngineResult<FileFacts> {
  FileArtifact::decode(bytes).map(|artifact| artifact.facts)
}

// Databases written before artifacts were compressed still hold plain JSON blobs; a zlib stream never starts with '{'.
fn artifact_json(bytes: &[u8]) -> EngineResult<Cow<'_, [u8]>> {
  if bytes.first() == Some(&JSON_OBJECT_START) {
    return Ok(Cow::Borrowed(bytes));
  }
  let mut json = Vec::with_capacity(bytes.len() * 6);
  ZlibDecoder::new(bytes).read_to_end(&mut json).map_err(|error| EngineError::codec("file artifact", error))?;
  Ok(Cow::Owned(json))
}

#[cfg(test)]
mod tests {
  use atlas_core::{ContentHash, FileRef, Language};

  use super::*;

  fn artifact() -> FileArtifact {
    let repo = RepoName::new("acme").unwrap();
    let path = RelPath::new("src/lib.rs").unwrap();
    let file = FileRef {
      repo,
      path,
      language: Language::Rust,
      content_hash: ContentHash::of(b"fn main() {}"),
      is_declaration_file: false,
    };
    FileArtifact {
      facts: FileFacts::new(file, Producer::new("atlas-lang-rust", 3)),
      detections: Vec::new(),
      detectors: vec![Producer::new("axum", 1)],
      package: None,
      generated: true,
      size: 12,
    }
  }

  #[test]
  fn should_round_trip_a_compressed_artifact() {
    let original = artifact();

    let encoded = original.encode().unwrap();

    assert_ne!(encoded.first(), Some(&JSON_OBJECT_START));
    assert_eq!(FileArtifact::decode(&encoded).unwrap(), original);
  }

  #[test]
  fn should_decode_an_artifact_stored_as_plain_json() {
    let original = artifact();
    let legacy = serde_json::to_vec(&original).unwrap();

    assert_eq!(FileArtifact::decode(&legacy).unwrap(), original);
    assert_eq!(artifact_file_facts(&legacy).unwrap(), original.facts);
  }

  #[test]
  fn should_reject_bytes_that_are_neither_json_nor_zlib() {
    assert!(matches!(FileArtifact::decode(b"not an artifact"), Err(EngineError::Codec { what: "file artifact", .. })));
  }
}
