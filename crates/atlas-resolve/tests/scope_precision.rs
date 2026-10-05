mod common;

use atlas_core::{Certainty, CertaintyClass, Confidence, EdgeKind, EntityId, EntityKind, KeyContext, RelPath};
use common::{Project, Synthetic, rust, single, ts};

const RESOLVED: &[CertaintyClass] = &[CertaintyClass::Resolved];

fn manifest(name: &str, dependencies: &str) -> String {
  format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dependencies}")
}

fn path_dependency(name: &str) -> String {
  format!("{name} = {{ path = \"../{name}\" }}\n")
}

fn ws(package: &str, module: &[&str], items: &[&str], kind: EntityKind) -> EntityId {
  rust("ws", package, module, items, kind)
}

fn analyze_impl(package: &str, module: &[&str], owner: &str) -> EntityId {
  ws(package, module, &[owner, "<Analyzer>", "analyze"], EntityKind::Method)
}

fn dispatch_targets(project: &Project, from: EntityId, rule: &str) -> Vec<EntityId> {
  let candidate = Certainty::heuristic(Confidence::new(0.3).unwrap(), rule);
  let mut targets: Vec<EntityId> = project
    .edges_from(EdgeKind::Calls, from)
    .into_iter()
    .filter(|edge| edge.certainty == candidate)
    .map(|edge| edge.to)
    .collect();
  targets.sort();
  targets
}

fn sorted(mut ids: Vec<EntityId>) -> Vec<EntityId> {
  ids.sort();
  ids
}

fn demo(module: &[&str], items: &[&str], kind: EntityKind) -> EntityId {
  rust("demo", "demo", module, items, kind)
}

#[test]
fn should_let_a_crate_local_macro_rules_shadow_the_std_macro_of_the_same_name() {
  let manifest = manifest("demo", "");
  let synthetic = single(
    "demo",
    &[
      ("Cargo.toml", &manifest),
      ("src/lib.rs", "mod model;\nmod render;\n"),
      (
        "src/model.rs",
        "macro_rules! line {\n  ($out:expr) => { $out.push('\\n') };\n}\npub(crate) use line;\npub fn own(out: &mut String) { line!(out); }\n",
      ),
      (
        "src/render.rs",
        "use super::model::line;\npub fn emit(out: &mut String) { line!(out); }\npub fn nested(out: &mut String) { use crate::model::line as l; l!(out); }\n",
      ),
    ],
  );
  let project = &synthetic.project;
  let line = demo(&["model"], &["line"], EntityKind::Macro);
  for from in [
    demo(&["model"], &["own"], EntityKind::Function),
    demo(&["render"], &["emit"], EntityKind::Function),
    demo(&["render"], &["nested"], EntityKind::Function),
  ] {
    project.assert_edge(EdgeKind::References, from, line, RESOLVED);
    assert!(project.unresolved_named(from, "line").is_empty(), "{}", project.dump());
    assert!(project.unresolved_named(from, "l").is_empty(), "{}", project.dump());
  }
}

