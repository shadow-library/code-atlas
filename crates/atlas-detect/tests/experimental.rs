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
