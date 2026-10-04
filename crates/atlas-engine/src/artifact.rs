use atlas_core::{Detection, Diagnostic, FileFacts, PackageContext, ParseStatus, Producer, RelPath, RepoName};
use serde::{Deserialize, Serialize};

use crate::error::{EngineError, EngineResult};

pub(crate) const ARTIFACT_VERSION: u32 = 1;

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
    serde_json::to_vec(self).map_err(|error| EngineError::codec("file artifact", error))
  }

  pub fn decode(bytes: &[u8]) -> EngineResult<Self> {
    serde_json::from_slice(bytes).map_err(|error| EngineError::codec("file artifact", error))
  }
}
