use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{RelPath, RepoName, RustTarget, ts_module_path};
use tempfile::TempDir;

use super::*;
use crate::discovery::discover;
use crate::registry::RepoConfig;

struct Repo {
  _dir: TempDir,
  root: PathBuf,
}

impl Repo {
  fn new() -> Self {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    Self { _dir: dir, root }
  }

  fn with(files: &[(&str, &str)]) -> Self {
    let repo = Self::new();
    for (path, content) in files {
      repo.write(path, content);
    }
    repo
  }

  fn write(&self, relative: &str, content: &str) {
    let path = self.root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
  }

  fn graph(&self) -> PackageGraph {
    graph_of(&self.root)
  }
}

fn graph_of(root: &Path) -> PackageGraph {
  let config = RepoConfig {
    name: RepoName::new("repo").unwrap(),
    path: root.to_path_buf(),
    include: Vec::new(),
    exclude: Vec::new(),
    languages: None,
  };
  let discovered = discover(&config, 1 << 20).unwrap();
  PackageGraph::load(root, &discovered.files)
}

fn rel(path: &str) -> RelPath {
  RelPath::new(path).unwrap()
}

fn dir(path: &str) -> RepoDir {
  RepoDir::new(rel(path))
}

fn workspace_root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn cargo<'a>(graph: &'a PackageGraph, name: &str) -> &'a CargoPackage {
  graph
    .packages
    .iter()
    .filter_map(PackageInfo::as_cargo)
    .find(|package| package.name == name)
    .unwrap_or_else(|| panic!("no cargo package {name}"))
}

fn npm<'a>(graph: &'a PackageGraph, name: &str) -> &'a NpmPackage {
  graph
    .packages
    .iter()
    .filter_map(PackageInfo::as_npm)
    .find(|package| package.name.as_deref() == Some(name))
    .unwrap_or_else(|| panic!("no npm package {name}"))
}

fn target_names(package: &CargoPackage) -> Vec<(RustTarget, String, String)> {
  package
    .targets
    .iter()
    .map(|target| (target.target.clone(), target.name.clone(), target.root_file.to_string()))
    .collect()
}

#[test]
fn should_parse_the_acme_billing_fixture() {
  let graph = graph_of(&workspace_root().join("fixtures/acme/acme-billing"));
  assert!(graph.warnings.is_empty(), "{:?}", graph.warnings);
  let package = cargo(&graph, "acme-billing");
  assert_eq!(package.version.as_deref(), Some("0.3.0"));
  assert_eq!(
    target_names(package),
    [
      (RustTarget::Lib, "acme_billing".to_owned(), "src/lib.rs".to_owned()),
      (RustTarget::Bin("acme-billing".to_owned()), "acme-billing".to_owned(), "src/main.rs".to_owned()),
      (RustTarget::Test("invoices".to_owned()), "invoices".to_owned(), "tests/invoices.rs".to_owned()),
    ]
  );
  assert_eq!(package.workspace_root, Some(RepoDir::root()));
  let tokio = package.dependencies.iter().find(|dep| dep.local_name == "tokio").unwrap();
  assert_eq!(
    (tokio.section, tokio.version.as_deref(), &tokio.source),
    (DependencySection::Normal, Some("1"), &CargoDependencySource::Registry)
  );
  let tower = package.dependencies.iter().find(|dep| dep.local_name == "tower").unwrap();
  assert_eq!((tower.section, tower.version.as_deref()), (DependencySection::Dev, Some("0.5")));
  assert_eq!(package.dependencies.iter().filter(|dep| dep.section == DependencySection::Normal).count(), 5);
  assert_eq!(graph.workspaces.len(), 1);
  assert!(!graph.workspaces[0].is_virtual);
  assert!(graph.workspaces[0].members.is_empty());
}

#[test]
fn should_treat_an_empty_workspace_table_as_its_own_workspace_root() {
  let repo = Repo::with(&[("Cargo.toml", "[package]\nname = \"solo\"\nversion = \"1.0.0\"\n\n[workspace]\n"), ("src/lib.rs", "")]);
  let graph = repo.graph();
  assert_eq!(cargo(&graph, "solo").workspace_root, Some(RepoDir::root()));
  assert_eq!(graph.workspaces.len(), 1);
  assert_eq!(graph.workspaces[0].root, RepoDir::root());
}

