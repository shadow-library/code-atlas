use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use atlas_core::{Entity, EntityId, RelPath, RepoName};
use atlas_project::{DocScope, is_note_path};
use serde::Serialize;

use crate::QueryEngine;
use crate::error::{QueryError, QueryResult};
use crate::freshness::FreshnessState;
use crate::graph::Graph;
use crate::model::{Caveat, CertaintyTag, EntityRef, Location, Response, Targeted};
use crate::policy::Policy;
use crate::queries::common::resolve_one;
use crate::target::TargetFilter;

pub(crate) use self::corpus::DocCache;
use self::corpus::{Corpus, LoadedDoc};
use self::markdown::{Origin, RawSection, new_frontmatter, parse, stamp_existing};
use self::rank::{SNIPPET_CHARS, SectionTerms, bm25, clip, query_terms, snippet};
use self::resolve::{Found, Resolver, Wanted, file_of, mention_certainty};

mod corpus;
pub(crate) mod markdown;
mod rank;
mod resolve;

pub const DEFAULT_DOCS_LIMIT: usize = 10;
const MENTIONS_SHOWN: usize = 3;
const CANDIDATES_SHOWN: usize = 3;
const POINTERS_SHOWN: usize = 3;
const EXCERPT_CHARS: usize = 400;
const FUSED_SNIPPET_CHARS: usize = 160;
const MAPPED_DOCS: usize = 20;
const MAPPED_HEADINGS: usize = 12;
const MAPPED_DEPTH: usize = 2;
const DOCS_DIR: &str = "docs/";
const README_STEM: &str = "readme";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocKind {
  Doc,
  Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorStatus {
  Current,
  Stale,
  Unstamped,
  Missing,
  Unreadable,
}

impl AnchorStatus {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Current => "current",
      Self::Stale => "stale",
      Self::Unstamped => "unstamped",
      Self::Missing => "missing",
      Self::Unreadable => "unreadable",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteState {
  Current,
  Stale,
  Unstamped,
  Unverified,
  Unanchored,
}

impl NoteState {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Current => "current",
      Self::Stale => "stale",
      Self::Unstamped => "unstamped",
      Self::Unverified => "unverified",
      Self::Unanchored => "unanchored",
    }
  }

  fn of(anchors: &[ResolvedAnchor]) -> Self {
    let any = |status: AnchorStatus| anchors.iter().any(|anchor| anchor.status == status);
    match () {
      _ if anchors.is_empty() => Self::Unanchored,
      _ if any(AnchorStatus::Stale) || any(AnchorStatus::Missing) => Self::Stale,
      _ if any(AnchorStatus::Unreadable) => Self::Unverified,
      _ if any(AnchorStatus::Unstamped) => Self::Unstamped,
      _ => Self::Current,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MentionView {
  Resolved {
    text: String,
    line: u32,
    certainty: CertaintyTag,
    candidates: Vec<EntityRef>,
    total: usize,
  },
  Drift {
    text: String,
    line: u32,
  },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DocSection {
  pub repo: String,
  pub path: String,
  pub absolute_path: String,
  pub heading: String,
  pub start_line: u32,
  pub end_line: u32,
  pub kind: DocKind,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub note_status: Option<NoteState>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub score: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub rank: Option<usize>,
  pub snippet: String,
  pub mentions: Vec<MentionView>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DocsRequest {
  pub query: Option<String>,
  pub target: Option<String>,
  pub notes_only: bool,
  pub limit: usize,
}

impl Default for DocsRequest {
  fn default() -> Self {
    Self {
      query: None,
      target: None,
      notes_only: false,
      limit: DEFAULT_DOCS_LIMIT,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DocsAnswer {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub query: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub about: Option<EntityRef>,
  pub notes_only: bool,
  pub total: usize,
  pub sections: Vec<DocSection>,
  pub caveats: Vec<Caveat>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnchorView {
  pub target: String,
  pub status: AnchorStatus,
  pub declared: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub entity: Option<EntityRef>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub certainty: Option<CertaintyTag>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoteView {
  pub repo: String,
  pub path: String,
  pub absolute_path: String,
  pub title: String,
  pub status: NoteState,
  pub anchors: Vec<AnchorView>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NotesRequest {
  pub stale_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotesAnswer {
  pub stale_only: bool,
  pub notes: Vec<NoteView>,
  pub caveats: Vec<Caveat>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoteExcerpt {
  pub heading: String,
  pub start_line: u32,
  pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnchoredNote {
  pub note: NoteView,
  pub sections: Vec<NoteExcerpt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileNotesRequest {
  pub file: String,
  pub require_fresh: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileNotesAnswer {
  pub file: Location,
  pub notes: Vec<AnchoredNote>,
  pub caveats: Vec<Caveat>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotePointer {
  pub repo: String,
  pub path: String,
  pub heading: String,
  pub status: NoteState,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub certainty: Option<CertaintyTag>,
  pub anchored_to: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DocPointer {
  pub repo: String,
  pub path: String,
  pub heading: String,
  pub start_line: u32,
  pub certainty: CertaintyTag,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DocPointers {
  pub notes: Vec<NotePointer>,
  pub notes_total: usize,
  pub docs: Vec<DocPointer>,
  pub docs_total: usize,
}

impl DocPointers {
  pub fn is_empty(&self) -> bool {
    self.notes.is_empty() && self.docs.is_empty()
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocOutline {
  pub path: String,
  pub headings: Vec<String>,
  #[serde(skip_serializing_if = "is_zero")]
  pub more_headings: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepoDocs {
  pub repo: String,
  pub docs: Vec<DocOutline>,
  #[serde(skip_serializing_if = "is_zero")]
  pub more_docs: usize,
  pub notes: usize,
  pub stale_notes: usize,
}

fn is_zero(value: &usize) -> bool {
  *value == 0
}

fn is_mapped_doc(path: &str) -> bool {
  let readme = !path.contains('/') && path.to_ascii_lowercase().starts_with(README_STEM);
  readme || path.starts_with(DOCS_DIR)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StampEdit {
  pub start: usize,
  pub end: usize,
  pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteStamp {
  Edits { edits: Vec<StampEdit>, missing: Vec<String> },
  Refused { reason: String },
}

pub(crate) struct ResolvedAnchor {
  target: String,
  declared: bool,
  entity: Option<Entity>,
  status: AnchorStatus,
  certainty: Option<CertaintyTag>,
}

impl ResolvedAnchor {
  fn view(&self) -> AnchorView {
    AnchorView {
      target: self.target.clone(),
      status: self.status,
      declared: self.declared,
      entity: self.entity.as_ref().map(EntityRef::of),
      certainty: self.certainty.clone(),
    }
  }
}

pub(crate) struct NoteReport {
  anchors: Vec<ResolvedAnchor>,
  state: NoteState,
}

impl NoteReport {
  fn view(&self, doc: &LoadedDoc, keep: impl Fn(&ResolvedAnchor) -> bool) -> NoteView {
    NoteView {
      repo: doc.repo.to_string(),
      path: doc.path.to_string(),
      absolute_path: doc.absolute.display().to_string(),
      title: doc.title(),
      status: self.state,
      anchors: self.anchors.iter().filter(|anchor| keep(anchor)).map(ResolvedAnchor::view).collect(),
    }
  }
}

impl QueryEngine {
  pub fn docs(&self, request: &DocsRequest) -> QueryResult<Response<Targeted<DocsAnswer>>> {
    if request.query.as_deref().is_none_or(|query| query.trim().is_empty()) && request.target.is_none() {
      return Err(QueryError::invalid("give a query, a target or both"));
    }
    let freshness = self.barrier()?;
    let corpus = self.corpus(if request.notes_only { DocScope::Notes } else { DocScope::All });
    let project = self.engine().project();
    let answer = self.read(Policy::with_floor(0.0), |graph| -> QueryResult<Targeted<DocsAnswer>> {
      let resolver = Resolver::new(graph, &project);
      let about = match &request.target {
        Some(spec) => match resolve_one(self, graph, spec, &TargetFilter::default())? {
          Ok(entity) => Some(entity),
          Err(outcome) => return Ok(outcome),
        },
        None => None,
      };
      let terms: Option<BTreeSet<String>> = request.query.as_deref().map(query_terms);
      let wanted = match &about {
        Some(entity) => {
          let mut wanted = Wanted::default();
          wanted.add(graph, entity)?;
          Some(wanted)
        }
        None => None,
      };
      let mut ranked = rank_sections(&resolver, &corpus, terms.as_ref(), wanted.as_ref())?;
      let total = ranked.len();
      ranked.truncate(request.limit);
      let sections = sections_of(&resolver, &corpus, ranked, &terms.unwrap_or_default(), Shape::Full)?;
      Ok(Targeted::Found(DocsAnswer {
        query: request.query.clone(),
        about: about.as_ref().map(EntityRef::of),
        notes_only: request.notes_only,
        total,
        sections,
        caveats: corpus.caveats(),
      }))
    })?;
    Ok(Response { freshness, answer })
  }

  pub(crate) fn doc_hits(&self, graph: &Graph<'_, '_>, query: &str, fusion: DocFusion, limit: usize) -> QueryResult<Vec<DocSection>> {
    let corpus = self.corpus(DocScope::All);
    let project = self.engine().project();
    let resolver = Resolver::new(graph, &project);
    let terms = query_terms(query);
    let mut ranked = rank_sections(&resolver, &corpus, Some(&terms), None)?;
    if fusion == DocFusion::Fused {
      ranked.retain(|ranked| corpus.docs[ranked.doc].terms[ranked.section].admits_fused(&terms));
    }
    ranked.truncate(limit);
    let shape = match fusion {
      DocFusion::Fused => Shape::Compact,
      DocFusion::Only => Shape::Full,
    };
    sections_of(&resolver, &corpus, ranked, &terms, shape)
  }

  pub fn notes(&self, request: &NotesRequest) -> QueryResult<Response<NotesAnswer>> {
    let freshness = self.barrier()?;
    let corpus = self.corpus(DocScope::Notes);
    let project = self.engine().project();
    let answer = self.read(Policy::with_floor(0.0), |graph| -> QueryResult<NotesAnswer> {
      let resolver = Resolver::new(graph, &project);
      let mut notes = Vec::new();
      for doc in corpus.docs.iter().filter(|doc| doc.note) {
        let report = note_report(&resolver, doc)?;
        if request.stale_only && report.state != NoteState::Stale {
          continue;
        }
        notes.push(report.view(doc, |_| true));
      }
      Ok(NotesAnswer {
        stale_only: request.stale_only,
        notes,
        caveats: corpus.caveats(),
      })
    })?;
    Ok(Response { freshness, answer })
  }

  pub fn notes_for_file(&self, request: &FileNotesRequest) -> QueryResult<Response<Targeted<FileNotesAnswer>>> {
    let freshness = self.barrier()?;
    let skip = request.require_fresh && freshness.state != FreshnessState::Fresh;
    let corpus = match skip {
      true => Corpus::empty(),
      false => self.corpus(DocScope::Notes),
    };
    let project = self.engine().project();
    let answer = self.read(Policy::with_floor(0.0), |graph| -> QueryResult<Targeted<FileNotesAnswer>> {
      let target = match resolve_one(self, graph, &request.file, &TargetFilter::default())? {
        Ok(target) => target,
        Err(outcome) => return Ok(outcome),
      };
      let Some((repo, path)) = file_of(&target) else {
        return Ok(Targeted::NotFound {
          spec: request.file.clone(),
          hint: "the target is not in a file".to_owned(),
        });
      };
      let file = Location {
        repo: repo.to_string(),
        path: path.to_string(),
        line: None,
        column: None,
      };
      if is_note_path(path.as_str()) {
        return Ok(Targeted::Found(FileNotesAnswer {
          file,
          notes: Vec::new(),
          caveats: Vec::new(),
        }));
      }
      let resolver = Resolver::new(graph, &project);
      let in_file = |entity: &Entity| file_of(entity).is_some_and(|(entity_repo, entity_path)| entity_repo == repo && entity_path == path);
      let mut notes = Vec::new();
      for doc in corpus.docs.iter().filter(|doc| doc.note) {
        let report = note_report(&resolver, doc)?;
        let anchored: Vec<&Entity> = report.anchors.iter().filter_map(|anchor| anchor.entity.as_ref()).filter(|entity| in_file(entity)).collect();
        if anchored.is_empty() {
          continue;
        }
        let mut wanted = Wanted::default();
        for entity in &anchored {
          wanted.ids.insert(entity.id);
          wanted.add(graph, entity)?;
        }
        let mut sections = Vec::new();
        for section in &doc.parsed.sections {
          if !resolver.matches(doc, section, &wanted)?.is_empty() {
            sections.push(excerpt(section));
          }
        }
        if sections.is_empty() {
          sections.extend(doc.parsed.sections.first().map(excerpt));
        }
        notes.push(AnchoredNote {
          note: report.view(doc, |anchor| anchor.entity.as_ref().is_some_and(in_file)),
          sections,
        });
      }
      let caveats = match skip {
        true => vec![Caveat::Note {
          message: "the index is not fresh, so notes were not read".to_owned(),
        }],
        false => corpus.caveats(),
      };
      Ok(Targeted::Found(FileNotesAnswer { file, notes, caveats }))
    })?;
    Ok(Response { freshness, answer })
  }

  pub(crate) fn doc_pointers(&self, graph: &Graph<'_, '_>, targets: &[Entity], scope: DocScope) -> QueryResult<DocPointers> {
    let corpus = self.corpus(scope);
    if corpus.docs.is_empty() {
      return Ok(DocPointers::default());
    }
    let project = self.engine().project();
    let resolver = Resolver::new(graph, &project);
    let mut wanted = Wanted::default();
    for target in targets {
      wanted.add(graph, target)?;
    }
    let mut pointers = DocPointers::default();
    for doc in &corpus.docs {
      if doc.note {
        let report = note_report(&resolver, doc)?;
        let anchored: Vec<&ResolvedAnchor> = report
          .anchors
          .iter()
          .filter(|anchor| anchor.entity.as_ref().is_some_and(|entity| wanted.ids.contains(&entity.id)))
          .collect();
        let Some(first) = anchored.first() else { continue };
        pointers.notes_total += 1;
        if pointers.notes.len() == POINTERS_SHOWN {
          continue;
        }
        let mut anchored_to: Vec<String> = anchored
          .iter()
          .filter_map(|anchor| anchor.entity.as_ref())
          .map(|entity| EntityRef::of(entity).short)
          .collect();
        anchored_to.dedup();
        pointers.notes.push(NotePointer {
          repo: doc.repo.to_string(),
          path: doc.path.to_string(),
          heading: heading_for(&resolver, doc, first)?,
          status: report.state,
          certainty: anchored.iter().all(|anchor| anchor.certainty.is_some()).then(|| first.certainty.clone()).flatten(),
          anchored_to,
        });
        continue;
      }
      for section in &doc.parsed.sections {
        let matched = resolver.matches(doc, section, &wanted)?;
        let Some(fewest) = matched.iter().map(|matched| matched.candidates.len()).min() else {
          continue;
        };
        pointers.docs_total += 1;
        if pointers.docs.len() < POINTERS_SHOWN {
          pointers.docs.push(DocPointer {
            repo: doc.repo.to_string(),
            path: doc.path.to_string(),
            heading: section.heading_path(),
            start_line: section.start_line,
            certainty: mention_certainty(fewest)?,
          });
        }
      }
    }
    Ok(pointers)
  }

  pub(crate) fn note_pointers_for(&self, graph: &Graph<'_, '_>, items: &[EntityId]) -> QueryResult<Vec<NotePointer>> {
    let mut targets = Vec::new();
    for id in items {
      targets.extend(graph.entity(*id)?);
    }
    if targets.is_empty() {
      return Ok(Vec::new());
    }
    Ok(self.doc_pointers(graph, &targets, DocScope::Notes)?.notes)
  }

  pub fn note_stamp(&self, repo: &RepoName, path: &RelPath, text: &str) -> QueryResult<Response<NoteStamp>> {
    let freshness = self.barrier()?;
    let project = self.engine().project();
    let answer = self.read(Policy::with_floor(0.0), |graph| stamp(&Resolver::new(graph, &project), repo, path, text))?;
    Ok(Response { freshness, answer })
  }

  pub(crate) fn doc_map(&self, graph: &Graph<'_, '_>) -> QueryResult<Vec<RepoDocs>> {
    let corpus = self.corpus(DocScope::All);
    let project = self.engine().project();
    let resolver = Resolver::new(graph, &project);
    let mut map = Vec::new();
    for repo in &project.repos {
      let mut entry = RepoDocs {
        repo: repo.name.to_string(),
        docs: Vec::new(),
        more_docs: 0,
        notes: 0,
        stale_notes: 0,
      };
      for doc in corpus.docs.iter().filter(|doc| doc.repo == repo.name) {
        if doc.note {
          entry.notes += 1;
          if note_report(&resolver, doc)?.state == NoteState::Stale {
            entry.stale_notes += 1;
          }
          continue;
        }
        if !is_mapped_doc(doc.path.as_str()) {
          continue;
        }
        if entry.docs.len() == MAPPED_DOCS {
          entry.more_docs += 1;
          continue;
        }
        let headings: Vec<String> = doc
          .parsed
          .sections
          .iter()
          .filter(|section| section.heading.len() <= MAPPED_DEPTH && !section.preamble)
          .map(RawSection::heading_path)
          .collect();
        entry.docs.push(DocOutline {
          path: doc.path.to_string(),
          more_headings: headings.len().saturating_sub(MAPPED_HEADINGS),
          headings: headings.into_iter().take(MAPPED_HEADINGS).collect(),
        });
      }
      map.push(entry);
    }
    Ok(map)
  }
}

fn excerpt(section: &RawSection) -> NoteExcerpt {
  let text = section.body.lines().flat_map(str::split_whitespace).collect::<Vec<_>>().join(" ");
  NoteExcerpt {
    heading: section.heading_path(),
    start_line: section.start_line,
    text: clip(&text, EXCERPT_CHARS),
  }
}

fn heading_for(resolver: &Resolver<'_, '_, '_>, doc: &LoadedDoc, anchor: &ResolvedAnchor) -> QueryResult<String> {
  let Some(entity) = &anchor.entity else { return Ok(doc.title()) };
  let mut wanted = Wanted::default();
  wanted.ids.insert(entity.id);
  wanted.add(resolver.graph, entity)?;
  for section in &doc.parsed.sections {
    if !resolver.matches(doc, section, &wanted)?.is_empty() {
      return Ok(section.heading_path());
    }
  }
  Ok(doc.title())
}

fn note_report(resolver: &Resolver<'_, '_, '_>, doc: &LoadedDoc) -> QueryResult<NoteReport> {
  let declared = doc.parsed.frontmatter.as_ref().map(|frontmatter| frontmatter.anchors.as_slice()).unwrap_or_default();
  let mut anchors = Vec::new();
  if declared.is_empty() {
    for (entity, certainty) in mention_anchors(resolver, doc)? {
      anchors.push(ResolvedAnchor {
        target: resolver.anchor_spec(&entity)?.unwrap_or_else(|| entity.key.to_string()),
        declared: false,
        entity: Some(entity),
        status: AnchorStatus::Unstamped,
        certainty: Some(certainty),
      });
    }
  }
  for raw in declared {
    let entity = resolver.anchor_entity(&raw.target)?;
    let status = match (&entity, &raw.body) {
      (None, _) => AnchorStatus::Missing,
      (Some(_), None) => AnchorStatus::Unstamped,
      (Some(entity), Some(body)) => match resolver.body_hash(entity)? {
        Some(hash) if hash == *body => AnchorStatus::Current,
        Some(_) => AnchorStatus::Stale,
        None => AnchorStatus::Unreadable,
      },
    };
    anchors.push(ResolvedAnchor {
      target: raw.target.clone(),
      declared: true,
      entity,
      status,
      certainty: None,
    });
  }
  let state = NoteState::of(&anchors);
  Ok(NoteReport { anchors, state })
}

fn mention_anchors(resolver: &Resolver<'_, '_, '_>, doc: &LoadedDoc) -> QueryResult<Vec<(Entity, CertaintyTag)>> {
  let mut seen = HashSet::new();
  let mut anchors = Vec::new();
  for mention in doc.parsed.sections.iter().flat_map(|section| &section.mentions) {
    if mention.origin == Origin::Fenced {
      continue;
    }
    let Found::Entities(found) = resolver.resolve(doc, mention)? else { continue };
    let [entity] = found.as_slice() else { continue };
    if seen.insert(entity.id) {
      anchors.push((entity.clone(), mention_certainty(1)?));
    }
  }
  Ok(anchors)
}

fn stamp(resolver: &Resolver<'_, '_, '_>, repo: &RepoName, path: &RelPath, text: &str) -> QueryResult<NoteStamp> {
  if !is_note_path(path.as_str()) {
    return Ok(NoteStamp::Refused {
      reason: format!("{repo}:{path} is not a note: notes live under .code-atlas/notes/; nothing was written"),
    });
  }
  let name = path.as_str().rsplit('/').next().unwrap_or(path.as_str());
  let doc = LoadedDoc {
    repo: repo.clone(),
    path: path.clone(),
    absolute: PathBuf::new(),
    note: true,
    parsed: Arc::new(parse(text, name)),
    terms: Arc::new(Vec::new()),
  };
  let to_edit = |replacement: markdown::Replacement| StampEdit {
    start: replacement.range.start,
    end: replacement.range.end,
    text: replacement.text,
  };
  match &doc.parsed.frontmatter {
    Some(frontmatter) if frontmatter.anchors.is_empty() => Ok(NoteStamp::Refused {
      reason: format!("{repo}:{path} has frontmatter without anchors; add an anchors list or remove the frontmatter; nothing was written"),
    }),
    Some(frontmatter) => {
      let mut stamped = Vec::new();
      let mut missing = Vec::new();
      for anchor in &frontmatter.anchors {
        let hash = match resolver.anchor_entity(&anchor.target)? {
          Some(entity) => resolver.body_hash(&entity)?,
          None => None,
        };
        match hash {
          Some(hash) => stamped.push((anchor, hash)),
          None => missing.push(anchor.target.clone()),
        }
      }
      Ok(NoteStamp::Edits {
        edits: stamp_existing(text, &stamped).into_iter().map(to_edit).collect(),
        missing,
      })
    }
    None => {
      let mut anchors = Vec::new();
      for (entity, _) in mention_anchors(resolver, &doc)? {
        let (Some(spec), Some(hash)) = (resolver.anchor_spec(&entity)?, resolver.body_hash(&entity)?) else {
          continue;
        };
        anchors.push((spec, hash));
      }
      if anchors.is_empty() {
        return Ok(NoteStamp::Refused {
          reason: format!("{repo}:{path} names no code that resolves to exactly one symbol or file; name it in backticks; nothing was written"),
        });
      }
      Ok(NoteStamp::Edits {
        edits: vec![to_edit(new_frontmatter(text, &anchors))],
        missing: Vec::new(),
      })
    }
  }
}

fn mention_views(resolver: &Resolver<'_, '_, '_>, doc: &LoadedDoc, section: &RawSection) -> QueryResult<Vec<MentionView>> {
  let mut seen = HashSet::new();
  let mut views = Vec::new();
  let ordered = section
    .mentions
    .iter()
    .filter(|mention| mention.origin != Origin::Fenced)
    .chain(section.mentions.iter().filter(|mention| mention.origin == Origin::Fenced));
  for mention in ordered {
    if views.len() == MENTIONS_SHOWN {
      break;
    }
    if !seen.insert(mention.text.as_str()) {
      continue;
    }
    match resolver.resolve(doc, mention)? {
      Found::Entities(found) => views.push(MentionView::Resolved {
        text: mention.text.clone(),
        line: mention.line,
        certainty: mention_certainty(found.len())?,
        candidates: found.iter().take(CANDIDATES_SHOWN).map(EntityRef::of).collect(),
        total: found.len(),
      }),
      Found::Drift => views.push(MentionView::Drift {
        text: mention.text.clone(),
        line: mention.line,
      }),
      Found::Nothing => {}
    }
  }
  Ok(views)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocFusion {
  Fused,
  Only,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
  Compact,
  Full,
}

struct Ranked {
  doc: usize,
  section: usize,
  score: Option<f64>,
  views: Vec<MentionView>,
}

fn rank_sections(resolver: &Resolver<'_, '_, '_>, corpus: &Corpus, terms: Option<&BTreeSet<String>>, wanted: Option<&Wanted>) -> QueryResult<Vec<Ranked>> {
  let mut candidates: Vec<Ranked> = Vec::new();
  for (doc_index, doc) in corpus.docs.iter().enumerate() {
    for (section_index, section) in doc.parsed.sections.iter().enumerate() {
      let Some(wanted) = wanted else {
        candidates.push(Ranked {
          doc: doc_index,
          section: section_index,
          score: None,
          views: Vec::new(),
        });
        continue;
      };
      let matched = resolver.matches(doc, section, wanted)?;
      if matched.is_empty() {
        continue;
      }
      let mut views = Vec::new();
      for found in matched.iter().take(MENTIONS_SHOWN) {
        views.push(MentionView::Resolved {
          text: found.mention.text.clone(),
          line: found.mention.line,
          certainty: mention_certainty(found.candidates.len())?,
          candidates: found.candidates.iter().take(CANDIDATES_SHOWN).map(EntityRef::of).collect(),
          total: found.candidates.len(),
        });
      }
      candidates.push(Ranked {
        doc: doc_index,
        section: section_index,
        score: None,
        views,
      });
    }
  }
  if let Some(terms) = terms {
    let all: Vec<&SectionTerms> = corpus.docs.iter().flat_map(|doc| doc.terms.iter()).collect();
    let scores = bm25(&all, terms);
    let offsets: Vec<usize> = corpus
      .docs
      .iter()
      .scan(0, |offset, doc| {
        let start = *offset;
        *offset += doc.terms.len();
        Some(start)
      })
      .collect();
    candidates.retain_mut(|ranked| {
      let score = scores.get(offsets[ranked.doc] + ranked.section).copied().unwrap_or(0.0);
      ranked.score = Some((score * 1000.0).round() / 1000.0);
      score > 0.0
    });
  }
  let place = |ranked: &Ranked| {
    let doc = &corpus.docs[ranked.doc];
    (!doc.note, doc.repo.clone(), doc.path.clone(), doc.parsed.sections[ranked.section].start_line)
  };
  candidates.sort_by(|left, right| match (left.score, right.score) {
    (Some(left_score), Some(right_score)) => right_score.total_cmp(&left_score).then_with(|| place(left).cmp(&place(right))),
    _ => place(left).cmp(&place(right)),
  });
  Ok(candidates)
}

fn sections_of(resolver: &Resolver<'_, '_, '_>, corpus: &Corpus, ranked: Vec<Ranked>, terms: &BTreeSet<String>, shape: Shape) -> QueryResult<Vec<DocSection>> {
  let mut states: HashMap<usize, NoteState> = HashMap::new();
  let mut sections = Vec::with_capacity(ranked.len());
  for ranked in ranked {
    let doc = &corpus.docs[ranked.doc];
    let section = &doc.parsed.sections[ranked.section];
    let note_status = match (doc.note, states.get(&ranked.doc)) {
      (false, _) => None,
      (true, Some(state)) => Some(*state),
      (true, None) => {
        let state = note_report(resolver, doc)?.state;
        states.insert(ranked.doc, state);
        Some(state)
      }
    };
    let (mentions, snippet_chars) = match (shape, ranked.views.is_empty()) {
      (Shape::Compact, _) => (Vec::new(), FUSED_SNIPPET_CHARS),
      (Shape::Full, true) => (mention_views(resolver, doc, section)?, SNIPPET_CHARS),
      (Shape::Full, false) => (ranked.views, SNIPPET_CHARS),
    };
    sections.push(DocSection {
      repo: doc.repo.to_string(),
      path: doc.path.to_string(),
      absolute_path: doc.absolute.display().to_string(),
      heading: section.heading_path(),
      start_line: section.start_line,
      end_line: section.end_line,
      kind: if doc.note { DocKind::Note } else { DocKind::Doc },
      note_status,
      score: ranked.score,
      rank: None,
      snippet: snippet(&section.body, terms, snippet_chars),
      mentions,
    });
  }
  Ok(sections)
}
