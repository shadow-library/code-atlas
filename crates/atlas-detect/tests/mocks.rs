mod common;

use atlas_core::{ATTR_MOCK, ATTR_MOCK_TYPE, EdgeKind, NodeRef};
use atlas_detect::MockDetector;
use common::{analyze, fixture_files, lines, render_file, render_source, run};

fn sites(repo: &str, path: &str, source: &str) -> Vec<String> {
  let analyzed = analyze(repo, path, source);
  site_lines(&analyzed)
}

fn site_lines(analyzed: &common::Analyzed) -> Vec<String> {
  let (detection, _) = run(&MockDetector, analyzed);
  let mut sites: Vec<String> = detection
    .edges
    .iter()
    .map(|edge| {
      assert_eq!(edge.kind, EdgeKind::References);
      let NodeRef::Unresolved { path, from } = &edge.to else {
        panic!("mock edges target unresolved paths: {edge:?}")
      };
      assert_eq!(edge.from, NodeRef::Local(from.unwrap()));
      let text = &analyzed.source[edge.span.start_byte as usize..edge.span.end_byte as usize];
      format!(
        "{}:{} {} {} {}/{} {}",
        edge.span.start_line,
        text,
        path.join("."),
        edge.certainty.tag(),
        edge.attrs["mock_site"].as_str().unwrap(),
        edge.attrs[ATTR_MOCK_TYPE].as_str().unwrap(),
        edge.attrs[ATTR_MOCK].as_str().unwrap()
      )
    })
    .collect();
  sites.sort();
  sites
}

#[test]
fn should_emit_heuristic_mock_references_for_the_acme_controller_spec() {
  assert_eq!(
    render_file(&MockDetector, "acme/acme-api", "src/orders/orders.controller.spec.ts"),
    lines(&[
      "references OrdersController -> ?OrdersService::prototype::create [heuristic 0.60]",
      "references OrdersController -> ?OrdersService::prototype::findOne [heuristic 0.60]",
      "references OrdersController.should delegate create to the service -> ?OrdersService::prototype::create [heuristic 0.60]",
      "references OrdersController.should delegate findOne to the service -> ?OrdersService::prototype::findOne [heuristic 0.60]",
    ])
  );
  let analyzed = fixture_files("acme/acme-api")
    .into_iter()
    .find(|(path, _)| path == "src/orders/orders.controller.spec.ts")
    .unwrap()
    .1;
  assert_eq!(
    site_lines(&analyzed),
    [
      "11:findOne OrdersService.prototype.findOne [heuristic 0.60] mock_setup/cast replaced",
      "13:findOne OrdersService.prototype.findOne [heuristic 0.60] call_assertion/cast spied",
      "17:create OrdersService.prototype.create [heuristic 0.60] mock_setup/cast replaced",
      "19:create OrdersService.prototype.create [heuristic 0.60] call_assertion/cast spied",
      "5:create OrdersService.prototype.create [heuristic 0.60] fn_property/cast replaced",
      "5:findOne OrdersService.prototype.findOne [heuristic 0.60] fn_property/cast replaced",
    ]
  );
}