#[test]
fn should_parse_this_workspaces_own_root_manifest() {
  let graph = graph_of(&workspace_root());
  let workspace = graph.workspaces.iter().find(|workspace| workspace.root.is_root()).expect("root workspace");
  assert!(workspace.is_virtual);
  assert!(workspace.member_patterns.contains(&"crates/atlas-core".to_owned()));
  assert!(workspace.members.contains(&dir("crates/atlas-core")));
  assert!(workspace.members.contains(&dir("crates/atlas-project")));
  assert!(!workspace.members.iter().any(|member| member.as_str().starts_with("fixtures")));

  let core = cargo(&graph, "atlas-core");
  assert_eq!(core.version.as_deref(), Some("0.1.0"));
  assert_eq!(core.workspace_root, Some(RepoDir::root()));
  assert_eq!(core.lib().map(|lib| lib.name.as_str()), Some("atlas_core"));
  let blake3 = core.dependencies.iter().find(|dep| dep.local_name == "blake3").unwrap();
  assert_eq!((blake3.version.as_deref(), &blake3.source), (Some("1"), &CargoDependencySource::Registry));

  let project = cargo(&graph, "atlas-project");
  let core_dep = project.dependencies.iter().find(|dep| dep.local_name == "atlas-core").unwrap();
  assert_eq!(
    core_dep.source,
    CargoDependencySource::Path {
      raw: "crates/atlas-core".to_owned(),
      resolved: Some(dir("crates/atlas-core"))
    }
  );
  assert_eq!(core_dep.extern_crate_name(), "atlas_core");

  let billing = cargo(&graph, "acme-billing");
  assert_eq!(billing.workspace_root, Some(dir("fixtures/acme/acme-billing")));
  let owning = graph.package_for_file(&rel("fixtures/acme/acme-billing/src/lib.rs")).unwrap();
  assert_eq!(owning.name(), Some("acme-billing"));
  assert_eq!(graph.package_for_file(&rel("crates/atlas-core/src/ids.rs")).unwrap().name(), Some("atlas-core"));
}

#[test]
fn should_read_the_lib_rename_and_hyphen_to_underscore_names() {
  let repo = Repo::with(&[
    ("a/Cargo.toml", "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n"),
    ("a/src/lib.rs", ""),
    (
      "b/Cargo.toml",
      "[package]\nname = \"orig\"\nversion = \"0.1.0\"\n[lib]\nname = \"renamed\"\npath = \"lib/entry.rs\"\n",
    ),
    ("b/lib/entry.rs", ""),
  ]);
  let graph = repo.graph();
  let a = cargo(&graph, "my-crate");
  assert_eq!(a.lib().map(|lib| (lib.name.as_str(), lib.root_file.as_str())), Some(("my_crate", "a/src/lib.rs")));
  let b = cargo(&graph, "orig");
  assert_eq!(b.lib().map(|lib| (lib.name.as_str(), lib.root_file.as_str())), Some(("renamed", "b/lib/entry.rs")));
}

#[test]
fn should_discover_explicit_and_automatic_targets() {
  let manifest = r#"
[package]
name = "multi"
version = "0.1.0"

[[bin]]
name = "custom"
path = "tools/custom_main.rs"

[[test]]
name = "integration"

[[example]]
name = "demo"
path = "examples/demo/main.rs"

[[bench]]
name = "speed"
"#;
  let repo = Repo::with(&[
    ("Cargo.toml", manifest),
    ("build.rs", ""),
    ("src/lib.rs", ""),
    ("src/main.rs", ""),
    ("src/bin/tool.rs", ""),
    ("src/bin/multi_dir/main.rs", ""),
    ("src/bin/multi_dir/helper.rs", ""),
    ("tools/custom_main.rs", ""),
    ("tests/integration.rs", ""),
    ("tests/other.rs", ""),
    ("tests/nested/main.rs", ""),
    ("examples/demo/main.rs", ""),
    ("examples/solo.rs", ""),
    ("benches/speed.rs", ""),
  ]);
  let graph = repo.graph();
  let mut actual = target_names(cargo(&graph, "multi"));
  actual.sort_by(|a, b| a.2.cmp(&b.2));
  let expected: Vec<(RustTarget, String, String)> = vec![
    (RustTarget::BuildScript, "build-script-build", "build.rs"),
    (RustTarget::Bench("speed".into()), "speed", "benches/speed.rs"),
    (RustTarget::Example("demo".into()), "demo", "examples/demo/main.rs"),
    (RustTarget::Example("solo".into()), "solo", "examples/solo.rs"),
    (RustTarget::Lib, "multi", "src/lib.rs"),
    (RustTarget::Bin("multi".into()), "multi", "src/main.rs"),
    (RustTarget::Bin("multi_dir".into()), "multi_dir", "src/bin/multi_dir/main.rs"),
    (RustTarget::Bin("tool".into()), "tool", "src/bin/tool.rs"),
    (RustTarget::Test("integration".into()), "integration", "tests/integration.rs"),
    (RustTarget::Test("nested".into()), "nested", "tests/nested/main.rs"),
    (RustTarget::Test("other".into()), "other", "tests/other.rs"),
    (RustTarget::Bin("custom".into()), "custom", "tools/custom_main.rs"),
  ]
  .into_iter()
  .map(|(target, name, path)| (target, name.to_owned(), path.to_owned()))
  .collect();
  let mut expected = expected;
  expected.sort_by(|a, b| a.2.cmp(&b.2));
  assert_eq!(actual, expected);
}

