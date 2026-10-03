mod common;

use common::registry_render;

#[test]
fn should_not_turn_map_and_cache_lookups_into_routes_or_clients() {
  let source = "import express from 'express';\n\
const map = new Map<string, string>();\n\
export function lookup(cache: Cache, key: string) {\n  map.get('x');\n  cache.get(key);\n  cache.post('/items', {});\n  return map.delete('/users/:id');\n}\n";
  assert!(registry_render("app", "src/lookup.ts", source).is_empty());
}

#[test]
fn should_not_treat_a_router_variable_that_is_not_an_express_router_as_one() {
  let source = "import { useRouter } from 'next/router';\nimport express from 'express';\n\
export function Page() {\n  const router = useRouter();\n  router.get('/users/:id');\n  return null;\n}\n\
export function wire(router: AppRouter) { router.post('/items', handler); }\n";
  assert!(registry_render("web", "src/page.tsx", source).is_empty());
}

#[test]
fn should_not_treat_emit_on_a_non_bus_object_as_an_event() {
  let source = "export function stream(socket: Socket, res: Response, stream: Readable, busStop: Stop, queueItems: Item[]) {\n  socket.emit('message', 1);\n  stream.on('data', () => {});\n  res.send('ok');\n  busStop.emit('arrived');\n  queueItems.send('x');\n}\n";
  assert!(registry_render("svc", "src/stream.ts", source).is_empty());
}

#[test]
fn should_not_extract_tables_from_sql_looking_log_messages() {
  let ts = "export function report(logger: Logger) {\n  logger.info('SELECT * FROM users failed');\n  console.log(`DELETE FROM sessions took ${ms}ms`);\n}\n";
  assert!(registry_render("svc", "src/report.ts", ts).is_empty());
  let rust = "pub fn report() {\n  tracing::info!(\"SELECT * FROM users failed\");\n  println!(\"INSERT INTO audit\");\n}\n";
  assert!(registry_render("svc", "src/report.rs", rust).is_empty());
}

#[test]
fn should_not_treat_a_locally_shadowed_fetch_or_unrelated_axios_as_http_clients() {
  let source = "import { fetch } from './cache';\nimport axios from './not-axios';\n\
export function load() { fetch('/users'); axios.get('/orders'); }\n\
export function local() { const fetch = (u: string) => u; return fetch('/x'); }\n";
  assert!(registry_render("web", "src/load.ts", source).is_empty());
}

#[test]
fn should_not_treat_non_reqwest_get_calls_as_http_clients() {
  let source = "use std::collections::HashMap;\npub fn lookup(map: &HashMap<String, String>, cache: Cache) {\n  map.get(\"/users\");\n  cache.get(\"https://x\");\n  let client = Client::new();\n  client.get(\"https://y\");\n}\n";
  assert!(registry_render("svc", "src/lookup.rs", source).is_empty());
}
