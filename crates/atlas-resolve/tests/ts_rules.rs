mod common;

use atlas_core::{Certainty, CertaintyClass, Confidence, EdgeKind, EntityKind, UnresolvedReason};
use atlas_resolve::Resolution;
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

#[test]
fn should_reference_class_fields_read_through_this_including_inherited_ones() {
  let synthetic = single(
    "app",
    &[
      ("package.json", PACKAGE),
      (
        "src/main.ts",
        r#"import { UsersService } from './users';
export class Base {
  protected shared = 1;
}
export class Service extends Base {
  constructor(private readonly usersService: UsersService) { super(); }
  run() {
    const a = this.usersService;
    const b = this.shared;
    return [a, b, this.usersService.count];
  }
}
"#,
      ),
      ("src/users.ts", "export class UsersService { count = 0; }\n"),
    ],
  );
  let project = &synthetic.project;
  let run = app(&["src", "main"], &["Service", "run"], EntityKind::Method);
  let users = project.assert_edge(EdgeKind::References, run, app(&["src", "main"], &["Service", "usersService"], EntityKind::Field), RESOLVED);
  assert_eq!(users.evidence.len(), 2);
  project.assert_edge(EdgeKind::References, run, app(&["src", "main"], &["Base", "shared"], EntityKind::Field), RESOLVED);
}

fn via(edge: &atlas_core::Edge) -> Option<&str> {
  edge.attrs.get("via").and_then(|value| value.as_str())
}

const PROVIDERS: &str = r#"export interface Provider {
  readonly label: string;
  charge(amount: number): number;
  refund(id: string): void;
}
export abstract class Base implements Provider {
  abstract readonly label: string;
  abstract charge(amount: number): number;
  refund(id: string): void {}
  protected log(message: string): void {}
  static create(): void {}
  private secret(): void {}
  #hidden(): void {}
}
export class Stripe extends Base implements Provider {
  readonly label = 'stripe';
  charge(amount: number): number { return amount; }
  refund(id: string): void {}
  get label2(): string { return ''; }
  static create(): void {}
  secret(): void {}
  #hidden(): void {}
  constructor() { super(); }
}
export class Paypal implements Provider {
  label = 'paypal';
  charge = (amount: number): number => amount;
  refund(id: string): void {}
  audit(): void {}
}
"#;

#[test]
fn should_link_class_methods_to_the_nearest_member_of_each_heritage_target() {
  let synthetic = single("app", &[("package.json", PACKAGE), ("src/main.ts", PROVIDERS)]);
  let project = &synthetic.project;
  let member = |items: &[&str]| app(&["src", "main"], items, EntityKind::Method);
  let expected = [
    (["Base", "charge"], ["Provider", "charge"], "interface"),
    (["Base", "refund"], ["Provider", "refund"], "interface"),
    (["Stripe", "charge"], ["Base", "charge"], "class"),
    (["Stripe", "charge"], ["Provider", "charge"], "interface"),
    (["Stripe", "refund"], ["Base", "refund"], "class"),
    (["Stripe", "refund"], ["Provider", "refund"], "interface"),
    (["Stripe", "create"], ["Base", "create"], "class"),
    (["Paypal", "charge"], ["Provider", "charge"], "interface"),
    (["Paypal", "refund"], ["Provider", "refund"], "interface"),
  ];
  for (from, to, kind) in expected {
    let edge = project.assert_edge(EdgeKind::Implements, member(&from), member(&to), RESOLVED);
    assert_eq!(edge.certainty, Certainty::Resolved);
    assert_eq!(via(edge), Some(kind), "{from:?} -> {to:?}");
  }
  for from in [
    ["Base", "log"],
    ["Stripe", "secret"],
    ["Stripe", "#hidden"],
    ["Stripe", "label2"],
    ["Stripe", "constructor"],
    ["Paypal", "audit"],
  ] {
    assert_eq!(project.edges_from(EdgeKind::Implements, member(&from)), Vec::<&atlas_core::Edge>::new(), "{from:?}");
  }
  project.assert_no_edge(EdgeKind::Implements, member(&["Paypal", "charge"]), member(&["Base", "charge"]));
  for field in [["Stripe", "label"], ["Paypal", "label"]] {
    assert!(project.edges_from(EdgeKind::Implements, app(&["src", "main"], &field, EntityKind::Field)).is_empty());
  }
}

