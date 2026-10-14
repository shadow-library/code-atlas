mod common;

use atlas_core::{ArgSummary, FileFacts, ParseStatus, ReferenceFact, Span, TemplatePart};
use common::{analyze, refs};

fn reference<'f>(facts: &'f FileFacts, head: &str) -> &'f ReferenceFact {
  facts.references.iter().find(|reference| reference.path.first().is_some_and(|first| first == head)).unwrap()
}

fn symbol_span(facts: &FileFacts, name: &str) -> Span {
  facts.symbols.iter().find(|symbol| symbol.name == name).unwrap().name_span
}

fn signatures(facts: &FileFacts) -> Vec<&str> {
  facts.symbols.iter().filter_map(|symbol| symbol.signature.as_deref()).collect()
}

fn error_count(facts: &FileFacts) -> u32 {
  match &facts.parse_status {
    ParseStatus::Partial { error_count, .. } => *error_count,
    _ => 0,
  }
}

fn template(text: &str) -> Vec<ArgSummary> {
  vec![ArgSummary::Template(vec![TemplatePart::Lit(text.to_owned())])]
}

#[test]
fn should_parse_type_arguments_on_an_identifier_tag_without_keeping_them_in_facts() {
  let source = "export const rows = sql<Row[]>`select 1`;\n";

  let facts = analyze("src/rows.ts", source);

  assert_eq!(facts.parse_status, ParseStatus::Ok);
  assert_eq!(refs(&facts), ["rows -Call-> sql"]);
  assert_eq!(reference(&facts, "sql").args, template("select 1"));
  assert_eq!(signatures(&facts), ["const rows = sql `select 1`"]);
}

#[test]
fn should_parse_type_arguments_on_a_call_tag_and_record_the_tag_call() {
  let source = "export const rows = db()<Row[]>`select 1`;\nexport const plain = this.pool.tx()`select 2`;\n";

  let facts = analyze("src/rows.ts", source);

  assert_eq!(facts.parse_status, ParseStatus::Ok);
  assert_eq!(
    refs(&facts),
    [
      "rows -Call-> (db())",
      "rows -Call-> db",
      "plain -Call-> (this.pool.tx())",
      "plain -MethodCall-> this.pool.tx"
    ]
  );
  let tag = reference(&facts, "(db())");
  assert_eq!(tag.receiver, None);
  assert_eq!(tag.args, template("select 1"));
  assert_eq!((tag.span.start_byte, tag.span.end_byte), (20, 41));
  assert_eq!(reference(&facts, "(this.pool.tx())").args, template("select 2"));
}

#[test]
fn should_read_type_arguments_that_parse_as_comparisons_as_tagged_calls() {
  let source = "export const count = sql<number>`count(*)`;\nexport const one = db.select<Row>`select 1`;\nexport const kind = sql<'a' | \"b\">`select 2`;\nexport const last = db()<X>`select 3`;\n";

  let facts = analyze("src/simple.ts", source);

  assert_eq!(facts.parse_status, ParseStatus::Ok);
  assert_eq!(
    refs(&facts),
    [
      "count -Call-> sql",
      "one -MethodCall-> db.select",
      "kind -Call-> sql",
      "last -Call-> (db())",
      "last -Call-> db"
    ]
  );
  assert_eq!(reference(&facts, "db").args, template("select 1"));
  assert_eq!(
    signatures(&facts),
    [
      "const count = sql `count(*)`",
      "const one = db.select `select 1`",
      "const kind = sql `select 2`",
      "const last = db() `select 3`"
    ]
  );
}

