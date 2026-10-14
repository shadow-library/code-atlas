use std::collections::BTreeMap;

use atlas_core::{ATTR_MOCK, ATTR_TEST, Edge, EdgeKind, Entity, EntityId, EntityKey, EntityKind, LanguageFamily, TestInfo};
use atlas_store::EntityFilter;
use serde::Serialize;
use serde_json::Value;

use crate::QueryEngine;
use crate::engine::unresolved_risk;
use crate::error::QueryResult;
use crate::graph::{Graph, Step, WalkSpec};
use crate::model::{Caveat, CertaintyTag, EntityRef, Hop, Listing, Response, Targeted, short_name};
use crate::policy::{Direction, Policy};
use crate::queries::common::{DEFAULT_LIMIT, MAX_NODES, file_entity, is_test, resolve_one, sibling_test_paths};
use crate::queries::tests_for_fallback::{DIRECT_CONFIDENCE, FALLBACK_CAP, HELPER_CONFIDENCE, needs_fallback, possible_tests};
use crate::queries::traverse::risk_name;
use crate::target::TargetFilter;

const TEST_DEPTH: u32 = 3;
const ATTR_SITE_ATTRS: &str = "site_attrs";
const MOCK_REPLACED: &str = "replaced";

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
  Spied,
  Mocked,
  ScopeMocked,
  PossibleCall,
}

impl TestReach {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::DirectCall => "direct",
      Self::CoversFile => "covers file",
      Self::ImportedByTestFile => "test file imports it",
      Self::CallGraph => "via call graph",
      Self::Colocated => "colocated",
      Self::Spied => "spies on it (may call through)",
      Self::Mocked => "mocks it (does not exercise it)",
      Self::ScopeMocked => "its scope mocks it (may not exercise it)",
      Self::PossibleCall => "possibly tests it",
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
  #[serde(skip_serializing_if = "Option::is_none")]
  pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FallbackTests {
  pub explanation: String,
  pub tests: Listing<TestItem>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub capped_at: Option<usize>,
  pub suppressed_by_mocks: usize,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub replaced: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TestsForAnswer {
  pub target: EntityRef,
  pub tests: Listing<TestItem>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub fallback: Option<FallbackTests>,
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
      let fallback = match needs_fallback(&tests) {
        true => fallback_tests(graph, &target, &tests, request.limit, &mut caveats)?,
        false => None,
      };
      if tests.is_empty() && fallback.as_ref().is_none_or(|fallback| fallback.tests.is_empty()) {
        caveats.extend(unresolved_risk(graph, target.id, risk_name(&target))?);
      }
      Ok(Targeted::Found(TestsForAnswer {
        target: EntityRef::of(&target),
        tests: Listing::truncated(tests, request.limit),
        fallback,
        caveats,
      }))
    })?;
    let answer = self.with_problem_files(answer, freshness.run, |answer| &mut answer.caveats)?;
    Ok(Response { freshness, answer })
  }
}

fn fallback_tests(graph: &Graph<'_, '_>, target: &Entity, tests: &[TestItem], limit: usize, caveats: &mut Vec<Caveat>) -> QueryResult<Option<FallbackTests>> {
  let found = possible_tests(graph, target, tests)?;
  if found.hidden_below_floor > 0 {
    let floor = graph.policy.min_confidence;
    match caveats.iter_mut().find_map(|caveat| match caveat {
      Caveat::BelowConfidenceFloor { hidden, .. } => Some(hidden),
      _ => None,
    }) {
      Some(hidden) => *hidden += found.hidden_below_floor,
      None => caveats.push(Caveat::BelowConfidenceFloor {
        hidden: found.hidden_below_floor,
        floor,
      }),
    }
  }
  if found.items.is_empty() && found.suppressed == 0 {
    return Ok(None);
  }
  let shown = limit.min(FALLBACK_CAP);
  let capped_at = (found.items.len() > shown && limit > FALLBACK_CAP).then_some(FALLBACK_CAP);
  let explanation = format!(
    "no test reaches the method through the call graph; these tests call a method of this name on a value of unknown type in a file that imports the class directly [heuristic {:.2}] or through a test helper [heuristic {:.2}]; the receiver may be another type, and jest.mock/vi.mock of the module is not visible to the index",
    DIRECT_CONFIDENCE, HELPER_CONFIDENCE
  );
  Ok(Some(FallbackTests {
    explanation,
    tests: Listing::truncated(found.items, shown),
    capped_at,
    suppressed_by_mocks: found.suppressed,
    replaced: found.replaced,
  }))
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
  let mocks = mock_edges(graph, &seeds)?;
  let mut found: BTreeMap<EntityId, TestItem> = BTreeMap::new();
  for reached in &walk.reached {
    let Some(entity) = graph.entity(reached.id)? else { continue };
    if !is_test(&entity) {
      continue;
    }
    let Some(last) = reached.last_hop() else { continue };
    let reach = match (reached.hops.iter().filter_map(|hop| mocking_of(&hop.edge)).max(), reached.depth) {
      (Some(mocking), _) => mocking.reach(),
      (None, 1) => TestReach::DirectCall,
      (None, _) => TestReach::CallGraph,
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
      note: None,
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
        note: None,
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
        note: None,
      };
      keep_best(&mut found, test.id, item);
    }
  }
  let mut scopes = MockScopes::new(graph, &mocks);
  scopes.note_real_reaches(&mut found)?;
  if scope == TestScope::All {
    scopes.add_scope_only_tests(&mut found)?;
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
          note: None,
        };
        keep_best(found, test.id, item);
      }
    }
  }
  Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Mocking {
  Spied,
  Replaced,
}

