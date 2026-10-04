use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use atlas_core::{ContentHash, EntityId, KeyContext, Owner, RelPath, RepoName, RunId, Span, UNPACKAGED};
use atlas_project::{FileStamp, PackageGraph, Project};
use atlas_resolve::SurfaceDelta;
use atlas_scip::ScipFacts;
use atlas_similarity::SimilarityIndex;
use atlas_store::{DerivedBlob, FactDelta, FileRecord, FreshnessPolicy, InferredQuery, NewInferredFact, OwnedFacts, RunOutcome, RunReport, Store, WriteTx};
use rayon::prelude::*;
use serde_json::{Value, json};

use crate::analyze::{AnalysisJob, AnalysisOutcome, Toolchain};
use crate::artifact::{ARTIFACT_VERSION, FileArtifact, FileKey};
use crate::error::{EngineError, EngineResult};
use crate::lineage::{LineageSide, MIN_MIGRATION_CONFIDENCE, RenamePair, lineage_edge, match_renames, retained_lineage};
use crate::manifests::manifest_owner_facts;
use crate::options::{EngineOptions, IndexMode};
use crate::owner::{FinalOwner, OwnerDraft, build_owner};
use crate::packages::{clone_graph, default_context, graph_fingerprint, package_context, same_graph};
use crate::passes::{LinkPass, PASS_LINEAGE, PASS_REPOS, PassContext, PassError, PassOutput, ProjectPass, StructurePass, repos_pass};
use crate::report::{FailedFile, RunOutcomeKind, RunSummary};
use crate::scan::{RepoScan, ScanOutcome, ScannedRepo, is_code_file, is_manifest};
use crate::similarity::{replace_symbols, symbol_docs};
use crate::state::{
  DIRTY_BLOB, EngineMeta, EngineState, FACTS_VERSION, FileState, META_BLOB, META_BLOB_VERSION, RepoHealth, RepoMeta, ScipMeta, count_inferred_stale, read_known_repos, symbols_of,
  write_known_repos,
};

const MAX_RESOLVE_ROUNDS: usize = 4;
const MAX_CHANGE_SET_ENTRIES: usize = 1_000;

pub(crate) struct RunEnv<'a> {
  pub store: &'a Store,
  pub project: &'a Project,
  pub toolchain: &'a Toolchain,
  pub options: &'a EngineOptions,
}

#[derive(Default)]
struct Ledger {
  phases: BTreeMap<String, u64>,
  counts: BTreeMap<String, u64>,
  failed: Vec<FailedFile>,
  partial: Vec<String>,
}

impl Ledger {
  fn time<T>(&mut self, phase: &str, f: impl FnOnce(&mut Self) -> T) -> T {
    let span = tracing::info_span!("index_phase", phase);
    let _entered = span.enter();
    let started = Instant::now();
    let value = f(self);
    let elapsed = started.elapsed();
    *self.phases.entry(phase.to_owned()).or_default() += elapsed.as_millis() as u64;
    tracing::debug!(phase, elapsed_ms = elapsed.as_millis() as u64, "phase finished");
    value
  }

  fn add(&mut self, key: &str, amount: u64) {
    *self.counts.entry(key.to_owned()).or_default() += amount;
  }

  fn record_delta(&mut self, delta: &FactDelta) {
    self.add("entities_added", delta.entities.added);
    self.add("entities_removed", delta.entities.removed);
    self.add("entities_changed", delta.entities.changed);
    self.add("edges_added", delta.edges.added);
    self.add("edges_removed", delta.edges.removed);
    self.add("edges_changed", delta.edges.changed);
    self.add("unresolved_added", delta.unresolved.added);
    self.add("unresolved_removed", delta.unresolved.removed);
  }

  fn outcome(&self) -> RunOutcomeKind {
    if self.partial.is_empty() && self.failed.is_empty() {
      return RunOutcomeKind::Ok;
    }
    let mut reasons = self.partial.clone();
    if !self.failed.is_empty() {
      reasons.push(format!("{} file(s) or repo(s) failed", self.failed.len()));
    }
    RunOutcomeKind::Partial { reason: reasons.join("; ") }
  }
}

struct RepoPlan {
  started_ns: i64,
  stamps: BTreeMap<RelPath, FileStamp>,
  added: Vec<RelPath>,
  modified: Vec<RelPath>,
  deleted: Vec<RelPath>,
  restamped: Vec<RelPath>,
}

enum Write {
  RemoveFile {
    key: FileKey,
  },
  File {
    key: FileKey,
    facts: OwnedFacts,
    update: Option<Box<FileUpdate>>,
  },
  Owner {
    owner: Owner,
    facts: OwnedFacts,
  },
  Record {
    record: FileRecord,
  },
  Restamp {
    key: FileKey,
    stamp: FileStamp,
  },
  DeleteRecord {
    key: FileKey,
  },
}

struct FileUpdate {
  artifact: Vec<u8>,
  record: FileRecord,
  content_changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presence {
  Present,
  Absent,
}

#[derive(Default)]
struct Events {
  seen: HashMap<EntityId, (Presence, Presence)>,
}

impl Events {
  fn absorb(&mut self, delta: &FactDelta) {
    let changes = delta
      .created_entities
      .iter()
      .map(|id| (*id, Presence::Present))
      .chain(delta.collected_entities.iter().map(|id| (*id, Presence::Absent)));
    for (id, now) in changes {
      self.seen.entry(id).and_modify(|(_, last)| *last = now).or_insert((now, now));
    }
  }

  fn len(&self) -> usize {
    self.seen.len()
  }

  fn created(&self) -> BTreeSet<EntityId> {
    self.matching(Presence::Present)
  }

  fn collected(&self) -> BTreeSet<EntityId> {
    self.matching(Presence::Absent)
  }

