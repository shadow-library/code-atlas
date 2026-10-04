use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{Edge, EntityKey, FileFacts, RelPath, RepoName, Span, SymbolKey};
use atlas_engine::Engine;
use atlas_project::Project;
use atlas_scip::{ScipFileContext, ScipIndexer, ScipIngestContext, ScipInputFile, ScipOutcome, ScipRequest, ingest};
use serde::{Deserialize, Serialize};

use super::error::{QualityError, QualityResult};
use super::snapshot::Snapshot;

const CARGO_MANIFEST: &str = "Cargo.toml";
const NPM_MANIFEST: &str = "package.json";
const TS_EXTENSIONS: [&str; 8] = ["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLine {
  pub repo: String,
  pub root: String,
  pub indexer: String,
  pub files: usize,
  pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexDigest {
  pub target: String,
  pub repo: String,
  pub root: String,
  pub indexer: String,
  pub blake3: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ecosystem {
  Cargo,
  Npm,
}

impl Ecosystem {
  fn indexer_name(self) -> &'static str {
    match self {
      Self::Cargo => "rust-analyzer",
      Self::Npm => "scip-typescript",
    }
  }
}

#[derive(Debug, Clone)]
pub struct OracleRun {
  pub repo: RepoName,
  pub root: PathBuf,
  pub ecosystem: Ecosystem,
  pub files: Vec<RelPath>,
}

pub struct RunContext<'a> {
  pub target: &'a str,
  pub project: &'a Project,
  pub engine: &'a Engine,
  pub snapshot: &'a Snapshot,
  pub cache_dir: &'a Path,
  pub cargo: Option<&'a dyn ScipIndexer>,
  pub npm: Option<&'a dyn ScipIndexer>,
}

pub struct RunOutput {
  pub line: PackageLine,
  pub digest: Option<IndexDigest>,
  pub per_file: BTreeMap<(RepoName, RelPath), Vec<Edge>>,
}

fn directory_of(path: &RelPath) -> PathBuf {
  Path::new(path.as_str()).parent().map(Path::to_owned).unwrap_or_default()
}

fn under(path: &RelPath, directory: &Path) -> bool {
  Path::new(path.as_str()).starts_with(directory)
}

fn extension_of(path: &RelPath) -> &str {
  Path::new(path.as_str()).extension().and_then(|extension| extension.to_str()).unwrap_or_default()
}

fn nearest_directory<'d>(path: &RelPath, directories: &'d [PathBuf]) -> Option<&'d PathBuf> {
  directories
    .iter()
    .filter(|directory| under(path, directory))
    .max_by_key(|directory| directory.components().count())
}

pub fn plan(snapshot: &Snapshot, project: &Project) -> Vec<OracleRun> {
  let mut runs = Vec::new();
  for repo in &project.repos {
    let paths: Vec<RelPath> = snapshot.files.keys().filter(|(name, _)| *name == repo.name).map(|(_, path)| path.clone()).collect();
    let rust_files: Vec<RelPath> = paths.iter().filter(|path| extension_of(path) == "rs").cloned().collect();
    let cargo_dirs: Vec<PathBuf> = paths
      .iter()
      .filter(|path| path.as_str().rsplit('/').next() == Some(CARGO_MANIFEST))
      .map(directory_of)
      .collect();
    let is_workspace =
      fs::read_to_string(repo.path.join(CARGO_MANIFEST)).is_ok_and(|text| text.contains("[workspace]")) && cargo_dirs.iter().any(|directory| directory.as_os_str().is_empty());
    match is_workspace {
      true if !rust_files.is_empty() => runs.push(OracleRun {
        repo: repo.name.clone(),
        root: PathBuf::new(),
        ecosystem: Ecosystem::Cargo,
        files: rust_files,
      }),
      true => {}
      false => {
        for directory in &cargo_dirs {
          let files: Vec<RelPath> = rust_files.iter().filter(|path| nearest_directory(path, &cargo_dirs) == Some(directory)).cloned().collect();
          if !files.is_empty() {
            runs.push(OracleRun {
              repo: repo.name.clone(),
              root: directory.clone(),
              ecosystem: Ecosystem::Cargo,
              files,
            });
          }
        }
      }
    }
    let npm_dirs: Vec<PathBuf> = paths
      .iter()
      .filter(|path| path.as_str().rsplit('/').next() == Some(NPM_MANIFEST))
      .map(directory_of)
      .collect();
    for directory in &npm_dirs {
      let files: Vec<RelPath> = paths
        .iter()
        .filter(|path| TS_EXTENSIONS.contains(&extension_of(path)) && nearest_directory(path, &npm_dirs) == Some(directory))
        .cloned()
        .collect();
      if !files.is_empty() {
        runs.push(OracleRun {
          repo: repo.name.clone(),
          root: directory.clone(),
          ecosystem: Ecosystem::Npm,
          files,
        });
      }
    }
  }
  runs
}

fn relative_to_root(path: &RelPath, root: &Path) -> Option<RelPath> {
  match root.as_os_str().is_empty() {
    true => Some(path.clone()),
    false => RelPath::new(Path::new(path.as_str()).strip_prefix(root).ok()?.to_str()?).ok(),
  }
}

