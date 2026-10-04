mod common;

use std::time::{Duration, Instant};

use atlas_core::{Certainty, CertaintyClass, Confidence, EdgeKind, EntityId, EntityKind, LanguageFamily, UnresolvedReason};
use atlas_resolve::resolve_file;
use common::{Synthetic, file, rust, single, symbol_n, synthetic, ts};

const RESOLVED: &[CertaintyClass] = &[CertaintyClass::Resolved];
const AUTHORITATIVE: &[CertaintyClass] = &[CertaintyClass::Resolved, CertaintyClass::Syntactic, CertaintyClass::CompilerVerified];
const PACKAGE_JSON: &str = r#"{ "name": "app", "version": "1.0.0" }"#;

fn manifest(name: &str, dependencies: &str) -> String {
  format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dependencies}")
}

fn rust_project(source: &str) -> Synthetic {
  let manifest = manifest("demo", "");
  single("demo", &[("Cargo.toml", &manifest), ("src/lib.rs", source)])
}

fn ts_project(files: &[(&str, &str)]) -> Synthetic {
  let mut all = vec![("package.json", PACKAGE_JSON)];
  all.extend_from_slice(files);
  single("app", &all)
}

fn demo(module: &[&str], items: &[&str], kind: EntityKind) -> EntityId {
  rust("demo", "demo", module, items, kind)
}

fn app(module: &[&str], items: &[&str], kind: EntityKind) -> EntityId {
  ts("app", "app", module, items, kind)
}

fn heuristic(confidence: f32, rule: &str) -> Certainty {
  Certainty::heuristic(Confidence::new(confidence).unwrap(), rule)
}

fn assert_no_edges_to(project: &common::Project, from: EntityId, to: EntityId) {
  for kind in [EdgeKind::Calls, EdgeKind::References, EdgeKind::Instantiates, EdgeKind::TypeOf] {
    project.assert_no_edge(kind, from, to);
  }
}

fn sorted(mut ids: Vec<EntityId>) -> Vec<EntityId> {
  ids.sort();
  ids
}

#[test]
fn should_not_resolve_fn_local_items_to_module_items() {
  let synthetic = rust_project(
    "pub struct Config;\nimpl Config { pub fn load() {} }\npub enum Mode { A }\npub fn other() {\n  struct Config;\n  impl Config { fn load() {} }\n  enum Mode { A }\n  let _c: Config = Config;\n  Config::load();\n  let _m = Mode::A;\n}\npub fn plain() { let _c = Config; }\n",
  );
  let project = &synthetic.project;
  let other = demo(&[], &["other"], EntityKind::Function);
  assert_no_edges_to(project, other, demo(&[], &["Config"], EntityKind::Struct));
  assert_no_edges_to(project, other, demo(&[], &["Config", "load"], EntityKind::Method));
  let local = UnresolvedReason::Unsupported {
    detail: "fn-local item".to_owned(),
  };
  for name in ["Config", "load"] {
    project.assert_unresolved(other, name, &local);
  }
  project.assert_certainty(
    EdgeKind::References,
    other,
    demo(&[], &["Mode", "A"], EntityKind::Variant),
    &heuristic(0.5, "fn-local-item-possible"),
  );
  project.assert_edge(
    EdgeKind::References,
    demo(&[], &["plain"], EntityKind::Function),
    demo(&[], &["Config"], EntityKind::Struct),
    RESOLVED,
  );
}

#[test]
fn should_downgrade_local_type_shadowing_without_an_impl_to_heuristic() {
  let synthetic = rust_project("pub fn helper() {}\npub struct Config;\npub fn other() {\n  struct Config;\n  let _c = Config;\n}\n");
  let project = &synthetic.project;
  let other = demo(&[], &["other"], EntityKind::Function);
  project.assert_certainty(
    EdgeKind::References,
    other,
    demo(&[], &["Config"], EntityKind::Struct),
    &heuristic(0.5, "fn-local-item-possible"),
  );
}

#[test]
fn should_treat_fn_scoped_uses_that_disagree_with_the_module_binding_as_ambiguous() {
  let synthetic = rust_project(
    "mod a { pub fn run() {} }\nmod b { pub fn run() {} }\nuse a::run;\npub fn f() {\n  use crate::b::run;\n  run();\n}\npub fn g() {\n  {\n    use crate::b::run;\n  }\n  run();\n}\npub fn h() {\n  use crate::a::run;\n  run();\n}\n",
  );
  let project = &synthetic.project;
  let a_run = demo(&["a"], &["run"], EntityKind::Function);
  let b_run = demo(&["b"], &["run"], EntityKind::Function);
  for caller in ["f", "g"] {
    let from = demo(&[], &[caller], EntityKind::Function);
    project.assert_no_edge(EdgeKind::Calls, from, a_run);
    project.assert_no_edge(EdgeKind::Calls, from, b_run);
    project.assert_unresolved(
      from,
      "run",
      &UnresolvedReason::Ambiguous {
        candidates: sorted(vec![a_run, b_run]),
      },
    );
  }
  project.assert_edge(EdgeKind::Calls, demo(&[], &["h"], EntityKind::Function), a_run, RESOLVED);
}

#[test]
fn should_cap_receiver_types_recovered_from_parameter_text_at_heuristic() {
  let synthetic = rust_project(
    "use std::sync::Arc;\npub trait Store { fn insert(&self); }\npub struct Pg;\npub struct Mem;\nimpl Pg { pub fn insert(&self) {} pub fn save(&self) {} }\nimpl Mem { pub fn insert(&self) {} pub fn save(&self) {} pub fn new() -> Self { Mem } }\npub fn f<T: Store>(x: T) { let x = Mem::new(); x.insert(); }\npub fn g(x: Arc<Pg>) { let x: Arc<Mem> = Arc::new(Mem); x.save(); }\npub fn h(x: Pg) { let x = Mem::new(); x.save(); }\npub fn k(x: Arc<Pg>) { let c = |x: Arc<Mem>| x.save(); }\npub fn m(x: Pg) { if let Some(x) = Some(Mem) { x.save(); } }\n",
  );
  let project = &synthetic.project;
  project.assert_certainty(
    EdgeKind::Calls,
    demo(&[], &["f"], EntityKind::Function),
    demo(&[], &["Store", "insert"], EntityKind::Method),
    &heuristic(0.7, "param-type-recovery"),
  );
  for caller in ["g", "k"] {
    let from = demo(&[], &[caller], EntityKind::Function);
    project.assert_edge(EdgeKind::Calls, from, demo(&[], &["Mem", "save"], EntityKind::Method), RESOLVED);
    project.assert_no_edge(EdgeKind::Calls, from, demo(&[], &["Pg", "save"], EntityKind::Method));
  }
  for caller in ["f", "h", "m"] {
    let from = demo(&[], &[caller], EntityKind::Function);
    for target in [
      demo(&[], &["Pg", "save"], EntityKind::Method),
      demo(&[], &["Mem", "save"], EntityKind::Method),
      demo(&[], &["Store", "insert"], EntityKind::Method),
    ] {
      project.assert_no_edge_of_class(EdgeKind::Calls, from, target, AUTHORITATIVE);
    }
  }
}

