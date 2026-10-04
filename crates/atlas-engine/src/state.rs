use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Instant;

use atlas_core::{ContentHash, EntityId, EntityKind, KeyContext, Owner, PackageContext, RelPath, RepoName, RunId, SymbolKey};
use atlas_project::{FileStamp, PackageGraph, Project, RepoConfig, discover};
use atlas_resolve::ProjectSymbolTable;
use atlas_similarity::SimilarityIndex;
use atlas_store::{EntityFilter, FreshnessPolicy, InferredQuery, ReadTx, Store, WriteTx};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::analyze::Toolchain;
use crate::artifact::{ARTIFACT_VERSION, FileArtifact, FileKey};
use crate::error::{EngineError, EngineResult};
use crate::options::EngineOptions;
use crate::owner::{OwnerDraft, SymbolEntry, build_owner};
use crate::packages::{clone_graph, default_context, graph_fingerprint, package_context};
use crate::similarity::{SIMILARITY_BLOB, decode_persisted, replace_symbols, symbol_docs};

pub(crate) const META_BLOB: &str = "engine:meta";
pub(crate) const META_BLOB_VERSION: u32 = 2;
pub(crate) const DIRTY_BLOB: &str = "engine:dirty";
pub(crate) const SCIP_BLOB: &str = "engine:scip";
pub(crate) const SCIP_BLOB_VERSION: u32 = 1;
pub(crate) const REPOS_BLOB: &str = "engine:repos";
pub(crate) const REPOS_BLOB_VERSION: u32 = 1;
pub(crate) const PROJECT_BLOB: &str = "engine:project";
pub(crate) const PROJECT_BLOB_VERSION: u32 = 1;
pub(crate) const FACTS_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RepoMeta {
  pub graph_fingerprint: String,
  pub scan_started_ns: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ScipMeta {
  pub last_run_ms: Option<i64>,
  pub lagging: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EngineMeta {
  pub facts_version: u32,
  pub resolver_version: u32,
  pub artifact_version: u32,
  pub generation: Option<i64>,
  pub repos: BTreeMap<String, RepoMeta>,
}

impl ScipMeta {
  pub fn read(tx: &ReadTx<'_>) -> EngineResult<Self> {
    let Some(blob) = tx.blob(SCIP_BLOB)?.filter(|blob| blob.version == SCIP_BLOB_VERSION) else {
      return Ok(Self::default());
    };
    serde_json::from_slice(&blob.bytes).map_err(|error| EngineError::codec("scip meta", error))
  }

  pub fn write(&self, tx: &mut WriteTx<'_>) -> EngineResult<()> {
    let bytes = serde_json::to_vec(self).map_err(|error| EngineError::codec("scip meta", error))?;
    tx.put_blob(SCIP_BLOB, SCIP_BLOB_VERSION, &bytes)?;
    Ok(())
  }
}

pub(crate) fn project_fingerprint(project: &Project) -> String {
  let mut repos: Vec<&RepoConfig> = project.repos.iter().collect();
  repos.sort_by(|left, right| left.name.cmp(&right.name));
  let identity = serde_json::json!({
    "name": project.name.as_str(),
    "repos": repos,
    "max_file_bytes": project.settings.max_file_bytes,
  });
  blake3::hash(identity.to_string().as_bytes()).to_hex().to_string()
}

pub(crate) fn read_project_fingerprint(tx: &ReadTx<'_>) -> EngineResult<Option<String>> {
  let Some(blob) = tx.blob(PROJECT_BLOB)?.filter(|blob| blob.version == PROJECT_BLOB_VERSION) else {
    return Ok(None);
  };
  String::from_utf8(blob.bytes).map(Some).map_err(|error| EngineError::codec("project fingerprint", error))
}

pub(crate) fn write_project_fingerprint(tx: &mut WriteTx<'_>, fingerprint: &str) -> EngineResult<()> {
  tx.put_blob(PROJECT_BLOB, PROJECT_BLOB_VERSION, fingerprint.as_bytes())?;
  Ok(())
}

pub(crate) fn read_known_repos(tx: &ReadTx<'_>) -> EngineResult<BTreeSet<RepoName>> {
  let Some(blob) = tx.blob(REPOS_BLOB)?.filter(|blob| blob.version == REPOS_BLOB_VERSION) else {
    return Ok(BTreeSet::new());
  };
  let names: Vec<String> = serde_json::from_slice(&blob.bytes).map_err(|error| EngineError::codec("known repos", error))?;
  Ok(names.into_iter().filter_map(|name| RepoName::new(name).ok()).collect())
}

pub(crate) fn write_known_repos(tx: &mut WriteTx<'_>, repos: &BTreeSet<RepoName>) -> EngineResult<()> {
  let names: Vec<&str> = repos.iter().map(RepoName::as_str).collect();
  let bytes = serde_json::to_vec(&names).map_err(|error| EngineError::codec("known repos", error))?;
  tx.put_blob(REPOS_BLOB, REPOS_BLOB_VERSION, &bytes)?;
  Ok(())
}

fn indexed_repos(tx: &ReadTx<'_>) -> EngineResult<BTreeSet<RepoName>> {
  let mut repos: BTreeSet<RepoName> = tx
    .entities_by_kind(&EntityKind::Repository, &EntityFilter::default())?
    .into_iter()
    .filter_map(|entity| entity.repo)
    .collect();
  repos.extend(read_known_repos(tx)?);
  if let Some(blob) = tx.blob(META_BLOB)?
    && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&blob.bytes)
    && let Some(known) = value.get("repos").and_then(serde_json::Value::as_object)
  {
    repos.extend(known.keys().filter_map(|name| RepoName::new(name).ok()));
  }
  Ok(repos)
}

impl EngineMeta {
  pub fn read(tx: &ReadTx<'_>) -> EngineResult<Option<Self>> {
    let Some(blob) = tx.blob(META_BLOB)? else { return Ok(None) };
    if blob.version != META_BLOB_VERSION {
      return Ok(None);
    }
    serde_json::from_slice(&blob.bytes).map(Some).map_err(|error| EngineError::codec("engine meta", error))
  }

  pub fn is_compatible(&self) -> bool {
    self.facts_version == FACTS_VERSION && self.resolver_version == atlas_resolve::RESOLVER_VERSION && self.artifact_version == ARTIFACT_VERSION
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RepoHealth {
  Present,
  Missing { reason: String },
}

pub(crate) struct RepoState {
  pub config: RepoConfig,
  pub stamps: HashMap<RelPath, FileStamp>,
  pub graph: Option<PackageGraph>,
  pub scan_started_ns: Option<i64>,
  pub health: RepoHealth,
}

#[derive(Debug, Clone)]
pub(crate) struct FileState {
  pub hash: ContentHash,
  pub upsert_context: KeyContext,
  pub package: Option<PackageContext>,
  pub symbols: Vec<(EntityId, SymbolKey, EntityKind)>,
  pub wanted: BTreeSet<EntityId>,
  pub failure: Option<String>,
  pub dangling: u64,
}

impl FileState {
  pub fn symbol_ids(&self) -> impl Iterator<Item = EntityId> + '_ {
    self.symbols.iter().map(|(id, _, _)| *id)
  }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Pending {
  pub force_full_resolve: bool,
  pub reanalyze: BTreeSet<FileKey>,
  pub purge_repos: BTreeSet<RepoName>,
}

impl Pending {
  pub fn is_empty(&self) -> bool {
    !self.force_full_resolve && self.reanalyze.is_empty() && self.purge_repos.is_empty()
  }
}

pub(crate) struct EngineState {
  pub repos: BTreeMap<RepoName, RepoState>,
  pub files: BTreeMap<FileKey, FileState>,
  pub table: ProjectSymbolTable,
  pub wanted_by: HashMap<EntityId, BTreeSet<FileKey>>,
  pub generation: Option<RunId>,
  pub pending: Pending,
  pub stale_inferred: u64,
  pub scip: ScipMeta,
  pub similarity_persisted: Option<RunId>,
}

impl EngineState {
  pub fn set_file(&mut self, key: FileKey, state: FileState) {
    self.unlink_wanted(&key);
    for id in &state.wanted {
      self.wanted_by.entry(*id).or_default().insert(key.clone());
    }
    self.files.insert(key, state);
  }

  pub fn remove_file(&mut self, key: &FileKey) -> Option<FileState> {
    self.unlink_wanted(key);
    self.files.remove(key)
  }

  fn unlink_wanted(&mut self, key: &FileKey) {
    let Some(previous) = self.files.get(key) else { return };
    for id in &previous.wanted {
      if let Some(owners) = self.wanted_by.get_mut(id) {
        owners.remove(key);
        if owners.is_empty() {
          self.wanted_by.remove(id);
        }
      }
    }
  }

  pub fn graph(&self, repo: &RepoName) -> Option<&PackageGraph> {
    self.repos.get(repo).and_then(|state| state.graph.as_ref())
  }
}

pub(crate) fn symbols_of(entries: &[SymbolEntry]) -> Vec<(EntityId, SymbolKey, EntityKind)> {
  entries.iter().map(|entry| (entry.id, entry.key.clone(), entry.kind.clone())).collect()
}

pub(crate) fn count_inferred_stale(tx: &ReadTx<'_>) -> EngineResult<u64> {
  let facts = tx.inferred_facts(&InferredQuery {
    subject: None,
    relation: None,
    freshness: FreshnessPolicy::IncludeStale,
  })?;
  Ok(facts.iter().filter(|fact| !fact.freshness.is_current()).count() as u64)
}

struct LoadedFile {
  key: FileKey,
  artifact: Option<FileArtifact>,
}

pub(crate) struct Loaded {
  pub state: EngineState,
  pub similarity: SimilarityIndex,
  pub interrupted: bool,
}

pub(crate) fn load_state(store: &Store, project: &Project, toolchain: &Toolchain, options: &EngineOptions) -> EngineResult<Loaded> {
  let started = Instant::now();
  let (meta, dirty, records, blobs, stale_inferred, similarity_blob, indexed, scip) = store.read(|tx| -> EngineResult<_> {
    let meta = EngineMeta::read(tx)?;
    let indexed = indexed_repos(tx)?;
    let scip = ScipMeta::read(tx)?;
    let dirty = tx.blob(DIRTY_BLOB)?.is_some();
    let mut records = BTreeMap::new();
    let mut blobs = Vec::new();
    for repo in &project.repos {
      let files = tx.files_in_repo(&repo.name)?;
      for record in &files {
        if record.language.is_none() {
          continue;
        }
        let blob = tx.blob(&FileArtifact::blob_name(&record.repo, &record.path))?;
        blobs.push(((record.repo.clone(), record.path.clone()), blob));
      }
      records.insert(repo.name.clone(), files);
    }
    Ok((meta, dirty, records, blobs, count_inferred_stale(tx)?, tx.blob(SIMILARITY_BLOB)?, indexed, scip))
  })?;
  let meta = meta.filter(EngineMeta::is_compatible);
  let loaded: Vec<LoadedFile> = blobs
    .into_par_iter()
    .map(|(key, blob)| LoadedFile {
      artifact: blob.filter(|blob| blob.version == ARTIFACT_VERSION).and_then(|blob| FileArtifact::decode(&blob.bytes).ok()),
      key,
    })
    .collect();
  let mut pending = Pending {
    force_full_resolve: meta.is_none(),
    ..Pending::default()
  };
  let mut table = ProjectSymbolTable::new();
  let mut repos = BTreeMap::new();
  for config in &project.repos {
    let stamps: HashMap<RelPath, FileStamp> = records
      .get(&config.name)
      .into_iter()
      .flatten()
      .map(|record| {
        let stamp = FileStamp {
          size: record.size,
          mtime_ns: record.mtime_ns,
          inode: record.inode.unwrap_or(0),
          hash: record.content_hash,
        };
        (record.path.clone(), stamp)
      })
      .collect();
    let repo_meta = meta.as_ref().and_then(|meta| meta.repos.get(config.name.as_str()));
    let (graph, health) = match discover(config, project.settings.max_file_bytes) {
      Ok(discovered) => (Some(PackageGraph::load(&config.path, &discovered.files)), RepoHealth::Present),
      Err(error) => (None, RepoHealth::Missing { reason: error.to_string() }),
    };
    let fingerprint = graph.as_ref().map(graph_fingerprint);
    if fingerprint.as_deref() != repo_meta.map(|meta| meta.graph_fingerprint.as_str()) && !stamps.is_empty() {
      pending.force_full_resolve = true;
    }
    if let Some(graph) = &graph {
      table.set_repo_packages(config.name.clone(), clone_graph(graph));
    }
    repos.insert(
      config.name.clone(),
      RepoState {
        config: config.clone(),
        stamps,
        graph,
        scan_started_ns: repo_meta.and_then(|meta| meta.scan_started_ns),
        health,
      },
    );
  }
  let configured: BTreeSet<&RepoName> = project.repos.iter().map(|repo| &repo.name).collect();
  pending.purge_repos = indexed.into_iter().filter(|repo| !configured.contains(repo)).collect();
  let mut artifacts: BTreeMap<FileKey, FileArtifact> = BTreeMap::new();
  let mut contexts: BTreeMap<FileKey, (KeyContext, Option<PackageContext>)> = BTreeMap::new();
  let empty = PackageGraph::default();
  for file in loaded {
    let (repo, path) = &file.key;
    let graph = repos.get(repo).and_then(|state: &RepoState| state.graph.as_ref()).unwrap_or(&empty);
    let package = package_context(graph, path);
    let Some(artifact) = file.artifact.filter(|artifact| toolchain.is_current(artifact, package.as_ref())) else {
      pending.reanalyze.insert(file.key);
      continue;
    };
    let context = default_context(graph, path);
    if let Err(error) = table.upsert_file(&artifact.facts, &context) {
      tracing::warn!(repo = %repo, path = %path, error = %error, "stored facts do not fit the symbol table; reanalyzing the file");
      pending.reanalyze.insert(file.key);
      continue;
    }
    contexts.insert(file.key.clone(), (context, package));
    artifacts.insert(file.key, artifact);
  }
  table.refresh();
  let drafts: Vec<(FileKey, EngineResult<OwnerDraft>)> = artifacts
    .par_iter()
    .map(|(key, artifact)| {
      (
        key.clone(),
        build_owner(artifact, &table, repos.get(&key.0).and_then(|state: &RepoState| state.graph.as_ref())),
      )
    })
    .collect();
  let mut state = EngineState {
    repos,
    files: BTreeMap::new(),
    table,
    wanted_by: HashMap::new(),
    generation: meta.as_ref().and_then(|meta| meta.generation).map(RunId),
    pending,
    stale_inferred,
    scip,
    similarity_persisted: None,
  };
  let persisted_similarity = match dirty {
    true => None,
    false => decode_persisted(similarity_blob, options.similarity, meta.as_ref().and_then(|meta| meta.generation)),
  };
  let rebuild_similarity = persisted_similarity.is_none();
  if !rebuild_similarity {
    state.similarity_persisted = state.generation;
  }
  let mut similarity = match persisted_similarity {
    Some(index) => index,
    None => SimilarityIndex::new(options.similarity)?,
  };
  let mut resolved = Vec::with_capacity(drafts.len());
  for (key, draft) in drafts {
    match draft {
      Ok(draft) => resolved.push(draft),
      Err(error) => {
        tracing::warn!(repo = %key.0, path = %key.1, error = %error, "stored facts could not be resolved; reanalyzing the file and re-resolving every file on the next run");
        state.pending.force_full_resolve = true;
        state.pending.reanalyze.insert(key);
      }
    }
  }
  let drafts = resolved;
  let asserted: HashSet<EntityId> = drafts.iter().flat_map(|draft| draft.entities.keys().copied()).collect();
  let required: BTreeSet<EntityId> = drafts.iter().flat_map(OwnerDraft::wanted).filter(|id| !asserted.contains(id)).collect();
  let existing: HashSet<EntityId> = store.read(|tx| -> EngineResult<HashSet<EntityId>> {
    let mut existing = HashSet::new();
    for id in required {
      if tx.entity_assertions(id)?.iter().any(|assertion| matches!(assertion.owner, Owner::File { .. })) {
        existing.insert(id);
      }
    }
    Ok(existing)
  })?;
  for draft in drafts {
    let key = draft.key.clone();
    let (Some(artifact), Some((context, package))) = (artifacts.get(&key), contexts.remove(&key)) else {
      continue;
    };
    if rebuild_similarity {
      replace_symbols(&mut similarity, [], symbol_docs(artifact, &draft.symbols))?;
    }
    let file_state = FileState {
      hash: artifact.facts.file.content_hash,
      upsert_context: context,
      package,
      symbols: symbols_of(&draft.symbols),
      wanted: draft.wanted(),
      failure: artifact.failure().map(str::to_owned),
      dangling: draft.dangling_count(|id| asserted.contains(&id) || existing.contains(&id)),
    };
    state.set_file(key, file_state);
  }
  tracing::info!(
    project = %project.name,
    files = state.files.len(),
    reanalyze = state.pending.reanalyze.len(),
    force_full_resolve = state.pending.force_full_resolve,
    similarity_rebuilt = rebuild_similarity,
    elapsed_ms = started.elapsed().as_millis() as u64,
    "loaded engine state"
  );
  Ok(Loaded {
    state,
    similarity,
    interrupted: dirty,
  })
}