const WORKSPACE: &str = "[workspace]\nmembers = [\"base\", \"plugin\", \"mid\", \"app\", \"other\"]\n";
const BASE_LIB: &str = "pub trait Analyzer {\n  fn analyze(&self) -> u32;\n  fn describe(&self) -> u32 { self.analyze() }\n  fn rank(&self) -> u32 { Self::analyze(self) }\n}\n#[cfg(test)]\nmod tests {\n  use super::Analyzer;\n  struct Fake;\n  impl Analyzer for Fake { fn analyze(&self) -> u32 { 0 } }\n  fn run(analyzer: &dyn Analyzer) -> u32 { analyzer.analyze() }\n  fn boxed(analyzer: Box<dyn Analyzer>) -> u32 { analyzer.analyze() }\n  fn generic<T: Analyzer>(analyzer: &T) -> u32 { analyzer.analyze() }\n}\n";
const PLUGIN_LIB: &str = "use base::Analyzer;\npub struct RustAnalyzer;\nimpl Analyzer for RustAnalyzer { fn analyze(&self) -> u32 { 1 } }\n#[cfg(test)]\nmod tests {\n  struct PluginFake;\n  impl base::Analyzer for PluginFake { fn analyze(&self) -> u32 { 2 } }\n}\n";
const PLUGIN_TEST: &str = "struct ItFake;\nimpl base::Analyzer for ItFake { fn analyze(&self) -> u32 { 3 } }\n";
const PLUGIN_BIN: &str = "struct BinAnalyzer;\nimpl base::Analyzer for BinAnalyzer { fn analyze(&self) -> u32 { 4 } }\nfn main() {}\n";
const MID_LIB: &str = "use base::Analyzer;\npub fn make() -> plugin::RustAnalyzer { plugin::RustAnalyzer }\npub fn lib_run(analyzer: &dyn Analyzer) -> u32 { analyzer.analyze() }\n#[cfg(test)]\nmod tests {\n  use base::Analyzer;\n  fn unit_run(analyzer: &dyn Analyzer) -> u32 { analyzer.analyze() }\n}\n";
const MID_TEST: &str = "use base::Analyzer;\nfn check(analyzer: &dyn Analyzer) -> u32 { analyzer.analyze() }\n";
const APP_LIB: &str = "use base::Analyzer;\npub struct Local;\nimpl Analyzer for Local { fn analyze(&self) -> u32 { 5 } }\npub fn run(analyzer: &dyn Analyzer) -> u32 { analyzer.analyze() }\npub fn by_path(analyzer: &dyn Analyzer) -> u32 { Analyzer::analyze(analyzer) }\n";
const OTHER_LIB: &str = "pub struct OtherAnalyzer;\nimpl base::Analyzer for OtherAnalyzer { fn analyze(&self) -> u32 { 6 } }\n";

fn dispatch_workspace() -> common::Synthetic {
  let base = manifest("base", "");
  let plugin = manifest("plugin", &path_dependency("base"));
  let mid = format!(
    "{}\n[dev-dependencies]\n{}",
    manifest("mid", &(path_dependency("plugin") + &path_dependency("base"))),
    path_dependency("other")
  );
  let app = manifest("app", &(path_dependency("base") + &path_dependency("mid")));
  let other = manifest("other", &path_dependency("base"));
  single(
    "ws",
    &[
      ("Cargo.toml", WORKSPACE),
      ("base/Cargo.toml", &base),
      ("base/src/lib.rs", BASE_LIB),
      ("plugin/Cargo.toml", &plugin),
      ("plugin/src/lib.rs", PLUGIN_LIB),
      ("plugin/src/bin/tool.rs", PLUGIN_BIN),
      ("plugin/tests/it.rs", PLUGIN_TEST),
      ("mid/Cargo.toml", &mid),
      ("mid/src/lib.rs", MID_LIB),
      ("mid/tests/check.rs", MID_TEST),
      ("app/Cargo.toml", &app),
      ("app/src/lib.rs", APP_LIB),
      ("other/Cargo.toml", &other),
      ("other/src/lib.rs", OTHER_LIB),
    ],
  )
}

#[test]
fn should_limit_rust_dispatch_candidates_to_the_callers_package_and_its_dependencies() {
  let synthetic = dispatch_workspace();
  let project = &synthetic.project;
  let fake = analyze_impl("base", &["tests"], "Fake");
  for caller in ["run", "boxed", "generic"] {
    let from = ws("base", &["tests"], &[caller], EntityKind::Function);
    assert_eq!(dispatch_targets(project, from, "dyn-dispatch-candidate"), vec![fake], "{caller}\n{}", project.dump());
  }
  let rust_analyzer = analyze_impl("plugin", &[], "RustAnalyzer");
  let local = analyze_impl("app", &[], "Local");
  let run = ws("app", &[], &["run"], EntityKind::Function);
  assert_eq!(
    dispatch_targets(project, run, "dyn-dispatch-candidate"),
    sorted(vec![rust_analyzer, local]),
    "{}",
    project.dump()
  );
  let by_path = ws("app", &[], &["by_path"], EntityKind::Function);
  assert_eq!(
    dispatch_targets(project, by_path, "override-dispatch-candidate"),
    sorted(vec![rust_analyzer, local]),
    "{}",
    project.dump()
  );
  let check = ws("mid/test/check", &[], &["check"], EntityKind::Function);
  let other = analyze_impl("other", &[], "OtherAnalyzer");
  assert_eq!(
    dispatch_targets(project, check, "dyn-dispatch-candidate"),
    sorted(vec![rust_analyzer, other]),
    "{}",
    project.dump()
  );
}