#[test]
fn should_attribute_deref_wrapper_methods_to_the_wrapper() {
  let synthetic = rust_project(
    "use std::sync::Arc;\nuse std::rc::Rc;\npub struct Inner;\nimpl Clone for Inner { fn clone(&self) -> Self { Inner } }\nimpl Inner { pub fn as_ref(&self) -> &Self { self } pub fn strong_count(&self) {} pub fn clone_it(&self) {} }\npub fn f(x: Arc<Inner>, y: Box<Inner>, z: Rc<Inner>) { let _a = x.clone(); let _b = y.clone(); let _c = z.clone(); let _d = x.as_ref(); x.strong_count(); x.clone_it(); }\npub struct H { pub s: Arc<Inner> }\nimpl H { pub fn g(&self) { let _ = self.s.clone(); } }\n",
  );
  let project = &synthetic.project;
  let f = demo(&[], &["f"], EntityKind::Function);
  let g = demo(&[], &["H", "g"], EntityKind::Method);
  let clone = demo(&[], &["Inner", "<Clone>", "clone"], EntityKind::Method);
  for (from, target) in [
    (f, clone),
    (g, clone),
    (f, demo(&[], &["Inner", "as_ref"], EntityKind::Method)),
    (f, demo(&[], &["Inner", "strong_count"], EntityKind::Method)),
  ] {
    project.assert_no_edge(EdgeKind::Calls, from, target);
  }
  for name in ["clone", "as_ref", "strong_count"] {
    let found = project.unresolved_named(f, name);
    assert!(
      !found.is_empty() && found.iter().all(|unresolved| unresolved.reason == UnresolvedReason::External { package: "std".to_owned() }),
      "{name}: {found:#?}"
    );
  }
  project.assert_unresolved(g, "clone", &UnresolvedReason::External { package: "std".to_owned() });
  project.assert_edge(EdgeKind::Calls, f, demo(&[], &["Inner", "clone_it"], EntityKind::Method), RESOLVED);
}

#[test]
fn should_never_resolve_generic_parameters_to_same_named_types() {
  let synthetic = rust_project(
    "pub trait Repo { fn find(&self); }\npub struct R;\nimpl R { pub fn find(&self) {} }\npub struct Svc<R> { repo: R }\nimpl<R: Repo> Svc<R> {\n  pub fn f(&self, r: &R) { r.find(); self.repo.find(); }\n}\npub trait Visitor<R> { fn visit(&self, r: R) { r.find(); } }\npub struct Bounded<B: Repo> { inner: B }\nimpl<B: Repo> Bounded<B> { pub fn go(&self) { self.inner.find(); } }\n",
  );
  let project = &synthetic.project;
  let concrete = demo(&[], &["R", "find"], EntityKind::Method);
  for from in [demo(&[], &["Svc", "f"], EntityKind::Method), demo(&[], &["Visitor", "visit"], EntityKind::Method)] {
    project.assert_no_edge_of_class(EdgeKind::Calls, from, concrete, AUTHORITATIVE);
  }
  project.assert_edge(
    EdgeKind::Calls,
    demo(&[], &["Bounded", "go"], EntityKind::Method),
    demo(&[], &["Repo", "find"], EntityKind::Method),
    RESOLVED,
  );
  let ts_synthetic = ts_project(&[
    ("src/model.ts", "export class Model { save() {} }\n"),
    (
      "src/use.ts",
      "import { Model } from './model';\nexport class Base<Model extends { save(): void }> { m!: Model; go(x: Model) { x.save(); this.m.save(); } }\nexport function f<Model>(x: Model): Model { return x; }\n",
    ),
  ]);
  let go = app(&["src", "use"], &["Base", "go"], EntityKind::Method);
  ts_synthetic
    .project
    .assert_no_edge_of_class(EdgeKind::Calls, go, app(&["src", "model"], &["Model", "save"], EntityKind::Method), AUTHORITATIVE);
  ts_synthetic.project.assert_no_edge(
    EdgeKind::References,
    app(&["src", "use"], &["f"], EntityKind::Function),
    app(&["src", "model"], &["Model"], EntityKind::Class),
  );
}

