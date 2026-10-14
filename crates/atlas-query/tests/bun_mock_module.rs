mod common;

use atlas_query::{ImpactRequest, Targeted, TestReach, TestsForRequest};

const PACKAGE: &str = r#"{ "name": "shop", "version": "1.0.0" }"#;
const DB: &str = "export function query(sql: string) { return [sql]; }\nexport function close() { return 0; }\n";
const USERS_SERVICE: &str = "import { close, query } from './db';\n\
export function listUsers() { return query('select 1'); }\n\
export function shutdown() { return close(); }\n";
const USERS_SPEC: &str = "import { describe, expect, it, mock } from 'bun:test';\n\
import { query } from './db';\n\
import { listUsers } from './users.service';\n\
\n\
mock.module('./db', () => ({ query: mock(() => []) }));\n\
\n\
describe('users', () => {\n\
  it('lists users', () => {\n\
    listUsers();\n\
  });\n\
\n\
  it('checks nothing', () => {\n\
    expect(1).toBe(1);\n\
  });\n\
});\n";
const BLIND_SPEC: &str = "import { it, mock } from 'bun:test';\n\
import { listUsers } from './users.service';\n\
\n\
mock.module('./db', () => ({ query: mock(() => []) }));\n\
\n\
it('lists users without importing the replaced name', () => {\n\
  listUsers();\n\
});\n";
const SHUTDOWN_SPEC: &str = "import { it } from 'bun:test';\n\
import { shutdown } from './users.service';\n\
\n\
it('shuts down for real', () => {\n\
  shutdown();\n\
});\n";

const REAL_SPEC: &str = "import { it } from 'bun:test';\n\
import { listUsers } from './users.service';\n\
\n\
it('lists users for real', () => {\n\
  listUsers();\n\
});\n";

const LATE_SPEC: &str = "import { it, mock } from 'bun:test';\n\
import { listUsers } from './users.service';\n\
\n\
it('replaces the module inside its own body', () => {\n\
  mock.module('./db', () => ({}));\n\
  listUsers();\n\
});\n\
\n\
it('runs without a replacement of its own', () => {\n\
  listUsers();\n\
});\n";
const DIRECT_SPEC: &str = "import { it, mock } from 'bun:test';\n\
import { query } from './db';\n\
\n\
mock.module('./db', () => ({ query: mock(() => []) }));\n\
\n\
it('calls the replaced export directly', () => {\n\
  query('select 1');\n\
});\n";
const OTHER_MODULE_SPEC: &str = "import { it, mock } from 'bun:test';\n\
import { listUsers } from './users.service';\n\
\n\
mock.module('./clock', () => ({}));\n\
\n\
it('replaces an unrelated module', () => {\n\
  listUsers();\n\
});\n";
const CLOCK: &str = "export function now() { return 1; }\n";

struct Listed {
  test: String,
  reach: TestReach,
  note: Option<String>,
}

fn listed_with(target: &str, extra: &[(&str, &str)]) -> Vec<Listed> {
  let mut files = vec![("package.json", PACKAGE), ("src/db.ts", DB), ("src/users.service.ts", USERS_SERVICE)];
  files.extend_from_slice(extra);
  let fixture = common::single_repo("shop", &files);
  let Targeted::Found(answer) = fixture.query.tests_for(&TestsForRequest::new(target)).unwrap().answer else {
    panic!("{target} did not resolve");
  };
  answer
    .tests
    .items
    .into_iter()
    .map(|test| Listed {
      test: test.test.short,
      reach: test.reach,
      note: test.note,
    })
    .collect()
}

fn listed(target: &str) -> Vec<Listed> {
  listed_with(
    target,
    &[
      ("src/users.service.test.ts", USERS_SPEC),
      ("src/shutdown.test.ts", SHUTDOWN_SPEC),
      ("src/real.test.ts", REAL_SPEC),
    ],
  )
}

fn summary(tests: &[Listed]) -> Vec<(&str, TestReach, Option<&str>)> {
  tests.iter().map(|test| (test.test.as_str(), test.reach, test.note.as_deref())).collect()
}

