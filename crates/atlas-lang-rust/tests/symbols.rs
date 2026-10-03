mod common;

use atlas_core::{
  ArgSummary, AttributeFact, EntityKind, ImplInfo, KeyContext, LocalSymbolId, Modifier, Modifiers, ModuleBody, ParamFact, RefKind, Span, TestInfo, TestKind, TypePath, Visibility,
};
use common::{analyze, analyze_at, item, refs_of, strings, symbol, symbol_names};

type TestSummary = (String, EntityKind, Option<(String, TestKind)>);

#[test]
fn should_extract_free_functions_with_signature_visibility_and_modifiers() {
  let source = "pub async unsafe fn load<T: Clone>(\n  id: u64,\n) -> Option<T> where T: Default {\n  None\n}\npub(crate) const extern \"C\" fn raw() {}\n";
  let facts = analyze(source);
  let load = item(&facts, &["load"]);
  assert_eq!(load.kind, EntityKind::Function);
  assert_eq!(load.visibility, Visibility::Public);
  assert_eq!(
    load.signature.as_deref(),
    Some("pub async unsafe fn load<T: Clone>( id: u64, ) -> Option<T> where T: Default")
  );
  assert_eq!(load.modifiers, Modifiers::from(vec![Modifier::Async, Modifier::Unsafe, Modifier::Generic]));
  assert_eq!(load.span, Span::of_text(source, 0, source.find("}\n").unwrap() + 1).unwrap());
  assert_eq!(load.name_span, Span::of_text(source, 20, 24).unwrap());
  let raw = item(&facts, &["raw"]);
  assert_eq!(raw.visibility, Visibility::Crate);
  assert_eq!(raw.modifiers, Modifiers::from(vec![Modifier::Const, Modifier::Extern]));
  assert_eq!(raw.signature.as_deref(), Some("pub(crate) const extern \"C\" fn raw()"));
}

#[test]
fn should_key_inherent_methods_under_the_self_type() {
  let facts = analyze("struct Store;\nimpl Store {\n  pub fn open(&self) {}\n}\n");
  let open = item(&facts, &["Store", "open"]);
  assert_eq!(open.kind, EntityKind::Method);
  assert_eq!(open.parent, None);
  assert_eq!(
    open.impl_of,
    Some(ImplInfo {
      self_type: TypePath::new(strings(&["Store"])),
      trait_ref: None,
    })
  );
}

#[test]
fn should_key_trait_impl_methods_with_the_trait_segment() {
  let facts = analyze("impl From<io::Error> for AppError {\n  fn from(error: io::Error) -> Self { todo!() }\n}\nimpl fmt::Display for AppError {\n  fn fmt(&self) {}\n}\n");
  let from = item(&facts, &["AppError", "<From<io::Error>>", "from"]);
  assert_eq!(from.kind, EntityKind::Method);
  assert_eq!(from.visibility, Visibility::Public);
  assert_eq!(
    from.impl_of,
    Some(ImplInfo {
      self_type: TypePath::new(strings(&["AppError"])),
      trait_ref: Some(TypePath {
        segments: strings(&["From"]),
        generic_args: Some("io::Error".into()),
      }),
    })
  );
  let fmt = item(&facts, &["AppError", "<Display>", "fmt"]);
  assert_eq!(
    fmt.impl_of.and_then(|info| info.trait_ref).map(|trait_ref| trait_ref.segments),
    Some(strings(&["fmt", "Display"]))
  );
}

#[test]
fn should_strip_generics_from_impl_self_types_into_generic_args() {
  let facts = analyze("impl<T: Send> crate::cache::Cache<T, u8> {\n  fn get(&self) {}\n}\nimpl<'a> Handler for &'a [u8] {\n  fn call(&self) {}\n}\n");
  let get = item(&facts, &["Cache", "get"]);
  assert_eq!(
    get.impl_of.map(|info| info.self_type),
    Some(TypePath {
      segments: strings(&["crate", "cache", "Cache"]),
      generic_args: Some("T, u8".into()),
    })
  );
  let call = item(&facts, &["&'a [u8]", "<Handler>", "call"]);
  assert_eq!(call.impl_of.map(|info| info.self_type), Some(TypePath::new(strings(&["&'a [u8]"]))));
}

