mod common;

use atlas_core::{ARG_MAX_TEXT, ARG_MAX_WIDTH, ArgSummary, ChainHead, ChainStep, DeclaredBinding, ObjectEntry, ObjectKey, ReceiverChain, ReceiverHint, RefKind, RefOrigin, Span};
use common::{analyze, paths_of, reference, refs_of, strings};

type ChainLink = (String, Vec<String>, Option<ReceiverHint>, Vec<ArgSummary>);

fn path(segments: &[&str]) -> ArgSummary {
  ArgSummary::Path(strings(segments))
}

fn text(value: &str) -> ArgSummary {
  ArgSummary::Str(value.to_owned())
}

fn chained(head: ChainHead, steps: Vec<ChainStep>) -> ReceiverHint {
  ReceiverHint::Chain(Box::new(ReceiverChain { head, steps }))
}

fn binding(name: &str, hint: ReceiverHint) -> ChainHead {
  ChainHead::Binding { name: name.to_owned(), hint }
}

fn call(path: &[&str], args: Vec<Option<String>>) -> ChainHead {
  ChainHead::Call { path: strings(path), args }
}

fn receiver_of<'a>(facts: &'a atlas_core::FileFacts, method: &str) -> Vec<&'a ReceiverHint> {
  facts
    .references
    .iter()
    .filter(|reference| reference.kind == RefKind::MethodCall && reference.name() == Some(method))
    .filter_map(|reference| reference.receiver.as_ref())
    .collect()
}

#[test]
fn should_emit_calls_for_path_expressions() {
  let facts =
    analyze("fn run() {\n  foo();\n  a::b::c();\n  Self::new();\n  <T as Into<u8>>::into(x);\n  <Vec<u8>>::new();\n  Vec::<u8>::with_capacity(1);\n  parse::<Config>();\n}\n");
  assert_eq!(
    paths_of(&facts, RefKind::Call),
    ["foo", "a::b::c", "Self::new", "T::<Into<u8>>::into", "Vec::new", "Vec::with_capacity", "parse"]
  );
  let call = reference(&facts, RefKind::Call, &["a", "b", "c"]);
  let source = "fn run() {\n  foo();\n  a::b::c();";
  assert_eq!(call.span, Span::of_text(source, source.len() - 4, source.len() - 3).unwrap());
  assert_eq!(paths_of(&facts, RefKind::TypeRef), ["T", "Into", "Vec", "Config"]);
}

#[test]
fn should_emit_method_calls_with_receiver_hints() {
  let facts = analyze("fn run(&self, client: Client) {\n  self.flush();\n  client.send(1);\n  self.pool.get().unwrap();\n}\n");
  let calls: Vec<(Vec<String>, Option<ReceiverHint>)> = facts
    .references
    .iter()
    .filter(|reference| reference.kind == RefKind::MethodCall)
    .map(|reference| (reference.path.clone(), reference.receiver.clone()))
    .collect();
  assert_eq!(
    calls,
    [
      (strings(&["self", "flush"]), Some(ReceiverHint::SelfValue)),
      (strings(&["client", "send"]), Some(ReceiverHint::TypeHint(strings(&["Client"])))),
      (strings(&["self", "pool", "get"]), Some(ReceiverHint::SelfValue)),
      (
        strings(&["(self.pool.get())", "unwrap"]),
        Some(chained(
          binding("self", ReceiverHint::SelfValue),
          vec![ChainStep::Field("pool".into()), ChainStep::Method("get".into())]
        ))
      ),
    ]
  );
  assert_eq!(reference(&facts, RefKind::MethodCall, &["client", "send"]).args, [ArgSummary::Number("1".into())]);
}