#[test]
fn should_report_every_reach_under_a_module_mock_as_scope_mocked_and_keep_other_files_real() {
  assert_eq!(
    summary(&listed("query")),
    [
      ("lists users for real", TestReach::CallGraph, None),
      ("users", TestReach::ScopeMocked, Some("file src/users.service.test.ts mocks query [heuristic 0.60]")),
      ("users.checks nothing", TestReach::ScopeMocked, None),
      (
        "users.lists users",
        TestReach::ScopeMocked,
        Some("file src/users.service.test.ts mocks query [heuristic 0.60]")
      ),
    ]
  );
}

#[test]
fn should_keep_plain_reaches_of_exports_the_factory_does_not_replace() {
  assert_eq!(summary(&listed("close")), [("shuts down for real", TestReach::CallGraph, None)]);
}

#[test]
fn should_report_a_reach_in_a_file_that_does_not_import_the_replaced_name_as_scope_mocked() {
  let tests = listed_with("query", &[("src/blind.test.ts", BLIND_SPEC)]);
  assert_eq!(
    summary(&tests),
    [(
      "lists users without importing the replaced name",
      TestReach::ScopeMocked,
      Some("file src/blind.test.ts mocks src/db.ts [heuristic 0.60]")
    )]
  );
}

#[test]
fn should_treat_the_whole_module_as_replaced_for_exports_the_factory_omits() {
  let tests = listed_with("close", &[("src/blind.test.ts", BLIND_SPEC), ("src/shutdown.test.ts", SHUTDOWN_SPEC)]);
  assert_eq!(summary(&tests), [("shuts down for real", TestReach::CallGraph, None)]);
  let shut = "import { it, mock } from 'bun:test';\nimport { shutdown } from './users.service';\nmock.module('./db', () => ({}));\nit('shuts down on a replaced module', () => {\n  shutdown();\n});\n";
  let tests = listed_with("close", &[("src/shut.test.ts", shut)]);
  assert_eq!(
    summary(&tests),
    [(
      "shuts down on a replaced module",
      TestReach::ScopeMocked,
      Some("file src/shut.test.ts mocks src/db.ts [heuristic 0.60]")
    )]
  );
}

#[test]
fn should_treat_a_module_mock_inside_a_test_body_as_replacing_the_module_for_the_whole_file() {
  let tests = listed_with("query", &[("src/late.test.ts", LATE_SPEC)]);
  assert_eq!(
    summary(&tests),
    [
      (
        "replaces the module inside its own body",
        TestReach::ScopeMocked,
        Some("file src/late.test.ts mocks src/db.ts [heuristic 0.60]")
      ),
      (
        "runs without a replacement of its own",
        TestReach::ScopeMocked,
        Some("file src/late.test.ts mocks src/db.ts [heuristic 0.60]")
      ),
    ]
  );
}

#[test]
fn should_report_a_test_that_replaces_its_own_subject_module_as_scope_mocked_not_covering_it() {
  let own = "import { it, mock } from 'bun:test';\nmock.module('./db', () => ({}));\nit('exercises nothing real', () => {});\n";
  let tests = listed_with("src/db.ts", &[("src/db.test.ts", own)]);
  assert_eq!(
    summary(&tests),
    [(
      "exercises nothing real",
      TestReach::ScopeMocked,
      Some("file src/db.test.ts mocks src/db.ts [heuristic 0.60]")
    )]
  );
}

#[test]
fn should_report_a_direct_call_of_a_replaced_export_as_scope_mocked() {
  let tests = listed_with("query", &[("src/direct.test.ts", DIRECT_SPEC)]);
  assert_eq!(
    summary(&tests),
    [(
      "calls the replaced export directly",
      TestReach::ScopeMocked,
      Some("file src/direct.test.ts mocks query [heuristic 0.60]")
    )]
  );
}

#[test]
fn should_keep_a_reach_real_when_the_file_replaces_an_unrelated_module() {
  let tests = listed_with("query", &[("src/other.test.ts", OTHER_MODULE_SPEC), ("src/clock.ts", CLOCK)]);
  assert_eq!(summary(&tests), [("replaces an unrelated module", TestReach::CallGraph, None)]);
}