  fn matching(&self, outcome: Presence) -> BTreeSet<EntityId> {
    self
      .seen
      .iter()
      .filter(|(_, (first, last))| *first == outcome && *last == outcome)
      .map(|(id, _)| *id)
      .collect()
  }
}

struct Processed {
  key: FileKey,
  symbols: Vec<crate::owner::SymbolEntry>,
  wanted: BTreeSet<EntityId>,
  dangling: u64,
}

#[derive(Default)]
struct LineageInputs {
  disappeared: Vec<LineageSide>,
  appeared: Vec<LineageSide>,
}

enum MissingArtifact {
  Fail,
  Skip,
}

fn file_owner(key: &FileKey) -> Owner {
  Owner::File {
    repo: key.0.clone(),
    path: key.1.clone(),
  }
}

pub(crate) fn key_label(key: &FileKey) -> String {
  format!("{}:{}", key.0, key.1)
}

fn capped(keys: impl IntoIterator<Item = String>) -> Value {
  let mut list: Vec<String> = keys.into_iter().collect();
  let total = list.len();
  list.truncate(MAX_CHANGE_SET_ENTRIES);
  json!({ "total": total, "entries": list })
}

pub(crate) fn execute(
  env: &RunEnv<'_>,
  state: &mut EngineState,
  similarity: &RwLock<Arc<SimilarityIndex>>,
  scans: Vec<RepoScan>,
  mode: IndexMode,
  trigger: &str,
) -> EngineResult<RunSummary> {
  let started = Instant::now();
  let run = env.store.write(|tx| -> EngineResult<RunId> {
    let run = tx.begin_run(trigger)?;
    tx.put_blob(DIRTY_BLOB, 1, &run.0.to_le_bytes())?;
    let mut known = read_known_repos(tx)?;
    known.extend(env.project.repos.iter().map(|repo| repo.name.clone()));
    write_known_repos(tx, &known)?;
    Ok(run)
  })?;
  let span = tracing::info_span!("index_run", run = run.0, trigger, mode = ?mode);
  let _entered = span.enter();
  let mut ledger = Ledger::default();
  match Pipeline::new(env, state, similarity, run, mode).execute(scans, &mut ledger) {
    Ok(files_updated) => {
      let outcome = ledger.outcome();
      tracing::info!(run = run.0, files_updated, elapsed_ms = started.elapsed().as_millis() as u64, outcome = ?outcome, "index run finished");
      Ok(RunSummary {
        run,
        outcome,
        files_updated,
        failed_files: ledger.failed,
        phase_timings_ms: ledger.phases,
        counts: ledger.counts,
        duration: started.elapsed(),
      })
    }
    Err(error) => {
      tracing::error!(run = run.0, error = %error, "index run failed");
      let report = RunReport {
        phase_timings_ms: ledger.phases,
        counts: ledger.counts,
        change_set: Value::Null,
      };
      let recorded = env.store.write(|tx| -> EngineResult<()> {
        tx.finish_run(run, RunOutcome::Failed { error: error.to_string() }, &report)?;
        Ok(())
      });
      if let Err(finish_error) = recorded {
        tracing::error!(run = run.0, error = %finish_error, "could not record the failed run");
      }
      Err(error)
    }
  }
}

struct Pipeline<'a, 'e> {
  env: &'a RunEnv<'e>,
  state: &'a mut EngineState,
  similarity: &'a RwLock<Arc<SimilarityIndex>>,
  run: RunId,
  mode: IndexMode,
  plans: BTreeMap<RepoName, RepoPlan>,
  deleted: BTreeSet<FileKey>,
  graph_changed: BTreeSet<RepoName>,
  moved: BTreeSet<(FileKey, FileKey)>,
  purged: BTreeMap<RepoName, Vec<FileRecord>>,
  events: Events,
  converged: Convergence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Convergence {
  Converged,
  Exhausted,
}

impl<'a, 'e> Pipeline<'a, 'e> {
  fn new(env: &'a RunEnv<'e>, state: &'a mut EngineState, similarity: &'a RwLock<Arc<SimilarityIndex>>, run: RunId, mode: IndexMode) -> Self {
    Self {
      env,
      state,
      similarity,
      run,
      mode,
      plans: BTreeMap::new(),
      deleted: BTreeSet::new(),
      graph_changed: BTreeSet::new(),
      moved: BTreeSet::new(),
      purged: BTreeMap::new(),
      events: Events::default(),
      converged: Convergence::Converged,
    }
  }

  fn graph(&self, repo: &RepoName) -> Option<&PackageGraph> {
    self.state.graph(repo)
  }

  fn execute(mut self, scans: Vec<RepoScan>, ledger: &mut Ledger) -> EngineResult<u64> {
    let to_analyze = ledger.time("plan", |ledger| self.plan(scans, ledger))?;
    let analyzed = ledger.time("analyze", |ledger| self.analyze(to_analyze, ledger));
    let analyzed_keys: BTreeSet<FileKey> = analyzed.keys().cloned().collect();
    let (contexts, recontexted, resolve) = ledger.time("symbols", |_| self.update_table(&analyzed))?;
    ledger.add("files_analyzed", analyzed_keys.len() as u64);
    ledger.add("files_deleted", self.deleted.len() as u64);
    ledger.add("owners_resolved", resolve.len() as u64);
    let mut artifacts: BTreeMap<FileKey, Arc<FileArtifact>> = analyzed.into_iter().map(|(key, artifact)| (key, Arc::new(artifact))).collect();
    artifacts.extend(recontexted);
    ledger.time("load", |ledger| self.load_artifacts(&resolve, &mut artifacts, ledger))?;
    let drafts = ledger.time("resolve", |_| self.drafts(&resolve, &artifacts))?;
    let fixed = ledger.time("manifests", |_| self.fixed_owners());
    let finalized = ledger.time("existence", |ledger| self.finalize(drafts, &fixed, &resolve, ledger))?;
    let lineage_inputs = ledger.time("lineage_inputs", |_| self.lineage_inputs(&finalized, &artifacts, &analyzed_keys))?;
    let mut writes: Vec<Write> = self.deleted.iter().map(|key| Write::RemoveFile { key: key.clone() }).collect();
    let (file_writes, mut processed) = ledger.time("encode", |ledger| self.file_writes(finalized, &artifacts, &analyzed_keys, ledger))?;
    writes.extend(file_writes);
    writes.extend(fixed.into_iter().map(|(owner, facts)| Write::Owner { owner, facts }));
    writes.extend(self.record_writes());
    ledger.time("persist", |ledger| self.persist(writes, ledger))?;
    self.env.options.fault_injection.crash_point();
    let follow_up = ledger.time("dependents", |ledger| self.follow_up(&resolve, &processed, ledger))?;
    processed.extend(follow_up);
    ledger.time("similarity", |_| self.update_similarity(&processed, &artifacts, &analyzed_keys))?;
    self.update_file_states(&processed, &artifacts, &contexts);
    let renames = ledger.time("lineage", |_| self.lineage(lineage_inputs))?;
    ledger.add("renames_detected", renames as u64);
    ledger.time("passes", |ledger| self.project_passes(ledger))?;
    let files_updated = (self.deleted.len() + analyzed_keys.len()) as u64;
    ledger.time("finish", |ledger| self.finish(ledger, &analyzed_keys))?;
    if self.converged == Convergence::Exhausted {
      self.state.pending.force_full_resolve = true;
    }
    Ok(files_updated)
  }