#[test]
fn should_emit_a_method_call_for_every_link_of_a_call_chain() {
  let facts = analyze("pub fn router(state: AppState) -> Router {\n  Router::new()\n    .route(\"/users\", get(list_users))\n    .with_state(state)\n}\n");
  let chain: Vec<ChainLink> = facts
    .references
    .iter()
    .filter(|reference| matches!(reference.kind, RefKind::Call | RefKind::MethodCall))
    .map(|reference| (format!("{:?}", reference.kind), reference.path.clone(), reference.receiver.clone(), reference.args.clone()))
    .collect();
  assert_eq!(
    chain,
    [
      ("Call".to_owned(), strings(&["Router", "new"]), None, vec![]),
      (
        "MethodCall".to_owned(),
        strings(&["(Router::new())", "route"]),
        Some(chained(call(&["Router", "new"], vec![]), vec![])),
        vec![
          text("/users"),
          ArgSummary::Call {
            callee: strings(&["get"]),
            args: vec![path(&["list_users"])],
          },
        ]
      ),
      ("Call".to_owned(), strings(&["get"]), None, vec![path(&["list_users"])]),
      (
        "MethodCall".to_owned(),
        strings(&["(Router::new() .route(\"/users\", get(list_users)))", "with_state"]),
        Some(chained(call(&["Router", "new"], vec![]), vec![ChainStep::Method("route".into())])),
        vec![path(&["state"])]
      ),
    ]
  );
  assert_eq!(refs_of(&facts, RefKind::ValueRef), [("router".to_owned(), strings(&["list_users"]))]);
}

#[test]
fn should_truncate_long_receiver_expressions() {
  let chain = format!("fn run() {{ start(){}.finish(); }}\n", ".step(\"a long argument string\")".repeat(20));
  let facts = analyze(&chain);
  let finish = facts
    .references
    .iter()
    .find(|reference| reference.path.last().is_some_and(|name| name == "finish"))
    .unwrap();
  let Some(ReceiverHint::Expr(receiver)) = &finish.receiver else {
    panic!("expected an expression receiver")
  };
  assert_eq!(finish.path, [format!("({receiver})"), "finish".to_owned()]);
  assert!(receiver.starts_with("start().step(\"a long argument string\")"));
  assert!(receiver.ends_with('…'));
  assert!(receiver.len() <= ARG_MAX_TEXT);
}

#[test]
fn should_emit_macro_calls_with_format_style_arguments() {
  let facts = analyze("fn log(user: &User) {\n  println!(\"user {} has {n}\", user.name, n = COUNT);\n  let url = format!(r\"{}/users\", BASE);\n  std::println!();\n}\n");
  assert_eq!(paths_of(&facts, RefKind::MacroCall), ["println", "format", "std::println"]);
  assert_eq!(
    reference(&facts, RefKind::MacroCall, &["println"]).args,
    [
      text("user {} has {n}"),
      path(&["user", "name"]),
      ArgSummary::Named {
        name: "n".into(),
        value: Box::new(path(&["COUNT"])),
      },
    ]
  );
  assert_eq!(reference(&facts, RefKind::MacroCall, &["format"]).args, [text("{}/users"), path(&["BASE"])]);
  assert_eq!(reference(&facts, RefKind::MacroCall, &["std", "println"]).args, []);
  assert_eq!(paths_of(&facts, RefKind::ValueRef), ["user::name", "COUNT", "BASE"]);
}

#[test]
fn should_keep_arguments_of_non_expression_macros_opaque() {
  let facts = analyze("define_ops! { add => +, sub => - }\nfn run() { html! { <div class=\"x\"></div> } }\n");
  assert_eq!(
    reference(&facts, RefKind::MacroCall, &["define_ops"]).args,
    [ArgSummary::Other("add => +".into()), ArgSummary::Other("sub => -".into())]
  );
  assert_eq!(reference(&facts, RefKind::MacroCall, &["html"]).args, [ArgSummary::Other("<div class=\"x\"></div>".into())]);
  assert_eq!(reference(&facts, RefKind::MacroCall, &["define_ops"]).from, None);
  assert_eq!(facts.references.len(), 2);
}

#[test]
fn should_extract_references_inside_expression_macro_arguments() {
  let source = "#[test]\nfn converts() {\n  assert_eq!(to_cents(\"12.34\"), 1234);\n  assert!(matches!(parse(input), Some(Token::Word)));\n}\n";
  let facts = analyze(source);
  assert_eq!(paths_of(&facts, RefKind::MacroCall), ["assert_eq", "assert", "matches"]);
  assert_eq!(paths_of(&facts, RefKind::Call), ["to_cents", "parse"]);
  assert_eq!(paths_of(&facts, RefKind::ValueRef), ["input"]);
  let call = reference(&facts, RefKind::Call, &["to_cents"]);
  assert_eq!(call.args, [text("12.34")]);
  let start = source.find("to_cents").unwrap();
  assert_eq!(call.span, Span::of_text(source, start, start + "to_cents".len()).unwrap());
}

