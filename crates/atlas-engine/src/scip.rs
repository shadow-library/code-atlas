use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use atlas_core::{ContentHash, Owner, RelPath, RepoName, RunId};
use atlas_project::{Ecosystem, PackageInfo};
use atlas_scip::{ScipFacts, ScipFileContext, ScipIndexer, ScipIngestContext, ScipInputFile, ScipOutcome, ScipRequest, ScipStats, ingest};
use atlas_store::{OwnedFacts, RunOutcome, RunReport};
use serde_json::json;

use crate::artifact::{FileArtifact, FileKey};
use crate::engine::{Engine, lock_inner};
use crate::error::{EngineError, EngineResult};
use crate::run::key_label;
use crate::state::ScipMeta;

pub struct ScipIndexers<'a> {
  pub cargo: Option<&'a dyn ScipIndexer>,
  pub npm: Option<&'a dyn ScipIndexer>,
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
  pub indexer: Option<String>,
  pub outcome: ScipPackageOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipReport {
  pub run: RunId,
  pub packages: Vec<ScipPackageReport>,
}

struct PackageJob {
  repo: RepoName,
  manifest: RelPath,
  ecosystem: Ecosystem,
  root: PathBuf,
  package_dir: Option<RelPath>,
  files: BTreeMap<RelPath, ContentHash>,
}