  fn plan(&mut self, scans: Vec<RepoScan>, ledger: &mut Ledger) -> EngineResult<BTreeMap<FileKey, ContentHash>> {
    let mut to_analyze: BTreeMap<FileKey, ContentHash> = BTreeMap::new();
    for scan in scans {
      let repo = scan.repo.clone();
      let Some(repo_state) = self.state.repos.get_mut(&repo) else { continue };
      match scan.outcome {
        ScanOutcome::Missing { reason } => {
          ledger.failed.push(FailedFile {
            repo: repo.clone(),
            path: None,
            reason: format!("repository is unavailable: {reason}"),
          });
          let known: Vec<RelPath> = repo_state.stamps.keys().cloned().collect();
          self.deleted.extend(known.iter().filter(|path| is_code_file(path)).map(|path| (repo.clone(), path.clone())));
          if repo_state.graph.take().is_some() {
            self.state.table.remove_repo_packages(&repo);
            self.graph_changed.insert(repo.clone());
          }
          repo_state.health = RepoHealth::Missing { reason };
          self.plans.insert(
            repo,
            RepoPlan {
              started_ns: scan.started_ns,
              stamps: BTreeMap::new(),
              added: Vec::new(),
              modified: Vec::new(),
              deleted: known,
              restamped: Vec::new(),
            },
          );
        }
        ScanOutcome::Scanned(scanned) => {
          let ScannedRepo { files, changes, walk_errors } = *scanned;
          repo_state.health = RepoHealth::Present;
          ledger.failed.extend(walk_errors.into_iter().map(|error| FailedFile {
            repo: repo.clone(),
            path: None,
            reason: format!("walk error: {error}"),
          }));
          ledger.failed.extend(changes.errors.iter().map(|error| FailedFile {
            repo: repo.clone(),
            path: Some(error.path.clone()),
            reason: format!("unreadable: {}", error.message),
          }));
          self
            .deleted
            .extend(changes.deleted.iter().filter(|path| is_code_file(path)).map(|path| (repo.clone(), path.clone())));
          let full = self.mode == IndexMode::Full;
          let candidates = changes
            .added
            .iter()
            .chain(&changes.modified)
            .chain(full.then_some(&changes.unchanged).into_iter().flatten())
            .chain(full.then_some(&changes.restamped).into_iter().flatten());
          for path in candidates.filter(|path| is_code_file(path)) {
            if let Some(stamp) = changes.stamps.get(path) {
              to_analyze.insert((repo.clone(), path.clone()), stamp.hash);
            }
          }
          let manifest_touched = changes.added.iter().chain(&changes.modified).chain(&changes.deleted).any(is_manifest);
          let file_set_changed = !changes.added.is_empty() || !changes.deleted.is_empty();
          if repo_state.graph.is_none() || manifest_touched || file_set_changed || full {
            let graph = PackageGraph::load(&repo_state.config.path, &files);
            if repo_state.graph.as_ref().is_none_or(|current| !same_graph(current, &graph)) {
              self.state.table.set_repo_packages(repo.clone(), clone_graph(&graph));
              self.graph_changed.insert(repo.clone());
              repo_state.graph = Some(graph);
            }
          }
          for (from, to) in &changes.renamed_files {
            self.moved.insert(((repo.clone(), from.clone()), (repo.clone(), to.clone())));
          }
          self.plans.insert(
            repo,
            RepoPlan {
              started_ns: scan.started_ns,
              stamps: changes.stamps,
              added: changes.added,
              modified: changes.modified,
              deleted: changes.deleted,
              restamped: changes.restamped,
            },
          );
        }
      }
    }
    for repo in self.state.pending.purge_repos.clone() {
      let records = self.env.store.read(|tx| tx.files_in_repo(&repo))?;
      self
        .deleted
        .extend(records.iter().filter(|record| is_code_file(&record.path)).map(|record| (repo.clone(), record.path.clone())));
      self.purged.insert(repo, records);
    }
    for key in self.state.pending.reanalyze.clone() {
      let stamp = self.plans.get(&key.0).and_then(|plan| plan.stamps.get(&key.1));
      if let Some(stamp) = stamp
        && !self.deleted.contains(&key)
      {
        to_analyze.insert(key, stamp.hash);
      }
    }
    let empty = PackageGraph::default();
    let repackaged: Vec<(FileKey, ContentHash)> = self
      .state
      .files
      .iter()
      .filter(|(key, _)| self.graph_changed.contains(&key.0) && !self.deleted.contains(*key) && !to_analyze.contains_key(*key))
      .filter(|(key, file)| package_context(self.state.graph(&key.0).unwrap_or(&empty), &key.1) != file.package)
      .filter_map(|(key, _)| Some((key.clone(), self.plans.get(&key.0)?.stamps.get(&key.1)?.hash)))
      .collect();
    ledger.add("files_repackaged", repackaged.len() as u64);
    to_analyze.extend(repackaged);
    for plan in self.plans.values() {
      ledger.add("files_added", plan.added.len() as u64);
      ledger.add("files_modified", plan.modified.len() as u64);
      ledger.add("files_restamped", plan.restamped.len() as u64);
    }
    ledger.add("graphs_changed", self.graph_changed.len() as u64);
    Ok(to_analyze)
  }

  fn analyze(&mut self, to_analyze: BTreeMap<FileKey, ContentHash>, ledger: &mut Ledger) -> BTreeMap<FileKey, FileArtifact> {
    let empty = PackageGraph::default();
    let jobs: Vec<AnalysisJob> = to_analyze
      .into_iter()
      .filter_map(|(key, hash)| {
        let root = &self.state.repos.get(&key.0)?.config.path;
        Some(AnalysisJob {
          absolute: root.join(key.1.as_str()),
          expected_hash: hash,
          package: package_context(self.graph(&key.0).unwrap_or(&empty), &key.1),
          key,
        })
      })
      .collect();
    let toolchain = self.env.toolchain;
    let faults = &self.env.options.fault_injection;
    let outcomes: Vec<(FileKey, AnalysisOutcome)> = jobs.par_iter().map(|job| (job.key.clone(), toolchain.analyze(job, faults))).collect();
    let mut analyzed = BTreeMap::new();
    for (key, outcome) in outcomes {
      match outcome {
        AnalysisOutcome::Vanished => {
          if let Some(plan) = self.plans.get_mut(&key.0) {
            plan.stamps.remove(&key.1);
          }
          self.deleted.insert(key);
        }
        AnalysisOutcome::Analyzed(artifact) => {
          if let Some(reason) = artifact.failure() {
            ledger.failed.push(FailedFile {
              repo: key.0.clone(),
              path: Some(key.1.clone()),
              reason: reason.to_owned(),
            });
          }
          if let Some(stamp) = self.plans.get_mut(&key.0).and_then(|plan| plan.stamps.get_mut(&key.1)) {
            stamp.hash = artifact.facts.file.content_hash;
          }
          analyzed.insert(key, *artifact);
        }
      }
    }
    analyzed
  }

