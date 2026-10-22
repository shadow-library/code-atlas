use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use atlas_core::{Entity, EntityId, EntityKey, ParseStatus, RefKind, RepoName, UnresolvedReason, UnresolvedRef};
use atlas_engine::{Engine, EngineError};
use serde::Serialize;

use crate::error::QueryResult;
use crate::freshness::Freshness;
use crate::graph::Graph;
use crate::model::{Caveat, Location, MacroGapScope, ProblemFile, Targeted, UnresolvedSite, short_name};
use crate::policy::Policy;

const RISK_SITES_SHOWN: usize = 5;
const PROBLEM_FILES_SHOWN: usize = 5;
const EXCERPT_LINES: usize = 8;

pub struct QueryEngine {
  engine: Arc<Engine>,
  barrier: Barrier,
  problems: Mutex<Option<CachedProblems>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Barrier {
  Enforced,
  Skipped,
}

struct CachedProblems {
  run: Option<i64>,
  files: Arc<Vec<ProblemFile>>,
}

impl QueryEngine {
  pub fn new(engine: Arc<Engine>) -> Self {
    Self {
      engine,
      barrier: Barrier::Enforced,
      problems: Mutex::new(None),
    }
  }

  #[doc(hidden)]
  pub fn without_barrier(engine: Arc<Engine>) -> Self {
    Self {
      engine,
      barrier: Barrier::Skipped,
      problems: Mutex::new(None),
    }
  }

  pub fn engine(&self) -> &Arc<Engine> {
    &self.engine
  }

  pub(crate) fn barrier(&self) -> QueryResult<Freshness> {
    let report = match self.barrier {
      Barrier::Enforced => self.engine.ensure_fresh()?,
      Barrier::Skipped => self.engine.skip_barrier(),
    };
    Ok(Freshness::from_report(&report))
  }

  pub(crate) fn read<T>(&self, policy: Policy, query: impl FnOnce(&Graph<'_, '_>) -> QueryResult<T>) -> QueryResult<T> {
    self.engine.store().read(|tx| query(&Graph::new(tx, policy)))
  }

  pub(crate) fn repos(&self) -> Vec<RepoName> {
    self.engine.project().repos.iter().map(|repo| repo.name.clone()).collect()
  }

  pub(crate) fn sources<'q>(&'q self, freshness: &'q mut Freshness) -> SourceReader<'q> {
    SourceReader {
      query: self,
      freshness,
      resynced: false,
    }
  }

  pub(crate) fn problem_files(&self, run: Option<i64>) -> QueryResult<Option<Caveat>> {
    let files = self.cached_problems(run)?;
    if files.is_empty() {
      return Ok(None);
    }
    Ok(Some(Caveat::ProblemFiles {
      total: files.len(),
      files: files.iter().take(PROBLEM_FILES_SHOWN).cloned().collect(),
    }))
  }

  pub(crate) fn with_problem_files<T>(&self, answer: Targeted<T>, run: Option<i64>, caveats: impl FnOnce(&mut T) -> &mut Vec<Caveat>) -> QueryResult<Targeted<T>> {
    let Targeted::Found(mut found) = answer else { return Ok(answer) };
    caveats(&mut found).extend(self.problem_files(run)?);
    Ok(Targeted::Found(found))
  }

