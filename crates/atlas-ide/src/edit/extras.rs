use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use atlas_core::{ContentHash, Entity, RelPath, RepoName};
use atlas_edit::{EditPlan, EditScope, FileEdits, FileTarget, TextEdit};
use atlas_query::{GraphReference, GraphReferences, ReferenceSide, Targeted};
use atlas_store::EntityFilter;
use serde::{Deserialize, Serialize};

use super::merge_edits;
use crate::error::IdeResult;
use crate::references::{ExtraReason, GraphExtra};
use crate::service::IdeService;
use crate::target::{ProjectFiles, offset_of, read_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyReason {
  LanguageServer,
  OutsideLanguageServer,
  Mock,
  Heuristic,
  Http,
  Dynamic,
  TokenMismatch,
}

impl VerifyReason {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::LanguageServer => "reference",
      Self::OutsideLanguageServer => "outside the language server",
      Self::Mock => "mock",
      Self::Heuristic => "heuristic",
      Self::Http => "http",
      Self::Dynamic => "dynamic",
      Self::TokenMismatch => "token mismatch",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifySite {
  pub reason: VerifyReason,
  pub repo: String,
  pub path: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub line: Option<u32>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub column: Option<u32>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub from: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub edge_kind: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub certainty: Option<String>,
  pub authoritative: bool,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub snippet: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub note: Option<String>,
}

impl VerifySite {
  pub fn place(&self) -> String {
    match (self.line, self.column) {
      (Some(line), Some(column)) => format!("{}:{}:{line}:{column}", self.repo, self.path),
      (Some(line), None) => format!("{}:{}:{line}", self.repo, self.path),
      (None, _) => format!("{}:{}", self.repo, self.path),
    }
  }

  pub(crate) fn sort_key(&self) -> (String, String, Option<u32>, Option<u32>, VerifyReason) {
    self.key()
  }

  fn key(&self) -> (String, String, Option<u32>, Option<u32>, VerifyReason) {
    (self.repo.clone(), self.path.clone(), self.line, self.column, self.reason)
  }

  pub(crate) fn with_note(mut self, note: impl Into<String>) -> Self {
    self.note = Some(note.into());
    self
  }
}

impl fmt::Display for VerifySite {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    let mut line = self.reason.as_str().to_owned();
    if let Some(certainty) = &self.certainty {
      line = format!("{line} {certainty}");
    }
    line = format!("{line}  {}", self.place());
    if let Some(from) = &self.from {
      line = format!("{line}  {from}");
    }
    if let Some(snippet) = &self.snippet {
      line = format!("{line}  {snippet}");
    }
    if let Some(note) = &self.note {
      line = format!("{line}  ({note})");
    }
    formatter.write_str(&line)
  }
}

pub(crate) const MAX_CANDIDATE_FILES: usize = 200;

pub(crate) struct Candidates {
  pub open: Vec<PathBuf>,
  pub caveats: Vec<String>,
  pub unloaded: Vec<String>,
}

impl IdeService {
  pub(crate) fn exact_candidates(&self, files: &ProjectFiles, sites: &[VerifySite], anchor: &crate::target::FileRef, admits: impl Fn(&Path) -> bool) -> IdeResult<Candidates> {
    let mut candidates = candidate_files(files, sites, &anchor.absolute, &admits);
    let typescript = crate::servers::kind_of(&anchor.absolute) == Some(atlas_lsp::ServerKind::TypeScript);
    if let (true, Some(root)) = (typescript, files.root(&anchor.repo)) {
      let projects = self.project_representatives(anchor, root, &admits)?;
      for path in projects.open {
        if !candidates.open.contains(&path) {
          candidates.open.push(path);
        }
      }
      candidates.caveats.extend(projects.caveats);
      candidates.unloaded = projects.unloaded;
    }
    Ok(candidates)
  }
}

fn candidate_files(files: &ProjectFiles, sites: &[VerifySite], anchor: &Path, admits: impl Fn(&Path) -> bool) -> Candidates {
  let mut paths: Vec<PathBuf> = sites
    .iter()
    .filter(|site| site.reason != VerifyReason::Http)
    .filter_map(|site| {
      let repo = RepoName::new(&site.repo).ok()?;
      let path = files.root(&repo)?.join(&site.path);
      let path = atlas_lsp::canonical_path(&path).ok()?;
      (path != anchor && path.is_file() && admits(&path)).then_some(path)
    })
    .collect();
  paths.sort();
  paths.dedup();
  let total = paths.len();
  paths.truncate(MAX_CANDIDATE_FILES.saturating_sub(1));
  let caveats = (total > paths.len())
    .then(|| {
      format!(
        "the index lists {total} files that use the target; only {} were opened in the language server, so projects of the others may not be loaded",
        paths.len()
      )
    })
    .into_iter()
    .collect();
  let mut open = vec![anchor.to_path_buf()];
  open.extend(paths);
  Candidates {
    open,
    caveats,
    unloaded: Vec::new(),
  }
}

pub(crate) struct GraphSites {
  pub sites: Vec<VerifySite>,
  pub caveats: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
  Family,
  TargetOnly,
}

impl IdeService {
  pub(crate) fn overloads(&self, target: &Entity) -> IdeResult<Vec<Entity>> {
    let (Some(repo), Some(path), Some(symbol)) = (&target.repo, &target.path, target.key.as_symbol()) else {
      return Ok(Vec::new());
    };
    let base = symbol.clone().with_disambiguator(0);
    let mut siblings: Vec<Entity> = self
      .query
      .engine()
      .store()
      .read(|tx| tx.entities_in_file(repo, path, &EntityFilter::default()))?
      .into_iter()
      .filter(|other| other.id != target.id && other.key.as_symbol().is_some_and(|key| key.clone().with_disambiguator(0) == base))
      .collect();
    siblings.sort_by_key(|other| other.span.map_or(0, |span| span.start_byte));
    Ok(siblings)
  }

  pub(crate) fn graph_sites(&self, target: &Entity, reach: Reach) -> IdeResult<GraphSites> {
    let target_repo = target.repo.clone();
    let mut members = vec![target.clone()];
    members.extend(self.overloads(target)?);
    if reach == Reach::Family {
      let mut keys = BTreeSet::new();
      for member in &members {
        keys.extend(
          self
            .query
            .graph_references(member)?
            .references
            .iter()
            .filter(|row| row.side == ReferenceSide::Implemented)
            .map(|row| row.other.key.clone()),
        );
      }
      for key in keys {
        if let Targeted::Found(member) = self.query.resolve_target(&key)?
          && members.iter().all(|known| known.id != member.id)
        {
          members.push(member);
        }
      }
    }
    let mut sites = Vec::new();
    let mut caveats = Vec::new();
    let mut unverified = BTreeSet::new();
    let mut unlocated = 0;
    for member in &members {
      let graph = self.query.graph_references(member)?;
      collect_rows(&graph, target_repo.as_ref(), &mut sites, &mut unlocated);
      unverified.extend(graph.unverified_files.iter().map(ToString::to_string));
      sites.extend(self.http_consumers(member)?.into_iter().map(http_site));
    }
    if unlocated > 0 {
      caveats.push(format!("{unlocated} graph usage(s) have no source location and cannot be checked"));
    }
    if !unverified.is_empty() {
      caveats.push(format!(
        "graph sites in {} were not checked: the files changed since indexing",
        unverified.into_iter().collect::<Vec<_>>().join(", ")
      ));
    }
    let mut seen = BTreeSet::new();
    sites.retain(|site| seen.insert(site.key()));
    sites.sort_by_key(VerifySite::key);
    Ok(GraphSites { sites, caveats })
  }
}

fn collect_rows(graph: &GraphReferences, target_repo: Option<&RepoName>, sites: &mut Vec<VerifySite>, unlocated: &mut usize) {
  for row in graph.references.iter().filter(|row| row.side != ReferenceSide::Heritage) {
    let Some(location) = &row.location else {
      *unlocated += 1;
      continue;
    };
    let reason = classify(row, &location.repo, location.column, target_repo);
    sites.push(VerifySite {
      reason,
      repo: location.repo.clone(),
      path: location.path.clone(),
      line: location.line,
      column: location.column,
      from: Some(row.other.tagged_short()),
      edge_kind: Some(row.edge_kind.clone()),
      certainty: Some(row.certainty.tag.clone()),
      authoritative: row.certainty.authoritative,
      snippet: row.snippet.clone(),
      note: row.mock.as_ref().map(|mock| format!("mock: {mock}")),
    });
  }
  for unresolved in &graph.unresolved {
    sites.push(VerifySite {
      reason: VerifyReason::Dynamic,
      repo: unresolved.location.repo.clone(),
      path: unresolved.location.path.clone(),
      line: unresolved.location.line,
      column: unresolved.location.column,
      from: None,
      edge_kind: None,
      certainty: Some("[unresolved]".to_owned()),
      authoritative: false,
      snippet: None,
      note: Some(format!("{} ({})", unresolved.path, unresolved.reason)),
    });
  }
}

fn classify(row: &GraphReference, site_repo: &str, column: Option<u32>, target_repo: Option<&RepoName>) -> VerifyReason {
  if row.mock.is_some() {
    return VerifyReason::Mock;
  }
  if column.is_none() {
    return VerifyReason::TokenMismatch;
  }
  if target_repo.is_none_or(|repo| repo.as_str() != site_repo) || row.certainty.authoritative {
    return VerifyReason::OutsideLanguageServer;
  }
  VerifyReason::Heuristic
}

fn http_site(extra: GraphExtra) -> VerifySite {
  let endpoint = match &extra.reason {
    ExtraReason::Http { endpoint } => Some(format!("calls {endpoint}")),
    _ => None,
  };
  let location = extra.location.unwrap_or_else(|| atlas_query::Location {
    repo: String::new(),
    path: String::new(),
    line: None,
    column: None,
  });
  VerifySite {
    reason: VerifyReason::Http,
    repo: location.repo,
    path: location.path,
    line: location.line,
    column: location.column,
    from: extra.from.as_ref().map(atlas_query::EntityRef::tagged_short),
    edge_kind: extra.edge_kind,
    certainty: extra.certainty.map(|certainty| certainty.tag),
    authoritative: false,
    snippet: extra.snippet,
    note: endpoint,
  }
}

struct Covered<'a> {
  text: &'a str,
  ranges: Vec<(usize, usize)>,
}

