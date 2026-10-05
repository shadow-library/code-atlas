mod common;

use atlas_core::{Certainty, CertaintyClass, Confidence, EdgeKind, EntityId, EntityKind, UnresolvedReason};
use common::{Synthetic, rust, single, synthetic};

const RESOLVED: &[CertaintyClass] = &[CertaintyClass::Resolved];

fn manifest(name: &str, dependencies: &str) -> String {
  format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dependencies}")
}

fn item(module: &[&str], items: &[&str], kind: EntityKind) -> EntityId {
  rust("demo", "demo", module, items, kind)
}

fn method(module: &[&str], items: &[&str]) -> EntityId {
  item(module, items, EntityKind::Method)
}

fn heuristic(confidence: f32, rule: &str) -> Certainty {
  Certainty::heuristic(Confidence::new(confidence).unwrap(), rule)
}

const STORE: &str = r#"pub type StoreResult<T> = Result<T, StoreError>;

pub struct StoreError;

pub struct Entity {
  pub name: String,
}

impl Entity {
  pub fn label(&self) -> &str {
    &self.name
  }
}

pub struct ReadTx;

impl ReadTx {
  pub fn entity_by_key(&self, key: &str) -> StoreResult<Option<Entity>> {
    Ok(None)
  }
}

pub struct BulkWrites<'a> {
  store: &'a Store,
}

impl BulkWrites<'_> {
  pub fn commit(&self) {}
}

pub struct Store {
  tx: ReadTx,
}

impl Store {
  pub fn open() -> StoreResult<Self> {
    Ok(Store { tx: ReadTx })
  }
  pub fn tx(&self) -> &ReadTx {
    &self.tx
  }
  pub fn bulk_writes(&self) -> StoreResult<BulkWrites<'_>> {
    Ok(BulkWrites { store: self })
  }
  pub fn maybe(&self) -> Option<Entity> {
    None
  }
  pub async fn load(&self) -> Entity {
    Entity { name: String::new() }
  }
  pub fn later(&self) -> impl std::future::Future<Output = Entity> {
    async { Entity { name: String::new() } }
  }
  pub fn with_name(mut self, name: &str) -> Self {
    self
  }
}
"#;

const GRAPH: &str = r#"use crate::store::{ReadTx, Store, StoreResult};

pub struct Graph<'a> {
  store: &'a Store,
}

impl<'a> Graph<'a> {
  pub fn tx(&self) -> &'a ReadTx {
    self.store.tx()
  }
}

pub fn query(graph: &Graph<'_>) -> StoreResult<()> {
  graph.tx().entity_by_key("k")?;
  graph.tx().entity_by_key("k")?.unwrap().label();
  Ok(())
}

pub fn opened() {
  Store::open().unwrap().tx().entity_by_key("k");
  Store::open().unwrap().with_name("x").bulk_writes().unwrap().commit();
}

pub async fn awaited(store: &Store) {
  store.load().await.label();
  store.load().label();
  store.later().await.label();
  store.maybe().map(|entity| entity.name.len());
}
"#;

const BUDGET: &str = r#"use std::sync::{Mutex, MutexGuard};

pub struct Budget;

impl Budget {
  pub fn try_reserve(&mut self, estimate: u64) -> bool {
    true
  }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
  mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn spend(budget: &Mutex<Budget>) {
  lock(budget).try_reserve(1);
  let local = Mutex::new(Budget);
  lock(&local).try_reserve(2);
}
"#;

fn chain_project() -> Synthetic {
  let manifest = manifest("demo", "");
  single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      ("src/lib.rs", "pub mod budget;\npub mod graph;\npub mod store;\n"),
      ("src/store.rs", STORE),
      ("src/graph.rs", GRAPH),
      ("src/budget.rs", BUDGET),
    ],
  )
}

#[test]
fn should_resolve_calls_through_declared_return_types() {
  let synthetic = chain_project();
  let project = &synthetic.project;
  let query = item(&["graph"], &["query"], EntityKind::Function);
  let entity_by_key = method(&["store"], &["ReadTx", "entity_by_key"]);
  let edge = project.assert_edge(EdgeKind::Calls, query, entity_by_key, RESOLVED);
  assert_eq!(edge.evidence.len(), 2);
  project.assert_certainty(EdgeKind::Calls, query, method(&["store"], &["Entity", "label"]), &heuristic(0.8, "unwrap:Result"));
  project.assert_unresolved(query, "unwrap", &UnresolvedReason::External { package: "std".to_owned() });
  let tx = method(&["graph"], &["Graph", "tx"]);
  project.assert_edge(EdgeKind::Calls, method(&["graph"], &["Graph", "tx"]), method(&["store"], &["Store", "tx"]), RESOLVED);
  project.assert_edge(EdgeKind::Calls, query, tx, RESOLVED);
}

