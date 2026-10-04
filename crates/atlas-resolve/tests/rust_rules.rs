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
  project.assert_edge(EdgeKind::Calls, outside, start, RESOLVED);
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

fn type_anchor_project() -> common::Synthetic {
  let manifest = manifest("demo", "");
  single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      (
        "src/lib.rs",
        r#"pub mod store;
pub struct Foo;
pub struct T;
pub struct Config;
pub trait Shape { fn area(&self) -> u32; }

pub fn uses_derived() { store::InMemory::default(); }
pub fn uses_std_trait() { Config::from_str("x"); }
pub fn uses_turbofish() { Vec::<Foo>::new(); }
pub fn uses_generic<T: Default>() { T::default(); }
pub fn uses_qualified() { <Config as Default>::default(); }
pub fn uses_project_method() { store::InMemory::make(); }
pub fn uses_const() { Config::LIMIT; }
pub fn uses_trait_external() { Shape::describe(); }
"#,
      ),
      (
        "src/store.rs",
        "#[derive(Default)]\npub struct InMemory;\nimpl InMemory {\n  pub fn make() -> Self { InMemory }\n}\n",
      ),
    ],
  )
}

#[test]
fn should_keep_the_type_edge_when_a_derived_method_is_external() {
  let synthetic = type_anchor_project();
  let project = &synthetic.project;
  let caller = item(&[], &["uses_derived"], EntityKind::Function);
  project.assert_edge(EdgeKind::References, caller, item(&["store"], &["InMemory"], EntityKind::Struct), RESOLVED);
  project.assert_unresolved(caller, "default", &UnresolvedReason::External { package: "std".to_owned() });
}

#[test]
fn should_keep_the_type_edge_when_a_std_trait_method_is_not_a_project_method() {
  let synthetic = type_anchor_project();
  let project = &synthetic.project;
  let caller = item(&[], &["uses_std_trait"], EntityKind::Function);
  project.assert_edge(EdgeKind::References, caller, item(&[], &["Config"], EntityKind::Struct), RESOLVED);
  assert_eq!(project.unresolved_named(caller, "from_str").len(), 1);
}

#[test]
fn should_keep_the_type_edge_for_qualified_paths_and_associated_items() {
  let synthetic = type_anchor_project();
  let project = &synthetic.project;
  let config = item(&[], &["Config"], EntityKind::Struct);
  project.assert_edge(EdgeKind::References, item(&[], &["uses_qualified"], EntityKind::Function), config, RESOLVED);
  project.assert_edge(EdgeKind::References, item(&[], &["uses_const"], EntityKind::Function), config, RESOLVED);
  project.assert_edge(
    EdgeKind::References,
    item(&[], &["uses_trait_external"], EntityKind::Function),
    item(&[], &["Shape"], EntityKind::Trait),
    RESOLVED,
  );
}

#[test]
fn should_not_add_a_type_edge_for_project_methods_turbofish_or_generic_params() {
  let synthetic = type_anchor_project();
  let project = &synthetic.project;
  let in_memory = item(&["store"], &["InMemory"], EntityKind::Struct);
  let make = item(&["store"], &["InMemory", "make"], EntityKind::Method);
  let caller = item(&[], &["uses_project_method"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, caller, make, RESOLVED);
  project.assert_no_edge(EdgeKind::References, caller, in_memory);
  let foo = item(&[], &["Foo"], EntityKind::Struct);
  let turbofish = item(&[], &["uses_turbofish"], EntityKind::Function);
  project.assert_edge(EdgeKind::References, turbofish, foo, RESOLVED);
  assert_eq!(project.edges_from(EdgeKind::References, turbofish).len(), 1);
  let generic = item(&[], &["uses_generic"], EntityKind::Function);
  project.assert_no_edge(EdgeKind::References, generic, item(&[], &["T"], EntityKind::Struct));
}

fn field(owner: &str, name: &str) -> atlas_core::EntityId {
  item(&[], &[owner, name], EntityKind::Field)
}

#[test]
fn should_resolve_field_accesses_on_typed_receivers_to_field_symbols() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      (
        "src/lib.rs",
        r#"use std::sync::Arc;

pub struct Config { pub name: String, pub retries: u32 }
pub struct Store { count: u32, config: Config, shared: Arc<Config>, boxed: Box<Config> }
pub struct Other { count: u32 }
pub struct Pair(pub u32, pub Config);

impl Store {
  pub fn bump(&mut self) {
    self.count += 1;
    let _ = self.config.name.len();
  }
  pub fn read(&self) -> u32 {
    self.count + self.count + self.count + self.count + self.count
  }
  pub fn through(&self) -> u32 {
    self.shared.retries + self.boxed.retries
  }
  pub fn logged(&self) -> String {
    format!("{}", self.count)
  }
}

pub fn typed(store: &Store, config: Box<Config>) -> u32 {
  store.count + config.retries
}

pub fn pair(p: Pair) -> u32 {
  p.0
}

pub fn wrapper(shared: Arc<Config>) -> u32 {
  shared.retries
}
"#,
      ),
    ],
  );
  let project = &synthetic.project;
  let method = |name: &str| item(&[], &["Store", name], EntityKind::Method);
  project.assert_edge(EdgeKind::References, method("bump"), field("Store", "count"), RESOLVED);
  project.assert_edge(EdgeKind::References, method("bump"), field("Store", "config"), RESOLVED);
  project.assert_edge(EdgeKind::References, method("bump"), field("Config", "name"), RESOLVED);
  let read = project.assert_edge(EdgeKind::References, method("read"), field("Store", "count"), RESOLVED);
  assert_eq!(read.evidence.len(), 4);
  for name in ["shared", "boxed"] {
    project.assert_edge(EdgeKind::References, method("through"), field("Store", name), RESOLVED);
  }
  project.assert_edge(EdgeKind::References, method("through"), field("Config", "retries"), RESOLVED);
  project.assert_edge(EdgeKind::References, method("logged"), field("Store", "count"), RESOLVED);
  let typed = item(&[], &["typed"], EntityKind::Function);
  project.assert_edge(EdgeKind::References, typed, field("Store", "count"), RESOLVED);
  project.assert_edge(EdgeKind::References, typed, field("Config", "retries"), RESOLVED);
  project.assert_no_edge(EdgeKind::References, typed, field("Other", "count"));
  project.assert_edge(EdgeKind::References, item(&[], &["wrapper"], EntityKind::Function), field("Config", "retries"), RESOLVED);
  let pair = item(&[], &["pair"], EntityKind::Function);
  let targets: Vec<_> = project.edges_from(EdgeKind::References, pair).iter().map(|edge| edge.to).collect();
  assert_eq!(targets, [item(&[], &["Pair"], EntityKind::Struct)]);
  assert!(project.unresolved_named(pair, "0").is_empty());
}

