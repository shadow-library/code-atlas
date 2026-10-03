mod common;

use atlas_core::{ARG_MAX_WIDTH, ArgSummary, ObjectEntry, ObjectKey, RefKind, TemplatePart};
use common::analyze;

fn args_of(source: &str, callee: &str) -> Vec<ArgSummary> {
  let facts = analyze("src/args.ts", source);
  facts
    .references
    .iter()
    .find(|reference| matches!(reference.kind, RefKind::Call | RefKind::MethodCall) && reference.path.join(".") == callee)
    .unwrap_or_else(|| panic!("no call to {callee}"))
    .args
    .clone()
}

fn path(segments: &[&str]) -> ArgSummary {
  ArgSummary::Path(segments.iter().map(|segment| (*segment).to_owned()).collect())
}

fn entry(key: ObjectKey, value: ArgSummary) -> ObjectEntry {
  ObjectEntry { key, value }
}

#[test]
fn should_summarise_literals_templates_and_concatenation() {
  let args = args_of(
    "send('a\\'b\\n', `/users/${id}/x`, `plain`, '/a/' + id + '/b', 42, true, false, null, undefined, /re/g);",
    "send",
  );
  assert_eq!(
    args,
    [
      ArgSummary::Str("a'b\n".into()),
      ArgSummary::Template(vec![TemplatePart::Lit("/users/".into()), TemplatePart::Expr(path(&["id"])), TemplatePart::Lit("/x".into())]),
      ArgSummary::Template(vec![TemplatePart::Lit("plain".into())]),
      ArgSummary::Concat(vec![ArgSummary::Str("/a/".into()), path(&["id"]), ArgSummary::Str("/b".into())]),
      ArgSummary::Number("42".into()),
      ArgSummary::Bool(true),
      ArgSummary::Bool(false),
      ArgSummary::Null,
      path(&["undefined"]),
      ArgSummary::Other("/re/g".into()),
    ]
  );
  assert_eq!(args[3].to_pattern("{}").as_deref(), Some("/a/{}/b"));
}

#[test]
fn should_summarise_objects_arrays_closures_paths_and_calls() {
  let args = args_of(
    "fetch(`${API}/users`, { method: 'POST', headers: { Authorization: `Bearer ${token}`, 'x-api-key': key }, body, ...rest, [computed]: 1, run(a) {} }, [1, x], (a, { b }) => a, process.env.API_URL, this.base, build(1), (value as string), items[0], items['key']);",
    "fetch",
  );
  assert_eq!(
    args,
    [
      ArgSummary::Template(vec![TemplatePart::Expr(path(&["API"])), TemplatePart::Lit("/users".into())]),
      ArgSummary::Object(vec![
        entry(ObjectKey::Ident("method".into()), ArgSummary::Str("POST".into())),
        entry(
          ObjectKey::Ident("headers".into()),
          ArgSummary::Object(vec![
            entry(
              ObjectKey::Ident("Authorization".into()),
              ArgSummary::Template(vec![TemplatePart::Lit("Bearer ".into()), TemplatePart::Expr(path(&["token"]))]),
            ),
            entry(ObjectKey::Str("x-api-key".into()), path(&["key"])),
          ]),
        ),
        entry(ObjectKey::Ident("body".into()), path(&["body"])),
        entry(ObjectKey::Spread, path(&["rest"])),
        entry(ObjectKey::Computed("[computed]".into()), ArgSummary::Number("1".into())),
        entry(ObjectKey::Ident("run".into()), ArgSummary::Closure { params: vec!["a".into()] }),
      ]),
      ArgSummary::Array(vec![ArgSummary::Number("1".into()), path(&["x"])]),
      ArgSummary::Closure {
        params: vec!["a".into(), "{ b }".into()]
      },
      path(&["process", "env", "API_URL"]),
      path(&["this", "base"]),
      ArgSummary::Call {
        callee: vec!["build".into()],
        args: vec![ArgSummary::Number("1".into())]
      },
      path(&["value"]),
      path(&["items", "0"]),
      path(&["items", "key"]),
    ]
  );
}

#[test]
fn should_truncate_deep_and_wide_arguments() {
  let deep = args_of("f({ a: { b: { c: { d: { e: 1 } } } } });", "f");
  let ArgSummary::Object(level0) = &deep[0] else { panic!() };
  let ArgSummary::Object(level1) = &level0[0].value else { panic!() };
  let ArgSummary::Object(level2) = &level1[0].value else { panic!() };
  let ArgSummary::Object(level3) = &level2[0].value else { panic!() };
  assert_eq!(level3[0].value, ArgSummary::Truncated);
  let many: Vec<String> = (0..20).map(|index| index.to_string()).collect();
  let wide = args_of(&format!("g({});", many.join(", ")), "g");
  assert_eq!(wide.len(), ARG_MAX_WIDTH + 1);
  assert_eq!(wide.last(), Some(&ArgSummary::Truncated));
  let long = "x".repeat(400);
  let text = args_of(&format!("h('{long}');"), "h");
  let ArgSummary::Str(value) = &text[0] else { panic!() };
  assert!(value.ends_with('…') && value.len() <= atlas_core::ARG_MAX_TEXT);
}

#[test]
fn should_fingerprint_bodies_independently_of_names_and_literals() {
  let facts = analyze(
    "src/sums.ts",
    "export function a(values: number[]) { return values.reduce((x, y) => x + y, 0); }\nexport function b(items: number[]) { return items.reduce((p, q) => p + q, 10); }\nexport function c(items: number[]) { return items.reduce((p, q) => p * q, 10); }\n",
  );
  let bodies: Vec<_> = facts.symbols.iter().map(|symbol| symbol.body.clone().unwrap()).collect();
  assert_eq!(bodies[0].normalized_tokens, bodies[1].normalized_tokens);
  assert_ne!(bodies[1].normalized_tokens, bodies[2].normalized_tokens);
  assert_eq!(bodies[0].token_count, u32::try_from(bodies[0].normalized_tokens.len()).unwrap());
}