#[test]
fn should_weaken_path_call_heads_to_the_constructor_signature_rule() {
  let synthetic = chain_project();
  let project = &synthetic.project;
  let opened = item(&["graph"], &["opened"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, opened, method(&["store"], &["Store", "open"]), RESOLVED);
  project.assert_certainty(EdgeKind::Calls, opened, method(&["store"], &["Store", "with_name"]), &heuristic(0.8, "unwrap:Result"));
  project.assert_certainty(EdgeKind::Calls, opened, method(&["store"], &["Store", "bulk_writes"]), &heuristic(0.8, "unwrap:Result"));
  project.assert_certainty(EdgeKind::Calls, opened, method(&["store"], &["BulkWrites", "commit"]), &heuristic(0.8, "unwrap:Result"));
  project.assert_certainty(EdgeKind::Calls, opened, method(&["store"], &["ReadTx", "entity_by_key"]), &heuristic(0.8, "unwrap:Result"));
}

#[test]
fn should_await_only_async_fns_and_attribute_future_and_option_methods_to_std() {
  let synthetic = chain_project();
  let project = &synthetic.project;
  let awaited = item(&["graph"], &["awaited"], EntityKind::Function);
  let label = method(&["store"], &["Entity", "label"]);
  let edge = project.assert_edge(EdgeKind::Calls, awaited, label, RESOLVED);
  assert_eq!(edge.evidence.len(), 1);
  let unresolved: Vec<_> = project
    .unresolved_named(awaited, "label")
    .into_iter()
    .map(|unresolved| (unresolved.path[0].clone(), unresolved.reason.clone()))
    .collect();
  assert_eq!(
    unresolved,
    [
      ("(store.load())".to_owned(), UnresolvedReason::External { package: "std".to_owned() }),
      ("(store.later().await)".to_owned(), UnresolvedReason::Dynamic),
    ]
  );
  project.assert_unresolved(awaited, "map", &UnresolvedReason::External { package: "std".to_owned() });
}

#[test]
fn should_bind_a_generic_return_type_from_a_declared_argument_and_deref_the_guard() {
  let synthetic = chain_project();
  let project = &synthetic.project;
  let spend = item(&["budget"], &["spend"], EntityKind::Function);
  let try_reserve = method(&["budget"], &["Budget", "try_reserve"]);
  let edge = project.assert_edge(EdgeKind::Calls, spend, try_reserve, &[CertaintyClass::Heuristic]);
  assert_eq!(edge.certainty, heuristic(0.8, "deref:MutexGuard"));
  assert_eq!(edge.evidence.len(), 1);
  let local: Vec<_> = project
    .unresolved_named(spend, "try_reserve")
    .into_iter()
    .map(|unresolved| unresolved.reason.clone())
    .collect();
  assert_eq!(local, [UnresolvedReason::Dynamic]);
}

const SHAPES: &str = r#"pub trait Shape {
  type Output;
  fn area(&self) -> f64;
  fn output(&self) -> Self::Output;
  fn scaled(&self) -> Self where Self: Sized;
  fn report(&self) -> Report;
}

pub trait Builder {
  fn finish(self) -> Self where Self: Sized {
    self
  }
}

pub struct Report;

impl Report {
  pub fn print(&self) {}
}

pub struct Circle;

impl Circle {
  pub fn run(&self) {}
}

impl Builder for Circle {}

impl Shape for Circle {
  type Output = Circle;
  fn area(&self) -> f64 {
    1.0
  }
  fn output(&self) -> Circle {
    Circle
  }
  fn scaled(&self) -> Self {
    Circle
  }
  fn report(&self) -> Report {
    Report
  }
}

pub struct Square;

impl Square {
  pub fn run(&self) {}
}

pub struct Left {
  pub inner: Circle,
}

pub struct Right {
  pub inner: Square,
}

pub struct Wrapper(pub Circle);

impl Wrapper {
  pub fn unwrap_circle(&self) -> &Circle {
    &self.0
  }
}

pub type Handle = Holder;

pub struct Holder;