#[test]
fn should_infer_the_mocked_type_from_providers_mocked_types_factories_and_names() {
  let source = "import { Test } from '@nestjs/testing';\n\
import { createMock } from '@golevelup/ts-jest';\n\
import { OrdersService } from './orders.service';\n\
import { UsersService } from './users.service';\n\
import { Mailer } from './mailer';\n\
import { Clock } from './clock';\n\
import { Audit } from './audit';\n\
\n\
describe('OrdersController', () => {\n\
  const users = { findOne: vi.fn() };\n\
  let orders: jest.Mocked<OrdersService>;\n\
  const mailer = createMock<Mailer>();\n\
  const usersService = {};\n\
  beforeEach(async () => {\n\
    orders = { findOne: jest.fn() } as unknown as jest.Mocked<OrdersService>;\n\
    await Test.createTestingModule({\n\
      providers: [{ provide: UsersService, useValue: users }, { provide: Audit, useValue: { record: jest.fn() } }],\n\
    }).compile();\n\
  });\n\
  it('works', () => {\n\
    orders.findOne.mockReturnValueOnce(null);\n\
    mailer.send.mockImplementation(() => undefined);\n\
    usersService.remove.mockRejectedValue(new Error('x'));\n\
    jest.spyOn(Clock.prototype, 'now');\n\
    expect(orders.findOne).not.toHaveBeenCalled();\n\
  });\n\
});\n";
  assert_eq!(
    sites("api", "src/orders/orders.controller.spec.ts", source),
    [
      "10:findOne UsersService.prototype.findOne [heuristic 0.60] fn_property/provider replaced",
      "15:findOne OrdersService.prototype.findOne [heuristic 0.60] fn_property/cast replaced",
      "17:record Audit.prototype.record [heuristic 0.60] fn_property/provider replaced",
      "21:findOne OrdersService.prototype.findOne [heuristic 0.60] mock_setup/mocked_type replaced",
      "22:send Mailer.prototype.send [heuristic 0.60] mock_setup/create_mock replaced",
      "23:remove UsersService.prototype.remove [heuristic 0.60] mock_setup/naming replaced",
      "24:now Clock.prototype.now [heuristic 0.60] spy_on/prototype spied",
      "25:findOne OrdersService.prototype.findOne [heuristic 0.60] call_assertion/mocked_type replaced",
    ]
  );
}

#[test]
fn should_type_spies_on_instances_and_classes() {
  let source = "import { UsersService } from './users.service';\n\
import { Clock } from './clock';\n\
\n\
describe('UsersService', () => {\n\
  it('spies', () => {\n\
    const service = new UsersService(repository);\n\
    vi.spyOn(service, 'findOne').mockResolvedValue(null);\n\
    jest.spyOn(Clock, 'now');\n\
    service.findOne.mockClear();\n\
  });\n\
});\n";
  assert_eq!(
    sites("api", "src/users/users.service.test.ts", source),
    [
      "7:findOne UsersService.prototype.findOne [heuristic 0.60] spy_on/instantiation replaced",
      "8:now Clock.now [heuristic 0.60] spy_on/class spied",
      "9:findOne UsersService.prototype.findOne [heuristic 0.60] mock_setup/instantiation spied",
    ]
  );
}

#[test]
fn should_ignore_mocks_outside_test_code_and_untyped_or_ambiguous_mocks() {
  let mocks = "import { OrdersService } from './orders.service';\n\
export function stub() {\n\
  const ordersService = { findOne: jest.fn() } as unknown as OrdersService;\n\
  ordersService.findOne.mockResolvedValue(null);\n\
  return ordersService;\n\
}\n";
  assert!(render_source(&MockDetector, "api", "src/orders/orders.stub.ts", mocks).is_empty());
  let untyped = "import { OrdersService } from './orders.service';\n\
import { Mailer } from './mailer';\n\
import { Other } from './other';\n\
describe('x', () => {\n\
  const repository = { findOneBy: jest.fn() };\n\
  const service = new OrdersService(repository as never);\n\
  const ordersService = { run: jest.fn() };\n\
  use(ordersService as unknown as Mailer);\n\
  use(ordersService as unknown as Other);\n\
  it('y', () => {\n\
    repository.findOneBy.mockResolvedValue(null);\n\
    expect(ordersService.run).toHaveBeenCalled();\n\
    this.ordersService.findOne.mockResolvedValue(null);\n\
    expect(service.findOne).toBe(1);\n\
    unknownThing.findOne.mockResolvedValue(null);\n\
    jest.spyOn(console, 'log');\n\
  });\n\
});\n";
  assert!(render_source(&MockDetector, "api", "src/orders/orders.spec.ts", untyped).is_empty());
}

#[test]
fn should_scope_each_mock_to_the_nearest_visible_declaration() {
  let source = "import { OrdersService } from './orders.service';\n\
import { UsersService } from './users.service';\n\
describe('a', () => {\n\
  const service = { findOne: jest.fn() } as unknown as OrdersService;\n\
  describe('nested', () => { const service = createMock<UsersService>(); });\n\
  it('a1', () => { service.findOne.mockResolvedValue(null); });\n\
});\n\
describe('b', () => {\n\
  const service = { findOne: jest.fn() } as unknown as UsersService;\n\
  it('b1', () => { service.findOne.mockResolvedValue(null); });\n\
});\n";
  assert_eq!(
    sites("api", "src/app.spec.ts", source),
    [
      "10:findOne UsersService.prototype.findOne [heuristic 0.60] mock_setup/cast replaced",
      "4:findOne OrdersService.prototype.findOne [heuristic 0.60] fn_property/cast replaced",
      "6:findOne OrdersService.prototype.findOne [heuristic 0.60] mock_setup/cast replaced",
      "9:findOne UsersService.prototype.findOne [heuristic 0.60] fn_property/cast replaced",
    ]
  );
}

