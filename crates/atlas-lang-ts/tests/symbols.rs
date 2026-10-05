mod common;

use atlas_core::{AnalysisError, AnalysisInput, ArgSummary, ContentHash, EntityKind, FileRef, KeyContext, LanguageAnalyzer, Modifier, Modifiers, RelPath, RepoName, Visibility};
use atlas_lang_ts::{MAX_SOURCE_BYTES, TsAnalyzer};
use common::{analyze, symbol_names};

fn symbol<'a>(facts: &'a atlas_core::FileFacts, path: &str) -> &'a atlas_core::SymbolFact {
  facts
    .symbols
    .iter()
    .find(|symbol| symbol.item_path.join(".") == path)
    .unwrap_or_else(|| panic!("no symbol {path} in {:?}", symbol_names(facts)))
}

type Attributes = Vec<(Vec<String>, Vec<ArgSummary>)>;

fn modifiers(list: &[Modifier]) -> Modifiers {
  list.iter().copied().collect()
}

fn file(path: &str) -> FileRef {
  FileRef::detect(RepoName::new("fixture").unwrap(), RelPath::new(path).unwrap(), ContentHash::of(b"")).unwrap()
}

#[test]
fn should_parse_each_extension_with_its_grammar() {
  let typescript = analyze("src/a.ts", "export function f<T>(value: T): T { return value; }");
  assert_eq!(typescript.parse_status, atlas_core::ParseStatus::Ok);
  let tsx = analyze("src/a.tsx", "export const view = <Card<string> title=\"x\" />;");
  assert_eq!(tsx.parse_status, atlas_core::ParseStatus::Ok);
  let jsx_in_js = analyze("src/a.js", "export const view = <Card title=\"x\" />;");
  assert_eq!(jsx_in_js.parse_status, atlas_core::ParseStatus::Ok);
  for path in ["src/a.mjs", "src/a.cjs", "src/a.jsx", "src/a.mts", "src/a.cts"] {
    assert_eq!(analyze(path, "export function f() {}").parse_status, atlas_core::ParseStatus::Ok, "{path}");
  }
  let generic_arrow_in_ts = analyze("src/a.ts", "const id = <T,>(value: T) => value;");
  assert_eq!(symbol_names(&generic_arrow_in_ts), ["id#function"]);
}

#[test]
fn should_reject_unsupported_languages_and_oversized_sources() {
  let rust = file("src/lib.rs");
  assert_eq!(
    TsAnalyzer::new().analyze(&AnalysisInput {
      file: &rust,
      source: "fn main() {}"
    }),
    Err(AnalysisError::UnsupportedLanguage(atlas_core::Language::Rust))
  );
  let big = "a".repeat(MAX_SOURCE_BYTES + 1);
  let ts = file("src/big.ts");
  assert_eq!(
    TsAnalyzer::new().analyze(&AnalysisInput { file: &ts, source: &big }),
    Err(AnalysisError::SourceTooLarge {
      bytes: MAX_SOURCE_BYTES + 1,
      limit: MAX_SOURCE_BYTES
    })
  );
  assert!(TsAnalyzer::new().supports(atlas_core::Language::Jsx));
  assert!(!TsAnalyzer::new().supports(atlas_core::Language::Rust));
}

#[test]
fn should_extract_declarations_without_bodies_from_declaration_files() {
  let source = r#"
export declare function connect(url: string): Client;
export declare class Client {
  query<T>(sql: string): Promise<T[]>;
  readonly url: string;
}
declare module 'express' {
  interface Request { user?: User }
}
declare global {
  interface Window { atlas: Client }
}
export as namespace Atlas;
"#;
  let facts = analyze("types/index.d.ts", source);
  assert!(facts.file.is_declaration_file);
  assert_eq!(
    symbol_names(&facts),
    [
      "connect#function",
      "Client#class",
      "Client.query#method",
      "Client.url#field",
      "express#namespace",
      "express.Request#interface",
      "express.Request.user#field",
      "global#namespace",
      "global.Window#interface",
      "global.Window.atlas#field",
    ]
  );
  assert!(facts.symbols.iter().all(|symbol| symbol.body.is_none()));
  assert_eq!(symbol(&facts, "connect").modifiers, modifiers(&[Modifier::Declare]));
  assert_eq!(symbol(&facts, "connect").signature.as_deref(), Some("function connect(url: string): Client"));
  assert_eq!(symbol(&facts, "Client").modifiers, modifiers(&[Modifier::Declare]));
  assert_eq!(symbol(&facts, "Client").visibility, Visibility::Exported);
  assert_eq!(symbol(&facts, "express").modifiers, modifiers(&[Modifier::Declare]));
  assert_eq!(symbol(&facts, "express.Request.user").modifiers, modifiers(&[Modifier::Optional]));
}