#[test]
fn should_not_emit_value_refs_from_custom_macro_arguments() {
  let facts = analyze("impl Point {\n  getter!(x, i64);\n}\nfn run() { my_macro!(helper(value), OTHER); }\n");
  assert_eq!(reference(&facts, RefKind::MacroCall, &["getter"]).args, [path(&["x"]), path(&["i64"])]);
  assert_eq!(paths_of(&facts, RefKind::Call), ["helper"]);
  assert_eq!(paths_of(&facts, RefKind::ValueRef), Vec::<String>::new());
}

#[test]
fn should_emit_instantiations_for_struct_literals() {
  let facts = analyze(
    "fn build() -> Invoice {\n  let id = 1;\n  Invoice { id, total: compute(), ..Default::default() }\n}\nimpl Point { fn origin() -> Self { Self { x: 0 } } }\nfn variant() { let _ = api::Event::Created { at: NOW }; }\n",
  );
  assert_eq!(
    refs_of(&facts, RefKind::Instantiation),
    [
      ("build".to_owned(), strings(&["Invoice"])),
      ("origin".to_owned(), strings(&["Self"])),
      ("variant".to_owned(), strings(&["api", "Event", "Created"]))
    ]
  );
  assert_eq!(paths_of(&facts, RefKind::Call), ["compute", "Default::default"]);
  assert_eq!(paths_of(&facts, RefKind::ValueRef), ["NOW"]);
}

#[test]
fn should_emit_type_refs_for_signatures_fields_bounds_and_annotations() {
  let source = "struct Holder<T: Store> where T: Clone { inner: Arc<Mutex<T>>, count: usize }\nimpl<T> Repository for Holder<T> where T: Store {\n  fn load<'a, K: Key>(&'a self, key: K) -> Result<Self::Item, Error> where K: Debug {\n    let cache: Option<&Cache> = None;\n    let values = items.iter().collect::<Vec<Value>>();\n    let size = 1 as Size;\n    Ok(todo!())\n  }\n}\n";
  let facts = analyze(source);
  assert_eq!(
    refs_of(&facts, RefKind::TypeRef),
    [
      ("Holder".to_owned(), strings(&["Store"])),
      ("Holder".to_owned(), strings(&["Clone"])),
      ("inner".to_owned(), strings(&["Arc"])),
      ("inner".to_owned(), strings(&["Mutex"])),
      ("<file>".to_owned(), strings(&["Repository"])),
      ("<file>".to_owned(), strings(&["Holder"])),
      ("<file>".to_owned(), strings(&["Store"])),
      ("load".to_owned(), strings(&["Key"])),
      ("load".to_owned(), strings(&["Result"])),
      ("load".to_owned(), strings(&["Error"])),
      ("load".to_owned(), strings(&["Debug"])),
      ("load".to_owned(), strings(&["Option"])),
      ("load".to_owned(), strings(&["Cache"])),
      ("load".to_owned(), strings(&["Vec"])),
      ("load".to_owned(), strings(&["Value"])),
      ("load".to_owned(), strings(&["Size"])),
    ]
  );
}

#[test]
fn should_dedupe_type_refs_per_symbol() {
  let facts = analyze("fn convert(a: Money, b: Money) -> Money { let c: Money = a; c }\nfn other(a: Money) {}\n");
  assert_eq!(
    refs_of(&facts, RefKind::TypeRef),
    [("convert".to_owned(), strings(&["Money"])), ("other".to_owned(), strings(&["Money"]))]
  );
}

#[test]
fn should_emit_value_refs_for_functions_passed_as_values_but_not_for_locals() {
  let source = "fn wire(config: Config) {\n  let handler = make();\n  let (left, mut right) = split();\n  register(handler, left, right, config, on_event, |event| event.id);\n  for item in items { use_it(item); }\n  if let Some(found) = lookup { found; }\n  match message { Message::Text(body) => body, other => other };\n  fn local_helper() {}\n  run(local_helper);\n  T::method;\n}\n";
  let facts = analyze(source);
  assert_eq!(
    paths_of(&facts, RefKind::ValueRef),
    ["on_event", "event::id", "items", "lookup", "message", "Message::Text", "T::method"]
  );
}