pub(crate) struct Coverage<'a> {
  files: BTreeMap<(String, String), Covered<'a>>,
}

impl<'a> Coverage<'a> {
  pub(crate) fn of_plan(plan: &EditPlan, originals: &'a BTreeMap<FileTarget, String>) -> Self {
    let mut files = BTreeMap::new();
    for file in &plan.files {
      let Some(text) = originals.get(&file.file) else { continue };
      let ranges = file.edits.iter().map(|edit| (edit.start, edit.end)).collect();
      files.insert((file.file.repo.to_string(), file.file.path.to_string()), Covered { text: text.as_str(), ranges });
    }
    Self { files }
  }

  pub(crate) fn covers(&self, site: &VerifySite) -> bool {
    let Some(Covered { text, ranges }) = self.files.get(&(site.repo.clone(), site.path.clone())) else {
      return false;
    };
    let Some(line) = site.line else { return false };
    match site.column {
      Some(column) => match offset_of(text, line, column) {
        Ok(offset) => ranges.iter().any(|(start, end)| *start <= offset && (offset < *end || (start == end && offset == *start))),
        Err(_) => false,
      },
      None => ranges.iter().any(|(start, _)| line_of(text, *start) == line),
    }
  }
}

fn line_of(text: &str, offset: usize) -> u32 {
  u32::try_from(text[..offset.min(text.len())].matches('\n').count() + 1).unwrap_or(u32::MAX)
}