#[test]
fn should_label_heritage_as_extends_or_implements_and_not_as_a_parallel_reference() {
  let synthetic = single(
    "app",
    &[
      ("package.json", PACKAGE),
      (
        "src/main.ts",
        r#"import { Store } from './store';
import { External } from 'lib';
export interface User { id: string; }
export interface Named { name(): string; }
export interface Keyed extends Named { key(): string; }
export class Base<T> { load(): T | undefined { return undefined; } }
export class Users extends Base<User> implements Store<User>, Keyed {
  name(): string { return 'users'; }
  key(): string { return 'id'; }
  load(): User | undefined { return undefined; }
}
export class Wrapped extends External {
  render(): string { return ''; }
}
"#,
      ),
      ("src/store.ts", "export interface Store<T> { load(): T | undefined; }\n"),
    ],
  );
  let project = &synthetic.project;
  let main = |items: &[&str], kind: EntityKind| app(&["src", "main"], items, kind);
  let users = main(&["Users"], EntityKind::Class);
  let store = app(&["src", "store"], &["Store"], EntityKind::Interface);
  let base = main(&["Base"], EntityKind::Class);
  let keyed = main(&["Keyed"], EntityKind::Interface);
  let named = main(&["Named"], EntityKind::Interface);
  project.assert_edge(EdgeKind::Extends, users, base, RESOLVED);
  project.assert_edge(EdgeKind::Implements, users, store, RESOLVED);
  project.assert_edge(EdgeKind::Implements, users, keyed, RESOLVED);
  project.assert_edge(EdgeKind::Extends, keyed, named, RESOLVED);
  for (from, to) in [(users, base), (users, store), (users, keyed), (keyed, named)] {
    project.assert_no_edge(EdgeKind::References, from, to);
  }
  project.assert_edge(EdgeKind::References, users, main(&["User"], EntityKind::Interface), RESOLVED);
  let method = |items: &[&str]| main(items, EntityKind::Method);
  let name = project.assert_edge(EdgeKind::Implements, method(&["Users", "name"]), method(&["Named", "name"]), RESOLVED);
  assert_eq!(via(name), Some("interface"));
  project.assert_edge(EdgeKind::Implements, method(&["Users", "key"]), method(&["Keyed", "key"]), RESOLVED);
  project.assert_edge(
    EdgeKind::Implements,
    method(&["Users", "load"]),
    app(&["src", "store"], &["Store", "load"], EntityKind::Method),
    RESOLVED,
  );
  project.assert_edge(EdgeKind::Implements, method(&["Users", "load"]), method(&["Base", "load"]), RESOLVED);
  let wrapped = main(&["Wrapped"], EntityKind::Class);
  assert!(project.edges_from(EdgeKind::Implements, method(&["Wrapped", "render"])).is_empty());
  project.assert_unresolved(wrapped, "External", &UnresolvedReason::External { package: "lib".to_owned() });
}

