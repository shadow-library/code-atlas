mod common;

use atlas_query::{Caveat, Targeted, TestReach, TestsForAnswer, TestsForRequest, render_text};

const PACKAGE: &str = r#"{ "name": "projects", "version": "1.0.0" }"#;
const SERVICE: &str = "export class ProjectService {\n\
  constructor(private readonly db: object) {}\n\
  update(id: string) { return id; }\n\
  remove(id: string) { return id; }\n\
  archive() { return 1; }\n\
  rename(id: string) { return id; }\n\
}\n\
export class AuditLog {\n\
  update(id: string) { return id; }\n\
  remove(id: string) { return id; }\n\
}\n\
export class TaskService {\n\
  update(id: string) { return id; }\n\
}\n\
export class UserService {\n\
  update(id: string) { return id; }\n\
}\n";
const CONTROLLER: &str = "import { ProjectService } from './project.service';\n\
export class ProjectController {\n\
  constructor(private readonly projects: ProjectService) {}\n\
}\n";
const HELPER: &str = "import { ProjectService } from '../../src/project.service';\n\
export function makeService() {\n\
  const db = {};\n\
  return { service: new ProjectService(db), db };\n\
}\n\
export function makeCache() {\n\
  return { cache: new Map<string, string>() };\n\
}\n";
const PROJECT_SPEC: &str = "import { ProjectService } from '../src/project.service';\n\
import { makeService } from './helpers/make-service';\n\
\n\
describe('ProjectService', () => {\n\
  it('updates a project', () => {\n\
    const { service } = makeService();\n\
    service.update('p1');\n\
    service.update('p2');\n\
  });\n\
\n\
  it('archives directly', () => {\n\
    const real = new ProjectService({});\n\
    real.archive();\n\
  });\n\
\n\
  it('removes through the factory', () => {\n\
    const { service } = makeService();\n\
    service.remove('p1');\n\
  });\n\
});\n";
const CACHE_SPEC: &str = "import { makeCache } from './helpers/make-service';\n\
\n\
describe('cache', () => {\n\
  it('updates an unrelated cache', () => {\n\
    const { cache } = makeCache();\n\
    cache.update('k');\n\
  });\n\
});\n";
const MOCKED_SPEC: &str = "import { ProjectService } from '../src/project.service';\n\
import { makeService } from './helpers/make-service';\n\
\n\
describe('mocked project service', () => {\n\
  const stub = { rename: jest.fn() } as unknown as ProjectService;\n\
\n\
  it('updates beside a mock', () => {\n\
    const { service } = makeService();\n\
    service.update('p1');\n\
  });\n\
});\n";
const TYPED_SPEC: &str = "import { AuditLog } from '../src/project.service';\n\
\n\
describe('audit', () => {\n\
  it('updates the log', () => {\n\
    const log: AuditLog = build();\n\
    log.update('a1');\n\
  });\n\
});\n";
const CONTROLLER_SPEC: &str = "import { ProjectController } from '../src/project.controller';\n\
\n\
describe('ProjectController', () => {\n\
  it('updates through the controller', () => {\n\
    const { controller } = build(ProjectController);\n\
    controller.update('p1');\n\
  });\n\
});\n";
const BARREL: &str = "export * from './make-service';\n";
const BARREL_SPEC: &str = "import { makeService } from './helpers';\n\
\n\
describe('barrel', () => {\n\
  it('updates through the barrel', () => {\n\
    const { service } = makeService();\n\
    service.update('p1');\n\
  });\n\
});\n";
const SPY_SPEC: &str = "import { ProjectService } from '../src/project.service';\n\
import { makeService } from './helpers/make-service';\n\
\n\
describe('spied project service', () => {\n\
  it('updates beside a spy', () => {\n\
    jest.spyOn(ProjectService.prototype, 'rename');\n\
    const { service } = makeService();\n\
    service.update('p1');\n\
  });\n\
});\n";
const MODULE_MOCK_SPEC: &str = "import { makeService } from './helpers/make-service';\n\
jest.mock('../src/project.service');\n\
\n\
describe('module mocked', () => {\n\
  it('updates beside a module mock', () => {\n\
    const { service } = makeService();\n\
    service.update('p1');\n\
  });\n\
});\n";
const MOCKED_HELPER: &str = "import { ProjectService } from '../../src/project.service';\n\
export function makeMockedService() {\n\
  return { service: { update: jest.fn() } as unknown as ProjectService };\n\
}\n";
const HELPER_MOCK_SPEC: &str = "import { makeMockedService } from './helpers/mocked-service';\n\
\n\
describe('helper mocked', () => {\n\
  it('updates a mock built by a helper', () => {\n\
    const { service } = makeMockedService();\n\
    service.update('p1');\n\
  });\n\
});\n";
const FILES: [(&str, &str); 15] = [
  ("tests/helpers/index.ts", BARREL),
  ("tests/barrel.spec.ts", BARREL_SPEC),
  ("tests/spy.spec.ts", SPY_SPEC),
  ("tests/module-mock.spec.ts", MODULE_MOCK_SPEC),
  ("tests/helpers/mocked-service.ts", MOCKED_HELPER),
  ("tests/helper-mock.spec.ts", HELPER_MOCK_SPEC),
  ("package.json", PACKAGE),
  ("src/project.service.ts", SERVICE),
  ("src/project.controller.ts", CONTROLLER),
  ("tests/helpers/make-service.ts", HELPER),
  ("tests/project.spec.ts", PROJECT_SPEC),
  ("tests/cache.spec.ts", CACHE_SPEC),
  ("tests/mocked.spec.ts", MOCKED_SPEC),
  ("tests/typed.spec.ts", TYPED_SPEC),
  ("tests/controller.spec.ts", CONTROLLER_SPEC),
];