#[test]
fn should_extract_trait_declarations_and_default_methods() {
  let facts = analyze("/// Persists rows.\npub trait Store: Send + Sync {\n  fn get(&self) -> u8;\n  fn has(&self) -> bool { true }\n}\n");
  let store = item(&facts, &["Store"]);
  assert_eq!(store.kind, EntityKind::Trait);
  assert_eq!(store.doc.as_deref(), Some("Persists rows."));
  assert_eq!(store.extends, vec![strings(&["Send"]), strings(&["Sync"])]);
  assert_eq!(store.signature.as_deref(), Some("pub trait Store: Send + Sync"));
  let get = item(&facts, &["Store", "get"]);
  assert_eq!((get.kind, get.parent, get.visibility.clone()), (EntityKind::Method, Some(store.local), Visibility::Public));
  assert_eq!(get.modifiers, Modifiers::from(vec![Modifier::Abstract]));
  assert_eq!(get.signature.as_deref(), Some("fn get(&self) -> u8"));
  assert_eq!(get.body, None);
  let has = item(&facts, &["Store", "has"]);
  assert_eq!(has.modifiers, Modifiers::from(vec![Modifier::Default]));
  assert!(has.body.is_some());
}

#[test]
fn should_extract_named_struct_fields_but_not_tuple_fields() {
  let facts = analyze("pub struct Point<T> where T: Copy {\n  /// Horizontal.\n  pub x: T,\n  y: i64,\n}\npub struct Meters(pub f64);\npub struct Unit;\n");
  assert_eq!(symbol_names(&facts), ["Point#struct", "Point::x#field", "Point::y#field", "Meters#struct", "Unit#struct"]);
  let point = item(&facts, &["Point"]);
  assert_eq!(point.signature.as_deref(), Some("pub struct Point<T> where T: Copy"));
  assert_eq!(point.modifiers, Modifiers::from(vec![Modifier::Generic]));
  let x = item(&facts, &["Point", "x"]);
  assert_eq!(
    (x.parent, x.visibility.clone(), x.doc.as_deref()),
    (Some(point.local), Visibility::Public, Some("Horizontal."))
  );
  assert_eq!(x.signature.as_deref(), Some("pub x: T"));
  assert_eq!(item(&facts, &["Point", "y"]).visibility, Visibility::Private);
  assert_eq!(item(&facts, &["Meters"]).signature.as_deref(), Some("pub struct Meters(pub f64)"));
}

#[test]
fn should_treat_unions_as_structs() {
  let facts = analyze("union Bits { int: u32, float: f32 }\n");
  assert_eq!(symbol_names(&facts), ["Bits#struct", "Bits::int#field", "Bits::float#field"]);
  assert_eq!(item(&facts, &["Bits"]).signature.as_deref(), Some("union Bits"));
}

#[test]
fn should_extract_enums_and_variants_with_enum_visibility() {
  let facts = analyze("pub(crate) enum Shape {\n  #[default]\n  Empty,\n  Circle(f64),\n  Rect { w: f64, h: f64 },\n  Code = 7,\n}\n");
  assert_eq!(
    symbol_names(&facts),
    ["Shape#enum", "Shape::Empty#variant", "Shape::Circle#variant", "Shape::Rect#variant", "Shape::Code#variant"]
  );
  let shape = item(&facts, &["Shape"]);
  let empty = item(&facts, &["Shape", "Empty"]);
  assert_eq!((empty.parent, empty.visibility.clone()), (Some(shape.local), Visibility::Crate));
  assert_eq!(empty.attributes.iter().map(|attribute| attribute.path.clone()).collect::<Vec<_>>(), [strings(&["default"])]);
  assert_eq!(item(&facts, &["Shape", "Rect"]).signature.as_deref(), Some("Rect { w: f64, h: f64 }"));
  assert_eq!(item(&facts, &["Shape", "Code"]).signature.as_deref(), Some("Code = 7"));
}

