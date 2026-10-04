mod common;

use std::sync::OnceLock;

use atlas_core::{CertaintyClass, ContentHash, EdgeKind, EntityId, EntityKey, EntityKind, PackageEcosystem, UnresolvedReason};
use atlas_resolve::manifest_edges;
use common::{Project, file, fixtures_root, rust, ts};

const RESOLVED: &[CertaintyClass] = &[CertaintyClass::Resolved];
const HEURISTIC: &[CertaintyClass] = &[CertaintyClass::Heuristic];
const STRUCTURAL: &[CertaintyClass] = &[CertaintyClass::Syntactic, CertaintyClass::Resolved];

fn acme() -> &'static Project {
  static PROJECT: OnceLock<Project> = OnceLock::new();
  PROJECT.get_or_init(|| {
    let root = fixtures_root().join("acme");
    Project::load(&[
      ("acme-api", root.join("acme-api")),
      ("acme-sdk", root.join("acme-sdk")),
      ("acme-web", root.join("acme-web")),
      ("acme-billing", root.join("acme-billing")),
    ])
  })
}

fn api(module: &[&str], items: &[&str], kind: EntityKind) -> EntityId {
  ts("acme-api", "acme-api", module, items, kind)
}

fn sdk(module: &[&str], items: &[&str], kind: EntityKind) -> EntityId {
  ts("acme-sdk", "@acme/sdk", module, items, kind)
}

fn web(module: &[&str], items: &[&str], kind: EntityKind) -> EntityId {
  ts("acme-web", "acme-web", module, items, kind)
}

fn billing(module: &[&str], items: &[&str], kind: EntityKind) -> EntityId {
  rust("acme-billing", "acme-billing", module, items, kind)
}

fn min_confidence(project: &Project, kind: EdgeKind, from: EntityId, to: EntityId, minimum: f32) {
  let edge = project.assert_edge(kind, from, to, &[CertaintyClass::Resolved, CertaintyClass::Heuristic]);
  if let Some(confidence) = edge.certainty.confidence() {
    assert!(confidence.value() >= minimum, "{:?} below {minimum}", edge.certainty);
  }
}

#[test]
fn should_resolve_the_nestjs_di_chain_through_constructor_parameter_properties() {
  let project = acme();
  let users = ["src", "users", "users.controller"];
  let service = ["src", "users", "users.service"];
  for method in ["findOne", "create", "update", "remove"] {
    project.assert_edge(
      EdgeKind::Calls,
      api(&users, &["UsersController", method], EntityKind::Method),
      api(&service, &["UsersService", method], EntityKind::Method),
      RESOLVED,
    );
  }
  project.assert_edge(
    EdgeKind::Calls,
    api(&service, &["UsersService", "update"], EntityKind::Method),
    api(&service, &["UsersService", "findOne"], EntityKind::Method),
    RESOLVED,
  );
  let orders = ["src", "orders", "orders.controller"];
  let orders_service = ["src", "orders", "orders.service"];
  project.assert_edge(
    EdgeKind::Calls,
    api(&orders, &["OrdersController", "findOne"], EntityKind::Method),
    api(&orders_service, &["OrdersService", "findOne"], EntityKind::Method),
    RESOLVED,
  );
  project.assert_no_edge(
    EdgeKind::Calls,
    api(&orders, &["OrdersController", "findOne"], EntityKind::Method),
    api(&service, &["UsersService", "findOne"], EntityKind::Method),
  );
  project.assert_edge(
    EdgeKind::Calls,
    api(&orders_service, &["OrdersService", "create"], EntityKind::Method),
    api(&["src", "billing", "billing.service"], &["BillingService", "applyTax"], EntityKind::Method),
    RESOLVED,
  );
}

#[test]
fn should_stop_the_di_chain_at_the_external_repository_with_its_package() {
  let project = acme();
  let find_one = api(&["src", "users", "users.service"], &["UsersService", "findOne"], EntityKind::Method);
  project.assert_unresolved(find_one, "findOneBy", &UnresolvedReason::External { package: "typeorm".to_owned() });
  assert!(project.edges_from(EdgeKind::Calls, find_one).is_empty());
}