#[test]
fn should_not_treat_generic_parameters_as_value_or_type_refs() {
  let facts = analyze("fn make<T: Default, const N: usize>() -> [T; N] { let x = N; T::default(); <T>::new() }\n");
  assert_eq!(paths_of(&facts, RefKind::ValueRef), Vec::<String>::new());
  assert_eq!(paths_of(&facts, RefKind::TypeRef), ["Default"]);
  assert_eq!(paths_of(&facts, RefKind::Call), ["T::default", "T::new"]);
}

#[test]
fn should_not_emit_calls_to_local_closures() {
  let facts = analyze("fn run() {\n  let parse = |raw: &str| raw.len();\n  parse(\"x\");\n  helper(parse);\n}\n");
  assert_eq!(paths_of(&facts, RefKind::Call), ["helper"]);
  assert_eq!(paths_of(&facts, RefKind::MethodCall), ["raw::len"]);
}

#[test]
fn should_summarise_call_arguments() {
  let source = r##"fn run() {
  call(
    "a\n\"b\"", r#"raw\n"#, 42, -7, 1.5, true, 'c', &config.port, &mut buffer,
    |req, _| req, format!("id-{}", id), Point { x: 1, ..base }, [1, 2], (a, b),
    "base".to_string() + "/x" + path, env::var("PORT")?, client.fetch(), (value), maybe.await, x as u8,
  );
}
"##;
  let facts = analyze(source);
  let args = reference(&facts, RefKind::Call, &["call"]).args.clone();
  assert_eq!(args.len(), ARG_MAX_WIDTH);
  assert_eq!(
    args,
    [
      text("a\n\"b\""),
      text("raw\\n"),
      ArgSummary::Number("42".into()),
      ArgSummary::Number("-7".into()),
      ArgSummary::Number("1.5".into()),
      ArgSummary::Bool(true),
      ArgSummary::Other("'c'".into()),
      path(&["config", "port"]),
      path(&["buffer"]),
      ArgSummary::Closure { params: strings(&["req", "_"]) },
      ArgSummary::Call {
        callee: strings(&["format!"]),
        args: vec![text("id-{}"), path(&["id"])],
      },
      ArgSummary::Object(vec![
        ObjectEntry {
          key: ObjectKey::Ident("x".into()),
          value: ArgSummary::Number("1".into()),
        },
        ObjectEntry {
          key: ObjectKey::Spread,
          value: path(&["base"]),
        },
      ]),
      ArgSummary::Array(vec![ArgSummary::Number("1".into()), ArgSummary::Number("2".into())]),
      ArgSummary::Array(vec![path(&["a"]), path(&["b"])]),
      ArgSummary::Concat(vec![text("base"), text("/x"), path(&["path"])]),
      ArgSummary::Truncated,
    ]
  );
}

#[test]
fn should_truncate_nested_arguments_beyond_the_depth_limit() {
  let facts = analyze("fn run() { outer(a(b(c(d(e))))); }\n");
  let nested = |callee: &str, inner: ArgSummary| ArgSummary::Call {
    callee: strings(&[callee]),
    args: vec![inner],
  };
  assert_eq!(
    reference(&facts, RefKind::Call, &["outer"]).args,
    [nested("a", nested("b", nested("c", nested("d", ArgSummary::Truncated))))]
  );
}

#[test]
fn should_summarise_the_remaining_argument_forms() {
  let facts = analyze("fn run() { call(env::var(\"PORT\")?, client.fetch(), (value), maybe.await, x as u8, if a { b } else { c }); }\n");
  assert_eq!(
    reference(&facts, RefKind::Call, &["call"]).args,
    [
      ArgSummary::Call {
        callee: strings(&["env", "var"]),
        args: vec![text("PORT")],
      },
      ArgSummary::Call {
        callee: strings(&["client", "fetch"]),
        args: vec![],
      },
      path(&["value"]),
      path(&["maybe"]),
      path(&["x"]),
      ArgSummary::Other("if a { b } else { c }".into()),
    ]
  );
}

#[test]
fn should_attribute_impl_header_type_refs_to_the_enclosing_module() {
  let facts = analyze("impl Display for Money {}\nmod inner { impl Clone for Thing {} }\n");
  assert_eq!(
    refs_of(&facts, RefKind::TypeRef),
    [
      ("<file>".to_owned(), strings(&["Display"])),
      ("<file>".to_owned(), strings(&["Money"])),
      ("inner".to_owned(), strings(&["Clone"])),
      ("inner".to_owned(), strings(&["Thing"])),
    ]
  );
}