  #[allow(clippy::type_complexity)]
  fn update_table(&mut self, analyzed: &BTreeMap<FileKey, FileArtifact>) -> EngineResult<(BTreeMap<FileKey, KeyContext>, BTreeMap<FileKey, Arc<FileArtifact>>, BTreeSet<FileKey>)> {
    let empty = PackageGraph::default();
    let mut deltas: Vec<SurfaceDelta> = Vec::new();
    for (repo, path) in &self.deleted {
      deltas.extend(self.state.table.remove_file(repo, path));
    }
    let mut contexts = BTreeMap::new();
    for (key, artifact) in analyzed {
      let context = default_context(self.graph(&key.0).unwrap_or(&empty), &key.1);
      deltas.push(self.state.table.upsert_file(&artifact.facts, &context)?);
      contexts.insert(key.clone(), context);
    }
    let shifted: BTreeMap<FileKey, KeyContext> = self
      .state
      .files
      .iter()
      .filter(|(key, _)| self.graph_changed.contains(&key.0) && !analyzed.contains_key(*key) && !self.deleted.contains(*key))
      .filter_map(|(key, file)| {
        let context = default_context(self.state.graph(&key.0).unwrap_or(&empty), &key.1);
        (context != file.upsert_context).then(|| (key.clone(), context))
      })
      .collect();
    let mut loaded = self.read_artifacts(&shifted.keys().cloned().collect(), MissingArtifact::Fail)?;
    let mut recontexted = BTreeMap::new();
    for (key, context) in shifted {
      let Some(artifact) = loaded.remove(&key) else { continue };
      deltas.push(self.state.table.upsert_file(&artifact.facts, &context)?);
      contexts.insert(key.clone(), context);
      recontexted.insert(key, Arc::new(artifact));
    }
    let relocations = self.state.table.refresh();
    let full = self.mode == IndexMode::Full || self.state.pending.force_full_resolve || !self.graph_changed.is_empty();
    let mut resolve: BTreeSet<FileKey> = match full {
      true => self.state.files.keys().cloned().chain(analyzed.keys().cloned()).collect(),
      false => {
        let mut set: BTreeSet<FileKey> = analyzed.keys().cloned().chain(recontexted.keys().cloned()).collect();
        set.extend(relocations.iter().map(|relocation| (relocation.repo.clone(), relocation.path.clone())));
        set.extend(self.state.table.affected_files(&deltas, &relocations));
        set
      }
    };
    resolve.retain(|key| !self.deleted.contains(key) && self.state.table.contains_file(&key.0, &key.1));
    tracing::debug!(
      deltas = deltas.len(),
      relocations = relocations.len(),
      resolve = resolve.len(),
      full,
      "computed the re-resolve set"
    );
    Ok((contexts, recontexted, resolve))
  }

  fn read_artifacts(&self, keys: &BTreeSet<FileKey>, missing: MissingArtifact) -> EngineResult<BTreeMap<FileKey, FileArtifact>> {
    if keys.is_empty() {
      return Ok(BTreeMap::new());
    }
    let blobs = self
      .env
      .store
      .read(|tx| -> EngineResult<Vec<(FileKey, Option<DerivedBlob>)>> { keys.iter().map(|key| Ok((key.clone(), tx.blob(&FileArtifact::blob_name(&key.0, &key.1))?))).collect() })?;
    let decoded: Vec<EngineResult<Option<(FileKey, FileArtifact)>>> = blobs
      .into_par_iter()
      .map(|(key, blob)| match (blob.filter(|blob| blob.version == ARTIFACT_VERSION), &missing) {
        (Some(blob), _) => Ok(Some((key, FileArtifact::decode(&blob.bytes)?))),
        (None, MissingArtifact::Skip) => Ok(None),
        (None, MissingArtifact::Fail) => Err(EngineError::codec("file artifact", format!("{} has no current artifact", key_label(&key)))),
      })
      .collect();
    let mut artifacts = BTreeMap::new();
    for item in decoded {
      if let Some((key, artifact)) = item? {
        artifacts.insert(key, artifact);
      }
    }
    Ok(artifacts)
  }

  fn load_artifacts(&self, keys: &BTreeSet<FileKey>, artifacts: &mut BTreeMap<FileKey, Arc<FileArtifact>>, ledger: &mut Ledger) -> EngineResult<()> {
    let missing: BTreeSet<FileKey> = keys.iter().filter(|key| !artifacts.contains_key(*key)).cloned().collect();
    let loaded = self.read_artifacts(&missing, MissingArtifact::Fail)?;
    ledger.add("artifacts_loaded", loaded.len() as u64);
    artifacts.extend(loaded.into_iter().map(|(key, artifact)| (key, Arc::new(artifact))));
    Ok(())
  }

  fn drafts(&self, keys: &BTreeSet<FileKey>, artifacts: &BTreeMap<FileKey, Arc<FileArtifact>>) -> EngineResult<Vec<OwnerDraft>> {
    let table = &self.state.table;
    keys
      .par_iter()
      .map(|key| {
        let artifact = artifacts.get(key).ok_or_else(|| EngineError::NotIndexed {
          repo: key.0.clone(),
          path: key.1.clone(),
        })?;
        build_owner(artifact, table, self.graph(&key.0))
      })
      .collect()
  }

  fn fixed_owners(&self) -> Vec<(Owner, OwnedFacts)> {
    let mut owners = Vec::new();
    for (repo, plan) in &self.plans {
      let mut facts = match self.graph(repo) {
        Some(graph) => manifest_owner_facts(&self.state.table, repo, graph, |path| plan.stamps.get(path).map(|stamp| stamp.hash)),
        None => BTreeMap::new(),
      };
      let known = plan.stamps.keys().chain(&plan.deleted).filter(|path| is_manifest(path));
      for path in known {
        facts.entry(path.clone()).or_default();
      }
      owners.extend(facts.into_iter().map(|(path, facts)| (Owner::File { repo: repo.clone(), path }, facts)));
    }
    for (repo, records) in &self.purged {
      for record in records.iter().filter(|record| is_manifest(&record.path)) {
        let owner = Owner::File {
          repo: repo.clone(),
          path: record.path.clone(),
        };
        owners.push((owner, OwnedFacts::default()));
      }
    }
    owners.push((Owner::Project { pass: PASS_REPOS.to_owned() }, repos_pass(self.env.project)));
    owners
  }

