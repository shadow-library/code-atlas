use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IdeError {
  #[error("path {path} is outside every registered repo")]
  OutsideProject { path: String },
}

pub type IdeResult<T> = Result<T, IdeError>;
