mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use atlas_core::{FileFacts, KeyContext, RelPath};
use atlas_project::{PackageGraph, RepoConfig, discover};
use atlas_resolve::{ProjectSymbolTable, ResolvedFile, resolve_file};
use common::{Project, fixtures_root};

type Corpus = Vec<(FileFacts, KeyContext)>;

fn repos() -> Vec<(&'static str, PathBuf)> {
  let acme = fixtures_root().join("acme");
  vec![
    ("acme-api", acme.join("acme-api")),
    ("acme-sdk", acme.join("acme-sdk")),
    ("acme-web", acme.join("acme-web")),
    ("acme-billing", acme.join("acme-billing")),
    ("duplicate-names-rust", fixtures_root().join("adversarial/duplicate-names-rust")),
  ]
}

fn graph(name: &str, root: &Path) -> PackageGraph {
  let config = RepoConfig {
    name: common::repo(name),
    path: root.to_path_buf(),
    include: Vec::new(),
    exclude: Vec::new(),
    languages: None,
  };
  PackageGraph::load(root, &discover(&config, 2 * 1024 * 1024).unwrap().files)
}

fn table_with_packages() -> ProjectSymbolTable {
  let mut table = ProjectSymbolTable::new();
  for (name, root) in repos() {
    table.set_repo_packages(common::repo(name), graph(name, &root));
  }
  table
}

fn from_scratch(corpus: &Corpus) -> ProjectSymbolTable {
  let mut table = table_with_packages();
  for (facts, context) in corpus {
    table.upsert_file(facts, context).unwrap();
  }
  table.refresh();
  table
}

fn resolve_all(table: &ProjectSymbolTable, corpus: &Corpus) -> BTreeMap<(String, String), ResolvedFile> {
  corpus
    .iter()
    .map(|(facts, _)| ((facts.file.repo.to_string(), facts.file.path.to_string()), resolve_file(facts, table).unwrap()))
    .collect()
}

fn assert_equivalent(incremental: &ProjectSymbolTable, corpus: &Corpus) {
  let expected = resolve_all(&from_scratch(corpus), corpus);
  let actual = resolve_all(incremental, corpus);
  assert_eq!(actual.len(), expected.len());
  for (key, expected) in &expected {
    assert_eq!(actual.get(key), Some(expected), "{key:?} differs from a table built from scratch");
  }
}

fn without(corpus: &Corpus, removed: &[(&str, &str)]) -> Corpus {
  corpus
    .iter()
    .filter(|(facts, _)| !removed.iter().any(|(repo, path)| facts.file.repo.as_str() == *repo && facts.file.path.as_str() == *path))
    .cloned()
    .collect()
}

fn replaced(corpus: &Corpus, repo: &str, path: &str, source: &str) -> Corpus {
  corpus
    .iter()
    .map(|(facts, context)| match facts.file.repo.as_str() == repo && facts.file.path.as_str() == path {
      true => (common::analyze(&facts.file.repo, path, source), context.clone()),
      false => (facts.clone(), context.clone()),
    })
    .collect()
}

#[test]
fn should_match_a_from_scratch_table_after_upsert_and_remove_sequences() {
  let loaded = Project::load(&repos());
  let full: Corpus = loaded.repos.iter().flat_map(|source| source.files.iter().cloned()).collect();

  let mut table = table_with_packages();
  for (facts, context) in full.iter().rev() {
    table.upsert_file(facts, context).unwrap();
  }
  table.refresh();
  assert_equivalent(&table, &full);

  let removed = [
    ("acme-sdk", "src/money.ts"),
    ("acme-billing", "src/store.rs"),
    ("duplicate-names-rust", "src/b.rs"),
    ("acme-sdk", "src/index.ts"),
  ];
  for (repo, path) in removed {
    assert!(table.remove_file(&common::repo(repo), &RelPath::new(path).unwrap()).is_some());
  }
  assert!(table.remove_file(&common::repo("acme-sdk"), &RelPath::new("src/money.ts").unwrap()).is_none());
  table.refresh();
  let reduced = without(&full, &removed);
  assert_equivalent(&table, &reduced);

  let edited = replaced(
    &full,
    "duplicate-names-rust",
    "src/lib.rs",
    "pub mod a;\n#[path = \"b.rs\"]\npub mod renamed;\npub mod consumer;\n#[cfg(feature = \"ambiguous-demo\")]\npub mod ambiguous;\n",
  );
  let edited = replaced(
    &edited,
    "acme-billing",
    "src/handlers.rs",
    &std::fs::read_to_string(fixtures_root().join("acme/acme-billing/src/handlers.rs"))
      .unwrap()
      .replace("store.insert(invoice)", "store.find(\"x\"); store.insert(invoice)"),
  );
  for (facts, context) in edited.iter().filter(|(facts, _)| {
    removed.iter().any(|(repo, path)| facts.file.repo.as_str() == *repo && facts.file.path.as_str() == *path)
      || matches!(facts.file.path.as_str(), "src/lib.rs" | "src/handlers.rs")
  }) {
    table.upsert_file(facts, context).unwrap();
  }
  let relocations = table.refresh();
  assert!(
    relocations
      .iter()
      .any(|relocation| relocation.path.as_str() == "src/b.rs" && relocation.to.module_path == ["renamed"]),
    "{relocations:#?}"
  );
  assert_equivalent(&table, &edited);

  for (facts, context) in &full {
    table.upsert_file(facts, context).unwrap();
  }
  table.refresh();
  assert_equivalent(&table, &full);
}