pub(crate) fn is_identifier_char(character: char) -> bool {
  character.is_alphanumeric() || character == '_' || character == '$'
}

pub(crate) fn reads_token(text: &str, offset: usize, name: &str) -> bool {
  let Some(rest) = text.get(offset..) else { return false };
  if !rest.starts_with(name) {
    return false;
  }
  let before = text[..offset].chars().next_back();
  let after = rest[name.len()..].chars().next();
  !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char)
}

fn token_offsets(text: &str, name: &str) -> Vec<usize> {
  text
    .match_indices(name)
    .map(|(offset, _)| offset)
    .filter(|offset| reads_token(text, *offset, name))
    .collect()
}

fn neighbours(text: &str, offset: usize, length: usize) -> (Option<char>, Option<char>) {
  let before = text[..offset].chars().rev().find(|character| !character.is_whitespace());
  let after = text[offset + length..].chars().find(|character| !character.is_whitespace());
  (before, after)
}

pub(crate) struct GraphRename<'a> {
  pub files: &'a ProjectFiles,
  pub scope: &'a EditScope,
  pub old_name: &'a str,
  pub new_name: &'a str,
}

struct GraphEdits {
  target: FileTarget,
  text: String,
  edits: Vec<TextEdit>,
}

pub(crate) struct GraphApplied {
  pub applied: Vec<VerifySite>,
  pub remaining: Vec<VerifySite>,
}

impl IdeService {
  pub(crate) fn apply_graph_sites(
    &self,
    rename: &GraphRename<'_>,
    plan: &mut EditPlan,
    originals: &mut BTreeMap<FileTarget, String>,
    sites: Vec<VerifySite>,
  ) -> IdeResult<GraphApplied> {
    let (candidates, mut remaining): (Vec<VerifySite>, Vec<VerifySite>) = sites
      .into_iter()
      .partition(|site| site.reason == VerifyReason::OutsideLanguageServer && site.authoritative && site.column.is_some() && site.line.is_some());
    let mut by_file: BTreeMap<(String, String), Vec<VerifySite>> = BTreeMap::new();
    for site in candidates {
      by_file.entry((site.repo.clone(), site.path.clone())).or_default().push(site);
    }
    let mut applied = Vec::new();
    for ((repo, path), sites) in by_file {
      match self.graph_file_edits(rename, plan, originals, &repo, &path, &sites)? {
        Ok(GraphEdits { target, text, edits }) => {
          originals.entry(target.clone()).or_insert(text.clone());
          let expected = plan
            .files
            .iter()
            .find(|file| file.file == target)
            .map_or_else(|| ContentHash::of(text.as_bytes()), |file| file.expected);
          merge_edits(plan, FileEdits::new(target, expected, edits));
          applied.extend(sites.into_iter().map(|site| site.with_note("applied from the graph")));
        }
        Err(note) => remaining.extend(sites.into_iter().map(|site| site.with_note(note.clone()))),
      }
    }
    for site in remaining
      .iter_mut()
      .filter(|site| site.reason == VerifyReason::OutsideLanguageServer && !site.authoritative && site.note.is_none())
    {
      site.note = Some("not applied: the graph link is not authoritative".to_owned());
    }
    remaining.sort_by_key(VerifySite::key);
    Ok(GraphApplied { applied, remaining })
  }

