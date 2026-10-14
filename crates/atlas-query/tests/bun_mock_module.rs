mod common;

use atlas_query::{Targeted, TestReach, TestsForRequest};

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

struct Listed {
  test: String,
  reach: TestReach,
  note: Option<String>,
}

fn listed(target: &str) -> Vec<Listed> {
  let fixture = common::single_repo(
    "shop",
    &[
      ("package.json", PACKAGE),
      ("src/db.ts", DB),
      ("src/users.service.ts", USERS_SERVICE),
      ("src/users.service.test.ts", USERS_SPEC),
      ("src/shutdown.test.ts", SHUTDOWN_SPEC),
      ("src/real.test.ts", REAL_SPEC),
    ],
  );
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

fn summary(tests: &[Listed]) -> Vec<(&str, TestReach, Option<&str>)> {
  tests.iter().map(|test| (test.test.as_str(), test.reach, test.note.as_deref())).collect()
}

#[test]
fn should_note_a_module_mock_on_the_reaches_of_the_replacing_file_and_hedge_its_other_tests() {
  assert_eq!(
    summary(&listed("query")),
    [
      ("users", TestReach::ImportedByTestFile, Some("file src/users.service.test.ts mocks query [heuristic 0.60]")),
      ("lists users for real", TestReach::CallGraph, None),
      (
        "users.lists users",
        TestReach::CallGraph,
        Some("file src/users.service.test.ts mocks query [heuristic 0.60]")
      ),
      ("users.checks nothing", TestReach::ScopeMocked, None),
    ]
  );
}

#[test]
fn should_keep_plain_reaches_of_exports_the_factory_does_not_replace() {
  assert_eq!(summary(&listed("close")), [("shuts down for real", TestReach::CallGraph, None)]);
}

#[test]
fn should_currently_report_a_test_that_does_not_import_the_replaced_name_as_an_unnoted_real_reach() {
  let fixture = common::single_repo(
    "shop",
    &[
      ("package.json", PACKAGE),
      ("src/db.ts", DB),
      ("src/users.service.ts", USERS_SERVICE),
      ("src/blind.test.ts", BLIND_SPEC),
    ],
  );
  let Targeted::Found(answer) = fixture.query.tests_for(&TestsForRequest::new("query")).unwrap().answer else {
    panic!("query did not resolve");
  };
  let tests: Vec<Listed> = answer
    .tests
    .items
    .into_iter()
    .map(|test| Listed {
      test: test.test.short,
      reach: test.reach,
      note: test.note,
    })
    .collect();
  assert_eq!(summary(&tests), [("lists users without importing the replaced name", TestReach::CallGraph, None)]);
}
