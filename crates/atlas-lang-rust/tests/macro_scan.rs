mod common;

use atlas_core::{ReceiverHint, RefKind, RefOrigin};
use common::{analyze, strings};

fn scanned(body: &str) -> Vec<String> {
  let facts = analyze(&format!("fn host() {{ dsl! {{ {body} }} }}\n"));
  facts
    .references
    .iter()
    .filter(|reference| reference.kind != RefKind::MacroCall)
    .map(|reference| {
      assert_eq!(reference.origin, RefOrigin::MacroTokenScan);
      reference.path.join("::")
    })
    .collect()
}

fn receiver_hints(source: &str) -> Vec<(Vec<String>, Option<ReceiverHint>)> {
  analyze(source)
    .references
    .iter()
    .filter(|reference| reference.kind == RefKind::MethodCall)
    .map(|reference| (reference.path.clone(), reference.receiver.clone()))
    .collect()
}

fn expr(name: &str) -> Option<ReceiverHint> {
  Some(ReceiverHint::Expr(name.into()))
}

fn typed(segments: &[&str]) -> Option<ReceiverHint> {
  Some(ReceiverHint::TypeHint(strings(segments)))
}

#[test]
fn should_skip_tuple_patterns_before_match_arrows() {
  assert_eq!(scanned("Foo(y) => h(y)"), ["h"]);
}

#[test]
fn should_skip_enum_variant_declarations() {
  assert_eq!(scanned("enum E { A(u8) } stuff(a)"), ["stuff"]);
}

#[test]
fn should_skip_closure_trait_objects() {
  assert_eq!(scanned("type T = Box<dyn Fn(u8)>; stuff(a)"), ["stuff"]);
}

#[test]
fn should_skip_closure_trait_bounds() {
  assert_eq!(scanned("fn apply<F: FnMut(i32)>(f: F) { stuff(a) }"), ["stuff"]);
}

#[test]
fn should_skip_if_let_patterns() {
  assert_eq!(scanned("if let Some(Foo(z)) = value => bar(x)"), ["bar"]);
}

#[test]
fn should_skip_for_loop_patterns() {
  assert_eq!(scanned("for Item(k) in items => bar(x)"), ["bar"]);
}

#[test]
fn should_skip_qualified_tuple_patterns() {
  assert_eq!(scanned("Foo::Bar(a, b) => bar(x)"), ["bar"]);
}

#[test]
fn should_skip_attributes() {
  assert_eq!(scanned("#[derive(Debug)] struct S; stuff(a)"), ["stuff"]);
}

#[test]
fn should_skip_metavariable_calls() {
  assert_eq!(scanned("$name(args) => stuff(a)"), ["stuff"]);
}

#[test]
fn should_skip_struct_patterns_before_match_arrows() {
  assert_eq!(scanned("Foo { a, b } => h(y)"), ["h"]);
}

#[test]
fn should_keep_calls_methods_and_literals_found_by_the_scan() {
  assert_eq!(
    scanned("key => h(y), other => worker.bar(x), last => Report { code: stuff(a) }"),
    ["h", "worker::bar", "Report", "stuff"]
  );
}

#[test]
fn should_tag_individually_parsed_pieces_of_unknown_macros_as_token_scan() {
  let facts = analyze("fn host() { my_dsl!(key: compute(1), other(2)); }\n");
  let origins: Vec<(Vec<String>, RefOrigin)> = facts
    .references
    .iter()
    .filter(|reference| reference.kind == RefKind::Call)
    .map(|reference| (reference.path.clone(), reference.origin))
    .collect();
  assert_eq!(
    origins,
    [(strings(&["compute"]), RefOrigin::MacroTokenScan), (strings(&["other"]), RefOrigin::MacroTokenScan)]
  );
}

#[test]
fn should_keep_syntax_origin_for_known_expression_macro_families() {
  let facts = analyze("fn host() { anyhow!(code = %c, \"failed\", reason()); log::warn!(target: \"x\", \"y\", check()); bail!(ctx = ?c, \"z\", cause()); }\n");
  let origins: Vec<(Vec<String>, RefOrigin)> = facts
    .references
    .iter()
    .filter(|reference| reference.kind == RefKind::Call)
    .map(|reference| (reference.path.clone(), reference.origin))
    .collect();
  assert_eq!(
    origins,
    [
      (strings(&["reason"]), RefOrigin::Syntax),
      (strings(&["check"]), RefOrigin::Syntax),
      (strings(&["cause"]), RefOrigin::Syntax)
    ]
  );
}

#[test]
fn should_not_infer_a_newtype_field_type_for_arbitrary_wrappers() {
  assert_eq!(
    receiver_hints("struct Wrapper<T>(Vec<T>);\nfn a(Wrapper(inner): Wrapper<Foo>) { inner.len(); }\n"),
    [(strings(&["inner", "len"]), expr("inner"))]
  );
  assert_eq!(
    receiver_hints("struct Meters<T>(f64, PhantomData<T>);\nfn b(Meters(m, ..): Meters<Unit>) { m.abs(); }\n"),
    [(strings(&["m", "abs"]), expr("m"))]
  );
}

#[test]
fn should_infer_field_types_of_well_known_extractors_and_option() {
  assert_eq!(
    receiver_hints(
      "fn c(State(s): State<AppState>, Json(body): Json<CreateUser>, x: Option<Q>) {\n  s.run();\n  body.validate();\n  let Some(o): Option<Q> = x else { return };\n  o.go();\n}\n"
    ),
    [
      (strings(&["s", "run"]), typed(&["AppState"])),
      (strings(&["body", "validate"]), typed(&["CreateUser"])),
      (strings(&["o", "go"]), typed(&["Q"])),
    ]
  );
}

#[test]
fn should_not_trust_extractor_names_shadowed_by_a_local_type() {
  assert_eq!(
    receiver_hints("struct State<T>(Vec<T>);\nfn e(State(s): State<AppState>) { s.run(); }\n"),
    [(strings(&["s", "run"]), expr("s"))]
  );
}

#[test]
fn should_not_infer_types_from_constructor_calls() {
  assert_eq!(
    receiver_hints("fn f() { let c = Client::new(); c.send(); let d = Config { retries: 1 }; d.apply(); }\n"),
    [(strings(&["c", "send"]), expr("c")), (strings(&["d", "apply"]), typed(&["Config"]))]
  );
}