fn keys_by_symbol(snapshot: &Snapshot, repo: &RepoName, path: &RelPath) -> HashMap<(Span, String, String), VecDeque<SymbolKey>> {
  let mut keys: HashMap<(Span, String, String), VecDeque<SymbolKey>> = HashMap::new();
  for entity in snapshot.entities.values() {
    let (EntityKey::Symbol(key), Some(span)) = (&entity.key, entity.span) else { continue };
    if entity.repo.as_ref() != Some(repo) || entity.path.as_ref() != Some(path) {
      continue;
    }
    keys.entry((span, entity.name.clone(), entity.kind.to_string())).or_default().push_back(key.clone());
  }
  keys
}

fn file_context(ctx: &RunContext<'_>, repo: &RepoName, path: &RelPath, repo_root: &Path) -> Option<ScipFileContext> {
  let blob = ctx.engine.store().read(|tx| tx.blob(&format!("artifact:{repo}:{path}"))).ok()??;
  let artifact: serde_json::Value = serde_json::from_slice(&blob.bytes).ok()?;
  let facts: FileFacts = serde_json::from_value(artifact.get("facts")?.clone()).ok()?;
  let source = fs::read_to_string(repo_root.join(path.as_str())).ok()?;
  let mut available = keys_by_symbol(ctx.snapshot, repo, path);
  let mut keys = Vec::with_capacity(facts.symbols.len());
  for symbol in &facts.symbols {
    keys.push(available.get_mut(&(symbol.span, symbol.name.clone(), symbol.kind.to_string()))?.pop_front()?);
  }
  Some(ScipFileContext::from_facts(&facts, keys, source))
}

pub fn execute_run(run: &OracleRun, ctx: &RunContext<'_>) -> QualityResult<RunOutput> {
  let line = |outcome: String| PackageLine {
    repo: run.repo.to_string(),
    root: run.root.display().to_string(),
    indexer: run.ecosystem.indexer_name().to_owned(),
    files: run.files.len(),
    outcome,
  };
  let skipped = |outcome: String| RunOutput {
    line: line(outcome),
    digest: None,
    per_file: BTreeMap::new(),
  };
  let indexer = match run.ecosystem {
    Ecosystem::Cargo => ctx.cargo,
    Ecosystem::Npm => ctx.npm,
  };
  let Some(indexer) = indexer else {
    return Ok(skipped("no indexer is available for this ecosystem".to_owned()));
  };
  let Some(config) = ctx.project.repos.iter().find(|candidate| candidate.name == run.repo) else {
    return Ok(skipped("the repo is not part of the project".to_owned()));
  };
  let inputs: Vec<ScipInputFile> = run
    .files
    .iter()
    .filter_map(|path| {
      Some(ScipInputFile {
        path: relative_to_root(path, &run.root)?,
        content_hash: ctx.snapshot.files.get(&(run.repo.clone(), path.clone()))?.content_hash,
      })
    })
    .collect();
  let request = ScipRequest {
    root: config.path.join(&run.root),
    cache_dir: ctx.cache_dir.to_owned(),
    files: inputs,
  };
  let scip_run = indexer.index(&request);
  let path = match (&scip_run.outcome, &scip_run.output_path) {
    (ScipOutcome::Ok, Some(path)) => path.clone(),
    (ScipOutcome::Failed { reason, .. }, _) => return Ok(skipped(format!("indexer failed: {reason}"))),
    (ScipOutcome::TimedOut, _) => return Ok(skipped("indexer timed out".to_owned())),
    (ScipOutcome::Unavailable { reason }, _) => return Ok(skipped(format!("indexer unavailable: {reason}"))),
    (ScipOutcome::Ok, None) => return Ok(skipped("indexer reported success without an index".to_owned())),
  };
  let bytes = fs::read(&path).map_err(|error| QualityError::io(&path, error))?;
  let digest = blake3::hash(&bytes).to_hex().to_string();
  let mut context = ScipIngestContext::new(run.repo.clone());
  if !run.root.as_os_str().is_empty() {
    let package_dir = RelPath::new(&run.root.display().to_string()).map_err(|error| QualityError::scenario(ctx.target, error.to_string()))?;
    context = context.in_package_dir(package_dir);
  }
  let mut unmatched = 0usize;
  for path in &run.files {
    match file_context(ctx, &run.repo, path, &config.path) {
      Some(file) => context = context.with_file(path.clone(), file),
      None => unmatched += 1,
    }
  }
  let facts = ingest(&bytes, &context).map_err(|error| QualityError::scenario(ctx.target, error.to_string()))?;
  let per_file = facts.per_file;
  Ok(RunOutput {
    line: line(format!(
      "indexed {} files, {} edges ingested, {unmatched} files without usable facts",
      facts.stats.documents,
      per_file.values().map(Vec::len).sum::<usize>()
    )),
    digest: Some(IndexDigest {
      target: ctx.target.to_owned(),
      repo: run.repo.to_string(),
      root: run.root.display().to_string(),
      indexer: run.ecosystem.indexer_name().to_owned(),
      blake3: digest,
    }),
    per_file,
  })
}
