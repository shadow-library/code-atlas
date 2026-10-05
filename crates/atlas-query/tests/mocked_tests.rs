mod common;

use atlas_query::{Targeted, TestReach, TestsForRequest};

const PACKAGE: &str = r#"{ "name": "shop", "version": "1.0.0" }"#;
const SERVICE: &str = "export class OrdersService {\n\
  findOne(id: string) { return id; }\n\
  audit() { return 1; }\n\
  onlyMocked() { return 2; }\n\
  spied() { return 3; }\n\
  stubbed() { return 4; }\n\
}\n";
const CONTROLLER: &str = "import { OrdersService } from './orders.service';\n\
export class OrdersController {\n\
  constructor(private readonly orders: OrdersService) {}\n\
  findOne(id: string) { return this.orders.findOne(id); }\n\
  audit() { return this.orders.audit(); }\n\
  spied() { return this.orders.spied(); }\n\
  stubbed() { return this.orders.stubbed(); }\n\
}\n";
const CONTROLLER_SPEC: &str = "import { OrdersController } from './orders.controller';\n\
import { OrdersService } from './orders.service';\n\
\n\
function makeService() {\n\
  return { onlyMocked: jest.fn() } as unknown as OrdersService;\n\
}\n\
\n\
describe('OrdersController', () => {\n\
  const ordersService = { findOne: jest.fn() } as unknown as OrdersService;\n\
  const controller = new OrdersController(ordersService);\n\
\n\
  it('reads through the mocked service', () => {\n\
    controller.findOne('o1');\n\
    controller.audit();\n\
  });\n\
\n\
  it('finds orders for real', () => {\n\
    const service = new OrdersService();\n\
    service.findOne('o1');\n\
  });\n\
\n\
  it('builds its service through a helper', () => {\n\
    makeService();\n\
  });\n\
\n\
  it('spies on a real service', () => {\n\
    const real = new OrdersService();\n\
    jest.spyOn(real, 'spied');\n\
    const wired = new OrdersController(real);\n\
    wired.spied();\n\
  });\n\
\n\
  it('stubs a real service', () => {\n\
    const real = new OrdersService();\n\
    jest.spyOn(real, 'stubbed').mockReturnValue(9);\n\
    const wired = new OrdersController(real);\n\
    wired.stubbed();\n\
  });\n\
});\n";
const SERVICE_SPEC: &str = "import { OrdersService } from './orders.service';\n\
\n\
describe('OrdersService', () => {\n\
  const service = { audit: jest.fn() } as unknown as OrdersService;\n\
  it('counts audits', () => {\n\
    service.audit.mockReturnValue(1);\n\
  });\n\
});\n";

const BILLING_SERVICE: &str = "export class BillingService {\n  charge(amount: number) { return amount; }\n}\n";
const BILLING_CONTROLLER: &str = "import { BillingService } from './billing.service';\n\
export class BillingController {\n\
  constructor(private readonly billing: BillingService) {}\n\
  pay(amount: number) { return this.billing.charge(amount); }\n\
}\n";
const BILLING_SPEC: &str = "import { BillingController } from './billing.controller';\n\
import { BillingService } from './billing.service';\n\
\n\
describe('BillingController', () => {\n\
  const billing = { charge: jest.fn() } as unknown as BillingService;\n\
\n\
  it('pays through a real service', () => {\n\
    const c: BillingController = new BillingController(new BillingService());\n\
    c.pay(5);\n\
  });\n\
\n\
  it('only lives beside the mock', () => {\n\
    expect(billing).toBeDefined();\n\
  });\n\
});\n";

struct Listed {
  test: String,
  reach: TestReach,
  relation: String,
  note: Option<String>,
}

