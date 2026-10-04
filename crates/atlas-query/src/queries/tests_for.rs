use std::collections::BTreeMap;

use atlas_core::{ATTR_TEST, EdgeKind, Entity, EntityId, EntityKey, LanguageFamily, TestInfo};
use atlas_store::EntityFilter;
use serde::Serialize;

use crate::QueryEngine;
use crate::engine::unresolved_risk;
use crate::error::QueryResult;
use crate::graph::{Graph, Step, WalkSpec};
use crate::model::{Caveat, CertaintyTag, EntityRef, Hop, Listing, Response, Targeted};
use crate::policy::{Direction, Policy};
use crate::queries::common::{DEFAULT_LIMIT, MAX_NODES, file_entity, is_test, resolve_one, sibling_test_paths};
use crate::queries::traverse::risk_name;
use crate::target::TargetFilter;

const TEST_DEPTH: u32 = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct TestsForRequest {
  pub target: String,
  pub filter: TargetFilter,
  pub limit: usize,
  pub min_confidence: Option<f32>,
}

impl TestsForRequest {
  pub fn new(target: impl Into<String>) -> Self {
    Self {
      target: target.into(),
      filter: TargetFilter::default(),
      limit: DEFAULT_LIMIT,
      min_confidence: None,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TestReach {
  DirectCall,
  CoversFile,
  ImportedByTestFile,
  CallGraph,
  Colocated,
}

impl TestReach {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::DirectCall => "direct",
      Self::CoversFile => "covers file",
      Self::ImportedByTestFile => "test file imports it",
      Self::CallGraph => "via call graph",
      Self::Colocated => "colocated",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TestItem {
  pub test: EntityRef,
  pub reach: TestReach,
  pub depth: u32,
  pub relation: Hop,
  pub path: Vec<Hop>,
  pub uncertain: bool,
  pub run: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TestsForAnswer {
  pub target: EntityRef,
  pub tests: Listing<TestItem>,
  pub caveats: Vec<Caveat>,
}

impl QueryEngine {
  pub fn tests_for(&self, request: &TestsForRequest) -> QueryResult<Response<Targeted<TestsForAnswer>>> {
    let freshness = self.barrier()?;
    let policy = Policy::with_floor(request.min_confidence.unwrap_or(0.0));
    let answer = self.read(policy, |graph| -> QueryResult<Targeted<TestsForAnswer>> {
      let target = match resolve_one(self, graph, &request.target, &request.filter)? {
        Ok(target) => target,
        Err(outcome) => return Ok(outcome),
      };
      let (tests, mut caveats) = tests_of(graph, &target, TEST_DEPTH, TestScope::All)?;
      if tests.is_empty() {
        caveats.extend(unresolved_risk(graph, target.id, risk_name(&target))?);
      }
      Ok(Targeted::Found(TestsForAnswer {
        target: EntityRef::of(&target),
        tests: Listing::truncated(tests, request.limit),
        caveats,
      }))
    })?;
    let answer = self.with_problem_files(answer, freshness.run, |answer| &mut answer.caveats)?;
    Ok(Response { freshness, answer })
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestScope {
  Direct,
  All,
}

pub(crate) fn direct_tests(graph: &Graph<'_, '_>, target: &Entity) -> QueryResult<Vec<TestItem>> {
  let (tests, _) = tests_of(graph, target, 1, TestScope::Direct)?;
  Ok(
    tests
      .into_iter()
      .filter(|test| matches!(test.reach, TestReach::DirectCall | TestReach::CoversFile))
      .collect(),
  )
}

pub(crate) fn tests_of(graph: &Graph<'_, '_>, target: &Entity, depth: u32, scope: TestScope) -> QueryResult<(Vec<TestItem>, Vec<Caveat>)> {
  let mut seeds = vec![target.id];
  seeds.extend(graph.children(target.id)?.into_iter().filter(|child| !is_test(child)).map(|child| child.id));
  let steps: Vec<Step> = [EdgeKind::Calls, EdgeKind::Instantiates, EdgeKind::References, EdgeKind::TypeOf, EdgeKind::Tests]
    .into_iter()
    .map(|kind| Step::new(kind, Direction::Reverse, 1.0))
    .collect();
  let walk = graph.walk(
    &WalkSpec {
      seeds: &seeds,
      steps: &steps,
      max_depth: depth,
      max_nodes: MAX_NODES,
    },
    |entity| !is_test(entity),
  )?;
  let mut found: BTreeMap<EntityId, TestItem> = BTreeMap::new();
  for reached in &walk.reached {
    let Some(entity) = graph.entity(reached.id)? else { continue };
    if !is_test(&entity) {
      continue;
    }
    let Some(last) = reached.last_hop() else { continue };
    let reach = match reached.depth {
      1 => TestReach::DirectCall,
      _ => TestReach::CallGraph,
    };
    let path = match reached.depth > 1 {
      true => graph.path(reached)?,
      false => Vec::new(),
    };
    let item = TestItem {
      run: run_hint(&entity),
      test: EntityRef::of(&entity),
      reach,
      depth: reached.depth,
      relation: graph.relation(&last.edge, reached.id)?,
      path,
      uncertain: reached.is_uncertain(),
    };
    keep_best(&mut found, entity.id, item);
  }
  if scope == TestScope::All {
    add_importing_tests(graph, &seeds, &mut found)?;
  }
  if let Some(file) = file_entity(target) {
    for edge in graph.edges_in(EntityId::of(&file), &[EdgeKind::CoversFile])? {
      let Some(test) = graph.entity(edge.from)? else { continue };
      let item = TestItem {
        run: run_hint(&test),
        test: EntityRef::of(&test),
        reach: TestReach::CoversFile,
        depth: 1,
        relation: graph.relation(&edge, test.id)?,
        path: Vec::new(),
        uncertain: !edge.certainty.is_authoritative(),
      };
      keep_best(&mut found, test.id, item);
    }
    let neighbours = match scope {
      TestScope::All => colocated(graph, target)?,
      TestScope::Direct => Vec::new(),
    };
    for test in neighbours {
      let item = TestItem {
        run: run_hint(&test),
        test: EntityRef::of(&test),
        reach: TestReach::Colocated,
        depth: 1,
        relation: Hop {
          edge: String::new(),
          kind: "colocated".to_owned(),
          phrase: "colocated with".to_owned(),
          entity: file.display_name(),
          certainty: CertaintyTag {
            class: "heuristic".to_owned(),
            confidence: None,
            freshness: None,
            authoritative: false,
            tag: "[heuristic naming convention]".to_owned(),
          },
        },
        path: Vec::new(),
        uncertain: true,
      };
      keep_best(&mut found, test.id, item);
    }
  }
  let mut tests: Vec<TestItem> = found.into_values().collect();
  tests.sort_by_key(|test| (test.reach, test.depth, test.uncertain, test.test.sort_key()));
  let mut caveats = Vec::new();
  if walk.truncated {
    caveats.push(Caveat::Truncated { max_nodes: MAX_NODES });
  }
  if walk.hidden_below_floor > 0 {
    caveats.push(Caveat::BelowConfidenceFloor {
      hidden: walk.hidden_below_floor,
      floor: graph.policy.min_confidence,
    });
  }
  Ok((tests, caveats))
}

fn add_importing_tests(graph: &Graph<'_, '_>, seeds: &[EntityId], found: &mut BTreeMap<EntityId, TestItem>) -> QueryResult<()> {
  for seed in seeds {
    for edge in graph.edges_in(*seed, &[EdgeKind::Imports])? {
      let Some(importer) = graph.entity(edge.from)? else { continue };
      let (Some(repo), EntityKey::File { path, .. }) = (&importer.repo, &importer.key) else {
        continue;
      };
      for test in top_level_tests(graph, repo, path)? {
        let item = TestItem {
          run: run_hint(&test),
          test: EntityRef::of(&test),
          reach: TestReach::ImportedByTestFile,
          depth: 1,
          relation: graph.relation(&edge, importer.id)?,
          path: Vec::new(),
          uncertain: !edge.certainty.is_authoritative(),
        };
        keep_best(found, test.id, item);
      }
    }
  }
  Ok(())
}

fn keep_best(found: &mut BTreeMap<EntityId, TestItem>, id: EntityId, item: TestItem) {
  match found.get(&id) {
    Some(existing) if (existing.reach, existing.depth, existing.uncertain) <= (item.reach, item.depth, item.uncertain) => {}
    _ => {
      found.insert(id, item);
    }
  }
}

fn colocated(graph: &Graph<'_, '_>, target: &Entity) -> QueryResult<Vec<Entity>> {
  let (Some(repo), Some(path)) = (&target.repo, &target.path) else {
    return Ok(Vec::new());
  };
  let mut paths = sibling_test_paths(path);
  if matches!(crate::queries::common::language(target), Some(LanguageFamily::Rust)) {
    paths.push(path.clone());
  }
  let mut tests = Vec::new();
  for candidate in paths {
    tests.extend(top_level_tests(graph, repo, &candidate)?.into_iter().filter(|test| test.id != target.id));
  }
  Ok(tests)
}

fn top_level_tests(graph: &Graph<'_, '_>, repo: &atlas_core::RepoName, path: &atlas_core::RelPath) -> QueryResult<Vec<Entity>> {
  let mut tests = Vec::new();
  for entity in graph.tx().entities_in_file(repo, path, &EntityFilter::default())? {
    if !is_test(&entity) {
      continue;
    }
    let top_level = match graph.parent(entity.id)? {
      Some(parent) => !is_test(&parent),
      None => true,
    };
    if top_level {
      tests.push(entity);
    }
  }
  Ok(tests)
}

pub(crate) fn run_hint(entity: &Entity) -> Option<String> {
  let EntityKey::Symbol(symbol) = &entity.key else { return None };
  let repo = entity.repo.as_ref()?;
  match symbol.family() {
    LanguageFamily::Rust => {
      let mut parts = symbol.package().split('/');
      let package = parts.next()?;
      let target = match (parts.next(), parts.next()) {
        (Some("test"), Some(name)) => format!("--test {name}"),
        (Some("bin"), Some(name)) => format!("--bin {name}"),
        (Some("example"), Some(name)) => format!("--example {name}"),
        (Some("bench"), Some(name)) => format!("--bench {name}"),
        _ => "--lib".to_owned(),
      };
      let path: Vec<&str> = symbol.module_path().iter().chain(symbol.item_path()).map(String::as_str).collect();
      Some(format!("[{repo}] cargo test -p {package} {target} {}", path.join("::")))
    }
    _ => {
      let file = entity.path.as_ref()?;
      let name = symbol.item_path().join(" ");
      let framework = entity
        .attrs
        .get(ATTR_TEST)
        .and_then(|value| serde_json::from_value::<TestInfo>(value.clone()).ok())
        .map(|info| info.framework)
        .unwrap_or_default();
      let command = match framework.as_str() {
        "vitest" => format!("npx vitest run {file}"),
        "mocha" => format!("npx mocha {file}"),
        _ => format!("npx jest {file}"),
      };
      let filter = match framework.as_str() {
        "mocha" => "-g",
        _ => "-t",
      };
      Some(format!("[{repo}] {command} {filter} {name:?}"))
    }
  }
}