fn now_ms() -> i64 {
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

impl Engine {
  pub fn scip(&self, indexers: &ScipIndexers<'_>) -> EngineResult<ScipReport> {
    let jobs = self.scip_jobs()?;
    let run = self.store.write(|tx| -> EngineResult<RunId> { Ok(tx.begin_run("scip")?) })?;
    let cache_dir = self.home.project_data_dir(&self.project().name).join("scip-cache");
    let mut packages = Vec::new();
    for job in jobs {
      let indexer = match job.ecosystem {
        Ecosystem::Cargo => indexers.cargo,
        Ecosystem::Npm => indexers.npm,
      };
      let outcome = match indexer {
        None => ScipPackageOutcome::NoIndexer,
        Some(indexer) => self.scip_package(&job, indexer, &cache_dir, run)?,
      };
      tracing::info!(repo = %job.repo, manifest = %job.manifest, outcome = ?outcome, "scip package finished");
      packages.push(ScipPackageReport {
        repo: job.repo,
        manifest: job.manifest,
        indexer: indexer.map(|indexer| indexer.id().to_owned()),
        outcome,
      });
    }
    let ingested = packages.iter().filter(|package| matches!(package.outcome, ScipPackageOutcome::Ingested { .. })).count() as u64;
    let report = RunReport {
      phase_timings_ms: BTreeMap::new(),
      counts: BTreeMap::from([("packages".to_owned(), packages.len() as u64), ("packages_ingested".to_owned(), ingested)]),
      change_set: json!({ "packages": packages.iter().map(|package| format!("{}:{}", package.repo, package.manifest)).collect::<Vec<_>>() }),
    };
    self.store.write(|tx| -> EngineResult<()> { Ok(tx.finish_run(run, RunOutcome::Ok, &report)?) })?;
    Ok(ScipReport { run, packages })
  }

  fn scip_jobs(&self) -> EngineResult<Vec<PackageJob>> {
    let mut inner = lock_inner(&self.inner);
    self.sync(&mut inner)?;
    let state = &inner.state;
    let mut jobs = Vec::new();
    for (repo, repo_state) in &state.repos {
      let Some(graph) = &repo_state.graph else { continue };
      for package in &graph.packages {
        let files: BTreeMap<RelPath, ContentHash> = state
          .files
          .iter()
          .filter(|((file_repo, path), file)| file_repo == repo && file.failure.is_none() && owns(graph.package_for_file(path), package))
          .map(|((_, path), file)| (path.clone(), file.hash))
          .collect();
        if files.is_empty() {
          continue;
        }
        let package_dir = package.root.as_rel_path().cloned();
        jobs.push(PackageJob {
          repo: repo.clone(),
          manifest: package.manifest.clone(),
          ecosystem: package.ecosystem(),
          root: repo_state.config.path.join(package.root.as_str()),
          package_dir,
          files,
        });
      }
    }
    Ok(jobs)
  }

  fn scip_package(&self, job: &PackageJob, indexer: &dyn ScipIndexer, cache_dir: &std::path::Path, run: RunId) -> EngineResult<ScipPackageOutcome> {
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
    let scip_run = indexer.index(&request);
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
    let mut inner = lock_inner(&self.inner);
    let state = &inner.state;
    let current: BTreeSet<FileKey> = job
      .files
      .iter()
      .filter(|(path, hash)| state.files.get(&(job.repo.clone(), (*path).clone())).is_some_and(|file| file.hash == **hash))
      .map(|(path, _)| (job.repo.clone(), path.clone()))
      .collect();
    let artifacts = self.store.read(|tx| -> EngineResult<Vec<(FileKey, Option<atlas_store::DerivedBlob>)>> {
      current.iter().map(|key| Ok((key.clone(), tx.blob(&FileArtifact::blob_name(&key.0, &key.1))?))).collect()
    })?;
    let mut context = ScipIngestContext::new(job.repo.clone());
    if let Some(dir) = &job.package_dir {
      context = context.in_package_dir(dir.clone());
    }
    let mut included: BTreeSet<FileKey> = BTreeSet::new();
    let mut lagging: BTreeSet<FileKey> = job.files.keys().map(|path| (job.repo.clone(), path.clone())).filter(|key| !current.contains(key)).collect();
    for (key, blob) in artifacts {
      let indexed = job.files.get(&key.1).copied();
      let Some(artifact) = blob.map(|blob| FileArtifact::decode(&blob.bytes)).transpose()? else {
        lagging.insert(key);
        continue;
      };
      let source = state.repos.get(&key.0).and_then(|repo| std::fs::read_to_string(repo.config.path.join(key.1.as_str())).ok());
      let placement = state.table.placement(&key.0, &key.1);
      let consistent = source.as_ref().is_some_and(|source| Some(ContentHash::of(source.as_bytes())) == indexed) && Some(artifact.facts.file.content_hash) == indexed;
      let (Some(source), Some(placement), true) = (source, placement, consistent) else {
        tracing::info!(repo = %key.0, path = %key.1, "file changed while the SCIP indexer ran; leaving it lagging");
        lagging.insert(key);
        continue;
      };
      let keys = artifact.facts.symbol_keys(&placement.context)?;
      context = context.with_file(key.1.clone(), ScipFileContext::from_facts(&artifact.facts, keys, source));
      included.insert(key);
    }
    let facts = match ingest(&bytes, &context) {
      Ok(facts) => facts,
      Err(error) => {
        return Ok(ScipPackageOutcome::IndexerFailed {
          reason: format!("unreadable index: {error}"),
        });
      }
    };
    let mut edges_written = 0u64;
    let mut written: BTreeSet<FileKey> = BTreeSet::new();
    let timeout = self.options.store.lock_timeout;
    let store = &self.store;
    let scip_meta = inner.lock.hold(timeout, |_| {
      store.write(|tx| -> EngineResult<ScipMeta> {
        tx.resume_run(run)?;
        let mut scip_meta = ScipMeta::read(tx)?;
        for key in &lagging {
          scip_meta.lagging.insert(key_label(key));
        }
        for key in &included {
          let indexed = job.files.get(&key.1).copied();
          if tx.file(&key.0, &key.1)?.map(|record| record.content_hash) != indexed {
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
        scip_meta.last_run_ms = Some(now_ms());
        scip_meta.write(tx)?;
        Ok(scip_meta)
      })
    })??;
    inner.state.scip = scip_meta;
    Ok(ScipPackageOutcome::Ingested {
      files: written.len() as u64,
      edges: edges_written,
      skipped_changed: (lagging.len() + included.len() - written.len()) as u64,
      stats: facts.stats,
    })
  }
}

fn owns(found: Option<&PackageInfo>, package: &PackageInfo) -> bool {
  found.is_some_and(|found| found.manifest == package.manifest)
}