const BUN_ORDERS_SPEC: &str = "import { describe, it, mock, spyOn } from 'bun:test';\n\
import { OrdersService } from './orders.service';\n\
\n\
describe('Orders', () => {\n\
  it('replaces a property', () => {\n\
    const service = { findOne: mock(() => 1) } as unknown as OrdersService;\n\
  });\n\
  it('spies', () => {\n\
    const real = new OrdersService();\n\
    spyOn(real, 'audit');\n\
  });\n\
  it('stubs', () => {\n\
    const real = new OrdersService();\n\
    spyOn(real, 'create').mockReturnValue(1);\n\
  });\n\
});\n";

#[test]
fn should_treat_bun_test_mock_and_spy_on_like_their_jest_counterparts() {
  assert_eq!(
    sites("api", "src/orders.spec.ts", BUN_ORDERS_SPEC),
    [
      "10:audit OrdersService.prototype.audit [heuristic 0.60] spy_on/instantiation spied",
      "14:create OrdersService.prototype.create [heuristic 0.60] spy_on/instantiation replaced",
      "6:findOne OrdersService.prototype.findOne [heuristic 0.60] fn_property/cast replaced",
    ]
  );
}

#[test]
fn should_follow_bun_test_aliases_and_namespace_imports() {
  let aliased = "import { mock as fake, spyOn as spy } from 'bun:test';\nimport { OrdersService } from './orders.service';\n\
it('a', () => {\n  const service = { findOne: fake(() => 1) } as unknown as OrdersService;\n  const real = new OrdersService();\n  spy(real, 'audit').mockResolvedValue(2);\n});\n";
  assert_eq!(
    sites("api", "src/orders.spec.ts", aliased),
    [
      "4:findOne OrdersService.prototype.findOne [heuristic 0.60] fn_property/cast replaced",
      "6:audit OrdersService.prototype.audit [heuristic 0.60] spy_on/instantiation replaced",
    ]
  );
  let namespaced = "import * as bun from 'bun:test';\nimport { OrdersService } from './orders.service';\n\
it('a', () => {\n  const service = { findOne: bun.mock(() => 1) } as unknown as OrdersService;\n  const real = new OrdersService();\n  bun.spyOn(real, 'audit');\n});\n";
  assert_eq!(
    sites("api", "src/orders.spec.ts", namespaced),
    [
      "4:findOne OrdersService.prototype.findOne [heuristic 0.60] fn_property/cast replaced",
      "6:audit OrdersService.prototype.audit [heuristic 0.60] spy_on/instantiation spied",
    ]
  );
}

