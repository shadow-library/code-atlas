mod common;

use atlas_detect::{DbDetector, EventDetector, JobDetector};
use common::{lines, render_file, render_repo, render_source};

#[test]
fn should_link_order_created_publishers_and_subscribers_in_both_projects() {
  assert_eq!(
    render_file(&EventDetector, "acme/acme-api", "src/orders/orders.service.ts"),
    lines(&[
      "event:order.created [heuristic 0.80]",
      "publishes OrdersService.create -> event:order.created [heuristic 0.80]"
    ])
  );
  assert_eq!(
    render_repo(&EventDetector, "acme/acme-billing"),
    lines(&[
      "event:order.created [heuristic 0.80]",
      "subscribes consume_order_created -> event:order.created [heuristic 0.80]"
    ])
  );
  assert_eq!(
    render_repo(&EventDetector, "globex/globex-api"),
    lines(&[
      "event:order.created [heuristic 0.70]",
      "publishes OrdersService.create -> event:order.created [heuristic 0.70]"
    ])
  );
  assert_eq!(
    render_repo(&EventDetector, "globex/globex-worker"),
    lines(&[
      "event:order.created [heuristic 0.80]",
      "subscribes handle_order_created -> event:order.created [heuristic 0.80]"
    ])
  );
}

#[test]
fn should_detect_nest_event_handlers_and_kafka_topics() {
  let source = "import { OnEvent } from '@nestjs/event-emitter';\n\
export class Listener {\n  @OnEvent('user.created')\n  handle() {}\n}\n\
export async function publish(producer: Producer) { await producer.send({ topic: 'audit', messages: [] }); }\n\
export async function consume(consumer: Consumer) { await consumer.subscribe({ topics: ['a', 'b'] }); }\n";
  assert_eq!(
    render_source(&EventDetector, "svc", "src/events.ts", source),
    lines(&[
      "event:a [heuristic 0.80]",
      "event:audit [heuristic 0.80]",
      "event:b [heuristic 0.80]",
      "event:user.created [heuristic 0.85]",
      "publishes publish -> event:audit [heuristic 0.80]",
      "subscribes Listener.handle -> event:user.created [heuristic 0.85]",
      "subscribes consume -> event:a [heuristic 0.80]",
      "subscribes consume -> event:b [heuristic 0.80]",
    ])
  );
}

#[test]
fn should_detect_scheduled_jobs() {
  let source = "import { Cron, Interval } from '@nestjs/schedule';\n\
import cron from 'node-cron';\n\
export class Reports {\n  @Cron('0 3 * * *')\n  nightly() {}\n  @Interval(5000)\n  poll() {}\n}\n\
cron.schedule('*/5 * * * *', sweep);\n\
setInterval(() => flush(), 1000);\n\
function sweep() {}\n";
  assert_eq!(
    render_source(&JobDetector, "svc", "src/jobs.ts", source),
    lines(&[
      "defines Reports.nightly -> job:svc:Reports.nightly [heuristic 0.85]",
      "defines Reports.poll -> job:svc:Reports.poll [heuristic 0.85]",
      "defines file:svc:src/jobs.ts -> job:svc:interval 1000 [heuristic 0.70]",
      "defines file:svc:src/jobs.ts -> job:svc:sweep [heuristic 0.70]",
      "job:svc:Reports.nightly [heuristic 0.85]",
      "job:svc:Reports.poll [heuristic 0.85]",
      "job:svc:interval 1000 [heuristic 0.70]",
      "job:svc:sweep [heuristic 0.70]",
    ])
  );
  let rust = "use tokio_cron_scheduler::Job;\npub fn jobs() { let job = Job::new_async(\"0 * * * * *\", |_, _| Box::pin(async {})); }\n";
  assert_eq!(
    render_source(&JobDetector, "svc", "src/jobs.rs", rust),
    lines(&["defines jobs -> job:svc:cron 0 * * * * * [heuristic 0.70]", "job:svc:cron 0 * * * * * [heuristic 0.70]"])
  );
}

#[test]
fn should_map_orm_models_to_tables() {
  assert_eq!(
    render_repo(&DbDetector, "acme/acme-api"),
    lines(&[
      "maps_to_table Order -> table:orders [syntactic]",
      "maps_to_table User -> table:users [syntactic]",
      "table:orders [syntactic]",
      "table:users [syntactic]",
    ])
  );
  let source = "import { Entity } from 'typeorm';\nimport { Table, Model } from 'sequelize-typescript';\n\
@Entity() export class OrderItem {}\n@Table({ tableName: 'carts' }) export class Cart extends Model {}\n@Table export class Bare {}\n";
  assert_eq!(
    render_source(&DbDetector, "svc", "src/models.ts", source),
    lines(&[
      "maps_to_table Cart -> table:carts [syntactic]",
      "maps_to_table OrderItem -> table:order_item [heuristic 0.60]",
      "table:carts [syntactic]",
      "table:order_item [heuristic 0.60]",
    ])
  );
}