  fn finalize(&self, drafts: Vec<OwnerDraft>, fixed: &[(Owner, OwnedFacts)], resolve: &BTreeSet<FileKey>, ledger: &mut Ledger) -> EngineResult<Vec<FinalOwner>> {
    let mut asserted: HashSet<EntityId> = drafts.iter().flat_map(|draft| draft.entities.keys().copied()).collect();
    asserted.extend(fixed.iter().flat_map(|(_, facts)| facts.entities.iter().map(|entity| entity.id)));
    let mut replaced: BTreeSet<Owner> = resolve.iter().chain(&self.deleted).map(file_owner).collect();
    replaced.extend(fixed.iter().map(|(owner, _)| owner.clone()));
    let required: BTreeSet<EntityId> = drafts.iter().flat_map(OwnerDraft::wanted).filter(|id| !asserted.contains(id)).collect();
    let existing = self.existing_elsewhere(&required, &replaced)?;
    ledger.add("detector_targets_checked", required.len() as u64);
    let finalized: Vec<FinalOwner> = drafts
      .into_iter()
      .map(|draft| draft.finalize(|id| asserted.contains(&id) || existing.contains(&id)))
      .collect();
    for owner in &finalized {
      ledger.add("dangling_detector_edges_dropped", owner.counts.dangling_detector_edges);
      ledger.add("unresolvable_detector_edges_dropped", owner.counts.unresolvable_detector_edges);
    }
    Ok(finalized)
  }

  fn existing_elsewhere(&self, required: &BTreeSet<EntityId>, replaced: &BTreeSet<Owner>) -> EngineResult<HashSet<EntityId>> {
    if required.is_empty() {
      return Ok(HashSet::new());
    }
    self.env.store.read(|tx| -> EngineResult<HashSet<EntityId>> {
      let mut existing = HashSet::new();
      for id in required {
        let assertions = tx.entity_assertions(*id)?;
        if assertions
          .iter()
          .any(|assertion| matches!(assertion.owner, Owner::File { .. }) && !replaced.contains(&assertion.owner))
        {
          existing.insert(*id);
        }
      }
      Ok(existing)
    })
  }

  fn lineage_inputs(&self, finalized: &[FinalOwner], artifacts: &BTreeMap<FileKey, Arc<FileArtifact>>, analyzed: &BTreeSet<FileKey>) -> EngineResult<LineageInputs> {
    let mut appeared = Vec::new();
    let mut gone: BTreeMap<FileKey, Vec<(EntityId, atlas_core::SymbolKey, atlas_core::EntityKind)>> = BTreeMap::new();
    for owner in finalized {
      let before = self.state.files.get(&owner.key);
      let old_ids: BTreeSet<EntityId> = before.map(|state| state.symbol_ids().collect()).unwrap_or_default();
      let new_ids: BTreeSet<EntityId> = owner.symbols.iter().map(|symbol| symbol.id).collect();
      let Some(artifact) = artifacts.get(&owner.key) else { continue };
      for symbol in owner.symbols.iter().filter(|symbol| !old_ids.contains(&symbol.id)) {
        let Some(fact) = artifact.facts.symbols.get(symbol.local) else { continue };
        appeared.push(LineageSide {
          id: symbol.id,
          key: symbol.key.clone(),
          kind: symbol.kind.clone(),
          file: owner.key.clone(),
          body: fact.body.clone(),
          span: fact.span,
          content_hash: artifact.facts.file.content_hash,
        });
      }
      let lost: Vec<_> = before
        .into_iter()
        .flat_map(|state| &state.symbols)
        .filter(|(id, _, _)| !new_ids.contains(id))
        .cloned()
        .collect();
      if !lost.is_empty() {
        gone.insert(owner.key.clone(), lost);
      }
    }
    for key in &self.deleted {
      if let Some(before) = self.state.files.get(key).filter(|state| !state.symbols.is_empty()) {
        gone.insert(key.clone(), before.symbols.clone());
      }
    }
    if appeared.is_empty() || gone.is_empty() {
      return Ok(LineageInputs::default());
    }
    let rewritten: BTreeSet<FileKey> = gone.keys().filter(|key| analyzed.contains(*key) || self.deleted.contains(*key)).cloned().collect();
    let old_artifacts = self.read_artifacts(&rewritten, MissingArtifact::Skip)?;
    let mut disappeared = Vec::new();
    for (key, lost) in gone {
      let facts = match (old_artifacts.get(&key), rewritten.contains(&key)) {
        (Some(artifact), _) => &artifact.facts,
        (None, true) => continue,
        (None, false) => match artifacts.get(&key) {
          Some(artifact) => &artifact.facts,
          None => continue,
        },
      };
      for (id, symbol_key, kind) in lost {
        let fact = facts
          .symbols
          .iter()
          .find(|fact| fact.item_path == symbol_key.item_path() && fact.kind == kind && symbol_key.module_path().ends_with(&fact.module_suffix));
        disappeared.push(LineageSide {
          id,
          key: symbol_key,
          kind,
          file: key.clone(),
          body: fact.and_then(|fact| fact.body.clone()),
          span: fact.map_or_else(|| Span::from_zero_based_rows(0, 0, 0, 0, 0, 0), |fact| fact.span),
          content_hash: facts.file.content_hash,
        });
      }
    }
    Ok(LineageInputs { disappeared, appeared })
  }

  fn file_writes(
    &self,
    finalized: Vec<FinalOwner>,
    artifacts: &BTreeMap<FileKey, Arc<FileArtifact>>,
    analyzed: &BTreeSet<FileKey>,
    ledger: &mut Ledger,
  ) -> EngineResult<(Vec<Write>, Vec<Processed>)> {
    let encode_started = Instant::now();
    let encoded: Vec<(FileKey, EngineResult<Vec<u8>>)> = analyzed
      .par_iter()
      .filter_map(|key| artifacts.get(key).map(|artifact| (key.clone(), artifact.encode())))
      .collect();
    let mut encoded_by_key = BTreeMap::new();
    for (key, bytes) in encoded {
      let bytes = bytes?;
      ledger.add("artifact_bytes", bytes.len() as u64);
      encoded_by_key.insert(key, bytes);
    }
    if !analyzed.is_empty() {
      ledger.add("artifact_encode_us", encode_started.elapsed().as_micros() as u64);
    }
    let mut writes = Vec::new();
    let mut processed = Vec::new();
    for owner in finalized {
      let update = match (encoded_by_key.remove(&owner.key), artifacts.get(&owner.key)) {
        (Some(bytes), Some(artifact)) => Some(Box::new(self.file_update(&owner.key, artifact, bytes))),
        _ => None,
      };
      processed.push(Processed {
        key: owner.key.clone(),
        symbols: owner.symbols,
        wanted: owner.wanted,
        dangling: owner.counts.dangling_detector_edges,
      });
      writes.push(Write::File {
        key: owner.key,
        facts: owner.facts,
        update,
      });
    }
    Ok((writes, processed))
  }

