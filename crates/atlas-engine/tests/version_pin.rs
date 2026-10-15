mod common;

use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{ContentHash, EdgeKind, Owner};
use atlas_engine::{PASS_STRUCTURE, STRUCTURE_VERSION};
use atlas_project::Project;
use common::{copy_dir, digest, fixtures_root, fresh, open, project, repo_config};

const PINNED_STRUCTURE: (u32, &str, &str) = (
  1,
  "d89718c807cad9028e46e394bf04f8826ac28913475b8e281d9e945e6b55917d",
  "ec3f51b2a78d7e98e15aa5e2c7e63822eeeec8282aac7b8cc0ed688a426f1776",
);
const PINNED_LINK: (u32, &str, &str) = (
  5,
  "8a445d617caa1e9ee155259e2efc16689031bfd802e72ceb32fdaf0dc9c9efed",
  "5f879ce9422628ce9d8a12738fb59a610c12396828d994f9d44a468e52f573cb",
);

#[derive(Default)]
struct PassDigests {
  inputs: Vec<u8>,
  output: Vec<u8>,
}

fn walk_dirs(dir: &Path) -> Vec<PathBuf> {
  let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
    .unwrap()
    .flatten()
    .filter(|entry| entry.file_type().unwrap().is_dir())
    .map(|entry| entry.path())
    .collect();
  dirs.sort();
  dirs
}

fn fixture_projects(tree: &Path) -> Vec<Project> {
  let mut projects = Vec::new();
  for group in walk_dirs(&fixtures_root()) {
    let group_name = group.file_name().unwrap().to_string_lossy().into_owned();
    let repos: Vec<(String, PathBuf)> = walk_dirs(&group)
      .into_iter()
      .map(|source| {
        let name = source.file_name().unwrap().to_string_lossy().into_owned();
        let target = tree.join(&group_name).join(&name);
        copy_dir(&source, &target);
        (name, target)
      })
      .collect();
    match group_name == "adversarial" {
      true => projects.extend(repos.iter().map(|(name, path)| project(name, vec![repo_config(name, path)]))),
      false => projects.push(project(&group_name, repos.iter().map(|(name, path)| repo_config(name, path)).collect())),
    }
  }
  projects
}

fn link_corpus_project(tree: &Path) -> Project {
  let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pin-corpus/link");
  let repos = walk_dirs(&root)
    .into_iter()
    .map(|source| {
      let name = source.file_name().unwrap().to_string_lossy().into_owned();
      let target = tree.join("link-corpus").join(&name);
      copy_dir(&source, &target);
      repo_config(&name, &target)
    })
    .collect();
  project("link-corpus", repos)
}

fn link_family_project(tree: &Path) -> Project {
  let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pin-corpus/link-family");
  let repos = walk_dirs(&root)
    .into_iter()
    .map(|source| {
      let name = source.file_name().unwrap().to_string_lossy().into_owned();
      let target = tree.join("link-family").join(&name);
      copy_dir(&source, &target);
      repo_config(&name, &target)
    })
    .collect();
  let definition = fs::read_to_string(root.join("acme-http.toml")).unwrap();
  let mut project = project("link-family", repos);
  project.settings.detect.route_families = vec!["acme-http".to_owned()];
  project.settings.detect.families = vec![atlas_detect::DecoratorFamily::parse("acme-http", &definition).unwrap().family];
  project
}

fn link_monorepo_project(tree: &Path) -> Project {
  let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pin-corpus/link-monorepo");
  let repos = walk_dirs(&root)
    .into_iter()
    .map(|source| {
      let name = source.file_name().unwrap().to_string_lossy().into_owned();
      let target = tree.join("link-monorepo").join(&name);
      copy_dir(&source, &target);
      repo_config(&name, &target)
    })
    .collect();
  let definition = fs::read_to_string(root.join("acme-orders.toml")).unwrap();
  let mut project = project("link-monorepo", repos);
  project.settings.detect.route_families = vec!["acme-orders".to_owned()];
  project.settings.detect.families = vec![atlas_detect::DecoratorFamily::parse("acme-orders", &definition).unwrap().family];
  project
}

fn canonical_owner(facts: &atlas_store::OwnedFacts) -> Vec<u8> {
  let mut lines: Vec<String> = facts.entities.iter().map(|entity| serde_json::to_string(entity).unwrap()).collect();
  lines.extend(facts.edges.iter().map(|edge| serde_json::to_string(edge).unwrap()));
  lines.sort();
  lines.join("\n").into_bytes()
}