#[test]
fn should_admit_dev_dependencies_only_for_test_callers() {
  let synthetic = dispatch_workspace();
  let project = &synthetic.project;
  let rust_analyzer = analyze_impl("plugin", &[], "RustAnalyzer");
  let other = analyze_impl("other", &[], "OtherAnalyzer");
  let lib_run = ws("mid", &[], &["lib_run"], EntityKind::Function);
  assert_eq!(dispatch_targets(project, lib_run, "dyn-dispatch-candidate"), vec![rust_analyzer], "{}", project.dump());
  let unit_run = ws("mid", &["tests"], &["unit_run"], EntityKind::Function);
  assert_eq!(
    dispatch_targets(project, unit_run, "dyn-dispatch-candidate"),
    sorted(vec![rust_analyzer, other]),
    "{}",
    project.dump()
  );
}

#[test]
fn should_let_an_unpackaged_caller_dispatch_within_its_own_repo_excluding_other_packages_tests() {
  let synthetic = common::synthetic(&[
    (
      "mono",
      &[
        ("packages/lib/package.json", r#"{ "name": "@mono/lib", "version": "1.0.0", "main": "src/index.ts" }"#),
        (
          "packages/lib/src/index.ts",
          "export interface Store { find(): number }\nexport class MemStore implements Store { find() { return 1; } }\n",
        ),
        (
          "packages/lib/src/fake.spec.ts",
          "import { Store } from './index';\nexport class SpecStore implements Store { find() { return 2; } }\n",
        ),
        (
          "scripts/run.ts",
          "import { Store } from '../packages/lib/src/index';\nexport class LocalStore implements Store { find() { return 3; } }\nexport function go(store: Store) { return store.find(); }\n",
        ),
      ],
    ),
    ("far", &[("x.ts", "export class Far { find() { return 4; } }\n")]),
  ]);
  let project = &synthetic.project;
  let go = common::symbol(
    atlas_core::LanguageFamily::Ts,
    "mono",
    atlas_core::UNPACKAGED,
    &["scripts", "run"],
    &["go"],
    EntityKind::Function,
  );
  let mem = ts("mono", "@mono/lib", &["src", "index"], &["MemStore", "find"], EntityKind::Method);
  let local = common::symbol(
    atlas_core::LanguageFamily::Ts,
    "mono",
    atlas_core::UNPACKAGED,
    &["scripts", "run"],
    &["LocalStore", "find"],
    EntityKind::Method,
  );
  assert_eq!(
    dispatch_targets(project, go, "interface-dispatch-candidate"),
    sorted(vec![mem, local]),
    "{}",
    project.dump()
  );
}

#[test]
fn should_dispatch_self_in_a_trait_default_body_to_own_package_code_and_other_packages_non_test_implementers() {
  let synthetic = dispatch_workspace();
  let project = &synthetic.project;
  let describe = ws("base", &[], &["Analyzer", "describe"], EntityKind::Method);
  let implementers = sorted(vec![
    analyze_impl("base", &["tests"], "Fake"),
    analyze_impl("plugin", &[], "RustAnalyzer"),
    analyze_impl("plugin/bin/tool", &[], "BinAnalyzer"),
    analyze_impl("app", &[], "Local"),
    analyze_impl("other", &[], "OtherAnalyzer"),
  ]);
  assert_eq!(dispatch_targets(project, describe, "dyn-dispatch-candidate"), implementers, "{}", project.dump());
  let rank = ws("base", &[], &["Analyzer", "rank"], EntityKind::Method);
  assert_eq!(dispatch_targets(project, rank, "override-dispatch-candidate"), implementers, "{}", project.dump());
}

#[test]
fn should_limit_ts_dispatch_candidates_to_the_callers_package_and_its_dependencies() {
  let package = |name: &str, dependencies: &[&str]| {
    let dependencies: Vec<String> = dependencies.iter().map(|dependency| format!("\"{dependency}\": \"*\"")).collect();
    format!(
      r#"{{ "name": "{name}", "version": "1.0.0", "main": "src/index.ts", "dependencies": {{ {} }} }}"#,
      dependencies.join(", ")
    )
  };
  let (base, plugin, app, other) = (
    package("@ws/base", &[]),
    package("@ws/plugin", &["@ws/base"]),
    package("@ws/app", &["@ws/plugin", "@ws/base"]),
    package("@ws/other", &["@ws/base"]),
  );
  let synthetic = single(
    "mono",
    &[
      ("package.json", r#"{ "name": "root", "version": "1.0.0", "workspaces": ["packages/*"] }"#),
      ("packages/base/package.json", &base),
      (
        "packages/base/src/index.ts",
        "export interface Analyzer { analyze(): number }\nexport abstract class Base { abstract step(): number; run() { return this.step(); } }\n",
      ),
      ("packages/plugin/package.json", &plugin),
      (
        "packages/plugin/src/index.ts",
        "import { Analyzer, Base } from '@ws/base';\nexport class TsAnalyzer implements Analyzer { analyze() { return 1; } }\nexport class Impl extends Base { step() { return 1; } }\n",
      ),
      (
        "packages/plugin/src/fake.spec.ts",
        "import { Analyzer, Base } from '@ws/base';\nexport class SpecFake implements Analyzer { analyze() { return 2; } }\nexport class SpecImpl extends Base { step() { return 2; } }\n",
      ),
      ("packages/app/package.json", &app),
      (
        "packages/app/src/index.ts",
        "import { Analyzer, Base } from '@ws/base';\nexport function run(analyzer: Analyzer, base: Base) { return analyzer.analyze() + base.step(); }\n",
      ),
      ("packages/other/package.json", &other),
      (
        "packages/other/src/index.ts",
        "import { Analyzer, Base } from '@ws/base';\nexport class OtherAnalyzer implements Analyzer { analyze() { return 3; } }\nexport class Stray extends Base { step() { return 3; } }\n",
      ),
    ],
  );
  let project = &synthetic.project;
  let run = ts("mono", "@ws/app", &["src", "index"], &["run"], EntityKind::Function);
  let ts_analyzer = ts("mono", "@ws/plugin", &["src", "index"], &["TsAnalyzer", "analyze"], EntityKind::Method);
  assert_eq!(dispatch_targets(project, run, "interface-dispatch-candidate"), vec![ts_analyzer], "{}", project.dump());
  let step = |package: &str, class: &str| ts("mono", package, &["src", "index"], &[class, "step"], EntityKind::Method);
  assert_eq!(
    dispatch_targets(project, run, "override-dispatch-candidate"),
    vec![step("@ws/plugin", "Impl")],
    "{}",
    project.dump()
  );
  let template = ts("mono", "@ws/base", &["src", "index"], &["Base", "run"], EntityKind::Method);
  assert_eq!(
    dispatch_targets(project, template, "override-dispatch-candidate"),
    sorted(vec![step("@ws/plugin", "Impl"), step("@ws/other", "Stray")]),
    "{}",
    project.dump()
  );
}

fn load_owned(repo: &str, files: &[(&str, String)]) -> Synthetic {
  let borrowed: Vec<(&str, &str)> = files.iter().map(|(path, content)| (*path, content.as_str())).collect();
  single(repo, &borrowed)
}

fn assert_edit_matches_full(repo: &str, files: &[(&str, String)], edit: (&str, &str, KeyContext), dependent: &str) -> Synthetic {
  let (path, source, context) = edit;
  let mut incremental = load_owned(repo, files);
  let project = &mut incremental.project;
  let facts = common::analyze(&common::repo(repo), path, source);
  let delta = project.table.upsert_file(&facts, &context).unwrap();
  let relocations = project.table.refresh();
  let affected = project.table.affected_files(&[delta], &relocations);
  assert!(affected.contains(&(common::repo(repo), RelPath::new(dependent).unwrap())), "{affected:?}");
  let reresolved = atlas_resolve::resolve_file(project.facts(repo, dependent), &project.table).unwrap();
  let edited: Vec<(&str, String)> = files
    .iter()
    .map(|(file, content)| (*file, if *file == path { source.to_owned() } else { content.clone() }))
    .collect();
  let full = load_owned(repo, &edited);
  assert_eq!(&reresolved, full.project.result(repo, dependent));
  full
}

fn context(package: &str, module_path: &[&str]) -> KeyContext {
  KeyContext {
    package: package.to_owned(),
    module_path: module_path.iter().map(|segment| (*segment).to_owned()).collect(),
  }
}

#[test]
fn should_re_resolve_transitive_dependents_when_a_dependency_gates_an_impl_behind_cfg_test() {
  let files = vec![
    ("Cargo.toml", "[workspace]\nmembers = [\"base\", \"plugin\", \"mid\", \"app\"]\n".to_owned()),
    ("base/Cargo.toml", manifest("base", "")),
    ("base/src/lib.rs", "pub trait Analyzer { fn analyze(&self) -> u32; }\n".to_owned()),
    ("plugin/Cargo.toml", manifest("plugin", &path_dependency("base"))),
    ("plugin/src/lib.rs", "mod imp;\n".to_owned()),
    (
      "plugin/src/imp.rs",
      "pub struct RustAnalyzer;\nimpl base::Analyzer for RustAnalyzer { fn analyze(&self) -> u32 { 1 } }\n".to_owned(),
    ),
    ("mid/Cargo.toml", manifest("mid", &path_dependency("plugin"))),
    ("mid/src/lib.rs", "pub fn noop() {}\n".to_owned()),
    ("app/Cargo.toml", manifest("app", &(path_dependency("base") + &path_dependency("mid")))),
    (
      "app/src/lib.rs",
      "use base::Analyzer;\npub fn run(analyzer: &dyn Analyzer) -> u32 { analyzer.analyze() }\n".to_owned(),
    ),
  ];
  let run = ws("app", &[], &["run"], EntityKind::Function);
  let before = load_owned("ws", &files);
  assert_eq!(
    dispatch_targets(&before.project, run, "dyn-dispatch-candidate"),
    vec![analyze_impl("plugin", &["imp"], "RustAnalyzer")]
  );
  let gated = ("plugin/src/lib.rs", "#[cfg(test)]\nmod imp;\n", context("plugin", &[]));
  let full = assert_edit_matches_full("ws", &files, gated, "app/src/lib.rs");
  assert!(dispatch_targets(&full.project, run, "dyn-dispatch-candidate").is_empty(), "{}", full.project.dump());
}

#[test]
fn should_re_resolve_macro_importers_when_a_use_moves_above_its_macro_rules() {
  let files = vec![
    ("Cargo.toml", manifest("demo", "")),
    ("src/lib.rs", "mod render;\nmod text;\n".to_owned()),
    (
      "src/text.rs",
      "macro_rules! line {\n  ($out:expr) => { $out.push('\\n') };\n}\npub(crate) use line;\n".to_owned(),
    ),
    ("src/render.rs", "use crate::text::line;\npub fn render(out: &mut String) { line!(out); }\n".to_owned()),
  ];
  let render = demo(&["render"], &["render"], EntityKind::Function);
  let line = demo(&["text"], &["line"], EntityKind::Macro);
  load_owned("demo", &files).project.assert_edge(EdgeKind::References, render, line, RESOLVED);
  let reordered = (
    "src/text.rs",
    "pub(crate) use line;\nmacro_rules! line {\n  ($out:expr) => { $out.push('\\n') };\n}\n",
    context("demo", &["text"]),
  );
  let full = assert_edit_matches_full("demo", &files, reordered, "src/render.rs");
  full.project.assert_no_edge(EdgeKind::References, render, line);
}
