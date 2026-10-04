mod common;

use atlas_core::{Certainty, CertaintyClass, Confidence, EdgeKind, EntityKind, UnresolvedReason};
use common::{file, single, synthetic, ts};

const RESOLVED: &[CertaintyClass] = &[CertaintyClass::Resolved];
const HEURISTIC: &[CertaintyClass] = &[CertaintyClass::Heuristic];
const PACKAGE: &str = r#"{ "name": "app", "version": "1.0.0" }"#;

fn app(module: &[&str], items: &[&str], kind: EntityKind) -> atlas_core::EntityId {
  ts("app", "app", module, items, kind)
}

#[test]
fn should_probe_extensions_index_files_tsconfig_paths_and_base_url() {
  let synthetic = single(
    "app",
    &[
      ("package.json", PACKAGE),
      (
        "tsconfig.json",
        r#"{ "compilerOptions": { "baseUrl": "src", "paths": { "@lib/*": ["lib/*", "fallback/*"], "@exact": ["lib/exact"] } } }"#,
      ),
      (
        "src/main.ts",
        r#"import { viaJs } from './util.js';
import { fromIndex } from './dir';
import { mapped } from '@lib/mapped';
import { second } from '@lib/second';
import { exact } from '@exact';
import { rooted } from 'lib/rooted';
import { readFile } from 'node:fs';
import { join } from 'path';
import React from 'react';
export function main() { viaJs(); fromIndex(); mapped(); second(); exact(); rooted(); readFile(); join(); React.createElement(); }
"#,
      ),
      ("src/util.ts", "export function viaJs() {}\n"),
      ("src/dir/index.ts", "export function fromIndex() {}\n"),
      ("src/lib/mapped.ts", "export function mapped() {}\n"),
      ("src/fallback/second.tsx", "export function second() {}\n"),
      ("src/lib/exact.ts", "export function exact() {}\n"),
      ("src/lib/rooted.ts", "export function rooted() {}\n"),
    ],
  );
  let project = &synthetic.project;
  let main = app(&["src", "main"], &["main"], EntityKind::Function);
  let targets = [
    (&["src", "util"][..], "viaJs"),
    (&["src", "dir", "index"][..], "fromIndex"),
    (&["src", "lib", "mapped"][..], "mapped"),
    (&["src", "fallback", "second"][..], "second"),
    (&["src", "lib", "exact"][..], "exact"),
    (&["src", "lib", "rooted"][..], "rooted"),
  ];
  for (module, name) in targets {
    project.assert_edge(EdgeKind::Calls, main, app(module, &[name], EntityKind::Function), RESOLVED);
  }
  project.assert_unresolved(main, "readFile", &UnresolvedReason::External { package: "node:fs".to_owned() });
  project.assert_unresolved(main, "join", &UnresolvedReason::External { package: "path".to_owned() });
  project.assert_unresolved(main, "createElement", &UnresolvedReason::External { package: "react".to_owned() });
}

#[test]
fn should_follow_named_default_namespace_and_commonjs_bindings() {
  let synthetic = single(
    "app",
    &[
      ("package.json", PACKAGE),
      (
        "src/main.ts",
        r#"import def, { renamed as local } from './exports';
import * as all from './exports';
import { ns } from './exports';
const cjs = require('./cjs');
const { named } = require('./cjs');
export function main() { def(); local(); all.original(); ns.inner(); named(); cjs.named(); }
"#,
      ),
      (
        "src/exports.ts",
        "function original() {}\nexport default function fallback() {}\nexport { original as renamed, original };\nexport * as ns from './nested';\n",
      ),
      ("src/nested.ts", "export function inner() {}\nexport default function hidden() {}\n"),
      ("src/cjs.js", "function named() {}\nmodule.exports = { named };\n"),
    ],
  );
  let project = &synthetic.project;
  let main = app(&["src", "main"], &["main"], EntityKind::Function);
  let original = app(&["src", "exports"], &["original"], EntityKind::Function);
  let call = project.assert_edge(EdgeKind::Calls, main, original, RESOLVED);
  assert_eq!(call.evidence.len(), 2);
  project.assert_edge(EdgeKind::Calls, main, app(&["src", "exports"], &["fallback"], EntityKind::Function), RESOLVED);
  project.assert_edge(EdgeKind::Calls, main, app(&["src", "nested"], &["inner"], EntityKind::Function), RESOLVED);
  let named = project.assert_edge(EdgeKind::Calls, main, app(&["src", "cjs"], &["named"], EntityKind::Function), RESOLVED);
  assert_eq!(named.evidence.len(), 2);
  project.assert_edge(EdgeKind::Imports, file("app", "src/main.ts"), file("app", "src/cjs.js"), RESOLVED);
}

