mod common;

use atlas_core::{Certainty, Confidence, EdgeKind, EntityKind};
use common::{Synthetic, file, single, ts};

const ROOT_PACKAGE: &str = r#"{ "name": "root", "private": true, "workspaces": ["packages/*"] }"#;
const APP_PACKAGE: &str = r#"{ "name": "@scope/app", "version": "1.0.0", "dependencies": { "@scope/core": "*" } }"#;
const DIST_EXPORTS: &str =
  r#"{ "name": "@scope/core", "exports": { ".": { "types": "./dist/index.d.ts", "import": "./dist/index.js" }, "./*": { "types": "./dist/*.d.ts", "import": "./dist/*.js" } } }"#;
const OUT_DIR_SRC: &str = r#"{ "compilerOptions": { "outDir": "dist", "rootDir": "src" }, "include": ["src"] }"#;
const DECLARING: &str = r#"{ "compilerOptions": { "outDir": "dist", "rootDir": "src", "declaration": true }, "include": ["src"] }"#;
const HELLO: &str = "export function hello() {}\n";
const MAIN: &str = "packages/app/src/main.ts";

fn workspace(files: &[(&str, &str)], specifier: &str) -> Synthetic {
  let main = format!("import {{ hello }} from '{specifier}';\nexport function main() {{ hello(); }}\n");
  let mut all: Vec<(&str, &str)> = vec![("package.json", ROOT_PACKAGE), ("packages/app/package.json", APP_PACKAGE), (MAIN, &main)];
  all.extend_from_slice(files);
  single("mono", &all)
}

fn import_of(synthetic: &Synthetic, target: &str) -> Option<Certainty> {
  let edges = synthetic.project.edges_between(EdgeKind::Imports, file("mono", MAIN), file("mono", target));
  match edges.as_slice() {
    [] => None,
    [edge] => Some(edge.certainty.clone()),
    _ => panic!("several imports edges to {target}: {edges:#?}"),
  }
}

fn dist_to_src() -> Option<Certainty> {
  Some(Certainty::heuristic(Confidence::new(0.85).unwrap(), "dist-to-src"))
}

fn core_with(tsconfigs: &[(&str, &str)], sources: &[&str]) -> Vec<(String, String)> {
  let mut files = vec![("packages/core/package.json".to_owned(), DIST_EXPORTS.to_owned())];
  files.extend(tsconfigs.iter().map(|(path, content)| (format!("packages/core/{path}"), (*content).to_owned())));
  files.extend(sources.iter().map(|path| (format!("packages/core/{path}"), HELLO.to_owned())));
  files
}

fn load(files: &[(String, String)], specifier: &str) -> Synthetic {
  let borrowed: Vec<(&str, &str)> = files.iter().map(|(path, content)| (path.as_str(), content.as_str())).collect();
  workspace(&borrowed, specifier)
}

#[test]
fn should_resolve_a_workspace_package_exactly_through_a_declared_out_dir_and_root_dir() {
  let synthetic = load(&core_with(&[("tsconfig.json", OUT_DIR_SRC)], &["src/index.ts"]), "@scope/core");
  assert_eq!(import_of(&synthetic, "packages/core/src/index.ts"), Some(Certainty::Resolved));
  let main = ts("mono", "@scope/app", &["src", "main"], &["main"], EntityKind::Function);
  let hello = ts("mono", "@scope/core", &["src", "index"], &["hello"], EntityKind::Function);
  synthetic.project.assert_certainty(EdgeKind::Calls, main, hello, &Certainty::Resolved);
}

#[test]
fn should_keep_the_dist_to_src_guess_when_no_tsconfig_declares_the_mapping() {
  let synthetic = load(&core_with(&[], &["src/index.ts"]), "@scope/core");
  assert_eq!(import_of(&synthetic, "packages/core/src/index.ts"), dist_to_src());
}