impl Holder {
  pub fn shape(&self) -> Box<dyn Shape<Output = Circle>> {
    Box::new(Circle)
  }
  pub fn get<T: Default>(&self) -> T {
    T::default()
  }
  pub fn json(&self) -> serde_json::Value {
    serde_json::Value::Null
  }
  pub fn left(&mut self) -> &mut Left {
    todo!()
  }
}

pub fn handle() -> Handle {
  Holder
}

pub fn chains(holder: &mut Holder, circle: Circle) {
  holder.shape().area();
  holder.shape().output().run();
  holder.shape().scaled().run();
  holder.shape().report().print();
  holder.get().run();
  holder.json().get("x").run();
  missing().inner.run();
  circle.finish().run();
  Wrapper(Circle).unwrap_circle().run();
  handle().left().inner.run();
}

pub fn outer() {
  fn nested(holder: &Holder) {
    holder.shape().report().print();
  }
}
"#;

fn shapes_project() -> Synthetic {
  let manifest = manifest("demo", "serde_json = \"1\"\n");
  single("demo", &[("Cargo.toml", &manifest), ("src/lib.rs", SHAPES)])
}

#[test]
fn should_follow_dispatch_rules_on_trait_object_hops_and_never_guess_through_unknown_hops() {
  let synthetic = shapes_project();
  let project = &synthetic.project;
  let chains = item(&[], &["chains"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, chains, method(&[], &["Shape", "area"]), RESOLVED);
  project.assert_certainty(
    EdgeKind::Calls,
    chains,
    method(&[], &["Circle", "<Shape>", "area"]),
    &heuristic(0.3, "dyn-dispatch-candidate"),
  );
  project.assert_edge(EdgeKind::Calls, chains, method(&[], &["Report", "print"]), RESOLVED);
  project.assert_edge(EdgeKind::Calls, chains, method(&[], &["Shape", "output"]), RESOLVED);
  let runs: Vec<_> = project
    .unresolved_named(chains, "run")
    .into_iter()
    .map(|unresolved| (unresolved.path[0].clone(), unresolved.reason.clone()))
    .collect();
  assert_eq!(
    runs,
    [
      ("(holder.shape().output())".to_owned(), UnresolvedReason::Dynamic),
      ("(holder.shape().scaled())".to_owned(), UnresolvedReason::Dynamic),
      ("(holder.get())".to_owned(), UnresolvedReason::Dynamic),
      ("(holder.json().get(\"x\"))".to_owned(), UnresolvedReason::Dynamic),
      ("(missing().inner)".to_owned(), UnresolvedReason::Dynamic),
    ]
  );
  project.assert_no_edge(EdgeKind::Calls, chains, method(&[], &["Square", "run"]));
}

#[test]
fn should_resolve_self_returning_defaults_tuple_constructors_aliases_and_mut_references() {
  let synthetic = shapes_project();
  let project = &synthetic.project;
  let chains = item(&[], &["chains"], EntityKind::Function);
  let edge = project.assert_edge(EdgeKind::Calls, chains, method(&[], &["Circle", "run"]), RESOLVED);
  assert_eq!(edge.evidence.len(), 3);
  project.assert_edge(EdgeKind::Calls, chains, method(&[], &["Wrapper", "unwrap_circle"]), RESOLVED);
  project.assert_edge(EdgeKind::Calls, chains, method(&[], &["Holder", "left"]), RESOLVED);
}

#[test]
fn should_leave_chains_inside_nested_items_unresolved() {
  let synthetic = shapes_project();
  let project = &synthetic.project;
  let outer = item(&[], &["outer"], EntityKind::Function);
  project.assert_no_edge(EdgeKind::Calls, outer, method(&[], &["Report", "print"]));
  project.assert_unresolved(outer, "print", &UnresolvedReason::Dynamic);
}

#[test]
fn should_let_extern_crates_win_over_external_glob_guesses() {
  let app_manifest = manifest("app", "corelib = { path = \"../corelib\", package = \"core-lib\" }\nrayon = \"1\"\n");
  let core_manifest = manifest("core-lib", "");
  let synthetic = synthetic(&[
    (
      "app",
      &[
        ("Cargo.toml", app_manifest.as_str()),
        (
          "src/lib.rs",
          "use corelib::Table;\nuse rayon::prelude::*;\n\npub struct State {\n  pub table: Table,\n}\n\npub struct Run<'a> {\n  state: &'a mut State,\n}\n\nimpl Run<'_> {\n  pub fn go(&mut self) {\n    self.state.table.remove_file();\n    corelib::Table::default().remove_file();\n    unknown_name();\n  }\n}\n",
        ),
      ],
    ),
    (
      "corelib",
      &[
        ("Cargo.toml", core_manifest.as_str()),
        (
          "src/lib.rs",
          "#[derive(Default)]\npub struct Table;\n\nimpl Table {\n  pub fn remove_file(&mut self) {}\n}\n",
        ),
      ],
    ),
  ]);
  let project = &synthetic.project;
  let go = rust("app", "app", &[], &["Run", "go"], EntityKind::Method);
  let remove_file = rust("corelib", "core-lib", &[], &["Table", "remove_file"], EntityKind::Method);
  let edge = project.assert_edge(EdgeKind::Calls, go, remove_file, RESOLVED);
  assert_eq!(edge.evidence.len(), 1);
  project.assert_unresolved(go, "unknown_name", &UnresolvedReason::External { package: "rayon".to_owned() });
  project.assert_edge(
    EdgeKind::Imports,
    common::file("app", "src/lib.rs"),
    rust("corelib", "core-lib", &[], &["Table"], EntityKind::Struct),
    RESOLVED,
  );
}

#[test]
fn should_cap_chains_found_by_the_macro_token_scan() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      ("src/lib.rs", "pub mod budget;\npub mod graph;\npub mod store;\npub mod scan;\n"),
      ("src/store.rs", STORE),
      ("src/graph.rs", GRAPH),
      ("src/budget.rs", BUDGET),
      (
        "src/scan.rs",
        "use crate::graph::Graph;\n\nmacro_rules! probe {\n  ($($t:tt)*) => {};\n}\n\npub fn scanned(graph: &Graph<'_>) {\n  probe!(target: graph.tx().entity_by_key(\"k\"), mode fast);\n}\n",
      ),
    ],
  );
  let project = &synthetic.project;
  let scanned = item(&["scan"], &["scanned"], EntityKind::Function);
  project.assert_certainty(
    EdgeKind::Calls,
    scanned,
    method(&["store"], &["ReadTx", "entity_by_key"]),
    &heuristic(0.6, "macro-token-scan"),
  );
}

