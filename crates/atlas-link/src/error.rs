use atlas_core::CoreError;
use atlas_store::StoreError;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LinkError {
  #[error(transparent)]
  Store(#[from] StoreError),
  #[error(transparent)]
  Core(#[from] CoreError),
}

pub type LinkResult<T> = Result<T, LinkError>;