#[test]
fn should_extract_functions_generators_and_function_valued_variables() {
  let source = r#"
export function plain(a: number): number { return a; }
async function fetchAll() {}
function* ids() { yield 1; }
export async function* stream() {}
export const arrow = async (x: string) => x;
let assigned = function named() {};
var legacy = function* () {};
const notAFunction = compute();
"#;
  let facts = analyze("src/functions.ts", source);
  assert_eq!(
    symbol_names(&facts),
    [
      "plain#function",
      "fetchAll#function",
      "ids#function",
      "stream#function",
      "arrow#function",
      "assigned#function",
      "legacy#function",
      "notAFunction#constant",
    ]
  );
  assert_eq!(symbol(&facts, "plain").visibility, Visibility::Exported);
  assert_eq!(symbol(&facts, "fetchAll").visibility, Visibility::Module);
  assert_eq!(symbol(&facts, "fetchAll").modifiers, modifiers(&[Modifier::Async]));
  assert_eq!(symbol(&facts, "ids").modifiers, modifiers(&[Modifier::Generator]));
  assert_eq!(symbol(&facts, "stream").modifiers, modifiers(&[Modifier::Async, Modifier::Generator]));
  assert_eq!(symbol(&facts, "arrow").modifiers, modifiers(&[Modifier::Async]));
  assert_eq!(symbol(&facts, "arrow").signature.as_deref(), Some("const arrow = async (x: string) =>"));
  assert_eq!(symbol(&facts, "arrow").params[0].name, "x");
  assert_eq!(symbol(&facts, "legacy").modifiers, modifiers(&[Modifier::Generator]));
  assert_eq!(symbol(&facts, "notAFunction").signature.as_deref(), Some("const notAFunction = compute()"));
  assert!(symbol(&facts, "plain").body.is_some());
  assert!(symbol(&facts, "notAFunction").body.is_none());
}

