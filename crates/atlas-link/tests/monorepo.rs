mod common;

use atlas_core::{EdgeKind, EntityKind};
use atlas_detect::DecoratorFamily;
use atlas_project::ProjectSettings;
use common::{Indexed, index_tree_with, pass_facts, write_tree};
use serde_json::{Value, json};

fn nest_app(dir: &str, package: &str, prefix: &str, param: &str) -> Vec<(String, String)> {
  vec![
    (
      format!("{dir}/package.json"),
      format!(r#"{{ "name": "{package}", "version": "1.0.0", "dependencies": {{ "@nestjs/common": "^10.0.0", "@nestjs/core": "^10.0.0" }} }}"#),
    ),
    (
      format!("{dir}/src/main.ts"),
      format!(
        "import {{ NestFactory }} from '@nestjs/core';\nimport {{ AppModule }} from './app.module';\n\nasync function bootstrap(): Promise<void> {{\n  const app = await NestFactory.create(AppModule);\n  app.setGlobalPrefix('{prefix}');\n  await app.listen(3000);\n}}\n\nbootstrap();\n"
      ),
    ),
    (format!("{dir}/src/app.module.ts"), "export class AppModule {}\n".to_owned()),
    (
      format!("{dir}/src/items.controller.ts"),
      format!(
        "import {{ Controller, Get, Param }} from '@nestjs/common';\n\n@Controller('items')\nexport class ItemsController {{\n  @Get(':{param}')\n  findOne(@Param('{param}') {param}: string) {{\n    return {{ {param} }};\n  }}\n}}\n"
      ),
    ),
    (
      format!("{dir}/src/health.controller.ts"),
      "import { Controller, Get } from '@nestjs/common';\n\n@Controller('health')\nexport class HealthController {\n  @Get()\n  check() {\n    return 'ok';\n  }\n}\n".to_owned(),
    ),
  ]
}

const WEB: &[(&str, &str)] = &[
  (
    "mono/apps/web/package.json",
    r#"{ "name": "@acme/web", "version": "1.0.0", "dependencies": { "openapi-fetch": "^0.13.0" } }"#,
  ),
  (
    "mono/apps/web/src/books.ts",
    r#"import createClient from 'openapi-fetch';
import type { paths } from '@acme/books-api/openapi';

const books = createClient<paths>({ baseUrl: '/' });

export const loadBookItem = (id: string) => books.GET('/api/items/{id}', { params: { path: { id } } });

export const checkBooks = () => books.GET('/api/health');
"#,
  ),
  (
    "mono/apps/web/src/orders.ts",
    r#"import createClient from 'openapi-fetch';
import type { paths } from '../../orders-api/src/openapi';

const orders = createClient<paths>();

export const loadOrderItem = (itemId: string) => orders.GET('/api/items/{itemId}', { params: { path: { itemId } } });
"#,
  ),
  (
    "mono/apps/web/src/api/client.ts",
    r#"import createClient from 'openapi-fetch';
import type { paths } from '../../../orders-api/src/openapi';

export const ordersApi = createClient<paths>({ baseUrl: '/api/' });
"#,
  ),
  (
    "mono/apps/web/src/order-page.ts",
    r#"import { ordersApi } from './api/client';

export const loadOrderPageItem = (itemId: string) => ordersApi.GET('/items/{itemId}', { params: { path: { itemId } } });

export const checkOrderPage = () => ordersApi.GET('/health');
"#,
  ),
  (
    "mono/apps/web/src/status.ts",
    r#"import createClient from 'openapi-fetch';
import type { paths } from '../../orders-api/src/openapi';

const status = createClient<paths>({ baseUrl: process.env.ORDERS_URL });

export const checkOrderStatus = () => status.GET('/health');
"#,
  ),
  (
    "solo/package.json",
    r#"{ "name": "solo", "version": "1.0.0", "dependencies": { "@nestjs/common": "^10.0.0" } }"#,
  ),
  (
    "solo/src/ping.controller.ts",
    "import { Controller, Get } from '@nestjs/common';\n\n@Controller('ping')\nexport class PingController {\n  @Get()\n  ping() {\n    return 'pong';\n  }\n}\n",
  ),
  (
    "mono/apps/web/src/untyped.ts",
    r#"export async function loadAnyItem(id: string) {
  return fetch(`/api/items/${id}`);
}
"#,
  ),
];

