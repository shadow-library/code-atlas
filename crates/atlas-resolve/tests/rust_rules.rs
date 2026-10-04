mod common;

use atlas_core::{Certainty, CertaintyClass, Confidence, EdgeKind, EntityKind, KeyContext, RefKind, RefOrigin, UnresolvedReason};
use atlas_resolve::{ProjectSymbolTable, Resolution, resolve_file, resolve_path_in_file};
use common::{rust, single, synthetic};

const RESOLVED: &[CertaintyClass] = &[CertaintyClass::Resolved];
const HEURISTIC: &[CertaintyClass] = &[CertaintyClass::Heuristic];

fn manifest(name: &str, dependencies: &str) -> String {
  format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dependencies}")
}

fn item(module: &[&str], items: &[&str], kind: EntityKind) -> atlas_core::EntityId {
  rust("demo", "demo", module, items, kind)
}

#[test]
fn should_follow_pub_use_chains_and_stop_on_cycles() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      (
        "src/lib.rs",
        "pub mod a;\npub mod b;\npub mod c;\npub mod x;\npub mod y;\npub fn go() {\n  a::Thing::new();\n  crate::a::Alias::new();\n  x::Loop::new();\n}\n",
      ),
      ("src/a.rs", "pub use crate::b::Thing;\npub use self::Thing as Alias;\n"),
      ("src/b.rs", "pub use super::c::Original as Thing;\n"),
      ("src/c.rs", "pub struct Original;\nimpl Original {\n  pub fn new() -> Self { Original }\n}\n"),
      ("src/x.rs", "pub use crate::y::Loop;\n"),
      ("src/y.rs", "pub use crate::x::Loop;\n"),
    ],
  );
  let project = &synthetic.project;
  let go = item(&[], &["go"], EntityKind::Function);
  let new = project.assert_edge(EdgeKind::Calls, go, item(&["c"], &["Original", "new"], EntityKind::Method), RESOLVED);
  assert_eq!(new.evidence.len(), 2);
  project.assert_unresolved(go, "new", &UnresolvedReason::NotFound);
  project.assert_edge(
    EdgeKind::ReExports,
    common::file("demo", "src/b.rs"),
    item(&["c"], &["Original"], EntityKind::Struct),
    RESOLVED,
  );
}

#[test]
fn should_resolve_self_paths_fields_and_deref_wrappers() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      (
        "src/lib.rs",
        r#"use std::sync::Arc;

pub struct Engine;
impl Engine {
  pub fn start(&self) {}
}

pub trait Trait {
  fn provided(&self) {}
  fn required(&self);
}

pub struct Holder {
  boxed: Box<Engine>,
  shared: Arc<Engine>,
  plain: Engine,
}

impl Trait for Holder {
  fn required(&self) {
    self.provided();
  }
}

impl Holder {
  pub fn make() -> Self {
    Self::helper();
    Holder { boxed: Box::new(Engine), shared: Arc::new(Engine), plain: Engine }
  }
  fn helper() {}
  pub fn run(&self) {
    self.boxed.start();
    self.shared.start();
    self.plain.start();
    self.shared.clone();
    self.required();
  }
}

pub fn outside(holder: &Holder, engine: Box<Engine>) {
  holder.run();
  engine.start();
  holder.provided();
}
"#,
      ),
    ],
  );
  let project = &synthetic.project;
  let start = item(&[], &["Engine", "start"], EntityKind::Method);
  let run = item(&[], &["Holder", "run"], EntityKind::Method);
  let edge = project.assert_edge(EdgeKind::Calls, run, start, RESOLVED);
  assert_eq!(edge.evidence.len(), 3);
  project.assert_unresolved(run, "clone", &UnresolvedReason::External { package: "std".to_owned() });
  project.assert_edge(EdgeKind::Calls, run, item(&[], &["Holder", "<Trait>", "required"], EntityKind::Method), RESOLVED);
  project.assert_edge(
    EdgeKind::Calls,
    item(&[], &["Holder", "make"], EntityKind::Method),
    item(&[], &["Holder", "helper"], EntityKind::Method),
    RESOLVED,
  );
  project.assert_edge(
    EdgeKind::Calls,
    item(&[], &["Holder", "<Trait>", "required"], EntityKind::Method),
    item(&[], &["Trait", "provided"], EntityKind::Method),
    RESOLVED,
  );
  let outside = item(&[], &["outside"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, outside, run, RESOLVED);
  let recovered = project.assert_edge(EdgeKind::Calls, outside, start, HEURISTIC);
  assert_eq!(recovered.certainty, Certainty::heuristic(Confidence::new(0.7).unwrap(), "param-type-recovery"));
  project.assert_edge(EdgeKind::Calls, outside, item(&[], &["Trait", "provided"], EntityKind::Method), RESOLVED);
  project.assert_edge(EdgeKind::Contains, item(&[], &["Holder"], EntityKind::Struct), run, RESOLVED);
}

