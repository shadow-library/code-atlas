mod common;

use atlas_core::{CertaintyClass, EdgeKind, EntityKind, KeyContext, UnresolvedReason};
use atlas_resolve::Reachability;
use common::{rust, single};

const RESOLVED: &[CertaintyClass] = &[CertaintyClass::Resolved];
const MANIFEST: &str = "[package]\nname = \"tool\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

#[test]
fn should_override_the_default_path_of_bin_helper_modules_loaded_with_a_path_attribute() {
  let synthetic = single(
    "tool",
    &[
      ("Cargo.toml", MANIFEST),
      ("src/lib.rs", ""),
      ("src/bin/x.rs", "#[path = \"x/foo.rs\"]\nmod foo;\nfn main() { foo::helper(); }\n"),
      ("src/bin/x/foo.rs", "pub fn helper() {}\n"),
    ],
  );
  let project = &synthetic.project;
  let helper = project.result("tool", "src/bin/x/foo.rs");
  assert_eq!(helper.module_path_override, Some(vec!["foo".to_owned()]));
  assert_eq!(helper.package_override, None);
  assert_eq!(helper.reachability, Reachability::Reachable);
  assert_eq!(
    project
      .table
      .placement(&common::repo("tool"), &atlas_core::RelPath::new("src/bin/x/foo.rs").unwrap())
      .unwrap()
      .context,
    KeyContext {
      package: "tool/bin/x".to_owned(),
      module_path: vec!["foo".to_owned()],
    }
  );
  project.assert_edge(
    EdgeKind::Calls,
    rust("tool", "tool/bin/x", &[], &["main"], EntityKind::Function),
    rust("tool", "tool/bin/x", &["foo"], &["helper"], EntityKind::Function),
    RESOLVED,
  );
}

#[test]
fn should_treat_bin_roots_as_mod_rs_files_like_rustc() {
  let synthetic = single(
    "tool",
    &[
      ("Cargo.toml", MANIFEST),
      ("src/lib.rs", ""),
      ("src/bin/x.rs", "mod foo;\nfn main() { foo::helper(); }\n"),
      ("src/bin/x/foo.rs", "pub fn helper() {}\n"),
    ],
  );
  let project = &synthetic.project;
  let helper = project.result("tool", "src/bin/x/foo.rs");
  assert_eq!(helper.reachability, Reachability::Unreachable);
  assert_eq!(helper.module_path_override, None);
  project.assert_unresolved(rust("tool", "tool/bin/x", &[], &["main"], EntityKind::Function), "helper", &UnresolvedReason::NotFound);
}

