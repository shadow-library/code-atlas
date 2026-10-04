use std::collections::BTreeSet;

use atlas_core::{EdgeKind, Entity, EntityKey, EntityKind, Span};
use atlas_semantic::{CapabilityInput, SemanticTask, SourceRef, SymbolDigest};
use atlas_store::{EdgeFilter, EntityFilter, ReadTx};

use super::eligibility::is_production_code;
use super::sources::Sources;
use crate::engine::Engine;
use crate::error::EngineResult;

const MIN_SUMMARY_SYMBOLS: usize = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct SummaryCandidate {
  pub file: Entity,
  pub symbols: Vec<SymbolDigest>,
  pub imports: Vec<String>,
}

fn digests(tx: &ReadTx<'_>, file: &Entity) -> EngineResult<Vec<SymbolDigest>> {
  let EntityKey::File { repo, path } = &file.key else { return Ok(Vec::new()) };
  let mut symbols: Vec<Entity> = tx
    .entities_in_file(repo, path, &EntityFilter::default())?
    .into_iter()
    .filter(|entity| entity.kind.is_code_symbol() && entity.signature.is_some() && is_production_code(entity))
    .collect();
  symbols.sort_by_key(|entity| (entity.span.map(|span| span.start_byte), entity.key.to_string()));
  Ok(
    symbols
      .into_iter()
      .filter_map(|entity| {
        Some(SymbolDigest {
          signature: entity.signature?,
          doc: entity.doc,
        })
      })
      .collect(),
  )
}

fn imports(tx: &ReadTx<'_>, file: &Entity) -> EngineResult<Vec<String>> {
  let filter = EdgeFilter {
    kinds: vec![EdgeKind::Imports],
    ..EdgeFilter::default()
  };
  let mut names = BTreeSet::new();
  for edge in tx.edges_from(file.id, &filter)? {
    names.insert(tx.entity(edge.to)?.map_or_else(|| edge.to.to_string(), |target| target.key.display_name()));
  }
  Ok(names.into_iter().collect())
}

pub(super) fn candidate_for(tx: &ReadTx<'_>, file: &Entity) -> EngineResult<Option<SummaryCandidate>> {
  if !matches!(file.key, EntityKey::File { .. }) || !is_production_code(file) {
    return Ok(None);
  }
  let symbols = digests(tx, file)?;
  if symbols.len() < MIN_SUMMARY_SYMBOLS {
    return Ok(None);
  }
  Ok(Some(SummaryCandidate {
    file: file.clone(),
    symbols,
    imports: imports(tx, file)?,
  }))
}

pub(super) fn plan(tx: &ReadTx<'_>) -> EngineResult<Vec<SummaryCandidate>> {
  let mut candidates = Vec::new();
  for file in tx.entities_by_kind(&EntityKind::File, &EntityFilter::default())? {
    candidates.extend(candidate_for(tx, &file)?);
  }
  candidates.sort_by(|a, b| b.symbols.len().cmp(&a.symbols.len()).then_with(|| a.file.key.to_string().cmp(&b.file.key.to_string())));
  Ok(candidates)
}

pub(super) fn task(engine: &Engine, sources: &mut Sources, candidate: &SummaryCandidate) -> Option<SemanticTask> {
  let EntityKey::File { repo, path } = &candidate.file.key else { return None };
  let (text, file_hash) = sources.get(engine, repo, path)?;
  let span = Span::of_text(text, 0, text.len())?;
  let source = SourceRef {
    repo: repo.clone(),
    path: path.clone(),
    span,
    file_hash,
  };
  let input = CapabilityInput::new(
    candidate.file.id,
    format!("{repo}:{path}"),
    vec![source],
    candidate.symbols.clone(),
    candidate.imports.clone(),
  )
  .ok()?;
  Some(SemanticTask::SummarizeCapability(input))
}
