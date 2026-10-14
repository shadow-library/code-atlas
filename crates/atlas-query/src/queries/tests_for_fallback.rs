use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{Certainty, Confidence, EdgeKind, Entity, EntityId, EntityKey, EntityKind, ReceiverHint, RelPath, RepoName, UnresolvedReason, UnresolvedRef};
use atlas_store::EntityFilter;

use crate::dynamic::dynamic_member_files;
use crate::error::QueryResult;
use crate::graph::Graph;
use crate::model::{CertaintyTag, EntityRef, Hop, short_name};
use crate::queries::common::{file_entity, is_test};
use crate::queries::tests_for::{MockScopes, TestItem, TestReach, mock_edges, run_hint};

pub(crate) const DIRECT_CONFIDENCE: f32 = 0.4;
pub(crate) const HELPER_CONFIDENCE: f32 = 0.35;
pub(crate) const FALLBACK_CAP: usize = 20;
// atlas-resolve truncates the candidates of an ambiguous call to 32, so a full list may have dropped the target
const RESOLVER_CANDIDATE_CAP: usize = 32;
const HELPER_HOPS: usize = 2;
const FALLBACK_RULE: &str = "unknown-receiver-method-name";
const SUPPORT_DIRECTORIES: [&str; 13] = [
  "test",
  "tests",
  "__tests__",
  "__mocks__",
  "__fixtures__",
  "__setup__",
  "fixtures",
  "testing",
  "test-utils",
  "test-helpers",
  "support",
  "spec",
  "e2e",
];
const SUPPORT_TOKENS: [&str; 11] = [
  "helper",
  "helpers",
  "factory",
  "factories",
  "fixture",
  "fixtures",
  "mock",
  "mocks",
  "builder",
  "builders",
  "setup",
];
const MODULE_MOCK_OWNERS: [&str; 2] = ["jest", "vi"];

#[derive(Debug, Default)]
pub(crate) struct Fallback {
  pub(crate) items: Vec<TestItem>,
  pub(crate) suppressed: usize,
  pub(crate) replaced: Vec<String>,
  pub(crate) hidden_below_floor: usize,
}

type Candidates = BTreeMap<(RepoName, RelPath), Option<RelPath>>;

pub(crate) fn needs_fallback(tests: &[TestItem]) -> bool {
  !tests.iter().any(|test| matches!(test.reach, TestReach::DirectCall | TestReach::CallGraph))
}