  fn file_update(&self, key: &FileKey, artifact: &FileArtifact, bytes: Vec<u8>) -> FileUpdate {
    let stamp = self.plans.get(&key.0).and_then(|plan| plan.stamps.get(&key.1));
    FileUpdate {
      artifact: bytes,
      record: FileRecord {
        repo: key.0.clone(),
        path: key.1.clone(),
        language: Some(artifact.facts.file.language),
        content_hash: artifact.facts.file.content_hash,
        size: stamp.map_or(artifact.size, |stamp| stamp.size),
        mtime_ns: stamp.map_or(0, |stamp| stamp.mtime_ns),
        inode: stamp.map(|stamp| stamp.inode),
        parse_status: artifact.facts.parse_status.clone(),
        analyzer: Some(artifact.facts.analyzer.clone()),
        indexed_run: Some(self.run),
      },
      content_changed: self.state.files.get(key).map(|state| state.hash) != Some(artifact.facts.file.content_hash),
    }
  }

  fn record_writes(&self) -> Vec<Write> {
    let mut writes = Vec::new();
    for (repo, plan) in &self.plans {
      for path in plan.added.iter().chain(&plan.modified).filter(|path| is_manifest(path)) {
        if let Some(stamp) = plan.stamps.get(path) {
          writes.push(Write::Record {
            record: manifest_record(repo, path, stamp, self.run),
          });
        }
      }
      for path in &plan.restamped {
        if let Some(stamp) = plan.stamps.get(path) {
          writes.push(Write::Restamp {
            key: (repo.clone(), path.clone()),
            stamp: stamp.clone(),
          });
        }
      }
      for path in plan.deleted.iter().filter(|path| is_manifest(path)) {
        writes.push(Write::DeleteRecord {
          key: (repo.clone(), path.clone()),
        });
      }
    }
    for (repo, records) in &self.purged {
      for record in records.iter().filter(|record| is_manifest(&record.path)) {
        writes.push(Write::DeleteRecord {
          key: (repo.clone(), record.path.clone()),
        });
      }
    }
    writes
  }

  fn persist(&mut self, writes: Vec<Write>, ledger: &mut Ledger) -> EngineResult<()> {
    let max_owners = self.env.options.batch_max_owners.max(1);
    let max_duration = self.env.options.batch_max_duration.max(Duration::from_millis(1));
    let mut pending = writes.into_iter().peekable();
    let run = self.run;
    while pending.peek().is_some() {
      let (deltas, scip) = self.env.store.write(|tx| -> EngineResult<(Vec<FactDelta>, ScipMeta)> {
        tx.resume_run(run)?;
        let started = Instant::now();
        let mut deltas = Vec::new();
        let mut dropped: BTreeSet<FileKey> = BTreeSet::new();
        let mut written = 0usize;
        while written < max_owners && started.elapsed() < max_duration {
          let Some(write) = pending.next() else { break };
          written += 1;
          apply_write(tx, write, &mut deltas, &mut dropped)?;
        }
        let mut scip = ScipMeta::read(tx)?;
        if !dropped.is_empty() {
          for key in &dropped {
            let label = key_label(key);
            match tx.file(&key.0, &key.1)?.is_some() {
              true => {
                scip.lagging.insert(label.clone());
                scip.dropped_in.insert(label, run.0);
              }
              false => {
                scip.lagging.remove(&label);
                scip.dropped_in.remove(&label);
              }
            }
          }
          scip.write(tx)?;
        }
        Ok((deltas, scip))
      })?;
      self.state.scip = scip;
      ledger.add("persist_batches", 1);
      for delta in deltas {
        ledger.record_delta(&delta);
        self.events.absorb(&delta);
      }
    }
    Ok(())
  }

  fn follow_up(&mut self, resolve: &BTreeSet<FileKey>, first: &[Processed], ledger: &mut Ledger) -> EngineResult<Vec<Processed>> {
    let mut processed_keys: BTreeSet<FileKey> = resolve.iter().chain(&self.deleted).cloned().collect();
    let mut names: BTreeMap<EntityId, String> = first
      .iter()
      .flat_map(|item| item.symbols.iter().map(|symbol| (symbol.id, symbol.key.name().to_owned())))
      .collect();
    for item in first {
      self.stage_wanted(item);
    }
    let mut extra = Vec::new();
    let mut handled: Option<usize> = None;
    for round in 1..=MAX_RESOLVE_ROUNDS {
      if handled == Some(self.events.len()) {
        break;
      }
      handled = Some(self.events.len());
      let created = self.events.created();
      let collected = self.events.collected();
      let mut next: BTreeSet<FileKey> = created
        .iter()
        .chain(&collected)
        .filter_map(|id| self.state.wanted_by.get(id))
        .flatten()
        .filter(|key| !processed_keys.contains(*key))
        .cloned()
        .collect();
      ledger.add("wanted_target_owners", next.len() as u64);
      let safety = self.safety_net(&created, &collected, &names)?;
      let safety: BTreeSet<FileKey> = safety
        .into_iter()
        .filter(|key| !processed_keys.contains(key) && !next.contains(key) && self.state.files.contains_key(key))
        .collect();
      if !safety.is_empty() {
        tracing::warn!(round, owners = safety.len(), sample = %safety.iter().take(5).map(key_label).collect::<Vec<_>>().join(", "), "re-resolving owners missed by dependency tracking");
        ledger.add("safety_net_owners", safety.len() as u64);
      }
      next.extend(safety);
      next.retain(|key| self.state.table.contains_file(&key.0, &key.1));
      if next.is_empty() {
        break;
      }
      if round == MAX_RESOLVE_ROUNDS {
        tracing::error!(
          rounds = MAX_RESOLVE_ROUNDS,
          pending = next.len(),
          "dependency tracking did not converge; the next run re-resolves every file"
        );
        ledger.partial.push(format!("dependency tracking did not converge after {MAX_RESOLVE_ROUNDS} rounds"));
        self.converged = Convergence::Exhausted;
        break;
      }
      ledger.add("dependent_rounds", 1);
      ledger.add("owners_resolved", next.len() as u64);
      let mut artifacts = BTreeMap::new();
      self.load_artifacts(&next, &mut artifacts, ledger)?;
      let drafts = self.drafts(&next, &artifacts)?;
      let finalized = self.finalize(drafts, &[], &next, ledger)?;
      let (writes, processed) = self.file_writes(finalized, &artifacts, &BTreeSet::new(), ledger)?;
      self.persist(writes, ledger)?;
      for item in &processed {
        names.extend(item.symbols.iter().map(|symbol| (symbol.id, symbol.key.name().to_owned())));
        self.stage_wanted(item);
      }
      processed_keys.extend(next);
      extra.extend(processed);
    }
    Ok(extra)
  }

  fn safety_net(&self, created: &BTreeSet<EntityId>, collected: &BTreeSet<EntityId>, names: &BTreeMap<EntityId, String>) -> EngineResult<BTreeSet<FileKey>> {
    let created_names: Vec<&str> = created.iter().filter_map(|id| names.get(id).map(String::as_str)).collect();
    let collected_ids: Vec<EntityId> = collected.iter().copied().collect();
    self.env.store.read(|tx| -> EngineResult<BTreeSet<FileKey>> {
      let mut owners = BTreeSet::new();
      for owner in tx.owners_referencing(&collected_ids)? {
        if let Owner::File { repo, path } = owner {
          owners.insert((repo, path));
        }
      }
      for found in tx.unresolved_by_names(&created_names)? {
        if let Owner::File { repo, path } = found.owner {
          owners.insert((repo, path));
        }
      }
      Ok(owners)
    })
  }