#[test]
fn should_extract_class_members_with_visibility_and_modifiers() {
  let source = r#"
export abstract class Repo<T> extends Base<T> implements Store<T>, ns.Disposable {
  static instances = 0;
  #secret = 1;
  protected readonly name: string;
  declare tag?: string;
  override handler = async (event: Event) => event;
  constructor(private readonly db: Db, public label?: string, plain: number) { super(); }
  get size(): number { return 0; }
  set size(value: number) {}
  static async *scan() {}
  #hidden() {}
  protected abstract find(id: string): T;
  abstract get count(): number;
  optional?(): void;
}
"#;
  let facts = analyze("src/repo.ts", source);
  assert_eq!(
    symbol_names(&facts),
    [
      "Repo#class",
      "Repo.instances#field",
      "Repo.#secret#field",
      "Repo.name#field",
      "Repo.tag#field",
      "Repo.handler#method",
      "Repo.constructor#method",
      "Repo.db#field",
      "Repo.label#field",
      "Repo.size#method",
      "Repo.size#method",
      "Repo.scan#method",
      "Repo.#hidden#method",
      "Repo.find#method",
      "Repo.count#method",
      "Repo.optional#method",
    ]
  );
  let class = symbol(&facts, "Repo");
  assert_eq!(class.modifiers, modifiers(&[Modifier::Abstract]));
  assert_eq!(class.extends, [vec!["Base".to_owned()]]);
  assert_eq!(class.implements, [vec!["Store".to_owned()], vec!["ns".to_owned(), "Disposable".to_owned()]]);
  assert_eq!(
    class.signature.as_deref(),
    Some("abstract class Repo<T> extends Base<T> implements Store<T>, ns.Disposable")
  );
  let by_index = |index: usize| &facts.symbols[index];
  assert_eq!(
    (by_index(1).visibility.clone(), by_index(1).modifiers),
    (Visibility::Public, modifiers(&[Modifier::Static]))
  );
  assert_eq!(by_index(2).visibility, Visibility::Private);
  assert_eq!(
    (by_index(3).visibility.clone(), by_index(3).modifiers),
    (Visibility::Protected, modifiers(&[Modifier::Readonly]))
  );
  assert_eq!(by_index(4).modifiers, modifiers(&[Modifier::Declare, Modifier::Optional]));
  assert_eq!(by_index(5).modifiers, modifiers(&[Modifier::Async, Modifier::Override]));
  assert_eq!(by_index(5).params[0].type_text.as_deref(), Some("Event"));
  assert_eq!(by_index(6).params.iter().map(|param| param.name.as_str()).collect::<Vec<_>>(), ["db", "label", "plain"]);
  assert_eq!(
    (by_index(7).visibility.clone(), by_index(7).modifiers),
    (Visibility::Private, modifiers(&[Modifier::Readonly]))
  );
  assert_eq!(by_index(7).signature.as_deref(), Some("private readonly db: Db"));
  assert_eq!(
    (by_index(8).visibility.clone(), by_index(8).modifiers),
    (Visibility::Public, modifiers(&[Modifier::Optional]))
  );
  assert_eq!(by_index(9).modifiers, modifiers(&[Modifier::Getter]));
  assert_eq!(by_index(10).modifiers, modifiers(&[Modifier::Setter]));
  assert_eq!(by_index(11).modifiers, modifiers(&[Modifier::Async, Modifier::Static, Modifier::Generator]));
  assert_eq!(by_index(12).visibility, Visibility::Private);
  assert_eq!(
    (by_index(13).visibility.clone(), by_index(13).modifiers),
    (Visibility::Protected, modifiers(&[Modifier::Abstract]))
  );
  assert_eq!(by_index(14).modifiers, modifiers(&[Modifier::Abstract, Modifier::Getter]));
  assert_eq!(by_index(15).modifiers, modifiers(&[Modifier::Optional]));
  assert!(facts.symbols[1..].iter().all(|member| member.parent == Some(atlas_core::LocalSymbolId(0))));
}

#[test]
fn should_extract_javascript_class_fields_and_decorated_members() {
  let source = "@sealed\nexport class Widget extends Base {\n  static count = 0;\n  #state = () => 1;\n  @bound handle() {}\n}\n";
  let facts = analyze("src/widget.js", source);
  assert_eq!(symbol_names(&facts), ["Widget#class", "Widget.count#field", "Widget.#state#method", "Widget.handle#method"]);
  assert_eq!(symbol(&facts, "Widget").attributes[0].path, ["sealed"]);
  assert_eq!(symbol(&facts, "Widget").extends, [vec!["Base".to_owned()]]);
  assert_eq!(symbol(&facts, "Widget.handle").attributes[0].path, ["bound"]);
  assert_eq!(symbol(&facts, "Widget.handle").signature.as_deref(), Some("handle()"));
}

#[test]
fn should_extract_interfaces_with_member_kinds_and_extends() {
  let source =
    "export interface Repo<T> extends Base<T>, ns.Named {\n  readonly id: string;\n  label?: string;\n  find(id: string): T;\n  [key: string]: unknown;\n  (): void;\n}\n";
  let facts = analyze("src/repo.ts", source);
  assert_eq!(symbol_names(&facts), ["Repo#interface", "Repo.id#field", "Repo.label#field", "Repo.find#method"]);
  let repo = symbol(&facts, "Repo");
  assert_eq!(repo.extends, [vec!["Base".to_owned()], vec!["ns".to_owned(), "Named".to_owned()]]);
  assert_eq!(repo.signature.as_deref(), Some("interface Repo<T> extends Base<T>, ns.Named"));
  assert_eq!(symbol(&facts, "Repo.id").modifiers, modifiers(&[Modifier::Readonly]));
  assert_eq!(symbol(&facts, "Repo.label").modifiers, modifiers(&[Modifier::Optional]));
  assert_eq!(symbol(&facts, "Repo.find").signature.as_deref(), Some("find(id: string): T"));
  assert_eq!(symbol(&facts, "Repo.find").visibility, Visibility::Public);
}