#[test]
fn should_keep_ts_value_and_type_namespaces_apart_in_either_declaration_order() {
  let synthetic = ts_project(&[
    (
      "src/c.ts",
      "export type Config = { a: number };\nexport const Config = { parse(x: unknown) { return x; } };\nexport interface Svc { run(): void }\nexport function Svc() {}\n",
    ),
    ("src/d.ts", "export const User = { make() {} };\nexport type User = { id: string };\n"),
    ("src/e.ts", "export type User = { id: string };\nexport const User = { make() {} };\n"),
    (
      "src/use.ts",
      "import { Config, Svc } from './c';\nimport { User as First } from './d';\nimport { User as Second } from './e';\nexport function go(c: Config) { Svc(); }\nexport function t(s: Svc) { s.run(); }\nfunction consume(..._values: unknown[]) {}\nexport function u(a: First, b: Second) { consume(First, Second); }\n",
    ),
  ]);
  let project = &synthetic.project;
  let go = app(&["src", "use"], &["go"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, go, app(&["src", "c"], &["Svc"], EntityKind::Function), RESOLVED);
  project.assert_no_edge(EdgeKind::Calls, go, app(&["src", "c"], &["Svc"], EntityKind::Interface));
  project.assert_no_edge(EdgeKind::References, go, app(&["src", "c"], &["Svc"], EntityKind::Interface));
  project.assert_edge(EdgeKind::References, go, app(&["src", "c"], &["Config"], EntityKind::TypeAlias), RESOLVED);
  project.assert_no_edge(EdgeKind::References, go, app(&["src", "c"], &["Config"], EntityKind::Constant));
  let t = app(&["src", "use"], &["t"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, t, app(&["src", "c"], &["Svc", "run"], EntityKind::Method), RESOLVED);
  let u = app(&["src", "use"], &["u"], EntityKind::Function);
  for module in ["d", "e"] {
    project.assert_edge(EdgeKind::References, u, app(&["src", module], &["User"], EntityKind::TypeAlias), RESOLVED);
    let constant = app(&["src", module], &["User"], EntityKind::Constant);
    let value = project.edges_between(EdgeKind::References, u, constant);
    assert_eq!(value.len(), 1, "value reference to {module}.User");
  }
}

#[test]
fn should_separate_static_and_instance_ts_members() {
  let synthetic = ts_project(&[(
    "src/s.ts",
    "export class K { static make() {} make() {} }\nexport function a() { K.make(); }\nexport function b(k: K) { k.make(); }\nexport class A {\n  static helper() {}\n  helper() {}\n  static run() { this.helper(); }\n  go() { this.helper(); }\n  static { this.helper(); }\n}\n",
  )]);
  let project = &synthetic.project;
  let n = |items: &[&str], kind: EntityKind, index: u32| symbol_n(LanguageFamily::Ts, "app", "app", &["src", "s"], items, kind, index);
  let static_make = n(&["K", "make"], EntityKind::Method, 0);
  let instance_make = n(&["K", "make"], EntityKind::Method, 1);
  project.assert_edge(EdgeKind::Calls, app(&["src", "s"], &["a"], EntityKind::Function), static_make, RESOLVED);
  project.assert_no_edge(EdgeKind::Calls, app(&["src", "s"], &["a"], EntityKind::Function), instance_make);
  project.assert_edge(EdgeKind::Calls, app(&["src", "s"], &["b"], EntityKind::Function), instance_make, RESOLVED);
  project.assert_no_edge(EdgeKind::Calls, app(&["src", "s"], &["b"], EntityKind::Function), static_make);
  let static_helper = n(&["A", "helper"], EntityKind::Method, 0);
  let instance_helper = n(&["A", "helper"], EntityKind::Method, 1);
  let static_run = app(&["src", "s"], &["A", "run"], EntityKind::Method);
  let go = app(&["src", "s"], &["A", "go"], EntityKind::Method);
  let class = app(&["src", "s"], &["A"], EntityKind::Class);
  for from in [static_run, class] {
    project.assert_edge(EdgeKind::Calls, from, static_helper, RESOLVED);
    project.assert_no_edge(EdgeKind::Calls, from, instance_helper);
  }
  project.assert_edge(EdgeKind::Calls, go, instance_helper, RESOLVED);
  project.assert_no_edge(EdgeKind::Calls, go, static_helper);
}

#[test]
fn should_propagate_heuristic_certainty_through_extends_hops() {
  let synthetic = synthetic(&[(
    "mono",
    &[
      ("package.json", r#"{ "name": "root", "version": "1.0.0", "workspaces": ["packages/*"] }"#),
      (
        "packages/sdk/package.json",
        r#"{ "name": "@acme/sdk", "version": "1.0.0", "exports": { ".": { "types": "./dist/index.d.ts", "import": "./dist/index.js" } } }"#,
      ),
      ("packages/sdk/src/index.ts", "export class Base { hello() {} }\n"),
      (
        "packages/web/package.json",
        r#"{ "name": "web", "version": "1.0.0", "dependencies": { "@acme/sdk": "*" } }"#,
      ),
      (
        "packages/web/src/app.ts",
        "import { Base } from '@acme/sdk';\nexport class Mine extends Base { go() { this.hello(); } }\n",
      ),
    ],
  )]);
  let project = &synthetic.project;
  let go = ts("mono", "web", &["src", "app"], &["Mine", "go"], EntityKind::Method);
  let hello = ts("mono", "@acme/sdk", &["src", "index"], &["Base", "hello"], EntityKind::Method);
  project.assert_certainty(EdgeKind::Calls, go, hello, &heuristic(0.85, "dist-to-src"));
}

#[test]
fn should_respect_macro_rules_textual_order() {
  let synthetic = rust_project("pub fn f() { vec![1]; mk!(); }\nmacro_rules! vec { () => {} }\nmacro_rules! mk { () => {} }\npub fn g() { vec![1]; mk!(); }\n");
  let project = &synthetic.project;
  let f = demo(&[], &["f"], EntityKind::Function);
  let g = demo(&[], &["g"], EntityKind::Function);
  for name in ["vec", "mk"] {
    let definition = demo(&[], &[name], EntityKind::Macro);
    project.assert_no_edge(EdgeKind::References, f, definition);
    project.assert_edge(EdgeKind::References, g, definition, RESOLVED);
  }
  project.assert_unresolved(f, "vec", &UnresolvedReason::External { package: "std".to_owned() });
  project.assert_unresolved(f, "mk", &UnresolvedReason::NotFound);
}

#[test]
fn should_not_match_registry_dependencies_to_same_workspace_members() {
  let root = "[workspace]\nmembers = [\"app\", \"log\"]\n";
  let app_manifest = manifest("app", "log = \"0.4\"");
  let log_manifest = manifest("log", "");
  let synthetic = single(
    "ws",
    &[
      ("Cargo.toml", root),
      ("app/Cargo.toml", &app_manifest),
      ("app/src/lib.rs", "pub fn go() { log::info(); }\n"),
      ("log/Cargo.toml", &log_manifest),
      ("log/src/lib.rs", "pub fn info() {}\n"),
    ],
  );
  let project = &synthetic.project;
  let go = rust("ws", "app", &[], &["go"], EntityKind::Function);
  project.assert_no_edge(EdgeKind::Calls, go, rust("ws", "log", &[], &["info"], EntityKind::Function));
  project.assert_unresolved(go, "info", &UnresolvedReason::External { package: "log".to_owned() });
}

#[test]
fn should_report_a_crate_present_in_two_repos_as_ambiguous() {
  let common_manifest = manifest("common", "");
  let user_manifest = manifest("user", "common = \"1\"");
  let synthetic = synthetic(&[
    ("r1", &[("Cargo.toml", common_manifest.as_str()), ("src/lib.rs", "pub fn util() {}\n")]),
    ("r2", &[("Cargo.toml", common_manifest.as_str()), ("src/lib.rs", "pub fn util() {}\n")]),
    ("r3", &[("Cargo.toml", user_manifest.as_str()), ("src/lib.rs", "pub fn go() { common::util(); }\n")]),
  ]);
  let project = &synthetic.project;
  let go = rust("r3", "user", &[], &["go"], EntityKind::Function);
  let found = project.unresolved_named(go, "util");
  assert!(
    matches!(&found[0].reason, UnresolvedReason::Ambiguous { candidates } if candidates.len() == 2),
    "{found:#?}"
  );
  assert!(project.edges_from(EdgeKind::Calls, go).is_empty());
}

#[test]
fn should_prefer_ts_sources_over_a_literal_js_specifier() {
  let synthetic = ts_project(&[
    ("src/a.ts", "export function a() {}\n"),
    ("src/a.js", "exports.a = function () {};\n"),
    ("src/use.ts", "import { a } from './a.js';\nexport function go() { a(); }\n"),
  ]);
  let project = &synthetic.project;
  project.assert_edge(EdgeKind::Imports, file("app", "src/use.ts"), file("app", "src/a.ts"), RESOLVED);
  project.assert_no_edge(EdgeKind::Imports, file("app", "src/use.ts"), file("app", "src/a.js"));
}

#[test]
fn should_resolve_cheap_namespace_and_alias_cases() {
  let synthetic = rust_project(
    "extern crate self as demo2;\nmod a { pub struct Conf { pub x: u8 } }\nmod b { #[allow(non_snake_case)] pub fn Conf() {} }\nuse a::Conf;\nuse b::*;\npub fn f() { Conf(); }\npub fn g(_c: Conf) {}\npub fn h() { demo2::f(); }\n",
  );
  let project = &synthetic.project;
  let f = demo(&[], &["f"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, f, demo(&["b"], &["Conf"], EntityKind::Function), RESOLVED);
  project.assert_no_edge(EdgeKind::Instantiates, f, demo(&["a"], &["Conf"], EntityKind::Struct));
  project.assert_edge(
    EdgeKind::References,
    demo(&[], &["g"], EntityKind::Function),
    demo(&["a"], &["Conf"], EntityKind::Struct),
    RESOLVED,
  );
  project.assert_edge(EdgeKind::Calls, demo(&[], &["h"], EntityKind::Function), f, RESOLVED);
  let overloads = ts_project(&[(
    "src/s.ts",
    "export function f(a: string): void;\nexport function f(a: number): void;\nexport function f(a: any) {}\nexport function go() { f(1); }\n",
  )]);
  let implementation = symbol_n(LanguageFamily::Ts, "app", "app", &["src", "s"], &["f"], EntityKind::Function, 2);
  overloads
    .project
    .assert_edge(EdgeKind::Calls, app(&["src", "s"], &["go"], EntityKind::Function), implementation, RESOLVED);
}

#[test]
fn should_index_a_path_file_reached_twice_under_both_modules() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      ("src/lib.rs", "pub mod a;\npub mod b;\npub fn go() { a::s::f(); b::s::f(); }\n"),
      ("src/a.rs", "#[path = \"shared.rs\"]\npub mod s;\n"),
      ("src/b.rs", "#[path = \"shared.rs\"]\npub mod s;\n"),
      ("src/shared.rs", "pub fn f() {}\n"),
    ],
  );
  let project = &synthetic.project;
  let edge = project.assert_edge(
    EdgeKind::Calls,
    demo(&[], &["go"], EntityKind::Function),
    demo(&["a", "s"], &["f"], EntityKind::Function),
    RESOLVED,
  );
  assert_eq!(edge.evidence.len(), 2);
  let placement = project.table.placement(&common::repo("demo"), &atlas_core::RelPath::new("src/shared.rs").unwrap()).unwrap();
  assert_eq!(placement.also_in.len(), 1);
  assert_eq!(placement.also_in[0].module_path, ["b", "s"]);
}

#[test]
fn should_add_override_candidates_for_virtual_dispatch() {
  let synthetic = ts_project(&[(
    "src/s.ts",
    "export class Base { template() { this.step(); } step() {} }\nexport class Sub extends Base { step() {} parent() { super.step(); } }\nexport abstract class Repo { abstract find(): void; }\nexport class PgRepo extends Repo { find() {} }\nexport class Svc { constructor(private repo: Repo) {} go(b: Base) { this.repo.find(); b.step(); } }\n",
  )]);
  let project = &synthetic.project;
  let member = |items: &[&str]| app(&["src", "s"], items, EntityKind::Method);
  let candidate = heuristic(0.3, "override-dispatch-candidate");
  project.assert_edge(EdgeKind::Calls, member(&["Base", "template"]), member(&["Base", "step"]), RESOLVED);
  project.assert_certainty(EdgeKind::Calls, member(&["Base", "template"]), member(&["Sub", "step"]), &candidate);
  project.assert_certainty(EdgeKind::Calls, member(&["Svc", "go"]), member(&["Sub", "step"]), &candidate);
  project.assert_certainty(EdgeKind::Calls, member(&["Svc", "go"]), member(&["PgRepo", "find"]), &candidate);
  project.assert_edge(EdgeKind::Calls, member(&["Svc", "go"]), member(&["Repo", "find"]), RESOLVED);
  project.assert_edge(EdgeKind::Calls, member(&["Sub", "parent"]), member(&["Base", "step"]), RESOLVED);
  project.assert_no_edge(EdgeKind::Calls, member(&["Sub", "parent"]), member(&["Sub", "step"]));
  let callers: Vec<EntityId> = project.edges_to(EdgeKind::Calls, member(&["Sub", "step"])).iter().map(|edge| edge.from).collect();
  assert_eq!(sorted(callers), sorted(vec![member(&["Base", "template"]), member(&["Svc", "go"])]));

  let rust_synthetic = rust_project(
    "pub trait Handler {\n  fn handle(&self) { self.step(); Self::step2(); }\n  fn step(&self) {}\n  fn step2() {}\n}\npub struct A;\nimpl Handler for A { fn step(&self) {} fn step2() {} }\npub struct B;\nimpl Handler for B {}\n",
  );
  let rust_project = &rust_synthetic.project;
  let handle = demo(&[], &["Handler", "handle"], EntityKind::Method);
  rust_project.assert_edge(EdgeKind::Calls, handle, demo(&[], &["Handler", "step2"], EntityKind::Method), RESOLVED);
  rust_project.assert_certainty(EdgeKind::Calls, handle, demo(&[], &["A", "<Handler>", "step2"], EntityKind::Method), &candidate);
  rust_project.assert_certainty(
    EdgeKind::Calls,
    handle,
    demo(&[], &["A", "<Handler>", "step"], EntityKind::Method),
    &heuristic(0.3, "dyn-dispatch-candidate"),
  );
}

fn glob_web(k: usize) -> String {
  let mut lib = String::new();
  for i in 0..k {
    lib.push_str(&format!("pub mod m{i} {{\n"));
    for j in (0..k).filter(|j| *j != i) {
      lib.push_str(&format!("  pub use crate::m{j}::*;\n"));
    }
    lib.push_str(&format!("  pub fn own{i}() {{}}\n}}\n"));
  }
  lib.push_str("pub fn go() { m0::missing(); m0::own5(); m3::own11(); }\n");
  lib
}

fn timed_resolution(project: &common::Project, repo: &str, path: &str) -> Duration {
  let facts = project.facts(repo, path);
  let started = Instant::now();
  resolve_file(facts, &project.table).unwrap();
  started.elapsed()
}

#[test]
fn should_resolve_mutual_glob_and_star_webs_quickly() {
  let synthetic = rust_project(&glob_web(12));
  let project = &synthetic.project;
  let go = demo(&[], &["go"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, go, demo(&["m5"], &["own5"], EntityKind::Function), RESOLVED);
  project.assert_edge(EdgeKind::Calls, go, demo(&["m11"], &["own11"], EntityKind::Function), RESOLVED);
  project.assert_unresolved(go, "missing", &UnresolvedReason::NotFound);
  let elapsed = timed_resolution(project, "demo", "src/lib.rs");
  assert!(elapsed < Duration::from_millis(100), "glob web took {elapsed:?}");

  let mut files: Vec<(String, String)> = Vec::new();
  for i in 0..12 {
    let mut body: String = (0..12).filter(|j| *j != i).map(|j| format!("export * from './f{j}';\n")).collect();
    body.push_str(&format!("export function own{i}() {{}}\n"));
    files.push((format!("src/f{i}.ts"), body));
  }
  files.push((
    "src/use.ts".to_owned(),
    "import { missing, own3 } from './f0';\nexport function go() { missing(); own3(); }\n".to_owned(),
  ));
  let borrowed: Vec<(&str, &str)> = files.iter().map(|(path, body)| (path.as_str(), body.as_str())).collect();
  let star = ts_project(&borrowed);
  let ts_go = app(&["src", "use"], &["go"], EntityKind::Function);
  star
    .project
    .assert_edge(EdgeKind::Calls, ts_go, app(&["src", "f3"], &["own3"], EntityKind::Function), RESOLVED);
  star.project.assert_unresolved(ts_go, "missing", &UnresolvedReason::NotFound);
  let elapsed = timed_resolution(&star.project, "app", "src/use.ts");
  assert!(elapsed < Duration::from_millis(100), "star web took {elapsed:?}");
}

#[test]
fn should_keep_a_single_candidate_for_a_ubiquitous_method_name_unresolved() {
  let synthetic = rust_project("pub struct Bag;\nimpl Bag { pub fn iter(&self) {} }\npub fn f(values: Vec<u8>) { let v = values; v.iter(); }\n");
  let project = &synthetic.project;
  let f = demo(&[], &["f"], EntityKind::Function);
  let iter = demo(&[], &["Bag", "iter"], EntityKind::Method);
  project.assert_no_edge(EdgeKind::Calls, f, iter);
  project.assert_unresolved(f, "iter", &UnresolvedReason::Ambiguous { candidates: vec![iter] });
}

#[test]
fn should_not_fall_through_a_fn_local_glob_or_module_to_module_items() {
  let synthetic = rust_project(
    "pub enum Outer { A }\npub use Outer::*;\npub mod m { pub fn g() {} }\npub fn variants() {\n  enum Mode { A }\n  use Mode::*;\n  let _x = A;\n}\npub fn modules() {\n  mod m { pub fn g() {} }\n  m::g();\n}\npub fn plain() { let _x = A; m::g(); }\n",
  );
  let project = &synthetic.project;
  let variants = demo(&[], &["variants"], EntityKind::Function);
  let outer_a = demo(&[], &["Outer", "A"], EntityKind::Variant);
  project.assert_no_edge(EdgeKind::References, variants, outer_a);
  project.assert_unresolved(
    variants,
    "A",
    &UnresolvedReason::Unsupported {
      detail: "fn-local item".to_owned(),
    },
  );
  let g = demo(&["m"], &["g"], EntityKind::Function);
  project.assert_certainty(EdgeKind::Calls, demo(&[], &["modules"], EntityKind::Function), g, &heuristic(0.5, "fn-local-item-possible"));
  let plain = demo(&[], &["plain"], EntityKind::Function);
  project.assert_edge(EdgeKind::References, plain, outer_a, RESOLVED);
  project.assert_edge(EdgeKind::Calls, plain, g, RESOLVED);
}

#[test]
fn should_weaken_items_shadowed_inside_const_and_static_initializer_blocks() {
  let synthetic = rust_project("pub struct Config;\npub const C: () = {\n  struct Config;\n  let _c = Config;\n};\npub const D: Config = Config;\n");
  let project = &synthetic.project;
  let config = demo(&[], &["Config"], EntityKind::Struct);
  project.assert_certainty(
    EdgeKind::References,
    demo(&[], &["C"], EntityKind::Constant),
    config,
    &heuristic(0.5, "fn-local-item-possible"),
  );
  project.assert_no_edge_of_class(EdgeKind::References, demo(&[], &["C"], EntityKind::Constant), config, AUTHORITATIVE);
  project.assert_edge(EdgeKind::References, demo(&[], &["D"], EntityKind::Constant), config, RESOLVED);
}

#[test]
fn should_keep_refs_in_const_annotation_and_plain_initializers_resolved() {
  let synthetic = rust_project(
    "use std::sync::LazyLock;\npub struct Config;\nimpl Config {\n  pub const fn new() -> Self { Config }\n}\npub struct Foo;\nimpl Foo {\n  pub fn build() -> Foo { Foo }\n}\npub const DEFAULT: Config = Config::new();\nstatic X: LazyLock<Foo> = LazyLock::new(Foo::build);\n",
  );
  let project = &synthetic.project;
  let config = demo(&[], &["Config"], EntityKind::Struct);
  let foo = demo(&[], &["Foo"], EntityKind::Struct);
  let default = demo(&[], &["DEFAULT"], EntityKind::Constant);
  let x = demo(&[], &["X"], EntityKind::Constant);
  project.assert_edge(EdgeKind::TypeOf, default, config, RESOLVED);
  project.assert_edge(EdgeKind::Calls, default, demo(&[], &["Config", "new"], EntityKind::Method), RESOLVED);
  project.assert_edge(EdgeKind::TypeOf, x, foo, RESOLVED);
  project.assert_edge(EdgeKind::References, x, demo(&[], &["Foo", "build"], EntityKind::Method), RESOLVED);
}

#[test]
fn should_keep_the_annotation_resolved_while_weakening_the_initializer_block() {
  let synthetic = rust_project("pub struct Config;\npub const C: Config = {\n  struct Config;\n  let _c = Config;\n  Config\n};\n");
  let project = &synthetic.project;
  let config = demo(&[], &["Config"], EntityKind::Struct);
  let c = demo(&[], &["C"], EntityKind::Constant);
  project.assert_edge(EdgeKind::TypeOf, c, config, RESOLVED);
  project.assert_certainty(EdgeKind::References, c, config, &heuristic(0.5, "fn-local-item-possible"));
}

#[test]
fn should_keep_a_multi_line_annotation_and_item_free_initializer_resolved() {
  let synthetic = rust_project(
    "pub struct Config;\npub struct Wrap<T>(T);\npub const fn make() -> Config { Config }\n#[allow(dead_code)]\n/// docs\npub const VALUE: Wrap<\n  Config,\n> =\n  Wrap(\n    make(),\n  );\n",
  );
  let project = &synthetic.project;
  let value = demo(&[], &["VALUE"], EntityKind::Constant);
  project.assert_edge(EdgeKind::TypeOf, value, demo(&[], &["Config"], EntityKind::Struct), RESOLVED);
  project.assert_edge(EdgeKind::Calls, value, demo(&[], &["make"], EntityKind::Function), RESOLVED);
}

#[test]
fn should_not_let_a_struct_literal_field_or_string_trigger_the_item_scan() {
  let synthetic = rust_project(
    "pub struct Config;\npub struct Raw { pub r#type: u8 }\npub const R: Raw = { let _c = Config; Raw { r#type: 1 } };\npub const S: &str = \"struct enum mod fn\";\n",
  );
  let project = &synthetic.project;
  let config = demo(&[], &["Config"], EntityKind::Struct);
  let r = demo(&[], &["R"], EntityKind::Constant);
  project.assert_edge(EdgeKind::References, r, config, RESOLVED);
}

#[test]
fn should_weaken_when_an_item_hides_past_the_fingerprint_token_cap() {
  let filler = "0,".repeat(3_000);
  let source = format!(
    "pub struct Config;\npub const BIG: () = {{ let _a = [{filler}]; struct Config; let _c = Config; }};\npub fn big() {{ let _a = [{filler}]; struct Config; let _c = Config; }}\n"
  );
  let synthetic = rust_project(&source);
  let project = &synthetic.project;
  let config = demo(&[], &["Config"], EntityKind::Struct);
  let weak = heuristic(0.5, "fn-local-item-possible");
  for caller in [demo(&[], &["BIG"], EntityKind::Constant), demo(&[], &["big"], EntityKind::Function)] {
    project.assert_no_edge_of_class(EdgeKind::References, caller, config, AUTHORITATIVE);
    project.assert_certainty(EdgeKind::References, caller, config, &weak);
  }
}

#[test]
fn should_weaken_annotation_refs_when_the_annotation_declares_an_item_block() {
  let synthetic = rust_project("pub struct Config;\npub const C: [Config; { struct Config; 1 }] = [Config];\n");
  let project = &synthetic.project;
  let config = demo(&[], &["Config"], EntityKind::Struct);
  let c = demo(&[], &["C"], EntityKind::Constant);
  project.assert_no_edge_of_class(EdgeKind::TypeOf, c, config, AUTHORITATIVE);
  project.assert_no_edge_of_class(EdgeKind::References, c, config, AUTHORITATIVE);
}

#[test]
fn should_resolve_associated_trait_cfg_and_commented_consts() {
  let synthetic = rust_project(
    "pub struct Config;\npub struct Wrap<T>(T);\npub const fn make() -> Wrap<Config> { Wrap(Config) }\npub struct S;\nimpl S {\n  pub const E: Config = Config;\n}\npub trait T {\n  const N: Config;\n  const M: Config = Config;\n}\n#[cfg(unix)]\npub const U: Config = Config;\npub const V: Wrap<\n  // note\n  Config, // trailing\n> = make();\n",
  );
  let project = &synthetic.project;
  let config = demo(&[], &["Config"], EntityKind::Struct);
  for owner in [
    demo(&[], &["S", "E"], EntityKind::Constant),
    demo(&[], &["T", "N"], EntityKind::Constant),
    demo(&[], &["T", "M"], EntityKind::Constant),
    demo(&[], &["U"], EntityKind::Constant),
    demo(&[], &["V"], EntityKind::Constant),
  ] {
    project.assert_edge(EdgeKind::TypeOf, owner, config, RESOLVED);
  }
  project.assert_edge(EdgeKind::References, demo(&[], &["S", "E"], EntityKind::Constant), config, RESOLVED);
  project.assert_edge(
    EdgeKind::Calls,
    demo(&[], &["V"], EntityKind::Constant),
    demo(&[], &["make"], EntityKind::Function),
    RESOLVED,
  );
}

#[test]
fn should_keep_detector_lookups_regardless_of_call_order() {
  let manifest = manifest("demo", "");
  for detector_first in [true, false] {
    let mut synthetic = single(
      "demo",
      &[
        ("Cargo.toml", &manifest),
        ("src/lib.rs", "pub mod a;\npub mod b;\npub mod c;\n"),
        ("src/a.rs", "pub fn handler() {}\n"),
        ("src/b.rs", "pub fn routes() {}\n"),
        ("src/c.rs", "pub fn handler() {}\n"),
      ],
    );
    let project = &mut synthetic.project;
    let b = project.facts("demo", "src/b.rs").clone();
    let path = ["crate".to_owned(), "a".to_owned(), "handler".to_owned()];
    if detector_first {
      atlas_resolve::resolve_path_in_file(&b, &path, None, &project.table).unwrap();
      resolve_file(&b, &project.table).unwrap();
    } else {
      resolve_file(&b, &project.table).unwrap();
      atlas_resolve::resolve_path_in_file(&b, &path, None, &project.table).unwrap();
    }
    let facts = common::analyze(&common::repo("demo"), "src/a.rs", "pub use crate::c::handler;\n");
    let context = atlas_core::KeyContext {
      package: "demo".to_owned(),
      module_path: vec!["a".to_owned()],
    };
    let delta = project.table.upsert_file(&facts, &context).unwrap();
    let relocations = project.table.refresh();
    let affected = project.table.affected_files(&[delta], &relocations);
    assert!(
      affected.contains(&(common::repo("demo"), atlas_core::RelPath::new("src/b.rs").unwrap())),
      "{detector_first}: {affected:?}"
    );
    let after = atlas_resolve::resolve_path_in_file(&b, &path, None, &project.table).unwrap();
    assert_eq!(
      after,
      atlas_resolve::Resolution::Resolved(demo(&["c"], &["handler"], EntityKind::Function), Certainty::Resolved)
    );
  }
}

#[test]
fn should_list_callers_of_an_interface_when_a_class_newly_implements_it() {
  let mut synthetic = ts_project(&[
    ("src/base.ts", "export interface Repo { find(): void }\n"),
    ("src/mem.ts", "export class Mem { find() {} }\n"),
    ("src/use.ts", "import { Repo } from './base';\nexport function go(r: Repo) { r.find(); }\n"),
  ]);
  let project = &mut synthetic.project;
  let facts = common::analyze(
    &common::repo("app"),
    "src/mem.ts",
    "import { Repo } from './base';\nexport class Mem implements Repo { find() {} }\n",
  );
  let context = atlas_core::KeyContext {
    package: "app".to_owned(),
    module_path: vec!["src".to_owned(), "mem".to_owned()],
  };
  let delta = project.table.upsert_file(&facts, &context).unwrap();
  assert!(delta.names.contains("Repo"), "{:?}", delta.names);
  let relocations = project.table.refresh();
  let affected = project.table.affected_files(&[delta], &relocations);
  assert!(affected.contains(&(common::repo("app"), atlas_core::RelPath::new("src/use.ts").unwrap())), "{affected:?}");
  let use_facts = project.facts("app", "src/use.ts").clone();
  let resolved = resolve_file(&use_facts, &project.table).unwrap();
  let candidate = app(&["src", "mem"], &["Mem", "find"], EntityKind::Method);
  assert!(
    resolved
      .edges
      .iter()
      .any(|edge| edge.to == candidate && edge.certainty == heuristic(0.3, "interface-dispatch-candidate"))
  );
}

#[test]
fn should_weaken_a_glob_of_a_fn_local_module_that_shares_a_module_level_name() {
  let synthetic = rust_project(
    "pub mod m { pub fn g() {} }\npub use m::*;\npub fn f() {\n  mod m { pub fn g() {} }\n  use m::*;\n  g();\n}\npub struct S;\nimpl S {\n  pub fn method(&self) {\n    mod m { pub fn g() {} }\n    use m::*;\n    g();\n  }\n}\npub fn outer() {\n  fn inner() {\n    mod m { pub fn g() {} }\n    use m::*;\n    g();\n  }\n}\npub fn plain() {\n  use m::*;\n  g();\n}\n",
  );
  let project = &synthetic.project;
  let g = demo(&["m"], &["g"], EntityKind::Function);
  let weak = heuristic(0.5, "fn-local-item-possible");
  for caller in [
    demo(&[], &["f"], EntityKind::Function),
    demo(&[], &["S", "method"], EntityKind::Method),
    demo(&[], &["outer"], EntityKind::Function),
  ] {
    project.assert_no_edge_of_class(EdgeKind::Calls, caller, g, AUTHORITATIVE);
    project.assert_certainty(EdgeKind::Calls, caller, g, &weak);
  }
  project.assert_edge(EdgeKind::Calls, demo(&[], &["plain"], EntityKind::Function), g, RESOLVED);
}

#[test]
fn should_weaken_a_const_initializer_block_that_declares_an_item_on_one_line() {
  let synthetic = rust_project(
    "pub struct Config;\npub const C: () = { struct Config; let _c = Config; };\npub const D: Config = Config;\npub struct S;\nimpl S {\n  pub const E: () = { struct Config; let _c = Config; };\n}\npub fn f() {\n  const G: () = { struct Config; let _c = Config; };\n}\npub fn h() { let _c = Config; }\n",
  );
  let project = &synthetic.project;
  let config = demo(&[], &["Config"], EntityKind::Struct);
  let weak = heuristic(0.5, "fn-local-item-possible");
  for caller in [
    demo(&[], &["C"], EntityKind::Constant),
    demo(&[], &["S", "E"], EntityKind::Constant),
    demo(&[], &["f"], EntityKind::Function),
  ] {
    project.assert_no_edge_of_class(EdgeKind::References, caller, config, AUTHORITATIVE);
    project.assert_certainty(EdgeKind::References, caller, config, &weak);
  }
  project.assert_edge(EdgeKind::References, demo(&[], &["D"], EntityKind::Constant), config, RESOLVED);
  project.assert_edge(EdgeKind::References, demo(&[], &["h"], EntityKind::Function), config, RESOLVED);
}

#[test]
fn should_never_resolve_self_inside_an_impl_nested_in_a_method_to_the_outer_impl() {
  let synthetic = rust_project(
    "pub struct Foo { pub a: u32 }\npub struct Bar { pub a: u32 }\npub trait T { fn g(&self) -> u32; }\nimpl Foo {\n  pub fn helper(&self) {}\n  pub fn make() -> Self { Foo { a: 0 } }\n  pub fn m(&self) -> u32 {\n    struct L { a: u32 }\n    impl L { fn helper(&self) {} fn make() -> Self { L { a: 0 } } fn g(&self, other: &Self) -> u32 { self.helper(); let _ = Self::make(); let _ = Self { a: 1 }; other.a + self.a } }\n    0\n  }\n  pub fn n(&self) -> u32 {\n    impl T for Bar { fn g(&self) -> u32 { self.helper(); let _ = Self::make(); self.a } }\n    self.a\n  }\n}\n",
  );
  let project = &synthetic.project;
  let m = demo(&[], &["Foo", "m"], EntityKind::Method);
  let n = demo(&[], &["Foo", "n"], EntityKind::Method);
  let outer = [
    demo(&[], &["Foo", "helper"], EntityKind::Method),
    demo(&[], &["Foo", "make"], EntityKind::Method),
    demo(&[], &["Foo"], EntityKind::Struct),
  ];
  for from in [m, n] {
    for target in outer {
      for kind in [EdgeKind::Calls, EdgeKind::References, EdgeKind::Instantiates] {
        project.assert_no_edge_of_class(kind, from, target, AUTHORITATIVE);
      }
    }
  }
  project.assert_no_edge_of_class(EdgeKind::References, m, demo(&[], &["Foo", "a"], EntityKind::Field), AUTHORITATIVE);
  project.assert_unresolved(m, "make", &UnresolvedReason::Dynamic);
  project.assert_unresolved(n, "make", &UnresolvedReason::Dynamic);
  let outer_field = project.assert_edge(EdgeKind::References, n, demo(&[], &["Foo", "a"], EntityKind::Field), RESOLVED);
  assert_eq!(outer_field.evidence.len(), 1);
}

#[test]
fn should_not_guess_fields_by_name_when_a_declared_receiver_type_does_not_resolve() {
  let synthetic = rust_project(
    "macro_rules! mk { ($n:ident) => { pub struct $n { pub b: u32 } } }\nmk!(Gen);\npub struct Bar { pub b: u32 }\npub fn typed(g: &Gen) -> u32 { g.b }\npub fn boxed(g: Box<Gen>) -> u32 { g.b }\npub fn untyped() -> u32 { let g = make(); g.b }\nfn make() -> Bar { Bar { b: 0 } }\n",
  );
  let project = &synthetic.project;
  let bar_b = demo(&[], &["Bar", "b"], EntityKind::Field);
  for caller in ["typed", "boxed"] {
    let from = demo(&[], &[caller], EntityKind::Function);
    project.assert_no_edge(EdgeKind::References, from, bar_b);
    project.assert_unresolved(from, "b", &UnresolvedReason::NotFound);
  }
  project.assert_certainty(
    EdgeKind::References,
    demo(&[], &["untyped"], EntityKind::Function),
    bar_b,
    &heuristic(0.5, "field-name-only"),
  );
}

fn evidence_lines(edge: &atlas_core::Edge) -> Vec<u32> {
  edge.evidence.iter().map(|evidence| evidence.span.start_line).collect()
}

#[test]
fn should_leave_self_receivers_of_items_nested_in_a_body_unresolved_instead_of_guessing_by_name() {
  let synthetic = rust_project(
    "pub struct Outer { pub value: u32 }\npub struct Other { pub count: u32 }\nimpl Outer {\n  pub fn helper(&self) {}\n  pub fn run(&self) -> u32 {\n    self.helper();\n    struct Inner { count: u32 }\n    impl Inner {\n      fn helper(&self) {}\n      fn go(&self) -> u32 { self.helper(); self.count }\n    }\n    self.value\n  }\n  pub fn only_inner(&self) {\n    trait Local { fn helper(&self) {} fn go(&self) { self.helper(); } }\n  }\n}\n",
  );
  let project = &synthetic.project;
  let run = demo(&[], &["Outer", "run"], EntityKind::Method);
  let only_inner = demo(&[], &["Outer", "only_inner"], EntityKind::Method);
  let outer_helper = demo(&[], &["Outer", "helper"], EntityKind::Method);
  let nested_self = UnresolvedReason::Unsupported {
    detail: "self in nested item".to_owned(),
  };
  let outer_call = project.assert_edge(EdgeKind::Calls, run, outer_helper, RESOLVED);
  assert_eq!(evidence_lines(outer_call), vec![6]);
  project.assert_no_edge(EdgeKind::Calls, only_inner, outer_helper);
  project.assert_edge(EdgeKind::References, run, demo(&[], &["Outer", "value"], EntityKind::Field), RESOLVED);
  project.assert_no_edge(EdgeKind::References, run, demo(&[], &["Other", "count"], EntityKind::Field));
  assert_eq!(project.unresolved_named(run, "helper").len(), 1);
  project.assert_unresolved(run, "helper", &nested_self);
  project.assert_unresolved(run, "count", &nested_self);
  project.assert_unresolved(only_inner, "helper", &nested_self);
}

#[test]
fn should_leave_rebound_this_inside_a_class_unresolved_and_keep_name_only_outside_classes() {
  let synthetic = ts_project(&[(
    "src/s.ts",
    "export class Other { ping() {} }\nexport class Outer {\n  helper() {}\n  run() {\n    this.helper();\n    const arrow = () => this.helper();\n    class Inner { helper() {} go() { this.helper(); this.ping(); } }\n    const callback = function () { this.helper(); };\n    return [arrow, Inner, callback];\n  }\n}\nexport function free() { this.helper(); }\nexport const store = { a() { this.ping(); }, b() {} };\n",
  )]);
  let project = &synthetic.project;
  let run = app(&["src", "s"], &["Outer", "run"], EntityKind::Method);
  let free = app(&["src", "s"], &["free"], EntityKind::Function);
  let store = app(&["src", "s"], &["store"], EntityKind::Constant);
  let outer_helper = app(&["src", "s"], &["Outer", "helper"], EntityKind::Method);
  let other_ping = app(&["src", "s"], &["Other", "ping"], EntityKind::Method);
  let rebound = UnresolvedReason::Unsupported {
    detail: "rebound this".to_owned(),
  };
  let outer_call = project.assert_edge(EdgeKind::Calls, run, outer_helper, RESOLVED);
  assert_eq!(evidence_lines(outer_call), vec![5, 6]);
  project.assert_no_edge(EdgeKind::Calls, run, other_ping);
  assert_eq!(project.unresolved_named(run, "helper").len(), 2);
  project.assert_unresolved(run, "helper", &rebound);
  project.assert_unresolved(run, "ping", &rebound);
  let name_only = heuristic(0.5, "method-name-only");
  project.assert_certainty(EdgeKind::Calls, free, outer_helper, &name_only);
  project.assert_certainty(EdgeKind::Calls, store, other_ping, &name_only);
}