#[test]
fn should_attribute_const_and_static_initialisers_to_the_constant() {
  let facts = analyze("static CLIENT: Lazy<Client> = Lazy::new(|| Client::new(BASE_URL));\n");
  assert_eq!(
    refs_of(&facts, RefKind::Call),
    [("CLIENT".to_owned(), strings(&["Lazy", "new"])), ("CLIENT".to_owned(), strings(&["Client", "new"]))]
  );
  assert_eq!(refs_of(&facts, RefKind::ValueRef), [("CLIENT".to_owned(), strings(&["BASE_URL"]))]);
}

#[test]
fn should_summarise_nested_builtin_macros_and_emit_each_invocation() {
  let facts = analyze("include!(concat!(env!(\"OUT_DIR\"), \"/constants.rs\"));\n");
  let env = ArgSummary::Call {
    callee: strings(&["env!"]),
    args: vec![text("OUT_DIR")],
  };
  assert_eq!(
    reference(&facts, RefKind::MacroCall, &["include"]).args,
    [ArgSummary::Call {
      callee: strings(&["concat!"]),
      args: vec![env.clone(), text("/constants.rs")],
    }]
  );
  assert_eq!(reference(&facts, RefKind::MacroCall, &["concat"]).args, [env, text("/constants.rs")]);
  assert_eq!(
    refs_of(&facts, RefKind::MacroCall).iter().map(|(from, _)| from.as_str()).collect::<Vec<_>>(),
    ["<file>", "<file>", "<file>"]
  );
}

#[test]
fn should_describe_the_head_segment_of_method_call_paths() {
  let source = "fn handler<T: Store>(State(state): State<AppState>, client: &mut Client, generic: &T) {\n  let cache = Cache::new();\n  let raw = make();\n  let typed: Arc<Store> = make();\n  let built = Config { retries: 1 };\n  state.acme.get_order();\n  cache.get();\n  raw.len();\n  typed.put();\n  built.apply();\n  generic.read();\n  client.send();\n  CONFIG.reload();\n}\n";
  let facts = analyze(source);
  let calls: Vec<(Vec<String>, Option<ReceiverHint>)> = facts
    .references
    .iter()
    .filter(|reference| reference.kind == RefKind::MethodCall)
    .map(|reference| (reference.path.clone(), reference.receiver.clone()))
    .collect();
  let hint = |segments: &[&str]| Some(ReceiverHint::TypeHint(strings(segments)));
  let declared = |type_text: &str, binding: DeclaredBinding| {
    Some(ReceiverHint::Declared {
      type_text: type_text.to_owned(),
      binding,
    })
  };
  assert_eq!(
    calls,
    [
      (strings(&["state", "acme", "get_order"]), hint(&["AppState"])),
      (strings(&["cache", "get"]), Some(ReceiverHint::Expr("cache".into()))),
      (strings(&["raw", "len"]), Some(ReceiverHint::Expr("raw".into()))),
      (strings(&["typed", "put"]), declared("Arc<Store>", DeclaredBinding::Let)),
      (strings(&["built", "apply"]), hint(&["Config"])),
      (strings(&["generic", "read"]), declared("&T", DeclaredBinding::Param)),
      (strings(&["client", "send"]), hint(&["Client"])),
      (strings(&["CONFIG", "reload"]), Some(ReceiverHint::Identifier("CONFIG".into()))),
    ]
  );
}

#[test]
fn should_parse_tracing_style_arguments_piece_by_piece() {
  let facts = analyze("fn log(id: u64) { info!(target: \"billing\", user = %id, debug = ?state.value, \"created\", compute()); }\n");
  let named = |name: &str, value: ArgSummary| ArgSummary::Named {
    name: name.into(),
    value: Box::new(value),
  };
  assert_eq!(
    reference(&facts, RefKind::MacroCall, &["info"]).args,
    [
      named("target", text("billing")),
      named("user", path(&["id"])),
      named("debug", path(&["state", "value"])),
      text("created"),
      ArgSummary::Call {
        callee: strings(&["compute"]),
        args: vec![],
      },
    ]
  );
  let compute = reference(&facts, RefKind::Call, &["compute"]);
  assert_eq!(compute.origin, RefOrigin::Syntax);
  assert_eq!(paths_of(&facts, RefKind::ValueRef), ["state::value"]);
}