const REPO: &str = "export class Repo {\n  save(value: string) { return value; }\n  remove(value: string) { return value; }\n}\n\
export class Cache {\n  save(value: string) { return value; }\n}\n\
export class Queue {\n  save(value: string) { return value; }\n}\n\
export class Store {\n  save(value: string) { return value; }\n}\n";
const PLAIN_REPO_SPEC: &str = "import { describe, it } from 'bun:test';\n\
import { Repo } from './repo';\n\
\n\
describe('plain', () => {\n\
  it('saves through a built value', () => {\n\
    const { repo } = build();\n\
    repo.save('a');\n\
  });\n\
});\n";
const REPLACED_REPO_SPEC: &str = "import { describe, it, mock } from 'bun:test';\n\
import { Repo } from './repo';\n\
\n\
mock.module('./repo', () => ({ Repo: mock(() => ({})) }));\n\
\n\
describe('replaced', () => {\n\
  it('saves through a replaced module', () => {\n\
    const { repo } = build();\n\
    repo.save('a');\n\
  });\n\
});\n";
const BLIND_REPO_SPEC: &str = "import { describe, it, mock } from 'bun:test';\n\
import { Repo } from './repo';\n\
\n\
mock.module('./repo', () => ({}));\n\
\n\
describe('blind', () => {\n\
  it('saves through a module replaced without a factory export', () => {\n\
    const { repo } = build();\n\
    repo.save('a');\n\
  });\n\
});\n";

fn fallback_of(specs: &[(&str, &str)]) -> (Vec<(String, String)>, usize, Vec<String>) {
  let mut files = vec![("package.json", PACKAGE), ("src/repo.ts", REPO)];
  files.extend_from_slice(specs);
  let fixture = common::single_repo("shop", &files);
  let Targeted::Found(answer) = fixture.query.tests_for(&TestsForRequest::new("Repo.save")).unwrap().answer else {
    panic!("Repo.save did not resolve");
  };
  let Some(fallback) = answer.fallback else { return (Vec::new(), 0, Vec::new()) };
  let items = fallback.tests.items.into_iter().map(|test| (test.test.short, test.note.unwrap_or_default())).collect();
  (items, fallback.suppressed_by_mocks, fallback.replaced)
}

#[test]
fn should_list_a_plain_fallback_candidate_of_a_class_whose_module_is_not_replaced() {
  let (items, suppressed, _) = fallback_of(&[("src/plain.test.ts", PLAIN_REPO_SPEC)]);
  assert_eq!(items, [("plain.saves through a built value".to_owned(), String::new())]);
  assert_eq!(suppressed, 0);
}

#[test]
fn should_leave_fallback_candidates_under_a_module_mock_of_the_class_file_out_and_say_what_was_replaced() {
  let (items, suppressed, replaced) = fallback_of(&[
    ("src/plain.test.ts", PLAIN_REPO_SPEC),
    ("src/replaced.test.ts", REPLACED_REPO_SPEC),
    ("src/blind.test.ts", BLIND_REPO_SPEC),
  ]);
  assert_eq!(items, [("plain.saves through a built value".to_owned(), String::new())]);
  assert_eq!(suppressed, 2);
  assert_eq!(replaced, ["Repo", "src/repo.ts"]);
}

#[test]
fn should_list_a_test_file_that_only_replaces_a_module_among_relevant_tests_not_direct_dependents() {
  let fixture = common::single_repo(
    "shop",
    &[
      ("package.json", PACKAGE),
      ("src/db.ts", DB),
      ("src/users.service.ts", USERS_SERVICE),
      ("src/blind.test.ts", BLIND_SPEC),
    ],
  );
  let Targeted::Found(answer) = fixture.query.impact(&ImpactRequest::new(["src/db.ts"])).unwrap().answer else {
    panic!("src/db.ts did not resolve");
  };
  let names = |items: &[atlas_query::ImpactItem]| items.iter().map(|item| item.entity.short.clone()).collect::<Vec<_>>();
  assert_eq!(names(&answer.direct.items), ["listUsers", "shutdown", "src/users.service.ts"]);
  assert_eq!(names(&answer.tests.items), ["lists users without importing the replaced name", "src/blind.test.ts"]);
}