fn pass_digests(scratch: &Path) -> (PassDigests, PassDigests) {
  let mut structure = PassDigests::default();
  let mut link = PassDigests::default();
  let tree = scratch.join("tree");
  let mut projects: Vec<(Project, bool)> = fixture_projects(&tree).into_iter().map(|project| (project, true)).collect();
  projects.push((link_corpus_project(&tree), false));
  projects.push((link_family_project(&tree), false));
  projects.push((link_monorepo_project(&tree), false));
  for (index, (project, with_structure)) in projects.iter().enumerate() {
    let engine = open(project, &scratch.join(format!("home-{index}")));
    assert!(fresh(&engine).is_fresh());
    let mut passes = vec![(&mut link, atlas_link::PASS_NAME)];
    if *with_structure {
      passes.push((&mut structure, PASS_STRUCTURE));
    }
    for (digests, pass) in passes {
      let owner = Owner::Project { pass: pass.to_owned() };
      let facts = engine.store().read(|tx| tx.owned_facts(&owner)).unwrap();
      digests.output.extend(project.name.as_str().as_bytes());
      digests.output.extend(canonical_owner(&facts));
      engine.store().write(|tx| tx.remove_owner(&owner)).unwrap();
      digests.inputs.extend(digest(&engine).as_bytes());
    }
  }
  (structure, link)
}

fn assert_pinned(constant: &str, pinned: (u32, &str, &str), version: u32, digests: &PassDigests) {
  let inputs = ContentHash::of(&digests.inputs).to_string();
  let output = ContentHash::of(&digests.output).to_string();
  if (version, inputs.as_str(), output.as_str()) == pinned {
    return;
  }
  let advice = match (inputs == pinned.1, version == pinned.0) {
    (false, _) => format!("the facts this pass reads changed (fixtures or an upstream producer), so re-pin; if the pass's code changed as well, also bump {constant}"),
    (true, true) => format!("the pass output over unchanged inputs changed: bump {constant} so the engine re-runs the passes on stored indexes, then re-pin"),
    (true, false) => format!("{constant} changed: re-pin"),
  };
  panic!("{advice}\nnew pin: ({version}, \"{inputs}\", \"{output}\")");
}

#[test]
fn should_change_the_structure_and_link_pass_output_over_the_fixtures_only_with_a_version_bump() {
  let scratch = tempfile::tempdir().unwrap();
  let (structure, link) = pass_digests(scratch.path());
  assert_pinned("atlas_engine::STRUCTURE_VERSION", PINNED_STRUCTURE, STRUCTURE_VERSION, &structure);
  assert_pinned("atlas_link::LINK_VERSION", PINNED_LINK, atlas_link::LINK_VERSION, &link);
}

#[test]
fn should_derive_auth_usage_in_the_link_pin_corpus_from_production_code_only() {
  let scratch = tempfile::tempdir().unwrap();
  let engine = open(&link_corpus_project(scratch.path()), &scratch.path().join("home"));
  assert!(fresh(&engine).is_fresh());
  let owner = Owner::Project {
    pass: atlas_link::PASS_NAME.to_owned(),
  };
  let users = engine
    .store()
    .read(|tx| -> Result<Vec<String>, atlas_store::StoreError> {
      let mut names = Vec::new();
      for edge in tx.owned_facts(&owner)?.edges.iter().filter(|edge| edge.kind == EdgeKind::UsesAuth) {
        names.extend(tx.entity(edge.from)?.map(|entity| entity.name));
      }
      names.sort();
      Ok(names)
    })
    .unwrap();
  assert_eq!(users, vec!["loadMe".to_owned(), "load_order".to_owned()]);
}

#[test]
fn should_compose_family_prefixes_and_link_surface_clients_and_option_routes_in_the_link_pin_corpus() {
  let scratch = tempfile::tempdir().unwrap();
  let engine = open(&link_family_project(scratch.path()), &scratch.path().join("home"));
  assert!(fresh(&engine).is_fresh());
  let owner = Owner::Project {
    pass: atlas_link::PASS_NAME.to_owned(),
  };
  let links = engine
    .store()
    .read(|tx| -> Result<Vec<String>, atlas_store::StoreError> {
      let mut links = Vec::new();
      for edge in tx.owned_facts(&owner)?.edges.iter().filter(|edge| edge.kind == EdgeKind::CallsEndpoint) {
        let (Some(from), Some(to)) = (tx.entity(edge.from)?, tx.entity(edge.to)?) else {
          continue;
        };
        links.push(format!("{} -> {}", from.name, to.key));
      }
      links.sort();
      Ok(links)
    })
    .unwrap();
  assert_eq!(
    links,
    [
      "checkHealth -> endpoint:family-api:GET /api/health",
      "createOrder -> endpoint:family-api:POST /api/orders",
      "findExport -> endpoint:family-api:GET /api/exports/{exportId}",
      "findOrder -> endpoint:family-api:GET /api/orders/{id}",
      "latestExportFile -> endpoint:family-api:GET /api/exports/{exportId}/drafts/{n}/files/latest",
      "loadOrder -> endpoint:family-api:GET /api/orders/{id}",
      "loadProject -> endpoint:family-api:GET /api/projects/{id}",
      "runExport -> endpoint:family-api:POST /api/exports/run",
    ]
  );
}

