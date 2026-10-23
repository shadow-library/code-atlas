use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use atlas_core::{
  Certainty, Confidence, ContentHash, EdgeKind, Entity, EntityId, EntityKind, FileFacts, Language, LanguageFamily, ParseStatus, RefKind, RelPath, RepoName, SymbolFact,
  is_test_code, is_test_path,
};
use atlas_similarity::{Field, SMALL_BODY_TOKENS, SearchOptions, SimilarityIndex, name_words};
use atlas_store::EntityFilter;
use serde::Serialize;

use crate::QueryEngine;
use crate::error::{QueryError, QueryResult};
use crate::graph::Graph;
use crate::model::{Caveat, CertaintyTag, EntityRef, Location, Response};
use crate::policy::Policy;
use crate::queries::common::{is_test, language, locate_file, package_key};
use crate::queries::counterparts::{PackageIndex, is_test_or_generated_path};
use crate::queries::similar::{analyse_text, top_level_callables};

const MIN_UNIT_TOKENS: usize = 12;
const NEAR_DUPLICATE_FLOOR: f64 = 0.6;
const NEAR_DUPLICATES_FETCHED: usize = 20;
const PREVIOUS_MATCHES_FETCHED: usize = 50;
const NAME_HITS_FETCHED: usize = 25;
const MIN_SHARED_NAME_PARTS: usize = 2;
const NAME_MATCH_CONFIDENCE: f32 = 0.5;
const PER_UNIT: usize = 2;
const PER_ANSWER: usize = 3;
const MAX_LITERALS: usize = 8;
const MAX_LITERAL_CHARS: usize = 60;
const NEAR_DUPLICATE_RULE: &str = "reuse-near-duplicate";
const NAME_PARTS_RULE: &str = "reuse-name-parts";
const CALLED: [RefKind; 3] = [RefKind::Call, RefKind::MethodCall, RefKind::Instantiation];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReuseRequest {
  pub repo_path: String,
  pub text: Option<String>,
  pub lines: Option<(u32, u32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexState {
  Current,
  Changed,
  NotIndexed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitStatus {
  New,
  Changed,
  OnDisk,
}

impl UnitStatus {
  pub fn label(self) -> &'static str {
    match self {
      Self::New => "new",
      Self::Changed => "changed",
      Self::OnDisk => "on disk",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum ReuseReason {
  NearDuplicate { jaccard: f64 },
  SameNameParts { parts: Vec<String> },
}

impl ReuseReason {
  pub fn text(&self) -> String {
    match self {
      Self::NearDuplicate { jaccard } => format!("near duplicate {jaccard:.2}"),
      Self::SameNameParts { parts } => format!("same name parts: {}", parts.join(", ")),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReuseCandidate {
  pub entity: EntityRef,
  pub start_line: u32,
  pub end_line: u32,
  pub signature: Option<String>,
  pub certainty: CertaintyTag,
  pub reason: ReuseReason,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReuseUnit {
  pub name: String,
  pub kind: String,
  pub start_line: u32,
  pub end_line: u32,
  pub tokens: u32,
  pub status: UnitStatus,
  pub candidates: Vec<ReuseCandidate>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReuseAnswer {
  pub file: Location,
  pub index_state: IndexState,
  pub units: Vec<ReuseUnit>,
  pub caveats: Vec<Caveat>,
}

impl ReuseAnswer {
  pub fn candidates(&self) -> usize {
    self.units.iter().map(|unit| unit.candidates.len()).sum()
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Region {
  start: u32,
  end: u32,
}

impl Region {
  fn touches(self, start: u32, end: u32) -> bool {
    start <= self.end && self.start <= end
  }
}

struct Unit {
  name: String,
  kind: EntityKind,
  start_line: u32,
  end_line: u32,
  tokens: u32,
  body: Vec<u32>,
  status: UnitStatus,
  previous: Option<Vec<u32>>,
  called: BTreeSet<String>,
  query: String,
}

struct Prepared {
  family: LanguageFamily,
  units: Vec<Unit>,
  old_region: Region,
}

impl QueryEngine {
  pub fn reuse_check(&self, request: &ReuseRequest) -> QueryResult<Response<ReuseAnswer>> {
    let freshness = self.barrier()?;
    let project = self.engine().project();
    let (repo, path) = locate_file(&project, &request.repo_path)?;
    let root = project
      .repos
      .iter()
      .find(|config| config.name == repo)
      .map(|config| config.path.clone())
      .ok_or_else(|| QueryError::invalid(format!("unknown repo {repo}")))?;
    let max_bytes = project.settings.max_file_bytes;
    let disk = read_capped(&root.join(path.as_str()), max_bytes);
    if request.text.as_ref().is_some_and(|text| text.len() as u64 > max_bytes) {
      return Err(QueryError::invalid(format!("the text is over the {max_bytes} byte file limit")));
    }
    let mut caveats = Vec::new();
    let prepared = self.prepare(request, &repo, &path, &disk, &mut caveats)?;
    let index = self.engine().similarity();
    let answer = self.read(Policy::with_floor(0.0), |graph| -> QueryResult<ReuseAnswer> {
      let index_state = index_state(graph, &repo, &path, &disk)?;
      let units = match &prepared {
        Some(prepared) => check(graph, &index, &repo, &path, prepared)?,
        None => Vec::new(),
      };
      Ok(ReuseAnswer {
        file: Location {
          repo: repo.to_string(),
          path: path.to_string(),
          line: None,
          column: None,
        },
        index_state,
        units,
        caveats,
      })
    })?;
    Ok(Response { freshness, answer })
  }

  fn prepare(&self, request: &ReuseRequest, repo: &RepoName, path: &RelPath, disk: &OnDisk, caveats: &mut Vec<Caveat>) -> QueryResult<Option<Prepared>> {
    let mut note = |message: String| caveats.push(Caveat::Note { message });
    let Some(language) = Language::from_path(path.as_str()) else {
      note(format!("{repo}:{path} is not a Rust or TypeScript file, so nothing was checked"));
      return Ok(None);
    };
    if is_test_or_generated_path(path.as_str()) || is_test_path(path.as_str()) {
      note(format!("{repo}:{path} is a test or generated file, so nothing was checked"));
      return Ok(None);
    }
    let disk_text = match disk {
      OnDisk::Missing => None,
      OnDisk::TooLarge => {
        note(format!("{repo}:{path} on disk is over the max_file_bytes limit, so nothing was checked"));
        return Ok(None);
      }
      OnDisk::Read(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
    };
    let (new_text, old_text) = match (&request.text, disk_text) {
      (Some(text), old) => (text.clone(), old),
      (None, Some(text)) => (text, None),
      (None, None) => return Err(QueryError::invalid(format!("{repo}:{path} does not exist; give the text to check"))),
    };
    if old_text.as_deref() == Some(new_text.as_str()) {
      note("the text is the same as the file on disk, so nothing was checked".to_owned());
      return Ok(None);
    }
    let facts = analyse_text(self, language, &new_text)?;
    if let ParseStatus::Failed { reason } = &facts.parse_status {
      note(format!("the text could not be analysed: {reason}"));
      return Ok(None);
    }
    let whole = Region {
      start: 1,
      end: line_count(&new_text),
    };
    let within_lines = |region: Region| match request.lines {
      Some((start, end)) => Region {
        start: start.max(region.start),
        end: end.min(region.end),
      },
      None => region,
    };
    let (new_region, old_region, previous) = match (&request.text, &old_text) {
      (_, Some(old)) => {
        let (old_region, new_region) = changed_regions(old, &new_text);
        let old_facts = analyse_text(self, language, old)?;
        (within_lines(new_region), old_region, Some(unit_bodies(&old_facts)))
      }
      (Some(_), None) => (within_lines(whole), whole, Some(HashMap::new())),
      (None, None) => (within_lines(whole), within_lines(whole), None),
    };
    let mut unchanged = 0;
    let mut units = Vec::new();
    for symbol in top_level_callables(&facts) {
      let tokens = symbol.body.as_ref().map_or(0, |body| body.normalized_tokens.len());
      if tokens < MIN_UNIT_TOKENS || symbol.name.is_empty() || !new_region.touches(symbol.span.start_line, symbol.span.end_line) {
        continue;
      }
      let body = symbol.body.as_ref().map(|body| body.normalized_tokens.clone()).unwrap_or_default();
      let (status, before) = match &previous {
        None => (UnitStatus::OnDisk, None),
        Some(previous) => match previous.get(&qualified(symbol)) {
          Some(old) if *old == body => {
            unchanged += 1;
            continue;
          }
          Some(old) => (UnitStatus::Changed, Some(old.clone())),
          None => (UnitStatus::New, None),
        },
      };
      let called = called_names(&facts, symbol);
      let body_text = new_text.get(symbol.span.start_byte as usize..symbol.span.end_byte as usize).unwrap_or_default();
      let literals = literals(body_text, language.family());
      let query = std::iter::once(symbol.name.as_str())
        .chain(called.iter().map(String::as_str))
        .chain(literals.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
      units.push(Unit {
        name: symbol.name.clone(),
        kind: symbol.kind.clone(),
        start_line: symbol.span.start_line,
        end_line: symbol.span.end_line,
        tokens: symbol.body.as_ref().map_or(0, |body| body.token_count),
        body,
        status,
        previous: before,
        called,
        query,
      });
    }
    if unchanged > 0 {
      note(format!("{unchanged} unchanged functions were not checked"));
    }
    Ok(Some(Prepared {
      family: language.family(),
      units,
      old_region,
    }))
  }
}

fn line_count(text: &str) -> u32 {
  u32::try_from(text.split('\n').count()).unwrap_or(u32::MAX)
}

fn changed_regions(old: &str, new: &str) -> (Region, Region) {
  let old_lines: Vec<&str> = old.split('\n').collect();
  let new_lines: Vec<&str> = new.split('\n').collect();
  let prefix = old_lines.iter().zip(&new_lines).take_while(|(left, right)| left == right).count();
  let room = old_lines.len().min(new_lines.len()) - prefix;
  let suffix = old_lines
    .iter()
    .rev()
    .zip(new_lines.iter().rev())
    .take(room)
    .take_while(|(left, right)| left == right)
    .count();
  let region = |lines: usize| Region {
    start: u32::try_from(prefix + 1).unwrap_or(u32::MAX),
    end: u32::try_from(lines - suffix).unwrap_or(u32::MAX),
  };
  (region(old_lines.len()), region(new_lines.len()))
}

fn qualified(symbol: &SymbolFact) -> String {
  match symbol.item_path.is_empty() {
    true => symbol.name.clone(),
    false => symbol.item_path.join("."),
  }
}

fn unit_bodies(facts: &FileFacts) -> HashMap<String, Vec<u32>> {
  top_level_callables(facts)
    .into_iter()
    .map(|symbol| (qualified(symbol), symbol.body.as_ref().map(|body| body.normalized_tokens.clone()).unwrap_or_default()))
    .collect()
}

fn called_names(facts: &FileFacts, unit: &SymbolFact) -> BTreeSet<String> {
  let inside = |mut local| {
    loop {
      if local == unit.local {
        return true;
      }
      match facts.symbols.iter().find(|symbol| symbol.local == local).and_then(|symbol| symbol.parent) {
        Some(parent) => local = parent,
        None => return false,
      }
    }
  };
  facts
    .references
    .iter()
    .filter(|reference| CALLED.contains(&reference.kind) && reference.from.is_some_and(inside))
    .filter_map(|reference| reference.name())
    .filter(|name| name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$'))
    .map(str::to_owned)
    .collect()
}

fn literals(body: &str, family: LanguageFamily) -> Vec<String> {
  let quotes: &[char] = match family {
    LanguageFamily::Rust => &['"'],
    _ => &['\'', '"', '`'],
  };
  let mut found: Vec<String> = Vec::new();
  let mut chars = body.chars().peekable();
  while let Some(c) = chars.next() {
    match (c, chars.peek()) {
      ('/', Some('/')) => {
        while chars.next_if(|next| *next != '\n').is_some() {}
        continue;
      }
      ('/', Some('*')) => {
        chars.next();
        let mut previous = ' ';
        for next in chars.by_ref() {
          if previous == '*' && next == '/' {
            break;
          }
          previous = next;
        }
        continue;
      }
      _ if !quotes.contains(&c) => continue,
      _ => {}
    }
    let mut literal = String::new();
    let mut escaped = false;
    for next in chars.by_ref() {
      match (escaped, next) {
        (true, _) => escaped = false,
        (false, '\\') => escaped = true,
        (false, quote) if quote == c => break,
        (false, '\n') if c != '`' => break,
        (false, other) => literal.push(other),
      }
    }
    let kept = literal.chars().count() <= MAX_LITERAL_CHARS && literal.chars().any(char::is_alphabetic);
    if kept && !found.contains(&literal) && found.len() < MAX_LITERALS {
      found.push(literal);
    }
  }
  found
}

enum OnDisk {
  Missing,
  TooLarge,
  Read(Vec<u8>),
}

fn read_capped(path: &Path, max_bytes: u64) -> OnDisk {
  match fs::metadata(path) {
    Ok(metadata) if metadata.len() > max_bytes => OnDisk::TooLarge,
    Ok(_) => fs::read(path).map_or(OnDisk::Missing, OnDisk::Read),
    Err(_) => OnDisk::Missing,
  }
}

fn index_state(graph: &Graph<'_, '_>, repo: &RepoName, path: &RelPath, disk: &OnDisk) -> QueryResult<IndexState> {
  let Some(record) = graph.tx().file(repo, path)? else {
    return Ok(IndexState::NotIndexed);
  };
  Ok(match matches!(disk, OnDisk::Read(bytes) if ContentHash::of(bytes) == record.content_hash) {
    true => IndexState::Current,
    false => IndexState::Changed,
  })
}

struct Scope {
  family: LanguageFamily,
  repo: RepoName,
  excluded: BTreeSet<EntityId>,
  packages: Option<BTreeSet<EntityId>>,
}

impl Scope {
  fn admits(&self, entity: &Entity) -> bool {
    matches!(entity.kind, EntityKind::Function | EntityKind::Method)
      && !self.excluded.contains(&entity.id)
      && !is_test(entity)
      && !is_test_code(entity)
      && language(entity) == Some(self.family)
      && entity
        .path
        .as_ref()
        .is_some_and(|path| !is_test_path(path.as_str()) && !is_test_or_generated_path(path.as_str()))
  }

  fn in_packages(&self, entity: &Entity) -> bool {
    match &self.packages {
      Some(allowed) => package_key(entity).is_some_and(|key| allowed.contains(&EntityId::of(&key))),
      None => entity.repo.as_ref() == Some(&self.repo),
    }
  }
}

fn check(graph: &Graph<'_, '_>, index: &SimilarityIndex, repo: &RepoName, path: &RelPath, prepared: &Prepared) -> QueryResult<Vec<ReuseUnit>> {
  let names: BTreeSet<&str> = prepared.units.iter().map(|unit| unit.name.as_str()).collect();
  let excluded = graph
    .tx()
    .entities_in_file(repo, path, &EntityFilter::default())?
    .into_iter()
    .filter(|entity| {
      let overlaps = entity.span.is_some_and(|span| prepared.old_region.touches(span.start_line, span.end_line));
      overlaps || names.contains(entity.name.as_str())
    })
    .map(|entity| entity.id)
    .collect();
  let packages = PackageIndex::load(graph)?;
  let allowed = match packages.nearest(repo, path.as_str()) {
    Some(place) => {
      let mut allowed: BTreeSet<EntityId> = graph.edges_out(place.id, &[EdgeKind::DependsOnPackage])?.into_iter().map(|edge| edge.to).collect();
      allowed.insert(place.id);
      Some(allowed)
    }
    None => None,
  };
  let scope = Scope {
    family: prepared.family,
    repo: repo.clone(),
    excluded,
    packages: allowed,
  };
  let mut found: Vec<(usize, ReuseCandidate)> = Vec::new();
  for (position, unit) in prepared.units.iter().enumerate() {
    let mut candidates = near_duplicates(graph, index, &scope, unit)?;
    if candidates.len() < PER_UNIT
      && unit.status != UnitStatus::Changed
      && let Some(candidate) = name_match(graph, index, &scope, unit, &candidates)?
    {
      candidates.push(candidate);
    }
    candidates.truncate(PER_UNIT);
    found.extend(candidates.into_iter().map(|candidate| (position, candidate)));
  }
  let strength = |candidate: &ReuseCandidate| candidate.certainty.confidence.unwrap_or(0.0);
  let mut order: Vec<usize> = (0..found.len()).collect();
  order.sort_by(|left, right| strength(&found[*right].1).total_cmp(&strength(&found[*left].1)).then_with(|| left.cmp(right)));
  let kept: BTreeSet<usize> = order.into_iter().take(PER_ANSWER).collect();
  let mut units: Vec<ReuseUnit> = prepared
    .units
    .iter()
    .map(|unit| ReuseUnit {
      name: unit.name.clone(),
      kind: unit.kind.as_str().to_owned(),
      start_line: unit.start_line,
      end_line: unit.end_line,
      tokens: unit.tokens,
      status: unit.status,
      candidates: Vec::new(),
    })
    .collect();
  for (slot, (position, candidate)) in found.into_iter().enumerate() {
    if kept.contains(&slot) {
      units[position].candidates.push(candidate);
    }
  }
  Ok(units)
}

fn candidate(entity: &Entity, certainty: Certainty, reason: ReuseReason) -> ReuseCandidate {
  ReuseCandidate {
    entity: EntityRef::of(entity),
    start_line: entity.span.map_or(0, |span| span.start_line),
    end_line: entity.span.map_or(0, |span| span.end_line),
    signature: entity.signature.clone(),
    certainty: CertaintyTag::of(&certainty),
    reason,
  }
}

fn heuristic(confidence: f32, rule: &str) -> QueryResult<Certainty> {
  let confidence = Confidence::new(confidence).map_err(|error| QueryError::invalid(error.to_string()))?;
  Ok(Certainty::heuristic(confidence, rule))
}

fn near_duplicates(graph: &Graph<'_, '_>, index: &SimilarityIndex, scope: &Scope, unit: &Unit) -> QueryResult<Vec<ReuseCandidate>> {
  let before: BTreeSet<EntityId> = match &unit.previous {
    Some(previous) => index
      .near_duplicates_of_tokens(previous, NEAR_DUPLICATE_FLOOR, PREVIOUS_MATCHES_FETCHED)
      .into_iter()
      .map(|found| found.entity)
      .collect(),
    None => BTreeSet::new(),
  };
  let mut candidates = Vec::new();
  if unit.body.len() < SMALL_BODY_TOKENS {
    return Ok(candidates);
  }
  for found in index.near_duplicates_of_tokens(&unit.body, NEAR_DUPLICATE_FLOOR, NEAR_DUPLICATES_FETCHED) {
    if candidates.len() == PER_UNIT {
      break;
    }
    if before.contains(&found.entity) {
      continue;
    }
    let Some(entity) = graph.entity(found.entity)? else { continue };
    if !scope.admits(&entity) {
      continue;
    }
    let jaccard = (found.exact_jaccard * 100.0).round() / 100.0;
    candidates.push(candidate(&entity, heuristic(jaccard as f32, NEAR_DUPLICATE_RULE)?, ReuseReason::NearDuplicate { jaccard }));
  }
  Ok(candidates)
}

fn name_match(graph: &Graph<'_, '_>, index: &SimilarityIndex, scope: &Scope, unit: &Unit, taken: &[ReuseCandidate]) -> QueryResult<Option<ReuseCandidate>> {
  let options = SearchOptions {
    k: NAME_HITS_FETCHED,
    kinds: vec![EntityKind::Function, EntityKind::Method],
    repos: Vec::new(),
    families: vec![scope.family],
    path_prefix: None,
  };
  for hit in index.search(&unit.query, &options) {
    if !hit.matched_fields.iter().any(|matched| matches!(matched.field, Field::Name | Field::Signature)) || unit.called.contains(&hit.name) {
      continue;
    }
    let Some(entity) = graph.entity(hit.entity)? else { continue };
    if !scope.admits(&entity) || !scope.in_packages(&entity) {
      continue;
    }
    if taken.iter().any(|known| known.entity.id == entity.id.to_string()) {
      return Ok(None);
    }
    let parts: Vec<String> = name_words(&unit.name).intersection(&name_words(&entity.name)).cloned().collect();
    if parts.len() < MIN_SHARED_NAME_PARTS {
      return Ok(None);
    }
    return Ok(Some(candidate(
      &entity,
      heuristic(NAME_MATCH_CONFIDENCE, NAME_PARTS_RULE)?,
      ReuseReason::SameNameParts { parts },
    )));
  }
  Ok(None)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_find_the_changed_lines_on_both_sides_of_an_edit() {
    let old = "a\nb\nc\nd\n";
    assert_eq!(changed_regions(old, "a\nb\nx\ny\nd\n"), (Region { start: 3, end: 3 }, Region { start: 3, end: 4 }));
    assert_eq!(changed_regions(old, "a\nb\nd\n"), (Region { start: 3, end: 3 }, Region { start: 3, end: 2 }));
    assert_eq!(changed_regions(old, "a\nb\nb\nc\nd\n"), (Region { start: 3, end: 2 }, Region { start: 3, end: 3 }));
    assert!(Region { start: 3, end: 2 }.touches(2, 3));
    assert!(!Region { start: 3, end: 2 }.touches(4, 9));
  }

  #[test]
  fn should_read_string_literals_but_not_comments_lifetimes_or_apostrophes() {
    let rust = "fn wait<'a>(header: &'a str) -> u32 {\n  // the server's hint\n  /* isn't it */ parse(header, \"Retry-After\", r\"seconds\")\n}";
    assert_eq!(literals(rust, LanguageFamily::Rust), ["Retry-After", "seconds"]);
    let script = "function wait(header) {\n  // don't guess\n  return parse(header, 'Retry-After', `${unit}s`) / 2;\n}";
    assert_eq!(literals(script, LanguageFamily::Ts), ["Retry-After", "${unit}s"]);
  }
}