#[test]
fn should_extract_type_aliases_enums_and_namespaces() {
  let source = r#"
export type Id<T> = string & { brand: T };
export const enum Color { Red = 'red', Green }
enum Plain { 'quoted-name' = 1 }
export namespace Outer.Inner {
  export function helper() {}
  const hidden = 1;
}
namespace Solo { export interface Shape {} }
"#;
  let facts = analyze("src/types.ts", source);
  assert_eq!(
    symbol_names(&facts),
    [
      "Id#type_alias",
      "Color#enum",
      "Color.Red#variant",
      "Color.Green#variant",
      "Plain#enum",
      "Plain.quoted-name#variant",
      "Outer.Inner#namespace",
      "Outer.Inner.helper#function",
      "Outer.Inner.hidden#constant",
      "Solo#namespace",
      "Solo.Shape#interface",
    ]
  );
  assert_eq!(symbol(&facts, "Id").signature.as_deref(), Some("type Id<T> = string & { brand: T }"));
  assert_eq!(symbol(&facts, "Color").modifiers, modifiers(&[Modifier::Const]));
  assert_eq!(symbol(&facts, "Color.Red").signature.as_deref(), Some("Red = 'red'"));
  assert_eq!(symbol(&facts, "Outer.Inner").name, "Inner");
  assert_eq!(symbol(&facts, "Outer.Inner").visibility, Visibility::Exported);
  assert_eq!(symbol(&facts, "Outer.Inner.helper").visibility, Visibility::Exported);
  assert_eq!(symbol(&facts, "Outer.Inner.hidden").visibility, Visibility::Module);
  assert_eq!(symbol(&facts, "Outer.Inner.helper").parent, Some(atlas_core::LocalSymbolId(6)));
}

#[test]
fn should_extract_module_level_variables_including_destructuring() {
  let source = "export const A = 1, B = 'b';\nlet counter = 0;\nvar legacy;\nexport const { left, right: renamed, ...rest } = pair;\nconst [first] = list;\n";
  let facts = analyze("src/values.ts", source);
  assert_eq!(
    symbol_names(&facts),
    [
      "A#constant",
      "B#constant",
      "counter#variable",
      "legacy#variable",
      "left#constant",
      "renamed#constant",
      "rest#constant",
      "first#constant",
    ]
  );
  assert_eq!(symbol(&facts, "B").signature.as_deref(), Some("const B = 'b'"));
  assert_eq!(symbol(&facts, "legacy").signature.as_deref(), Some("var legacy"));
  assert_eq!(symbol(&facts, "renamed").visibility, Visibility::Exported);
  assert_eq!(symbol(&facts, "first").visibility, Visibility::Module);
}