#[test]
fn should_honour_auto_discovery_opt_outs_and_a_disabled_build_script() {
  let repo = Repo::with(&[
    (
      "Cargo.toml",
      "[package]\nname = \"p\"\nversion = \"0.1.0\"\nautobins = false\nautotests = false\nbuild = false\n",
    ),
    ("src/lib.rs", ""),
    ("src/main.rs", ""),
    ("src/bin/x.rs", ""),
    ("tests/t.rs", ""),
    ("build.rs", ""),
  ]);
  let graph = repo.graph();
  assert_eq!(target_names(cargo(&graph, "p")), [(RustTarget::Lib, "p".to_owned(), "src/lib.rs".to_owned())]);
}

#[test]
fn should_read_all_dependency_sections_and_renames() {
  let manifest = r#"
[package]
name = "deps"
version = "0.1.0"

[dependencies]
plain = "1.2"
foo = { package = "bar", version = "2" }
local = { path = "../local" }
outside = { path = "../../../elsewhere" }
fromgit = { git = "https://example.com/x.git" }
maybe = { version = "1", optional = true }

[dev-dependencies]
devonly = "3"

[build-dependencies]
builder = "4"

[target.'cfg(unix)'.dependencies]
unixy = "5"
"#;
  let repo = Repo::with(&[("crates/deps/Cargo.toml", manifest), ("crates/deps/src/lib.rs", "")]);
  let graph = repo.graph();
  let deps = &cargo(&graph, "deps").dependencies;
  let get = |name: &str| deps.iter().find(|dep| dep.local_name == name).unwrap_or_else(|| panic!("{name}"));
  assert_eq!(deps.len(), 9);
  assert_eq!(
    (get("foo").local_name.as_str(), get("foo").package_name.as_str(), get("foo").extern_crate_name().as_str()),
    ("foo", "bar", "foo")
  );
  assert_eq!(get("plain").package_name, "plain");
  assert_eq!(
    get("local").source,
    CargoDependencySource::Path {
      raw: "../local".into(),
      resolved: Some(dir("crates/local"))
    }
  );
  assert_eq!(
    get("outside").source,
    CargoDependencySource::Path {
      raw: "../../../elsewhere".into(),
      resolved: None
    }
  );
  assert_eq!(
    get("fromgit").source,
    CargoDependencySource::Git {
      url: "https://example.com/x.git".into()
    }
  );
  assert!(get("maybe").optional);
  assert_eq!(get("devonly").section, DependencySection::Dev);
  assert_eq!(get("builder").section, DependencySection::Build);
  assert_eq!(get("unixy").target_cfg.as_deref(), Some("cfg(unix)"));
}

#[test]
fn should_resolve_workspace_inherited_dependencies_including_renames_and_paths() {
  let root = r#"
[workspace]
members = ["crates/*"]
exclude = ["crates/skipped"]

[workspace.package]
version = "9.9.9"

[workspace.dependencies]
shared = { path = "crates/shared" }
serde = { version = "1", features = ["derive"] }
aliased = { package = "real-name", version = "0.3" }
"#;
  let member = r#"
[package]
name = "app"
version = { workspace = true }

[dependencies]
shared.workspace = true
serde = { workspace = true, optional = true }
aliased.workspace = true
missing.workspace = true
"#;
  let repo = Repo::with(&[
    ("Cargo.toml", root),
    ("crates/app/Cargo.toml", member),
    ("crates/app/src/lib.rs", ""),
    ("crates/shared/Cargo.toml", "[package]\nname = \"shared\"\nversion = \"0.0.1\"\n"),
    ("crates/skipped/Cargo.toml", "[package]\nname = \"skipped\"\nversion = \"0.0.1\"\n"),
  ]);
  let graph = repo.graph();
  let app = cargo(&graph, "app");
  assert_eq!(app.version.as_deref(), Some("9.9.9"));
  assert_eq!(app.workspace_root, Some(RepoDir::root()));
  let get = |name: &str| app.dependencies.iter().find(|dep| dep.local_name == name).unwrap();
  assert_eq!(
    get("shared").source,
    CargoDependencySource::Path {
      raw: "crates/shared".into(),
      resolved: Some(dir("crates/shared"))
    }
  );
  assert_eq!((get("serde").version.as_deref(), get("serde").optional), (Some("1"), true));
  assert_eq!((get("aliased").package_name.as_str(), get("aliased").version.as_deref()), ("real-name", Some("0.3")));
  assert_eq!(get("missing").source, CargoDependencySource::UnresolvedWorkspace);
  assert_eq!(graph.warnings.len(), 1);
  assert!(graph.warnings[0].error.contains("missing"));
  assert_eq!(graph.warnings[0].file, rel("crates/app/Cargo.toml"));

  let workspace = &graph.workspaces[0];
  assert!(workspace.is_virtual);
  assert_eq!(workspace.members, [dir("crates/app"), dir("crates/shared")]);
  assert_eq!(workspace.exclude_patterns, ["crates/skipped"]);
  assert_eq!(cargo(&graph, "skipped").workspace_root, None);
}

