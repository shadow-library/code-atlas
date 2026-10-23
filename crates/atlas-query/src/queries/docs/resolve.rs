use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::rc::Rc;

use atlas_core::{Certainty, Confidence, ContentHash, Entity, EntityId, EntityKey, RelPath, RepoName};
use atlas_project::Project;

use super::LoadedDoc;
use super::markdown::{Origin, RawMention, RawSection, Shape};
use crate::error::{QueryError, QueryResult};
use crate::graph::{FileKey, Graph};
use crate::model::CertaintyTag;
use crate::queries::symbols::{file_symbols, in_file_segments, source_order, symbols_named};
use crate::target::{Resolution, TargetFilter, TargetSpec, resolve};

const UNIQUE_CONFIDENCE: f32 = 0.7;
const AMBIGUOUS_CONFIDENCE: f32 = 0.4;
const MENTION_RULE: &str = "doc-mention";
const HASH_HEX: usize = 12;

pub(crate) enum Found {
  Entities(Vec<Entity>),
  Drift,
  Nothing,
}

pub(crate) fn mention_certainty(candidates: usize) -> QueryResult<CertaintyTag> {
  let confidence = match candidates {
    1 => UNIQUE_CONFIDENCE,
    _ => AMBIGUOUS_CONFIDENCE,
  };
  let confidence = Confidence::new(confidence).map_err(|error| QueryError::invalid(error.to_string()))?;
  Ok(CertaintyTag::of(&Certainty::heuristic(confidence, MENTION_RULE)))
}

pub(crate) fn file_of(entity: &Entity) -> Option<(RepoName, RelPath)> {
  match (&entity.key, &entity.repo, &entity.path) {
    (EntityKey::File { repo, path }, _, _) => Some((repo.clone(), path.clone())),
    (_, Some(repo), Some(path)) => Some((repo.clone(), path.clone())),
    _ => None,
  }
}

fn name_tail(text: &str) -> &str {
  text.rsplit(['.', ':']).next().unwrap_or(text)
}

fn file_name(text: &str) -> &str {
  text.rsplit('/').next().unwrap_or(text)
}

fn normalized(base: &str, relative: &str) -> Option<String> {
  let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
  for part in relative.split('/') {
    match part {
      "" | "." => {}
      ".." => {
        parts.pop()?;
      }
      other => parts.push(other),
    }
  }
  (!parts.is_empty()).then(|| parts.join("/"))
}

#[derive(Default)]
pub(crate) struct Wanted {
  pub ids: HashSet<EntityId>,
  names: HashSet<String>,
  files: HashSet<String>,
}

impl Wanted {
  pub fn add(&mut self, graph: &Graph<'_, '_>, entity: &Entity) -> QueryResult<()> {
    self.ids.insert(entity.id);
    if let EntityKey::File { repo, path } = &entity.key {
      self.files.insert(file_name(path.as_str()).to_owned());
      for symbol in file_symbols(graph, repo, path)? {
        self.ids.insert(symbol.id);
        self.names.insert(symbol.name.clone());
      }
      return Ok(());
    }
    self.names.insert(entity.name.clone());
    Ok(())
  }

  fn may_match(&self, mention: &RawMention) -> bool {
    match mention.shape {
      Shape::Path => self.files.contains(file_name(&mention.text)),
      Shape::Name { .. } => self.names.contains(name_tail(&mention.text)),
    }
  }
}

pub(crate) struct Matched<'s> {
  pub mention: &'s RawMention,
  pub candidates: Vec<Entity>,
}

pub(crate) struct Resolver<'g, 'a, 'c> {
  pub graph: &'g Graph<'a, 'c>,
  project: &'g Project,
  repos: Vec<RepoName>,
  names: RefCell<HashMap<String, Rc<Vec<Entity>>>>,
  texts: RefCell<HashMap<FileKey, Option<Rc<String>>>>,
}

impl<'g, 'a, 'c> Resolver<'g, 'a, 'c> {
  pub fn new(graph: &'g Graph<'a, 'c>, project: &'g Project) -> Self {
    Self {
      graph,
      project,
      repos: project.repos.iter().map(|repo| repo.name.clone()).collect(),
      names: RefCell::new(HashMap::new()),
      texts: RefCell::new(HashMap::new()),
    }
  }

  pub fn resolve(&self, doc: &LoadedDoc, mention: &RawMention) -> QueryResult<Found> {
    match mention.shape {
      Shape::Path => Ok(
        self
          .path_entity(doc, &mention.text, mention.origin)?
          .map_or(Found::Nothing, |entity| Found::Entities(vec![entity])),
      ),
      Shape::Name { drift_eligible } => {
        let found = self.named(&mention.text)?;
        Ok(match (found.is_empty(), drift_eligible && mention.origin == Origin::Code) {
          (false, _) => Found::Entities(found.as_ref().clone()),
          (true, true) => Found::Drift,
          (true, false) => Found::Nothing,
        })
      }
    }
  }

  fn named(&self, text: &str) -> QueryResult<Rc<Vec<Entity>>> {
    if let Some(known) = self.names.borrow().get(text) {
      return Ok(Rc::clone(known));
    }
    let mut found = symbols_named(self.graph, text)?;
    let simple = name_tail(text);
    if found.is_empty() && simple != text {
      found = symbols_named(self.graph, simple)?;
      if found.len() != 1 {
        found.clear();
      }
    }
    let found = Rc::new(found);
    self.names.borrow_mut().insert(text.to_owned(), Rc::clone(&found));
    Ok(found)
  }