fn changed_and_affected(
  project: &mut Project,
  repo: &str,
  path: &str,
  source: &str,
  context: KeyContext,
) -> (atlas_resolve::SurfaceDelta, std::collections::BTreeSet<(atlas_core::RepoName, RelPath)>) {
  let facts = common::analyze(&common::repo(repo), path, source);
  let delta = project.table.upsert_file(&facts, &context).unwrap();
  let relocations = project.table.refresh();
  let affected = project.table.affected_files(std::slice::from_ref(&delta), &relocations);
  (delta, affected)
}

fn key(repo: &str, path: &str) -> (atlas_core::RepoName, RelPath) {
  (common::repo(repo), RelPath::new(path).unwrap())
}

#[test]
fn should_list_dependents_when_a_new_inherent_impl_shadows_a_trait_method() {
  let manifest = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
  let mut synthetic = common::single(
    "demo",
    &[
      ("Cargo.toml", manifest),
      ("src/lib.rs", "pub mod a;\npub mod b;\npub mod c;\npub mod d;\n"),
      ("src/a.rs", "pub trait T { fn m(&self); }\npub struct S;\nimpl T for S { fn m(&self) {} }\n"),
      ("src/b.rs", "use crate::a::{S, T};\npub fn f(s: &S) { s.m(); }\n"),
      ("src/c.rs", "use crate::a::S;\n"),
      ("src/d.rs", "pub fn unrelated() {}\n"),
    ],
  );
  let project = &mut synthetic.project;
  let context = KeyContext {
    package: "demo".to_owned(),
    module_path: vec!["c".to_owned()],
  };
  let (unchanged, nothing) = changed_and_affected(project, "demo", "src/c.rs", "use crate::a::S;\n", context.clone());
  assert_eq!(unchanged.change, atlas_resolve::SurfaceChange::Unchanged);
  assert!(nothing.is_empty(), "{nothing:?}");
  let (delta, affected) = changed_and_affected(project, "demo", "src/c.rs", "use crate::a::S;\nimpl S { pub fn m(&self) {} }\n", context);
  assert_eq!(delta.change, atlas_resolve::SurfaceChange::Changed);
  assert!(delta.names.contains("m") && delta.names.contains("S"), "{:?}", delta.names);
  assert!(affected.contains(&key("demo", "src/b.rs")), "{affected:?}");
  assert!(!affected.contains(&key("demo", "src/d.rs")), "{affected:?}");
  let b = project.facts("demo", "src/b.rs").clone();
  let resolved = resolve_file(&b, &project.table).unwrap();
  let f = common::rust("demo", "demo", &["b"], &["f"], atlas_core::EntityKind::Function);
  let inherent = common::rust("demo", "demo", &["c"], &["S", "m"], atlas_core::EntityKind::Method);
  let trait_impl = common::rust("demo", "demo", &["a"], &["S", "<T>", "m"], atlas_core::EntityKind::Method);
  assert!(
    resolved
      .edges
      .iter()
      .any(|edge| edge.from == f && edge.to == inherent && edge.certainty == atlas_core::Certainty::Resolved)
  );
  assert!(!resolved.edges.iter().any(|edge| edge.from == f && edge.to == trait_impl));
}