#[test]
fn should_not_keep_the_wrapped_type_through_borrow_methods_the_inner_type_defines() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      (
        "src/lib.rs",
        "use std::rc::Rc;\n\npub struct B;\n\nimpl B {\n  pub fn go(&self) {}\n}\n\npub struct Borrowy;\n\nimpl Borrowy {\n  pub fn borrow(&self) -> B {\n    B\n  }\n  pub fn go(&self) {}\n}\n\npub struct Holder {\n  pub shared: Rc<Borrowy>,\n  pub boxed: Box<Borrowy>,\n}\n\nimpl Holder {\n  pub fn me(&self) -> &Holder {\n    self\n  }\n}\n\npub fn run(h: &Holder) {\n  h.me().shared.borrow().go();\n  h.me().boxed.as_mut().go();\n  h.me().shared.clone().go();\n}\n",
      ),
    ],
  );
  let project = &synthetic.project;
  let run = item(&[], &["run"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, run, method(&[], &["B", "go"]), RESOLVED);
  let edge = project.assert_edge(EdgeKind::Calls, run, method(&[], &["Borrowy", "go"]), RESOLVED);
  assert_eq!(edge.evidence.len(), 1);
  let unresolved = project.unresolved_named(run, "go");
  assert_eq!(unresolved.len(), 1);
  assert_eq!(unresolved[0].reason, UnresolvedReason::Dynamic);
  assert_eq!(unresolved[0].receiver, Some(atlas_core::ReceiverHint::Expr("h.me().boxed.as_mut()".to_owned())));
}

#[test]
fn should_apply_the_constructor_rule_to_qualified_path_heads() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      (
        "src/lib.rs",
        "pub trait Tr {\n  fn make() -> Self;\n}\n\npub struct A;\n\nimpl A {\n  pub fn go(&self) {}\n}\n\nimpl Tr for A {\n  fn make() -> Self {\n    A\n  }\n}\n\npub fn qualified() {\n  <A as Tr>::make().go();\n}\n\npub fn plain() {\n  A::make().go();\n}\n",
      ),
    ],
  );
  let project = &synthetic.project;
  let go = method(&[], &["A", "go"]);
  for caller in ["qualified", "plain"] {
    project.assert_certainty(EdgeKind::Calls, item(&[], &[caller], EntityKind::Function), go, &heuristic(0.9, "constructor-signature"));
  }
}

