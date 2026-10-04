use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use atlas_core::{ContentHash, KeyContext, Owner, RelPath, RepoName, RunId};
use atlas_project::{Ecosystem, PackageGraph, PackageInfo, RepoDir};
use atlas_scip::{ScipFacts, ScipFileContext, ScipIndexer, ScipIngestContext, ScipInputFile, ScipOutcome, ScipRequest, ScipStats, ingest};
use atlas_store::{OwnedFacts, RunOutcome, RunReport};
use serde_json::json;

use crate::artifact::{FileArtifact, FileKey};
use crate::engine::{Engine, lock_inner};
use crate::error::{EngineError, EngineResult};
use crate::lock::ScipLock;
use crate::packages::graph_fingerprint;
use crate::run::key_label;
use crate::state::{DIRTY_BLOB, EngineMeta, EngineState, ScipMeta};

pub struct ScipIndexers<'a> {
  pub cargo: Option<&'a dyn ScipIndexer>,
  pub npm: Option<&'a dyn ScipIndexer>,
}

impl<'a> ScipIndexers<'a> {
  fn for_ecosystem(&self, ecosystem: Ecosystem) -> Option<&'a dyn ScipIndexer> {
    match ecosystem {
      Ecosystem::Cargo => self.cargo,
      Ecosystem::Npm => self.npm,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScipPackageOutcome {
  Ingested { files: u64, edges: u64, skipped_changed: u64, stats: ScipStats },
  NoIndexer,
  IndexerUnavailable { reason: String },
  IndexerFailed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipPackageReport {
  pub repo: RepoName,
  pub manifest: RelPath,
  pub ecosystem: Ecosystem,
  pub indexer: Option<String>,
  pub outcome: ScipPackageOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipReport {
  pub run: RunId,
  pub packages: Vec<ScipPackageReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScipTrigger {
  Explicit,
  Automatic,
}

impl ScipTrigger {
  fn ledger_name(self) -> &'static str {
    match self {
      Self::Explicit => "scip",
      Self::Automatic => "scip-auto",
    }
  }
}

pub(crate) type HeldPackages<'a> = &'a dyn Fn(&RepoName, &RelPath, &str) -> bool;

pub(crate) struct ScipRunOutput {
  pub report: ScipReport,
  pub fingerprints: BTreeMap<(RepoName, RelPath), String>,
}

pub(crate) struct PackageJob {
  pub repo: RepoName,
  repo_root: PathBuf,
  pub manifest: RelPath,
  pub ecosystem: Ecosystem,
  root: PathBuf,
  package_dir: Option<RelPath>,
  pub files: BTreeMap<RelPath, ContentHash>,
  generation: Option<RunId>,
  graph_fingerprint: String,
  dropped_in: BTreeMap<String, i64>,
}

impl PackageJob {
  pub fn fingerprint(&self) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(self.graph_fingerprint.as_bytes());
    for (path, hash) in &self.files {
      hasher.update(&(path.as_str().len() as u64).to_le_bytes());
      hasher.update(path.as_str().as_bytes());
      hasher.update(hash.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
  }
}

pub(crate) fn package_jobs(state: &EngineState) -> Vec<PackageJob> {
  let mut jobs = Vec::new();
  for (repo, repo_state) in &state.repos {
    let Some(graph) = &repo_state.graph else { continue };
    let units = scip_units(graph);
    let mut unit_files: BTreeMap<RelPath, BTreeMap<RelPath, ContentHash>> = BTreeMap::new();
    for ((file_repo, path), file) in &state.files {
      if file_repo != repo || file.failure.is_some() {
        continue;
      }
      let Some((manifest, _, _)) = graph.package_for_file(path).and_then(|package| units.get(&package.manifest)) else {
        continue;
      };
      unit_files.entry(manifest.clone()).or_default().insert(path.clone(), file.hash);
    }
    let roots: BTreeMap<&RelPath, (&RepoDir, Ecosystem)> = units.values().map(|(manifest, root, ecosystem)| (manifest, (root, *ecosystem))).collect();
    let fingerprint = graph_fingerprint(graph);
    for (manifest, files) in unit_files {
      let Some((root, ecosystem)) = roots.get(&manifest).copied() else { continue };
      jobs.push(PackageJob {
        repo: repo.clone(),
        repo_root: repo_state.config.path.clone(),
        ecosystem,
        root: repo_state.config.path.join(root.as_str()),
        package_dir: root.as_rel_path().cloned(),
        manifest,
        files,
        generation: state.generation,
        graph_fingerprint: fingerprint.clone(),
        dropped_in: BTreeMap::new(),
      });
    }
  }
  jobs
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStat {
  len: u64,
  modified: Option<SystemTime>,
}

fn file_stat(path: &Path) -> Option<FileStat> {
  let metadata = std::fs::metadata(path).ok()?;
  Some(FileStat {
    len: metadata.len(),
    modified: metadata.modified().ok(),
  })
}

struct Snapshot {
  repos: Vec<RepoName>,
  current: BTreeMap<FileKey, KeyContext>,
}

struct Ingestion {
  facts: ScipFacts,
  included: BTreeSet<FileKey>,
  lagging: BTreeSet<FileKey>,
}

pub(crate) fn now_ms() -> i64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

fn relative_to(path: &RelPath, dir: Option<&RelPath>) -> Option<RelPath> {
  match dir {
    Some(dir) => RelPath::new(path.strip_dir_prefix(dir)?).ok(),
    None => Some(path.clone()),
  }
}

fn indexing_unit(graph: &PackageGraph, package: &PackageInfo) -> (RelPath, RepoDir) {
  let workspace = package
    .as_cargo()
    .and_then(|cargo| cargo.workspace_root.as_ref())
    .and_then(|root| graph.workspaces.iter().find(|workspace| workspace.ecosystem == Ecosystem::Cargo && workspace.root == *root));
  match workspace {
    Some(workspace) => (workspace.manifest.clone(), workspace.root.clone()),
    None => (package.manifest.clone(), package.root.clone()),
  }
}

fn scip_units(graph: &PackageGraph) -> HashMap<RelPath, (RelPath, RepoDir, Ecosystem)> {
  graph
    .packages
    .iter()
    .map(|package| {
      let (manifest, root) = indexing_unit(graph, package);
      (package.manifest.clone(), (manifest, root, package.ecosystem()))
    })
    .collect()
}

impl Engine {
  pub fn scip(&self, indexers: &ScipIndexers<'_>) -> EngineResult<ScipReport> {
    let mut lock = ScipLock::open(&self.db_path())?;
    let output = lock.hold(|| self.scip_locked(indexers, ScipTrigger::Explicit, &AtomicBool::new(false), &|_, _, _| false))??;
    Ok(output.report)
  }

  pub(crate) fn scip_locked(&self, indexers: &ScipIndexers<'_>, trigger: ScipTrigger, cancel: &AtomicBool, held: HeldPackages<'_>) -> EngineResult<ScipRunOutput> {
    let span = tracing::info_span!("scip_run", trigger = trigger.ledger_name());
    let _entered = span.enter();
    let started = Instant::now();
    let mut fingerprints = BTreeMap::new();
    let mut jobs = Vec::new();
    for job in self.scip_jobs()? {
      let fingerprint = job.fingerprint();
      if held(&job.repo, &job.manifest, &fingerprint) {
        tracing::debug!(repo = %job.repo, manifest = %job.manifest, "skipping a package whose indexer failed on the same inputs");
        continue;
      }
      fingerprints.insert((job.repo.clone(), job.manifest.clone()), fingerprint);
      jobs.push(job);
    }
    let run = self.store.write(|tx| -> EngineResult<RunId> { Ok(tx.begin_run(trigger.ledger_name())?) })?;
    let cache_dir = self.home.project_data_dir(&self.project().name).join("scip-cache");
    let mut packages = Vec::new();
    let mut cancelled = false;
    for job in jobs {
      if cancel.load(Ordering::SeqCst) {
        cancelled = true;
        break;
      }
      let indexer = indexers.for_ecosystem(job.ecosystem);
      let outcome = match indexer {
        None => Ok(ScipPackageOutcome::NoIndexer),
        Some(indexer) => self.scip_package(&job, indexer, &cache_dir, run, cancel),
      };
      let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
          let failed = RunOutcome::Failed { error: error.to_string() };
          if let Err(ledger) = self.store.write(|tx| tx.finish_run(run, failed, &RunReport::default())) {
            tracing::warn!(error = %ledger, "could not record the failed SCIP run");
          }
          return Err(error);
        }
      };
      tracing::info!(repo = %job.repo, manifest = %job.manifest, outcome = ?outcome, "scip package finished");
      packages.push(ScipPackageReport {
        repo: job.repo,
        manifest: job.manifest,
        ecosystem: job.ecosystem,
        indexer: indexer.map(|indexer| indexer.id().to_owned()),
        outcome,
      });
    }
    cancelled |= cancel.load(Ordering::SeqCst);
    let report = RunReport {
      phase_timings_ms: BTreeMap::from([("total".to_owned(), started.elapsed().as_millis() as u64)]),
      counts: report_counts(&packages),
      change_set: json!({ "packages": packages.iter().map(|package| format!("{}:{}", package.repo, package.manifest)).collect::<Vec<_>>() }),
    };
    let outcome = match cancelled {
      true => RunOutcome::Partial {
        reason: "cancelled before every package was indexed".to_owned(),
      },
      false => RunOutcome::Ok,
    };
    self.store.write(|tx| -> EngineResult<()> { Ok(tx.finish_run(run, outcome, &report)?) })?;
    Ok(ScipRunOutput {
      report: ScipReport { run, packages },
      fingerprints,
    })
  }

  fn scip_jobs(&self) -> EngineResult<Vec<PackageJob>> {
    let mut inner = lock_inner(&self.inner);
    self.sync(&mut inner)?;
    let state = &inner.state;
    let mut jobs = package_jobs(state);
    for job in &mut jobs {
      job.dropped_in = state.scip.dropped_in.clone();
    }
    Ok(jobs)
  }

  fn scip_package(&self, job: &PackageJob, indexer: &dyn ScipIndexer, cache_dir: &Path, run: RunId, cancel: &AtomicBool) -> EngineResult<ScipPackageOutcome> {
    let span = tracing::info_span!("scip_package", repo = %job.repo, manifest = %job.manifest, indexer = indexer.id(), files = job.files.len());
    let _entered = span.enter();
    let files: Vec<ScipInputFile> = job
      .files
      .iter()
      .filter_map(|(path, hash)| {
        Some(ScipInputFile {
          path: relative_to(path, job.package_dir.as_ref())?,
          content_hash: *hash,
        })
      })
      .collect();
    let request = ScipRequest {
      root: job.root.clone(),
      cache_dir: cache_dir.to_path_buf(),
      files,
    };
    let stats: BTreeMap<RelPath, Option<FileStat>> = job.files.keys().map(|path| (path.clone(), file_stat(&job.repo_root.join(path.as_str())))).collect();
    let scip_run = indexer.index_with_cancel(&request, cancel);
    for warning in &scip_run.warnings {
      tracing::info!(warning = %warning, "scip indexer warning");
    }
    let output = match (&scip_run.outcome, &scip_run.output_path) {
      (ScipOutcome::Ok, Some(path)) => path.clone(),
      (ScipOutcome::Ok, None) => {
        return Ok(ScipPackageOutcome::IndexerFailed {
          reason: "indexer reported success without an index".to_owned(),
        });
      }
      (ScipOutcome::Unavailable { reason }, _) => return Ok(ScipPackageOutcome::IndexerUnavailable { reason: reason.clone() }),
      (ScipOutcome::Failed { reason, .. }, _) => return Ok(ScipPackageOutcome::IndexerFailed { reason: reason.clone() }),
      (ScipOutcome::TimedOut, _) => {
        return Ok(ScipPackageOutcome::IndexerFailed {
          reason: "indexer timed out".to_owned(),
        });
      }
    };
    let bytes = std::fs::read(&output).map_err(|source| EngineError::io(&output, source))?;
    let snapshot = self.scip_snapshot(job);
    let Ingestion { facts, mut included, lagging } = match self.scip_ingest(job, &snapshot, &stats, &bytes)? {
      Ok(ingestion) => ingestion,
      Err(reason) => return Ok(ScipPackageOutcome::IndexerFailed { reason }),
    };
    let mut edges_written = 0u64;
    let mut package_stale = false;
    let mut written: BTreeSet<FileKey> = BTreeSet::new();
    let timeout = self.options.store.lock_timeout;
    let store = &self.store;
    let mut inner = lock_inner(&self.inner);
    let scip_meta = inner.lock.hold(timeout, |_| {
      store.write(|tx| -> EngineResult<ScipMeta> {
        tx.resume_run(run)?;
        let mut scip_meta = ScipMeta::read(tx)?;
        let meta = EngineMeta::read(tx)?;
        let moved = meta.as_ref().and_then(|meta| meta.generation).map(RunId) != job.generation;
        package_stale = tx.blob(DIRTY_BLOB)?.is_some()
          || moved && meta.as_ref().and_then(|meta| meta.repos.get(job.repo.as_str())).map(|repo| repo.graph_fingerprint.as_str()) != Some(job.graph_fingerprint.as_str());
        let reresolved = |label: &String| scip_meta.dropped_in.get(label) != job.dropped_in.get(label);
        if package_stale {
          tracing::info!(repo = %job.repo, manifest = %job.manifest, "the package graph changed or a run is unfinished after the SCIP indexer ran; leaving the package lagging");
          for path in job.files.keys() {
            scip_meta.lagging.insert(key_label(&(job.repo.clone(), path.clone())));
          }
          included.clear();
        }
        for key in &lagging {
          scip_meta.lagging.insert(key_label(key));
        }
        for key in &included {
          let indexed = job.files.get(&key.1).copied();
          if reresolved(&key_label(key)) || tx.file(&key.0, &key.1)?.map(|record| record.content_hash) != indexed {
            scip_meta.lagging.insert(key_label(key));
            continue;
          }
          written.insert(key.clone());
          let edges = facts.per_file.get(key).cloned().unwrap_or_default();
          edges_written += edges.len() as u64;
          let owner = Owner::Project {
            pass: ScipFacts::owner_pass(&key.0, &key.1),
          };
          tx.replace_owner(
            &owner,
            &OwnedFacts {
              entities: Vec::new(),
              edges,
              unresolved: Vec::new(),
            },
          )?;
          scip_meta.lagging.remove(&key_label(key));
        }
        let mut live: BTreeSet<String> = BTreeSet::new();
        for repo in &snapshot.repos {
          live.extend(tx.files_in_repo(repo)?.into_iter().map(|record| key_label(&(record.repo, record.path))));
        }
        scip_meta.lagging.retain(|label| live.contains(label));
        scip_meta.dropped_in.retain(|label, _| live.contains(label));
        scip_meta.last_run_ms = Some(now_ms());
        scip_meta.write(tx)?;
        Ok(scip_meta)
      })
    })??;
    inner.state.scip = scip_meta;
    Ok(ScipPackageOutcome::Ingested {
      files: written.len() as u64,
      edges: edges_written,
      skipped_changed: match package_stale {
        true => job.files.len() as u64,
        false => (lagging.len() + included.len() - written.len()) as u64,
      },
      stats: facts.stats,
    })
  }

  fn scip_snapshot(&self, job: &PackageJob) -> Snapshot {
    let inner = lock_inner(&self.inner);
    let state = &inner.state;
    let current = job
      .files
      .iter()
      .filter(|(path, hash)| state.files.get(&(job.repo.clone(), (*path).clone())).is_some_and(|file| file.hash == **hash))
      .filter_map(|(path, _)| Some(((job.repo.clone(), path.clone()), state.table.placement(&job.repo, path)?.context)))
      .collect();
    Snapshot {
      repos: state.repos.keys().cloned().collect(),
      current,
    }
  }

  /// Runs without the engine mutex so barriers are not blocked while a large index is decoded; the write phase re-checks every file.
  fn scip_ingest(&self, job: &PackageJob, snapshot: &Snapshot, stats: &BTreeMap<RelPath, Option<FileStat>>, bytes: &[u8]) -> EngineResult<Result<Ingestion, String>> {
    let artifacts = self.store.read(|tx| -> EngineResult<Vec<(FileKey, Option<atlas_store::DerivedBlob>)>> {
      snapshot
        .current
        .keys()
        .map(|key| Ok((key.clone(), tx.blob(&FileArtifact::blob_name(&key.0, &key.1))?)))
        .collect()
    })?;
    let mut context = ScipIngestContext::new(job.repo.clone());
    if let Some(dir) = &job.package_dir {
      context = context.in_package_dir(dir.clone());
    }
    let mut included: BTreeSet<FileKey> = BTreeSet::new();
    let mut lagging: BTreeSet<FileKey> = job
      .files
      .keys()
      .map(|path| (job.repo.clone(), path.clone()))
      .filter(|key| !snapshot.current.contains_key(key))
      .collect();
    for (key, blob) in artifacts {
      let indexed = job.files.get(&key.1).copied();
      let (Some(artifact), Some(placement)) = (blob.map(|blob| FileArtifact::decode(&blob.bytes)).transpose()?, snapshot.current.get(&key)) else {
        lagging.insert(key);
        continue;
      };
      let absolute = job.repo_root.join(key.1.as_str());
      let source = std::fs::read_to_string(&absolute).ok();
      let untouched = stats.get(&key.1).is_some_and(|before| before.is_some() && file_stat(&absolute) == *before);
      let consistent = source.as_ref().is_some_and(|source| Some(ContentHash::of(source.as_bytes())) == indexed) && Some(artifact.facts.file.content_hash) == indexed;
      let (Some(source), true, true) = (source, consistent, untouched) else {
        tracing::info!(repo = %key.0, path = %key.1, "file changed while the SCIP indexer ran; leaving it lagging");
        lagging.insert(key);
        continue;
      };
      let keys = artifact.facts.symbol_keys(placement)?;
      context = context.with_file(key.1.clone(), ScipFileContext::from_facts(&artifact.facts, keys, source));
      included.insert(key);
    }
    Ok(match ingest(bytes, &context) {
      Ok(facts) => Ok(Ingestion { facts, included, lagging }),
      Err(error) => Err(format!("unreadable index: {error}")),
    })
  }
}

fn report_counts(packages: &[ScipPackageReport]) -> BTreeMap<String, u64> {
  let mut counts = BTreeMap::from([("packages".to_owned(), packages.len() as u64)]);
  for package in packages {
    if let ScipPackageOutcome::Ingested { files, edges, .. } = &package.outcome {
      *counts.entry("packages_ingested".to_owned()).or_default() += 1;
      *counts.entry("files_ingested".to_owned()).or_default() += files;
      *counts.entry("edges".to_owned()).or_default() += edges;
    }
  }
  counts.entry("packages_ingested".to_owned()).or_default();
  counts
}