#[test]
fn should_bound_name_only_candidates_for_unknown_receivers() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      (
        "src/lib.rs",
        r#"pub struct A;
impl A { pub fn unique_name(&self) {} pub fn common(&self) {} }
pub struct B;
impl B { pub fn common(&self) {} }
pub struct C;
impl C { pub fn common(&self) {} }
pub struct D;
impl D { pub fn common(&self) {} }
pub fn caller(values: Vec<A>) {
  let first = values.into_iter().next();
  first.unique_name();
  first.common();
  first.missing();
}
"#,
      ),
    ],
  );
  let project = &synthetic.project;
  let caller = item(&[], &["caller"], EntityKind::Function);
  let edge = project.assert_edge(EdgeKind::Calls, caller, item(&[], &["A", "unique_name"], EntityKind::Method), HEURISTIC);
  assert_eq!(edge.certainty, Certainty::heuristic(Confidence::new(0.5).unwrap(), "method-name-only"));
  let common_methods: Vec<_> = ["A", "B", "C", "D"].iter().map(|owner| item(&[], &[owner, "common"], EntityKind::Method)).collect();
  let mut candidates = common_methods.clone();
  candidates.sort();
  project.assert_unresolved(caller, "common", &UnresolvedReason::Ambiguous { candidates });
  for method in common_methods {
    project.assert_no_edge(EdgeKind::Calls, caller, method);
  }
  project.assert_unresolved(caller, "missing", &UnresolvedReason::NotFound);
}

