mod common;

use std::fs;
use std::path::Path;

use atlas_core::EntityKind;
use atlas_detect::DecoratorFamily;
use atlas_engine::{FreshnessReport, RouteFamilyState, RouteFamilyStatus, ServerMatches};
use atlas_project::Project;
use atlas_store::EntityFilter;

use common::{fresh, open, project, repo_config};

const FAMILY: &str = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\nWrite = \"POST\"\n";
const CONTROLLER: &str = "import { Api, Read, Write } from '@acme/http';\n\n@Api('orders')\nexport class OrdersController {\n  @Read()\n  list() {\n    return [];\n  }\n\n  @Write()\n  create() {\n    return null;\n  }\n}\n";
const LOOKALIKE: &str = "import { Api, Read } from '@other/http';\n\n@Api('lookalike')\nexport class LookalikeController {\n  @Read()\n  list() {\n    return [];\n  }\n}\n";

fn family(name: &str, text: &str) -> DecoratorFamily {
  DecoratorFamily::parse(name, text).unwrap().family
}

fn service(root: &Path) -> Project {
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(root.join("package.json"), "{ \"name\": \"orders\", \"version\": \"1.0.0\" }\n").unwrap();
  fs::write(root.join("src/orders.controller.ts"), CONTROLLER).unwrap();
  fs::write(root.join("src/lookalike.controller.ts"), LOOKALIKE).unwrap();
  project("shop", vec![repo_config("orders", root)])
}

fn with_families(mut project: Project, enabled: &[&str], families: Vec<DecoratorFamily>) -> Project {
  project.settings.detect.route_families = enabled.iter().map(|name| (*name).to_owned()).collect();
  project.settings.detect.families = families;
  project
}

fn endpoints(engine: &atlas_engine::Engine) -> Vec<String> {
  let mut keys: Vec<String> = engine
    .store()
    .read(|tx| tx.entities_by_kind(&EntityKind::ApiEndpoint, &EntityFilter::default()))
    .unwrap()
    .into_iter()
    .map(|entity| entity.key.to_string())
    .collect();
  keys.sort();
  keys
}

fn client_calls(engine: &atlas_engine::Engine) -> Vec<String> {
  let mut keys: Vec<String> = engine
    .store()
    .read(|tx| tx.entities_by_kind(&EntityKind::ApiClientCall, &EntityFilter::default()))
    .unwrap()
    .into_iter()
    .map(|entity| entity.key.to_string())
    .collect();
  keys.sort();
  keys
}

fn files_analyzed(engine: &atlas_engine::Engine) -> u64 {
  engine.store().read(|tx| tx.runs(1)).unwrap()[0].report.counts["files_analyzed"]
}

#[test]
fn should_index_an_enabled_family_and_report_what_each_family_matched() {
  let scratch = tempfile::tempdir().unwrap();
  let home = scratch.path().join("home");
  let enabled = with_families(
    service(&scratch.path().join("orders")),
    &["acme-htp", "acme-http", "acme-rpc"],
    vec![family("acme-http", FAMILY), family("acme-rpc", &FAMILY.replace("@acme/http", "@acme/rpc"))],
  );
  let engine = open(&enabled, &home);

  assert!(fresh(&engine).is_fresh());

  assert_eq!(endpoints(&engine), ["endpoint:orders:GET /orders", "endpoint:orders:POST /orders"]);
  assert_eq!(
    engine.status().unwrap().route_families,
    [
      RouteFamilyStatus {
        name: "acme-htp".to_owned(),
        state: RouteFamilyState::Undeclared {
          path: home.join("config/detectors/acme-htp.toml").display().to_string(),
        },
      },
      RouteFamilyStatus {
        name: "acme-http".to_owned(),
        state: RouteFamilyState::Matched {
          server: Some(ServerMatches { controllers: 1, routes: 2 }),
          client_calls: None,
        },
      },
      RouteFamilyStatus {
        name: "acme-rpc".to_owned(),
        state: RouteFamilyState::Matched {
          server: Some(ServerMatches { controllers: 0, routes: 0 }),
          client_calls: None,
        },
      },
    ]
  );
}