fn answer(target: &str, configure: impl FnOnce(&mut TestsForRequest)) -> TestsForAnswer {
  let fixture = common::single_repo("projects", &FILES);
  let mut request = TestsForRequest::new(target);
  configure(&mut request);
  let Targeted::Found(answer) = fixture.query.tests_for(&request).unwrap().answer else {
    panic!("{target} did not resolve");
  };
  answer
}

fn reaches(answer: &TestsForAnswer) -> Vec<(String, TestReach)> {
  answer.tests.items.iter().map(|test| (test.test.short.clone(), test.reach)).collect()
}

struct Hedged {
  test: String,
  tag: String,
  imports: String,
  note: Option<String>,
}

fn hedged(answer: &TestsForAnswer) -> Vec<Hedged> {
  let fallback = answer.fallback.as_ref().expect("fallback");
  fallback
    .tests
    .items
    .iter()
    .map(|test| Hedged {
      test: test.test.short.clone(),
      tag: test.relation.certainty.tag.clone(),
      imports: test.relation.entity.clone(),
      note: test.note.clone(),
    })
    .collect()
}

fn find<'a>(items: &'a [Hedged], test: &str) -> &'a Hedged {
  items.iter().find(|item| item.test == test).unwrap_or_else(|| panic!("{test} not listed"))
}

#[test]
fn should_list_tests_that_call_the_method_on_a_factory_built_value_as_hedged_fallback() {
  let answer = answer("ProjectService.update", |_| {});

  assert_eq!(reaches(&answer), [("helper mocked.updates a mock built by a helper".to_owned(), TestReach::Mocked)]);
  let items = hedged(&answer);
  let names: Vec<&str> = items.iter().map(|item| item.test.as_str()).collect();
  assert_eq!(
    names,
    [
      "barrel.updates through the barrel",
      "cache.updates an unrelated cache",
      "mocked project service.updates beside a mock",
      "module mocked.updates beside a module mock",
      "ProjectService.updates a project",
      "spied project service.updates beside a spy",
    ]
  );
  let fallback = answer.fallback.as_ref().expect("fallback");
  assert_eq!(fallback.tests.total, 6);
  assert!(
    fallback
      .tests
      .items
      .iter()
      .all(|test| test.reach == TestReach::PossibleCall && test.uncertain && !test.relation.certainty.authoritative)
  );
  assert_eq!(
    fallback.tests.items[4].relation.phrase,
    "calls `.update(` on a value of unknown type at tests/project.spec.ts:7 (+1 more call sites); the file imports"
  );
}

#[test]
fn should_name_the_helper_for_one_hop_candidates_and_rate_them_below_direct_importers() {
  let items = hedged(&answer("ProjectService.update", |_| {}));

  let direct = find(&items, "ProjectService.updates a project");
  assert_eq!((direct.tag.as_str(), direct.imports.as_str()), ("[heuristic 0.40]", "ProjectService"));
  let helper = find(&items, "cache.updates an unrelated cache");
  assert_eq!(
    (helper.tag.as_str(), helper.imports.as_str()),
    ("[heuristic 0.35]", "ProjectService through tests/helpers/make-service.ts")
  );
}