#[test]
fn should_name_anonymous_default_exports_default_and_record_identifier_defaults() {
  let class = analyze("src/a.ts", "export default class { run() {} }");
  assert_eq!(symbol_names(&class), ["default#class", "default.run#method"]);
  assert_eq!(class.symbols[0].visibility, Visibility::ExportedDefault);
  let function = analyze("src/b.ts", "export default async function () {}");
  assert_eq!(symbol_names(&function), ["default#function"]);
  assert_eq!(function.symbols[0].modifiers, modifiers(&[Modifier::Async]));
  let arrow = analyze("src/c.ts", "export default (x: number) => x;");
  assert_eq!(symbol_names(&arrow), ["default#function"]);
  let named = analyze("src/d.ts", "export default function alpha() {}");
  assert_eq!(symbol_names(&named), ["alpha#function"]);
  assert_eq!(named.symbols[0].visibility, Visibility::ExportedDefault);
  let identifier = analyze("src/e.ts", "function handler() {}\nexport default handler;\n");
  assert_eq!(identifier.symbols[0].visibility, Visibility::ExportedDefault);
  assert_eq!(
    identifier.exports,
    [atlas_core::ExportFact::Default {
      target: atlas_core::DefaultExportTarget::Identifier("handler".into()),
      span: identifier.exports[0].span(),
    }]
  );
  let expression = analyze("src/f.ts", "export default { port: 3000 };");
  assert!(expression.symbols.is_empty());
  assert!(matches!(
    &expression.exports[0],
    atlas_core::ExportFact::Default { target: atlas_core::DefaultExportTarget::Expression(ArgSummary::Object(entries)), .. } if entries.len() == 1
  ));
}

#[test]
fn should_upgrade_visibility_of_symbols_exported_through_export_lists() {
  let facts = analyze(
    "src/a.ts",
    "function a() {}\nclass B {}\nconst c = 1;\nfunction d() {}\nexport { a, B as Bee, c as default };\n",
  );
  let visibilities: Vec<Visibility> = facts.symbols.iter().map(|symbol| symbol.visibility.clone()).collect();
  assert_eq!(visibilities, [Visibility::Exported, Visibility::Exported, Visibility::ExportedDefault, Visibility::Module]);
}

#[test]
fn should_emit_one_symbol_per_overload_and_disambiguate_keys_in_source_order() {
  let source = "export function parse(input: string): A;\nexport function parse(input: Buffer): A;\nexport function parse(input: unknown): A { return x; }\nclass P { m(a: string): void; m(a: any) {} }\n";
  let facts = analyze("src/parse.ts", source);
  assert_eq!(
    symbol_names(&facts),
    ["parse#function", "parse#function", "parse#function", "P#class", "P.m#method", "P.m#method"]
  );
  assert_eq!(facts.symbols[1].signature.as_deref(), Some("function parse(input: Buffer): A"));
  assert!(facts.symbols[0].body.is_none() && facts.symbols[2].body.is_some());
  let context = KeyContext {
    package: "pkg".into(),
    module_path: vec!["src".into(), "parse".into()],
  };
  let keys: Vec<String> = facts.symbol_keys(&context).unwrap().iter().map(ToString::to_string).collect();
  assert_eq!(
    keys,
    [
      "ts:fixture:pkg:src/parse::parse#function",
      "ts:fixture:pkg:src/parse::parse#function~1",
      "ts:fixture:pkg:src/parse::parse#function~2",
      "ts:fixture:pkg:src/parse::P#class",
      "ts:fixture:pkg:src/parse::P::m#method",
      "ts:fixture:pkg:src/parse::P::m#method~1",
    ]
  );
}

#[test]
fn should_record_docs_signatures_and_name_spans() {
  let source = "/**\n * Finds a user.\n *\n * @param id the user id\n */\nexport async function findUser(\n  id: string,\n  options?: Options,\n): Promise<User> {\n  return load(id);\n}\n\n/** stray */\n\nconst unrelated = 1;\n// line comment\nfunction noDoc() {}\n";
  let facts = analyze("src/users.ts", source);
  let find = symbol(&facts, "findUser");
  assert_eq!(find.doc.as_deref(), Some("Finds a user.\n\n@param id the user id"));
  assert_eq!(find.signature.as_deref(), Some("async function findUser(id: string, options?: Options): Promise<User>"));
  assert_eq!(find.name_span.text(source), Some("findUser"));
  assert_eq!(find.span.text(source).map(|text| text.starts_with("export async function")), Some(true));
  assert_eq!((find.span.start_line, find.span.end_line), (6, 11));
  assert_eq!(find.params[1].name, "options");
  assert_eq!(find.params[1].type_text.as_deref(), Some("Options"));
  assert_eq!(find.body.as_ref().map(|body| body.line_count), Some(3));
  assert_eq!(symbol(&facts, "unrelated").doc, None);
  assert_eq!(symbol(&facts, "noDoc").doc, None);
}