  fn cached_problems(&self, run: Option<i64>) -> QueryResult<Arc<Vec<ProblemFile>>> {
    let mut cache = self.problems.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(cached) = cache.as_ref().filter(|cached| cached.run == run) {
      return Ok(Arc::clone(&cached.files));
    }
    let repos = self.repos();
    let files = self.engine.store().read(|tx| -> QueryResult<Vec<ProblemFile>> {
      let mut problems = Vec::new();
      for repo in &repos {
        for record in tx.files_in_repo(repo)? {
          let status = match &record.parse_status {
            ParseStatus::Ok => continue,
            ParseStatus::Partial { error_count, .. } => format!("partial parse ({error_count} errors)"),
            ParseStatus::Failed { reason } => format!("failed: {reason}"),
          };
          problems.push(ProblemFile {
            location: Location {
              repo: record.repo.to_string(),
              path: record.path.to_string(),
              line: None,
              column: None,
            },
            status,
          });
        }
      }
      Ok(problems)
    })?;
    let files = Arc::new(files);
    *cache = Some(CachedProblems { run, files: Arc::clone(&files) });
    Ok(files)
  }
}

pub(crate) fn unresolved_risk(graph: &Graph<'_, '_>, target: EntityId, name: &str) -> QueryResult<Option<Caveat>> {
  if name.is_empty() {
    return Ok(None);
  }
  let mut by_reason: BTreeMap<String, usize> = BTreeMap::new();
  let mut sites = Vec::new();
  for found in graph.tx().unresolved_by_names(&[name])? {
    let reference = found.reference;
    let Some(reason) = risk_reason(&reference, target) else { continue };
    *by_reason.entry(reason.to_owned()).or_default() += 1;
    sites.push(UnresolvedSite {
      location: Location {
        repo: reference.repo.to_string(),
        path: reference.file.to_string(),
        line: Some(reference.span.start_line),
        column: None,
      },
      path: reference.path.join("."),
      reason: reason.to_owned(),
    });
  }
  if sites.is_empty() {
    return Ok(None);
  }
  sites.sort_by(|left, right| left.location.cmp(&right.location));
  let total = sites.len();
  sites.truncate(RISK_SITES_SHOWN);
  Ok(Some(Caveat::UnresolvedRisk {
    name: name.to_owned(),
    total,
    by_reason,
    sites,
  }))
}

pub(crate) fn risk_reason(reference: &UnresolvedRef, target: EntityId) -> Option<&'static str> {
  match &reference.reason {
    UnresolvedReason::Dynamic => Some("dynamic"),
    UnresolvedReason::Ambiguous { candidates } if candidates.is_empty() || candidates.contains(&target) => Some("ambiguous"),
    UnresolvedReason::NotFound if matches!(reference.kind, RefKind::MethodCall) => Some("unknown receiver"),
    UnresolvedReason::Unsupported { detail } if detail == "macro_generated" => Some("macro generated"),
    _ => None,
  }
}