#[test]
fn should_not_match_mock_or_spy_on_that_bun_test_does_not_provide() {
  let local = "import { OrdersService } from './orders.service';\nfunction mock(fn: () => number) { return fn; }\nfunction spyOn(target: object, name: string) { return target; }\n\
it('a', () => {\n  const service = { findOne: mock(() => 1) } as unknown as OrdersService;\n  const real = new OrdersService();\n  spyOn(real, 'audit');\n  mock.module('./x', () => ({ a: 1 }));\n});\n";
  assert_eq!(sites("api", "src/orders.spec.ts", local), Vec::<String>::new());
  let jest = "import { spyOn } from '@jest/globals';\nimport { mock } from 'jest-mock-extended';\nimport { OrdersService } from './orders.service';\n\
it('a', () => {\n  const service = { findOne: mock(() => 1) } as unknown as OrdersService;\n  const real = new OrdersService();\n  spyOn(real, 'audit');\n});\n";
  assert_eq!(sites("api", "src/orders.spec.ts", jest), Vec::<String>::new());
  let shadowed = "import { mock, spyOn } from 'bun:test';\nimport { OrdersService } from './orders.service';\n\
it('a', () => {\n  const mock = (fn: () => number) => fn;\n  const spyOn = (target: object, name: string) => target;\n  const service = { findOne: mock(() => 1) } as unknown as OrdersService;\n  const real = new OrdersService();\n  spyOn(real, 'audit');\n});\n";
  assert_eq!(sites("api", "src/orders.spec.ts", shadowed), Vec::<String>::new());
  let inner_block = "import { mock, spyOn } from 'bun:test';\nimport { OrdersService } from './orders.service';\n\
it('a', () => {\n  if (true) { const mock = 1; const spyOn = 2; }\n  const service = { findOne: mock(() => 1) } as unknown as OrdersService;\n  const real = new OrdersService();\n  spyOn(real, 'audit');\n});\n";
  let expected = [
    "5:findOne OrdersService.prototype.findOne [heuristic 0.60] fn_property/cast replaced",
    "7:audit OrdersService.prototype.audit [heuristic 0.60] spy_on/instantiation spied",
  ];
  assert_eq!(sites("api", "src/orders.spec.ts", inner_block), expected);
  let arrow_helper = inner_block.replace("if (true) { const mock = 1; const spyOn = 2; }", "const helper = () => { const mock = 1; return mock; };");
  assert_eq!(sites("api", "src/orders.spec.ts", &arrow_helper), expected);
  let nested_function = inner_block.replace("if (true) { const mock = 1; const spyOn = 2; }", "function helper() { const spyOn = 1; return spyOn; }");
  assert_eq!(sites("api", "src/orders.spec.ts", &nested_function), expected);
  let mixed = "import { mock } from 'bun:test';\nimport { OrdersService } from './orders.service';\n\
it('a', () => {\n  const real = new OrdersService();\n  spyOn(real, 'audit');\n});\n";
  assert_eq!(sites("api", "src/orders.spec.ts", mixed), Vec::<String>::new());
}

fn module_edges(path: &str, source: &str) -> (Vec<String>, Vec<String>) {
  let analyzed = analyze("api", path, source);
  let (detection, diagnostics) = run(&MockDetector, &analyzed);
  let mut edges: Vec<String> = detection
    .edges
    .iter()
    .map(|edge| {
      let text = &analyzed.source[edge.span.start_byte as usize..edge.span.end_byte as usize];
      format!(
        "{} -> {} {} {}/{}/{} `{text}`",
        common::node(&analyzed.facts, &edge.from),
        common::node(&analyzed.facts, &edge.to),
        edge.certainty.tag(),
        edge.attrs["mock_site"].as_str().unwrap(),
        edge.attrs[ATTR_MOCK_TYPE].as_str().unwrap(),
        edge.attrs[ATTR_MOCK].as_str().unwrap(),
      )
    })
    .collect();
  edges.sort();
  (edges, diagnostics.into_iter().map(|diagnostic| diagnostic.code).collect())
}

fn module_files(specifier_span: &str, stems: &[&str]) -> Vec<String> {
  stems
    .iter()
    .map(|stem| format!("file:api:src/users.test.ts -> file:api:{stem} [heuristic 0.60] module/module_file/replaced `{specifier_span}`"))
    .collect()
}

fn module_exports(names: &[&str]) -> Vec<String> {
  names
    .iter()
    .map(|name| format!("file:api:src/users.test.ts -> ?{name} [heuristic 0.60] module/module_export/replaced `{name}`"))
    .collect()
}

fn sorted_all(groups: &[Vec<String>]) -> Vec<String> {
  let mut all: Vec<String> = groups.concat();
  all.sort();
  all
}

#[test]
fn should_replace_a_module_listing_its_factory_exports_and_its_relative_file() {
  let source = "import { mock } from 'bun:test';\nimport { query } from './db';\n\
mock.module('./db', () => ({ query: mock(() => []), 'close': 1, helper() {}, default: 1, ...rest }));\n\
it('a', () => {});\n";
  let (edges, diagnostics) = module_edges("src/users.test.ts", source);
  assert_eq!(
    edges,
    sorted_all(&[
      module_exports(&["close", "helper", "query"]),
      module_files(
        "./db",
        &[
          "src/db.js",
          "src/db.jsx",
          "src/db.ts",
          "src/db.tsx",
          "src/db/index.js",
          "src/db/index.ts",
          "src/db/index.tsx"
        ]
      ),
    ])
  );
  assert_eq!(diagnostics, Vec::<String>::new());
}

