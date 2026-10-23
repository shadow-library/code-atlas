use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use atlas_core::{Certainty, CertaintyClass, EdgeKind, Entity, EntityId, EntityKey, EntityKind, RelPath, RepoName, is_test_path};
use atlas_project::Project;
use atlas_store::{EdgeFilter, EdgeLink, EntityFilter};
use serde::Serialize;
use serde_json::Value;

use crate::QueryEngine;
use crate::error::QueryResult;
use crate::graph::Graph;
use crate::model::{Caveat, CertaintyTag, Location, Response, short_name};
use crate::policy::Policy;
use crate::queries::common::{is_test, locate_file};
use crate::queries::counterparts::{Counterpart, counterparts_of, is_test_or_generated_path};

pub const DEFAULT_ALSO_CHANGE_LIMIT: usize = 6;
const RESTART: f64 = 0.15;
const ITERATIONS: usize = 20;
const MIN_PROPAGATED: f64 = 1e-5;
const MAX_EXPANDED: usize = 20_000;
const MAX_SCORED: usize = 5_000;
const MAX_TRACED_HOPS: usize = 8;
const COUNTERPART_FLOOR: f32 = 0.6;
const AUTHORITATIVE_WEIGHT: f64 = 1.0;
const HEURISTIC_WEIGHT: f64 = 0.5;
const INFERRED_WEIGHT: f64 = 0.25;
const STRUCTURAL: [EdgeKind; 2] = [EdgeKind::Contains, EdgeKind::Defines];
const ATTR_GENERATED: &str = "generated";
const COUNTERPART_REASON: &str = "same contract values";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlsoChangeRequest {
  pub changed: Vec<String>,
  pub limit: usize,
}

