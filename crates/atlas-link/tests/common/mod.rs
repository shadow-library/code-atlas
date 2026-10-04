#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{EdgeKind, Entity, EntityId, EntityKey, EntityKind, HttpMethod, Owner, ProjectName, RelPath, RepoName, normalize_route};
use atlas_engine::{Engine, EngineOptions};
use atlas_project::{AtlasHome, Project, ProjectSettings, RepoConfig};
use atlas_store::{EdgeFilter, EntityFilter, OwnedFacts, ReadTx};
use toml::Value;

pub fn fixtures_root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn copy_dir(from: &Path, to: &Path) {
  fs::create_dir_all(to).unwrap();
  for entry in fs::read_dir(from).unwrap() {
    let entry = entry.unwrap();
    let target = to.join(entry.file_name());
    match entry.file_type().unwrap().is_dir() {
      true => copy_dir(&entry.path(), &target),
      false => {
        fs::copy(entry.path(), &target).unwrap();
      }
    }
  }
}

pub struct Indexed {
  pub engine: Engine,
  pub project: Project,
  pub expected: Value,
}

pub fn index(scratch: &Path, name: &str, source: &Path, repos: &[&str], expected: &Path) -> Indexed {
  let expected = Value::Table(toml::from_str::<toml::Table>(&fs::read_to_string(expected).unwrap()).unwrap());
  Indexed {
    expected,
    ..index_tree(scratch, name, source, repos)
  }
}

pub fn write_tree(root: &Path, files: &[(&str, &str)]) {
  for (path, content) in files {
    let target = root.join(path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, content).unwrap();
  }
}

pub fn index_tree(scratch: &Path, name: &str, source: &Path, repos: &[&str]) -> Indexed {
  let repos = repos
    .iter()
    .map(|repo| {
      let target = scratch.join("tree").join(name).join(repo);
      copy_dir(&source.join(repo), &target);
      RepoConfig {
        name: RepoName::new(*repo).unwrap(),
        path: target,
        include: Vec::new(),
        exclude: Vec::new(),
        languages: None,
      }
    })
    .collect();
  let project = Project {
    name: ProjectName::new(name).unwrap(),
    settings: ProjectSettings::default(),
    repos,
  };
  let engine = Engine::open(&project, &AtlasHome::rooted_at(scratch.join("home")), EngineOptions::default()).unwrap();
  let report = engine.ensure_fresh().unwrap();
  assert!(report.is_fresh(), "{report:?}");
  Indexed {
    engine,
    project,
    expected: Value::Table(toml::Table::new()),
  }
}

pub fn acme(scratch: &Path) -> Indexed {
  let root = fixtures_root().join("acme");
  index(scratch, "acme", &root, &["acme-api", "acme-sdk", "acme-web", "acme-billing"], &root.join("expected.toml"))
}

pub fn globex(scratch: &Path) -> Indexed {
  let root = fixtures_root().join("globex");
  index(scratch, "globex", &root, &["globex-api", "globex-worker"], &root.join("expected.toml"))
}

pub fn computed_routes(scratch: &Path) -> Indexed {
  let root = fixtures_root().join("adversarial");
  index(
    scratch,
    "computed-routes-ts",
    &root,
    &["computed-routes-ts"],
    &root.join("computed-routes-ts/expected.toml"),
  )
}

pub fn pass_facts(engine: &Engine) -> OwnedFacts {
  let owner = Owner::Project {
    pass: atlas_link::PASS_NAME.to_owned(),
  };
  engine.store().read(|tx| tx.owned_facts(&owner)).unwrap()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selected {
  Id(EntityId),
  Kind(EntityKind),
}

fn text<'v>(value: &'v Value, field: &str) -> Option<&'v str> {
  value.get(field).and_then(Value::as_str)
}

fn repo(value: &Value) -> RepoName {
  RepoName::new(text(value, "repo").unwrap()).unwrap()
}