  fn path_entity(&self, doc: &LoadedDoc, text: &str, origin: Origin) -> QueryResult<Option<Entity>> {
    let (repo, relative) = match text.split_once(':') {
      Some((repo, rest)) => match self.repos.iter().find(|known| known.as_str() == repo) {
        Some(known) => (known.clone(), rest),
        None => return Ok(None),
      },
      None => (doc.repo.clone(), text),
    };
    let directory = doc.path.as_str().rsplit_once('/').map_or("", |(directory, _)| directory);
    let candidates = match origin {
      Origin::Link => [normalized(directory, relative), normalized("", relative)],
      _ => [normalized("", relative), normalized(directory, relative)],
    };
    for candidate in candidates.into_iter().flatten() {
      let Ok(path) = RelPath::new(&candidate) else { continue };
      if let Some(entity) = self.graph.tx().entity_by_key(&EntityKey::File { repo: repo.clone(), path })? {
        return Ok(Some(entity));
      }
    }
    Ok(None)
  }

  pub fn matches<'s>(&self, doc: &LoadedDoc, section: &'s RawSection, wanted: &Wanted) -> QueryResult<Vec<Matched<'s>>> {
    let mut seen = HashSet::new();
    let mut matched = Vec::new();
    for mention in &section.mentions {
      if !wanted.may_match(mention) || !seen.insert(mention.text.as_str()) {
        continue;
      }
      let Found::Entities(candidates) = self.resolve(doc, mention)? else { continue };
      if candidates.iter().any(|candidate| wanted.ids.contains(&candidate.id)) {
        matched.push(Matched { mention, candidates });
      }
    }
    Ok(matched)
  }

  pub fn anchor_entity(&self, spec: &str) -> QueryResult<Option<Entity>> {
    let Some((file, name)) = spec.split_once('#') else { return self.found(spec) };
    let Some(file) = self.found(file)? else { return Ok(None) };
    let EntityKey::File { repo, path } = &file.key else { return Ok(None) };
    let symbols = file_symbols(self.graph, repo, path)?;
    let mut matched = Vec::new();
    for symbol in &symbols {
      let segments = in_file_segments(self.graph, symbol)?;
      if [segments.join("/"), segments.join("."), segments.join("::")].iter().any(|joined| joined == name) {
        matched.push(symbol.clone());
      }
    }
    if matched.is_empty() {
      let simple: Vec<&Entity> = symbols.iter().filter(|symbol| symbol.name == name).collect();
      if let [only] = simple.as_slice() {
        matched.push((*only).clone());
      }
    }
    matched.sort_by(source_order);
    Ok(matched.into_iter().next())
  }

  fn found(&self, spec: &str) -> QueryResult<Option<Entity>> {
    if TargetSpec::parse(spec).is_err() {
      return Ok(None);
    }
    Ok(match resolve(self.graph, &self.repos, spec, &TargetFilter::default())? {
      Resolution::Found(entity) => Some(*entity),
      Resolution::Ambiguous(_) | Resolution::NotFound(_) => None,
    })
  }

  pub fn anchor_spec(&self, entity: &Entity) -> QueryResult<Option<String>> {
    let Some((repo, path)) = file_of(entity) else { return Ok(None) };
    if let EntityKey::File { .. } = entity.key {
      return Ok(Some(format!("{repo}:{path}")));
    }
    Ok(Some(format!("{repo}:{path}#{}", in_file_segments(self.graph, entity)?.join("/"))))
  }

  pub fn body_hash(&self, entity: &Entity) -> QueryResult<Option<String>> {
    let Some((repo, path)) = file_of(entity) else { return Ok(None) };
    let Some(text) = self.indexed_text(&repo, &path)? else { return Ok(None) };
    let bytes = match (&entity.key, entity.span) {
      (EntityKey::File { .. }, _) => Some(text.as_bytes()),
      (_, Some(span)) => text.as_bytes().get(span.start_byte as usize..span.end_byte as usize),
      _ => None,
    };
    Ok(bytes.map(|bytes| ContentHash::of(bytes).to_string()[..HASH_HEX].to_owned()))
  }

  fn indexed_text(&self, repo: &RepoName, path: &RelPath) -> QueryResult<Option<Rc<String>>> {
    let key = (repo.clone(), path.clone());
    if let Some(known) = self.texts.borrow().get(&key) {
      return Ok(known.clone());
    }
    let record = self.graph.tx().file(repo, path)?;
    let root = self.project.repos.iter().find(|config| config.name == *repo).map(|config| config.path.join(path.as_str()));
    let text = match (record, root) {
      (Some(record), Some(absolute)) => fs::read(absolute)
        .ok()
        .filter(|bytes| ContentHash::of(bytes) == record.content_hash)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(Rc::new),
      _ => None,
    };
    self.texts.borrow_mut().insert(key, text.clone());
    Ok(text)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_normalise_relative_paths_inside_the_repo_only() {
    assert_eq!(normalized("docs/guide", "../src/a.ts").as_deref(), Some("docs/src/a.ts"));
    assert_eq!(normalized("docs", "./../src/a.ts").as_deref(), Some("src/a.ts"));
    assert_eq!(normalized("", "../a.ts"), None);
    assert_eq!(normalized("", "src//a.ts").as_deref(), Some("src/a.ts"));
  }

  #[test]
  fn should_take_the_last_segment_of_a_name_or_path() {
    assert_eq!(name_tail("crate::retry::Backoff"), "Backoff");
    assert_eq!(name_tail("ApiClient.request"), "request");
    assert_eq!(file_name("src/lib/a.ts"), "a.ts");
  }
}