  fn graph_file_edits(
    &self,
    rename: &GraphRename<'_>,
    plan: &EditPlan,
    originals: &BTreeMap<FileTarget, String>,
    repo: &str,
    path: &str,
    sites: &[VerifySite],
  ) -> IdeResult<Result<GraphEdits, String>> {
    let (Ok(repo), Ok(path)) = (RepoName::new(repo), RelPath::new(path)) else {
      return Ok(Err("not a file of a registered repo".to_owned()));
    };
    let target = FileTarget::new(repo.clone(), path.clone());
    if let Err(refusal) = rename.scope.locate(&target) {
      return Ok(Err(format!("not applied: {} is {}", target, refusal.reason)));
    }
    let Some(root) = rename.files.root(&repo) else {
      return Ok(Err("not applied: unknown repo".to_owned()));
    };
    let text = match originals.get(&target) {
      Some(text) => text.clone(),
      None => {
        let Ok((text, hash)) = read_text(&root.join(path.as_str())) else {
          return Ok(Err("not applied: the file cannot be read as UTF-8".to_owned()));
        };
        let indexed = self.query.engine().store().read(|tx| tx.file(&repo, &path))?.map(|record| record.content_hash);
        if indexed != Some(hash) {
          return Ok(Err("not applied: the file changed since indexing".to_owned()));
        }
        text
      }
    };
    let server_ranges: Vec<(usize, usize)> = plan
      .files
      .iter()
      .filter(|file| file.file == target)
      .flat_map(|file| file.edits.iter().map(|edit| (edit.start, edit.end)))
      .collect();
    let mut edits = Vec::new();
    for site in sites {
      let (Some(line), Some(column)) = (site.line, site.column) else { continue };
      let Ok(offset) = offset_of(&text, line, column) else {
        return Ok(Err("not applied: the graph position is not in the file".to_owned()));
      };
      if !reads_token(&text, offset, rename.old_name) {
        return Ok(Err(format!("not applied: the name at {} does not read {}", site.place(), rename.old_name)));
      }
      if site.edge_kind.as_deref() == Some("re_exports") {
        return Ok(Err(
          "not applied: a re-export; renaming it changes what this module exports, so rename it by hand".to_owned(),
        ));
      }
      let (before, after) = neighbours(&text, offset, rename.old_name.len());
      let shorthand = matches!(before, Some('{' | ',')) && matches!(after, Some('}' | ',')) && site.edge_kind.as_deref() != Some("imports");
      let rest = text[offset + rename.old_name.len()..].trim_start();
      let property_key = rest.starts_with(':') && !rest.starts_with("::");
      if shorthand || property_key {
        return Ok(Err(format!(
          "not applied: {} is a property name or shorthand at {}; rename it by hand",
          rename.old_name,
          site.place()
        )));
      }
      edits.push(TextEdit::replace(offset..offset + rename.old_name.len(), rename.new_name));
    }
    let explained: BTreeSet<usize> = edits.iter().map(|edit| edit.start).chain(server_ranges.iter().map(|(start, _)| *start)).collect();
    let unexplained = token_offsets(&text, rename.old_name)
      .into_iter()
      .filter(|offset| !explained.contains(offset) && !server_ranges.iter().any(|(start, end)| start <= offset && offset < end))
      .count();
    if unexplained > 0 {
      return Ok(Err(format!(
        "not applied: the file has {unexplained} other occurrence(s) of {} that the graph does not explain; rename it by hand",
        rename.old_name
      )));
    }
    Ok(Ok(GraphEdits { target, text, edits }))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_read_a_name_token_only_on_identifier_boundaries() {
    let text = "import { toCents } from 'x';\nconst toCentsX = toCents(1) + mytoCents;\n";

    assert_eq!(token_offsets(text, "toCents"), vec![9, 46]);
    assert!(reads_token(text, 9, "toCents"));
    assert!(!reads_token(text, 35, "toCents"));
    assert_eq!(neighbours(text, 9, 7), (Some('{'), Some('}')));
  }
}