#[test]
fn should_reach_specs_that_import_the_helper_through_a_barrel() {
  let items = hedged(&answer("ProjectService.update", |_| {}));

  assert_eq!(
    find(&items, "barrel.updates through the barrel").imports,
    "ProjectService through tests/helpers/make-service.ts"
  );
}

#[test]
fn should_leave_a_test_built_on_a_mock_to_the_certain_path_and_out_of_the_fallback() {
  let answer = answer("ProjectService.update", |_| {});

  let fallback = answer.fallback.as_ref().expect("fallback");
  assert_eq!(fallback.suppressed_by_mocks, 0);
  assert!(fallback.tests.items.iter().all(|test| !test.test.short.starts_with("helper mocked")));
}

#[test]
fn should_keep_candidates_whose_scope_only_mocks_or_spies_a_different_member_with_a_note() {
  let items = hedged(&answer("ProjectService.update", |_| {}));

  assert_eq!(
    find(&items, "mocked project service.updates beside a mock").note.as_deref(),
    Some("suite mocked project service mocks ProjectService.rename [heuristic 0.60]")
  );
  assert_eq!(
    find(&items, "spied project service.updates beside a spy").note.as_deref(),
    Some("test spied project service.updates beside a spy spies on ProjectService.rename [heuristic 0.60]")
  );
}

#[test]
fn should_note_a_file_that_calls_jest_mock_on_a_module_the_index_cannot_name() {
  let items = hedged(&answer("ProjectService.update", |_| {}));

  assert_eq!(
    find(&items, "module mocked.updates beside a module mock").note.as_deref(),
    Some("this file calls jest.mock/vi.mock on a module the index cannot name; if it is the class's module, the class is replaced")
  );
  assert_eq!(find(&items, "ProjectService.updates a project").note, None);
}

#[test]
fn should_not_list_calls_on_values_with_a_declared_other_type_or_in_files_reached_through_production_code() {
  let fallback = answer("ProjectService.update", |_| {}).fallback.expect("fallback");

  let files: Vec<&str> = fallback
    .tests
    .items
    .iter()
    .filter_map(|test| test.test.location.as_ref())
    .map(|location| location.path.as_str())
    .collect();
  assert!(!files.contains(&"tests/typed.spec.ts"));
  assert!(!files.contains(&"tests/controller.spec.ts"));
}

#[test]
fn should_keep_the_certain_path_and_skip_the_fallback_when_a_test_reaches_the_method() {
  let heuristic = answer("ProjectService.remove", |_| {});
  assert_eq!(reaches(&heuristic), [("ProjectService.removes through the factory".to_owned(), TestReach::DirectCall)]);
  assert!(heuristic.fallback.is_none());

  let resolved = answer("ProjectService.archive", |_| {});
  assert_eq!(reaches(&resolved), [("ProjectService.archives directly".to_owned(), TestReach::DirectCall)]);
  assert!(resolved.fallback.is_none());
}

#[test]
fn should_hide_fallback_items_below_the_confidence_floor_with_one_caveat() {
  let answer = answer("ProjectService.update", |request| request.min_confidence = Some(0.38));

  let names: Vec<String> = hedged(&answer).into_iter().map(|item| item.test).collect();
  assert_eq!(
    names,
    [
      "mocked project service.updates beside a mock",
      "ProjectService.updates a project",
      "spied project service.updates beside a spy"
    ]
  );
  let floors: Vec<usize> = answer
    .caveats
    .iter()
    .filter_map(|caveat| match caveat {
      Caveat::BelowConfidenceFloor { hidden, .. } => Some(*hidden),
      _ => None,
    })
    .collect();
  assert_eq!(floors, [3]);
}

#[test]
fn should_truncate_the_fallback_listing_to_the_request_limit_and_keep_the_total() {
  let fallback = answer("ProjectService.update", |request| request.limit = 1).fallback.expect("fallback");

  assert_eq!(fallback.tests.total, 6);
  assert_eq!(fallback.tests.items.len(), 1);
  assert_eq!(fallback.capped_at, None);
}

#[test]
fn should_render_the_fallback_apart_from_certain_tests_and_name_its_origin() {
  let fixture = common::single_repo("projects", &FILES);
  let text = render_text(&fixture.query.tests_for(&TestsForRequest::new("ProjectService.update")).unwrap());

  let header = text.find("possible tests from the method-name fallback (6)").expect("fallback header");
  assert!(text.find("[mocks it (does not exercise it)]").expect("certain section") < header);
  assert!(text.contains("[possibly tests it]"));
  assert!(text.contains("through a test helper [heuristic 0.35]"));
}