  fn stage_wanted(&mut self, item: &Processed) {
    let Some(mut file) = self.state.files.get(&item.key).cloned() else { return };
    file.wanted = item.wanted.clone();
    self.state.set_file(item.key.clone(), file);
  }

  fn update_similarity(&mut self, processed: &[Processed], artifacts: &BTreeMap<FileKey, Arc<FileArtifact>>, analyzed: &BTreeSet<FileKey>) -> EngineResult<()> {
    let mut changes: Vec<(Vec<EntityId>, Vec<atlas_similarity::SimilarityDoc>)> = Vec::new();
    for key in &self.deleted {
      if let Some(state) = self.state.files.get(key) {
        changes.push((state.symbol_ids().collect(), Vec::new()));
      }
    }
    for item in processed {
      let old: BTreeSet<EntityId> = self.state.files.get(&item.key).map(|state| state.symbol_ids().collect()).unwrap_or_default();
      let new: BTreeSet<EntityId> = item.symbols.iter().map(|symbol| symbol.id).collect();
      if old == new && !analyzed.contains(&item.key) {
        continue;
      }
      let Some(artifact) = artifacts.get(&item.key) else { continue };
      changes.push((old.difference(&new).copied().collect(), symbol_docs(artifact, &item.symbols)));
    }
    if changes.is_empty() {
      return Ok(());
    }
    let mut guard = crate::similarity::write_lock(self.similarity);
    let index = crate::similarity::writable(&mut guard)?;
    for (removed, docs) in changes {
      replace_symbols(index, removed, docs)?;
    }
    Ok(())
  }

  fn update_file_states(&mut self, processed: &[Processed], artifacts: &BTreeMap<FileKey, Arc<FileArtifact>>, contexts: &BTreeMap<FileKey, KeyContext>) {
    for key in self.deleted.clone() {
      self.state.remove_file(&key);
    }
    for item in processed {
      let existing = self.state.files.get(&item.key).cloned();
      let file = match (artifacts.get(&item.key), existing) {
        (Some(artifact), existing) => FileState {
          hash: artifact.facts.file.content_hash,
          upsert_context: contexts
            .get(&item.key)
            .cloned()
            .or_else(|| existing.map(|state| state.upsert_context))
            .unwrap_or_else(|| KeyContext {
              package: UNPACKAGED.to_owned(),
              module_path: Vec::new(),
            }),
          package: artifact.package.clone(),
          symbols: symbols_of(&item.symbols),
          wanted: item.wanted.clone(),
          failure: artifact.failure().map(str::to_owned),
          dangling: item.dangling,
        },
        (None, Some(mut existing)) => {
          existing.symbols = symbols_of(&item.symbols);
          existing.wanted = item.wanted.clone();
          existing.dangling = item.dangling;
          existing
        }
        (None, None) => continue,
      };
      self.state.set_file(item.key.clone(), file);
    }
  }

  fn lineage(&mut self, inputs: LineageInputs) -> EngineResult<usize> {
    let collected = self.events.collected();
    let created = self.events.created();
    let disappeared: Vec<LineageSide> = inputs.disappeared.into_iter().filter(|side| collected.contains(&side.id)).collect();
    let appeared: Vec<LineageSide> = inputs.appeared.into_iter().filter(|side| created.contains(&side.id)).collect();
    let pairs = match_renames(&disappeared, &appeared, &self.moved);
    let fresh: Vec<_> = pairs.iter().filter_map(|pair| lineage_edge(pair, self.run)).collect();
    if fresh.is_empty() && collected.is_empty() {
      return Ok(0);
    }
    let owner = Owner::Project { pass: PASS_LINEAGE.to_owned() };
    let run = self.run;
    self.env.store.write(|tx| -> EngineResult<()> {
      tx.resume_run(run)?;
      let previous = tx.owned_facts(&owner)?.edges;
      if previous.is_empty() && fresh.is_empty() {
        return Ok(());
      }
      let mut exists: HashMap<EntityId, bool> = HashMap::new();
      for edge in previous.iter().chain(&fresh) {
        if let std::collections::hash_map::Entry::Vacant(slot) = exists.entry(edge.from) {
          slot.insert(tx.entity(edge.from)?.is_some());
        }
      }
      let edges = retained_lineage(previous, fresh, |id| exists.get(&id).copied().unwrap_or(false));
      tx.replace_owner(
        &owner,
        &OwnedFacts {
          entities: Vec::new(),
          edges,
          unresolved: Vec::new(),
        },
      )?;
      migrate_inferred(tx, &pairs, run)
    })?;
    Ok(pairs.len())
  }

  fn project_passes(&mut self, ledger: &mut Ledger) -> EngineResult<()> {
    let graphs: BTreeMap<RepoName, PackageGraph> = self
      .state
      .repos
      .iter()
      .filter_map(|(repo, state)| Some((repo.clone(), clone_graph(state.graph.as_ref()?))))
      .collect();
    let manifest_hashes: BTreeMap<(RepoName, RelPath), ContentHash> = self
      .plans
      .iter()
      .flat_map(|(repo, plan)| {
        plan
          .stamps
          .iter()
          .filter(|(path, _)| is_manifest(path))
          .map(move |(path, stamp)| ((repo.clone(), path.clone()), stamp.hash))
      })
      .collect();
    let context = PassContext {
      project: self.env.project,
      graphs: &graphs,
      manifest_hashes: &manifest_hashes,
    };
    let builtin: [Arc<dyn ProjectPass>; 2] = [Arc::new(StructurePass), Arc::new(LinkPass)];
    let passes: Vec<Arc<dyn ProjectPass>> = builtin.into_iter().chain(self.env.options.passes.iter().cloned()).collect();
    for pass in passes {
      let name = pass.name();
      let span = tracing::info_span!("project_pass", pass = name);
      let _entered = span.enter();
      let output = self.env.store.read(|tx| -> EngineResult<Result<PassOutput, PassError>> {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pass.run(tx, &context)));
        Ok(outcome.unwrap_or_else(|payload| Err(PassError::Failed(format!("panicked: {}", crate::analyze::panic_message(payload.as_ref()))))))
      })?;
      let facts = match output {
        Ok(output) => output.into_owned(),
        Err(error) => {
          tracing::error!(pass = name, error = %error, "project pass failed; its facts were removed");
          ledger.partial.push(format!("project pass {name} failed: {error}"));
          OwnedFacts::default()
        }
      };
      let run = self.run;
      let delta = self.env.store.write(|tx| -> EngineResult<FactDelta> {
        tx.resume_run(run)?;
        Ok(tx.replace_owner(&Owner::Project { pass: name.to_owned() }, &facts)?)
      })?;
      ledger.record_delta(&delta);
    }
    Ok(())
  }

