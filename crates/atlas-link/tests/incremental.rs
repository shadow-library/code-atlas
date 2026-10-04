mod common;

use std::fs;
use std::path::Path;

use common::{acme, index_tree, pass_facts};

fn edit(root: &Path, path: &str, from: &str, to: &str) {
  let file = root.join(path);
  let text = fs::read_to_string(&file).unwrap();
  assert!(text.contains(from), "{path} lacks {from}");
  fs::write(&file, text.replacen(from, to, 1)).unwrap();
}

#[test]
fn should_match_a_full_index_after_incremental_edits() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = acme(scratch.path());
  let tree = scratch.path().join("tree").join("acme");
  edit(
    &tree,
    "acme-api/src/main.ts",
    "  app.use(cookieParser());",
    "  app.setGlobalPrefix('api');\n  app.use(cookieParser());",
  );
  edit(
    &tree,
    "acme-api/src/orders/orders.controller.ts",
    "@UseGuards(JwtOrApiKeyGuard)",
    "@UseGuards(JwtAuthGuard)",
  );
  edit(&tree, "acme-web/src/hooks/useMe.ts", "/users/me", "/orders/me");
  let report = indexed.engine.ensure_fresh().unwrap();
  assert!(report.is_fresh(), "{report:?}");
  let incremental = pass_facts(&indexed.engine);

  let fresh_scratch = tempfile::tempdir().unwrap();
  let full = index_tree(fresh_scratch.path(), "acme", &tree, &["acme-api", "acme-sdk", "acme-web", "acme-billing"]);
  let expected = pass_facts(&full.engine);

  assert_eq!(
    serde_json::to_string_pretty(&incremental.entities).unwrap(),
    serde_json::to_string_pretty(&expected.entities).unwrap()
  );
  assert_eq!(
    serde_json::to_string_pretty(&incremental.edges).unwrap(),
    serde_json::to_string_pretty(&expected.edges).unwrap()
  );
  let composed = incremental
    .entities
    .iter()
    .filter(|entity| entity.key.to_string().starts_with("endpoint:acme-api:") && entity.key.to_string().contains(" /api/"))
    .count();
  assert_eq!(composed, 8);
  let get_order = incremental
    .edges
    .iter()
    .filter(|edge| edge.kind == atlas_core::EdgeKind::CallsEndpoint && edge.attrs["route"] == serde_json::json!("/api/orders/{id}"))
    .count();
  assert_eq!(get_order, 3);
}