fn monorepo(scratch: &std::path::Path) -> Indexed {
  let source = scratch.join("source");
  write_tree(&source, &[("mono/package.json", r#"{ "name": "mono", "private": true, "workspaces": ["apps/*"] }"#)]);
  for (dir, package, prefix, param) in [
    ("mono/apps/books-api", "@acme/books-api", "api", "id"),
    ("mono/apps/orders-api", "@acme/orders-api", "api", "itemId"),
    ("mono/apps/admin-api", "@acme/admin-api", "admin", "key"),
  ] {
    let files = nest_app(dir, package, prefix, param);
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(path, content)| (path.as_str(), content.as_str())).collect();
    write_tree(&source, &borrowed);
  }
  write_tree(&source, WEB);
  let client = "[client]\nfiles = [\"apps/web/src/api/*.ts\"]\nexports = [\"ordersApi\"]\nmethods = [\"GET\"]\n";
  let mut settings = ProjectSettings::default();
  settings.detect.route_families = vec!["acme-web-client".to_owned()];
  settings.detect.families = vec![DecoratorFamily::parse("acme-web-client", client).unwrap().family];
  index_tree_with(scratch, "mono", &source, &["mono", "solo"], settings)
}

fn links(indexed: &Indexed, caller: &str) -> Vec<(String, Value)> {
  let facts = pass_facts(&indexed.engine);
  let mut found: Vec<(String, Value)> = indexed
    .engine
    .store()
    .read(|tx| -> Result<_, atlas_store::StoreError> {
      let mut found = Vec::new();
      for edge in facts.edges.iter().filter(|edge| edge.kind == EdgeKind::CallsEndpoint) {
        if tx.entity(edge.from)?.unwrap().name != caller {
          continue;
        }
        let attrs = json!({
          "share": edge.attrs["share"],
          "client_package": edge.attrs.get("client_package"),
          "endpoint_packages": edge.attrs.get("endpoint_packages"),
          "target_package": edge.attrs.get("target_package"),
          "package_rule": edge.attrs.get("package_rule"),
        });
        found.push((tx.entity(edge.to)?.unwrap().key.to_string(), attrs));
      }
      Ok(found)
    })
    .unwrap();
  found.sort_by(|left, right| left.0.cmp(&right.0));
  found
}

#[test]
fn should_compose_each_app_with_the_global_prefix_of_its_own_package() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = monorepo(scratch.path());
  let facts = pass_facts(&indexed.engine);
  let mut composed: Vec<(String, String, Value)> = facts
    .entities
    .iter()
    .filter(|entity| entity.kind == EntityKind::ApiEndpoint && entity.attrs.contains_key("composed_from"))
    .map(|entity| (entity.key.to_string(), entity.certainty.tag(), entity.attrs["packages"].clone()))
    .collect();
  composed.sort_by(|left, right| left.0.cmp(&right.0));
  assert_eq!(
    composed,
    [
      ("endpoint:mono:GET /admin/health".to_owned(), "[heuristic 0.50]".to_owned(), json!(["@acme/admin-api"])),
      ("endpoint:mono:GET /admin/items/{key}".to_owned(), "[heuristic 0.85]".to_owned(), json!(["@acme/admin-api"])),
      (
        "endpoint:mono:GET /api/health".to_owned(),
        "[heuristic 0.50]".to_owned(),
        json!(["@acme/books-api", "@acme/orders-api"])
      ),
      ("endpoint:mono:GET /api/items/{id}".to_owned(), "[heuristic 0.85]".to_owned(), json!(["@acme/books-api"])),
      (
        "endpoint:mono:GET /api/items/{itemId}".to_owned(),
        "[heuristic 0.85]".to_owned(),
        json!(["@acme/orders-api"])
      ),
    ]
  );
}

#[test]
fn should_prefer_the_endpoint_of_the_package_the_generated_types_come_from() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = monorepo(scratch.path());
  let books = json!({
    "share": 1.0,
    "client_package": "@acme/web",
    "endpoint_packages": ["@acme/books-api"],
    "target_package": "@acme/books-api",
    "package_rule": "types-package",
  });
  assert_eq!(links(&indexed, "loadBookItem"), [("endpoint:mono:GET /api/items/{id}".to_owned(), books)]);
  let orders = json!({
    "share": 1.0,
    "client_package": "@acme/web",
    "endpoint_packages": ["@acme/orders-api"],
    "target_package": "@acme/orders-api",
    "package_rule": "types-package",
  });
  assert_eq!(links(&indexed, "loadOrderItem"), [("endpoint:mono:GET /api/items/{itemId}".to_owned(), orders)]);
}