#[test]
fn should_keep_the_detector_list_and_fingerprint_of_a_project_without_families() {
  let scratch = tempfile::tempdir().unwrap();
  let plain = service(&scratch.path().join("orders"));
  let engine = open(&plain, &scratch.path().join("home"));
  fresh(&engine);

  let adopted = engine.store().read(|tx| tx.blob("engine:project")).unwrap().unwrap();
  let mut repos: Vec<&atlas_project::RepoConfig> = plain.repos.iter().collect();
  repos.sort_by(|left, right| left.name.cmp(&right.name));
  let identity = serde_json::json!({
    "name": plain.name.as_str(),
    "repos": repos,
    "max_file_bytes": plain.settings.max_file_bytes,
    "store_schema": atlas_store::SCHEMA_VERSION,
  });

  assert_eq!(
    String::from_utf8(adopted.bytes).unwrap(),
    blake3::hash(identity.to_string().as_bytes()).to_hex().to_string()
  );
  assert!(endpoints(&engine).is_empty());
  assert!(engine.status().unwrap().route_families.is_empty());
}

#[test]
fn should_reanalyse_on_a_definition_change_and_stop_an_engine_holding_the_old_one_until_it_reloads() {
  let scratch = tempfile::tempdir().unwrap();
  let home = scratch.path().join("home");
  let base = service(&scratch.path().join("orders"));
  let first = with_families(base.clone(), &["acme-http"], vec![family("acme-http", FAMILY)]);
  let second = with_families(base.clone(), &["acme-http"], vec![family("acme-http", &FAMILY.replace("Read = \"GET\"", "Read = \"PUT\""))]);
  let old = open(&first, &home);
  assert!(fresh(&old).is_fresh());
  assert_eq!(endpoints(&old), ["endpoint:orders:GET /orders", "endpoint:orders:POST /orders"]);

  let new = open(&second, &home);
  assert!(fresh(&new).is_fresh());
  assert_eq!(files_analyzed(&new), 2);
  assert_eq!(endpoints(&new), ["endpoint:orders:POST /orders", "endpoint:orders:PUT /orders"]);

  let FreshnessReport::Degraded { reason, header, .. } = fresh(&old) else {
    panic!("the old engine kept indexing")
  };
  assert_eq!((reason.as_str(), header.run), ("project configuration changed; reopen the engine", None));
  old.reload_project(&second).unwrap();
  let reloaded = fresh(&old);
  assert!(reloaded.is_fresh() && reloaded.header().run.is_none(), "{reloaded:?}");

  old.reload_project(&base).unwrap();
  assert!(fresh(&old).is_fresh());
  assert_eq!(files_analyzed(&old), 2);
  assert!(endpoints(&old).is_empty());
}

#[test]
fn should_index_a_client_only_family_and_count_its_calls_and_silent_shapes() {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("orders");
  let client = "[client]\nfiles = [\"src/api/*.ts\"]\nmethods = [\"get\"]\n";
  let enabled = with_families(
    service(&root),
    &["acme-books", "acme-http", "acme-silent"],
    vec![
      family("acme-books", client),
      family("acme-http", &format!("{FAMILY}[client]\nmodules = [\"@acme/http/client\"]\n")),
      family("acme-silent", "[client]\nmodules = [\"@acme/none\"]\n"),
    ],
  );
  fs::create_dir_all(root.join("src/api")).unwrap();
  fs::write(root.join("src/api/books.ts"), "export const booksApi = { get: (path: string) => path };\n").unwrap();
  fs::write(
    root.join("src/shelf.ts"),
    "import { booksApi } from './api/books';\n\nexport const loadBook = (id: string) => booksApi.get('/books/{id}');\nexport const loadShelf = () => booksApi.get('/shelves');\n",
  )
  .unwrap();
  let engine = open(&enabled, &scratch.path().join("home"));

  assert!(fresh(&engine).is_fresh());

  assert_eq!(client_calls(&engine), ["client:orders:GET /books/{}", "client:orders:GET /shelves"]);
  assert_eq!(
    engine.status().unwrap().route_families,
    [
      RouteFamilyStatus {
        name: "acme-books".to_owned(),
        state: RouteFamilyState::Matched {
          server: None,
          client_calls: Some(2),
        },
      },
      RouteFamilyStatus {
        name: "acme-http".to_owned(),
        state: RouteFamilyState::Matched {
          server: Some(ServerMatches { controllers: 1, routes: 2 }),
          client_calls: Some(0),
        },
      },
      RouteFamilyStatus {
        name: "acme-silent".to_owned(),
        state: RouteFamilyState::Matched {
          server: None,
          client_calls: Some(0),
        },
      },
    ]
  );
}
