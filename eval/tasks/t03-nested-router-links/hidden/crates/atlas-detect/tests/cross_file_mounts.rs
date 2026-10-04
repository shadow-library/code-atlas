mod common;

use atlas_detect::ExpressEndpointDetector;
use common::{analyze, run};
use serde_json::{Value, json};

fn mounts(repo: &str, path: &str, source: &str) -> Vec<(Value, Value, Value)> {
  let analyzed = analyze(repo, path, source);
  run(&ExpressEndpointDetector, &analyzed)
    .0
    .edges
    .into_iter()
    .filter(|edge| edge.kind.to_string() == "mounts")
    .map(|edge| (edge.attrs["mount_prefix"].clone(), edge.attrs["mounted_on"].clone(), edge.attrs["mounted_on_scope"].clone()))
    .collect()
}

#[test]
fn should_name_the_parent_router_on_a_cross_file_mount_of_a_nested_router() {
  let source = "import { Router } from 'express';\nimport { ordersRouter } from './orders';\nexport const apiRouter = Router();\napiRouter.use('/orders', ordersRouter);\n";
  assert_eq!(
    mounts("shop-api", "src/routes/index.ts", source),
    [(json!("/orders"), json!("apiRouter"), json!("router_relative"))]
  );
}

#[test]
fn should_name_the_app_on_a_cross_file_mount_on_the_application() {
  let source = "import express from 'express';\nimport { apiRouter } from './routes';\nconst app = express();\napp.use('/api', apiRouter);\n";
  assert_eq!(mounts("shop-api", "src/server.ts", source), [(json!("/api"), json!("app"), json!("absolute"))]);
}