#[test]
fn should_attribute_a_route_two_apps_share_to_the_package_of_the_generated_types() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = monorepo(scratch.path());
  let health = json!({
    "share": 1.0,
    "client_package": "@acme/web",
    "endpoint_packages": ["@acme/books-api", "@acme/orders-api"],
    "target_package": "@acme/books-api",
    "package_rule": "types-package",
  });
  assert_eq!(links(&indexed, "checkBooks"), [("endpoint:mono:GET /api/health".to_owned(), health)]);
}

#[test]
fn should_keep_splitting_untyped_calls_across_every_matching_app() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = monorepo(scratch.path());
  let found = links(&indexed, "loadAnyItem");
  let shared = |package: &str| {
    json!({
      "share": 0.5,
      "client_package": "@acme/web",
      "endpoint_packages": [package],
      "target_package": null,
      "package_rule": null,
    })
  };
  assert_eq!(
    found,
    [
      ("endpoint:mono:GET /api/items/{id}".to_owned(), shared("@acme/books-api")),
      ("endpoint:mono:GET /api/items/{itemId}".to_owned(), shared("@acme/orders-api")),
    ]
  );
}

#[test]
fn should_record_the_packages_of_merged_endpoints_and_client_calls() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = monorepo(scratch.path());
  let facts = pass_facts(&indexed.engine);
  let packages = |key: &str| {
    facts
      .entities
      .iter()
      .find(|entity| entity.key.to_string() == key)
      .unwrap_or_else(|| panic!("no pass-owned {key}"))
      .attrs["packages"]
      .clone()
  };
  assert_eq!(packages("endpoint:mono:GET /health"), json!(["@acme/admin-api", "@acme/books-api", "@acme/orders-api"]));
  assert_eq!(packages("endpoint:mono:GET /items/{id}"), json!(["@acme/books-api"]));
  assert_eq!(packages("client:mono:GET /api/items/{}"), json!(["@acme/web"]));
}

#[test]
fn should_place_a_dynamic_base_call_only_on_the_prefix_composed_for_its_package() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = monorepo(scratch.path());
  let orders = json!({
    "share": 1.0,
    "client_package": "@acme/web",
    "endpoint_packages": ["@acme/books-api", "@acme/orders-api"],
    "target_package": "@acme/orders-api",
    "package_rule": "types-package",
  });
  assert_eq!(links(&indexed, "checkOrderStatus"), [("endpoint:mono:GET /api/health".to_owned(), orders)]);
}

#[test]
fn should_join_calls_on_an_imported_client_to_the_base_and_types_of_its_definition() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = monorepo(scratch.path());
  let orders = |packages: Value| {
    json!({
      "share": 1.0,
      "client_package": "@acme/web",
      "endpoint_packages": packages,
      "target_package": "@acme/orders-api",
      "package_rule": "types-package",
    })
  };
  assert_eq!(
    links(&indexed, "loadOrderPageItem"),
    [("endpoint:mono:GET /api/items/{itemId}".to_owned(), orders(json!(["@acme/orders-api"])))]
  );
  assert_eq!(
    links(&indexed, "checkOrderPage"),
    [("endpoint:mono:GET /api/health".to_owned(), orders(json!(["@acme/books-api", "@acme/orders-api"])))]
  );
  let facts = pass_facts(&indexed.engine);
  let defined_in: Vec<&Value> = facts
    .edges
    .iter()
    .filter(|edge| edge.kind == EdgeKind::CallsEndpoint)
    .filter_map(|edge| edge.attrs.get("client_defined_in"))
    .collect();
  assert_eq!(
    defined_in,
    [
      &json!("other:typed_client:mono:apps/web/src/api/client.ts#ordersApi"),
      &json!("other:typed_client:mono:apps/web/src/api/client.ts#ordersApi")
    ]
  );
}

#[test]
fn should_not_annotate_packages_in_a_repository_with_one_manifest() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = monorepo(scratch.path());
  let facts = pass_facts(&indexed.engine);
  let solo: Vec<String> = facts
    .entities
    .iter()
    .filter(|entity| entity.repo.as_ref().is_some_and(|repo| repo.as_str() == "solo") && entity.attrs.contains_key("packages"))
    .map(|entity| entity.key.to_string())
    .collect();
  assert_eq!(solo, Vec::<String>::new());
  let endpoint = indexed
    .engine
    .store()
    .read(|tx| tx.entity(atlas_core::EntityId::of(&"endpoint:solo:GET /ping".parse::<atlas_core::EntityKey>().unwrap())))
    .unwrap();
  assert!(endpoint.is_some_and(|endpoint| !endpoint.attrs.contains_key("packages")));
  assert!(
    facts
      .entities
      .iter()
      .any(|entity| entity.key.to_string() == "endpoint:mono:GET /items/{id}" && entity.attrs.contains_key("packages"))
  );
}