fn listed(target: &str) -> Vec<Listed> {
  let fixture = common::single_repo(
    "shop",
    &[
      ("package.json", PACKAGE),
      ("src/orders.service.ts", SERVICE),
      ("src/orders.controller.ts", CONTROLLER),
      ("src/orders.controller.spec.ts", CONTROLLER_SPEC),
      ("src/orders.service.spec.ts", SERVICE_SPEC),
      ("src/billing.service.ts", BILLING_SERVICE),
      ("src/billing.controller.ts", BILLING_CONTROLLER),
      ("src/billing.controller.spec.ts", BILLING_SPEC),
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
      relation: format!("{} {} {}", test.relation.phrase, test.relation.entity, test.relation.certainty.tag),
      note: test.note,
    })
    .collect()
}

fn tests_for(target: &str) -> Vec<(String, TestReach)> {
  listed(target).into_iter().map(|test| (test.test, test.reach)).collect()
}

fn reaches(expected: &[(&str, TestReach)]) -> Vec<(String, TestReach)> {
  expected.iter().map(|(name, reach)| ((*name).to_owned(), *reach)).collect()
}

#[test]
fn should_keep_a_direct_caller_beside_a_suite_that_mocks_the_target() {
  assert_eq!(
    tests_for("OrdersService.findOne"),
    reaches(&[
      ("OrdersController.finds orders for real", TestReach::DirectCall),
      ("OrdersService", TestReach::CoversFile),
      ("OrdersController.reads through the mocked service", TestReach::CallGraph),
      ("OrdersController", TestReach::Mocked),
      ("OrdersController.builds its service through a helper", TestReach::ScopeMocked),
      ("OrdersController.spies on a real service", TestReach::ScopeMocked),
      ("OrdersController.stubs a real service", TestReach::ScopeMocked),
    ])
  );
}

#[test]
fn should_keep_a_real_path_beside_a_suite_mock_and_hedge_tests_that_only_share_the_suite() {
  let tests = listed("BillingService.charge");

  let summary: Vec<(&str, TestReach, &str, Option<&str>)> = tests
    .iter()
    .map(|test| (test.test.as_str(), test.reach, test.relation.as_str(), test.note.as_deref()))
    .collect();
  assert_eq!(
    summary,
    [
      (
        "BillingController.pays through a real service",
        TestReach::CallGraph,
        "calls BillingController.pay [resolved]",
        Some("suite BillingController mocks BillingService.charge [heuristic 0.60]"),
      ),
      ("BillingController", TestReach::Mocked, "possibly references BillingService.charge [heuristic 0.60]", None),
      (
        "BillingController.only lives beside the mock",
        TestReach::ScopeMocked,
        "suite BillingController mocks BillingService.charge [heuristic 0.60]",
        None,
      ),
    ]
  );
}

#[test]
fn should_not_override_covers_file_and_keep_call_graph_reach_for_unmocked_members() {
  assert_eq!(
    tests_for("OrdersService.audit"),
    reaches(&[
      ("OrdersService", TestReach::CoversFile),
      ("OrdersController.reads through the mocked service", TestReach::CallGraph),
      ("OrdersService.counts audits", TestReach::Mocked),
    ])
  );
}

#[test]
fn should_classify_a_mock_built_by_a_helper_as_mocking_not_call_graph_reach() {
  assert_eq!(
    tests_for("OrdersService.onlyMocked"),
    reaches(&[
      ("OrdersService", TestReach::CoversFile),
      ("OrdersController.builds its service through a helper", TestReach::Mocked)
    ])
  );
}

#[test]
fn should_report_a_spy_on_a_real_instance_as_spying_and_a_stubbed_spy_as_mocking() {
  assert_eq!(
    tests_for("OrdersService.spied"),
    reaches(&[("OrdersService", TestReach::CoversFile), ("OrdersController.spies on a real service", TestReach::Spied)])
  );
  assert_eq!(
    tests_for("OrdersService.stubbed"),
    reaches(&[("OrdersService", TestReach::CoversFile), ("OrdersController.stubs a real service", TestReach::Mocked)])
  );
}