#[test]
fn should_resolve_in_project_crates_across_repos_and_mark_others_external() {
  let app_manifest = manifest("app", "corelib = { path = \"../corelib\", package = \"core-lib\" }\nserde_json = \"1\"\n");
  let core_manifest = manifest("core-lib", "");
  let synthetic = synthetic(&[
    (
      "app",
      &[
        ("Cargo.toml", app_manifest.as_str()),
        (
          "src/lib.rs",
          "use corelib::parse;\npub fn go() {\n  parse();\n  corelib::nested::deep();\n  serde_json::to_string(&1);\n  std::mem::drop(1);\n}\n",
        ),
      ],
    ),
    (
      "corelib",
      &[
        ("Cargo.toml", core_manifest.as_str()),
        ("src/lib.rs", "pub mod nested;\npub fn parse() {}\nfn private() {}\n"),
        ("src/nested.rs", "pub fn deep() {}\n"),
      ],
    ),
  ]);
  let project = &synthetic.project;
  let go = rust("app", "app", &[], &["go"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, go, rust("corelib", "core-lib", &[], &["parse"], EntityKind::Function), RESOLVED);
  project.assert_edge(EdgeKind::Calls, go, rust("corelib", "core-lib", &["nested"], &["deep"], EntityKind::Function), RESOLVED);
  project.assert_unresolved(go, "to_string", &UnresolvedReason::External { package: "serde_json".to_owned() });
  project.assert_unresolved(go, "drop", &UnresolvedReason::External { package: "std".to_owned() });
  project.assert_edge(
    EdgeKind::Imports,
    common::file("app", "src/lib.rs"),
    rust("corelib", "core-lib", &[], &["parse"], EntityKind::Function),
    RESOLVED,
  );
}

#[test]
fn should_hide_private_items_from_other_crates_and_globs() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      ("src/lib.rs", "mod inner;\nmod other;\n"),
      (
        "src/inner.rs",
        "fn secret() {}\npub fn open() {}\n#[cfg(test)]\nmod tests {\n  use super::*;\n  fn t() { secret(); open(); }\n}\n",
      ),
      ("src/other.rs", "use crate::inner::*;\nfn t() { secret(); open(); }\n"),
    ],
  );
  let project = &synthetic.project;
  let child = item(&["inner", "tests"], &["t"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, child, item(&["inner"], &["secret"], EntityKind::Function), RESOLVED);
  project.assert_edge(EdgeKind::Calls, child, item(&["inner"], &["open"], EntityKind::Function), RESOLVED);
  let sibling = item(&["other"], &["t"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, sibling, item(&["inner"], &["open"], EntityKind::Function), RESOLVED);
  project.assert_no_edge(EdgeKind::Calls, sibling, item(&["inner"], &["secret"], EntityKind::Function));
  project.assert_unresolved(sibling, "secret", &UnresolvedReason::NotFound);
}

#[test]
fn should_cap_macro_token_scan_references_at_heuristic() {
  let repo = common::repo("demo");
  let manifest = manifest("demo", "");
  let dir = common::write_repo(&[("Cargo.toml", &manifest), ("src/lib.rs", "")]);
  let mut project = common::Project::load(&[("demo", dir.path().to_path_buf())]);
  let mut facts = common::analyze(&repo, "src/lib.rs", "pub fn helper() {}\npub fn caller() { helper(); }\n");
  facts
    .references
    .iter_mut()
    .filter(|reference| reference.kind == RefKind::Call)
    .for_each(|reference| reference.origin = RefOrigin::MacroTokenScan);
  let context = KeyContext {
    package: "demo".to_owned(),
    module_path: Vec::new(),
  };
  project.table.upsert_file(&facts, &context).unwrap();
  project.table.refresh();
  let resolved = resolve_file(&facts, &project.table).unwrap();
  let helper = item(&[], &["helper"], EntityKind::Function);
  let edge = resolved.edges.iter().find(|edge| edge.to == helper).unwrap();
  assert_eq!(edge.certainty, Certainty::heuristic(Confidence::new(0.6).unwrap(), "macro-token-scan"));
  assert_eq!(resolved.stats.by_kind[&RefKind::Call].heuristic, 1);
}

#[test]
fn should_resolve_symbolic_paths_for_detectors() {
  let synthetic = common::Project::load(&[("acme-billing", common::fixtures_root().join("acme/acme-billing"))]);
  let facts = synthetic.facts("acme-billing", "src/lib.rs");
  let router = facts.symbols.iter().find(|symbol| symbol.name == "router").unwrap().local;
  let resolution = resolve_path_in_file(facts, &["create_invoice".to_owned()], Some(router), &synthetic.table).unwrap();
  let expected = rust("acme-billing", "acme-billing", &["handlers"], &["create_invoice"], EntityKind::Function);
  assert_eq!(resolution, Resolution::Resolved(expected, Certainty::Resolved));
  let external = resolve_path_in_file(facts, &["axum".to_owned(), "Router".to_owned()], None, &synthetic.table).unwrap();
  assert_eq!(external, Resolution::Unresolved(UnresolvedReason::External { package: "axum".to_owned() }));
}

#[test]
fn should_refuse_to_resolve_against_a_stale_table_or_file() {
  let repo = common::repo("demo");
  let mut table = ProjectSymbolTable::new();
  let facts = common::analyze(&repo, "src/lib.rs", "pub fn f() {}\n");
  assert_eq!(
    resolve_file(&facts, &table),
    Err(atlas_resolve::ResolveError::NotIndexed {
      repo: repo.clone(),
      path: facts.file.path.clone(),
    })
  );
  let context = KeyContext {
    package: "demo".to_owned(),
    module_path: Vec::new(),
  };
  table.upsert_file(&facts, &context).unwrap();
  assert!(resolve_file(&facts, &table).is_ok());
  let changed = common::analyze(&repo, "src/lib.rs", "pub fn g() {}\n");
  assert_eq!(
    resolve_file(&changed, &table),
    Err(atlas_resolve::ResolveError::StaleFile {
      repo,
      path: changed.file.path.clone(),
    })
  );
}
