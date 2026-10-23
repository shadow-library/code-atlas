use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{Certainty, Confidence, EdgeKind, Entity, EntityId, EntityKey, EntityKind, LanguageFamily, Owner, RelPath, RepoName};
use atlas_project::Project;
use atlas_store::{EntityFilter, FileRecord};
use rayon::prelude::*;
use serde::Serialize;
use serde_json::Value;

use crate::QueryEngine;
use crate::error::{QueryError, QueryResult};
use crate::freshness::FreshnessState;
use crate::graph::Graph;
use crate::model::{Caveat, CertaintyTag, Location, Response, Targeted, short_name};
use crate::policy::Policy;
use crate::queries::common::resolve_one;
use crate::target::TargetFilter;

use self::literals::{Syntax, Tokens, contract_tokens};

mod literals;

const MIN_SCORE: f64 = 25.0;
const MIN_SHARED: usize = 3;
const MAX_COUNTERPARTS: usize = 2;
const RUNNER_UP_SHARE: f64 = 0.6;
const RARE_SHARE: f64 = 0.02;
const MIN_RARE_CEILING: usize = 3;
const RAREST_SHOWN: usize = 5;
const SYMBOLS_SHOWN: usize = 3;
const ONLY_THERE_SHOWN: usize = 8;
const MIN_BLOCK_SHARED: usize = 2;
const NAMESPACE_FACTOR: usize = 4;
const MAX_LINE_CHARS: usize = 140;
const CONFIDENCE_CAP: f64 = 0.9;
const CONFIDENCE_HALF: f64 = 40.0;
const RULE: &str = "shared-contract-values";
const ATTR_MANIFEST: &str = "manifest";
const ATTR_GENERATED: &str = "generated";
const TEST_DIRS: [&str; 8] = ["tests", "test", "__tests__", "e2e", "testing", "fixtures", "mocks", "__mocks__"];
const TEST_SUFFIXES: [&str; 3] = [".spec.", ".test.", ".gen."];
const RUST_EXTENSION: &str = "rs";
const SCRIPT_EXTENSIONS: [&str; 8] = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

#[derive(Debug, Clone, PartialEq)]
pub struct CounterpartsRequest {
  pub target: String,
  pub filter: TargetFilter,
  pub require_fresh: bool,
}