impl Mocking {
  fn reach(self) -> TestReach {
    match self {
      Self::Spied => TestReach::Spied,
      Self::Replaced => TestReach::Mocked,
    }
  }
}

fn mocking_of(edge: &Edge) -> Option<Mocking> {
  if edge.kind != EdgeKind::References {
    return None;
  }
  let sites = edge.attrs.get(ATTR_SITE_ATTRS).and_then(Value::as_array).into_iter().flatten();
  let site_values = sites.filter_map(|site| site.get("attrs").and_then(|attrs| attrs.get(ATTR_MOCK)));
  edge
    .attrs
    .get(ATTR_MOCK)
    .into_iter()
    .chain(site_values)
    .map(|value| match value.as_str() == Some(MOCK_REPLACED) {
      true => Mocking::Replaced,
      false => Mocking::Spied,
    })
    .max()
}

pub(crate) struct MockEdge {
  edge: Edge,
  mocking: Mocking,
}

impl MockEdge {
  pub(crate) fn replaces(&self) -> bool {
    self.mocking == Mocking::Replaced
  }

  pub(crate) fn mocked(&self) -> EntityId {
    self.edge.to
  }
}

pub(crate) fn mock_edges(graph: &Graph<'_, '_>, seeds: &[EntityId]) -> QueryResult<BTreeMap<EntityId, MockEdge>> {
  let mut mocks: BTreeMap<EntityId, MockEdge> = BTreeMap::new();
  for seed in seeds {
    for edge in graph.edges_in(*seed, &[EdgeKind::References])? {
      let Some(mocking) = mocking_of(&edge) else { continue };
      if mocks.get(&edge.from).is_none_or(|existing| existing.mocking < mocking) {
        mocks.insert(edge.from, MockEdge { edge, mocking });
      }
    }
  }
  Ok(mocks)
}

pub(crate) struct MockScopes<'g, 'a, 'c, 'm> {
  graph: &'g Graph<'a, 'c>,
  mocks: &'m BTreeMap<EntityId, MockEdge>,
  callees: BTreeMap<EntityId, Vec<EntityId>>,
}