#[test]
fn should_scan_unparseable_macro_bodies_for_call_shapes() {
  let source = "async fn serve(mut inbox: Receiver) {\n  tokio::select! {\n    Some(message) = inbox.recv() => handle(message, Mode::Fast),\n    _ = shutdown.changed() => { stop_server(); Report { code: 1 }; }\n  }\n}\n";
  let facts = analyze(source);
  let scanned: Vec<(RefKind, Vec<String>, RefOrigin)> = facts
    .references
    .iter()
    .filter(|reference| reference.kind != RefKind::TypeRef)
    .map(|reference| (reference.kind, reference.path.clone(), reference.origin))
    .collect();
  assert_eq!(
    scanned,
    [
      (RefKind::MacroCall, strings(&["tokio", "select"]), RefOrigin::Syntax),
      (RefKind::MethodCall, strings(&["inbox", "recv"]), RefOrigin::MacroTokenScan),
      (RefKind::Call, strings(&["handle"]), RefOrigin::MacroTokenScan),
      (RefKind::MethodCall, strings(&["shutdown", "changed"]), RefOrigin::MacroTokenScan),
      (RefKind::Call, strings(&["stop_server"]), RefOrigin::MacroTokenScan),
      (RefKind::Instantiation, strings(&["Report"]), RefOrigin::MacroTokenScan),
    ]
  );
  let handle = reference(&facts, RefKind::Call, &["handle"]);
  assert_eq!(handle.args, [path(&["message"]), path(&["Mode", "Fast"])]);
  assert_eq!(
    reference(&facts, RefKind::MethodCall, &["inbox", "recv"]).receiver,
    Some(ReceiverHint::TypeHint(strings(&["Receiver"])))
  );
  assert_eq!(
    reference(&facts, RefKind::MethodCall, &["shutdown", "changed"]).receiver,
    Some(ReceiverHint::Identifier("shutdown".into()))
  );
  let start = source.find("handle").unwrap();
  assert_eq!(handle.span, Span::of_text(source, start, start + "handle".len()).unwrap());
}

#[test]
fn should_not_turn_dsl_identifiers_into_references() {
  let facts = analyze("routes! {\n  GET \"/users\" => list_users,\n  fn helper(x: u8) -> u8;\n  struct Wrapper(u8);\n  POST \"/users\" => create_user(payload),\n}\n");
  let refs: Vec<(RefKind, String, RefOrigin)> = facts
    .references
    .iter()
    .map(|reference| (reference.kind, reference.path.join("::"), reference.origin))
    .collect();
  assert_eq!(
    refs,
    [
      (RefKind::MacroCall, "routes".to_owned(), RefOrigin::Syntax),
      (RefKind::Call, "create_user".to_owned(), RefOrigin::MacroTokenScan),
    ]
  );
}

fn value_refs_with_receivers(facts: &atlas_core::FileFacts) -> Vec<(String, Option<ReceiverHint>)> {
  facts
    .references
    .iter()
    .filter(|reference| reference.kind == RefKind::ValueRef && reference.receiver.is_some())
    .map(|reference| (reference.path.join("."), reference.receiver.clone()))
    .collect()
}

#[test]
fn should_emit_field_reads_and_writes_but_not_method_callees() {
  let source = "impl Counter {\n  fn bump(&mut self, other: &Counter) {\n    self.count += 1;\n    self.config.name = other.config.name.clone();\n    let pair = make();\n    pair.0;\n    pair.1.label;\n    (self.callback)(1);\n    self.store.get();\n    make().field;\n    a::B.field;\n  }\n}\n";
  let facts = analyze(source);
  let hint = |segments: &[&str]| Some(ReceiverHint::TypeHint(strings(segments)));
  assert_eq!(
    value_refs_with_receivers(&facts),
    [
      ("self.count".to_owned(), Some(ReceiverHint::SelfValue)),
      ("self.config.name".to_owned(), Some(ReceiverHint::SelfValue)),
      ("self.config".to_owned(), Some(ReceiverHint::SelfValue)),
      ("other.config.name".to_owned(), hint(&["Counter"])),
      ("other.config".to_owned(), hint(&["Counter"])),
      ("pair.1.label".to_owned(), Some(ReceiverHint::Expr("pair".into()))),
      ("self.callback".to_owned(), Some(ReceiverHint::SelfValue)),
      ("self.store".to_owned(), Some(ReceiverHint::SelfValue)),
    ]
  );
  let count = reference(&facts, RefKind::ValueRef, &["self", "count"]);
  let prefix = "impl Counter {\n  fn bump(&mut self, other: &Counter) {\n    self.";
  assert_eq!(count.span, Span::of_text(source, prefix.len(), prefix.len() + "count".len()).unwrap());
  assert_eq!(paths_of(&facts, RefKind::MethodCall), ["other::config::name::clone", "self::store::get"]);
}