pub(crate) fn possible_tests(graph: &Graph<'_, '_>, target: &Entity, found: &[TestItem]) -> QueryResult<Fallback> {
  if target.kind != EntityKind::Method {
    return Ok(Fallback::default());
  }
  let Some(name) = target.key.as_symbol().and_then(|symbol| symbol.item_path().last()) else {
    return Ok(Fallback::default());
  };
  let Some(class) = graph.parent(target.id)? else { return Ok(Fallback::default()) };
  let (Some(direct), Some(helper)) = (rated(DIRECT_CONFIDENCE), rated(HELPER_CONFIDENCE)) else {
    return Ok(Fallback::default());
  };
  let candidates = candidate_files(graph, target)?;
  if candidates.is_empty() {
    return Ok(Fallback::default());
  }
  let known: BTreeSet<EntityId> = found.iter().filter_map(|test| test.test.id.parse().ok()).collect();
  let mut per_test: BTreeMap<EntityId, (Entity, Option<RelPath>, Vec<UnresolvedRef>)> = BTreeMap::new();
  for ((repo, path), via) in &candidates {
    for reference in graph.tx().unresolved_in_file_with_name_prefix(repo, path, name)? {
      if !is_unknown_receiver_call(&reference, name, target.id) {
        continue;
      }
      let Some(test) = graph.entity(reference.from)? else { continue };
      if !is_test(&test) || known.contains(&test.id) {
        continue;
      }
      per_test.entry(test.id).or_insert_with(|| (test, via.clone(), Vec::new())).2.push(reference);
    }
  }
  let mut members = vec![target.id, class.id];
  members.extend(graph.children(class.id)?.into_iter().map(|member| member.id));
  members.extend(file_entity(target).map(|file| EntityId::of(&file)));
  let mut replacing = mock_edges(graph, &[target.id, class.id])?;
  replacing.retain(|_, mock| mock.replaces());
  let all_mocks = mock_edges(graph, &members)?;
  let mut replacers = MockScopes::new(graph, &replacing);
  let mut noters = MockScopes::new(graph, &all_mocks);
  let floor = graph.policy.min_confidence;
  let mut module_mocks: BTreeMap<(RepoName, RelPath), bool> = BTreeMap::new();
  let mut fallback = Fallback::default();
  let mut replaced = BTreeSet::new();
  for (id, (test, via, mut sites)) in per_test {
    if let Some((_, mock)) = replacers.scope_mock(id)? {
      fallback.suppressed += 1;
      replaced.extend(graph.entity(mock.mocked())?.map(|mocked| short_name(&mocked.key)));
      continue;
    }
    let (certainty, confidence) = match via {
      Some(_) => (&helper, HELPER_CONFIDENCE),
      None => (&direct, DIRECT_CONFIDENCE),
    };
    if confidence < floor {
      fallback.hidden_below_floor += 1;
      continue;
    }
    sites.sort_by_key(|site| site.span.start_byte);
    let mut notes: Vec<String> = noters.scope_note(id)?.into_iter().collect();
    let file = (sites[0].repo.clone(), sites[0].file.clone());
    let module_mocked = match module_mocks.get(&file) {
      Some(known) => *known,
      None => {
        let mocked = calls_module_mock(graph, &file.0, &file.1)?;
        module_mocks.insert(file, mocked);
        mocked
      }
    };
    if module_mocked {
      notes.push("this file calls jest.mock/vi.mock on a module the index cannot name; if it is the class's module, the class is replaced".to_owned());
    }
    fallback.items.push(item(&test, name, &class, via.as_ref(), &sites, certainty, notes));
  }
  fallback.replaced = replaced.into_iter().collect();
  fallback.items.sort_by(|left, right| {
    confidence_of(right)
      .total_cmp(&confidence_of(left))
      .then_with(|| location_key(left).cmp(&location_key(right)))
      .then_with(|| left.test.key.cmp(&right.test.key))
  });
  Ok(fallback)
}

fn rated(confidence: f32) -> Option<Certainty> {
  Confidence::new(confidence).ok().map(|confidence| Certainty::heuristic(confidence, FALLBACK_RULE))
}

fn calls_module_mock(graph: &Graph<'_, '_>, repo: &RepoName, path: &RelPath) -> QueryResult<bool> {
  let references = graph.tx().unresolved_in_file_with_name_prefix(repo, path, "mock")?;
  Ok(
    references
      .iter()
      .any(|reference| matches!(reference.path.as_slice(), [owner, method] if MODULE_MOCK_OWNERS.contains(&owner.as_str()) && method == "mock")),
  )
}

fn confidence_of(item: &TestItem) -> f32 {
  item.relation.certainty.confidence.unwrap_or(0.0)
}

fn location_key(item: &TestItem) -> (Option<&str>, Option<u32>) {
  let location = item.test.location.as_ref();
  (location.map(|location| location.path.as_str()), location.and_then(|location| location.line))
}

fn is_unknown_receiver_call(reference: &UnresolvedRef, name: &str, target: EntityId) -> bool {
  let typed = matches!(reference.receiver, Some(ReceiverHint::TypeHint(_) | ReceiverHint::Declared { .. }));
  if typed || reference.path.len() < 2 || reference.path.last().is_none_or(|last| last != name) {
    return false;
  }
  match &reference.reason {
    UnresolvedReason::Dynamic => true,
    UnresolvedReason::Ambiguous { candidates } => candidates.is_empty() || candidates.contains(&target) || candidates.len() >= RESOLVER_CANDIDATE_CAP,
    _ => false,
  }
}

fn candidate_files(graph: &Graph<'_, '_>, target: &Entity) -> QueryResult<Candidates> {
  let own = target.repo.clone().zip(target.path.clone());
  let mut candidates = Candidates::new();
  for file in dynamic_member_files(graph, target)? {
    if Some(&file) != own.as_ref() {
      candidates.insert(file, None);
    }
  }
  let supports: Vec<(RepoName, RelPath)> = candidates.keys().filter(|(_, path)| is_test_support(path)).cloned().collect();
  for (repo, path) in supports {
    for importer in importers_through(graph, &repo, &path)? {
      if Some(&importer) != own.as_ref() {
        candidates.entry(importer).or_insert_with(|| Some(path.clone()));
      }
    }
  }
  Ok(candidates)
}