#[test]
fn should_record_decorators_on_classes_methods_properties_and_parameters() {
  let source = r#"
@Controller('users')
@UseGuards(JwtAuthGuard)
export class UsersController {
  @Inject(CACHE) @nest.Optional() private cache: Cache;

  constructor(@InjectRepository(User) private readonly users: Repository<User>) {}

  @Get(':id')
  @UseGuards(JwtAuthGuard)
  findOne(@Param('id') id: string, @Headers('x-api-key') key: string, @Req() request: Request) {
    return id;
  }
}
"#;
  let facts = analyze("src/users.controller.ts", source);
  let attributes = |path: &str| -> Attributes {
    symbol(&facts, path)
      .attributes
      .iter()
      .map(|attribute| (attribute.path.clone(), attribute.args.clone()))
      .collect()
  };
  let path = |segments: &[&str]| -> Vec<String> { segments.iter().map(|segment| (*segment).to_owned()).collect() };
  assert_eq!(
    attributes("UsersController"),
    [
      (path(&["Controller"]), vec![ArgSummary::Str("users".into())]),
      (path(&["UseGuards"]), vec![ArgSummary::Path(path(&["JwtAuthGuard"]))]),
    ]
  );
  assert_eq!(
    attributes("UsersController.cache"),
    [(path(&["Inject"]), vec![ArgSummary::Path(path(&["CACHE"]))]), (path(&["nest", "Optional"]), vec![])]
  );
  assert_eq!(
    attributes("UsersController.findOne"),
    [
      (path(&["Get"]), vec![ArgSummary::Str(":id".into())]),
      (path(&["UseGuards"]), vec![ArgSummary::Path(path(&["JwtAuthGuard"]))]),
    ]
  );
  let find = symbol(&facts, "UsersController.findOne");
  let params: Vec<(String, Option<String>, Attributes)> = find
    .params
    .iter()
    .map(|param| {
      (
        param.name.clone(),
        param.type_text.clone(),
        param.attributes.iter().map(|attribute| (attribute.path.clone(), attribute.args.clone())).collect(),
      )
    })
    .collect();
  assert_eq!(
    params,
    [
      ("id".into(), Some("string".into()), vec![(path(&["Param"]), vec![ArgSummary::Str("id".into())])]),
      ("key".into(), Some("string".into()), vec![(path(&["Headers"]), vec![ArgSummary::Str("x-api-key".into())])]),
      ("request".into(), Some("Request".into()), vec![(path(&["Req"]), vec![])]),
    ]
  );
  assert_eq!(find.signature.as_deref(), Some("findOne(id: string, key: string, request: Request)"));
  let constructor = symbol(&facts, "UsersController.constructor");
  assert_eq!(constructor.params[0].attributes[0].path, ["InjectRepository"]);
  assert_eq!(constructor.params[0].attributes[0].args, [ArgSummary::Path(path(&["User"]))]);
  assert_eq!(symbol(&facts, "UsersController.users").kind, EntityKind::Field);
  let span = &symbol(&facts, "UsersController.findOne").span;
  assert_eq!(span.text(source).map(|text| text.starts_with("@Get(':id')")), Some(true));
}

#[test]
fn should_record_test_symbols_only_in_test_contexts() {
  let source = "describe('suite', () => { it('case', () => {}); });\n";
  assert_eq!(symbol_names(&analyze("src/math.ts", source)), Vec::<String>::new());
  assert_eq!(symbol_names(&analyze("src/math.spec.ts", source)), ["suite#test_suite", "suite.case#test"]);
  assert_eq!(symbol_names(&analyze("src/__tests__/math.ts", source)), ["suite#test_suite", "suite.case#test"]);
  let imported = format!("import {{ describe, it }} from 'vitest';\n{source}");
  assert_eq!(symbol_names(&analyze("src/math.ts", &imported)), ["suite#test_suite", "suite.case#test"]);
}