#[test]
fn should_extract_tables_from_sql_literals_prisma_calls_and_sqlx_macros() {
  let sql = "export class UsersService {\n  async findOne(id: string) {\n    const { rows } = await this.pool.query('SELECT u.id FROM users u JOIN teams t ON t.id = u.team_id WHERE u.id = $1', [id]);\n    logger.info('select from users failed');\n    return rows[0];\n  }\n}\n";
  assert_eq!(
    render_source(&DbDetector, "svc", "src/users.service.ts", sql),
    lines(&[
      "reads_table UsersService.findOne -> table:teams [heuristic 0.60]",
      "reads_table UsersService.findOne -> table:users [heuristic 0.60]",
      "table:teams [heuristic 0.60]",
      "table:users [heuristic 0.60]",
    ])
  );
  let ts = "export async function sync(prisma: PrismaClient) {\n  await prisma.user.findMany();\n  await prisma.order.upsert({});\n  await prisma.$transaction([]);\n}\n";
  assert_eq!(
    render_source(&DbDetector, "svc", "src/sync.ts", ts),
    lines(&[
      "reads_table sync -> table:user [heuristic 0.60]",
      "table:order [heuristic 0.60]",
      "table:user [heuristic 0.60]",
      "writes_table sync -> table:order [heuristic 0.60]",
    ])
  );
  let rust = "pub async fn save(pool: &PgPool) {\n  sqlx::query!(\"INSERT INTO invoices (id) VALUES ($1)\", 1).execute(pool).await;\n  sqlx::query(\"UPDATE invoices SET paid = true\").execute(pool).await;\n}\n";
  assert_eq!(
    render_source(&DbDetector, "svc", "src/store.rs", rust),
    lines(&["table:invoices [heuristic 0.60]", "writes_table save -> table:invoices [heuristic 0.60]"])
  );
}

const DRIZZLE_SCHEMA: &str = "import { pgTable, pgSchema, text } from 'drizzle-orm/pg-core';\n\
import * as sqlite from 'drizzle-orm/sqlite-core';\n\
import { mysqlTable as mysql } from 'drizzle-orm/mysql-core';\n\
export const users = pgTable('users', { id: text('id') });\n\
const billing = pgSchema('billing');\n\
export const invoices = billing.table('invoices', { id: text('id') });\n\
export const credits = pgSchema('promo').table('credits', {});\n\
export const notes = sqlite.sqliteTable('notes', {});\n\
export const sessions = mysql(`sessions`, {});\n";

#[test]
fn should_map_drizzle_table_constants_to_tables_whatever_the_dialect_import_style() {
  assert_eq!(
    render_source(&DbDetector, "svc", "src/schema.ts", DRIZZLE_SCHEMA),
    lines(&[
      "maps_to_table credits -> table:promo.credits [syntactic]",
      "maps_to_table invoices -> table:billing.invoices [syntactic]",
      "maps_to_table notes -> table:notes [syntactic]",
      "maps_to_table sessions -> table:sessions [syntactic]",
      "maps_to_table users -> table:users [syntactic]",
      "table:billing.invoices [syntactic]",
      "table:notes [syntactic]",
      "table:promo.credits [syntactic]",
      "table:sessions [syntactic]",
      "table:users [syntactic]",
    ])
  );
}

#[test]
fn should_report_drizzle_tables_without_a_literal_name_or_a_constant_and_skip_lookalikes() {
  let dynamic = "import { pgTable } from 'drizzle-orm/pg-core';\nconst name = 'users';\nexport const users = pgTable(name, {});\nexport const tagged = pgTable(`t_${name}`, {});\n";
  let analyzed = common::analyze("svc", "src/schema.ts", dynamic);
  let (detection, diagnostics) = common::run(&DbDetector, &analyzed);
  assert_eq!(detection.edges, []);
  assert_eq!(
    diagnostics.iter().map(|diagnostic| diagnostic.code.as_str()).collect::<Vec<_>>(),
    ["db.drizzle_dynamic_table_name", "db.drizzle_dynamic_table_name"]
  );
  let wrapped = "import { pgTable } from 'drizzle-orm/pg-core';\nexport const schema = { users: pgTable('users', {}) };\n";
  let analyzed = common::analyze("svc", "src/schema.ts", wrapped);
  let (detection, diagnostics) = common::run(&DbDetector, &analyzed);
  assert_eq!(detection.edges, []);
  assert_eq!(
    diagnostics.iter().map(|diagnostic| diagnostic.code.as_str()).collect::<Vec<_>>(),
    ["db.drizzle_unbound_table"]
  );
  let foreign = "import { pgTable } from './my-orm';\nimport { pgSchema } from 'other-orm';\nexport const users = pgTable('users', {});\nconst s = pgSchema('x');\nexport const t = s.table('t', {});\n";
  assert_eq!(render_source(&DbDetector, "svc", "src/schema.ts", foreign), Vec::<String>::new());
}