#[test]
fn should_list_dependents_when_an_intermediate_ts_class_gains_an_override() {
  let mut synthetic = common::single(
    "app",
    &[
      ("package.json", r#"{ "name": "app", "version": "1.0.0" }"#),
      ("src/base.ts", "export class Base { m() {} }\n"),
      ("src/mid.ts", "import { Base } from './base';\nexport class Mid extends Base {}\n"),
      ("src/sub.ts", "import { Mid } from './mid';\nexport class Sub extends Mid { go() { this.m(); } }\n"),
      ("src/other.ts", "export function other() {}\n"),
    ],
  );
  let project = &mut synthetic.project;
  let context = KeyContext {
    package: "app".to_owned(),
    module_path: vec!["src".to_owned(), "mid".to_owned()],
  };
  let (delta, affected) = changed_and_affected(
    project,
    "app",
    "src/mid.ts",
    "import { Base } from './base';\nexport class Mid extends Base { m() {} }\n",
    context,
  );
  assert!(delta.names.contains("m"), "{:?}", delta.names);
  assert!(affected.contains(&key("app", "src/sub.ts")), "{affected:?}");
  assert!(!affected.contains(&key("app", "src/other.ts")), "{affected:?}");
  let sub = project.facts("app", "src/sub.ts").clone();
  let resolved = resolve_file(&sub, &project.table).unwrap();
  let go = common::ts("app", "app", &["src", "sub"], &["Sub", "go"], atlas_core::EntityKind::Method);
  let mid_m = common::ts("app", "app", &["src", "mid"], &["Mid", "m"], atlas_core::EntityKind::Method);
  let base_m = common::ts("app", "app", &["src", "base"], &["Base", "m"], atlas_core::EntityKind::Method);
  assert!(
    resolved
      .edges
      .iter()
      .any(|edge| edge.from == go && edge.to == mid_m && edge.certainty == atlas_core::Certainty::Resolved)
  );
  assert!(!resolved.edges.iter().any(|edge| edge.from == go && edge.to == base_m));
}

#[test]
fn should_list_implementers_when_an_interface_or_intermediate_class_gains_the_member() {
  let mut synthetic = common::single(
    "app",
    &[
      ("package.json", r#"{ "name": "app", "version": "1.0.0" }"#),
      ("src/shape.ts", "export interface Shape { area(): number; }\n"),
      ("src/base.ts", "export class Base { scale() {} }\n"),
      ("src/mid.ts", "import { Base } from './base';\nexport class Mid extends Base {}\n"),
      (
        "src/square.ts",
        "import { Shape } from './shape';\nimport { Mid } from './mid';\nexport class Square extends Mid implements Shape {\n  area() { return 1; }\n  scale() {}\n  side() { return 1; }\n}\n",
      ),
      ("src/other.ts", "export function other() {}\n"),
    ],
  );
  let project = &mut synthetic.project;
  let method = |module: &str, items: &[&str]| common::ts("app", "app", &["src", module], items, atlas_core::EntityKind::Method);
  let implements_from = |resolved: &ResolvedFile, from: atlas_core::EntityId| -> Vec<atlas_core::EntityId> {
    let mut targets: Vec<_> = resolved
      .structural
      .iter()
      .filter(|edge| edge.kind == atlas_core::EdgeKind::Implements && edge.from == from)
      .map(|edge| edge.to)
      .collect();
    targets.sort();
    targets
  };
  let context = |module: &str| KeyContext {
    package: "app".to_owned(),
    module_path: vec!["src".to_owned(), module.to_owned()],
  };
  let (_, affected) = changed_and_affected(
    project,
    "app",
    "src/shape.ts",
    "export interface Shape { area(): number; side(): number; }\n",
    context("shape"),
  );
  assert!(affected.contains(&key("app", "src/square.ts")), "{affected:?}");
  assert!(!affected.contains(&key("app", "src/other.ts")), "{affected:?}");
  let (_, affected) = changed_and_affected(
    project,
    "app",
    "src/mid.ts",
    "import { Base } from './base';\nexport class Mid extends Base { scale() {} }\n",
    context("mid"),
  );
  assert!(affected.contains(&key("app", "src/square.ts")), "{affected:?}");
  let square = project.facts("app", "src/square.ts").clone();
  let resolved = resolve_file(&square, &project.table).unwrap();
  assert_eq!(implements_from(&resolved, method("square", &["Square", "area"])), [method("shape", &["Shape", "area"])]);
  assert_eq!(implements_from(&resolved, method("square", &["Square", "side"])), [method("shape", &["Shape", "side"])]);
  assert_eq!(implements_from(&resolved, method("square", &["Square", "scale"])), [method("mid", &["Mid", "scale"])]);
}

#[test]
fn should_list_the_package_and_its_dependents_for_added_and_removed_files() {
  let mut synthetic = common::single(
    "app",
    &[
      ("package.json", r#"{ "name": "app", "version": "1.0.0" }"#),
      ("src/a.ts", "import { later } from './later';\nexport function a() { later(); }\n"),
      ("src/b.ts", "export function b() {}\n"),
    ],
  );
  let project = &mut synthetic.project;
  let context = KeyContext {
    package: "app".to_owned(),
    module_path: vec!["src".to_owned(), "later".to_owned()],
  };
  let (delta, affected) = changed_and_affected(project, "app", "src/later.ts", "export function later() {}\n", context);
  assert_eq!(delta.change, atlas_resolve::SurfaceChange::Added);
  assert!(affected.contains(&key("app", "src/a.ts")) && affected.contains(&key("app", "src/b.ts")), "{affected:?}");
  let removed = project.table.remove_file(&common::repo("app"), &RelPath::new("src/later.ts").unwrap()).unwrap();
  assert_eq!(removed.change, atlas_resolve::SurfaceChange::Removed);
  let relocations = project.table.refresh();
  let affected = project.table.affected_files(&[removed], &relocations);
  assert!(affected.contains(&key("app", "src/a.ts")), "{affected:?}");
}
