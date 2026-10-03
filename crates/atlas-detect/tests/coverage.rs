mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use atlas_core::{EdgeKind, NodeRef};
use atlas_detect::DetectorRegistry;
use common::{fixture_files, fixtures_root, node};

const ACME_REPOS: [&str; 4] = ["acme/acme-api", "acme/acme-sdk", "acme/acme-web", "acme/acme-billing"];
const KINDS: [&str; 6] = ["api_endpoint", "auth_scheme", "env_var", "permission", "event", "db_model"];

#[derive(Debug, Default, PartialEq, Eq)]
struct Row {
  expected: usize,
  exact: usize,
  missed: Vec<String>,
  extra: Vec<String>,
}

struct Produced {
  entities: BTreeMap<String, String>,
  models: BTreeSet<(String, String)>,
}

fn produced() -> Produced {
  let registry = DetectorRegistry::default();
  let mut entities = BTreeMap::new();
  let mut models = BTreeSet::new();
  for repo in ACME_REPOS {
    for (_, analyzed) in fixture_files(repo) {
      let (detection, _) = registry.run_all(&analyzed.input());
      for entity in &detection.entities {
        entities.insert(entity.key.to_string(), entity.certainty.class().as_str().to_owned());
      }
      for edge in detection.edges.iter().filter(|edge| edge.kind == EdgeKind::MapsToTable) {
        if let (NodeRef::Local(_), NodeRef::Key(table)) = (&edge.from, &edge.to) {
          models.insert((node(&analyzed.facts, &edge.from), table.to_string()));
        }
      }
    }
  }
  Produced { entities, models }
}

fn route_key(route: &str) -> String {
  route
    .split('/')
    .map(|segment| match segment.strip_prefix(':') {
      Some(name) => format!("{{{name}}}"),
      None => segment.to_owned(),
    })
    .collect::<Vec<_>>()
    .join("/")
}

fn expected_key(entity: &toml::Value) -> String {
  let field = |name: &str| entity.get(name).and_then(toml::Value::as_str).unwrap_or_default().to_owned();
  match field("kind").as_str() {
    "api_endpoint" => format!("endpoint:{}:{} {}", field("repo"), field("method"), route_key(&field("route"))),
    "auth_scheme" => format!("auth:{}:{}", field("repo"), field("scheme")),
    "env_var" => format!("env:{}:{}", field("repo"), field("name")),
    "permission" => format!("permission:{}:{}", field("repo"), field("name")),
    "event" => format!("event:{}", field("name")),
    "db_model" => format!("table:{}", field("name")),
    other => panic!("unexpected kind {other}"),
  }
}

fn kind_of_key(key: &str) -> Option<&'static str> {
  let prefix = key.split(':').next()?;
  match prefix {
    "endpoint" => Some("api_endpoint"),
    "auth" => Some("auth_scheme"),
    "env" => Some("env_var"),
    "permission" => Some("permission"),
    "event" => Some("event"),
    "table" => Some("db_model"),
    _ => None,
  }
}

fn coverage() -> BTreeMap<&'static str, Row> {
  let expected: toml::Value = toml::from_str(&fs::read_to_string(fixtures_root().join("acme/expected.toml")).unwrap()).unwrap();
  let produced = produced();
  let mut rows: BTreeMap<&'static str, Row> = KINDS.iter().map(|kind| (*kind, Row::default())).collect();
  let mut expected_keys = BTreeSet::new();
  for entity in expected["entity"].as_array().unwrap() {
    let Some(kind) = KINDS.iter().find(|kind| entity["kind"].as_str() == Some(**kind)) else {
      continue;
    };
    let key = expected_key(entity);
    expected_keys.insert(key.clone());
    let row = rows.get_mut(kind).unwrap();
    row.expected += 1;
    let allowed: Option<Vec<&str>> = entity
      .get("certainty")
      .and_then(toml::Value::as_array)
      .map(|classes| classes.iter().filter_map(toml::Value::as_str).collect());
    let class_ok = produced
      .entities
      .get(&key)
      .is_some_and(|class| allowed.as_ref().is_none_or(|allowed| allowed.contains(&class.as_str())));
    let model_ok = *kind != "db_model" || {
      let symbol = entity["sym"]["symbol"].as_str().unwrap().to_owned();
      produced.models.contains(&(symbol, key.clone()))
    };
    if class_ok && model_ok {
      row.exact += 1;
    } else {
      row.missed.push(key);
    }
  }
  for (key, _) in produced.entities.iter().filter(|(key, _)| !expected_keys.contains(*key)) {
    if let Some(kind) = kind_of_key(key) {
      rows.get_mut(kind).unwrap().extra.push(key.clone());
    }
  }
  rows
}

#[test]
fn should_reproduce_the_acme_domain_entity_ground_truth() {
  let rows = coverage();
  for (kind, row) in &rows {
    println!(
      "{kind:<13} expected {:>2} exact {:>2} missed {:?} extra {:?}",
      row.expected, row.exact, row.missed, row.extra
    );
  }
  let summary: Vec<(&str, usize, usize, usize, usize)> = rows.iter().map(|(kind, row)| (*kind, row.expected, row.exact, row.missed.len(), row.extra.len())).collect();
  assert_eq!(
    summary,
    [
      ("api_endpoint", 10, 8, 2, 0),
      ("auth_scheme", 3, 3, 0, 0),
      ("db_model", 2, 2, 0, 0),
      ("env_var", 5, 5, 0, 2),
      ("event", 1, 1, 0, 0),
      ("permission", 3, 3, 0, 0),
    ]
  );
  assert_eq!(rows["env_var"].extra, ["env:acme-api:PORT", "env:acme-web:VITE_API_URL"]);
  assert_eq!(
    rows["api_endpoint"].missed,
    ["endpoint:acme-billing:POST /invoices", "endpoint:acme-billing:GET /invoices/{id}"]
  );
}
