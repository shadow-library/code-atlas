mod common;

use atlas_core::{EdgeKind, EntityId, EntityKey};
use atlas_store::EdgeFilter;

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

fn edges_into(key: &str, kinds: &[EdgeKind]) -> Vec<(String, String, String)> {
  let fixture = common::single_repo("shop", &[("package.json", PACKAGE), ("src/schema.ts", SCHEMA), ("src/repository.ts", REPOSITORY)]);
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

#[test]
fn should_link_the_functions_that_use_a_drizzle_table_declared_in_another_file_to_its_constant() {
  assert_eq!(
    edges_into("ts:shop:shop:src/schema::users#constant", &[EdgeKind::ReadsTable, EdgeKind::WritesTable]),
    [
      (
        "reads_table".to_owned(),
        "ts:shop:shop:src/repository::listUsers#function".to_owned(),
        "[heuristic 0.60]".to_owned()
      ),
      (
        "writes_table".to_owned(),
        "ts:shop:shop:src/repository::addUser#function".to_owned(),
        "[heuristic 0.60]".to_owned()
      ),
    ]
  );
}

#[test]
fn should_map_the_drizzle_constant_to_its_table() {
  assert_eq!(
    edges_into("table:users", &[EdgeKind::MapsToTable]),
    [("maps_to_table".to_owned(), "ts:shop:shop:src/schema::users#constant".to_owned(), "[syntactic]".to_owned())]
  );
}