#[test]
fn should_extract_type_aliases_consts_and_statics() {
  let facts = analyze("pub type Result<T> = std::result::Result<T, Error>;\npub const LIMIT: usize = 3;\nstatic mut COUNTER: u64 = 0;\nconst _: () = ();\n");
  assert_eq!(symbol_names(&facts), ["Result#type_alias", "LIMIT#constant", "COUNTER#constant"]);
  let alias = item(&facts, &["Result"]);
  assert_eq!(alias.signature.as_deref(), Some("pub type Result<T> = std::result::Result<T, Error>"));
  assert_eq!(alias.modifiers, Modifiers::from(vec![Modifier::Generic]));
  assert_eq!(item(&facts, &["LIMIT"]).signature.as_deref(), Some("pub const LIMIT: usize"));
  let counter = item(&facts, &["COUNTER"]);
  assert_eq!(counter.modifiers, Modifiers::from(vec![Modifier::Static, Modifier::Mutable]));
  assert_eq!(counter.signature.as_deref(), Some("static mut COUNTER: u64"));
}

#[test]
fn should_extract_inline_and_declared_modules_with_module_suffixes() {
  let source = "#[path = \"gen/api.rs\"]\npub mod api;\n#[cfg(feature = \"db\")]\nmod db;\nmod outer {\n  pub(super) mod inner {\n    pub fn deep() {}\n  }\n}\n";
  let facts = analyze(source);
  assert_eq!(
    symbol_names(&facts),
    ["api#module", "db#module", "outer#module", "outer::inner#module", "outer::inner::deep#function"]
  );
  let outer = symbol(&facts, &["outer"], &[]);
  let inner = symbol(&facts, &["outer", "inner"], &[]);
  let deep = symbol(&facts, &["outer", "inner"], &["deep"]);
  assert_eq!((outer.parent, inner.parent, deep.parent), (None, Some(outer.local), Some(inner.local)));
  assert_eq!(inner.visibility, Visibility::Restricted("super".into()));
  let declarations: Vec<_> = facts
    .module_decls
    .iter()
    .map(|declaration| {
      (
        declaration.name.as_str(),
        declaration.body.clone(),
        declaration.path_attr.as_deref(),
        declaration.module_suffix.clone(),
      )
    })
    .collect();
  assert_eq!(
    declarations,
    [
      ("api", ModuleBody::External, Some("gen/api.rs"), vec![]),
      ("db", ModuleBody::External, None, vec![]),
      ("outer", ModuleBody::Inline(outer.local), None, vec![]),
      ("inner", ModuleBody::Inline(inner.local), None, strings(&["outer"])),
    ]
  );
  assert_eq!(
    facts.module_decls[1].attributes[0],
    AttributeFact {
      path: strings(&["cfg"]),
      args: vec![ArgSummary::Named {
        name: "feature".into(),
        value: Box::new(ArgSummary::Str("db".into())),
      }],
      span: Span::of_text(source, source.find("#[cfg").unwrap(), source.find("\nmod db").unwrap()).unwrap(),
    }
  );
  assert_eq!(facts.module_decls[0].visibility, Visibility::Public);
  assert_eq!(facts.module_decls[0].span, Span::of_text(source, 0, source.find("\n#[cfg").unwrap()).unwrap());
}

#[test]
fn should_build_unique_symbol_keys_for_inline_modules() {
  let facts = analyze("mod tests {\n  fn helper() {}\n}\nfn helper() {}\n");
  let context = KeyContext {
    package: "billing".into(),
    module_path: strings(&["money"]),
  };
  let keys: Vec<String> = facts.symbol_keys(&context).unwrap().iter().map(ToString::to_string).collect();
  assert_eq!(
    keys,
    [
      "rust:fixture:billing:money/tests::#module",
      "rust:fixture:billing:money/tests::helper#function",
      "rust:fixture:billing:money::helper#function",
    ]
  );
}