#[test]
fn should_keep_at_most_four_field_access_spans_per_symbol_and_receiver() {
  let facts = analyze(
    "fn sum(&self) -> u32 { self.n + self.n + self.n + self.n + self.n }\nfn other(&self) -> u32 { self.n }\nfn shadow(n: &Num) -> u32 { n.v + { let n = make(); n.v } }\n",
  );
  let fields: Vec<(String, Vec<String>)> = refs_of(&facts, RefKind::ValueRef);
  assert_eq!(
    fields,
    [
      ("sum".to_owned(), strings(&["self", "n"])),
      ("sum".to_owned(), strings(&["self", "n"])),
      ("sum".to_owned(), strings(&["self", "n"])),
      ("sum".to_owned(), strings(&["self", "n"])),
      ("other".to_owned(), strings(&["self", "n"])),
      ("shadow".to_owned(), strings(&["n", "v"])),
      ("shadow".to_owned(), strings(&["n", "v"])),
    ]
  );
  let shadow_hints: Vec<Option<ReceiverHint>> = facts
    .references
    .iter()
    .filter(|reference| reference.path == ["n", "v"])
    .map(|reference| reference.receiver.clone())
    .collect();
  assert_eq!(shadow_hints, [Some(ReceiverHint::TypeHint(strings(&["Num"]))), Some(ReceiverHint::Expr("n".into()))]);
}

#[test]
fn should_carry_declared_type_text_for_generic_and_wrapper_bindings_only_while_they_are_in_scope() {
  let source = "fn run<T: Store>(a: &T, b: Box<dyn Store>, c: &mut Arc<Pg>, d: Pg, e: T::Key, State(f): State<Rc<App>>) {\n  a.get();\n  b.put();\n  c.save();\n  d.save();\n  e.len();\n  f.go();\n  let g: Arc<T> = make();\n  g.get();\n  let a = Mem::new();\n  a.get();\n  let h = |b: Box<Mem>| b.put();\n  b.put();\n  fn nested<U: Store>(u: &U, w: Arc<Pg>) { u.get(); w.save(); }\n}\n";
  let facts = analyze(source);
  let declared = |type_text: &str, binding: DeclaredBinding| {
    Some(ReceiverHint::Declared {
      type_text: type_text.to_owned(),
      binding,
    })
  };
  let calls: Vec<(String, Option<ReceiverHint>)> = facts
    .references
    .iter()
    .filter(|reference| reference.kind == RefKind::MethodCall)
    .map(|reference| (reference.path.join("."), reference.receiver.clone()))
    .collect();
  assert_eq!(
    calls,
    [
      ("a.get".to_owned(), declared("&T", DeclaredBinding::Param)),
      ("b.put".to_owned(), declared("Box<dyn Store>", DeclaredBinding::Param)),
      ("c.save".to_owned(), declared("&mut Arc<Pg>", DeclaredBinding::Param)),
      ("d.save".to_owned(), Some(ReceiverHint::TypeHint(strings(&["Pg"])))),
      ("e.len".to_owned(), Some(ReceiverHint::Expr("e".into()))),
      ("f.go".to_owned(), declared("Rc<App>", DeclaredBinding::Param)),
      ("g.get".to_owned(), declared("Arc<T>", DeclaredBinding::Let)),
      ("a.get".to_owned(), Some(ReceiverHint::Expr("a".into()))),
      ("b.put".to_owned(), declared("Box<Mem>", DeclaredBinding::Param)),
      ("b.put".to_owned(), declared("Box<dyn Store>", DeclaredBinding::Param)),
      ("u.get".to_owned(), Some(ReceiverHint::Expr("u".into()))),
      ("w.save".to_owned(), Some(ReceiverHint::TypeHint(strings(&["Arc"])))),
    ]
  );
}

