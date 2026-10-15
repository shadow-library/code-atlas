mod common;

use std::fs;

use atlas_core::{EdgeKind, EntityId, EntityKey, Owner};
use atlas_query::{ImpactRequest, PatternsRequest, Targeted};
use atlas_store::EdgeFilter;
use common::Fixture;

const LINK_PASS: &str = "link";
const PACKAGE: &str = r#"{ "name": "shop", "version": "1.0.0" }"#;
const SCHEMA: &str = "import { pgTable, text } from 'drizzle-orm/pg-core';\n\
export const users = pgTable('users', { id: text('id') });\n";
const REPOSITORY: &str = "import { eq } from 'drizzle-orm';\n\
import { db } from './client';\n\
import { users } from './schema';\n\
\n\
export async function listUsers() {\n\
  return db.select().from(users).where(eq(users.id, '1'));\n\
}\n\
\n\
export async function addUser() {\n\
  await db.insert(users).values({ id: '2' });\n\
}\n";

const TSCONFIG: &str = r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@shop/*": ["src/*"] } } }"#;
const USERS_TABLE: &str = "import { pgTable, text } from 'drizzle-orm/pg-core';\n\
export const users = pgTable('users', { id: text('id') });\n";
const ORDERS_TABLE: &str = "import { pgTable, text } from 'drizzle-orm/pg-core';\n\
export const orders = pgTable('orders', { id: text('id') });\n";
const TABLES_BARREL: &str = "export * from './users';\nexport * from './orders';\n";
const DATASTORE_BARREL: &str = "export * as schema from './tables';\n";
const LEGACY_BARREL: &str = "import * as tables from './tables';\nexport { tables as legacy };\n";
const NAMESPACE_REPOSITORY: &str = "import { eq } from 'drizzle-orm';\n\
import { schema } from '@shop/datastore';\n\
import * as datastore from './datastore';\n\
import { legacy } from './datastore/legacy';\n\
import { users } from '@shop/datastore/tables';\n\
import { db } from './client';\n\
\n\
export async function listUsers() {\n\
  return db.select().from(schema.users).where(eq(schema.users.id, '1'));\n\
}\n\
\n\
export async function placeOrder() {\n\
  await db.transaction(async (tx) => {\n\
    await tx.insert(schema.orders).values({ id: '1' });\n\
  });\n\
}\n\
\n\
export async function purgeOrders() {\n\
  await db.delete(datastore.schema.orders);\n\
}\n\
\n\
export async function listLegacyUsers() {\n\
  return db.select().from(legacy.users);\n\
}\n\
\n\
export async function touchColumn() {\n\
  await db.update(users.id).set({});\n\
}\n";

fn single_file_fixture() -> Fixture {
  common::single_repo("shop", &[("package.json", PACKAGE), ("src/schema.ts", SCHEMA), ("src/repository.ts", REPOSITORY)])
}

fn namespace_fixture() -> Fixture {
  common::single_repo(
    "shop",
    &[
      ("package.json", PACKAGE),
      ("tsconfig.json", TSCONFIG),
      ("src/client.ts", "export const db = {};\n"),
      ("src/datastore/tables/users.ts", USERS_TABLE),
      ("src/datastore/tables/orders.ts", ORDERS_TABLE),
      ("src/datastore/tables/index.ts", TABLES_BARREL),
      ("src/datastore/index.ts", DATASTORE_BARREL),
      ("src/datastore/legacy.ts", LEGACY_BARREL),
      ("src/repository.ts", NAMESPACE_REPOSITORY),
    ],
  )
}

fn edges_into(fixture: &Fixture, key: &str, kinds: &[EdgeKind]) -> Vec<(String, String, String)> {
  let engine = fixture.query.engine();
  engine.ensure_fresh().unwrap();
  let target = EntityId::of(&key.parse::<EntityKey>().unwrap());
  let edges = engine.store().read(|tx| tx.edges_to(target, &EdgeFilter::default())).unwrap();
  let mut rows: Vec<(String, String, String)> = edges
    .iter()
    .filter(|edge| kinds.contains(&edge.kind))
    .map(|edge| {
      let from = engine.store().read(|tx| tx.entity(edge.from)).unwrap().unwrap();
      (edge.kind.to_string(), from.key.to_string(), edge.certainty.tag())
    })
    .collect();
  rows.sort();
  rows
}

fn row(kind: &str, from: &str, tag: &str) -> (String, String, String) {
  (kind.to_owned(), from.to_owned(), tag.to_owned())
}

fn link_owned_accesses(fixture: &Fixture) -> Vec<String> {
  let engine = fixture.query.engine();
  engine.ensure_fresh().unwrap();
  let owner = Owner::Project { pass: LINK_PASS.to_owned() };
  let facts = engine.store().read(|tx| tx.owned_facts(&owner)).unwrap();
  let mut rows: Vec<String> = facts
    .edges
    .iter()
    .filter(|edge| matches!(edge.kind, EdgeKind::ReadsTable | EdgeKind::WritesTable))
    .map(|edge| {
      let from = engine.store().read(|tx| tx.entity(edge.from)).unwrap().unwrap();
      let to = engine.store().read(|tx| tx.entity(edge.to)).unwrap().unwrap();
      format!("{} {} -> {} {} via {}", edge.kind, from.name, to.key, edge.certainty.tag(), edge.attrs["via"])
    })
    .collect();
  rows.sort();
  rows
}

#[test]
fn should_link_the_functions_that_use_a_drizzle_table_declared_in_another_file_to_its_constant() {
  assert_eq!(
    edges_into(
      &single_file_fixture(),
      "ts:shop:shop:src/schema::users#constant",
      &[EdgeKind::ReadsTable, EdgeKind::WritesTable]
    ),
    [
      row("reads_table", "ts:shop:shop:src/repository::listUsers#function", "[heuristic 0.60]"),
      row("writes_table", "ts:shop:shop:src/repository::addUser#function", "[heuristic 0.60]"),
    ]
  );
}

#[test]
fn should_map_the_drizzle_constant_to_its_table() {
  assert_eq!(
    edges_into(&single_file_fixture(), "table:users", &[EdgeKind::MapsToTable]),
    [row("maps_to_table", "ts:shop:shop:src/schema::users#constant", "[syntactic]")]
  );
}

#[test]
fn should_link_drizzle_readers_and_writers_to_the_table_itself() {
  let fixture = single_file_fixture();
  assert_eq!(
    edges_into(&fixture, "table:users", &[EdgeKind::ReadsTable, EdgeKind::WritesTable]),
    [
      row("reads_table", "ts:shop:shop:src/repository::listUsers#function", "[heuristic 0.60]"),
      row("writes_table", "ts:shop:shop:src/repository::addUser#function", "[heuristic 0.60]"),
    ]
  );
  assert_eq!(
    link_owned_accesses(&fixture),
    [
      "reads_table listUsers -> table:users [heuristic 0.60] via [\"ts:shop:shop:src/schema::users#constant\"]",
      "writes_table addUser -> table:users [heuristic 0.60] via [\"ts:shop:shop:src/schema::users#constant\"]",
    ]
  );
}

#[test]
fn should_resolve_tables_behind_named_imports_of_re_exported_namespaces() {
  let fixture = namespace_fixture();
  assert_eq!(
    edges_into(
      &fixture,
      "ts:shop:shop:src/datastore/tables/users::users#constant",
      &[EdgeKind::ReadsTable, EdgeKind::WritesTable]
    ),
    [
      row("reads_table", "ts:shop:shop:src/repository::listLegacyUsers#function", "[heuristic 0.60]"),
      row("reads_table", "ts:shop:shop:src/repository::listUsers#function", "[heuristic 0.60]"),
    ]
  );
  assert_eq!(
    edges_into(&fixture, "table:orders", &[EdgeKind::ReadsTable, EdgeKind::WritesTable]),
    [
      row("writes_table", "ts:shop:shop:src/repository::placeOrder#function", "[heuristic 0.60]"),
      row("writes_table", "ts:shop:shop:src/repository::purgeOrders#function", "[heuristic 0.60]"),
    ]
  );
  assert_eq!(
    edges_into(&fixture, "table:users", &[EdgeKind::ReadsTable, EdgeKind::WritesTable]),
    [
      row("reads_table", "ts:shop:shop:src/repository::listLegacyUsers#function", "[heuristic 0.60]"),
      row("reads_table", "ts:shop:shop:src/repository::listUsers#function", "[heuristic 0.60]"),
    ]
  );
}

#[test]
fn should_show_drizzle_accesses_in_table_patterns_and_impact() {
  let fixture = namespace_fixture();
  let patterns = fixture.query.patterns(&PatternsRequest::new("db")).unwrap().answer;
  let shapes: Vec<(String, usize, Vec<String>)> = patterns
    .shapes
    .iter()
    .map(|shape| {
      let names = shape.exemplars.iter().map(|exemplar| exemplar.entity.short.clone()).collect();
      (shape.shape.clone(), shape.count, names)
    })
    .collect();
  assert_eq!(
    shapes,
    [
      ("maps to table via drizzle".to_owned(), 2, vec!["orders".to_owned(), "users".to_owned()]),
      ("reads table via drizzle".to_owned(), 2, vec!["listUsers".to_owned(), "listLegacyUsers".to_owned()]),
      ("writes table via drizzle".to_owned(), 2, vec!["placeOrder".to_owned(), "purgeOrders".to_owned()]),
    ]
  );
  let Targeted::Found(impact) = fixture.query.impact(&ImpactRequest::new(["table:orders"])).unwrap().answer else {
    panic!("table:orders must resolve");
  };
  let mut direct: Vec<&str> = impact.direct.items.iter().map(|item| item.entity.short.as_str()).collect();
  direct.sort();
  assert_eq!(direct, ["orders", "placeOrder", "purgeOrders"]);
}

#[test]
fn should_drop_the_table_links_when_the_barrel_stops_re_exporting_the_namespace() {
  let fixture = namespace_fixture();
  assert_eq!(edges_into(&fixture, "table:orders", &[EdgeKind::WritesTable]).len(), 2);
  fs::write(fixture.repo_root("shop").join("src/datastore/index.ts"), "export const schema = {};\n").unwrap();
  assert_eq!(edges_into(&fixture, "table:orders", &[EdgeKind::WritesTable]), []);
}