#[test]
fn should_extract_macro_rules_with_export_visibility() {
  let facts = analyze("#[macro_export]\nmacro_rules! public_macro { () => {} }\nmacro_rules! local_macro { ($x:expr) => { $x } }\n");
  let public = item(&facts, &["public_macro"]);
  assert_eq!((public.kind.clone(), public.visibility.clone()), (EntityKind::Macro, Visibility::Public));
  assert_eq!(public.signature.as_deref(), Some("macro_rules! public_macro"));
  assert_eq!(item(&facts, &["local_macro"]).visibility, Visibility::Private);
  assert!(facts.references.is_empty());
}

#[test]
fn should_not_extract_items_nested_in_function_bodies() {
  let facts =
    analyze("fn outer() {\n  struct Local { field: Shared }\n  fn inner(value: Input) { helper(value); }\n  impl Local { fn method(&self) { other(); } }\n  inner(Input);\n}\n");
  assert_eq!(symbol_names(&facts), ["outer#function"]);
  assert_eq!(
    refs_of(&facts, RefKind::Call),
    [("outer".to_owned(), strings(&["helper"])), ("outer".to_owned(), strings(&["other"]))]
  );
  assert_eq!(
    refs_of(&facts, RefKind::TypeRef),
    [
      ("outer".to_owned(), strings(&["Shared"])),
      ("outer".to_owned(), strings(&["Input"])),
      ("outer".to_owned(), strings(&["Local"]))
    ]
  );
}

#[test]
fn should_extract_associated_types_and_consts_in_traits_and_impls() {
  let facts =
    analyze("trait Codec {\n  type Output: Clone;\n  const NAME: &'static str;\n}\nimpl Codec for Json {\n  type Output = String;\n  const NAME: &'static str = \"json\";\n}\n");
  assert_eq!(
    symbol_names(&facts),
    [
      "Codec#trait",
      "Codec::Output#type_alias",
      "Codec::NAME#constant",
      "Json::<Codec>::Output#type_alias",
      "Json::<Codec>::NAME#constant"
    ]
  );
  let output = item(&facts, &["Codec", "Output"]);
  assert_eq!(output.parent, Some(item(&facts, &["Codec"]).local));
  assert_eq!(output.signature.as_deref(), Some("type Output: Clone"));
  let implemented = item(&facts, &["Json", "<Codec>", "NAME"]);
  assert_eq!(implemented.impl_of.map(|info| info.self_type.segments), Some(strings(&["Json"])));
  assert_eq!(implemented.signature.as_deref(), Some("const NAME: &'static str"));
}

#[test]
fn should_extract_extern_block_functions() {
  let facts = analyze("extern \"C\" {\n  pub fn abs(input: i32) -> i32;\n  static errno: i32;\n}\n");
  let abs = item(&facts, &["abs"]);
  assert_eq!((abs.kind.clone(), abs.modifiers), (EntityKind::Function, Modifiers::from(vec![Modifier::Extern])));
  assert_eq!(abs.signature.as_deref(), Some("pub fn abs(input: i32) -> i32"));
  assert_eq!(item(&facts, &["errno"]).modifiers, Modifiers::from(vec![Modifier::Static, Modifier::Extern]));
}

#[test]
fn should_record_traits_implemented_by_types_declared_in_the_same_module() {
  let facts = analyze(
    "impl std::error::Error for ApiError {}\nenum ApiError {}\nimpl From<u8> for ApiError { fn from(_: u8) -> Self { todo!() } }\nimpl From<u16> for ApiError { fn from(_: u16) -> Self { todo!() } }\nmod nested { impl super::Marker for ApiError {} }\nimpl Marker for Vec<ApiError> {}\n",
  );
  assert_eq!(item(&facts, &["ApiError"]).implements, vec![strings(&["std", "error", "Error"]), strings(&["From"])]);
}