#[test]
fn should_hide_the_outer_self_from_items_nested_in_a_method_body() {
  let facts = analyze(
    "impl Foo {\n  fn m(&self, other: &Self) {\n    self.a;\n    Self::make();\n    impl L { fn g(&self, x: &Self) { self.a; self.go(); Self::make(); let y = Self { a: 1 }; x.go(); y.go(); } }\n  }\n}\n",
  );
  let receivers: Vec<(String, Option<ReceiverHint>)> = facts
    .references
    .iter()
    .filter(|reference| reference.receiver.is_some())
    .map(|reference| (reference.path.join("."), reference.receiver.clone()))
    .collect();
  let expr = |name: &str| Some(ReceiverHint::Expr(name.into()));
  assert_eq!(
    receivers,
    [
      ("self.a".to_owned(), Some(ReceiverHint::SelfValue)),
      ("self.a".to_owned(), expr("self")),
      ("self.go".to_owned(), expr("self")),
      ("x.go".to_owned(), expr("x")),
      ("y.go".to_owned(), expr("y")),
    ]
  );
  assert_eq!(paths_of(&facts, RefKind::Call), ["Self::make", "(Self)::make"]);
  assert_eq!(paths_of(&facts, RefKind::Instantiation), ["(Self)"]);
}

#[test]
fn should_describe_call_and_try_receivers_as_chains_with_declared_heads() {
  let facts = analyze(
    "async fn run(graph: &Graph<'_>, budget: &Mutex<Budget>, plain: Plain) {\n  graph.tx().entity_by_key(1)?.unwrap().label();\n  lock(&budget, 3, plain).try_reserve(1);\n  Store::<u8>::open().await.flush();\n  (graph.tx()).find();\n  let local = Mutex::new(Budget);\n  lock(&local).spend();\n}\n",
  );
  let declared = |type_text: &str| ReceiverHint::Declared {
    type_text: type_text.to_owned(),
    binding: DeclaredBinding::Param,
  };
  assert_eq!(
    receiver_of(&facts, "label"),
    [&chained(
      binding("graph", declared("&Graph<'_>")),
      vec![
        ChainStep::Method("tx".into()),
        ChainStep::Method("entity_by_key".into()),
        ChainStep::Try,
        ChainStep::Method("unwrap".into()),
      ]
    )]
  );
  assert_eq!(
    receiver_of(&facts, "try_reserve"),
    [&chained(call(&["lock"], vec![Some("&Mutex<Budget>".into()), None, Some("Plain".into())]), vec![])]
  );
  assert_eq!(receiver_of(&facts, "flush"), [&chained(call(&["Store", "open"], vec![]), vec![ChainStep::Await])]);
  assert_eq!(
    receiver_of(&facts, "find"),
    [&chained(binding("graph", declared("&Graph<'_>")), vec![ChainStep::Method("tx".into())])]
  );
  assert_eq!(receiver_of(&facts, "spend"), [&chained(call(&["lock"], vec![None]), vec![])]);
}

#[test]
fn should_keep_expression_receivers_for_closures_generics_nested_items_and_long_chains() {
  let long = format!("x(){}", ".a()".repeat(9));
  let facts = analyze(&format!(
    "fn run<T: Make>(items: Vec<u8>) {{\n  let make = || 1;\n  make().closure();\n  T::new().generic();\n  items[0].indexed();\n  {long}.long();\n  fn nested(s: &S) {{ s.get().inner(); }}\n  let untyped = make();\n  untyped.get().untyped();\n}}\n"
  ));
  for method in ["closure", "generic", "indexed", "long", "inner", "untyped"] {
    let hints = receiver_of(&facts, method);
    assert!(matches!(hints.as_slice(), [ReceiverHint::Expr(_)]), "{method}: {hints:?}");
  }
  let eight = analyze(&format!("fn run() {{ x(){}.last(); }}\n", ".a()".repeat(8)));
  assert!(matches!(receiver_of(&eight, "last").as_slice(), [ReceiverHint::Chain(chain)] if chain.steps.len() == 8));
}