#[test]
fn should_warn_about_a_literal_workspace_member_without_a_manifest() {
  let repo = Repo::with(&[
    ("Cargo.toml", "[workspace]\nmembers = [\"present\", \"absent\", \"globbed/*\"]\n"),
    ("present/Cargo.toml", "[package]\nname = \"present\"\nversion = \"1.0.0\"\n"),
  ]);
  let graph = repo.graph();
  assert_eq!(graph.warnings.len(), 1);
  assert!(graph.warnings[0].error.contains("absent"));
}

#[test]
fn should_attach_packages_to_the_nearest_enclosing_workspace() {
  let repo = Repo::with(&[
    ("Cargo.toml", "[workspace]\nmembers = [\"inner\", \"flat\"]\n"),
    ("flat/Cargo.toml", "[package]\nname = \"flat\"\nversion = \"1.0.0\"\n"),
    ("inner/Cargo.toml", "[workspace]\nmembers = [\"leaf\"]\n"),
    ("inner/leaf/Cargo.toml", "[package]\nname = \"leaf\"\nversion = \"1.0.0\"\n"),
  ]);
  let graph = repo.graph();
  assert_eq!(cargo(&graph, "leaf").workspace_root, Some(dir("inner")));
  assert_eq!(cargo(&graph, "flat").workspace_root, Some(RepoDir::root()));
}

#[test]
fn should_record_a_warning_for_malformed_manifests_and_keep_loading_the_rest() {
  let repo = Repo::with(&[
    ("bad/Cargo.toml", "[package\nname ="),
    ("good/Cargo.toml", "[package]\nname = \"good\"\nversion = \"1.0.0\"\n"),
    ("empty/Cargo.toml", "# nothing\n"),
    ("badjson/package.json", "{ not json"),
    ("goodjson/package.json", "{\"name\": \"fine\"}"),
    ("arrayjson/package.json", "[]"),
    ("bad-ts/tsconfig.json", "{ \"a\": "),
  ]);
  let graph = repo.graph();
  assert_eq!(graph.packages.len(), 2);
  let flagged: Vec<_> = graph.warnings.iter().map(|warning| warning.file.as_str()).collect();
  assert_eq!(
    flagged,
    [
      "arrayjson/package.json",
      "bad-ts/tsconfig.json",
      "bad/Cargo.toml",
      "badjson/package.json",
      "empty/Cargo.toml"
    ]
  );
  assert!(graph.warnings.iter().all(|warning| !warning.error.is_empty()));
}

#[test]
fn should_parse_npm_fields_and_dependency_specs() {
  let package = r#"{
    "name": "@acme/lib", "version": "1.0.0", "main": "dist/index.js", "module": "dist/index.mjs", "types": "dist/index.d.ts",
    "dependencies": {"b": "^1.0.0", "a": "file:../a", "c": "workspace:*", "d": "link:../d", "e": "latest"},
    "devDependencies": {"f": "~2"},
    "peerDependencies": {"g": ">=1"}
  }"#;
  let repo = Repo::with(&[("package.json", package)]);
  let graph = repo.graph();
  let lib = npm(&graph, "@acme/lib");
  assert_eq!(
    (lib.version.as_deref(), lib.main.as_deref(), lib.module.as_deref(), lib.types.as_deref()),
    (Some("1.0.0"), Some("dist/index.js"), Some("dist/index.mjs"), Some("dist/index.d.ts"))
  );
  let summary: Vec<_> = lib.dependencies.iter().map(|dep| (dep.name.as_str(), dep.section, dep.spec.clone())).collect();
  assert_eq!(
    summary,
    [
      ("a", DependencySection::Normal, NpmSpec::File("../a".into())),
      ("b", DependencySection::Normal, NpmSpec::Semver("^1.0.0".into())),
      ("c", DependencySection::Normal, NpmSpec::Workspace("*".into())),
      ("d", DependencySection::Normal, NpmSpec::Link("../d".into())),
      ("e", DependencySection::Normal, NpmSpec::Other("latest".into())),
      ("f", DependencySection::Dev, NpmSpec::Semver("~2".into())),
      ("g", DependencySection::Peer, NpmSpec::Semver(">=1".into())),
    ]
  );
}