fn importers_through(graph: &Graph<'_, '_>, repo: &RepoName, path: &RelPath) -> QueryResult<BTreeSet<(RepoName, RelPath)>> {
  let mut importers = BTreeSet::new();
  let mut frontier = vec![(repo.clone(), path.clone())];
  let mut visited: BTreeSet<(RepoName, RelPath)> = frontier.iter().cloned().collect();
  for _ in 0..HELPER_HOPS {
    let mut next = Vec::new();
    for (repo, path) in std::mem::take(&mut frontier) {
      let mut anchors = vec![EntityId::of(&EntityKey::File {
        repo: repo.clone(),
        path: path.clone(),
      })];
      anchors.extend(
        graph
          .tx()
          .entities_in_file(&repo, &path, &EntityFilter::default())?
          .into_iter()
          .filter(|entity| entity.kind.is_code_symbol())
          .map(|entity| entity.id),
      );
      for anchor in anchors {
        for link in graph.links_in(anchor, &[EdgeKind::Imports, EdgeKind::ReExports])? {
          let Some(source) = graph.entity(link.from)? else { continue };
          let Some(file) = source.repo.clone().zip(source.path.clone()).or_else(|| file_of(&source)) else {
            continue;
          };
          importers.insert(file.clone());
          if link.kind == EdgeKind::ReExports && visited.insert(file.clone()) {
            next.push(file);
          }
        }
      }
    }
    frontier = next;
  }
  Ok(importers)
}

fn file_of(entity: &Entity) -> Option<(RepoName, RelPath)> {
  match &entity.key {
    EntityKey::File { repo, path } => Some((repo.clone(), path.clone())),
    _ => None,
  }
}

fn is_test_support(path: &RelPath) -> bool {
  let mut segments: Vec<&str> = path.as_str().split('/').collect();
  let Some(file) = segments.pop() else { return false };
  segments.iter().any(|segment| SUPPORT_DIRECTORIES.contains(segment)) || file.split(['.', '-', '_']).any(|token| SUPPORT_TOKENS.contains(&token)) || file.starts_with("test-utils")
}

fn item(test: &Entity, name: &str, class: &Entity, via: Option<&RelPath>, sites: &[UnresolvedRef], certainty: &Certainty, notes: Vec<String>) -> TestItem {
  let first = &sites[0];
  let class_name = short_name(&class.key);
  let more = match sites.len() {
    1 => String::new(),
    many => format!(" (+{} more call sites)", many - 1),
  };
  let imported = match via {
    Some(helper) => format!("{class_name} through {helper}"),
    None => class_name,
  };
  TestItem {
    run: run_hint(test),
    test: EntityRef::of(test),
    reach: TestReach::PossibleCall,
    depth: 1,
    relation: Hop {
      edge: String::new(),
      kind: "possible_call".to_owned(),
      phrase: format!(
        "calls `.{name}(` on a value of unknown type at {}:{}{more}; the file imports",
        first.file, first.span.start_line
      ),
      entity: imported,
      certainty: CertaintyTag::of(certainty),
    },
    path: Vec::new(),
    uncertain: true,
    note: (!notes.is_empty()).then(|| notes.join("; ")),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn support(path: &str) -> bool {
    is_test_support(&RelPath::new(path).unwrap())
  }

  #[test]
  fn should_treat_helper_paths_as_test_support_and_production_files_as_not() {
    assert!(support("tests/helpers/make-service.ts"));
    assert!(support("src/__tests__/util.ts"));
    assert!(support("src/project.factory.ts"));
    assert!(support("src/support/db.ts"));
    assert!(support("src/spec/db.ts"));
    assert!(support("src/__setup__/db.ts"));
    assert!(support("src/project.builder.ts"));
    assert!(support("src/db.setup.ts"));
    assert!(support("src/make-project.fixtures.ts"));
    assert!(!support("src/project.controller.ts"));
    assert!(!support("src/factorytown/index.ts"));
  }
}