#[test]
fn should_read_spaced_angles_before_a_template_as_type_arguments_like_typescript() {
  let source = "const x = a < b > `c`;\nconst y = f<T>(x);\nconst z = a < b > c;\n";

  let facts = analyze("src/spaced.ts", source);

  assert_eq!(facts.parse_status, ParseStatus::Ok);
  assert_eq!(
    refs(&facts),
    [
      "x -Call-> a",
      "y -Call-> f",
      "y -ValueRef-> x",
      "y -TypeRef-> T",
      "z -ValueRef-> a",
      "z -ValueRef-> b",
      "z -ValueRef-> c"
    ]
  );
}

#[test]
fn should_give_a_tag_the_same_facts_whether_or_not_the_file_has_other_errors() {
  let clean = analyze("src/tag.ts", "const v = a<B>`c`;\n");
  let broken = analyze("src/tag.ts", "const v = a<B>`c`;\nconst broken = ;\n");

  assert_eq!(error_count(&broken), 1);
  assert_eq!(refs(&clean), ["v -Call-> a"]);
  assert_eq!(refs(&broken), ["v -Call-> a"]);
}

#[test]
fn should_parse_nested_tags_and_tags_split_by_comments() {
  let source = "const q = `outer ${sql<Row>`inner ${db()<X>`deep ${id}`}`} end`;\nconst cmt = sql<Row> /* c */ `select 1`;\nconst lc = sql<Row> // c\n  `select 2`;\nexport function after() { return 1; }\n";

  let facts = analyze("src/nested.ts", source);

  assert_eq!(facts.parse_status, ParseStatus::Ok);
  assert_eq!(
    refs(&facts),
    ["q -Call-> sql", "q -Call-> (db())", "q -Call-> db", "q -ValueRef-> id", "cmt -Call-> sql", "lc -Call-> sql"]
  );
  let after = symbol_span(&facts, "after");
  assert_eq!((after.start_byte as usize, after.start_line, after.start_col), (source.find("after").unwrap(), 5, 16));
}

#[test]
fn should_keep_exact_offsets_after_a_masked_multiline_type_argument() {
  let source = "export const rows = sql<{\n  id: string;\n}>`select id`, later = 1;\nexport function after() {}\n";

  let facts = analyze("src/rows.tsx", source);

  assert_eq!(facts.parse_status, ParseStatus::Ok);
  let later = symbol_span(&facts, "later");
  let after = symbol_span(&facts, "after");
  assert_eq!((later.start_byte as usize, later.start_line, later.start_col), (source.find("later").unwrap(), 3, 15));
  assert_eq!((after.start_byte as usize, after.start_line, after.start_col), (source.find("after").unwrap(), 4, 16));
}

#[test]
fn should_keep_the_unmasked_tree_when_a_backtick_in_jsx_text_shifts_template_tracking() {
  let source = "import { sql } from \"./db\";\nconst rows = sql<Row[]>`select 1`;\nexport function Doc() {\n  return <p>Press the ` key</p>;\n}\nexport const label = `List<string>`;\nexport function after() { return helper(`Array<number>`, label); }\n";

  let facts = analyze("src/doc.tsx", source);

  assert_eq!(error_count(&facts), 1);
  assert_eq!(
    signatures(&facts),
    ["const rows = sql<Row[]>`select 1`", "function Doc()", "const label = `List<string>`", "function after()"]
  );
  assert_eq!(reference(&facts, "helper").args[0], template("Array<number>")[0]);
}

#[test]
fn should_keep_the_unmasked_tree_when_a_regex_after_a_paren_shifts_template_tracking() {
  let source = "import { sql } from \"./db\";\nconst rows = sql<Row[]>`select 1`;\nif (ok) /`/.test(s);\nexport const label = `Map<string, Item>`;\nexport function after() { return go(label); }\n";

  let facts = analyze("src/regex.ts", source);

  assert_eq!(error_count(&facts), 1);
  assert_eq!(
    signatures(&facts),
    ["const rows = sql<Row[]>`select 1`", "const label = `Map<string, Item>`", "function after()"]
  );
}

