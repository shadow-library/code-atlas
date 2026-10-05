use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use atlas_core::{ContentHash, RelPath, RepoName};
use atlas_project::{PathGlobs, WalkFilter};
use ignore::{DirEntry, WalkBuilder};
use regex::{Regex, RegexBuilder};

use crate::diff::{FileDiff, file_diff};
use crate::error::{EditError, EditResult, NotTextReason};
use crate::occurrence::{Occurrence, OccurrenceId, ensure_unique};
use crate::plan::{EditPlan, FileEdits, FileTarget, TextEdit};
use crate::scope::{EditScope, ScopeRepo, globs};

pub const UTF8_BOM: &str = "\u{feff}";
const BINARY_PROBE_BYTES: usize = 8 * 1024;

pub fn bom_len(content: &str) -> usize {
  if content.starts_with(UTF8_BOM) { UTF8_BOM.len() } else { 0 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
  Lf,
  CrLf,
}

impl LineEnding {
  pub fn dominant(content: &str) -> Self {
    let crlf = content.matches("\r\n").count();
    let lf = content.matches('\n').count() - crlf;
    if crlf > lf { Self::CrLf } else { Self::Lf }
  }

  pub fn as_str(self) -> &'static str {
    match self {
      Self::Lf => "\n",
      Self::CrLf => "\r\n",
    }
  }

  pub fn normalize(self, text: &str) -> String {
    let unix = text.replace("\r\n", "\n");
    match self {
      Self::Lf => unix,
      Self::CrLf => unix.replace('\n', "\r\n"),
    }
  }
}

pub(crate) struct LineIndex {
  starts: Vec<usize>,
}

impl LineIndex {
  pub(crate) fn new(content: &str) -> Self {
    let starts = std::iter::once(0).chain(content.match_indices('\n').map(|(offset, _)| offset + 1)).collect();
    Self { starts }
  }

  pub(crate) fn line_of(&self, offset: usize) -> usize {
    self.starts.partition_point(|start| *start <= offset).saturating_sub(1)
  }

  pub(crate) fn start(&self, line: usize) -> usize {
    self.starts[line]
  }

  pub(crate) fn line_count(&self) -> usize {
    self.starts.len()
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
  Literal(String),
  /// Multi-line, and `.` matches newlines (Serena-compatible); use `.*?` or `[^\n]*` for single-line matches.
  Regex(String),
}

enum Matcher {
  Literal(String),
  Regex(Regex),
}

impl Matcher {
  fn compile(pattern: &Pattern, template: &str) -> EditResult<Self> {
    match pattern {
      Pattern::Literal(text) | Pattern::Regex(text) if text.is_empty() => Err(EditError::EmptyPattern),
      Pattern::Literal(text) => Ok(Self::Literal(text.clone())),
      Pattern::Regex(source) => {
        let regex = RegexBuilder::new(source)
          .multi_line(true)
          .dot_matches_new_line(true)
          .crlf(true)
          .build()
          .map_err(|error| EditError::InvalidRegex {
            pattern: source.clone(),
            message: error.to_string(),
          })?;
        check_template(&regex, template)?;
        Ok(Self::Regex(regex))
      }
    }
  }

