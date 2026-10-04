mod common;

use std::fs;

use atlas_core::{ArgSummary, ExportFact, ImportBinding, ImportFact, LocalSymbolId, ReExportBinding, Span, TemplatePart, Visibility};
use common::{analyze, fixtures_root, symbol_names};

fn named(imported: &str, local: &str, type_only: bool) -> ImportBinding {
  ImportBinding::Named {
    imported: imported.into(),
    local: local.into(),
    type_only,
  }
}

fn without_spans(imports: &[ImportFact]) -> Vec<ImportFact> {
  let blank = Span::from_zero_based_rows(0, 0, 0, 0, 0, 0);
  imports
    .iter()
    .cloned()
    .map(|import| match import {
      ImportFact::TsImport {
        specifier, bindings, type_only, ..
      } => ImportFact::TsImport {
        specifier,
        bindings,
        type_only,
        span: blank,
      },
      ImportFact::TsRequire { specifier, bindings, scope, .. } => ImportFact::TsRequire {
        specifier,
        bindings,
        scope,
        span: blank,
      },
      ImportFact::TsDynamicImport { specifier, scope, .. } => ImportFact::TsDynamicImport { specifier, scope, span: blank },
      other => other,
    })
    .collect()
}

#[test]
fn should_record_every_import_form() {
  let source = r#"
import Default, { named, other as alias, type Shape } from './a';
import * as ns from './b';
import type { OnlyType } from './c';
import './side-effect';
import { "string-name" as stringName } from './d';
import legacy = require('./legacy');
const plain = require('./plain');
const { left, right: renamed, nested: { deep }, withDefault = 1 } = require('./destructured');
export async function load(name: string) {
  const local = require('./inner');
  const lazy = await import('./lazy');
  return import(`./plugins/${name}`) && require(name);
}
"#;
  let facts = analyze("src/imports.ts", source);
  let blank = Span::from_zero_based_rows(0, 0, 0, 0, 0, 0);
  let load = Some(LocalSymbolId(0));
  assert_eq!(
    without_spans(&facts.imports),
    [
      ImportFact::TsImport {
        specifier: "./a".into(),
        bindings: vec![
          ImportBinding::Default { local: "Default".into() },
          named("named", "named", false),
          named("other", "alias", false),
          named("Shape", "Shape", true),
        ],
        type_only: false,
        span: blank,
      },
      ImportFact::TsImport {
        specifier: "./b".into(),
        bindings: vec![ImportBinding::Namespace { local: "ns".into() }],
        type_only: false,
        span: blank,
      },
      ImportFact::TsImport {
        specifier: "./c".into(),
        bindings: vec![named("OnlyType", "OnlyType", true)],
        type_only: true,
        span: blank,
      },
      ImportFact::TsImport {
        specifier: "./side-effect".into(),
        bindings: vec![],
        type_only: false,
        span: blank,
      },
      ImportFact::TsImport {
        specifier: "./d".into(),
        bindings: vec![named("string-name", "stringName", false)],
        type_only: false,
        span: blank,
      },
      ImportFact::TsRequire {
        specifier: ArgSummary::Str("./legacy".into()),
        bindings: vec![ImportBinding::Namespace { local: "legacy".into() }],
        scope: None,
        span: blank,
      },
      ImportFact::TsRequire {
        specifier: ArgSummary::Str("./plain".into()),
        bindings: vec![ImportBinding::Namespace { local: "plain".into() }],
        scope: None,
        span: blank,
      },
      ImportFact::TsRequire {
        specifier: ArgSummary::Str("./destructured".into()),
        bindings: vec![named("left", "left", false), named("right", "renamed", false), named("withDefault", "withDefault", false)],
        scope: None,
        span: blank,
      },
      ImportFact::TsRequire {
        specifier: ArgSummary::Str("./inner".into()),
        bindings: vec![ImportBinding::Namespace { local: "local".into() }],
        scope: load,
        span: blank,
      },
      ImportFact::TsDynamicImport {
        specifier: ArgSummary::Str("./lazy".into()),
        scope: load,
        span: blank,
      },
      ImportFact::TsDynamicImport {
        specifier: ArgSummary::Template(vec![TemplatePart::Lit("./plugins/".into()), TemplatePart::Expr(ArgSummary::Path(vec!["name".into()]))]),
        scope: load,
        span: blank,
      },
      ImportFact::TsRequire {
        specifier: ArgSummary::Path(vec!["name".into()]),
        bindings: vec![],
        scope: load,
        span: blank,
      },
    ]
  );
  assert_eq!(symbol_names(&facts), ["load#function"]);
  assert!(facts.references.iter().all(|reference| reference.name() != Some("require")));
}