impl AlsoChangeRequest {
  pub fn new(changed: impl IntoIterator<Item = impl Into<String>>) -> Self {
    Self {
      changed: changed.into_iter().map(Into::into).collect(),
      limit: DEFAULT_ALSO_CHANGE_LIMIT,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlsoChangeAnswer {
  pub changed: Vec<Location>,
  pub files: Vec<AlsoChangeFile>,
  pub caveats: Vec<Caveat>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlsoChangeFile {
  pub repo: String,
  pub path: String,
  pub score: f64,
  pub reason: String,
  pub certainty: CertaintyTag,
}

impl QueryEngine {
  pub fn also_change(&self, request: &AlsoChangeRequest) -> QueryResult<Response<AlsoChangeAnswer>> {
    let freshness = self.barrier()?;
    let project = self.engine().project();
    let changed = request.changed.iter().map(|spec| locate_file(&project, spec)).collect::<QueryResult<Vec<_>>>()?;
    let answer = self.read(Policy::with_floor(0.0), |graph| also_change_of(graph, &project, &changed, request.limit))?;
    Ok(Response { freshness, answer })
  }
}

type FileKey = (RepoName, RelPath);

fn also_change_of(graph: &Graph<'_, '_>, project: &Project, changed: &[FileKey], limit: usize) -> QueryResult<AlsoChangeAnswer> {
  let mut caveats = Vec::new();
  let mut seeds: BTreeSet<EntityId> = BTreeSet::new();
  for (repo, path) in changed {
    let found = file_entities(graph, repo, path)?;
    if found.is_empty() {
      caveats.push(Caveat::Note {
        message: format!("{repo}:{path} is not indexed, so it seeds nothing"),
      });
    }
    seeds.extend(found);
  }
  let changed_set: BTreeSet<&FileKey> = changed.iter().collect();
  let mut walk = Walk::new(graph);
  let ranked = walk.rank(&seeds)?;
  tracing::debug!(
    seeds = seeds.len(),
    expanded = walk.adjacency.len(),
    ranked = ranked.len(),
    "ranked the graph around the changed files"
  );
  let mut files = FileScores::default();
  let mut excluded = Excluded::default();
  let mut by_mass: Vec<(EntityId, f64)> = ranked.iter().map(|(id, mass)| (*id, *mass)).collect();
  by_mass.sort_by(|left, right| right.1.total_cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
  for (id, mass) in by_mass.into_iter().take(MAX_SCORED) {
    let Some(entity) = walk.entity(id)? else { continue };
    let (Some(repo), Some(path)) = (entity.repo.clone(), entity.path.clone()) else {
      continue;
    };
    let key = (repo, path);
    if changed_set.contains(&key) || is_test(&entity) || excluded.test_or_generated(graph, &key)? {
      continue;
    }
    files.add(key, id, mass);
  }
  let top = files.scores.values().map(|file| file.score).fold(0.0_f64, f64::max);
  let boost = if top > 0.0 { top } else { 1.0 };
  let counterparts = changed_counterparts(graph, project, changed)?;
  let mut boosted: BTreeMap<FileKey, CertaintyTag> = BTreeMap::new();
  for counterpart in counterparts
    .iter()
    .filter(|counterpart| counterpart.certainty.confidence.is_some_and(|confidence| confidence >= COUNTERPART_FLOOR))
  {
    let (Ok(repo), Ok(path)) = (RepoName::new(&counterpart.repo), RelPath::new(&counterpart.path)) else {
      continue;
    };
    let key = (repo, path);
    if changed_set.contains(&key) || boosted.contains_key(&key) || excluded.test_or_generated(graph, &key)? {
      continue;
    }
    files.scores.entry(key.clone()).or_default().score += boost;
    boosted.insert(key, counterpart.certainty.clone());
  }
  let mut ranked_files: Vec<(FileKey, FileScore)> = files.scores.into_iter().collect();
  ranked_files.sort_by(|left, right| {
    right
      .1
      .score
      .total_cmp(&left.1.score)
      .then_with(|| boosted.contains_key(&right.0).cmp(&boosted.contains_key(&left.0)))
      .then_with(|| left.0.cmp(&right.0))
  });
  let best = ranked_files.first().map_or(0.0, |(_, file)| file.score);
  let mut chosen = Vec::new();
  for (key, file) in ranked_files.into_iter().take(limit) {
    let (reason, certainty) = match walk.direct_reason(&seeds, &key)? {
      Some(found) => found,
      None => match boosted.get(&key) {
        Some(certainty) => (COUNTERPART_REASON.to_owned(), certainty.clone()),
        None => walk.traced_reason(&seeds, &key, file.top)?,
      },
    };
    chosen.push(AlsoChangeFile {
      repo: key.0.to_string(),
      path: key.1.to_string(),
      score: ((file.score / best) * 100.0).round() / 100.0,
      reason,
      certainty,
    });
  }
  Ok(AlsoChangeAnswer {
    changed: changed
      .iter()
      .map(|(repo, path)| Location {
        repo: repo.to_string(),
        path: path.to_string(),
        line: None,
        column: None,
      })
      .collect(),
    files: chosen,
    caveats,
  })
}

fn file_entities(graph: &Graph<'_, '_>, repo: &RepoName, path: &RelPath) -> QueryResult<Vec<EntityId>> {
  let mut found: Vec<EntityId> = graph
    .tx()
    .entities_in_file(repo, path, &EntityFilter::default())?
    .into_iter()
    .filter(|entity| entity.path.as_ref() == Some(path))
    .map(|entity| entity.id)
    .collect();
  let file = EntityKey::File {
    repo: repo.clone(),
    path: path.clone(),
  };
  if let Some(file) = graph.tx().entity_by_key(&file)? {
    found.push(file.id);
  }
  found.sort();
  found.dedup();
  Ok(found)
}

fn changed_counterparts(graph: &Graph<'_, '_>, project: &Project, changed: &[FileKey]) -> QueryResult<Vec<Counterpart>> {
  let mut found = Vec::new();
  for (repo, path) in changed {
    found.extend(counterparts_of(graph, project, repo, path)?.counterparts);
  }
  Ok(found)
}

#[derive(Default)]
struct Excluded {
  known: HashMap<FileKey, bool>,
}

impl Excluded {
  fn test_or_generated(&mut self, graph: &Graph<'_, '_>, key: &FileKey) -> QueryResult<bool> {
    if let Some(known) = self.known.get(key) {
      return Ok(*known);
    }
    let path = key.1.as_str();
    let excluded = is_test_or_generated_path(path) || is_test_path(path) || {
      let file = EntityKey::File {
        repo: key.0.clone(),
        path: key.1.clone(),
      };
      graph
        .tx()
        .entity_by_key(&file)?
        .is_some_and(|file| file.attrs.get(ATTR_GENERATED).and_then(Value::as_bool) == Some(true))
    };
    self.known.insert(key.clone(), excluded);
    Ok(excluded)
  }
}

#[derive(Debug, Clone, Copy, Default)]
struct FileScore {
  score: f64,
  top: Option<EntityId>,
  top_mass: f64,
}

#[derive(Default)]
struct FileScores {
  scores: BTreeMap<FileKey, FileScore>,
}

impl FileScores {
  fn add(&mut self, key: FileKey, id: EntityId, mass: f64) {
    let file = self.scores.entry(key).or_default();
    file.score += mass;
    if file.top.is_none() || mass > file.top_mass {
      file.top = Some(id);
      file.top_mass = mass;
    }
  }
}

#[derive(Debug, Clone)]
struct Neighbor {
  node: EntityId,
  weight: f64,
  link: EdgeLink,
}

#[derive(Debug, Clone)]
struct Parent {
  node: EntityId,
  link: EdgeLink,
  share: f64,
}

struct Walk<'g, 'a, 'c> {
  graph: &'g Graph<'a, 'c>,
  adjacency: HashMap<EntityId, Rc<Vec<Neighbor>>>,
  entities: HashMap<EntityId, Option<Rc<Entity>>>,
  parents: BTreeMap<EntityId, Parent>,
}

impl<'g, 'a, 'c> Walk<'g, 'a, 'c> {
  fn new(graph: &'g Graph<'a, 'c>) -> Self {
    Self {
      graph,
      adjacency: HashMap::new(),
      entities: HashMap::new(),
      parents: BTreeMap::new(),
    }
  }

  fn neighbors(&mut self, id: EntityId) -> QueryResult<Option<Rc<Vec<Neighbor>>>> {
    if let Some(known) = self.adjacency.get(&id) {
      return Ok(Some(Rc::clone(known)));
    }
    if self.adjacency.len() >= MAX_EXPANDED {
      return Ok(None);
    }
    let filter = EdgeFilter::default();
    let tx = self.graph.tx();
    let mut found = Vec::new();
    for link in tx.edge_links_from(id, &filter)?.into_iter().chain(tx.edge_links_to(id, &filter)?) {
      if STRUCTURAL.contains(&link.kind) || link.from == link.to {
        continue;
      }
      let node = if link.from == id { link.to } else { link.from };
      found.push(Neighbor {
        node,
        weight: weight(link.certainty_class),
        link,
      });
    }
    let found = Rc::new(found);
    self.adjacency.insert(id, Rc::clone(&found));
    Ok(Some(found))
  }

  fn rank(&mut self, seeds: &BTreeSet<EntityId>) -> QueryResult<BTreeMap<EntityId, f64>> {
    if seeds.is_empty() {
      return Ok(BTreeMap::new());
    }
    let restart = 1.0 / seeds.len() as f64;
    let mut rank: BTreeMap<EntityId, f64> = seeds.iter().map(|seed| (*seed, restart)).collect();
    for _ in 0..ITERATIONS {
      let mut next: BTreeMap<EntityId, f64> = seeds.iter().map(|seed| (*seed, RESTART * restart)).collect();
      let mut parents: BTreeMap<EntityId, Parent> = BTreeMap::new();
      let mut dangling = 0.0;
      for (node, mass) in &rank {
        if *mass < MIN_PROPAGATED {
          continue;
        }
        let spread = (1.0 - RESTART) * mass;
        let neighbors = match self.neighbors(*node)? {
          Some(neighbors) if !neighbors.is_empty() => neighbors,
          _ => {
            dangling += spread;
            continue;
          }
        };
        let total: f64 = neighbors.iter().map(|neighbor| neighbor.weight).sum();
        for neighbor in neighbors.iter() {
          let share = spread * neighbor.weight / total;
          *next.entry(neighbor.node).or_default() += share;
          if parents.get(&neighbor.node).is_none_or(|parent| share > parent.share) {
            parents.insert(
              neighbor.node,
              Parent {
                node: *node,
                link: neighbor.link.clone(),
                share,
              },
            );
          }
        }
      }
      for seed in seeds {
        *next.entry(*seed).or_default() += dangling * restart;
      }
      rank = next;
      self.parents = parents;
    }
    Ok(rank)
  }

  fn entity(&mut self, id: EntityId) -> QueryResult<Option<Rc<Entity>>> {
    if let Some(known) = self.entities.get(&id) {
      return Ok(known.clone());
    }
    let found = self.graph.entity(id)?.map(Rc::new);
    self.entities.insert(id, found.clone());
    Ok(found)
  }

  fn file_of(&mut self, id: EntityId) -> QueryResult<Option<FileKey>> {
    Ok(self.entity(id)?.and_then(|entity| Some((entity.repo.clone()?, entity.path.clone()?))))
  }

  fn direct_reason(&mut self, seeds: &BTreeSet<EntityId>, key: &FileKey) -> QueryResult<Option<(String, CertaintyTag)>> {
    let mut best: Option<(f64, usize, EdgeLink, EntityId)> = None;
    for seed in seeds {
      let Some(neighbors) = self.neighbors(*seed)? else { continue };
      for neighbor in neighbors.iter() {
        if self.file_of(neighbor.node)?.as_ref() != Some(key) {
          continue;
        }
        let rank = (neighbor.weight, kind_priority(&neighbor.link.kind));
        let better = best
          .as_ref()
          .is_none_or(|(weight, priority, link, _)| rank.0 > *weight || (rank.0 == *weight && (rank.1 < *priority || (rank.1 == *priority && neighbor.link.id < link.id))));
        if better {
          best = Some((rank.0, rank.1, neighbor.link.clone(), *seed));
        }
      }
    }
    let Some((_, _, link, seed)) = best else { return Ok(None) };
    let Some(changed) = self.entity(seed)? else { return Ok(None) };
    let certainty = self.certainty(&link)?;
    Ok(Some((hop_text(&link, &changed), certainty)))
  }

  fn traced_reason(&mut self, seeds: &BTreeSet<EntityId>, key: &FileKey, top: Option<EntityId>) -> QueryResult<(String, CertaintyTag)> {
    let mut hops: Vec<(EntityId, EdgeLink)> = Vec::new();
    let mut current = top;
    while let Some(node) = current.filter(|node| !seeds.contains(node)) {
      let Some(parent) = self.parents.get(&node).cloned() else { break };
      if hops.len() == MAX_TRACED_HOPS || hops.iter().any(|(seen, _)| *seen == parent.node) {
        break;
      }
      hops.push((parent.node, parent.link));
      current = Some(parent.node);
    }
    let reached = current.is_some_and(|node| seeds.contains(&node));
    let mut weakest = CertaintyTag::of(&Certainty::Resolved);
    for (_, link) in &hops {
      let tag = self.certainty(link)?;
      if !tag.authoritative && weakest.confidence.is_none_or(|confidence| tag.confidence.is_some_and(|other| other < confidence)) {
        weakest = tag;
      }
    }
    let mut via = None;
    for (node, _) in hops.iter().filter(|(node, _)| !seeds.contains(node)) {
      let file = self.file_of(*node)?;
      if file.as_ref().is_some_and(|file| file != key) {
        via = file;
        break;
      }
    }
    let text = match (reached, via) {
      (true, Some((repo, path))) => format!("via {repo}:{path}, {} hops from a changed file", hops.len()),
      _ => "near the changed files in the graph".to_owned(),
    };
    Ok((text, weakest))
  }

  fn certainty(&self, link: &EdgeLink) -> QueryResult<CertaintyTag> {
    Ok(match self.graph.edge(link.id)? {
      Some(edge) => CertaintyTag::of(&edge.certainty),
      None => CertaintyTag::of(&Certainty::Resolved),
    })
  }
}

fn weight(class: CertaintyClass) -> f64 {
  match class {
    CertaintyClass::Heuristic => HEURISTIC_WEIGHT,
    CertaintyClass::Inferred => INFERRED_WEIGHT,
    CertaintyClass::CompilerVerified | CertaintyClass::Syntactic | CertaintyClass::Resolved | CertaintyClass::Declared => AUTHORITATIVE_WEIGHT,
  }
}

fn kind_priority(kind: &EdgeKind) -> usize {
  [
    EdgeKind::Calls,
    EdgeKind::Instantiates,
    EdgeKind::CallsEndpoint,
    EdgeKind::ExposesEndpoint,
    EdgeKind::Implements,
    EdgeKind::Extends,
    EdgeKind::References,
    EdgeKind::TypeOf,
    EdgeKind::Imports,
    EdgeKind::ReExports,
  ]
  .iter()
  .position(|known| known == kind)
  .unwrap_or(usize::MAX)
}

fn hop_text(link: &EdgeLink, changed: &Entity) -> String {
  let label = match link.to == changed.id {
    true => link.kind.forward_label(),
    false => link.kind.reverse_label(),
  };
  let kind = match changed.kind {
    EntityKind::File => "file",
    ref other => other.as_str(),
  };
  format!("{label} changed {kind} {}", short_name(&changed.key))
}
