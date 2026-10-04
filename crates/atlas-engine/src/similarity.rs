use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use atlas_core::RunId;
use atlas_similarity::{SimilarityConfig, SimilarityDoc, SimilarityIndex};
use atlas_store::DerivedBlob;

use crate::artifact::FileArtifact;
use crate::error::EngineResult;
use crate::owner::SymbolEntry;

pub(crate) const SIMILARITY_BLOB: &str = "similarity";
pub(crate) const SIMILARITY_BLOB_VERSION: u32 = 2;
const GENERATION_BYTES: usize = 8;

pub(crate) fn symbol_docs(artifact: &FileArtifact, symbols: &[SymbolEntry]) -> Vec<SimilarityDoc> {
  let facts = &artifact.facts;
  let mut referenced: BTreeMap<usize, Vec<String>> = BTreeMap::new();
  for reference in &facts.references {
    if let (Some(from), Some(name)) = (reference.from, reference.name()) {
      referenced.entry(from.index()).or_default().push(name.to_owned());
    }
  }
  symbols
    .iter()
    .filter_map(|symbol| {
      let fact = facts.symbols.get(symbol.local)?;
      Some(SimilarityDoc {
        entity: symbol.id,
        kind: symbol.kind.clone(),
        repo: facts.file.repo.clone(),
        name: fact.name.clone(),
        path: facts.file.path.clone(),
        signature: fact.signature.clone(),
        doc: fact.doc.clone(),
        referenced_names: referenced.get(&symbol.local).cloned().unwrap_or_default(),
        body: fact.body.clone(),
      })
    })
    .collect()
}

pub(crate) fn encode_persisted(index: &SimilarityIndex, generation: RunId) -> Vec<u8> {
  let mut bytes = generation.0.to_le_bytes().to_vec();
  bytes.extend(index.to_bytes());
  bytes
}

pub(crate) fn decode_persisted(blob: Option<DerivedBlob>, config: SimilarityConfig, generation: Option<i64>) -> Option<SimilarityIndex> {
  let blob = blob.filter(|blob| blob.version == SIMILARITY_BLOB_VERSION)?;
  let (stamp, payload) = blob.bytes.split_at_checked(GENERATION_BYTES)?;
  let stamped = i64::from_le_bytes(stamp.try_into().ok()?);
  if Some(stamped) != generation {
    tracing::info!(persisted = stamped, current = ?generation, "persisted similarity index is older than the index; rebuilding it");
    return None;
  }
  match SimilarityIndex::from_bytes(payload) {
    Ok(index) if index.config() == config => Some(index),
    Ok(_) => None,
    Err(error) => {
      tracing::warn!(error = %error, "persisted similarity index is unreadable; rebuilding it");
      None
    }
  }
}

pub(crate) fn replace_symbols(index: &mut SimilarityIndex, removed: impl IntoIterator<Item = atlas_core::EntityId>, docs: Vec<SimilarityDoc>) -> EngineResult<()> {
  for entity in removed {
    index.remove(entity);
  }
  for doc in docs {
    index.upsert(doc)?;
  }
  Ok(())
}

pub(crate) fn read_lock(lock: &RwLock<Arc<SimilarityIndex>>) -> RwLockReadGuard<'_, Arc<SimilarityIndex>> {
  lock.read().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn write_lock(lock: &RwLock<Arc<SimilarityIndex>>) -> RwLockWriteGuard<'_, Arc<SimilarityIndex>> {
  lock.write().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn writable(shared: &mut Arc<SimilarityIndex>) -> EngineResult<&mut SimilarityIndex> {
  if Arc::get_mut(shared).is_none() {
    tracing::debug!("a similarity snapshot is still in use; copying the index before updating it");
    *shared = Arc::new(SimilarityIndex::from_bytes(&shared.to_bytes())?);
  }
  Arc::get_mut(shared).ok_or_else(|| crate::error::EngineError::codec("similarity index", "snapshot is shared after copying"))
}