impl CounterpartsRequest {
  pub fn new(target: impl Into<String>) -> Self {
    Self {
      target: target.into(),
      filter: TargetFilter::default(),
      require_fresh: false,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CounterpartsAnswer {
  pub file: Location,
  pub counterparts: Vec<Counterpart>,
  pub caveats: Vec<Caveat>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Counterpart {
  pub repo: String,
  pub path: String,
  pub absolute_path: String,
  pub score: f64,
  pub certainty: CertaintyTag,
  pub shared: SharedValues,
  pub symbols: Vec<CounterpartSymbol>,
  pub only_there: Vec<OnlyThere>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SharedValues {
  pub count: usize,
  pub rarest: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CounterpartSymbol {
  pub name: String,
  pub kind: String,
  pub start_line: u32,
  pub end_line: u32,
  pub shared_lines: usize,
}

impl CounterpartSymbol {
  pub fn located(&self) -> String {
    match self.start_line == self.end_line {
      true => format!("{} {}", self.name, self.start_line),
      false => format!("{} {}-{}", self.name, self.start_line, self.end_line),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OnlyThere {
  pub token: String,
  pub line: u32,
  pub text: String,
}

#[derive(Default)]
pub(crate) struct Found {
  pub counterparts: Vec<Counterpart>,
  pub caveats: Vec<Caveat>,
}

impl Found {
  fn noted(message: String) -> Self {
    Self {
      counterparts: Vec::new(),
      caveats: vec![Caveat::Note { message }],
    }
  }

  fn unanswerable(repo: &RepoName, path: &RelPath) -> Self {
    Self::noted(format!(
      "{repo}:{path} is a test, generated, unreadable or oversized file, or not Rust or TypeScript: it has no counterparts"
    ))
  }
}

impl QueryEngine {
  pub fn counterparts(&self, request: &CounterpartsRequest) -> QueryResult<Response<Targeted<CounterpartsAnswer>>> {
    let freshness = self.barrier()?;
    let skip = request.require_fresh && freshness.state != FreshnessState::Fresh;
    let project = self.engine().project();
    let answer = self.read(Policy::with_floor(0.0), |graph| -> QueryResult<Targeted<CounterpartsAnswer>> {
      let target = match resolve_one(self, graph, &request.target, &request.filter)? {
        Ok(target) => target,
        Err(outcome) => return Ok(outcome),
      };
      let Some((repo, path)) = file_of(&target) else {
        return Ok(Targeted::NotFound {
          spec: request.target.clone(),
          hint: format!("{} is not in a file; counterparts need a file or a symbol defined in one", short_name(&target.key)),
        });
      };
      let found = match skip {
        true => Found::noted("the index is not fresh, so the counterparts were not computed".to_owned()),
        false => counterparts_of(graph, &project, &repo, &path)?,
      };
      Ok(Targeted::Found(CounterpartsAnswer {
        file: Location {
          repo: repo.to_string(),
          path: path.to_string(),
          line: None,
          column: None,
        },
        counterparts: found.counterparts,
        caveats: found.caveats,
      }))
    })?;
    Ok(Response { freshness, answer })
  }
}

fn file_of(entity: &Entity) -> Option<(RepoName, RelPath)> {
  match (&entity.key, &entity.repo, &entity.path) {
    (EntityKey::File { repo, path }, _, _) => Some((repo.clone(), path.clone())),
    (_, Some(repo), Some(path)) => Some((repo.clone(), path.clone())),
    _ => None,
  }
}

pub fn is_test_or_generated_path(path: &str) -> bool {
  let mut segments = path.split('/');
  let name = segments.next_back().unwrap_or_default();
  segments.any(|segment| TEST_DIRS.contains(&segment)) || is_test_or_generated_name(name)
}

fn is_test_or_generated_name(name: &str) -> bool {
  TEST_SUFFIXES.iter().any(|suffix| name.contains(suffix)) || name.ends_with("_test.rs")
}

pub fn may_have_counterparts(file_name: &str) -> bool {
  let scanned = file_name
    .rsplit_once('.')
    .is_some_and(|(_, extension)| extension == RUST_EXTENSION || SCRIPT_EXTENSIONS.contains(&extension));
  scanned && !is_test_or_generated_name(file_name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackagePlace {
  pub(crate) id: EntityId,
  dir: String,
  app: bool,
}

#[derive(Debug, Default)]
pub(crate) struct PackageIndex {
  places: BTreeMap<RepoName, Vec<PackagePlace>>,
}

impl PackageIndex {
  pub(crate) fn load(graph: &Graph<'_, '_>) -> QueryResult<Self> {
    let mut found: BTreeMap<RepoName, BTreeMap<String, (EntityId, bool)>> = BTreeMap::new();
    for package in graph.tx().entities_by_kind(&EntityKind::Package, &EntityFilter::default())? {
      let app = !graph.links_out(package.id, &[EdgeKind::PartOfApp])?.is_empty();
      for assertion in graph.tx().entity_assertions(package.id)? {
        let Owner::File { repo, path } = assertion.owner else { continue };
        if assertion.fact.attrs.get(ATTR_MANIFEST).and_then(Value::as_str) != Some(path.as_str()) {
          continue;
        }
        let dir = path.as_str().rsplit_once('/').map_or("", |(dir, _)| dir).to_owned();
        found.entry(repo).or_default().entry(dir).or_insert((package.id, false)).1 |= app;
      }
    }
    let places = found
      .into_iter()
      .map(|(repo, dirs)| {
        let mut places: Vec<PackagePlace> = dirs.into_iter().map(|(dir, (id, app))| PackagePlace { id, dir, app }).collect();
        places.sort_by(|left, right| right.dir.len().cmp(&left.dir.len()).then_with(|| left.dir.cmp(&right.dir)));
        (repo, places)
      })
      .collect();
    Ok(Self { places })
  }

  pub(crate) fn nearest(&self, repo: &RepoName, path: &str) -> Option<&PackagePlace> {
    self.places.get(repo)?.iter().find(|place| contains(&place.dir, path))
  }

  fn has_several(&self, repo: &RepoName) -> bool {
    self.places.get(repo).is_some_and(|places| places.len() > 1)
  }
}

fn contains(dir: &str, path: &str) -> bool {
  dir.is_empty() || path.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relation {
  Scope,
  Candidate,
  Unrelated,
}

struct Place<'p> {
  repo: &'p RepoName,
  path: &'p str,
  package: Option<&'p PackagePlace>,
}

fn relation(target: &Place<'_>, other: &Place<'_>, several_packages: bool) -> Relation {
  if other.repo != target.repo {
    return Relation::Candidate;
  }
  if !several_packages {
    return Relation::Scope;
  }
  match (target.package, other.package) {
    (mine, theirs) if mine.map(|place| &place.dir) == theirs.map(|place| &place.dir) => Relation::Scope,
    (_, Some(theirs)) if theirs.app || !contains(&theirs.dir, target.path) => Relation::Candidate,
    _ => Relation::Unrelated,
  }
}

struct Listed {
  repo: RepoName,
  path: RelPath,
  absolute: PathBuf,
  syntax: Syntax,
}

struct Scanned {
  file: Listed,
  ids: Vec<u32>,
}

#[derive(Default)]
struct Interner {
  ids: HashMap<String, u32>,
  names: Vec<String>,
}

impl Interner {
  fn intern(&mut self, token: String) -> u32 {
    if let Some(id) = self.ids.get(&token) {
      return *id;
    }
    let id = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
    self.names.push(token.clone());
    self.ids.insert(token, id);
    id
  }

  fn id(&self, token: &str) -> Option<u32> {
    self.ids.get(token).copied()
  }

  fn name(&self, id: u32) -> &str {
    self.names.get(id as usize).map_or("", String::as_str)
  }
}

fn syntax_of(record: &FileRecord) -> Option<Syntax> {
  let extension = record.path.as_str().rsplit_once('.').map(|(_, extension)| extension)?;
  match record.language.map(|language| language.family()) {
    Some(LanguageFamily::Rust) if extension == RUST_EXTENSION => Some(Syntax::Rust),
    Some(LanguageFamily::Ts) if SCRIPT_EXTENSIONS.contains(&extension) => Some(Syntax::Script),
    _ => None,
  }
}

fn is_generated(graph: &Graph<'_, '_>, repo: &RepoName, path: &RelPath) -> QueryResult<bool> {
  let key = EntityKey::File {
    repo: repo.clone(),
    path: path.clone(),
  };
  let file = graph.tx().entity_by_key(&key)?;
  Ok(file.is_some_and(|file| file.attrs.get(ATTR_GENERATED).and_then(Value::as_bool) == Some(true)))
}

fn generated_files(graph: &Graph<'_, '_>) -> QueryResult<HashSet<(RepoName, RelPath)>> {
  Ok(
    graph
      .tx()
      .entities_by_kind(&EntityKind::File, &EntityFilter::default())?
      .into_iter()
      .filter(|file| file.attrs.get(ATTR_GENERATED).and_then(Value::as_bool) == Some(true))
      .filter_map(|file| match file.key {
        EntityKey::File { repo, path } => Some((repo, path)),
        _ => None,
      })
      .collect(),
  )
}

fn target_file(graph: &Graph<'_, '_>, project: &Project, repo: &RepoName, path: &RelPath) -> QueryResult<Option<Listed>> {
  let Some(config) = project.repos.iter().find(|config| config.name == *repo) else {
    return Ok(None);
  };
  let Some(record) = graph.tx().file(repo, path)? else { return Ok(None) };
  let Some(syntax) = syntax_of(&record) else { return Ok(None) };
  if record.size > project.settings.max_file_bytes || is_test_or_generated_path(path.as_str()) || is_generated(graph, repo, path)? {
    return Ok(None);
  }
  Ok(Some(Listed {
    absolute: config.path.join(path.as_str()),
    repo: record.repo,
    path: record.path,
    syntax,
  }))
}

fn read_text(path: &Path, max_bytes: u64) -> Option<String> {
  let bytes = fs::read(path).ok()?;
  (bytes.len() as u64 <= max_bytes).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

pub(crate) fn counterparts_of(graph: &Graph<'_, '_>, project: &Project, repo: &RepoName, path: &RelPath) -> QueryResult<Found> {
  let max_bytes = project.settings.max_file_bytes;
  let Some(target) = target_file(graph, project, repo, path)? else {
    return Ok(Found::unanswerable(repo, path));
  };
  let Some(text) = read_text(&target.absolute, max_bytes) else {
    return Ok(Found::unanswerable(repo, path));
  };
  let target_tokens = contract_tokens(&text, target.syntax);
  if target_tokens.len() < MIN_SHARED {
    return Ok(Found::default());
  }
  let generated = generated_files(graph)?;
  let packages = PackageIndex::load(graph)?;
  let mut listed = Vec::new();
  for config in &project.repos {
    for record in graph.tx().files_in_repo(&config.name)? {
      let Some(syntax) = syntax_of(&record) else { continue };
      let excluded = record.size > max_bytes || is_test_or_generated_path(record.path.as_str()) || generated.contains(&(record.repo.clone(), record.path.clone()));
      if excluded || (record.repo == *repo && record.path == *path) {
        continue;
      }
      listed.push(Listed {
        absolute: config.path.join(record.path.as_str()),
        repo: record.repo,
        path: record.path,
        syntax,
      });
    }
  }
  let tokens: Vec<Option<Tokens>> = listed
    .par_iter()
    .map(|file| read_text(&file.absolute, max_bytes).map(|text| contract_tokens(&text, file.syntax)))
    .collect();
  let mut interner = Interner::default();
  let scanned: Vec<Scanned> = std::iter::once((target, Some(target_tokens)))
    .chain(listed.into_iter().zip(tokens))
    .filter_map(|(file, tokens)| {
      let mut ids: Vec<u32> = tokens?.into_keys().map(|token| interner.intern(token)).collect();
      ids.sort_unstable();
      Some(Scanned { file, ids })
    })
    .collect();
  let target = 0;
  let mut df = vec![0usize; interner.names.len()];
  for file in &scanned {
    for id in &file.ids {
      df[*id as usize] += 1;
    }
  }
  let total = scanned.len();
  let ceiling = ((total as f64 * RARE_SHARE).ceil() as usize).saturating_sub(1).max(MIN_RARE_CEILING);
  let weight = |id: u32| (total as f64 / df[id as usize] as f64).ln();
  let rare: HashSet<u32> = scanned[target].ids.iter().copied().filter(|id| (2..=ceiling).contains(&df[*id as usize])).collect();
  let target_file = &scanned[target].file;
  let target_place = Place {
    repo: &target_file.repo,
    path: target_file.path.as_str(),
    package: packages.nearest(&target_file.repo, target_file.path.as_str()),
  };
  let several = packages.has_several(&target_file.repo);
  let relation_of = |file: &Scanned| {
    let other = Place {
      repo: &file.file.repo,
      path: file.file.path.as_str(),
      package: packages.nearest(&file.file.repo, file.file.path.as_str()),
    };
    relation(&target_place, &other, several)
  };
  let mut ranked: Vec<(f64, usize, Vec<u32>)> = scanned
    .iter()
    .enumerate()
    .filter(|(_, file)| relation_of(file) == Relation::Candidate)
    .filter_map(|(index, file)| {
      let shared: Vec<u32> = file.ids.iter().copied().filter(|id| rare.contains(id)).collect();
      (shared.len() >= MIN_SHARED).then(|| (shared.iter().map(|id| weight(*id)).sum::<f64>(), index, shared))
    })
    .collect();
  let place = |index: usize| (&scanned[index].file.repo, &scanned[index].file.path);
  ranked.sort_by(|left, right| right.0.total_cmp(&left.0).then_with(|| place(left.1).cmp(&place(right.1))));
  let best = ranked.first().map_or(0.0, |(score, _, _)| *score);
  if best < MIN_SCORE {
    return Ok(Found::default());
  }
  let scope: HashSet<u32> = scanned
    .iter()
    .filter(|file| relation_of(file) == Relation::Scope)
    .flat_map(|file| file.ids.iter().copied())
    .collect();
  let mut counterparts = Vec::new();
  for (score, index, shared) in ranked.into_iter().take(MAX_COUNTERPARTS).take_while(|(score, _, _)| *score >= best * RUNNER_UP_SHARE) {
    let mut by_rarity = shared.clone();
    by_rarity.sort_by_key(|id| (df[*id as usize], interner.name(*id)));
    let file = &scanned[index].file;
    let shared_names: HashSet<&str> = shared.iter().map(|id| interner.name(*id)).collect();
    let detail = read_text(&file.absolute, max_bytes).map(|text| Detail::of(&text, file.syntax, &shared_names, &|token| interner.id(token).is_some_and(|id| scope.contains(&id))));
    let symbols = match &detail {
      Some(detail) => symbols(graph, file, &detail.shared_lines)?,
      None => Vec::new(),
    };
    let rounded = (score * 100.0).round() / 100.0;
    counterparts.push(Counterpart {
      repo: file.repo.to_string(),
      path: file.path.to_string(),
      absolute_path: file.absolute.display().to_string(),
      score: rounded,
      certainty: CertaintyTag::of(&certainty(score)?),
      shared: SharedValues {
        count: shared.len(),
        rarest: by_rarity.iter().take(RAREST_SHOWN).map(|id| interner.name(*id).to_owned()).collect(),
      },
      symbols,
      only_there: detail.map(|detail| detail.only_there).unwrap_or_default(),
    });
  }
  Ok(Found {
    counterparts,
    caveats: Vec::new(),
  })
}

fn certainty(score: f64) -> QueryResult<Certainty> {
  let confidence = ((score / (score + CONFIDENCE_HALF)).min(CONFIDENCE_CAP) * 100.0).round() / 100.0;
  let confidence = Confidence::new(confidence as f32).map_err(|error| QueryError::invalid(error.to_string()))?;
  Ok(Certainty::heuristic(confidence, RULE))
}

struct Detail {
  shared_lines: BTreeSet<u32>,
  only_there: Vec<OnlyThere>,
}

impl Detail {
  fn of(text: &str, syntax: Syntax, shared: &HashSet<&str>, in_scope: &dyn Fn(&str) -> bool) -> Self {
    let lines: Vec<&str> = text.lines().collect();
    let blocks = blocks(&lines);
    let tokens = contract_tokens(text, syntax);
    let block_of = |line: u32| blocks.get((line as usize).wrapping_sub(1)).copied().flatten();
    let mut block_shared: BTreeMap<usize, usize> = BTreeMap::new();
    let mut block_namespaces: BTreeMap<usize, BTreeSet<&str>> = BTreeMap::new();
    let mut shared_lines = BTreeSet::new();
    for (token, token_lines) in tokens.iter().filter(|(token, _)| shared.contains(token.as_str())) {
      for line in token_lines {
        shared_lines.insert(*line);
        if let Some(block) = block_of(*line) {
          *block_shared.entry(block).or_default() += 1;
          block_namespaces.entry(block).or_default().insert(namespace(token));
        }
      }
    }
    let mut best: BTreeMap<&str, (usize, u32)> = BTreeMap::new();
    for (token, token_lines) in tokens.iter().filter(|(token, _)| !shared.contains(token.as_str()) && !in_scope(token)) {
      for line in token_lines {
        let Some(block) = block_of(*line) else { continue };
        let count = block_shared.get(&block).copied().unwrap_or(0);
        if count < MIN_BLOCK_SHARED {
          continue;
        }
        let factor = match block_namespaces.get(&block).is_some_and(|namespaces| namespaces.contains(namespace(token))) {
          true => NAMESPACE_FACTOR,
          false => 1,
        };
        let rank = (count * factor, *line);
        let entry = best.entry(token.as_str()).or_insert(rank);
        if rank.0 > entry.0 {
          *entry = rank;
        }
      }
    }
    let mut ranked: Vec<(usize, u32, &str)> = best.into_iter().map(|(token, (rank, line))| (rank, line, token)).collect();
    ranked.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)).then_with(|| left.2.cmp(right.2)));
    let mut taken = BTreeSet::new();
    let only_there = ranked
      .into_iter()
      .filter(|(_, line, _)| taken.insert(*line))
      .take(ONLY_THERE_SHOWN)
      .map(|(_, line, token)| OnlyThere {
        token: token.to_owned(),
        line,
        text: clip(lines.get((line as usize).saturating_sub(1)).copied().unwrap_or_default().trim()),
      })
      .collect();
    Self { shared_lines, only_there }
  }
}

fn blocks(lines: &[&str]) -> Vec<Option<usize>> {
  let mut block = 0;
  let mut previous_blank = true;
  lines
    .iter()
    .map(|line| {
      let blank = line.trim().is_empty();
      if !blank && previous_blank {
        block += 1;
      }
      previous_blank = blank;
      (!blank).then_some(block)
    })
    .collect()
}

fn namespace(token: &str) -> &str {
  token.split(['.', '_', ':']).next().unwrap_or(token)
}

fn clip(line: &str) -> String {
  match line.char_indices().nth(MAX_LINE_CHARS) {
    Some((end, _)) => format!("{}…", &line[..end]),
    None => line.to_owned(),
  }
}

fn symbols(graph: &Graph<'_, '_>, file: &Listed, shared_lines: &BTreeSet<u32>) -> QueryResult<Vec<CounterpartSymbol>> {
  let mut holding: Vec<(usize, u32, u32, Entity)> = graph
    .tx()
    .entities_in_file(&file.repo, &file.path, &EntityFilter::default())?
    .into_iter()
    .filter(|entity| entity.kind.is_code_symbol() && entity.kind != EntityKind::Module && entity.path.as_ref() == Some(&file.path))
    .filter_map(|entity| {
      let span = entity.span?;
      let count = shared_lines.range(span.start_line..=span.end_line).count();
      (count > 0).then_some((count, span.start_line, span.end_line, entity))
    })
    .collect();
  holding.sort_by(|left, right| {
    right
      .0
      .cmp(&left.0)
      .then_with(|| left.2.saturating_sub(left.1).cmp(&right.2.saturating_sub(right.1)))
      .then_with(|| left.1.cmp(&right.1))
      .then_with(|| left.3.key.cmp(&right.3.key))
  });
  let mut picked: Vec<CounterpartSymbol> = Vec::new();
  for (count, start, end, entity) in holding {
    if picked.len() == SYMBOLS_SHOWN {
      break;
    }
    let overlaps = picked.iter().any(|symbol| start <= symbol.end_line && symbol.start_line <= end);
    if !overlaps {
      picked.push(CounterpartSymbol {
        name: short_name(&entity.key),
        kind: entity.kind.as_str().to_owned(),
        start_line: start,
        end_line: end,
        shared_lines: count,
      });
    }
  }
  Ok(picked)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn repo(name: &str) -> RepoName {
    RepoName::new(name).unwrap()
  }

  fn at<'p>(repo: &'p RepoName, path: &'p str, package: Option<&'p PackagePlace>) -> Place<'p> {
    Place { repo, path, package }
  }

  fn package(dir: &str, app: bool) -> PackagePlace {
    PackagePlace {
      id: EntityId::from_i64(0),
      dir: dir.to_owned(),
      app,
    }
  }

  #[test]
  fn should_scope_a_file_to_its_package_and_take_other_repos_apps_and_sibling_members_as_candidates() {
    let (mono, other) = (repo("mono"), repo("other"));
    let (web, api, kit, root) = (package("apps/web", true), package("apps/api", true), package("libs/kit", false), package("", false));
    let target = at(&mono, "apps/web/src/ops.ts", Some(&web));
    assert_eq!(relation(&target, &at(&other, "src/x.ts", None), true), Relation::Candidate);
    assert_eq!(relation(&target, &at(&mono, "apps/web/src/y.ts", Some(&web)), true), Relation::Scope);
    assert_eq!(relation(&target, &at(&mono, "apps/api/src/y.ts", Some(&api)), true), Relation::Candidate);
    assert_eq!(relation(&target, &at(&mono, "libs/kit/src/y.ts", Some(&kit)), true), Relation::Candidate);
    assert_eq!(relation(&target, &at(&mono, "scripts/y.ts", Some(&root)), true), Relation::Unrelated);
    assert_eq!(relation(&target, &at(&mono, "src/y.ts", None), true), Relation::Unrelated);
    assert_eq!(
      relation(&at(&mono, "scripts/y.ts", Some(&root)), &at(&mono, "apps/web/src/y.ts", Some(&web)), true),
      Relation::Candidate
    );
    assert_eq!(relation(&target, &at(&mono, "apps/web/src/y.ts", None), false), Relation::Scope);
  }

  #[test]
  fn should_recognise_test_and_generated_paths() {
    for path in [
      "tests/a.rs",
      "src/tests/a.ts",
      "src/__tests__/a.ts",
      "e2e/lib/a.ts",
      "src/testing/a.ts",
      "src/fixtures/a.ts",
      "src/__mocks__/a.ts",
      "test/a.ts",
      "src/a.spec.ts",
      "src/a.test.tsx",
      "src/a_test.rs",
      "src/api.gen.ts",
    ] {
      assert!(is_test_or_generated_path(path), "{path}");
    }
    for path in ["src/testing.ts", "src/contest/a.ts", "src/specs.ts", "src/a.rs"] {
      assert!(!is_test_or_generated_path(path), "{path}");
    }
  }

  #[test]
  fn should_tell_from_a_file_name_alone_whether_it_may_have_counterparts() {
    for name in ["a.rs", "a.ts", "a.tsx", "a.mjs", "a.cjs", "specs.ts"] {
      assert!(may_have_counterparts(name), "{name}");
    }
    for name in ["a.md", "a.json", "Makefile", "a.spec.ts", "a.test.tsx", "a_test.rs", "api.gen.ts", "a.rs.bak"] {
      assert!(!may_have_counterparts(name), "{name}");
    }
  }

  #[test]
  fn should_list_only_values_absent_from_the_scope_next_to_shared_ones_ranked_by_namespace() {
    let text = "export const SPECS = {\n  'op.keep': { a_field: 1 },\n  'op.send': { b_field: 2 },\n  'op.merge': { c_field: 3 },\n};\n\nconst lone = 'op.unrelated';\n";
    let shared: HashSet<&str> = ["op.keep", "op.send"].into_iter().collect();
    let detail = Detail::of(text, Syntax::Script, &shared, &|token| token == "a_field");
    assert_eq!(detail.shared_lines, BTreeSet::from([2, 3]));
    assert_eq!(
      detail.only_there,
      [
        OnlyThere {
          token: "op.merge".to_owned(),
          line: 4,
          text: "'op.merge': { c_field: 3 },".to_owned(),
        },
        OnlyThere {
          token: "b_field".to_owned(),
          line: 3,
          text: "'op.send': { b_field: 2 },".to_owned(),
        },
      ]
    );
  }

  #[test]
  fn should_clip_long_lines_on_a_char_boundary() {
    let long = "é".repeat(MAX_LINE_CHARS + 5);
    assert_eq!(clip(&long), format!("{}…", "é".repeat(MAX_LINE_CHARS)));
    assert_eq!(clip("short"), "short");
  }

  #[test]
  fn should_cap_confidence_at_point_nine() {
    assert_eq!(certainty(40.0).unwrap().tag(), "[heuristic 0.50]");
    assert_eq!(certainty(10_000.0).unwrap().tag(), "[heuristic 0.90]");
  }
}