#[test]
fn should_let_root_extern_crate_aliases_win_over_external_glob_guesses() {
  let app_manifest = manifest("app", "core_b = { path = \"../core_b\" }\nrayon = \"1\"\n");
  let core_manifest = manifest("core_b", "");
  let synthetic = synthetic(&[
    (
      "app",
      &[
        ("Cargo.toml", app_manifest.as_str()),
        ("src/lib.rs", "extern crate core_b as cb;\n\npub mod user;\n"),
        ("src/user.rs", "use rayon::prelude::*;\n\npub fn go(decoy: cb::model::Decoy) {\n  decoy.go();\n}\n"),
      ],
    ),
    (
      "core_b",
      &[
        ("Cargo.toml", core_manifest.as_str()),
        ("src/lib.rs", "pub mod model;\n"),
        ("src/model.rs", "pub struct Decoy;\n\nimpl Decoy {\n  pub fn go(&self) {}\n}\n"),
      ],
    ),
  ]);
  let project = &synthetic.project;
  let go = rust("app", "app", &["user"], &["go"], EntityKind::Function);
  project.assert_edge(EdgeKind::References, go, rust("core_b", "core_b", &["model"], &["Decoy"], EntityKind::Struct), RESOLVED);
  project.assert_edge(EdgeKind::Calls, go, rust("core_b", "core_b", &["model"], &["Decoy", "go"], EntityKind::Method), RESOLVED);
}

#[test]
fn should_leave_borrow_hops_unknown_whenever_the_borrow_trait_may_be_in_scope() {
  let manifest = manifest("demo", "rayon = \"1\"\n");
  let model = "use std::rc::Rc;\n\npub struct B;\n\nimpl B {\n  pub fn go(&self) {}\n}\n\npub struct Bw;\n\nimpl Bw {\n  pub fn borrow(&self) -> B {\n    B\n  }\n  pub fn borrow_mut(&self) -> B {\n    B\n  }\n}\n\npub struct H {\n  pub shared: Rc<Bw>,\n}\n\nimpl H {\n  pub fn me(&self) -> &H {\n    self\n  }\n}\n";
  let caller = |imports: &str| format!("{imports}use crate::model::H;\n\npub fn run(h: &H) {{\n  h.me().shared.borrow().go();\n  h.me().shared.borrow_mut().go();\n}}\n");
  let files = [
    ("plain", caller("")),
    ("anonymous", caller("use std::borrow::{Borrow as _, BorrowMut as _};\n")),
    ("renamed", caller("use core::borrow::Borrow as Bo;\nuse alloc::borrow::BorrowMut as Bm;\n")),
    ("project_glob", caller("use crate::prelude::*;\n")),
    ("external_glob", caller("use rayon::prelude::*;\n")),
  ];
  let mut sources: Vec<(String, String)> = vec![
    ("Cargo.toml".to_owned(), manifest),
    ("src/model.rs".to_owned(), model.to_owned()),
    ("src/prelude.rs".to_owned(), "pub use std::borrow::{Borrow, BorrowMut};\n".to_owned()),
    (
      "src/lib.rs".to_owned(),
      "pub mod model;\npub mod prelude;\npub mod plain;\npub mod anonymous;\npub mod renamed;\npub mod project_glob;\npub mod external_glob;\n".to_owned(),
    ),
  ];
  sources.extend(files.iter().map(|(module, source)| (format!("src/{module}.rs"), source.clone())));
  let borrowed: Vec<(&str, &str)> = sources.iter().map(|(path, source)| (path.as_str(), source.as_str())).collect();
  let synthetic = single("demo", &borrowed);
  let project = &synthetic.project;
  let go = method(&["model"], &["B", "go"]);
  for (module, _) in &files {
    let run = item(&[module], &["run"], EntityKind::Function);
    match *module {
      "plain" => {
        let edge = project.assert_edge(EdgeKind::Calls, run, go, RESOLVED);
        assert_eq!(edge.evidence.len(), 2, "{module}");
      }
      _ => {
        project.assert_no_edge(EdgeKind::Calls, run, go);
        let reasons: Vec<_> = project.unresolved_named(run, "go").into_iter().map(|unresolved| unresolved.reason.clone()).collect();
        assert_eq!(reasons, [UnresolvedReason::Dynamic, UnresolvedReason::Dynamic], "{module}");
      }
    }
  }
}