pub(crate) fn macro_generated_refs(graph: &Graph<'_, '_>, target: &Entity, owners: &[EntityId], scope: MacroGapScope) -> QueryResult<Option<Caveat>> {
  let (repo, path) = match (&target.key, &target.repo, &target.path) {
    (EntityKey::File { repo, path }, _, _) => (repo, path),
    (_, Some(repo), Some(path)) => (repo, path),
    _ => return Ok(None),
  };
  let mut sites: Vec<UnresolvedSite> = graph
    .tx()
    .unresolved_in_file_with_name_prefix(repo, path, "")?
    .into_iter()
    .filter(|reference| owners.contains(&reference.from) && matches!(&reference.reason, UnresolvedReason::Unsupported { detail } if detail == "macro_generated"))
    .map(|reference| UnresolvedSite {
      location: Location {
        repo: reference.repo.to_string(),
        path: reference.file.to_string(),
        line: Some(reference.span.start_line),
        column: None,
      },
      path: reference.path.join("."),
      reason: "macro generated".to_owned(),
    })
    .collect();
  if sites.is_empty() {
    return Ok(None);
  }
  sites.sort_by(|left, right| left.location.cmp(&right.location).then_with(|| left.path.cmp(&right.path)));
  let total = sites.len();
  sites.truncate(RISK_SITES_SHOWN);
  Ok(Some(Caveat::MacroGeneratedRefs {
    target: short_name(&target.key),
    scope,
    total,
    sites,
  }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnippetMode {
  Body { max_lines: Option<usize> },
  Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Snippet {
  Text { start_line: u32, lines: Vec<String>, omitted_lines: usize },
  Mismatch { detail: String },
  Unavailable { reason: String },
}

pub(crate) struct SourceReader<'q> {
  query: &'q QueryEngine,
  freshness: &'q mut Freshness,
  resynced: bool,
}

struct Located {
  repo: RepoName,
  path: atlas_core::RelPath,
  start_line: u32,
  end_line: u32,
}

impl SourceReader<'_> {
  pub fn snippet(&mut self, entity: EntityId, mode: SnippetMode, context: u32) -> QueryResult<Snippet> {
    loop {
      let Some(located) = self.locate(entity)? else {
        return Ok(Snippet::Unavailable {
          reason: "the symbol has no indexed source span".to_owned(),
        });
      };
      match self.query.engine.read_source(&located.repo, &located.path) {
        Ok((text, _)) => return Ok(extract(&text, &located, mode, context)),
        Err(EngineError::SourceChanged { repo, path }) if !self.resynced => {
          tracing::info!(repo = %repo, path = %path, "source changed since indexing; re-running the freshness barrier");
          self.resynced = true;
          let later = self.query.barrier()?;
          self.freshness.absorb_resync(later);
        }
        Err(EngineError::SourceChanged { repo, path }) => {
          return Ok(Snippet::Mismatch {
            detail: format!("{repo}:{path} still differs from the indexed content after re-indexing; not showing a stale snippet"),
          });
        }
        Err(error) => return Ok(Snippet::Unavailable { reason: error.to_string() }),
      }
    }
  }

  pub fn excerpt(&mut self, repo: &RepoName, path: &atlas_core::RelPath, start_line: u32, end_line: u32, expected: atlas_core::ContentHash) -> QueryResult<Snippet> {
    let located = Located {
      repo: repo.clone(),
      path: path.clone(),
      start_line,
      end_line,
    };
    loop {
      match self.query.engine.read_source(repo, path) {
        Ok((_, hash)) if hash != expected => {
          return Ok(Snippet::Mismatch {
            detail: format!("{repo}:{path} changed since this evidence was recorded; not showing a stale excerpt"),
          });
        }
        Ok((text, _)) => return Ok(extract(&text, &located, SnippetMode::Body { max_lines: Some(EXCERPT_LINES) }, 0)),
        Err(EngineError::SourceChanged { .. }) if !self.resynced => {
          self.resynced = true;
          let later = self.query.barrier()?;
          self.freshness.absorb_resync(later);
        }
        Err(EngineError::SourceChanged { repo, path }) => {
          return Ok(Snippet::Mismatch {
            detail: format!("{repo}:{path} still differs from the indexed content after re-indexing"),
          });
        }
        Err(error) => return Ok(Snippet::Unavailable { reason: error.to_string() }),
      }
    }
  }

  fn locate(&self, entity: EntityId) -> QueryResult<Option<Located>> {
    let entity = self.query.engine.store().read(|tx| tx.entity(entity))?;
    Ok(entity.and_then(|entity| {
      let span = entity.span?;
      Some(Located {
        repo: entity.repo?,
        path: entity.path?,
        start_line: span.start_line,
        end_line: span.end_line,
      })
    }))
  }
}

fn extract(text: &str, located: &Located, mode: SnippetMode, context: u32) -> Snippet {
  let lines: Vec<&str> = text.lines().collect();
  let first = located.start_line.saturating_sub(1).saturating_sub(context) as usize;
  let last = match mode {
    SnippetMode::Signature => located.start_line,
    SnippetMode::Body { .. } => located.end_line,
  };
  let last = (last.saturating_add(context) as usize).min(lines.len());
  if first >= last {
    return Snippet::Unavailable {
      reason: format!("span lines {}-{} are outside the file", located.start_line, located.end_line),
    };
  }
  let mut selected: Vec<String> = lines[first..last].iter().map(|line| (*line).to_owned()).collect();
  let mut omitted_lines = 0;
  if let SnippetMode::Body { max_lines: Some(max) } = mode
    && selected.len() > max
  {
    omitted_lines = selected.len() - max;
    selected.truncate(max);
  }
  Snippet::Text {
    start_line: u32::try_from(first + 1).unwrap_or(u32::MAX),
    lines: selected,
    omitted_lines,
  }
}