pub fn select(tx: &ReadTx<'_>, selector: &Value) -> Selected {
  match text(selector, "kind") {
    Some("api_endpoint") => match (text(selector, "method"), text(selector, "route")) {
      (Some(method), Some(route)) => Selected::Id(EntityId::of(&EntityKey::Endpoint {
        repo: repo(selector),
        method: method.parse::<HttpMethod>().unwrap(),
        route: normalize_route(route),
      })),
      _ => Selected::Kind(EntityKind::ApiEndpoint),
    },
    Some("auth_scheme") => Selected::Id(EntityId::of(&EntityKey::AuthScheme {
      repo: repo(selector),
      scheme: text(selector, "scheme").unwrap().to_owned(),
    })),
    Some("package") => {
      let name = text(selector, "name").unwrap();
      let found = tx.entities_by_name(name, &EntityFilter::default()).unwrap();
      let package = found
        .iter()
        .find(|entity| entity.kind == EntityKind::Package && entity.attrs.get("repo").and_then(|repo| repo.as_str()) == text(selector, "repo"))
        .unwrap_or_else(|| panic!("no package {selector:?}"));
      Selected::Id(package.id)
    }
    Some(kind) if selector.get("name").is_none() && selector.get("scheme").is_none() => Selected::Kind(kind.parse().unwrap()),
    Some(other) => panic!("unsupported selector kind {other}"),
    None => Selected::Id(symbol(tx, selector).id),
  }
}

pub fn symbol(tx: &ReadTx<'_>, selector: &Value) -> Entity {
  let repo = repo(selector);
  let path = RelPath::new(text(selector, "path").unwrap()).unwrap();
  let Some(name) = text(selector, "symbol") else {
    return tx.entity_by_key(&EntityKey::File { repo, path }).unwrap().unwrap();
  };
  let mut found: Vec<Entity> = tx
    .entities_in_file(&repo, &path, &EntityFilter::default())
    .unwrap()
    .into_iter()
    .filter(|entity| {
      entity.key.as_symbol().is_some_and(|key| {
        let items = key.item_path();
        items.join(".") == name || items.join("::") == name
      })
    })
    .collect();
  found.sort_by_key(|entity| entity.key.as_symbol().map(|key| key.disambiguator()));
  assert!(!found.is_empty(), "no symbol for {selector:?}");
  found.remove(0)
}

pub fn id(selected: &Selected) -> EntityId {
  match selected {
    Selected::Id(id) => *id,
    Selected::Kind(kind) => panic!("selector is a wildcard of {kind}"),
  }
}

pub fn edges_of(tx: &ReadTx<'_>, from: EntityId, kind: &EdgeKind) -> Vec<atlas_core::Edge> {
  tx.edges_from(
    from,
    &EdgeFilter {
      kinds: vec![kind.clone()],
      ..EdgeFilter::default()
    },
  )
  .unwrap()
}

pub fn entries<'v>(expected: &'v Value, section: &str) -> Vec<&'v Value> {
  expected.get(section).and_then(Value::as_array).map(|items| items.iter().collect()).unwrap_or_default()
}

pub fn classes(entry: &Value) -> Vec<String> {
  entry
    .get("certainty")
    .and_then(Value::as_array)
    .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
    .unwrap_or_default()
}

pub fn min_confidence(entry: &Value) -> Option<f64> {
  entry
    .get("min_confidence")
    .and_then(|value| value.as_float().or_else(|| value.as_integer().map(|int| int as f64)))
}

pub fn pass_assertion(tx: &ReadTx<'_>, id: EntityId) -> Option<Entity> {
  tx.entity_assertions(id)
    .unwrap()
    .into_iter()
    .find(|assertion| {
      assertion.owner
        == Owner::Project {
          pass: atlas_link::PASS_NAME.to_owned(),
        }
    })
    .map(|assertion| assertion.fact)
}