#[test]
fn should_match_unknown_receivers_by_field_name_only_when_the_name_is_unique_and_not_a_std_field() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      (
        "src/lib.rs",
        r#"pub mod cfg {
  pub struct Config { pub retries: u32, pub start: u32 }
}
pub struct Store { pub count: u32 }
pub struct Other { pub count: u32 }

fn make<T>() -> T { todo!() }

pub fn unique() -> u32 {
  let config: cfg::Config = make();
  let guess = make::<cfg::Config>();
  config.retries + guess.retries
}

pub fn shared_name(store: &Store) -> u32 {
  let store = make::<Store>();
  store.count
}

pub fn std_like() -> u32 {
  let range = make::<cfg::Config>();
  range.start
}

#[allow(non_camel_case_types)]
pub struct handle { pub other: u32 }

pub fn anchored() -> u32 {
  let handle = make::<cfg::Config>();
  handle.missing
}
"#,
      ),
    ],
  );
  let project = &synthetic.project;
  let retries = item(&["cfg"], &["Config", "retries"], EntityKind::Field);
  let unique = project.assert_edge(EdgeKind::References, item(&[], &["unique"], EntityKind::Function), retries, RESOLVED);
  assert_eq!(unique.evidence.len(), 1);
  let shared_name = item(&[], &["shared_name"], EntityKind::Function);
  for owner in ["Store", "Other"] {
    project.assert_no_edge(EdgeKind::References, shared_name, field(owner, "count"));
  }
  let found = project.unresolved_named(shared_name, "count");
  assert!(
    matches!(found.as_slice(), [unresolved] if matches!(&unresolved.reason, UnresolvedReason::Ambiguous { candidates } if candidates.len() == 2)),
    "{found:#?}"
  );
  let std_like = item(&[], &["std_like"], EntityKind::Function);
  project.assert_no_edge(EdgeKind::References, std_like, item(&["cfg"], &["Config", "start"], EntityKind::Field));
  let anchored = item(&[], &["anchored"], EntityKind::Function);
  let targets: Vec<_> = project.edges_from(EdgeKind::References, anchored).iter().map(|edge| edge.to).collect();
  assert_eq!(targets, [item(&["cfg"], &["Config"], EntityKind::Struct)]);
  project.assert_unresolved(anchored, "missing", &UnresolvedReason::NotFound);
}

#[test]
fn should_give_name_only_field_matches_heuristic_certainty() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      (
        "src/lib.rs",
        "pub struct Config { pub retries: u32 }\nfn make() -> Config { Config { retries: 1 } }\npub fn guess() -> u32 { let config = make(); config.retries }\n",
      ),
    ],
  );
  synthetic.project.assert_certainty(
    EdgeKind::References,
    item(&[], &["guess"], EntityKind::Function),
    field("Config", "retries"),
    &Certainty::heuristic(Confidence::new(0.5).unwrap(), "field-name-only"),
  );
}

