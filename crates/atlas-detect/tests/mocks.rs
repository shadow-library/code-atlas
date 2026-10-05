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