  fn finish(&mut self, ledger: &mut Ledger, analyzed: &BTreeSet<FileKey>) -> EngineResult<()> {
    let mut meta = EngineMeta {
      facts_version: FACTS_VERSION,
      resolver_version: atlas_resolve::RESOLVER_VERSION,
      artifact_version: ARTIFACT_VERSION,
      generation: Some(self.run.0),
      repos: BTreeMap::new(),
    };
    for (repo, plan) in &self.plans {
      meta.repos.insert(
        repo.to_string(),
        RepoMeta {
          graph_fingerprint: self.graph(repo).map(graph_fingerprint).unwrap_or_default(),
          scan_started_ns: Some(plan.started_ns),
        },
      );
    }
    ledger.add("similarity_documents", crate::similarity::read_lock(self.similarity).len() as u64);
    let meta_bytes = serde_json::to_vec(&meta).map_err(|error| EngineError::codec("engine meta", error))?;
    let change_set = json!({
      "analyzed": capped(analyzed.iter().map(key_label)),
      "deleted": capped(self.deleted.iter().map(key_label)),
      "moved": capped(self.moved.iter().map(|(from, to)| format!("{} -> {}", key_label(from), key_label(to)))),
      "graphs_changed": capped(self.graph_changed.iter().map(RepoName::to_string)),
    });
    let outcome = match ledger.outcome() {
      RunOutcomeKind::Ok => RunOutcome::Ok,
      RunOutcomeKind::Partial { reason } => RunOutcome::Partial { reason },
    };
    let report = RunReport {
      phase_timings_ms: ledger.phases.clone(),
      counts: ledger.counts.clone(),
      change_set,
    };
    let run = self.run;
    let converged = self.converged;
    let configured: BTreeSet<RepoName> = self.env.project.repos.iter().map(|repo| repo.name.clone()).collect();
    let stale = self.env.store.write(|tx| -> EngineResult<u64> {
      tx.resume_run(run)?;
      tx.put_blob(META_BLOB, META_BLOB_VERSION, &meta_bytes)?;
      write_known_repos(tx, &configured)?;
      if converged == Convergence::Converged {
        tx.delete_blob(DIRTY_BLOB)?;
      }
      let stale = count_inferred_stale(tx)?;
      tx.finish_run(run, outcome, &report)?;
      Ok(stale)
    })?;
    for (repo, plan) in std::mem::take(&mut self.plans) {
      if let Some(state) = self.state.repos.get_mut(&repo) {
        state.stamps = plan.stamps.into_iter().collect();
        state.scan_started_ns = Some(plan.started_ns);
      }
    }
    self.state.pending = Default::default();
    self.state.generation = Some(run);
    self.state.stale_inferred = stale;
    Ok(())
  }
}

fn manifest_record(repo: &RepoName, path: &RelPath, stamp: &FileStamp, run: RunId) -> FileRecord {
  FileRecord {
    repo: repo.clone(),
    path: path.clone(),
    language: None,
    content_hash: stamp.hash,
    size: stamp.size,
    mtime_ns: stamp.mtime_ns,
    inode: Some(stamp.inode),
    parse_status: atlas_core::ParseStatus::Ok,
    analyzer: None,
    indexed_run: Some(run),
  }
}

fn apply_write(tx: &mut WriteTx<'_>, write: Write, deltas: &mut Vec<FactDelta>, scip_dropped: &mut BTreeSet<FileKey>) -> EngineResult<()> {
  let scip_owner = |key: &FileKey| Owner::Project {
    pass: ScipFacts::owner_pass(&key.0, &key.1),
  };
  match write {
    Write::RemoveFile { key } => {
      deltas.push(tx.remove_owner(&file_owner(&key))?);
      deltas.push(tx.remove_owner(&scip_owner(&key))?);
      tx.delete_file(&key.0, &key.1)?;
      tx.delete_blob(&FileArtifact::blob_name(&key.0, &key.1))?;
      tx.mark_stale_where_evidence_removed(&key.0, &key.1)?;
      scip_dropped.insert(key);
    }
    Write::File { key, facts, update } => {
      deltas.push(tx.replace_owner(&file_owner(&key), &facts)?);
      deltas.push(tx.remove_owner(&scip_owner(&key))?);
      if let Some(update) = update {
        tx.put_blob(&FileArtifact::blob_name(&key.0, &key.1), ARTIFACT_VERSION, &update.artifact)?;
        tx.upsert_file(&update.record)?;
        if update.content_changed {
          tx.mark_stale_where_evidence_changed(&key.0, &key.1, &update.record.content_hash)?;
        }
      }
      scip_dropped.insert(key);
    }
    Write::Owner { owner, facts } => deltas.push(tx.replace_owner(&owner, &facts)?),
    Write::Record { record } => tx.upsert_file(&record)?,
    Write::Restamp { key, stamp } => {
      if let Some(mut record) = tx.file(&key.0, &key.1)? {
        record.size = stamp.size;
        record.mtime_ns = stamp.mtime_ns;
        record.inode = Some(stamp.inode);
        tx.upsert_file(&record)?;
      }
    }
    Write::DeleteRecord { key } => {
      tx.delete_file(&key.0, &key.1)?;
    }
  }
  Ok(())
}

fn migrate_inferred(tx: &mut WriteTx<'_>, pairs: &[RenamePair], run: RunId) -> EngineResult<()> {
  for pair in pairs.iter().filter(|pair| pair.confidence >= MIN_MIGRATION_CONFIDENCE) {
    let facts = tx.inferred_facts(&InferredQuery {
      subject: Some(pair.old),
      relation: None,
      freshness: FreshnessPolicy::IncludeStale,
    })?;
    for fact in facts {
      let subjects = fact.subjects.iter().map(|subject| if *subject == pair.old { pair.new } else { *subject }).collect();
      let migrated = tx.insert_inferred_fact(&NewInferredFact {
        subjects,
        relation: fact.relation.clone(),
        payload: fact.payload.clone(),
        certainty: fact.certainty.clone(),
        evidence: fact.evidence.clone(),
        created_run: fact.created_run.or(Some(run)),
        llm_call: fact.llm_call,
      })?;
      tx.mark_needs_revalidation(migrated, &format!("subject renamed from {}", pair.old_key))?;
      tx.delete_inferred_fact(fact.id)?;
      tracing::info!(old = %pair.old, new = %pair.new, fact = fact.id.0, "migrated an inferred fact to a renamed symbol");
    }
  }
  Ok(())
}
