mod common;

use std::collections::BTreeSet;

use atlas_query::{EntityRef, ImpactRequest, SymbolRequest, Targeted, TraverseRequest, UnusedCandidate, UnusedRequest};

fn found<T: std::fmt::Debug>(targeted: Targeted<T>) -> T {
  match targeted {
    Targeted::Found(answer) => answer,
    other => panic!("expected a resolved target, got {other:?}"),
  }
}

fn fixture() -> common::Fixture {
  common::fixture("adversarial", &["ts-overrides"])
}

#[test]
fn should_list_every_implementation_in_the_impact_of_an_interface_method() {
  let fixture = fixture();

  let answer = found(fixture.query.impact(&ImpactRequest::new(["PaymentProvider.charge"])).unwrap().answer);

  let direct: BTreeSet<(&str, &str)> = answer
    .direct
    .items
    .iter()
    .flat_map(|item| item.why.first().map(|hop| (item.entity.short.as_str(), hop.kind.as_str())))
    .collect();
  for implementation in ["StripeProvider.charge", "PaypalProvider.charge", "BaseProvider.charge"] {
    assert!(direct.contains(&(implementation, "implements")), "{implementation} missing from {direct:?}");
  }
  assert!(direct.contains(&("checkout", "calls")), "{direct:?}");
}

#[test]
fn should_count_method_level_implementors_of_an_interface_method() {
  let fixture = fixture();

  let answer = found(fixture.query.symbol(&SymbolRequest::new("PaymentProvider.charge")).unwrap().answer);

  assert_eq!(answer.counts.implementors.total, 3);
  assert_eq!(answer.counts.implementors.uncertain, 0);
}

#[test]
fn should_label_heritage_dependents_as_implements_or_extends_and_never_references() {
  let fixture = fixture();

  for (target, expected) in [
    (
      "PaymentProvider",
      vec![("BaseProvider", "implements"), ("PaypalProvider", "implements"), ("StripeProvider", "implements")],
    ),
    ("BaseProvider", vec![("StripeProvider", "extends")]),
  ] {
    let answer = found(fixture.query.dependents(&TraverseRequest::new(target)).unwrap().answer);
    let classes: Vec<(&str, &str)> = answer
      .groups
      .iter()
      .flat_map(|group| &group.items)
      .filter(|item| item.entity.kind == "class")
      .map(|item| (item.entity.short.as_str(), item.relation.kind.as_str()))
      .collect();
    assert_eq!(classes, expected, "dependents of {target}");
  }
  let response = fixture.query.dependents(&TraverseRequest::new("PaymentProvider")).unwrap();
  let text = fixture.text(&response);
  assert!(!text.lines().any(|line| line.contains("StripeProvider") && line.contains("references")), "{text}");
}

#[test]
fn should_exclude_only_methods_that_implement_a_member_when_the_hierarchy_is_in_the_project() {
  let fixture = fixture();

  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 100,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;

  let reported: Vec<&EntityRef> = answer
    .likely_unused
    .items
    .iter()
    .chain(&answer.possibly_unused.items)
    .map(|candidate: &UnusedCandidate| &candidate.entity)
    .collect();
  let shorts: BTreeSet<&str> = reported.iter().map(|entity| entity.short.as_str()).collect();
  for implementation in ["StripeProvider.charge", "StripeProvider.refund", "PaypalProvider.charge", "PaypalProvider.refund"] {
    assert!(!shorts.contains(implementation), "{implementation} reported: {shorts:?}");
  }
  assert_eq!(
    answer.excluded_entrypoints.get("method of a class implementing an interface (may be framework-invoked)"),
    None
  );
}