#[test]
fn should_record_every_export_form() {
  let source = r#"
const a = 1;
function c() {}
export { a as b, c };
export { x as y, default, default as Z } from './x';
export type { T } from './types';
export { type U, V } from './mixed';
export * from './all';
export * as ns from './ns';
export { c as default };
"#;
  let facts = analyze("src/index.ts", source);
  let spans: Vec<Span> = facts.exports.iter().map(ExportFact::span).collect();
  let binding = |imported: &str, exported: &str, type_only: bool| ReExportBinding {
    imported: imported.into(),
    exported: exported.into(),
    type_only,
  };
  assert_eq!(
    facts.exports,
    [
      ExportFact::Named {
        local: "a".into(),
        exported: "b".into(),
        type_only: false,
        span: spans[0],
      },
      ExportFact::Named {
        local: "c".into(),
        exported: "c".into(),
        type_only: false,
        span: spans[1],
      },
      ExportFact::ReExport {
        specifier: "./x".into(),
        bindings: vec![binding("x", "y", false), binding("default", "default", false), binding("default", "Z", false)],
        type_only: false,
        span: spans[2],
      },
      ExportFact::ReExport {
        specifier: "./types".into(),
        bindings: vec![binding("T", "T", true)],
        type_only: true,
        span: spans[3],
      },
      ExportFact::ReExport {
        specifier: "./mixed".into(),
        bindings: vec![binding("U", "U", true), binding("V", "V", false)],
        type_only: false,
        span: spans[4],
      },
      ExportFact::ReExportAll {
        specifier: "./all".into(),
        namespace: None,
        type_only: false,
        span: spans[5],
      },
      ExportFact::ReExportAll {
        specifier: "./ns".into(),
        namespace: Some("ns".into()),
        type_only: false,
        span: spans[6],
      },
      ExportFact::Named {
        local: "c".into(),
        exported: "default".into(),
        type_only: false,
        span: spans[7],
      },
    ]
  );
  assert_eq!(
    facts.symbols.iter().map(|symbol| symbol.visibility.clone()).collect::<Vec<_>>(),
    [Visibility::Exported, Visibility::Exported]
  );
}

#[test]
fn should_follow_the_reexport_chain_fixture() {
  let root = fixtures_root().join("adversarial/reexport-chain-ts/src");
  let lib = analyze("src/lib/index.ts", &fs::read_to_string(root.join("lib/index.ts")).unwrap());
  assert!(matches!(
    lib.exports.as_slice(),
    [ExportFact::ReExport { specifier, bindings, .. }] if specifier == "../core" && bindings[0].imported == "computeTotal" && bindings[0].exported == "total"
  ));
  let index = analyze("src/index.ts", &fs::read_to_string(root.join("index.ts")).unwrap());
  assert!(matches!(index.exports.as_slice(), [ExportFact::ReExportAll { specifier, namespace: None, .. }] if specifier == "./lib"));
}

#[test]
fn should_record_commonjs_exports_and_exported_function_assignments() {
  let source =
    "function helper() {}\nmodule.exports = { helper, version: 2 };\nexports.handler = async (event) => process(event);\nmodule.exports.name = 'svc';\nexports[key] = 1;\n";
  let facts = analyze("src/lambda.js", source);
  let exports: Vec<(Option<String>, ArgSummary)> = facts
    .exports
    .iter()
    .map(|export| match export {
      ExportFact::CommonJs { exported, value, .. } => (exported.clone(), value.clone()),
      other => panic!("unexpected export {other:?}"),
    })
    .collect();
  assert_eq!(exports.len(), 3);
  assert_eq!(exports[1], (Some("handler".into()), ArgSummary::Closure { params: vec!["event".into()] }));
  assert_eq!(exports[2], (Some("name".into()), ArgSummary::Str("svc".into())));
  assert_eq!(symbol_names(&facts), ["helper#function", "handler#function"]);
  assert_eq!(facts.symbols[0].visibility, Visibility::Exported);
  assert_eq!(facts.symbols[1].visibility, Visibility::Exported);
  assert_eq!(common::refs(&facts), ["<module> -ValueRef-> helper", "handler -Call-> process", "<module> -ValueRef-> key"]);
  let export_equals = analyze("src/legacy.ts", "class Api {}\nexport = Api;\n");
  assert!(matches!(
    export_equals.exports.as_slice(),
    [ExportFact::CommonJs { exported: None, value: ArgSummary::Path(path), .. }] if path == &["Api"]
  ));
  assert_eq!(export_equals.symbols[0].visibility, Visibility::Exported);
}