#[test]
fn should_read_and_write_drizzle_tables_through_the_query_builder() {
  let source = "import { eq } from 'drizzle-orm';\nimport { db } from '../db';\nimport { orders, users } from './schema';\n\
export async function run(tx: Tx) {\n\
  await db.select().from(users).where(eq(users.id, 1));\n\
  await db.select({ id: users.id }).from(users).leftJoin(orders, eq(orders.userId, users.id));\n\
  await db.insert(users).values({});\n\
  await tx.update(orders).set({});\n\
  await db.delete(orders);\n\
  await db.query.users.findMany();\n\
  await db.query.orders.findFirst({});\n\
}\n";
  assert_eq!(
    render_source(&DbDetector, "svc", "src/repo.ts", source),
    lines(&[
      "reads_table run -> ?orders [heuristic 0.60]",
      "reads_table run -> ?users [heuristic 0.60]",
      "writes_table run -> ?orders [heuristic 0.60]",
      "writes_table run -> ?users [heuristic 0.60]",
    ])
  );
}

#[test]
fn should_resolve_drizzle_namespace_tables_and_same_file_tables() {
  let source = "import { pgTable } from 'drizzle-orm/pg-core';\nimport * as schema from './schema';\n\
const local = pgTable('local', {});\n\
export async function run(db: Db) {\n\
  await db.insert(schema.users).values({});\n\
  await db.select().from(local);\n\
}\n";
  assert_eq!(
    render_source(&DbDetector, "svc", "src/repo.ts", source),
    lines(&[
      "maps_to_table local -> table:local [syntactic]",
      "reads_table run -> ?local [heuristic 0.60]",
      "table:local [syntactic]",
      "writes_table run -> ?schema::users [heuristic 0.60]",
    ])
  );
}

#[test]
fn should_leave_tables_behind_named_and_nested_imports_to_the_resolver() {
  let source = "import { eq } from 'drizzle-orm';\nimport { schema } from '@shop/datastore';\nimport * as datastore from '../datastore';\n\
import { db } from '../db';\n\
export async function run() {\n\
  await db.select().from(schema.users).where(eq(schema.users.id, 1));\n\
  await db.transaction(async (tx) => {\n\
    await tx.insert(schema.orders).values({});\n\
    await tx.select().from(datastore.schema.orders).innerJoin(datastore.tables.users, eq(datastore.tables.users.id, 1));\n\
  });\n\
  await db.delete(datastore.schema.orders.archive.rows);\n\
}\n";
  assert_eq!(
    render_source(&DbDetector, "svc", "src/repo.ts", source),
    lines(&[
      "reads_table run -> ?datastore::schema::orders [heuristic 0.60]",
      "reads_table run -> ?datastore::tables::users [heuristic 0.60]",
      "reads_table run -> ?schema::users [heuristic 0.60]",
      "writes_table run -> ?schema::orders [heuristic 0.60]",
    ])
  );
}

#[test]
fn should_skip_columns_of_a_named_import_used_as_a_table_in_the_same_file() {
  let source = "import { eq } from 'drizzle-orm';\nimport { db } from '../db';\nimport { users } from './schema';\n\
export async function run() {\n\
  await db.select().from(users);\n\
  await db.insert(users.id).values({});\n\
}\n";
  assert_eq!(
    render_source(&DbDetector, "svc", "src/repo.ts", source),
    lines(&["reads_table run -> ?users [heuristic 0.60]"])
  );
}

#[test]
fn should_not_read_drizzle_access_into_other_receivers_unknown_tables_or_files_without_drizzle() {
  let lookalikes = "import { eq } from 'drizzle-orm';\nimport { orders, users } from './schema';\n\
export function run(cache: Map<string, number>, items: Set<string>, rows: Rows) {\n\
  cache.delete(users);\n\
  items.delete(users);\n\
  rows.insert(users);\n\
  db.insert(unknown);\n\
  db.insert(users.id);\n\
  db.insert(makeTable());\n\
  db.query.orders.findMany();\n\
  db.query.users.count();\n\
  report.select().from(users);\n\
  db.select().from(users);\n\
}\n";
  assert_eq!(
    render_source(&DbDetector, "svc", "src/repo.ts", lookalikes),
    lines(&["reads_table run -> ?orders [heuristic 0.60]", "reads_table run -> ?users [heuristic 0.60]"])
  );
  let other =
    "import { db } from '../db';\nimport { users } from './schema';\nexport async function run() {\n  await db.insert(users).values({});\n  await db.select().from(users);\n}\n";
  assert_eq!(render_source(&DbDetector, "svc", "src/repo.ts", other), Vec::<String>::new());
}
