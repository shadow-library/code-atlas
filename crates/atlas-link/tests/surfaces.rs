mod common;

use atlas_core::{EdgeKind, EntityKind};
use atlas_detect::DecoratorFamily;
use atlas_project::ProjectSettings;
use common::{Indexed, index_tree_with, pass_facts, write_tree};
use serde_json::{Value, json};

const SERVER_FAMILY: &str = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\nWrite = \"POST\"\n[routes.options]\ndecorators = [\"Route\"]\n[global_prefix]\nfactory = \"AcmeServer.create\"\nmethod = \"setGlobalPrefix\"\n";

const CLIENT_FAMILY: &str = "[client]\nfiles = [\"apps/web/src/transport.ts\"]\nexports = [\"APIRequest\", \"apiClient\", \"legacyApi\"]\nbase = \"/api\"\n[client.surfaces]\nmodules = [\"@acme/http-kit\"]\nfactory = \"createGatewayClient\"\noption = \"surfaces\"\n";

const TREE: &[(&str, &str)] = &[
  ("kit/package.json", r#"{ "name": "kit", "private": true, "workspaces": ["apps/*"] }"#),
  (
    "kit/apps/server/package.json",
    r#"{ "name": "@acme/server", "version": "1.0.0", "dependencies": { "@acme/http": "^1.0.0" } }"#,
  ),
  (
    "kit/apps/server/src/main.ts",
    "import { AcmeServer } from '@acme/http';\n\nclass AppModule {}\n\nasync function boot() {\n  const app = await AcmeServer.create(AppModule);\n  app.setGlobalPrefix('api');\n}\n\nboot();\n",
  ),
  (
    "kit/apps/server/src/users.controller.ts",
    "import { Api, Read } from '@acme/http';\n\n@Api('v1/admin/users')\nexport class UsersController {\n  @Read()\n  list() {\n    return [];\n  }\n}\n",
  ),
  (
    "kit/apps/server/src/projects.controller.ts",
    "import { Api, Read, Write } from '@acme/http';\n\n@Api('v1/projects')\nexport class ProjectsController {\n  @Write(':id/x')\n  touch() {\n    return true;\n  }\n\n  @Read(':projectId/drafts/:n/versions/latest')\n  latest() {\n    return null;\n  }\n\n  @Read(':projectId/chapters')\n  chapters() {\n    return [];\n  }\n}\n",
  ),
  (
    "kit/apps/server/src/auth.controller.ts",
    "import { Api, Read } from '@acme/http';\n\n@Api('auth')\nexport class AuthController {\n  @Read('session')\n  session() {\n    return null;\n  }\n\n  @Read('projects/:id/versions/latest')\n  latestGrant() {\n    return null;\n  }\n}\n",
  ),
  (
    "kit/apps/server/src/imports.controller.ts",
    "import { Api, Route } from '@acme/http';\n\nconst IMPORT_ROUTE_OPTIONS = { method: 'POST', path: 'run' } as const;\n\n@Api('v1/imports')\nexport class ImportsController {\n  @Route(IMPORT_ROUTE_OPTIONS)\n  run() {\n    return true;\n  }\n}\n",
  ),
  (
    "kit/apps/web/package.json",
    r#"{ "name": "@acme/web", "version": "1.0.0", "dependencies": { "@acme/http-kit": "^1.0.0" } }"#,
  ),
  (
    "kit/apps/web/src/transport.ts",
    "import { createGatewayClient } from '@acme/http-kit';\nimport { makeLegacy } from './legacy';\n\nexport const apiClient = createGatewayClient({ surfaces: { v1: '/api/v1', auth: '/api/auth' }, ssr: false });\n\nexport const APIRequest = apiClient.v1;\n\nexport const legacyApi = makeLegacy();\n",
  ),
  (
    "kit/apps/web/src/pages.ts",
    r#"import { APIRequest, apiClient, legacyApi } from './transport';

export const loadUsers = (q: string) => APIRequest.get('/admin/users').query(q).execute();

export const touchProject = (id: string) => APIRequest.post(`/projects/${id}/x`).body({ id }).execute();

export const loadSession = () => apiClient.auth.get('/session');

export const runImport = () => APIRequest.post('/imports/run').execute();

export const legacyUsers = () => legacyApi.get('/v1/admin/users');

export const latestDraft = (id: string, n: number) => APIRequest.get(`${draftPath(id, n)}/versions/latest`).execute();

export const loadChapters = (base: string) => APIRequest.get(`${base}/chapters`).execute();

export const anyLatest = (prefix: string) => fetch(`${prefix}/versions/latest`);

export const loadCachedUsers = () => APIRequest.cache.get('/v1/admin/users');

export const loadNextUsers = () => apiClient.v2.get('/v1/admin/users');
"#,
  ),
];

fn kit(scratch: &std::path::Path) -> Indexed {
  let source = scratch.join("source");
  write_tree(&source, TREE);
  let mut settings = ProjectSettings::default();
  settings.detect.route_families = vec!["acme-http".to_owned(), "acme-kit".to_owned()];
  settings.detect.families = vec![
    DecoratorFamily::parse("acme-http", SERVER_FAMILY).unwrap().family,
    DecoratorFamily::parse("acme-kit", CLIENT_FAMILY).unwrap().family,
  ];
  index_tree_with(scratch, "kit", &source, &["kit"], settings)
}

fn links(indexed: &Indexed) -> Vec<String> {
  let facts = pass_facts(&indexed.engine);
  let mut found: Vec<String> = indexed
    .engine
    .store()
    .read(|tx| -> Result<_, atlas_store::StoreError> {
      let mut found = Vec::new();
      for edge in facts.edges.iter().filter(|edge| edge.kind == EdgeKind::CallsEndpoint) {
        let caller = tx.entity(edge.from)?.unwrap().name;
        let endpoint = tx.entity(edge.to)?.unwrap().key.to_string();
        let attr = |name: &str| edge.attrs.get(name).and_then(Value::as_str).unwrap_or("-").to_owned();
        found.push(format!(
          "{caller} -> {endpoint} ({} {}, share {}, base {}, defined in {})",
          attr("rule"),
          edge.certainty.tag(),
          edge.attrs["share"],
          attr("client_base"),
          attr("client_defined_in").rsplit('#').next().unwrap_or("-")
        ));
      }
      Ok(found)
    })
    .unwrap();
  found.sort();
  found
}

#[test]
fn should_link_surface_clients_option_routes_and_helper_prefixed_calls_end_to_end() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = kit(scratch.path());
  assert_eq!(
    links(&indexed),
    [
      "latestDraft -> endpoint:kit:GET /api/v1/projects/{projectId}/drafts/{n}/versions/latest (dynamic-head-prefix [heuristic 0.49], share 1.0, base /api/v1, defined in APIRequest)",
      "legacyUsers -> endpoint:kit:GET /api/v1/admin/users (path-template [heuristic 0.82], share 1.0, base /api, defined in -)",
      "loadSession -> endpoint:kit:GET /api/auth/session (path-template [heuristic 0.82], share 1.0, base /api/auth, defined in apiClient.auth)",
      "loadUsers -> endpoint:kit:GET /api/v1/admin/users (path-template [heuristic 0.82], share 1.0, base /api/v1, defined in APIRequest)",
      "runImport -> endpoint:kit:POST /api/v1/imports/run (path-template [heuristic 0.82], share 1.0, base /api/v1, defined in APIRequest)",
      "touchProject -> endpoint:kit:POST /api/v1/projects/{id}/x (path-template [heuristic 0.87], share 1.0, base /api/v1, defined in APIRequest)",
    ]
  );
}

#[test]
fn should_leave_a_helper_prefix_with_a_one_segment_tail_unmatched() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = kit(scratch.path());
  let facts = pass_facts(&indexed.engine);
  let unmatched: Vec<(String, Value)> = facts
    .entities
    .iter()
    .filter(|entity| entity.kind == EntityKind::ApiClientCall && entity.attrs.get("unmatched") == Some(&json!(true)))
    .map(|entity| (entity.key.to_string(), entity.attrs["unmatched_reason"].clone()))
    .collect();
  assert_eq!(unmatched, [("client:kit:GET {}/chapters".to_owned(), json!("no_matching_endpoint"))]);
}