#[test]
fn should_keep_default_out_of_star_re_exports_and_report_conflicting_stars() {
  let synthetic = single(
    "app",
    &[
      ("package.json", PACKAGE),
      (
        "src/main.ts",
        "import barrel, { clash, single } from './barrel';\nexport function main() { barrel(); clash(); single(); }\n",
      ),
      ("src/barrel.ts", "export * from './one';\nexport * from './two';\n"),
      (
        "src/one.ts",
        "export default function first() {}\nexport function clash() {}\nexport function single() {}\n",
      ),
      ("src/two.ts", "export function clash() {}\n"),
    ],
  );
  let project = &synthetic.project;
  let main = app(&["src", "main"], &["main"], EntityKind::Function);
  project.assert_unresolved(main, "barrel", &UnresolvedReason::NotFound);
  let mut candidates = vec![
    app(&["src", "one"], &["clash"], EntityKind::Function),
    app(&["src", "two"], &["clash"], EntityKind::Function),
  ];
  candidates.sort();
  project.assert_unresolved(main, "clash", &UnresolvedReason::Ambiguous { candidates });
  project.assert_edge(EdgeKind::Calls, main, app(&["src", "one"], &["single"], EntityKind::Function), RESOLVED);
}

#[test]
fn should_resolve_members_through_this_fields_extends_super_and_constructors() {
  let synthetic = single(
    "app",
    &[
      ("package.json", PACKAGE),
      (
        "src/main.ts",
        r#"import { Repo } from './repo';
class Base {
  protected log(): void {}
}
export class Service extends Base {
  private cache = new Repo();
  constructor(private readonly repo: Repo, public plain?: Repo) { super(); }
  run(): void {
    this.log();
    super.log();
    this.repo.find();
    this.cache.find();
    this.plain.find();
    const local = new Repo();
    local.find();
    Service.create();
  }
  static create(): Service { return new Service(new Repo()); }
}
export function outside(service: Service) { service.run(); }
"#,
      ),
      ("src/repo.ts", "export class Repo {\n  find(): void {}\n}\n"),
    ],
  );
  let project = &synthetic.project;
  let run = app(&["src", "main"], &["Service", "run"], EntityKind::Method);
  let log = project.assert_edge(EdgeKind::Calls, run, app(&["src", "main"], &["Base", "log"], EntityKind::Method), RESOLVED);
  assert_eq!(log.evidence.len(), 2);
  let find = project.assert_edge(EdgeKind::Calls, run, app(&["src", "repo"], &["Repo", "find"], EntityKind::Method), RESOLVED);
  assert_eq!(find.evidence.len(), 4);
  project.assert_edge(EdgeKind::Calls, run, app(&["src", "main"], &["Service", "create"], EntityKind::Method), RESOLVED);
  project.assert_edge(EdgeKind::Calls, app(&["src", "main"], &["outside"], EntityKind::Function), run, RESOLVED);
  project.assert_edge(
    EdgeKind::Extends,
    app(&["src", "main"], &["Service"], EntityKind::Class),
    app(&["src", "main"], &["Base"], EntityKind::Class),
    RESOLVED,
  );
}

#[test]
fn should_resolve_interface_receivers_with_heuristic_implementer_candidates() {
  let synthetic = single(
    "app",
    &[
      ("package.json", PACKAGE),
      (
        "src/main.ts",
        r#"export interface Store { get(): string; }
export class Memory implements Store { get(): string { return 'm'; } }
export class Disk implements Store { get(): string { return 'd'; } }
export class Unrelated { get(): string { return 'u'; } }
export function read(store: Store) { return store.get(); }
"#,
      ),
    ],
  );
  let project = &synthetic.project;
  let read = app(&["src", "main"], &["read"], EntityKind::Function);
  project.assert_edge(EdgeKind::Calls, read, app(&["src", "main"], &["Store", "get"], EntityKind::Method), RESOLVED);
  for implementor in ["Memory", "Disk"] {
    let edge = project.assert_edge(EdgeKind::Calls, read, app(&["src", "main"], &[implementor, "get"], EntityKind::Method), HEURISTIC);
    assert_eq!(edge.certainty, Certainty::heuristic(Confidence::new(0.3).unwrap(), "interface-dispatch-candidate"));
  }
  project.assert_no_edge(EdgeKind::Calls, read, app(&["src", "main"], &["Unrelated", "get"], EntityKind::Method));
  project.assert_edge(
    EdgeKind::Implements,
    app(&["src", "main"], &["Memory"], EntityKind::Class),
    app(&["src", "main"], &["Store"], EntityKind::Interface),
    RESOLVED,
  );
}

