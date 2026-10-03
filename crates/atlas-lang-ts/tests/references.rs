mod common;

use std::fs;

use atlas_core::{ArgSummary, ReceiverHint, RefKind};
use common::{analyze, fixtures_root, owner_name, refs};

fn refs_of_kind(facts: &atlas_core::FileFacts, kind: RefKind) -> Vec<String> {
  facts
    .references
    .iter()
    .filter(|reference| reference.kind == kind)
    .map(|reference| format!("{} -> {}", owner_name(facts, reference.from), reference.path.join(".")))
    .collect()
}

fn receiver_of(facts: &atlas_core::FileFacts, path: &str) -> Option<ReceiverHint> {
  facts
    .references
    .iter()
    .find(|reference| reference.path.join(".") == path)
    .and_then(|reference| reference.receiver.clone())
}

fn strings(values: &[&str]) -> Vec<String> {
  values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn should_follow_the_shadowing_fixture_rules() {
  let source = fs::read_to_string(fixtures_root().join("adversarial/shadowing-ts/src/render.ts")).unwrap();
  let facts = analyze("src/render.ts", &source);
  assert_eq!(refs_of_kind(&facts, RefKind::Call), ["renderImported -> format", "renderBlockScoped -> format"]);
  let block_call = facts
    .references
    .iter()
    .find(|reference| reference.kind == RefKind::Call && reference.from == Some(atlas_core::LocalSymbolId(3)));
  assert_eq!(block_call.map(|reference| reference.span.start_line), Some(19));
  assert!(
    facts
      .references
      .iter()
      .all(|reference| reference.path.first().map(String::as_str) != Some("format") || reference.kind == RefKind::Call)
  );
}

#[test]
fn should_respect_catch_loop_destructuring_hoisting_and_type_parameter_bindings() {
  let source = r#"
import { log, item, Model, helper, Box, value } from './deps';

export function run<Model>(input: Model[], { value }: Options): Model {
  for (const item of input) item.save();
  for (let i = 0; i < 1; i++) log(i);
  try { helper(); } catch (log) { log.report(); }
  const [first, { nested: Box }] = input;
  hoisted();
  function hoisted() {}
  if (value) { var late = value; }
  late();
  return value;
}

export function outside(): Model {
  item.save();
  return helper(Box, value);
}
"#;
  let facts = analyze("src/scope.ts", source);
  let calls = refs_of_kind(&facts, RefKind::Call);
  assert_eq!(calls, ["run -> log", "run -> helper", "outside -> helper"]);
  let method_calls: Vec<(Vec<String>, Option<ReceiverHint>)> = facts
    .references
    .iter()
    .filter(|reference| reference.kind == RefKind::MethodCall)
    .map(|reference| (reference.path.clone(), reference.receiver.clone()))
    .collect();
  assert_eq!(
    method_calls,
    [
      (strings(&["item", "save"]), Some(ReceiverHint::Expr("item".into()))),
      (strings(&["log", "report"]), Some(ReceiverHint::Expr("log".into()))),
      (strings(&["item", "save"]), Some(ReceiverHint::Identifier("item".into()))),
    ]
  );
  assert_eq!(refs_of_kind(&facts, RefKind::TypeRef), ["run -> Options", "outside -> Model"]);
  assert_eq!(refs_of_kind(&facts, RefKind::ValueRef), ["outside -> Box", "outside -> value"]);
}

#[test]
fn should_classify_calls_method_calls_and_receivers() {
  let source = r#"
import * as handlers from './handlers';
import api from './api';
const legacy = require('./legacy');

export class Service {
  private client = new HttpClient();
  run(repo: Repository, raw: unknown) {
    plain();
    handlers.start();
    legacy.boot();
    api.users.list();
    this.client.get('/x');
    super.dispose();
    repo.save();
    (raw as Cache).clear();
    makeClient().fetch();
    table[methodName]();
    table['literal']();
    const local = new Local();
    local.go();
  }
}
"#;
  let facts = analyze("src/service.ts", source);
  let calls: Vec<(RefKind, String, Option<ReceiverHint>)> = facts
    .references
    .iter()
    .filter(|reference| matches!(reference.kind, RefKind::Call | RefKind::MethodCall))
    .map(|reference| (reference.kind, reference.path.join("."), reference.receiver.clone()))
    .collect();
  assert_eq!(
    calls,
    [
      (RefKind::Call, "plain".into(), None),
      (RefKind::Call, "handlers.start".into(), None),
      (RefKind::Call, "legacy.boot".into(), None),
      (RefKind::MethodCall, "api.users.list".into(), Some(ReceiverHint::Identifier("api".into()))),
      (RefKind::MethodCall, "this.client.get".into(), Some(ReceiverHint::This)),
      (RefKind::MethodCall, "super.dispose".into(), Some(ReceiverHint::Expr("super".into()))),
      (RefKind::MethodCall, "repo.save".into(), Some(ReceiverHint::TypeHint(strings(&["Repository"])))),
      (RefKind::MethodCall, "(raw as Cache).clear".into(), Some(ReceiverHint::TypeHint(strings(&["Cache"])))),
      (RefKind::MethodCall, "(makeClient()).fetch".into(), Some(ReceiverHint::Expr("makeClient()".into()))),
      (RefKind::Call, "makeClient".into(), None),
      (RefKind::MethodCall, "table.[methodName]".into(), Some(ReceiverHint::Identifier("table".into()))),
      (RefKind::MethodCall, "table.literal".into(), Some(ReceiverHint::Identifier("table".into()))),
      (RefKind::MethodCall, "local.go".into(), Some(ReceiverHint::TypeHint(strings(&["Local"])))),
    ]
  );
  assert_eq!(refs_of_kind(&facts, RefKind::Instantiation), ["Service.client -> HttpClient", "Service.run -> Local"]);
}

#[test]
fn should_emit_type_references_but_skip_primitives_and_type_parameters() {
  let source = r#"
export function convert<T extends Base>(input: T, list: Array<Item>, n: number, flag: boolean): Promise<ns.Result<T>> {
  const value = input as unknown as Target;
  const checked = value satisfies Shape;
  return wrap<Output>(checked);
}
export type Mapped<T> = { [K in keyof T]: T[K] extends infer U ? U : Fallback };
"#;
  let facts = analyze("src/types.ts", source);
  assert_eq!(
    refs_of_kind(&facts, RefKind::TypeRef),
    [
      "convert -> Base",
      "convert -> Array",
      "convert -> Item",
      "convert -> Promise",
      "convert -> ns.Result",
      "convert -> Target",
      "convert -> Shape",
      "convert -> Output",
      "Mapped -> Fallback",
    ]
  );
}

#[test]
fn should_emit_heritage_jsx_value_and_decorator_references() {
  let source = r#"
import { Base, Guard, Card, Layout, handler } from './deps';

@Module({ providers: [Guard], imports: [Other.forRoot()] })
export class Feature extends Base implements Contract {}

export function View() {
  const Local = pick();
  return (
    <Layout.Main title="x" onClick={handler} {...rest}>
      <Card />
      <Local />
      <div onClick={handler} />
    </Layout.Main>
  );
}

app.get('/x', guard, handler, this.onRequest);
"#;
  let facts = analyze("src/feature.tsx", source);
  assert_eq!(refs_of_kind(&facts, RefKind::TypeRef), ["Feature -> Base", "Feature -> Contract"]);
  assert_eq!(refs_of_kind(&facts, RefKind::Decorator), ["Feature -> Module"]);
  assert_eq!(refs_of_kind(&facts, RefKind::JsxElement), ["View -> Layout.Main", "View -> Card"]);
  assert_eq!(
    refs_of_kind(&facts, RefKind::ValueRef),
    [
      "Feature -> Guard",
      "View -> handler",
      "View -> handler",
      "<module> -> guard",
      "<module> -> handler",
      "<module> -> this.onRequest",
    ]
  );
  let layout = facts.references.iter().find(|reference| reference.kind == RefKind::JsxElement).unwrap();
  assert_eq!(
    layout.args,
    [ArgSummary::Object(vec![
      atlas_core::ObjectEntry {
        key: atlas_core::ObjectKey::Ident("title".into()),
        value: ArgSummary::Str("x".into()),
      },
      atlas_core::ObjectEntry {
        key: atlas_core::ObjectKey::Ident("onClick".into()),
        value: ArgSummary::Path(strings(&["handler"])),
      },
      atlas_core::ObjectEntry {
        key: atlas_core::ObjectKey::Spread,
        value: ArgSummary::Path(strings(&["rest"])),
      },
    ])]
  );
  assert_eq!(receiver_of(&facts, "Other.forRoot"), Some(ReceiverHint::Identifier("Other".into())));
}

#[test]
fn should_attribute_module_level_code_to_the_module_and_initialisers_to_their_symbol() {
  let source = r#"
export const API_URL = process.env.API_URL ?? fallback();
export const acme = new AcmeClient({ baseUrl: API_URL, token: getToken() });
let mutable = init();
app.use(cors());
const handlers = {
  start() { boot(); },
  stop: () => halt(),
};
"#;
  let facts = analyze("src/config.ts", source);
  assert_eq!(
    refs(&facts),
    [
      "API_URL -Call-> fallback",
      "acme -Instantiation-> AcmeClient",
      "acme -ValueRef-> API_URL",
      "acme -Call-> getToken",
      "mutable -Call-> init",
      "<module> -MethodCall-> app.use",
      "<module> -Call-> cors",
      "handlers -Call-> boot",
      "handlers -Call-> halt",
    ]
  );
}

#[test]
fn should_not_emit_references_from_inside_syntax_errors() {
  let source = fs::read_to_string(fixtures_root().join("adversarial/broken-syntax/src/broken.ts")).unwrap();
  let facts = analyze("src/broken.ts", &source);
  assert_eq!(refs(&facts), ["Early.run -Call-> before", "after -Call-> before", "Late.go -Call-> after"]);
  let atlas_core::ParseStatus::Partial { error_count, first_errors } = &facts.parse_status else {
    panic!("expected a partial parse")
  };
  assert_eq!(*error_count, 1);
  assert_eq!(first_errors[0].span.start_line, 12);
  let garbled = analyze("src/garbled.ts", "export function ok() { good(); }\nfoo(bar(;\nexport class After { m() { fine(); } }\n");
  assert_eq!(common::symbol_names(&garbled), ["ok#function", "After#class", "After.m#method"]);
  assert!(refs(&garbled).contains(&"After.m -Call-> fine".to_owned()));
  assert!(!refs(&garbled).iter().any(|line| line.ends_with("-> bar")));
}

fn method_calls(source: &str) -> Vec<(Vec<String>, Option<ReceiverHint>)> {
  let facts = analyze("src/hints.tsx", source);
  facts
    .references
    .iter()
    .filter(|reference| reference.kind == RefKind::MethodCall)
    .map(|reference| (reference.path.clone(), reference.receiver.clone()))
    .collect()
}

#[test]
fn should_start_member_paths_with_the_head_the_receiver_describes() {
  let calls = method_calls("export function connect(options: AcmeClientOptions, name: string) {\n  options.baseUrl.replace('/', '');\n  handlers[name]();\n}\n");
  assert_eq!(
    calls,
    [
      (strings(&["options", "baseUrl", "replace"]), Some(ReceiverHint::TypeHint(strings(&["AcmeClientOptions"])))),
      (strings(&["handlers", "[name]"]), Some(ReceiverHint::Identifier("handlers".into()))),
    ]
  );
}

#[test]
fn should_not_hint_names_bound_by_destructuring_props() {
  let calls = method_calls("export function Card({ svc, user }: CardProps) { svc.run(); user.name.trim(); }\n");
  assert_eq!(
    calls,
    [
      (strings(&["svc", "run"]), Some(ReceiverHint::Expr("svc".into()))),
      (strings(&["user", "name", "trim"]), Some(ReceiverHint::Expr("user".into()))),
    ]
  );
}

#[test]
fn should_not_hint_names_destructured_from_an_instantiation() {
  let calls = method_calls("export function f() { const { x, y } = new Bar(); x.go(); const bar = new Bar(); bar.go(); }\n");
  assert_eq!(
    calls,
    [
      (strings(&["x", "go"]), Some(ReceiverHint::Expr("x".into()))),
      (strings(&["bar", "go"]), Some(ReceiverHint::TypeHint(strings(&["Bar"])))),
    ]
  );
}

#[test]
fn should_not_hint_names_bound_by_array_patterns() {
  let calls = method_calls("export function f([first]: Pair, list: Item[]) { const [a, b]: Pair = list; first.go(); a.go(); }\n");
  assert_eq!(
    calls,
    [
      (strings(&["first", "go"]), Some(ReceiverHint::Expr("first".into()))),
      (strings(&["a", "go"]), Some(ReceiverHint::Expr("a".into()))),
    ]
  );
}

#[test]
fn should_not_hint_names_bound_by_rest_patterns() {
  let calls = method_calls("export function f({ id, ...rest }: Props, ...others: Props[]) { rest.go(); others.map(go); }\n");
  assert_eq!(
    calls,
    [
      (strings(&["rest", "go"]), Some(ReceiverHint::Expr("rest".into()))),
      (strings(&["others", "map"]), Some(ReceiverHint::Expr("others".into()))),
    ]
  );
}

#[test]
fn should_not_hint_receivers_typed_with_type_parameters_or_local_types() {
  let calls = method_calls("export function a<T>(x: T, y: Service) { x.go(); y.go(); }\nexport function b() { class Local {} const z = new Local(); z.go(); }\n");
  assert_eq!(
    calls,
    [
      (strings(&["x", "go"]), Some(ReceiverHint::Expr("x".into()))),
      (strings(&["y", "go"]), Some(ReceiverHint::TypeHint(strings(&["Service"])))),
      (strings(&["z", "go"]), Some(ReceiverHint::Expr("z".into()))),
    ]
  );
}

#[test]
fn should_describe_the_head_of_value_reference_chains() {
  let facts = analyze(
    "src/routes.ts",
    "export class Wire { wire(local: Router) { app.use(router.handle, local.handle, this.onError); } }\n",
  );
  let values: Vec<(Vec<String>, Option<ReceiverHint>)> = facts
    .references
    .iter()
    .filter(|reference| reference.kind == RefKind::ValueRef)
    .map(|reference| (reference.path.clone(), reference.receiver.clone()))
    .collect();
  assert_eq!(
    values,
    [
      (strings(&["router", "handle"]), Some(ReceiverHint::Identifier("router".into()))),
      (strings(&["this", "onError"]), Some(ReceiverHint::This)),
    ]
  );
}

#[test]
fn should_capture_generic_calls_with_and_without_await() {
  let source = r#"
import { foo, X } from './deps';
export class S {
  async run(x: Pool, arg: number) {
    x.y<T>();
    await x.y<T>();
    await this.a.b<T>(arg);
    new X<T>();
    await new X<T>();
    foo<T>();
    await foo<T>();
    x?.y<T>();
    (await x.y<T>()).z();
    return x.y<T>();
  }
  async *gen(x: Pool) { yield x.y<T>(); }
}
"#;
  let facts = analyze("src/a.ts", source);
  let calls: Vec<String> = facts
    .references
    .iter()
    .filter(|reference| matches!(reference.kind, RefKind::Call | RefKind::MethodCall | RefKind::Instantiation))
    .map(|reference| format!("{:?} {}", reference.kind, reference.path.join(".")))
    .collect();
  assert_eq!(
    calls,
    [
      "MethodCall x.y",
      "MethodCall x.y",
      "MethodCall this.a.b",
      "Instantiation X",
      "Instantiation X",
      "Call foo",
      "Call foo",
      "MethodCall x.y",
      "MethodCall (await x.y<T>()).z",
      "MethodCall x.y",
      "MethodCall x.y",
      "MethodCall x.y",
    ]
  );
}

#[test]
fn should_keep_receiver_args_and_span_of_an_awaited_generic_call() {
  let source = "class S { async run(arg: number) { await this.a.b<T>(arg, 'q'); } }";
  let facts = analyze("src/a.ts", source);
  let call = facts.references.iter().find(|reference| reference.kind == RefKind::MethodCall).unwrap();
  assert_eq!(call.path, strings(&["this", "a", "b"]));
  assert_eq!(call.receiver, Some(ReceiverHint::This));
  assert_eq!(call.args.len(), 2);
  assert_eq!(&source[call.span.start_byte as usize..call.span.end_byte as usize], "this.a.b<T>(arg, 'q')");
}

#[test]
fn should_capture_awaited_generic_call_in_tsx() {
  let source = "import { useQuery } from './q';\nexport async function C() { const d = await useQuery<User>('k'); return <div />; }";
  let facts = analyze("src/c.tsx", source);
  assert_eq!(refs_of_kind(&facts, RefKind::Call), ["C -> useQuery"]);
}

fn this_calls(source: &str) -> Vec<(String, String, Option<ReceiverHint>)> {
  let facts = analyze("src/this.ts", source);
  facts
    .references
    .iter()
    .filter(|reference| reference.kind == RefKind::MethodCall && reference.path.first().map(String::as_str) == Some("this"))
    .map(|reference| (owner_name(&facts, reference.from), reference.path.join("."), reference.receiver.clone()))
    .collect()
}

fn rebound(owner: &str) -> (String, String, Option<ReceiverHint>) {
  (owner.to_owned(), "this.helper".to_owned(), Some(ReceiverHint::Expr("this".to_owned())))
}

fn bound(owner: &str) -> (String, String, Option<ReceiverHint>) {
  (owner.to_owned(), "this.helper".to_owned(), Some(ReceiverHint::This))
}

#[test]
fn should_rebind_this_inside_object_literal_methods() {
  let source = "class A { helper() {} go() { const o = { helper() {}, run() { this.helper(); } }; } }";
  assert_eq!(this_calls(source), [rebound("A.go")]);
}

#[test]
fn should_rebind_this_inside_object_literal_accessors() {
  let source = "class A { helper() {} go() { const o = { get value() { return this.helper(); }, set value(next) { this.helper(); } }; } }";
  assert_eq!(this_calls(source), [rebound("A.go"), rebound("A.go")]);
}

#[test]
fn should_rebind_this_inside_function_expression_callbacks() {
  let source = "class A { helper() {} go() { [1].forEach(function () { this.helper(); }); } }";
  assert_eq!(this_calls(source), [rebound("A.go")]);
}

#[test]
fn should_rebind_this_inside_function_declarations_nested_in_methods() {
  let source = "class A { helper() {} go() { function inner() { this.helper(); } inner(); } }";
  assert_eq!(this_calls(source), [rebound("A.go")]);
}

#[test]
fn should_keep_this_inside_arrow_functions() {
  let source = "class A { helper() {} go() { const f = () => this.helper(); const g = () => { [1].map(() => this.helper()); }; } }";
  assert_eq!(this_calls(source), [bound("A.go"), bound("A.go")]);
}

#[test]
fn should_keep_rebound_this_inside_arrows_nested_in_functions() {
  let source = "class A { helper() {} go() { [1].forEach(function () { const f = () => this.helper(); }); } }";
  assert_eq!(this_calls(source), [rebound("A.go")]);
}

#[test]
fn should_rebind_this_inside_methods_of_nested_classes() {
  let source = "class A { helper() {} go() { class B { helper() {} run() { this.helper(); } } } }";
  assert_eq!(this_calls(source), [rebound("A.go")]);
}

#[test]
fn should_restore_this_after_leaving_a_rebinding_construct() {
  let source = "class A { helper() {} go() { const o = { run() { this.helper(); } }; this.helper(); } }";
  assert_eq!(this_calls(source), [rebound("A.go"), bound("A.go")]);
}

#[test]
fn should_keep_this_inside_static_methods() {
  let source = "class A { static helper() {} static go() { this.helper(); } }";
  assert_eq!(this_calls(source), [bound("A.go")]);
}

#[test]
fn should_keep_this_inside_static_blocks() {
  let source = "class A { static helper() {} static { this.helper(); } }";
  assert_eq!(this_calls(source), [bound("A")]);
}

#[test]
fn should_rebind_this_in_value_references_inside_functions() {
  let source = "class A { helper() {} go() { register(this.helper); register(function () { register(this.helper); }); } }";
  let facts = analyze("src/this.ts", source);
  let hints: Vec<Option<ReceiverHint>> = facts
    .references
    .iter()
    .filter(|reference| reference.kind == RefKind::ValueRef && reference.path.join(".") == "this.helper")
    .map(|reference| reference.receiver.clone())
    .collect();
  assert_eq!(hints, [Some(ReceiverHint::This), Some(ReceiverHint::Expr("this".to_owned()))]);
}