#[test]
fn should_include_attributes_and_docs_in_symbol_spans() {
  let source = "// license header\n\n/// Says hi.\n#[inline]\n// note\npub fn hi() {}\n";
  let facts = analyze(source);
  let hi = item(&facts, &["hi"]);
  let start = source.find("/// Says").unwrap();
  assert_eq!(hi.span, Span::of_text(source, start, source.len() - 1).unwrap());
  assert_eq!(hi.doc.as_deref(), Some("Says hi."));
}

#[test]
fn should_collect_outer_docs_from_line_block_and_doc_attributes() {
  let facts = analyze("/// First line.\n///\n///   Indented.\n/** Block\n * second */\n#[doc = \" From attribute.\"]\nfn documented() {}\n");
  assert_eq!(
    item(&facts, &["documented"]).doc.as_deref(),
    Some("First line.\n\n  Indented.\nBlock\nsecond\nFrom attribute.")
  );
}

#[test]
fn should_attach_inner_docs_to_inline_modules_and_file_docs_to_the_file() {
  let facts = analyze("//! Crate docs.\n\n/// Outer.\nmod api {\n  //! Inner.\n  #![allow(dead_code)]\n}\nfn after() {}\n");
  let api = symbol(&facts, &["api"], &[]);
  assert_eq!(api.doc.as_deref(), Some("Outer.\nInner."));
  assert_eq!(api.attributes.iter().map(|attribute| attribute.path.clone()).collect::<Vec<_>>(), [strings(&["allow"])]);
  assert_eq!(item(&facts, &["after"]).doc, None);
  assert_eq!(facts.file_doc.as_deref(), Some("Crate docs."));
  let attributed = analyze("//! First.\n/*! Second. */\n#![doc = \" Third.\"]\nfn f() {}\n");
  assert_eq!(attributed.file_doc.as_deref(), Some("First.\nSecond.\nThird."));
  assert_eq!(analyze("/// Item doc.\nfn f() {}\n").file_doc, None);
}

#[test]
fn should_map_every_visibility_form() {
  let facts = analyze("pub fn a() {}\npub(crate) fn b() {}\npub(super) fn c() {}\npub(in crate::store) fn d() {}\npub(self) fn e() {}\nfn f() {}\n");
  let visibilities: Vec<Visibility> = facts.symbols.iter().map(|symbol| symbol.visibility.clone()).collect();
  assert_eq!(
    visibilities,
    [
      Visibility::Public,
      Visibility::Crate,
      Visibility::Restricted("super".into()),
      Visibility::Restricted("crate::store".into()),
      Visibility::Private,
      Visibility::Private,
    ]
  );
}

#[test]
fn should_summarise_attribute_arguments() {
  let source = r#"#[derive(Debug, Clone, serde::Serialize)]
#[cfg(all(test, feature = "db"))]
#[get("/users/{id}")]
#[async_trait]
#[serde(rename = "userId", default)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_arguments)]
#[deprecated = "use v2"]
#[instrument(skip(self), fields(user = %id))]
fn handler() {}
"#;
  let facts = analyze(source);
  let attributes: Vec<(Vec<String>, Vec<ArgSummary>)> = item(&facts, &["handler"])
    .attributes
    .into_iter()
    .map(|attribute| (attribute.path, attribute.args))
    .collect();
  let path = |segments: &[&str]| ArgSummary::Path(strings(segments));
  let named = |name: &str, value: ArgSummary| ArgSummary::Named {
    name: name.into(),
    value: Box::new(value),
  };
  assert_eq!(
    attributes,
    [
      (strings(&["derive"]), vec![path(&["Debug"]), path(&["Clone"]), path(&["serde", "Serialize"])]),
      (
        strings(&["cfg"]),
        vec![ArgSummary::Call {
          callee: strings(&["all"]),
          args: vec![path(&["test"]), named("feature", ArgSummary::Str("db".into()))],
        }]
      ),
      (strings(&["get"]), vec![ArgSummary::Str("/users/{id}".into())]),
      (strings(&["async_trait"]), vec![]),
      (strings(&["serde"]), vec![named("rename", ArgSummary::Str("userId".into())), path(&["default"])]),
      (
        strings(&["tokio", "test"]),
        vec![
          named("flavor", ArgSummary::Str("multi_thread".into())),
          named("worker_threads", ArgSummary::Number("2".into()))
        ]
      ),
      (strings(&["allow"]), vec![path(&["clippy", "too_many_arguments"])]),
      (strings(&["deprecated"]), vec![ArgSummary::Str("use v2".into())]),
      (
        strings(&["instrument"]),
        vec![
          ArgSummary::Call {
            callee: strings(&["skip"]),
            args: vec![path(&["self"])],
          },
          ArgSummary::Call {
            callee: strings(&["fields"]),
            args: vec![ArgSummary::Named {
              name: "user".into(),
              value: Box::new(ArgSummary::Other("%id".into())),
            }],
          },
        ]
      ),
    ]
  );
}