impl<'m> MockScopes<'_, '_, '_, 'm> {
  pub(crate) fn new<'g, 'a, 'c>(graph: &'g Graph<'a, 'c>, mocks: &'m BTreeMap<EntityId, MockEdge>) -> MockScopes<'g, 'a, 'c, 'm> {
    MockScopes {
      graph,
      mocks,
      callees: BTreeMap::new(),
    }
  }

  fn note_real_reaches(&mut self, found: &mut BTreeMap<EntityId, TestItem>) -> QueryResult<()> {
    if self.mocks.is_empty() {
      return Ok(());
    }
    for (id, item) in found.iter_mut() {
      if matches!(item.reach, TestReach::Spied | TestReach::Mocked) {
        continue;
      }
      item.note = self.scope_note(*id)?;
    }
    Ok(())
  }

  pub(crate) fn scope_note(&mut self, test: EntityId) -> QueryResult<Option<String>> {
    let Some((scope, mock)) = self.scope_mock(test)? else { return Ok(None) };
    let relation = self.scope_relation(&scope, mock)?;
    Ok(Some(format!("{} {} {}", relation.phrase, relation.entity, relation.certainty.tag)))
  }

  fn add_scope_only_tests(&mut self, found: &mut BTreeMap<EntityId, TestItem>) -> QueryResult<()> {
    let mut sources: Vec<(&EntityId, &MockEdge)> = self.mocks.iter().collect();
    sources.sort_by_key(|(id, mock)| (std::cmp::Reverse(mock.mocking), **id));
    for (source, mock) in sources {
      let Some(scope) = self.graph.entity(*source)? else { continue };
      let relation = self.scope_relation(&scope, mock)?;
      for test in self.tests_under(&scope)? {
        if found.contains_key(&test.id) {
          continue;
        }
        let item = TestItem {
          run: run_hint(&test),
          test: EntityRef::of(&test),
          reach: TestReach::ScopeMocked,
          depth: 1,
          relation: relation.clone(),
          path: Vec::new(),
          uncertain: true,
          note: None,
        };
        found.insert(test.id, item);
      }
    }
    Ok(())
  }

  fn tests_under(&self, scope: &Entity) -> QueryResult<Vec<Entity>> {
    if let (EntityKey::File { path, .. }, Some(repo)) = (&scope.key, &scope.repo) {
      return Ok(
        self
          .graph
          .tx()
          .entities_in_file(repo, path, &EntityFilter::default())?
          .into_iter()
          .filter(is_test)
          .collect(),
      );
    }
    if !is_test(scope) {
      return Ok(Vec::new());
    }
    let mut tests = Vec::new();
    let mut pending = vec![scope.id];
    while let Some(parent) = pending.pop() {
      for child in self.graph.children(parent)?.into_iter().filter(is_test) {
        pending.push(child.id);
        tests.push(child);
      }
    }
    Ok(tests)
  }

  fn scope_relation(&self, scope: &Entity, mock: &MockEdge) -> QueryResult<Hop> {
    let mut relation = self.graph.relation(&mock.edge, scope.id)?;
    let role = match scope.kind {
      EntityKind::TestSuite => "suite",
      EntityKind::Test => "test",
      EntityKind::File => "file",
      _ => "helper",
    };
    let verb = match mock.mocking {
      Mocking::Replaced => "mocks",
      Mocking::Spied => "spies on",
    };
    relation.phrase = format!("{role} {} {verb}", short_name(&scope.key));
    Ok(relation)
  }

  pub(crate) fn scope_mock(&mut self, test: EntityId) -> QueryResult<Option<(Entity, &'m MockEdge)>> {
    let Some(entity) = self.graph.entity(test)? else { return Ok(None) };
    let mut scopes: Vec<EntityId> = file_entity(&entity).map(|key| EntityId::of(&key)).into_iter().collect();
    let mut current = Some(entity);
    while let Some(scope) = current {
      scopes.push(scope.id);
      current = self.graph.parent(scope.id)?;
    }
    let mut candidates = scopes.clone();
    for scope in scopes {
      candidates.extend(self.callees_of(scope)?);
    }
    let best = candidates
      .into_iter()
      .filter_map(|candidate| self.mocks.get(&candidate).map(|mock| (candidate, mock)))
      .max_by_key(|(_, mock)| mock.mocking);
    let Some((scope, mock)) = best else { return Ok(None) };
    Ok(self.graph.entity(scope)?.map(|scope| (scope, mock)))
  }

  fn callees_of(&mut self, scope: EntityId) -> QueryResult<Vec<EntityId>> {
    if let Some(callees) = self.callees.get(&scope) {
      return Ok(callees.clone());
    }
    let callees: Vec<EntityId> = self.graph.edges_out(scope, &[EdgeKind::Calls])?.into_iter().map(|edge| edge.to).collect();
    self.callees.insert(scope, callees.clone());
    Ok(callees)
  }
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

#[cfg(test)]
mod tests {
  use atlas_core::{ATTR_MOCK_TYPE, Certainty, EntityKey, RepoName};
  use serde_json::json;

  use super::*;

  fn edge(kind: EdgeKind, certainty: Certainty, attrs: Value) -> Edge {
    let from = EntityId::of(&EntityKey::Repository {
      repo: RepoName::new("a").unwrap(),
    });
    let to = EntityId::of(&EntityKey::Repository {
      repo: RepoName::new("b").unwrap(),
    });
    let mut edge = Edge::new(kind, from, to, "", certainty);
    edge.attrs = serde_json::from_value(attrs).unwrap();
    edge
  }

  #[test]
  fn should_classify_mock_edges_by_their_attrs_whatever_their_certainty() {
    let resolved = edge(EdgeKind::References, Certainty::Resolved, json!({ ATTR_MOCK: "spied", ATTR_MOCK_TYPE: "cast" }));
    assert_eq!(mocking_of(&resolved), Some(Mocking::Spied));
    let merged = edge(
      EdgeKind::References,
      Certainty::Resolved,
      json!({ ATTR_MOCK: "spied", ATTR_SITE_ATTRS: [{ "span": 10, "attrs": { ATTR_MOCK: "replaced" } }] }),
    );
    assert_eq!(mocking_of(&merged), Some(Mocking::Replaced));
    assert_eq!(mocking_of(&edge(EdgeKind::References, Certainty::Resolved, json!({}))), None);
    assert_eq!(mocking_of(&edge(EdgeKind::Calls, Certainty::Resolved, json!({ ATTR_MOCK: "replaced" }))), None);
  }
}