#[test]
fn should_derive_auth_and_permissions_of_declared_guard_decorators_in_the_link_pin_corpus() {
  let scratch = tempfile::tempdir().unwrap();
  let engine = open(&link_family_project(scratch.path()), &scratch.path().join("home"));
  assert!(fresh(&engine).is_fresh());
  let owner = Owner::Project {
    pass: atlas_link::PASS_NAME.to_owned(),
  };
  let render = |tx: &atlas_store::ReadTx<'_>, edge: &atlas_core::Edge| -> Result<String, atlas_store::StoreError> {
    let (Some(from), Some(to)) = (tx.entity(edge.from)?, tx.entity(edge.to)?) else {
      return Ok(format!("{} dangling", edge.kind));
    };
    Ok(format!("{} {} -> {} {}", edge.kind, from.key, to.key, edge.certainty.tag()))
  };
  let lines = engine
    .store()
    .read(|tx| -> Result<Vec<String>, atlas_store::StoreError> {
      let mut lines = Vec::new();
      for edge in tx
        .owned_facts(&owner)?
        .edges
        .iter()
        .filter(|edge| matches!(edge.kind, EdgeKind::SupportsAuth | EdgeKind::UsesAuth))
      {
        lines.push(render(tx, edge)?);
      }
      let guard = "other:guard_decorator:family-api:acme-http:Authenticated".parse().unwrap();
      let guard = tx.entity_by_key(&guard)?.unwrap();
      for edge in tx.edges_from(guard.id, &atlas_store::EdgeFilter::default())? {
        lines.push(render(tx, &edge)?);
      }
      let permission = tx.entity_by_key(&"permission:family-api:PROJECTS_READ".parse().unwrap())?.unwrap();
      for edge in tx
        .edges_to(permission.id, &atlas_store::EdgeFilter::default())?
        .iter()
        .chain(&tx.edges_from(permission.id, &atlas_store::EdgeFilter::default())?)
      {
        lines.push(render(tx, edge)?);
      }
      lines.sort();
      Ok(lines)
    })
    .unwrap();
  assert_eq!(
    lines,
    [
      "implements_auth other:guard_decorator:family-api:acme-http:Authenticated -> auth:family-api:bearer [syntactic]",
      "references permission:family-api:PROJECTS_READ -> ts:family-api:family-api:src/permissions::PROJECTS_READ#constant [resolved]",
      "requires_permission ts:family-api:family-api:src/projects.controller::ProjectsController::find#method -> permission:family-api:PROJECTS_READ [heuristic 0.60]",
      "supports_auth endpoint:family-api:GET /api/projects/{id} -> auth:family-api:bearer [heuristic 0.99]",
      "supports_auth endpoint:family-api:GET /projects/{id} -> auth:family-api:bearer [heuristic 0.99]",
      "uses_auth ts:family-web:family-web:src/projects::loadProject#function -> auth:family-api:bearer [heuristic 0.75]",
    ]
  );
}

#[test]
fn should_place_monorepo_client_calls_by_package_in_the_link_pin_corpus() {
  let scratch = tempfile::tempdir().unwrap();
  let engine = open(&link_monorepo_project(scratch.path()), &scratch.path().join("home"));
  assert!(fresh(&engine).is_fresh());
  let owner = Owner::Project {
    pass: atlas_link::PASS_NAME.to_owned(),
  };
  let links = engine
    .store()
    .read(|tx| -> Result<Vec<String>, atlas_store::StoreError> {
      let mut links = Vec::new();
      for edge in tx.owned_facts(&owner)?.edges.iter().filter(|edge| edge.kind == EdgeKind::CallsEndpoint) {
        let (Some(from), Some(to)) = (tx.entity(edge.from)?, tx.entity(edge.to)?) else {
          continue;
        };
        let rule = edge.attrs.get("package_rule").and_then(serde_json::Value::as_str).unwrap_or("-");
        let target = edge.attrs.get("target_package").and_then(serde_json::Value::as_str).unwrap_or("-");
        links.push(format!("{} -> {} ({rule} {target}, share {})", from.name, to.key, edge.attrs["share"]));
      }
      links.sort();
      Ok(links)
    })
    .unwrap();
  assert_eq!(
    links,
    [
      "checkBooks -> endpoint:mono:GET /api/health (types-package @acme/books-api, share 1.0)",
      "checkOrders -> endpoint:mono:GET /api/health (configured-package @acme/orders-api, share 1.0)",
      "createOrderItem -> endpoint:mono:POST /api/items (configured-package @acme/orders-api, share 1.0)",
      "loadAnyItem -> endpoint:mono:GET /api/items/{id} (- -, share 0.5)",
      "loadAnyItem -> endpoint:mono:GET /api/items/{itemId} (- -, share 0.5)",
      "loadBookItem -> endpoint:mono:GET /api/items/{id} (types-package @acme/books-api, share 1.0)",
      "loadOrderItem -> endpoint:mono:GET /api/items/{itemId} (configured-package @acme/orders-api, share 1.0)",
    ]
  );
}