#[test]
fn should_match_accessors_by_kind_and_weaken_overrides_through_dist_to_src() {
  let synthetic = synthetic(&[
    (
      "web",
      &[
        ("package.json", r#"{ "name": "web", "dependencies": { "@x/built": "file:../built" } }"#),
        (
          "src/main.ts",
          r#"import { Model } from '@x/built';
export class Account extends Model {
  get value(): number { return 1; }
  set value(next: number) {}
  save(): void {}
}
"#,
        ),
      ],
    ),
    (
      "built",
      &[
        ("package.json", r#"{ "name": "@x/built", "main": "dist/index.js", "types": "dist/index.d.ts" }"#),
        (
          "src/index.ts",
          "export class Model {\n  get value(): number { return 0; }\n  set value(next: number) {}\n  save(): void {}\n}\n",
        ),
      ],
    ),
  ]);
  let project = &synthetic.project;
  let account = |items: &[&str], n: u32| common::symbol_n(atlas_core::LanguageFamily::Ts, "web", "web", &["src", "main"], items, EntityKind::Method, n);
  let model = |items: &[&str], n: u32| common::symbol_n(atlas_core::LanguageFamily::Ts, "built", "@x/built", &["src", "index"], items, EntityKind::Method, n);
  let dist = Certainty::heuristic(Confidence::new(0.85).unwrap(), "dist-to-src");
  for (n, items) in [(0, ["Account", "value"]), (1, ["Account", "value"]), (0, ["Account", "save"])] {
    let edges = project.edges_from(EdgeKind::Implements, account(&items, n));
    let targets: Vec<_> = edges.iter().map(|edge| (edge.to, edge.certainty.clone(), via(edge))).collect();
    let to = model(&[items[0].replace("Account", "Model").as_str(), items[1]], n);
    assert_eq!(targets, [(to, dist.clone(), Some("class"))], "{items:?}~{n}");
  }
}

#[test]
fn should_link_an_implementation_to_every_overload_of_the_nearest_member() {
  let synthetic = single(
    "app",
    &[
      ("package.json", PACKAGE),
      (
        "src/main.ts",
        r#"export interface Emitter {
  on(event: 'a', cb: () => void): void;
  on(event: 'b', cb: (x: number) => void): void;
}
export type ShapeAlias = { area(): number };
export class Bus implements Emitter {
  on(event: string, cb: (...args: any[]) => void): void {}
}
export class Square implements ShapeAlias {
  area(): number { return 1; }
}
"#,
      ),
    ],
  );
  let project = &synthetic.project;
  let method = |items: &[&str], n: u32| common::symbol_n(atlas_core::LanguageFamily::Ts, "app", "app", &["src", "main"], items, EntityKind::Method, n);
  let mut targets: Vec<_> = project
    .edges_from(EdgeKind::Implements, method(&["Bus", "on"], 0))
    .iter()
    .map(|edge| (edge.to, via(edge)))
    .collect();
  targets.sort();
  let mut expected = vec![(method(&["Emitter", "on"], 0), Some("interface")), (method(&["Emitter", "on"], 1), Some("interface"))];
  expected.sort();
  assert_eq!(targets, expected);
  assert!(project.edges_from(EdgeKind::Implements, method(&["Square", "area"], 0)).is_empty());
}

#[test]
fn should_resolve_detector_prototype_paths_to_instance_members() {
  let synthetic = single(
    "app",
    &[
      ("package.json", PACKAGE),
      (
        "src/orders.ts",
        "export class Base { audit() {} }\nexport class Orders extends Base { findOne() {} static create() {} }\nexport interface Port { send(): void }\n",
      ),
      (
        "src/orders.spec.ts",
        "import { type Orders, type Port } from './orders';\nimport { Mailer } from 'mailer';\ndescribe('Orders', () => {});\n",
      ),
    ],
  );
  let project = &synthetic.project;
  let spec = project.facts("app", "src/orders.spec.ts");
  let resolve = |path: &[&str]| atlas_resolve::resolve_path_in_file(spec, &common::strings(path), None, &project.table).unwrap();
  let orders = |member: &str| app(&["src", "orders"], &["Orders", member], EntityKind::Method);

  assert_eq!(resolve(&["Orders", "prototype", "findOne"]), Resolution::Resolved(orders("findOne"), Certainty::Resolved));
  assert_eq!(
    resolve(&["Orders", "prototype", "audit"]),
    Resolution::Resolved(app(&["src", "orders"], &["Base", "audit"], EntityKind::Method), Certainty::Resolved)
  );
  assert_eq!(
    resolve(&["Port", "prototype", "send"]),
    Resolution::Resolved(app(&["src", "orders"], &["Port", "send"], EntityKind::Method), Certainty::Resolved)
  );
  assert_eq!(resolve(&["Orders", "prototype", "create"]), Resolution::Unresolved(UnresolvedReason::NotFound));
  assert_eq!(resolve(&["Orders", "findOne"]), Resolution::Unresolved(UnresolvedReason::NotFound));
  assert_eq!(resolve(&["Orders", "create"]), Resolution::Resolved(orders("create"), Certainty::Resolved));
  assert_eq!(
    resolve(&["Mailer", "prototype", "send"]),
    Resolution::Unresolved(UnresolvedReason::External { package: "mailer".to_owned() })
  );
}