#[test]
fn should_read_block_bodied_factories_aliases_namespaces_and_explicit_extensions() {
  let block = "import { mock as fake } from 'bun:test';\nfake.module('../lib/db.js', () => { return { a: 1, b: fake(() => 2) }; });\n";
  assert_eq!(
    module_edges("src/users.test.ts", block).0,
    sorted_all(&[module_exports(&["a", "b"]), module_files("../lib/db.js", &["lib/db.js", "lib/db.ts", "lib/db.tsx"])])
  );
  let namespaced = "import * as bun from 'bun:test';\nbun.mock.module('./db.ts', () => ({ q: 1 }));\n";
  assert_eq!(
    module_edges("src/users.test.ts", namespaced).0,
    sorted_all(&[module_exports(&["q"]), module_files("./db.ts", &["src/db.ts"])])
  );
}

#[test]
fn should_replace_only_the_listed_exports_of_a_package_specifier_and_only_the_file_of_an_opaque_factory() {
  let package = "import { mock } from 'bun:test';\nmock.module('@acme/db', () => ({ query: 1 }));\n";
  assert_eq!(module_edges("src/users.test.ts", package).0, module_exports(&["query"]));
  let opaque = "import { mock } from 'bun:test';\nmock.module('./db', makeFactory);\n";
  assert_eq!(
    module_edges("src/users.test.ts", opaque).0,
    module_files(
      "./db",
      &[
        "src/db.js",
        "src/db.jsx",
        "src/db.ts",
        "src/db.tsx",
        "src/db/index.js",
        "src/db/index.ts",
        "src/db/index.tsx"
      ]
    )
  );
}

#[test]
fn should_report_a_non_literal_module_specifier_and_skip_non_test_files_and_foreign_mock_modules() {
  let dynamic = "import { mock } from 'bun:test';\nmock.module(path, () => ({ q: 1 }));\n";
  assert_eq!(module_edges("src/users.test.ts", dynamic), (Vec::new(), vec!["mock.dynamic_module".to_owned()]));
  let production = "import { mock } from 'bun:test';\nmock.module('./db', () => ({ q: 1 }));\n";
  assert_eq!(module_edges("src/users.ts", production), (Vec::new(), Vec::new()));
  let foreign = "const mock = { module(name: string, factory: () => object) {} };\nmock.module('./db', () => ({ q: 1 }));\n";
  assert_eq!(module_edges("src/users.test.ts", foreign), (Vec::new(), Vec::new()));
}

fn large_spec(tests: usize, properties: usize) -> String {
  let mut source = String::from("import { Service } from './service';\n\ndescribe('Service', () => {\n  const shared = {\n");
  for index in 0..properties {
    source.push_str(&format!("    m{index}: jest.fn(),\n"));
  }
  source.push_str("  } as unknown as Service;\n");
  for index in 0..tests {
    source.push_str(&format!(
      "  it('case {index}', () => {{\n    const service = {{ find: jest.fn() }} as unknown as Service;\n    service.find.mockReturnValue({index});\n    shared.m{}.mockResolvedValue(null);\n    expect(shared.m{}).toHaveBeenCalled();\n  }});\n",
      index % properties.max(1),
      index % properties.max(1)
    ));
  }
  source.push_str("});\n");
  source
}

#[test]
fn should_detect_mocks_in_a_large_spec_in_near_linear_time() {
  let source = large_spec(2000, 5000);
  let analyzed = analyze("api", "src/service.spec.ts", &source);

  let started = std::time::Instant::now();
  let (detection, _) = run(&MockDetector, &analyzed);
  let detect = started.elapsed();
  let started = std::time::Instant::now();
  let (all, _) = atlas_detect::DetectorRegistry::default().run_all(&analyzed.input());
  let registry = started.elapsed();

  eprintln!("mock detector {detect:?}, all detectors {registry:?}");
  assert_eq!(detection.edges.len(), 5000 + 2000 * 4);
  assert!(all.edges.len() >= detection.edges.len());
  assert!(detect.as_secs_f64() < 2.0, "mock detector took {detect:?}");
  assert!(registry.as_secs_f64() < 4.0, "all detectors took {registry:?}");
}