  fn replacements(&self, body: &str, line_ending: LineEnding, template: &str) -> Vec<(Range<usize>, String)> {
    match self {
      Self::Literal(needle) => {
        let needle = line_ending.normalize(needle);
        body
          .match_indices(needle.as_str())
          .map(|(start, found)| (start..start + found.len(), template.to_owned()))
          .collect()
      }
      Self::Regex(regex) => regex
        .captures_iter(body)
        .filter_map(|captures| {
          let whole = captures.get(0)?;
          let mut replacement = String::new();
          captures.expand(template, &mut replacement);
          Some((whole.range(), replacement))
        })
        .collect(),
    }
  }
}

fn check_template(regex: &Regex, template: &str) -> EditResult<()> {
  let bytes = template.as_bytes();
  let mut position = 0;
  while let Some(found) = template[position..].find('$') {
    let dollar = position + found;
    let rest = &template[dollar + 1..];
    let (reference, consumed) = if rest.starts_with('$') {
      (None, 1)
    } else if let Some(braced) = rest.strip_prefix('{') {
      match braced.find('}') {
        Some(close) => (Some(&braced[..close]), close + 2),
        None => (None, 0),
      }
    } else {
      let length = rest.bytes().take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_').count();
      ((length > 0).then(|| &rest[..length]), length)
    };
    if let Some(reference) = reference
      && !refers_to_group(regex, reference)
    {
      return Err(EditError::UnknownCaptureGroup { reference: reference.to_owned() });
    }
    position = (dollar + 1 + consumed).min(bytes.len());
  }
  Ok(())
}

fn refers_to_group(regex: &Regex, reference: &str) -> bool {
  match reference.parse::<usize>() {
    Ok(index) => index < regex.captures_len(),
    Err(_) => regex.capture_names().flatten().any(|name| name == reference),
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceContent {
  pub pattern: Pattern,
  pub text: String,
  pub all: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextPlan {
  pub plan: EditPlan,
  pub occurrences: Vec<Occurrence>,
  pub diffs: Vec<FileDiff>,
}

pub fn replace_content(scope: &EditScope, file: &FileTarget, request: &ReplaceContent) -> EditResult<TextPlan> {
  let target = scope.locate(file).map_err(|refusal| EditError::OutOfScope { refused: vec![refusal] })?;
  let bytes = fs::read(&target.absolute).map_err(|error| EditError::io("read", &target.absolute, error))?;
  let content = decode(&target.file, &bytes)?;
  replace_in_text(&target.file, content, request)
}

pub fn replace_in_text(file: &FileTarget, content: &str, request: &ReplaceContent) -> EditResult<TextPlan> {
  let matcher = Matcher::compile(&request.pattern, &request.text)?;
  let matches = find_matches(file, content, &matcher, &request.text)?;
  if matches.is_empty() {
    return Err(EditError::NoMatch { path: file.to_string() });
  }
  if matches.len() > 1 && !request.all {
    return Err(EditError::MultipleMatches {
      count: matches.len(),
      ids: matches.iter().map(|(occurrence, _)| occurrence.id.clone()).collect(),
    });
  }
  let mut plan = TextPlan::default();
  plan.push_file(file, content, matches)?;
  Ok(plan)
}

impl TextPlan {
  fn push_file(&mut self, file: &FileTarget, content: &str, matches: Vec<(Occurrence, TextEdit)>) -> EditResult<()> {
    if matches.is_empty() {
      return Ok(());
    }
    let (occurrences, edits): (Vec<_>, Vec<_>) = matches.into_iter().unzip();
    self.diffs.push(file_diff(file, content, &edits, &occurrences)?);
    self.plan.files.push(FileEdits::new(file.clone(), ContentHash::of(content.as_bytes()), edits));
    self.occurrences.extend(occurrences);
    Ok(())
  }
}

fn find_matches(file: &FileTarget, content: &str, matcher: &Matcher, text: &str) -> EditResult<Vec<(Occurrence, TextEdit)>> {
  let bom = bom_len(content);
  let line_ending = LineEnding::dominant(content);
  let template = line_ending.normalize(text);
  let found = matcher.replacements(&content[bom..], line_ending, &template);
  if found.is_empty() {
    return Ok(Vec::new());
  }
  let hash = ContentHash::of(content.as_bytes());
  let lines = LineIndex::new(content);
  found
    .into_iter()
    .map(|(range, replacement)| {
      let (start, end) = (range.start + bom, range.end + bom);
      if start == end {
        return Err(EditError::EmptyMatch {
          path: file.to_string(),
          offset: start,
        });
      }
      let line = lines.line_of(start);
      let occurrence = Occurrence {
        id: OccurrenceId::derive(file, &hash, start, end),
        file: file.clone(),
        start,
        end,
        line: line + 1,
        column: start - lines.start(line).max(bom) + 1,
      };
      Ok((occurrence, TextEdit::replace(start..end, replacement)))
    })
    .collect()
}

pub(crate) fn decode<'a>(file: &FileTarget, bytes: &'a [u8]) -> EditResult<&'a str> {
  if bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0) {
    return Err(EditError::NotText {
      path: file.to_string(),
      reason: NotTextReason::Binary,
    });
  }
  std::str::from_utf8(bytes).map_err(|_| EditError::NotText {
    path: file.to_string(),
    reason: NotTextReason::NotUtf8,
  })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceInFiles {
  pub pattern: Pattern,
  pub text: String,
  pub include: Vec<String>,
  pub exclude: Vec<String>,
  pub occurrences: Option<Vec<OccurrenceId>>,
  pub expected_count: Option<usize>,
  pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
  DryRun,
  ImplicitDryRun,
  Apply,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
  Binary,
  NotUtf8,
  TooLarge { size: u64, limit: u64 },
  Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedFile {
  pub file: FileTarget,
  pub reason: SkipReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesPlan {
  pub disposition: Disposition,
  pub total_matches: usize,
  pub text: TextPlan,
  pub skipped: Vec<SkippedFile>,
}

struct SearchedFile {
  file: FileTarget,
  content: String,
  matches: Vec<(Occurrence, TextEdit)>,
}

pub fn replace_in_files(scope: &EditScope, request: &ReplaceInFiles) -> EditResult<FilesPlan> {
  let matcher = Matcher::compile(&request.pattern, &request.text)?;
  let filters = UserFilters::new(scope, &request.include, &request.exclude)?;
  let mut searched = Vec::new();
  let mut skipped = Vec::new();
  for (index, repo) in scope.repos().iter().enumerate() {
    let Some(filter) = filters.for_repo(index) else { continue };
    let nested_roots = scope
      .repos()
      .iter()
      .filter(|other| other.root != repo.root && other.root.starts_with(&repo.root))
      .map(|other| other.root.clone())
      .collect();
    for (path, absolute) in walk(repo, filter, nested_roots) {
      let file = FileTarget::new(repo.name.clone(), path);
      match read_candidate(&file, &absolute, scope.max_file_bytes()) {
        Ok(content) => {
          let matches = find_matches(&file, &content, &matcher, &request.text)?;
          if !matches.is_empty() {
            searched.push(SearchedFile { file, content, matches });
          }
        }
        Err(reason) => skipped.push(SkippedFile { file, reason }),
      }
    }
  }
  let all: Vec<Occurrence> = searched.iter().flat_map(|file| file.matches.iter().map(|(occurrence, _)| occurrence.clone())).collect();
  ensure_unique(&all)?;
  let total_matches = all.len();
  if total_matches == 0 {
    return Err(EditError::NoMatch {
      path: "any searched file".to_owned(),
    });
  }
  if let Some(expected) = request.expected_count
    && expected != total_matches
  {
    return Err(EditError::ExpectedCountMismatch { expected, found: total_matches });
  }
  let selection = select(&all, request.occurrences.as_deref())?;
  let disposition = match (request.dry_run, &request.occurrences, request.expected_count) {
    (true, _, _) => Disposition::DryRun,
    (false, None, None) => Disposition::ImplicitDryRun,
    (false, _, _) => Disposition::Apply,
  };
  let mut text = TextPlan::default();
  for file in searched {
    let matches = file
      .matches
      .into_iter()
      .filter(|(occurrence, _)| selection.as_ref().is_none_or(|ids| ids.contains(&occurrence.id)))
      .collect();
    text.push_file(&file.file, &file.content, matches)?;
  }
  Ok(FilesPlan {
    disposition,
    total_matches,
    text,
    skipped,
  })
}

fn select(all: &[Occurrence], requested: Option<&[OccurrenceId]>) -> EditResult<Option<HashSet<OccurrenceId>>> {
  let Some(requested) = requested else { return Ok(None) };
  if requested.is_empty() {
    return Err(EditError::EmptySelection);
  }
  let known: HashSet<&OccurrenceId> = all.iter().map(|occurrence| &occurrence.id).collect();
  let unknown: BTreeSet<OccurrenceId> = requested.iter().filter(|id| !known.contains(id)).cloned().collect();
  if !unknown.is_empty() {
    return Err(EditError::UnknownOccurrences {
      ids: unknown.into_iter().collect(),
    });
  }
  Ok(Some(requested.iter().cloned().collect()))
}

fn read_candidate(file: &FileTarget, absolute: &Path, max_file_bytes: u64) -> Result<String, SkipReason> {
  let size = fs::metadata(absolute).map_err(|error| SkipReason::Unreadable(error.to_string()))?.len();
  if size > max_file_bytes {
    return Err(SkipReason::TooLarge { size, limit: max_file_bytes });
  }
  let bytes = fs::read(absolute).map_err(|error| SkipReason::Unreadable(error.to_string()))?;
  match decode(file, &bytes) {
    Ok(content) => Ok(content.to_owned()),
    Err(EditError::NotText {
      reason: NotTextReason::Binary, ..
    }) => Err(SkipReason::Binary),
    Err(_) => Err(SkipReason::NotUtf8),
  }
}

struct RepoFilter {
  include: PathGlobs,
  exclude: PathGlobs,
}

struct UserFilters {
  repos: Vec<Option<RepoFilter>>,
}

impl UserFilters {
  fn new(scope: &EditScope, include: &[String], exclude: &[String]) -> EditResult<Self> {
    let names: Vec<&RepoName> = scope.repos().iter().map(|repo| &repo.name).collect();
    let includes = partition(&names, include)?;
    let excludes = partition(&names, exclude)?;
    let restricted = !include.is_empty();
    let repos = includes
      .into_iter()
      .zip(excludes)
      .map(|(include, exclude)| {
        if restricted && include.is_empty() {
          return Ok(None);
        }
        Ok(Some(RepoFilter {
          include: globs(&include)?,
          exclude: globs(&exclude)?,
        }))
      })
      .collect::<EditResult<_>>()?;
    Ok(Self { repos })
  }

  fn for_repo(&self, index: usize) -> Option<&RepoFilter> {
    self.repos.get(index).and_then(Option::as_ref)
  }
}

fn partition(repos: &[&RepoName], patterns: &[String]) -> EditResult<Vec<Vec<String>>> {
  let mut per_repo = vec![Vec::new(); repos.len()];
  for pattern in patterns {
    let (negation, body) = pattern.strip_prefix('!').map_or(("", pattern.as_str()), |rest| ("!", rest));
    match body.split_once(':') {
      Some((prefix, glob)) if !glob.is_empty() && RepoName::new(prefix).is_ok() && !prefix.contains(['*', '?', '[', '{']) => {
        let index = repos
          .iter()
          .position(|repo| repo.as_str() == prefix)
          .ok_or_else(|| EditError::UnknownRepo(prefix.to_owned()))?;
        per_repo[index].push(format!("{negation}{glob}"));
      }
      _ => per_repo.iter_mut().for_each(|patterns| patterns.push(pattern.clone())),
    }
  }
  Ok(per_repo)
}

struct EntryFilter {
  root: PathBuf,
  repo: WalkFilter,
  user_exclude: PathGlobs,
  nested_roots: Vec<PathBuf>,
}

impl EntryFilter {
  fn admits(&self, entry: &DirEntry) -> bool {
    if !self.repo.admits(entry) || self.nested_roots.iter().any(|root| root == entry.path()) {
      return false;
    }
    let is_dir = entry.file_type().is_some_and(|file_type| file_type.is_dir());
    let Some(relative) = entry.path().strip_prefix(&self.root).ok().and_then(Path::to_str) else {
      return true;
    };
    !self.user_exclude.matches(relative, is_dir)
  }
}

fn walk(repo: &ScopeRepo, filter: &RepoFilter, nested_roots: Vec<PathBuf>) -> Vec<(RelPath, PathBuf)> {
  let entry_filter = EntryFilter {
    root: repo.root.clone(),
    repo: repo.filter.clone(),
    user_exclude: filter.exclude.clone(),
    nested_roots,
  };
  let walker = WalkBuilder::new(&repo.root)
    .hidden(true)
    .ignore(true)
    .git_ignore(true)
    .git_global(true)
    .git_exclude(true)
    .parents(false)
    .require_git(false)
    .follow_links(false)
    .sort_by_file_name(|left, right| left.cmp(right))
    .filter_entry(move |entry| entry_filter.admits(entry))
    .build();
  let mut files = Vec::new();
  for entry in walker {
    let entry = match entry {
      Ok(entry) => entry,
      Err(error) => {
        tracing::warn!(repo = %repo.name, %error, "skipping an unreadable entry while searching");
        continue;
      }
    };
    if !entry.file_type().is_some_and(|file_type| file_type.is_file()) {
      continue;
    }
    let Some(relative) = entry.path().strip_prefix(&repo.root).ok().and_then(Path::to_str) else {
      continue;
    };
    if relative.contains('\\') {
      continue;
    }
    if !repo.include.is_empty() && !repo.include.matches_within(relative, false) {
      continue;
    }
    if !filter.include.is_empty() && !filter.include.matches_within(relative, false) {
      continue;
    }
    let Ok(path) = RelPath::new(relative) else { continue };
    files.push((path, entry.into_path()));
  }
  files.sort();
  files
}