#[test]
fn should_walk_mod_rs_non_mod_rs_inline_and_path_attribute_modules() {
  let synthetic = single(
    "tool",
    &[
      ("Cargo.toml", MANIFEST),
      (
        "src/lib.rs",
        "mod a;\nmod n;\n#[path = \"impls/unix.rs\"]\nmod imp;\npub mod outer {\n  pub mod inner;\n}\n#[cfg(test)]\nmod tests;\npub fn g() {\n  a::b::f();\n  n::c::h();\n  imp::run();\n  outer::inner::deep();\n  crate::outer::inner::deep();\n}\n",
      ),
      ("src/a/mod.rs", "pub mod b;\n"),
      ("src/a/b.rs", "pub fn f() {}\n"),
      ("src/n.rs", "pub mod c;\n"),
      ("src/n/c.rs", "pub fn h() { super::super::g(); }\n"),
      ("src/impls/unix.rs", "pub fn run() {}\n"),
      ("src/outer/inner.rs", "pub fn deep() {}\n"),
      ("src/tests.rs", "use super::g;\nfn t() { g(); }\n"),
      ("src/orphan.rs", "pub fn lost() {}\n"),
    ],
  );
  let project = &synthetic.project;
  let g = rust("tool", "tool", &[], &["g"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, g, rust("tool", "tool", &["a", "b"], &["f"], EntityKind::Function), RESOLVED);
  project.assert_edge(EdgeKind::Calls, g, rust("tool", "tool", &["n", "c"], &["h"], EntityKind::Function), RESOLVED);
  project.assert_edge(EdgeKind::Calls, g, rust("tool", "tool", &["imp"], &["run"], EntityKind::Function), RESOLVED);
  let deep = project.assert_edge(EdgeKind::Calls, g, rust("tool", "tool", &["outer", "inner"], &["deep"], EntityKind::Function), RESOLVED);
  assert_eq!(deep.evidence.len(), 2);
  project.assert_edge(EdgeKind::Calls, rust("tool", "tool", &["n", "c"], &["h"], EntityKind::Function), g, RESOLVED);
  project.assert_edge(EdgeKind::Calls, rust("tool", "tool", &["tests"], &["t"], EntityKind::Function), g, RESOLVED);
  assert_eq!(project.result("tool", "src/impls/unix.rs").module_path_override, Some(vec!["imp".to_owned()]));
  assert_eq!(project.result("tool", "src/tests.rs").reachability, Reachability::CfgGated { cfg: "test".to_owned() });
  assert_eq!(project.result("tool", "src/orphan.rs").reachability, Reachability::Unreachable);
  assert_eq!(project.result("tool", "src/orphan.rs").module_path_override, None);
  assert_eq!(project.result("tool", "src/lib.rs").reachability, Reachability::TargetRoot);
  project.assert_edge(
    EdgeKind::Contains,
    rust("tool", "tool", &["a"], &[], EntityKind::Module),
    rust("tool", "tool", &["a", "b"], &[], EntityKind::Module),
    RESOLVED,
  );
}

#[test]
fn should_report_relocations_when_a_path_attribute_changes() {
  let dir = common::write_repo(&[
    ("Cargo.toml", MANIFEST),
    ("src/lib.rs", "mod imp;\n"),
    ("src/imp.rs", "pub fn run() {}\n"),
    ("src/other.rs", "pub fn run() {}\n"),
  ]);
  let mut project = common::Project::load(&[("tool", dir.path().to_path_buf())]);
  let repo = common::repo("tool");
  let other = atlas_core::RelPath::new("src/other.rs").unwrap();
  assert_eq!(project.table.placement(&repo, &other).unwrap().reachability, Reachability::Unreachable);
  let facts = common::analyze(&repo, "src/lib.rs", "#[path = \"other.rs\"]\nmod imp;\n");
  let context = KeyContext {
    package: "tool".to_owned(),
    module_path: Vec::new(),
  };
  project.table.upsert_file(&facts, &context).unwrap();
  assert!(!project.table.is_fresh());
  let mut relocations = project.table.refresh();
  relocations.sort_by(|a, b| a.path.cmp(&b.path));
  let moved: Vec<(&str, Vec<String>)> = relocations.iter().map(|relocation| (relocation.path.as_str(), relocation.to.module_path.clone())).collect();
  assert_eq!(moved, vec![("src/other.rs", vec!["imp".to_owned()])]);
  assert_eq!(
    project.table.placement(&repo, &atlas_core::RelPath::new("src/imp.rs").unwrap()).unwrap().reachability,
    Reachability::Unreachable
  );
  let consumer = common::analyze(&repo, "src/lib.rs", "#[path = \"other.rs\"]\nmod imp;\npub fn go() { imp::run(); }\n");
  project.table.upsert_file(&consumer, &context).unwrap();
  project.table.refresh();
  let resolved = atlas_resolve::resolve_file(&consumer, &project.table).unwrap();
  let run = common::rust("tool", "tool", &["imp"], &["run"], EntityKind::Function);
  assert!(
    resolved
      .edges
      .iter()
      .any(|edge| edge.kind == EdgeKind::Calls && edge.to == run && edge.certainty == atlas_core::Certainty::Resolved)
  );
  assert!(resolved.unresolved.is_empty(), "{:#?}", resolved.unresolved);
}
