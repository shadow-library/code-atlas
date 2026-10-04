use atlas_core::{CoreError, RelPath, RepoName};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Error)]
#[non_exhaustive]
pub enum ResolveError {
  #[error(transparent)]
  Core(#[from] CoreError),
  #[error("{repo}:{path} is not in the symbol table; upsert it before resolving")]
  NotIndexed { repo: RepoName, path: RelPath },
  #[error("{repo}:{path} changed since it was upserted; upsert the current facts before resolving")]
  StaleFile { repo: RepoName, path: RelPath },
  #[error("the symbol table has pending module-tree changes; call refresh() before resolving")]
  StaleTable,
}