#[test]
fn should_extract_params_with_patterns_types_and_attributes() {
  let source =
    "async fn get_user(State(state): State<AppState>, Path(id): Path<u64>, #[allow(unused)] mut extra: Vec<u8>) {}\nimpl S { fn by_ref(&mut self, (a, b): (u8, u8)) {} }\n";
  let facts = analyze(source);
  let params: Vec<(String, Option<String>, usize)> = item(&facts, &["get_user"])
    .params
    .into_iter()
    .map(|param| (param.name, param.type_text, param.attributes.len()))
    .collect();
  assert_eq!(
    params,
    [
      ("State(state)".to_owned(), Some("State<AppState>".to_owned()), 0),
      ("Path(id)".to_owned(), Some("Path<u64>".to_owned()), 0),
      ("extra".to_owned(), Some("Vec<u8>".to_owned()), 1),
    ]
  );
  let by_ref = item(&facts, &["S", "by_ref"]);
  assert_eq!(
    by_ref.params[0],
    ParamFact {
      name: "self".into(),
      type_text: Some("&mut self".into()),
      attributes: vec![],
      span: Span::of_text(source, source.find("&mut self").unwrap(), source.find("&mut self").unwrap() + 9).unwrap(),
    }
  );
  assert_eq!((by_ref.params[1].name.as_str(), by_ref.params[1].type_text.as_deref()), ("(a, b)", Some("(u8, u8)")));
}

#[test]
fn should_classify_unit_tests_benches_and_suites() {
  let facts =
    analyze("#[cfg(test)]\nmod tests {\n  #[test]\n  fn adds() {}\n  #[tokio::test]\n  async fn fetches() {}\n  #[bench]\n  fn speed(b: &mut Bencher) {}\n  fn helper() {}\n}\n");
  let suite = symbol(&facts, &["tests"], &[]);
  assert_eq!(suite.kind, EntityKind::Module);
  assert_eq!(
    suite
      .attributes
      .iter()
      .map(|attribute| (attribute.path.clone(), attribute.args.clone()))
      .collect::<Vec<_>>(),
    [(strings(&["cfg"]), vec![ArgSummary::Path(strings(&["test"]))])]
  );
  assert_eq!(
    suite.test,
    Some(TestInfo {
      framework: "rust".into(),
      kind: TestKind::Suite,
      display_name: None,
    })
  );
  let tests: Vec<TestSummary> = facts.symbols[1..]
    .iter()
    .map(|symbol| (symbol.name.clone(), symbol.kind.clone(), symbol.test.clone().map(|test| (test.framework, test.kind))))
    .collect();
  assert_eq!(
    tests,
    [
      ("adds".to_owned(), EntityKind::Test, Some(("rust".to_owned(), TestKind::Unit))),
      ("fetches".to_owned(), EntityKind::Test, Some(("tokio".to_owned(), TestKind::Unit))),
      ("speed".to_owned(), EntityKind::Test, Some(("rust".to_owned(), TestKind::Bench))),
      ("helper".to_owned(), EntityKind::Function, None),
    ]
  );
}

