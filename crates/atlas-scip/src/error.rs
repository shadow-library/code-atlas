use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ScipError {
  #[error("scip index is not valid protobuf: {0}")]
  Decode(#[from] protobuf::Error),
  #[error("scip cache io failed at {path}: {source}")]
  CacheIo { path: String, source: io::Error },
}
