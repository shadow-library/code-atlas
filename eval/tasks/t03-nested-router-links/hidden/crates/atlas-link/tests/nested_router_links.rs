mod common;

use atlas_core::{EdgeKind, EntityKind};
use common::{Indexed, index_tree, pass_facts, write_tree};

const STORE_API: &[(&str, &str)] = &[
  (
    "store-api/package.json",
    r#"{ "name": "store-api", "version": "1.0.0", "scripts": { "start": "node dist/server.js" }, "dependencies": { "express": "^4.0.0" } }"#,
  ),
  (
    "store-api/src/server.ts",
    r#"import express from 'express';

import { apiRouter } from './routes';
import { healthRouter } from './health';

const app = express();

app.use('/api', apiRouter);
app.use('/health', healthRouter);

app.listen(3000);
"#,
  ),
  (
    "store-api/src/health.ts",
    r#"import { Router } from 'express';

export const healthRouter = Router();

healthRouter.get('/live', (_req, res) => {
  res.send('ok');
});
"#,
  ),
  (
    "store-api/src/routes/index.ts",
    r#"import { Router } from 'express';

import { ordersRouter } from './orders';
import { refundsRouter } from './refunds';

export const apiRouter = Router();

apiRouter.use('/orders', ordersRouter);
apiRouter.use('/refunds', refundsRouter);
"#,
  ),
  (
    "store-api/src/routes/orders.ts",
    r#"import { Router } from 'express';

export const ordersRouter = Router();

ordersRouter.get('/:id', (req, res) => {
  res.json({ id: req.params.id });
});

ordersRouter.post('/', (req, res) => {
  res.status(201).json(req.body);
});
"#,
  ),
  (
    "store-api/src/routes/refunds.ts",
    r#"import { Router } from 'express';

export const refundsRouter = Router();

refundsRouter.delete('/:refundId', (req, res) => {
  res.status(204).end();
});
"#,
  ),
];

const STORE_WEB: &[(&str, &str)] = &[
  ("store-web/package.json", r#"{ "name": "store-web", "version": "1.0.0", "dependencies": {} }"#),
  (
    "store-web/src/api.ts",
    r#"export async function loadOrder(id: string) {
  return fetch(`${process.env.STORE_API_URL}/api/orders/${id}`);
}

export async function createOrder(body: unknown) {
  return fetch(`${process.env.STORE_API_URL}/api/orders`, { method: 'POST', body: JSON.stringify(body) });
}

export async function cancelRefund(id: string) {
  return fetch(`${process.env.STORE_API_URL}/api/refunds/${id}`, { method: 'DELETE' });
}

export async function checkLive() {
  return fetch(`${process.env.STORE_API_URL}/health/live`);
}
"#,
  ),
];

fn store(scratch: &std::path::Path) -> Indexed {
  let source = scratch.join("source");
  for files in [STORE_API, STORE_WEB] {
    write_tree(&source, files);
  }
  index_tree(scratch, "store", &source, &["store-api", "store-web"])
}

fn linked_endpoints(indexed: &Indexed, caller: &str) -> Vec<String> {
  let facts = pass_facts(&indexed.engine);
  let mut found: Vec<String> = indexed
    .engine
    .store()
    .read(|tx| -> Result<_, atlas_store::StoreError> {
      let mut found = Vec::new();
      for edge in facts.edges.iter().filter(|edge| edge.kind == EdgeKind::CallsEndpoint) {
        if tx.entity(edge.from)?.unwrap().name == caller {
          found.push(tx.entity(edge.to)?.unwrap().key.to_string());
        }
      }
      Ok(found)
    })
    .unwrap();
  found.sort();
  found
}

#[test]
fn should_compose_routes_of_routers_mounted_two_files_deep() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = store(scratch.path());
  let facts = pass_facts(&indexed.engine);
  let mut composed: Vec<String> = facts
    .entities
    .iter()
    .filter(|entity| entity.kind == EntityKind::ApiEndpoint && entity.attrs.contains_key("composed_from"))
    .map(|entity| entity.key.to_string())
    .collect();
  composed.sort();
  assert_eq!(
    composed,
    [
      "endpoint:store-api:DELETE /api/refunds/{refundId}",
      "endpoint:store-api:GET /api/orders/{id}",
      "endpoint:store-api:GET /health/live",
      "endpoint:store-api:POST /api/orders",
    ]
  );
}

#[test]
fn should_link_client_calls_to_routes_of_nested_routers() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = store(scratch.path());
  assert_eq!(linked_endpoints(&indexed, "loadOrder"), ["endpoint:store-api:GET /api/orders/{id}"]);
  assert_eq!(linked_endpoints(&indexed, "createOrder"), ["endpoint:store-api:POST /api/orders"]);
  assert_eq!(linked_endpoints(&indexed, "cancelRefund"), ["endpoint:store-api:DELETE /api/refunds/{refundId}"]);
}

#[test]
fn should_keep_linking_routers_mounted_directly_on_the_app() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = store(scratch.path());
  assert_eq!(linked_endpoints(&indexed, "checkLive"), ["endpoint:store-api:GET /health/live"]);
}