#[test]
fn should_classify_tests_under_a_tests_directory_as_integration_tests() {
  let source = "#[test]\nfn end_to_end() {}\n";
  let integration = analyze_at("crates/billing/tests/api.rs", source);
  assert_eq!(integration.symbols[0].test.as_ref().map(|test| test.kind), Some(TestKind::Integration));
  let unit = analyze_at("src/tests/api.rs", source);
  assert_eq!(unit.symbols[0].test.as_ref().map(|test| test.kind), Some(TestKind::Unit));
}

#[test]
fn should_strip_comments_from_signatures() {
  let facts = analyze("pub fn configure(\n  // the port\n  port: u16, /* inline */ host: &str,\n) -> bool { true }\n");
  assert_eq!(
    item(&facts, &["configure"]).signature.as_deref(),
    Some("pub fn configure( port: u16, host: &str, ) -> bool")
  );
}

#[test]
fn should_strip_raw_identifier_prefixes_from_names_and_paths() {
  let facts = analyze(
    "pub fn r#match(value: u8) { r#loop(); crate::r#type::run(); }
",
  );
  assert_eq!(symbol_names(&facts), ["match#function"]);
  assert_eq!(
    facts.references.iter().map(|reference| reference.path.clone()).collect::<Vec<_>>(),
    [strings(&["loop"]), strings(&["crate", "type", "run"])]
  );
}

type ImplSummary = (Vec<String>, Option<Vec<String>>, Vec<String>, Vec<String>, Option<LocalSymbolId>, Vec<Vec<String>>);

#[test]
fn should_emit_one_impl_fact_per_impl_block() {
  let source = "#[cfg(unix)]\nimpl<T: Send> Marker for Wrapper<T> {}\nmod inner {\n  impl Display for Money {}\n}\nfn local() {\n  #[allow(dead_code)]\n  impl Helper for Local { fn go(&self) { run(); } }\n}\n";
  let facts = analyze(source);
  let local = item(&facts, &["local"]).local;
  let inner = symbol(&facts, &["inner"], &[]).local;
  let impls: Vec<ImplSummary> = facts
    .impls
    .iter()
    .map(|implementation| {
      (
        implementation.self_type.segments.clone(),
        implementation.trait_ref.as_ref().map(|trait_ref| trait_ref.segments.clone()),
        implementation.generic_params.clone(),
        implementation.module_suffix.clone(),
        implementation.scope,
        implementation.attributes.iter().map(|attribute| attribute.path.clone()).collect(),
      )
    })
    .collect();
  assert_eq!(
    impls,
    [
      (strings(&["Wrapper"]), Some(strings(&["Marker"])), strings(&["T"]), vec![], None, vec![strings(&["cfg"])]),
      (strings(&["Money"]), Some(strings(&["Display"])), vec![], strings(&["inner"]), Some(inner), vec![]),
      (strings(&["Local"]), Some(strings(&["Helper"])), vec![], vec![], Some(local), vec![strings(&["allow"])]),
    ]
  );
  assert_eq!(facts.impls[0].span, Span::of_text(source, 0, source.find("{}").unwrap() + 2).unwrap());
  assert_eq!(facts.impls[0].self_type.generic_args.as_deref(), Some("T"));
  assert_eq!(symbol_names(&facts), ["inner#module", "local#function"]);
}

#[test]
fn should_not_link_blanket_impls_to_types_named_like_the_generic_parameter() {
  let facts = analyze("struct T;\nimpl<T: Clone> Blanket for T {}\nimpl Concrete for T {}\nfn scoped() { impl Nested for T {} }\n");
  assert_eq!(item(&facts, &["T"]).implements, vec![strings(&["Concrete"])]);
}