#[test]
fn should_parse_npm_exports_in_every_form() {
  let cases: [(&str, Option<NpmExports>); 6] = [
    (r#""exports": "./index.js""#, Some(NpmExports::Root(NpmExportTarget::Path("./index.js".into())))),
    (
      r#""exports": {"import": "./esm.js", "require": "./cjs.js"}"#,
      Some(NpmExports::Root(NpmExportTarget::Conditions(vec![
        ("import".into(), NpmExportTarget::Path("./esm.js".into())),
        ("require".into(), NpmExportTarget::Path("./cjs.js".into())),
      ]))),
    ),
    (
      r#""exports": {".": {"types": "./a.d.ts", "node": {"import": "./n.mjs", "default": "./n.js"}, "default": "./d.js"}, "./sub/*": "./src/*.js", "./private": null}"#,
      Some(NpmExports::Subpaths(vec![
        (
          ".".into(),
          NpmExportTarget::Conditions(vec![
            ("types".into(), NpmExportTarget::Path("./a.d.ts".into())),
            (
              "node".into(),
              NpmExportTarget::Conditions(vec![
                ("import".into(), NpmExportTarget::Path("./n.mjs".into())),
                ("default".into(), NpmExportTarget::Path("./n.js".into())),
              ]),
            ),
            ("default".into(), NpmExportTarget::Path("./d.js".into())),
          ]),
        ),
        ("./sub/*".into(), NpmExportTarget::Path("./src/*.js".into())),
        ("./private".into(), NpmExportTarget::Blocked),
      ])),
    ),
    (
      r#""exports": ["./first.js", {"default": "./second.js"}]"#,
      Some(NpmExports::Root(NpmExportTarget::Fallbacks(vec![
        NpmExportTarget::Path("./first.js".into()),
        NpmExportTarget::Conditions(vec![("default".into(), NpmExportTarget::Path("./second.js".into()))]),
      ]))),
    ),
    (r#""exports": {".": "./a.js", "import": "./b.js"}"#, None),
    (r#""exports": {".": 5}"#, None),
  ];
  for (field, expected) in cases {
    let repo = Repo::with(&[("package.json", &format!("{{\"name\": \"p\", {field}}}"))]);
    let graph = repo.graph();
    assert_eq!(npm(&graph, "p").exports, expected, "{field}");
    assert_eq!(graph.warnings.len(), usize::from(expected.is_none()), "{field}");
  }
}

#[test]
fn should_keep_export_condition_order() {
  let repo = Repo::with(&[("package.json", r#"{"name": "p", "exports": {"zeta": "./z.js", "alpha": "./a.js", "default": "./d.js"}}"#)]);
  let graph = repo.graph();
  let Some(NpmExports::Root(NpmExportTarget::Conditions(conditions))) = &npm(&graph, "p").exports else {
    panic!("expected conditions")
  };
  assert_eq!(conditions.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(), ["zeta", "alpha", "default"]);
}

#[test]
fn should_read_npm_workspaces_in_array_and_object_forms_with_globs_and_negation() {
  let repo = Repo::with(&[
    (
      "package.json",
      r#"{"name": "root", "private": true, "workspaces": ["packages/*", "!packages/legacy", "tools/cli"]}"#,
    ),
    ("packages/a/package.json", r#"{"name": "a"}"#),
    ("packages/legacy/package.json", r#"{"name": "legacy"}"#),
    ("packages/a/nested/package.json", r#"{"name": "too-deep"}"#),
    ("tools/cli/package.json", r#"{"name": "cli"}"#),
    (
      "other/object/package.json",
      r#"{"name": "obj-root", "workspaces": {"packages": ["apps/**"], "nohoist": ["x"]}}"#,
    ),
    ("other/object/apps/web/package.json", r#"{"name": "web"}"#),
    ("other/object/apps/web/deep/package.json", r#"{"name": "web-deep"}"#),
  ]);
  let graph = repo.graph();
  let root = graph.workspaces.iter().find(|workspace| workspace.root.is_root()).unwrap();
  assert_eq!(root.members, [dir("packages/a"), dir("tools/cli")]);
  assert_eq!(root.exclude_patterns, ["packages/legacy"]);
  let object = graph.workspaces.iter().find(|workspace| workspace.root == dir("other/object")).unwrap();
  assert_eq!(object.members, [dir("other/object/apps/web"), dir("other/object/apps/web/deep")]);
  assert_eq!(npm(&graph, "a").workspace_root, Some(RepoDir::root()));
  assert_eq!(npm(&graph, "legacy").workspace_root, None);
  assert_eq!(npm(&graph, "web-deep").workspace_root, Some(dir("other/object")));
}

#[test]
fn should_parse_the_acme_npm_fixtures() {
  let graph = graph_of(&workspace_root().join("fixtures/acme"));
  let web = npm(&graph, "acme-web");
  let sdk = web.dependencies.iter().find(|dep| dep.name == "@acme/sdk").unwrap();
  assert_eq!(sdk.spec, NpmSpec::File("../acme-sdk".into()));
  let sdk_package = npm(&graph, "@acme/sdk");
  assert_eq!(
    (sdk_package.main.as_deref(), sdk_package.types.as_deref()),
    (Some("dist/index.js"), Some("dist/index.d.ts"))
  );
  assert!(
    graph
      .tsconfigs
      .iter()
      .any(|config| config.path == rel("acme-web/tsconfig.json") && config.include == ["acme-web/src"])
  );
}

fn tsconfig<'a>(graph: &'a PackageGraph, path: &str) -> &'a TsConfigInfo {
  graph
    .tsconfigs
    .iter()
    .find(|config| config.path.as_str() == path)
    .unwrap_or_else(|| panic!("no tsconfig {path}"))
}

#[test]
fn should_parse_a_jsonc_tsconfig_with_comments_urls_and_trailing_commas() {
  let text = r#"{
    // compiler settings
    "compilerOptions": {
      "baseUrl": "./src", /* where imports start */
      "rootDir": "src",
      "paths": {
        "@app/*": ["app/*",],
        "@url": ["https://example.com//not-a-comment"],
        "@quote": ["say \"//\""]
      },
    },
    "include": ["src/**/*", "types"],
  }"#;
  let repo = Repo::with(&[("web/tsconfig.json", text)]);
  let graph = repo.graph();
  assert!(graph.warnings.is_empty(), "{:?}", graph.warnings);
  let config = tsconfig(&graph, "web/tsconfig.json");
  assert_eq!(config.base_url, Some(dir("web/src")));
  assert_eq!(config.root_dir, Some(dir("web/src")));
  assert_eq!(config.paths_base, Some(dir("web/src")));
  assert_eq!(config.include, ["web/src/**/*", "web/types"]);
  let paths: Vec<_> = config.paths.iter().map(|mapping| (mapping.pattern.as_str(), mapping.targets.clone())).collect();
  assert_eq!(
    paths,
    [
      ("@app/*", vec!["app/*".to_owned()]),
      ("@quote", vec!["say \"//\"".to_owned()]),
      ("@url", vec!["https://example.com//not-a-comment".to_owned()]),
    ]
  );
}

#[test]
fn should_resolve_paths_relative_to_the_declaring_config_without_a_base_url() {
  let repo = Repo::with(&[("pkg/tsconfig.json", r#"{"compilerOptions": {"paths": {"@x/*": ["lib/*"]}}}"#)]);
  let config_graph = repo.graph();
  assert_eq!(tsconfig(&config_graph, "pkg/tsconfig.json").paths_base, Some(dir("pkg")));
  assert_eq!(tsconfig(&config_graph, "pkg/tsconfig.json").base_url, None);
}

#[test]
fn should_follow_extends_chains_with_child_overrides() {
  let repo = Repo::with(&[
    (
      "tsconfig.base.json",
      r#"{"compilerOptions": {"baseUrl": ".", "paths": {"@old/*": ["old/*"]}, "rootDir": "src"}, "include": ["src"]}"#,
    ),
    (
      "configs/mid.json",
      r#"{"extends": "../tsconfig.base.json", "compilerOptions": {"paths": {"@mid/*": ["mid/*"]}}}"#,
    ),
    ("app/tsconfig.json", r#"{"extends": "../configs/mid", "compilerOptions": {"rootDir": "lib"}}"#),
  ]);
  let graph = repo.graph();
  assert!(graph.warnings.is_empty(), "{:?}", graph.warnings);
  let app = tsconfig(&graph, "app/tsconfig.json");
  assert_eq!(app.paths.iter().map(|mapping| mapping.pattern.as_str()).collect::<Vec<_>>(), ["@mid/*"]);
  assert_eq!(app.base_url, Some(RepoDir::root()));
  assert_eq!(app.paths_base, Some(RepoDir::root()));
  assert_eq!(app.root_dir, Some(dir("app/lib")));
  assert_eq!(app.include, ["src"]);
  assert_eq!(app.extends, [repo.root.join("configs/mid.json"), repo.root.join("tsconfig.base.json")]);
}

#[test]
fn should_apply_array_extends_in_order_with_later_entries_winning() {
  let repo = Repo::with(&[
    ("a.json", r#"{"compilerOptions": {"baseUrl": "from-a", "rootDir": "ra"}}"#),
    ("b.json", r#"{"compilerOptions": {"baseUrl": "from-b"}}"#),
    ("tsconfig.json", r#"{"extends": ["./a.json", "./b"]}"#),
  ]);
  let graph = repo.graph();
  let config = tsconfig(&graph, "tsconfig.json");
  assert_eq!(config.base_url, Some(dir("from-b")));
  assert_eq!(config.root_dir, Some(dir("ra")));
}

#[test]
fn should_detect_extends_cycles_and_still_return_what_it_has() {
  let repo = Repo::with(&[
    ("a/tsconfig.json", r#"{"extends": "../b/tsconfig.json", "compilerOptions": {"rootDir": "a-root"}}"#),
    ("b/tsconfig.json", r#"{"extends": "../a/tsconfig.json", "compilerOptions": {"baseUrl": "b-base"}}"#),
    ("self/tsconfig.json", r#"{"extends": "./tsconfig.json"}"#),
  ]);
  let graph = repo.graph();
  assert_eq!(graph.tsconfigs.len(), 3);
  let a = tsconfig(&graph, "a/tsconfig.json");
  assert_eq!(a.root_dir, Some(dir("a/a-root")));
  assert_eq!(a.base_url, Some(dir("b/b-base")));
  let cycle_warnings: Vec<_> = graph
    .warnings
    .iter()
    .filter(|warning| warning.error.contains("cycle"))
    .map(|warning| warning.file.as_str())
    .collect();
  assert_eq!(cycle_warnings, ["a/tsconfig.json", "b/tsconfig.json", "self/tsconfig.json"]);
}

#[test]
fn should_record_a_warning_for_a_missing_extends_target() {
  let repo = Repo::with(&[("tsconfig.json", r#"{"extends": "./nope.json", "compilerOptions": {"baseUrl": "x"}}"#)]);
  let graph = repo.graph();
  assert_eq!(graph.warnings.len(), 1);
  assert!(graph.warnings[0].error.contains("nope.json"));
  assert_eq!(tsconfig(&graph, "tsconfig.json").base_url, Some(dir("x")));
}

#[test]
fn should_resolve_package_extends_through_node_modules() {
  let repo = Repo::with(&[
    ("node_modules/@scope/preset/tsconfig.json", r#"{"compilerOptions": {"baseUrl": "preset-base"}}"#),
    ("node_modules/other-preset/package.json", r#"{"name": "other-preset", "tsconfig": "configs/main.json"}"#),
    ("node_modules/other-preset/configs/main.json", r#"{"compilerOptions": {"rootDir": "preset-root"}}"#),
    ("node_modules/direct/base.json", r#"{"compilerOptions": {"rootDir": "direct-root"}}"#),
    ("a/tsconfig.json", r#"{"extends": "@scope/preset"}"#),
    ("b/tsconfig.json", r#"{"extends": "other-preset"}"#),
    ("c/tsconfig.json", r#"{"extends": "direct/base.json"}"#),
    ("d/tsconfig.json", r#"{"extends": "absent-preset"}"#),
  ]);
  let graph = repo.graph();
  assert_eq!(tsconfig(&graph, "a/tsconfig.json").base_url, Some(dir("node_modules/@scope/preset/preset-base")));
  assert_eq!(tsconfig(&graph, "b/tsconfig.json").root_dir, Some(dir("node_modules/other-preset/configs/preset-root")));
  assert_eq!(tsconfig(&graph, "c/tsconfig.json").root_dir, Some(dir("node_modules/direct/direct-root")));
  assert_eq!(graph.warnings.len(), 1);
  assert_eq!(graph.warnings[0].file, rel("d/tsconfig.json"));
}

#[test]
fn should_govern_files_by_the_nearest_ancestor_tsconfig_ignoring_include_globs() {
  let repo = Repo::with(&[
    ("tsconfig.json", r#"{"include": ["src"]}"#),
    ("tsconfig.build.json", "{}"),
    ("packages/web/tsconfig.json", "{}"),
    ("packages/web/jsconfig.json", "{}"),
    ("packages/lib/jsconfig.json", "{}"),
    ("packages/web/src/a.ts", ""),
    ("packages/lib/src/b.js", ""),
    ("scripts/tool.ts", ""),
  ]);
  let graph = repo.graph();
  let governing = |file: &str| graph.tsconfig_for_file(&rel(file)).map(|config| config.path.to_string());
  assert_eq!(governing("packages/web/src/a.ts").as_deref(), Some("packages/web/tsconfig.json"));
  assert_eq!(governing("packages/lib/src/b.js").as_deref(), Some("packages/lib/jsconfig.json"));
  assert_eq!(governing("scripts/tool.ts").as_deref(), Some("tsconfig.json"));
  assert_eq!(graph.tsconfigs.len(), 5);
}

#[test]
fn should_pick_the_nearest_package_root_per_language_family() {
  let repo = Repo::with(&[
    ("Cargo.toml", "[package]\nname = \"rootcrate\"\nversion = \"0.1.0\"\n"),
    ("src/lib.rs", ""),
    ("package.json", r#"{"name": "root-web"}"#),
    ("web/package.json", r#"{"name": "inner-web"}"#),
    ("web/src/app.ts", ""),
    ("web/Cargo.toml", "[package]\nname = \"inner-crate\"\nversion = \"0.1.0\"\n"),
    ("web/src/native.rs", ""),
    ("scripts/tool.ts", ""),
    ("README.md", ""),
  ]);
  let graph = repo.graph();
  let name = |file: &str| graph.package_for_file(&rel(file)).and_then(PackageInfo::name).map(str::to_owned);
  assert_eq!(name("web/src/app.ts").as_deref(), Some("inner-web"));
  assert_eq!(name("web/src/native.rs").as_deref(), Some("inner-crate"));
  assert_eq!(name("scripts/tool.ts").as_deref(), Some("root-web"));
  assert_eq!(name("src/lib.rs").as_deref(), Some("rootcrate"));
}

#[test]
fn should_feed_ts_module_path_from_the_package_root() {
  let repo = Repo::with(&[("pkgs/ui/package.json", r#"{"name": "ui"}"#), ("pkgs/ui/src/button/index.tsx", "")]);
  let graph = repo.graph();
  let file = rel("pkgs/ui/src/button/index.tsx");
  let package = graph.package_for_file(&file).unwrap();
  assert_eq!(
    ts_module_path(&file, package.root.as_rel_path()),
    Some(vec!["src".to_owned(), "button".to_owned(), "index".to_owned()])
  );
}

fn target(kind: RustTarget, root: &str) -> RustTargetInfo {
  RustTargetInfo {
    target: kind,
    name: String::new(),
    root_file: rel(root),
  }
}

#[test]
fn should_compute_default_rust_module_paths_from_the_file_layout() {
  let lib = target(RustTarget::Lib, "crates/x/src/lib.rs");
  let cases: [(&str, Option<Vec<&str>>); 7] = [
    ("crates/x/src/lib.rs", Some(vec![])),
    ("crates/x/src/a.rs", Some(vec!["a"])),
    ("crates/x/src/a/b.rs", Some(vec!["a", "b"])),
    ("crates/x/src/a/mod.rs", Some(vec!["a"])),
    ("crates/x/src/a/b/mod.rs", Some(vec!["a", "b"])),
    ("crates/x/other/c.rs", None),
    ("crates/x/src/readme.md", None),
  ];
  for (file, expected) in cases {
    let expected = expected.map(|segments| segments.into_iter().map(str::to_owned).collect::<Vec<_>>());
    assert_eq!(default_rust_module_path(&lib, &rel(file)), expected, "{file}");
  }
  let main = target(RustTarget::Bin("x".into()), "src/main.rs");
  assert_eq!(default_rust_module_path(&main, &rel("src/main.rs")), Some(vec![]));
  assert_eq!(default_rust_module_path(&main, &rel("src/cli/args.rs")), Some(vec!["cli".to_owned(), "args".to_owned()]));
  let bin = target(RustTarget::Bin("tool".into()), "src/bin/tool.rs");
  assert_eq!(default_rust_module_path(&bin, &rel("src/bin/tool.rs")), Some(vec![]));
}

#[test]
fn should_report_which_rust_targets_a_file_belongs_to() {
  let repo = Repo::with(&[
    ("Cargo.toml", "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n"),
    ("build.rs", ""),
    ("src/lib.rs", ""),
    ("src/main.rs", ""),
    ("src/shared/util.rs", ""),
    ("src/bin/tool.rs", ""),
    ("src/bin/multi/main.rs", ""),
    ("src/bin/multi/parts/one.rs", ""),
    ("tests/api.rs", ""),
    ("tests/common/mod.rs", ""),
  ]);
  let graph = repo.graph();
  let targets = |file: &str| -> Vec<(RustTarget, Vec<String>)> { graph.rust_targets_for_file(&rel(file)).into_iter().map(|t| (t.target, t.module_path)).collect() };
  let strings = |segments: &[&str]| segments.iter().map(|segment| (*segment).to_owned()).collect::<Vec<_>>();

  assert_eq!(targets("src/lib.rs"), [(RustTarget::Lib, vec![])]);
  assert_eq!(targets("src/main.rs"), [(RustTarget::Bin("demo".into()), vec![])]);
  assert_eq!(targets("build.rs"), [(RustTarget::BuildScript, vec![])]);
  assert_eq!(targets("src/bin/tool.rs"), [(RustTarget::Bin("tool".into()), vec![])]);
  assert_eq!(
    targets("src/shared/util.rs"),
    [
      (RustTarget::Lib, strings(&["shared", "util"])),
      (RustTarget::Bin("demo".into()), strings(&["shared", "util"]))
    ]
  );
  assert_eq!(targets("src/bin/multi/parts/one.rs"), [(RustTarget::Bin("multi".into()), strings(&["parts", "one"]))]);
  assert_eq!(targets("tests/api.rs"), [(RustTarget::Test("api".into()), vec![])]);
  assert_eq!(targets("tests/common/mod.rs"), [(RustTarget::Test("api".into()), strings(&["common"]))]);
  assert!(graph.rust_targets_for_file(&rel("docs/guide.rs")).is_empty());
}