#[test]
fn should_resolve_deep_imports_through_subpath_patterns_and_build_paths() {
  let patterned = load(&core_with(&[("tsconfig.json", OUT_DIR_SRC)], &["src/util/strings.ts"]), "@scope/core/util/strings");
  assert_eq!(import_of(&patterned, "packages/core/src/util/strings.ts"), Some(Certainty::Resolved));

  let into_dist = workspace(
    &[
      ("packages/core/package.json", r#"{ "name": "@scope/core", "main": "dist/index.js" }"#),
      ("packages/core/tsconfig.json", OUT_DIR_SRC),
      ("packages/core/src/util/strings.ts", HELLO),
    ],
    "@scope/core/dist/util/strings",
  );
  assert_eq!(import_of(&into_dist, "packages/core/src/util/strings.ts"), Some(Certainty::Resolved));
}

#[test]
fn should_follow_extends_chains_relative_to_the_declaring_config() {
  let local_base = load(
    &core_with(
      &[("tsconfig.base.json", OUT_DIR_SRC), ("tsconfig.json", r#"{ "extends": "./tsconfig.base.json" }"#)],
      &["src/index.ts"],
    ),
    "@scope/core",
  );
  assert_eq!(import_of(&local_base, "packages/core/src/index.ts"), Some(Certainty::Resolved));

  let mut config_dir = core_with(&[("tsconfig.json", r#"{ "extends": "../../tsconfig.base.json" }"#)], &["src/index.ts"]);
  config_dir.push((
    "tsconfig.base.json".to_owned(),
    r#"{ "compilerOptions": { "outDir": "${configDir}/dist", "rootDir": "${configDir}/src" } }"#.to_owned(),
  ));
  assert_eq!(import_of(&load(&config_dir, "@scope/core"), "packages/core/src/index.ts"), Some(Certainty::Resolved));

  let mut repo_root = core_with(&[("tsconfig.json", r#"{ "extends": "../../tsconfig.base.json" }"#)], &["src/index.ts"]);
  repo_root.push((
    "tsconfig.base.json".to_owned(),
    r#"{ "compilerOptions": { "outDir": "dist", "rootDir": "src" } }"#.to_owned(),
  ));
  assert_eq!(import_of(&load(&repo_root, "@scope/core"), "packages/core/src/index.ts"), dist_to_src());
}

#[test]
fn should_derive_an_omitted_root_dir_from_include_or_composite() {
  let globbed = load(
    &core_with(
      &[("tsconfig.json", r#"{ "compilerOptions": { "outDir": "dist" }, "include": ["src/**/*.ts"] }"#)],
      &["src/index.ts"],
    ),
    "@scope/core",
  );
  assert_eq!(import_of(&globbed, "packages/core/src/index.ts"), Some(Certainty::Resolved));

  let spanning = workspace(
    &[
      ("packages/core/package.json", r#"{ "name": "@scope/core", "main": "dist/src/index.js" }"#),
      ("packages/core/tsconfig.json", r#"{ "compilerOptions": { "outDir": "dist" }, "include": ["src", "test"] }"#),
      ("packages/core/src/index.ts", HELLO),
      ("packages/core/test/index.test.ts", "export {};\n"),
    ],
    "@scope/core",
  );
  assert_eq!(import_of(&spanning, "packages/core/src/index.ts"), Some(Certainty::Resolved));

  let composite = load(
    &core_with(
      &[("tsconfig.json", r#"{ "compilerOptions": { "outDir": "dist", "composite": true }, "include": ["src"] }"#)],
      &["src/index.ts"],
    ),
    "@scope/core",
  );
  assert_eq!(import_of(&composite, "packages/core/src/index.ts"), dist_to_src());

  let undeterminable = load(
    &core_with(&[("tsconfig.json", r#"{ "compilerOptions": { "outDir": "dist" } }"#)], &["src/index.ts"]),
    "@scope/core",
  );
  assert_eq!(import_of(&undeterminable, "packages/core/src/index.ts"), dist_to_src());
}

#[test]
fn should_stay_heuristic_when_the_declared_source_file_is_missing() {
  let synthetic = load(
    &core_with(
      &[("tsconfig.json", r#"{ "compilerOptions": { "outDir": "dist", "rootDir": "lib-src" } }"#)],
      &["src/index.ts"],
    ),
    "@scope/core",
  );
  assert_eq!(import_of(&synthetic, "packages/core/src/index.ts"), dist_to_src());
}

#[test]
fn should_stay_heuristic_when_the_out_dir_does_not_contain_the_entry() {
  let synthetic = load(
    &core_with(&[("tsconfig.json", r#"{ "compilerOptions": { "outDir": "build", "rootDir": "src" } }"#)], &["src/index.ts"]),
    "@scope/core",
  );
  assert_eq!(import_of(&synthetic, "packages/core/src/index.ts"), dist_to_src());
}

#[test]
fn should_upgrade_only_when_every_emitting_config_maps_the_entry_to_one_source() {
  let whole_package = r#"{ "compilerOptions": { "outDir": "dist", "rootDir": "." }, "include": ["."] }"#;
  let disagreeing = load(
    &core_with(&[("tsconfig.json", whole_package), ("tsconfig.build.json", OUT_DIR_SRC)], &["src/index.ts", "index.ts"]),
    "@scope/core",
  );
  assert_eq!(import_of(&disagreeing, "packages/core/src/index.ts"), dist_to_src());
  assert_eq!(import_of(&disagreeing, "packages/core/index.ts"), None);

  let one_existing = load(
    &core_with(&[("tsconfig.json", whole_package), ("tsconfig.build.json", OUT_DIR_SRC)], &["src/index.ts"]),
    "@scope/core",
  );
  assert_eq!(import_of(&one_existing, "packages/core/src/index.ts"), Some(Certainty::Resolved));

  let no_emit = r#"{ "compilerOptions": { "outDir": "dist", "rootDir": ".", "noEmit": true }, "include": ["."] }"#;
  let ignored = load(
    &core_with(&[("tsconfig.json", no_emit), ("tsconfig.build.json", OUT_DIR_SRC)], &["src/index.ts", "index.ts"]),
    "@scope/core",
  );
  assert_eq!(import_of(&ignored, "packages/core/src/index.ts"), Some(Certainty::Resolved));

  let unknown_root = r#"{ "compilerOptions": { "outDir": "dist" } }"#;
  let blocked = load(
    &core_with(&[("tsconfig.json", unknown_root), ("tsconfig.build.json", OUT_DIR_SRC)], &["src/index.ts"]),
    "@scope/core",
  );
  assert_eq!(import_of(&blocked, "packages/core/src/index.ts"), dist_to_src());
}

#[test]
fn should_map_emitted_extensions_and_directory_entries_to_sources() {
  let cases = [
    (r#"{ "name": "@scope/core", "exports": { ".": { "import": "./dist/index.mjs" } } }"#, "src/index.mts"),
    (r#"{ "name": "@scope/core", "exports": { ".": { "require": "./dist/index.cjs" } } }"#, "src/index.cts"),
    (r#"{ "name": "@scope/core", "types": "./dist/index.d.ts" }"#, "src/index.tsx"),
    (r#"{ "name": "@scope/core", "types": "./dist/index.d.mts" }"#, "src/index.mts"),
    (r#"{ "name": "@scope/core", "main": "dist/index" }"#, "src/index.ts"),
    (r#"{ "name": "@scope/core", "main": "dist" }"#, "src/index.ts"),
    (r#"{ "name": "@scope/core", "main": "dist/feature" }"#, "src/feature/index.ts"),
  ];
  for (package, source) in cases {
    let synthetic = workspace(
      &[
        ("packages/core/package.json", package),
        ("packages/core/tsconfig.json", DECLARING),
        (&format!("packages/core/{source}"), HELLO),
      ],
      "@scope/core",
    );
    assert_eq!(import_of(&synthetic, &format!("packages/core/{source}")), Some(Certainty::Resolved), "{package}");
  }
}

#[test]
fn should_resolve_source_export_conditions_exactly_without_a_tsconfig() {
  for condition in ["bun", "source"] {
    let package = format!(r#"{{ "name": "@scope/core", "exports": {{ ".": {{ "types": "./dist/index.d.ts", "{condition}": "./src/index.ts", "import": "./dist/index.js" }} }} }}"#);
    let synthetic = workspace(&[("packages/core/package.json", &package), ("packages/core/src/index.ts", HELLO)], "@scope/core");
    assert_eq!(import_of(&synthetic, "packages/core/src/index.ts"), Some(Certainty::Resolved), "{condition}");
  }
}

fn typed_core(package: &str, tsconfigs: &[(&str, &str)]) -> Synthetic {
  let mut files: Vec<(String, String)> = vec![
    ("packages/core/package.json".to_owned(), package.to_owned()),
    ("packages/core/src/index.ts".to_owned(), HELLO.to_owned()),
    ("packages/core/src/types/index.ts".to_owned(), "export type Hello = string;\n".to_owned()),
  ];
  files.extend(tsconfigs.iter().map(|(path, content)| (format!("packages/core/{path}"), (*content).to_owned())));
  load(&files, "@scope/core")
}

fn typed_imports(synthetic: &Synthetic) -> (Option<Certainty>, Option<Certainty>) {
  (import_of(synthetic, "packages/core/src/index.ts"), import_of(synthetic, "packages/core/src/types/index.ts"))
}

#[test]
fn should_map_declarations_from_the_declaration_dir_not_the_out_dir() {
  let split = r#"{ "compilerOptions": { "outDir": "dist", "declaration": true, "declarationDir": "dist/types", "rootDir": "src" }, "include": ["src"] }"#;
  for package in [
    r#"{ "name": "@scope/core", "types": "./dist/types/index.d.ts", "main": "./dist/index.js" }"#,
    r#"{ "name": "@scope/core", "exports": { ".": { "types": "./dist/types/index.d.ts", "import": "./dist/index.js" } } }"#,
  ] {
    assert_eq!(
      typed_imports(&typed_core(package, &[("tsconfig.json", split)])),
      (Some(Certainty::Resolved), None),
      "{package}"
    );
  }

  let elsewhere = r#"{ "compilerOptions": { "outDir": "dist", "declaration": true, "declarationDir": "typings", "rootDir": "src" }, "include": ["src"] }"#;
  let outside_declaration_dir = typed_core(r#"{ "name": "@scope/core", "types": "./dist/types/index.d.ts" }"#, &[("tsconfig.json", elsewhere)]);
  assert_eq!(typed_imports(&outside_declaration_dir), (None, dist_to_src()));
}

#[test]
fn should_map_declarations_only_from_configs_that_emit_them() {
  let types_only = r#"{ "name": "@scope/core", "types": "./dist/index.d.ts" }"#;
  assert_eq!(typed_imports(&typed_core(types_only, &[("tsconfig.json", OUT_DIR_SRC)])), (dist_to_src(), None));
  let composite = r#"{ "compilerOptions": { "outDir": "dist", "rootDir": "src", "composite": true }, "include": ["src"] }"#;
  assert_eq!(typed_imports(&typed_core(types_only, &[("tsconfig.json", composite)])), (Some(Certainty::Resolved), None));
}

#[test]
fn should_not_map_scripts_from_a_declaration_only_config() {
  let declaration_only = r#"{ "compilerOptions": { "outDir": "dist", "rootDir": "src", "declaration": true, "emitDeclarationOnly": true }, "include": ["src"] }"#;
  let scripts = typed_core(r#"{ "name": "@scope/core", "main": "./dist/index.js" }"#, &[("tsconfig.json", declaration_only)]);
  assert_eq!(typed_imports(&scripts), (dist_to_src(), None));

  let package = r#"{ "name": "@scope/core", "exports": { ".": { "types": "./dist/types/index.d.ts", "import": "./dist/esm/index.js" } } }"#;
  let split_builds = typed_core(
    package,
    &[
      ("tsconfig.json", r#"{ "compilerOptions": { "noEmit": true }, "include": ["src", "test"] }"#),
      (
        "tsconfig.esm.json",
        r#"{ "extends": "./tsconfig.json", "compilerOptions": { "noEmit": false, "outDir": "dist/esm", "rootDir": "src" }, "include": ["src"] }"#,
      ),
      (
        "tsconfig.types.json",
        r#"{ "extends": "./tsconfig.json", "compilerOptions": { "noEmit": false, "emitDeclarationOnly": true, "declaration": true, "outDir": "dist/types", "rootDir": "src" }, "include": ["src"] }"#,
      ),
    ],
  );
  assert_eq!(typed_imports(&split_builds), (Some(Certainty::Resolved), None));
}