#[test]
fn should_bound_unknown_receivers_and_reject_computed_members() {
  let synthetic = single(
    "app",
    &[
      ("package.json", PACKAGE),
      (
        "src/main.ts",
        r#"class A { only(): void {} shared(): void {} }
class B { shared(): void {} }
class C { shared(): void {} }
class D { shared(): void {} }
export function main(value: any, key: string) {
  value.only();
  value.shared();
  value[key]();
  (value as any).only();
}
"#,
      ),
    ],
  );
  let project = &synthetic.project;
  let main = app(&["src", "main"], &["main"], EntityKind::Function);
  let only = project.assert_edge(EdgeKind::Calls, main, app(&["src", "main"], &["A", "only"], EntityKind::Method), HEURISTIC);
  assert_eq!(only.certainty, Certainty::heuristic(Confidence::new(0.5).unwrap(), "method-name-only"));
  assert_eq!(only.evidence.len(), 1);
  let found = project.unresolved_named(main, "shared");
  assert!(matches!(&found[0].reason, UnresolvedReason::Ambiguous { candidates } if candidates.len() == 4));
  project.assert_unresolved(main, "[key]", &UnresolvedReason::Dynamic);
  let expression = project.unresolved_named(main, "only");
  assert_eq!(expression.len(), 1);
  assert_eq!(expression[0].path, ["(value as any)", "only"]);
  assert_eq!(expression[0].reason, UnresolvedReason::Dynamic);
}

#[test]
fn should_resolve_in_project_packages_through_exports_and_dist_to_src() {
  let synthetic = synthetic(&[
    (
      "web",
      &[
        ("package.json", r#"{ "name": "web", "dependencies": { "@x/src-lib": "*", "@x/built": "file:../built" } }"#),
        (
          "src/main.ts",
          "import { direct } from '@x/src-lib';\nimport { sub } from '@x/src-lib/sub';\nimport { built } from '@x/built';\nexport function main() { direct(); sub(); built(); }\n",
        ),
      ],
    ),
    (
      "lib",
      &[
        ("package.json", r#"{ "name": "@x/src-lib", "exports": { ".": "./src/index.ts", "./*": "./src/*.ts" } }"#),
        ("src/index.ts", "export function direct() {}\n"),
        ("src/sub.ts", "export function sub() {}\n"),
      ],
    ),
    (
      "built",
      &[
        ("package.json", r#"{ "name": "@x/built", "main": "dist/index.js", "types": "dist/index.d.ts" }"#),
        ("src/index.ts", "export function built() {}\n"),
      ],
    ),
  ]);
  let project = &synthetic.project;
  let main = ts("web", "web", &["src", "main"], &["main"], EntityKind::Function);
  project.assert_edge(
    EdgeKind::Calls,
    main,
    ts("lib", "@x/src-lib", &["src", "index"], &["direct"], EntityKind::Function),
    RESOLVED,
  );
  project.assert_edge(EdgeKind::Calls, main, ts("lib", "@x/src-lib", &["src", "sub"], &["sub"], EntityKind::Function), RESOLVED);
  let built = project.assert_edge(
    EdgeKind::Calls,
    main,
    ts("built", "@x/built", &["src", "index"], &["built"], EntityKind::Function),
    HEURISTIC,
  );
  assert_eq!(built.certainty, Certainty::heuristic(Confidence::new(0.85).unwrap(), "dist-to-src"));
}

#[test]
fn should_keep_the_class_edge_when_a_static_comes_from_an_external_base() {
  let synthetic = single(
    "app",
    &[
      ("package.json", PACKAGE),
      (
        "src/main.ts",
        r#"import { Base } from 'lib';
export class Model extends Base {
  static own() {}
}
export function useExternal() { Model.table; Model.find(); }
export function useOwn() { Model.own(); }
export function useGeneric<Model>() { Model.thing; }
"#,
      ),
    ],
  );
  let project = &synthetic.project;
  let model = app(&["src", "main"], &["Model"], EntityKind::Class);
  let external = app(&["src", "main"], &["useExternal"], EntityKind::Function);
  project.assert_edge(EdgeKind::References, external, model, RESOLVED);
  project.assert_no_edge(EdgeKind::References, app(&["src", "main"], &["useOwn"], EntityKind::Function), model);
  project.assert_no_edge(EdgeKind::References, app(&["src", "main"], &["useGeneric"], EntityKind::Function), model);
}