#[test]
fn should_keep_the_unmasked_tree_when_a_type_annotation_ends_a_line_before_a_template() {
  let source = "import { sql } from \"./db\"\nlet x: Array<Item>\n`hello`.split(\"\")\nconst rows = sql<Row[]>`select 1`\nexport function after() { return x }\n";

  let facts = analyze("src/asi.ts", source);

  assert_eq!(error_count(&facts), 2);
  assert_eq!(signatures(&facts), ["let x: Array<Item>", "const rows = sql<Row[]>`select 1`", "function after()"]);
  assert_eq!(refs(&facts)[..2], ["x -TypeRef-> Array", "x -TypeRef-> Item"]);
}

#[test]
fn should_keep_the_unmasked_tree_when_masking_grows_the_bytes_under_errors() {
  let source = "const broken = ;\nimport { Global, Module } from '@nestjs/common';\nimport { AuthModule } from './auth/auth.module';\n\n@Global()\n@Module({\n  imports: [\n    ConfigModule.forRoot({ isGlobal: true }),\n    EventModule.forRoot(),\n    StoreModule.forRoot(storeConfig()),\n    AuthModule,\n  ],\nconst t = db()<{ id: string }[]>`select ${x} 1`;\n  providers: [Clock],\n})\nexport class AppModule {}\n";

  let facts = analyze("src/app.module.ts", source);

  assert_eq!(error_count(&facts), 6);
  assert_eq!(common::symbol_names(&facts), ["broken#constant", "AppModule#class"]);
  assert_eq!(refs(&facts).last().map(String::as_str), Some("AppModule -ValueRef-> string"));
}

#[test]
fn should_keep_comparisons_whose_angles_hold_a_call_or_arithmetic() {
  let source = "const t = sql<number>`count(*)`;\nconst a1 = a < b() > `c`;\nconst a2 = a < b + 1 > `c`;\n";

  let facts = analyze("src/compare.ts", source);

  assert_eq!(facts.parse_status, ParseStatus::Ok);
  assert_eq!(refs(&facts), ["t -Call-> sql", "a1 -ValueRef-> a", "a1 -Call-> b", "a2 -ValueRef-> a", "a2 -ValueRef-> b"]);
}

#[test]
fn should_keep_the_unmasked_tree_when_masking_moves_an_error_outside_the_original_errors() {
  let source = "import { Global, Module } from '@nestjs/common';\n\n@Global()\n@Module({\nconst a = sql<Row[]>`select 0`;\n  imports: [TokenModule.register({ secret: process.env.SECRET })],\n  providers: [TokenGuard, KeyGuard],\nconst b = db()<{ id: string }[]>`select ${x} 1`;\n  exports: [TokenGuard, KeyGuard],\n})\nexport class GuardModule {}\n";

  let facts = analyze("src/guard.module.ts", source);

  assert_eq!(error_count(&facts), 6);
  assert_eq!(common::symbol_names(&facts), ["GuardModule#class"]);
  assert_eq!(refs(&facts)[..2], ["GuardModule -Decorator-> Global", "GuardModule -Decorator-> Module"]);
}

#[test]
fn should_keep_the_unmasked_tree_when_a_satisfies_type_ends_a_line_before_a_template() {
  let source = "const t = sql<number>`count(*)`;\nconst w = x satisfies Foo<Bar>\n`y`.length\n";

  let facts = analyze("src/satisfies.ts", source);

  assert_eq!(error_count(&facts), 1);
  assert_eq!(
    refs(&facts),
    ["t -ValueRef-> sql", "t -ValueRef-> number", "w -ValueRef-> x", "w -TypeRef-> Foo", "w -TypeRef-> Bar"]
  );
}

#[test]
fn should_never_mask_javascript() {
  let source = "const v = sql<T>`c`;\nconst broken = ;\n";

  let facts = analyze("src/plain.js", source);

  assert_eq!(refs(&facts), ["v -ValueRef-> sql", "v -ValueRef-> T"]);
}
