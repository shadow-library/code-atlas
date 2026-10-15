mod common;

use atlas_core::{EdgeKind, EntityKind};
use atlas_detect::DecoratorFamily;
use atlas_project::ProjectSettings;
use common::{index_tree_with, pass_facts, write_tree};
use serde_json::json;

const FAMILY: &str = "packages = [\"@acme/gate\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n[guards]\ndecorators = [\"Authenticated\"]\nauth_schemes = { Authenticated = \"bearer\" }\n[access]\npublic = [\"Public\"]\n[global_prefix]\nfactory = \"AcmeServer.create\"\nmethod = \"setGlobalPrefix\"\n";

fn gate_app(dir: &str, package: &str, prefix: &str, health: &str) -> Vec<(String, String)> {
  vec![
    (
      format!("{dir}/package.json"),
      format!(r#"{{ "name": "{package}", "version": "1.0.0", "dependencies": {{ "@acme/gate": "^1.0.0" }} }}"#),
    ),
    (
      format!("{dir}/src/main.ts"),
      format!(
        "import {{ AcmeServer }} from '@acme/gate';\n\nclass AppModule {{}}\n\nasync function boot(): Promise<void> {{\n  const app = await AcmeServer.create(AppModule);\n  app.setGlobalPrefix('{prefix}');\n}}\n\nboot();\n"
      ),
    ),
    (format!("{dir}/src/health.controller.ts"), health.to_owned()),
  ]
}

#[test]
fn should_derive_the_requirement_of_a_route_shared_by_two_apps_per_exposing_handler_and_per_composed_package() {
  let scratch = tempfile::tempdir().unwrap();
  let source = scratch.path().join("source");
  let mut files = vec![(
    "mono/package.json".to_owned(),
    r#"{ "name": "mono", "private": true, "workspaces": ["apps/*"] }"#.to_owned(),
  )];
  files.extend(gate_app(
    "mono/apps/admin",
    "@acme/admin",
    "admin",
    "import { Api, Authenticated, Read } from '@acme/gate';\n\n@Api('health')\n@Authenticated()\nexport class HealthController {\n  @Read()\n  check() {\n    return 'ok';\n  }\n}\n",
  ));
  files.extend(gate_app(
    "mono/apps/shop",
    "@acme/shop",
    "shop",
    "import { Api, Public, Read } from '@acme/gate';\n\n@Api('health')\nexport class HealthController {\n  @Read()\n  @Public()\n  check() {\n    return 'ok';\n  }\n}\n",
  ));
  let borrowed: Vec<(&str, &str)> = files.iter().map(|(path, content)| (path.as_str(), content.as_str())).collect();
  write_tree(&source, &borrowed);
  let mut settings = ProjectSettings::default();
  settings.detect.route_families = vec!["acme-gate".to_owned()];
  settings.detect.families = vec![DecoratorFamily::parse("acme-gate", FAMILY).unwrap().family];
  let indexed = index_tree_with(scratch.path(), "gate", &source, &["mono"], settings);
  let facts = pass_facts(&indexed.engine);

  let mut requirements: Vec<(String, serde_json::Value, serde_json::Value)> = facts
    .entities
    .iter()
    .filter(|entity| entity.kind == EntityKind::ApiEndpoint && entity.attrs.contains_key("auth_requirement"))
    .map(|entity| {
      (
        entity.key.to_string(),
        entity.attrs["auth_requirement"].clone(),
        entity.attrs.get("auth_combinator").cloned().unwrap_or_default(),
      )
    })
    .collect();
  requirements.sort_by(|left, right| left.0.cmp(&right.0));
  assert_eq!(
    requirements,
    [
      ("endpoint:mono:GET /admin/health".to_owned(), json!({ "all_of": ["bearer"] }), json!("all_of")),
      (
        "endpoint:mono:GET /health".to_owned(),
        json!({ "per_handler": [
          { "file": "apps/admin/src/health.controller.ts", "package": "@acme/admin", "requirement": { "all_of": ["bearer"] } },
          { "file": "apps/shop/src/health.controller.ts", "package": "@acme/shop", "requirement": "public" },
        ] }),
        json!("per_handler")
      ),
      ("endpoint:mono:GET /shop/health".to_owned(), json!("public"), json!(null)),
    ]
  );
  let mut supported: Vec<(String, serde_json::Value)> = facts
    .edges
    .iter()
    .filter(|edge| edge.kind == EdgeKind::SupportsAuth)
    .map(|edge| {
      let endpoint = facts
        .entities
        .iter()
        .find(|entity| entity.id == edge.from)
        .map(|entity| entity.key.to_string())
        .unwrap_or_default();
      (endpoint, edge.attrs["combinator"].clone())
    })
    .collect();
  supported.sort_by(|left, right| left.0.cmp(&right.0));
  assert_eq!(
    supported,
    [
      ("endpoint:mono:GET /admin/health".to_owned(), json!("all_of")),
      ("endpoint:mono:GET /health".to_owned(), json!("per_handler")),
    ]
  );
}