#[test]
fn should_resolve_declared_generic_and_wrapper_receivers_through_bounds_and_arguments() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      (
        "src/lib.rs",
        r#"use std::sync::Arc;

pub trait Store { fn get(&self) -> u32; }
pub trait Other { fn get(&self) -> u32; }
pub struct Mem;
impl Store for Mem { fn get(&self) -> u32 { 1 } }
impl Mem { pub fn new() -> Self { Mem } }
pub struct Pg;
impl Pg { pub fn save(&self) {} }

pub fn bound<T: Store>(s: &T) -> u32 { s.get() }
pub fn where_clause<T>(s: T) -> u32 where T: Store { s.get() }
pub fn let_typed<T: Store>(make: fn() -> T) -> u32 { let s: T = make(); s.get() }
pub fn closure<T: Store>(items: Vec<T>) -> u32 { items.iter().map(|s: &T| s.get()).sum() }
pub fn boxed_generic<T: Store>(s: Box<T>) -> u32 { s.get() }
pub fn arc_dyn(make: fn() -> Arc<dyn Store>) -> u32 { let s: Arc<dyn Store> = make(); s.get() }
pub fn arc_concrete(p: Arc<Pg>) { p.save(); }
pub fn unbounded<T>(s: &T) -> u32 { s.get() }
"#,
      ),
    ],
  );
  let project = &synthetic.project;
  let store_get = item(&[], &["Store", "get"], EntityKind::Method);
  let mem_get = item(&[], &["Mem", "<Store>", "get"], EntityKind::Method);
  for caller in ["bound", "where_clause", "let_typed", "closure", "boxed_generic", "arc_dyn"] {
    let from = item(&[], &[caller], EntityKind::Function);
    project.assert_edge(EdgeKind::Calls, from, store_get, RESOLVED);
    project.assert_certainty(
      EdgeKind::Calls,
      from,
      mem_get,
      &Certainty::heuristic(Confidence::new(0.3).unwrap(), "dyn-dispatch-candidate"),
    );
    project.assert_no_edge(EdgeKind::Calls, from, item(&[], &["Other", "get"], EntityKind::Method));
  }
  project.assert_edge(
    EdgeKind::Calls,
    item(&[], &["arc_concrete"], EntityKind::Function),
    item(&[], &["Pg", "save"], EntityKind::Method),
    RESOLVED,
  );
  let unbounded = item(&[], &["unbounded"], EntityKind::Function);
  for target in [store_get, mem_get, item(&[], &["Other", "get"], EntityKind::Method)] {
    project.assert_no_edge_of_class(EdgeKind::Calls, unbounded, target, &[CertaintyClass::Resolved, CertaintyClass::Syntactic]);
  }
}

#[test]
fn should_keep_param_type_recovery_heuristic_when_a_shadowing_or_nested_binding_hides_the_param() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      (
        "src/lib.rs",
        r#"use std::sync::Arc;

pub trait Store { fn get(&self) -> u32; }
pub trait Other { fn get(&self) -> u32; }
pub struct Mem;
impl Mem { pub fn new() -> Self { Mem } pub fn get(&self) -> u32 { 0 } }
pub struct Pg;
impl Pg { pub fn save(&self) {} }
impl Mem { pub fn save(&self) {} }

pub fn shadowed<T: Store>(s: &T) -> u32 { let s = Mem::new(); s.get() }
pub fn pattern<T: Store>(s: &T, found: Option<Mem>) -> u32 { if let Some(s) = found { s.get() } else { 0 } }
pub fn closure<T: Store>(s: &T, all: Vec<Mem>) -> u32 { all.iter().map(|s| s.get()).sum() }
pub fn nested<T: Other>(t: &T) -> u32 {
  fn inner<T: Store>(t: &T) -> u32 { t.get() }
  inner(&Mem)
}
pub fn nested_wrapper(p: Arc<Pg>) {
  fn inner(p: Arc<Mem>) { p.save(); }
  inner(Arc::new(Mem));
}
"#,
      ),
    ],
  );
  let project = &synthetic.project;
  let recovered = Certainty::heuristic(Confidence::new(0.7).unwrap(), "param-type-recovery");
  for caller in ["shadowed", "pattern", "closure"] {
    project.assert_certainty(
      EdgeKind::Calls,
      item(&[], &[caller], EntityKind::Function),
      item(&[], &["Store", "get"], EntityKind::Method),
      &recovered,
    );
  }
  project.assert_edge(
    EdgeKind::Calls,
    item(&[], &["nested"], EntityKind::Function),
    item(&[], &["Other", "get"], EntityKind::Method),
    HEURISTIC,
  );
  project.assert_edge(
    EdgeKind::Calls,
    item(&[], &["nested_wrapper"], EntityKind::Function),
    item(&[], &["Pg", "save"], EntityKind::Method),
    HEURISTIC,
  );
  let authoritative = &[CertaintyClass::Resolved, CertaintyClass::Syntactic];
  for caller in ["shadowed", "pattern", "closure", "nested", "nested_wrapper"] {
    let from = item(&[], &[caller], EntityKind::Function);
    for target in [
      item(&[], &["Store", "get"], EntityKind::Method),
      item(&[], &["Other", "get"], EntityKind::Method),
      item(&[], &["Mem", "get"], EntityKind::Method),
      item(&[], &["Pg", "save"], EntityKind::Method),
      item(&[], &["Mem", "save"], EntityKind::Method),
    ] {
      project.assert_no_edge_of_class(EdgeKind::Calls, from, target, authoritative);
    }
  }
}
