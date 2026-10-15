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

fn methods(names: &[&str]) -> String {
  names
    .iter()
    .map(|name| format!("  @Read('{name}')\n  @Authenticated()\n  {name}() {{\n    return null;\n  }}\n"))
    .collect::<Vec<_>>()
    .join("\n")
}

#[test]
fn should_cite_only_the_closest_implementation_sites_of_a_guard_on_its_supports_auth_edges() {
  let scratch = tempfile::tempdir().unwrap();
  let source = scratch.path().join("source");
  let package = |name: &str| format!(r#"{{ "name": "{name}", "version": "1.0.0", "dependencies": {{ "@acme/gate": "^1.0.0" }} }}"#);
  let users = format!(
    "import {{ Api, Authenticated, Read }} from '@acme/gate';\n\n@Api('users')\nexport class UsersController {{\n{}}}\n",
    methods(&["a", "b", "c", "d", "e"])
  );
  let teams =
    "import { Api, Authenticated, Read } from '@acme/gate';\n\n@Api('teams')\n@Authenticated()\nexport class TeamsController {\n  @Read()\n  list() {\n    return [];\n  }\n}\n";
  let cart =
    "import { Api, Authenticated, Read } from '@acme/gate';\n\n@Api('cart')\n@Authenticated()\nexport class CartController {\n  @Read()\n  list() {\n    return [];\n  }\n}\n";
  let files = [
    (
      "mono/package.json".to_owned(),
      r#"{ "name": "mono", "private": true, "workspaces": ["apps/*"] }"#.to_owned(),
    ),
    ("mono/apps/admin/package.json".to_owned(), package("@acme/admin")),
    ("mono/apps/admin/src/users.controller.ts".to_owned(), users),
    ("mono/apps/admin/src/teams.controller.ts".to_owned(), teams.to_owned()),
    ("mono/apps/shop/package.json".to_owned(), package("@acme/shop")),
    ("mono/apps/shop/src/cart.controller.ts".to_owned(), cart.to_owned()),
  ];
  let borrowed: Vec<(&str, &str)> = files.iter().map(|(path, content)| (path.as_str(), content.as_str())).collect();
  write_tree(&source, &borrowed);
  let mut settings = ProjectSettings::default();
  settings.detect.route_families = vec!["acme-gate".to_owned()];
  settings.detect.families = vec![DecoratorFamily::parse("acme-gate", FAMILY).unwrap().family];
  let indexed = index_tree_with(scratch.path(), "gate", &source, &["mono"], settings);
  let facts = pass_facts(&indexed.engine);

  let mut cited: Vec<(String, Vec<String>)> = facts
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
      let mut sites: Vec<(u32, String)> = edge
        .evidence
        .iter()
        .map(|evidence| {
          (
            evidence.span.start_line,
            format!("{}:{} {}", evidence.path, evidence.span.start_line + 1, evidence.note.clone().unwrap_or_default()),
          )
        })
        .collect();
      sites.sort();
      let sites: Vec<String> = sites.into_iter().map(|(_, site)| site).collect();
      (endpoint, sites)
    })
    .filter(|(endpoint, _)| ["/users/a", "/users/e", "/teams", "/cart"].iter().any(|route| endpoint.ends_with(route)))
    .collect();
  cited.sort();
  assert_eq!(
    cited,
    [
      (
        "endpoint:mono:GET /cart".to_owned(),
        vec![
          "apps/shop/src/cart.controller.ts:5 implements auth".to_owned(),
          "apps/shop/src/cart.controller.ts:7 guard".to_owned(),
        ]
      ),
      (
        "endpoint:mono:GET /teams".to_owned(),
        vec![
          "apps/admin/src/teams.controller.ts:5 implements auth".to_owned(),
          "apps/admin/src/teams.controller.ts:7 guard".to_owned(),
        ]
      ),
      (
        "endpoint:mono:GET /users/a".to_owned(),
        vec![
          "apps/admin/src/users.controller.ts:6 guard".to_owned(),
          "apps/admin/src/users.controller.ts:7 implements auth".to_owned(),
          "apps/admin/src/users.controller.ts:13 implements auth".to_owned(),
          "apps/admin/src/users.controller.ts:19 implements auth".to_owned(),
        ]
      ),
      (
        "endpoint:mono:GET /users/e".to_owned(),
        vec![
          "apps/admin/src/users.controller.ts:19 implements auth".to_owned(),
          "apps/admin/src/users.controller.ts:25 implements auth".to_owned(),
          "apps/admin/src/users.controller.ts:30 guard".to_owned(),
          "apps/admin/src/users.controller.ts:31 implements auth".to_owned(),
        ]
      ),
    ]
  );
}