#[test]
fn should_resolve_decorators_and_guard_references() {
  let project = acme();
  let controller = ["src", "users", "users.controller"];
  project.assert_edge(
    EdgeKind::Calls,
    api(&controller, &["UsersController", "findOne"], EntityKind::Method),
    api(&["src", "auth", "require-permission.decorator"], &["RequirePermission"], EntityKind::Function),
    RESOLVED,
  );
  project.assert_edge(
    EdgeKind::References,
    api(&controller, &["UsersController"], EntityKind::Class),
    api(&["src", "auth", "jwt-auth.guard"], &["JwtAuthGuard"], EntityKind::Class),
    RESOLVED,
  );
  project.assert_unresolved(
    api(&controller, &["UsersController"], EntityKind::Class),
    "Controller",
    &UnresolvedReason::External {
      package: "@nestjs/common".to_owned(),
    },
  );
}

#[test]
fn should_resolve_the_billing_persist_call_to_the_trait_method() {
  let project = acme();
  let persist = billing(&["handlers"], &["persist"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, persist, billing(&["store"], &["InvoiceStore", "insert"], EntityKind::Method), RESOLVED);
  for implementor in ["PgInvoiceStore", "InMemoryInvoiceStore"] {
    let candidate = billing(&["store"], &[implementor, "<InvoiceStore>", "insert"], EntityKind::Method);
    project.assert_edge(EdgeKind::Calls, persist, candidate, HEURISTIC);
    project.assert_no_edge_of_class(EdgeKind::Calls, persist, candidate, RESOLVED);
  }
  project.assert_edge(EdgeKind::Calls, billing(&["handlers"], &["create_invoice"], EntityKind::Function), persist, RESOLVED);
  project.assert_edge(
    EdgeKind::Calls,
    billing(&["handlers"], &["get_invoice"], EntityKind::Function),
    billing(&["store"], &["InvoiceStore", "find"], EntityKind::Method),
    RESOLVED,
  );
  project.assert_edge(
    EdgeKind::Calls,
    billing(&["events"], &["consume_order_created"], EntityKind::Function),
    billing(&["money"], &["to_cents"], EntityKind::Function),
    RESOLVED,
  );
}

#[test]
fn should_resolve_billing_structure_and_handler_values() {
  let project = acme();
  let store_trait = billing(&["store"], &["InvoiceStore"], EntityKind::Trait);
  for implementor in ["PgInvoiceStore", "InMemoryInvoiceStore"] {
    project.assert_edge(EdgeKind::Implements, billing(&["store"], &[implementor], EntityKind::Struct), store_trait, STRUCTURAL);
  }
  let router = billing(&[], &["router"], EntityKind::Function);
  for handler in ["create_invoice", "get_invoice"] {
    project.assert_edge(EdgeKind::References, router, billing(&["handlers"], &[handler], EntityKind::Function), RESOLVED);
  }
  let main = rust("acme-billing", "acme-billing/bin/acme-billing", &[], &["main"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, main, router, RESOLVED);
  project.assert_edge(
    EdgeKind::Calls,
    main,
    billing(&["acme_client"], &["AcmeApiClient", "from_env"], EntityKind::Method),
    RESOLVED,
  );
}

#[test]
fn should_resolve_cross_repo_sdk_imports_to_acme_sdk_sources() {
  let project = acme();
  min_confidence(
    project,
    EdgeKind::Calls,
    web(&["src", "components", "OrderForm"], &["OrderForm"], EntityKind::Function),
    sdk(&["src", "money"], &["toCents"], EntityKind::Function),
    0.8,
  );
  min_confidence(
    project,
    EdgeKind::Calls,
    web(&["src", "hooks", "useUser"], &["useUser"], EntityKind::Function),
    sdk(&["src", "client"], &["AcmeClient", "getUser"], EntityKind::Method),
    0.7,
  );
  min_confidence(
    project,
    EdgeKind::Calls,
    web(&["src", "hooks", "useCreateOrder"], &["useCreateOrder"], EntityKind::Function),
    sdk(&["src", "client"], &["AcmeClient", "createOrder"], EntityKind::Method),
    0.7,
  );
  let instantiation = project.assert_edge(
    EdgeKind::Instantiates,
    web(&["src", "api", "client"], &["acme"], EntityKind::Constant),
    sdk(&["src", "client"], &["AcmeClient"], EntityKind::Class),
    HEURISTIC,
  );
  assert_eq!(
    instantiation.certainty,
    atlas_core::Certainty::heuristic(atlas_core::Confidence::new(0.85).unwrap(), "dist-to-src")
  );
  project.assert_edge(EdgeKind::Imports, file("acme-web", "src/api/client.ts"), file("acme-sdk", "src/index.ts"), HEURISTIC);
  project.assert_no_edge(
    EdgeKind::Calls,
    sdk(&["src", "money"], &["toCents"], EntityKind::Function),
    web(&["src", "utils", "currency"], &["parseMoney"], EntityKind::Function),
  );
}

#[test]
fn should_record_sdk_barrel_re_exports() {
  let project = acme();
  let index = file("acme-sdk", "src/index.ts");
  project.assert_edge(EdgeKind::ReExports, index, sdk(&["src", "money"], &["toCents"], EntityKind::Function), STRUCTURAL);
  project.assert_edge(EdgeKind::ReExports, index, file("acme-sdk", "src/client.ts"), STRUCTURAL);
}

#[test]
fn should_emit_syntactic_package_dependencies_linked_to_in_project_packages() {
  let project = acme();
  let web_repo = common::repo("acme-web");
  let manifests = manifest_edges(&project.table, &web_repo, |path| Some(ContentHash::of(path.as_str().as_bytes())));
  let [manifest] = manifests.as_slice() else {
    panic!("expected one manifest, got {manifests:#?}")
  };
  assert_eq!(manifest.manifest.as_str(), "package.json");
  let sdk_package = EntityId::of(&EntityKey::Package {
    ecosystem: PackageEcosystem::Npm,
    name: "@acme/sdk".to_owned(),
  });
  let react = EntityId::of(&EntityKey::Package {
    ecosystem: PackageEcosystem::Npm,
    name: "react".to_owned(),
  });
  let to_sdk = manifest.edges.iter().find(|edge| edge.to == sdk_package).unwrap();
  assert_eq!(to_sdk.kind, EdgeKind::DependsOnPackage);
  assert_eq!(to_sdk.certainty, atlas_core::Certainty::Syntactic);
  assert_eq!(to_sdk.attrs["in_project"], serde_json::json!({ "repo": "acme-sdk", "manifest": "package.json" }));
  assert_eq!(to_sdk.attrs["sections"], serde_json::json!(["normal"]));
  let to_react = manifest.edges.iter().find(|edge| edge.to == react).unwrap();
  assert!(!to_react.attrs.contains_key("in_project"));
  assert_eq!(manifest.edges.len(), 7);
}

#[test]
fn should_classify_every_reference_exactly_once() {
  let project = acme();
  for ((repo, path), result) in &project.results {
    let facts = project.facts(repo, path);
    let counted: u32 = result.stats.by_kind.values().map(|counts| counts.total()).sum();
    assert_eq!(counted as usize, facts.references.len(), "{repo}:{path}");
    assert!(
      result
        .edges
        .iter()
        .chain(&result.structural)
        .all(|edge| !edge.evidence.is_empty() && edge.certainty != atlas_core::Certainty::CompilerVerified)
    );
  }
}

#[test]
fn should_link_billing_tests_to_the_in_memory_store_through_its_derived_default() {
  let project = acme();
  let store = billing(&["store"], &["InMemoryInvoiceStore"], EntityKind::Struct);
  let from_tests: Vec<_> = project
    .result("acme-billing", "tests/invoices.rs")
    .edges
    .iter()
    .filter(|edge| edge.kind == EdgeKind::References && edge.to == store)
    .collect();
  assert_eq!(from_tests.len(), 2);
  assert!(from_tests.iter().all(|edge| edge.certainty.class() == CertaintyClass::Resolved));
}