#[test]
fn should_exclude_a_downstream_implementation_of_a_called_interface_method() {
  let fixture = common::single_repo(
    "mono",
    &[
      ("package.json", r#"{ "name": "mono", "version": "1.0.0", "workspaces": ["packages/*"] }"#),
      ("packages/core/package.json", r#"{ "name": "@m/core", "version": "1.0.0", "main": "src/index.ts" }"#),
      (
        "packages/core/src/index.ts",
        "export interface Shape { area(): number; perimeter(): number; }\nexport function measure(shape: Shape): number { return shape.area(); }\n",
      ),
      (
        "packages/app/package.json",
        r#"{ "name": "@m/app", "version": "1.0.0", "main": "src/index.ts", "dependencies": { "@m/core": "*" } }"#,
      ),
      (
        "packages/app/src/index.ts",
        "import { Shape } from '@m/core';\nexport class Square implements Shape {\n  area(): number { return 1; }\n  perimeter(): number { return 4; }\n}\n",
      ),
    ],
  );

  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 100,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;

  let shorts: BTreeSet<&str> = answer
    .likely_unused
    .items
    .iter()
    .chain(&answer.possibly_unused.items)
    .map(|candidate| candidate.entity.short.as_str())
    .collect();
  assert!(!shorts.contains("Square.area"), "{shorts:?}");
  assert!(shorts.contains("Square.perimeter") && shorts.contains("Shape.perimeter"), "{shorts:?}");
  assert!(!shorts.contains("Square"), "{shorts:?}");
  assert_eq!(answer.excluded_entrypoints.get("trait or interface implementation"), Some(&2));
}

#[test]
fn should_keep_the_class_level_exclusion_when_an_interface_is_external() {
  let fixture = common::single_repo(
    "guards",
    &[
      ("package.json", "{\n  \"name\": \"guards\",\n  \"version\": \"1.0.0\"\n}\n"),
      (
        "src/guards.ts",
        "import { CanActivate } from '@nestjs/common';\nexport interface Named { label(): string; }\nexport class AuthGuard implements CanActivate, Named {\n  canActivate(): boolean { return true; }\n  label(): string { return 'auth'; }\n}\nexport class Labels implements Named {\n  label(): string { return 'labels'; }\n  helper(): string { return 'unused'; }\n}\nexport function describe(named: Named): string { return named.label(); }\n",
      ),
    ],
  );

  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 100,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;

  let shorts: BTreeSet<&str> = answer
    .likely_unused
    .items
    .iter()
    .chain(&answer.possibly_unused.items)
    .map(|candidate| candidate.entity.short.as_str())
    .collect();
  assert!(shorts.contains("Labels.helper"), "{shorts:?}");
  assert!(!shorts.contains("AuthGuard.canActivate"), "{shorts:?}");
  assert!(!shorts.contains("Labels.label"), "{shorts:?}");
  assert_eq!(
    answer.excluded_entrypoints.get("method of a class implementing an interface (may be framework-invoked)"),
    Some(&1)
  );
}

const HERITAGE_TYPES: &str = r#"export interface Named { label(): string; }
export interface Shape { area(): number; }
export type ShapeAlias = { area(): number };
export class Model {
  build(): void {}
  live(): void {}
}
"#;

const HERITAGE_IMPLS: &str = r#"import { AuthGuard } from '@nestjs/passport';
import { Model, Named, Shape, ShapeAlias } from './types';
export class AliasSquare implements ShapeAlias {
  area(): number { return 1; }
  side(): number { return 1; }
}
export class JwtGuard extends AuthGuard('jwt') implements Named {
  handleRequest(user: unknown) { return user; }
  label() { return 'jwt'; }
}
export class Account extends Model {
  build(): void {}
  live(): void {}
}
export class Square implements Shape {
  area(): number { return 2; }
}
export function measure(shape: Shape, model: Model): number {
  model.live();
  return shape.area();
}
"#;

fn reported(fixture: &common::Fixture) -> BTreeSet<String> {
  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 200,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;
  answer
    .likely_unused
    .items
    .iter()
    .chain(&answer.possibly_unused.items)
    .map(|candidate| candidate.entity.short.clone())
    .collect()
}

#[test]
fn should_keep_the_class_level_exclusion_for_type_alias_and_call_expression_heritage() {
  let fixture = common::single_repo(
    "heritage",
    &[
      ("package.json", "{\n  \"name\": \"heritage\",\n  \"version\": \"1.0.0\"\n}\n"),
      ("src/types.ts", HERITAGE_TYPES),
      ("src/impls.ts", HERITAGE_IMPLS),
    ],
  );

  let shorts = reported(&fixture);

  for excluded in ["AliasSquare.area", "AliasSquare.side", "JwtGuard.handleRequest", "JwtGuard.label"] {
    assert!(!shorts.contains(excluded), "{excluded} reported: {shorts:?}");
  }
}

#[test]
fn should_report_a_dead_base_and_override_pair_but_not_a_live_one() {
  let fixture = common::single_repo(
    "heritage",
    &[
      ("package.json", "{\n  \"name\": \"heritage\",\n  \"version\": \"1.0.0\"\n}\n"),
      ("src/types.ts", HERITAGE_TYPES),
      ("src/impls.ts", HERITAGE_IMPLS),
    ],
  );

  let shorts = reported(&fixture);

  for dead in ["Model.build", "Account.build"] {
    assert!(shorts.contains(dead), "{dead} not reported: {shorts:?}");
  }
  for live in ["Model.live", "Account.live", "Shape.area", "Square.area"] {
    assert!(!shorts.contains(live), "{live} reported: {shorts:?}");
  }
}

#[test]
fn should_treat_an_override_chain_longer_than_the_walk_bound_as_live() {
  let mut source = String::from("export interface Step { m(): void; }\nexport function run(step: Step): void { step.m(); }\nexport class L0 implements Step { m(): void {} }\n");
  for level in 1..=40 {
    source.push_str(&format!("export class L{level} extends L{} {{ m(): void {{}} }}\n", level - 1));
  }
  let fixture = common::single_repo(
    "chain",
    &[
      ("package.json", "{\n  \"name\": \"chain\",\n  \"version\": \"1.0.0\"\n}\n"),
      ("src/chain.ts", source.as_str()),
    ],
  );

  let shorts = reported(&fixture);

  let methods: Vec<&String> = shorts.iter().filter(|short| short.ends_with(".m")).collect();
  assert!(methods.is_empty(), "{methods:?}");
}
