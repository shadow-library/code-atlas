mod common;

use atlas_edit::{EditError, Nesting, SymbolOp, SymbolSite, SymbolTarget, Syntax, TextEdit, apply, plan_symbol_edit, symbol_edit};
use common::{Workspace, edited, hash, target};

fn site<'a>(content: &'a str, declaration: &str, syntax: Syntax, nesting: Nesting) -> SymbolSite<'a> {
  let start = content.find(declaration).unwrap();
  SymbolSite {
    content,
    span: start..start + declaration.len(),
    floor: 0,
    syntax,
    nesting,
  }
}

fn run(site: &SymbolSite<'_>, op: SymbolOp) -> String {
  let edit = symbol_edit(site, &op).unwrap();
  edited(site.content, edit.start, edit.end, &edit.new_text)
}

const IMPL: &str = "impl Store {\n    /// Reads.\n    #[inline]\n    pub fn get(&self) -> u32 {\n        self.value\n    }\n}\n";
const GET: &str = "pub fn get(&self) -> u32 {\n        self.value\n    }";

#[test]
fn should_reindent_a_flush_left_body_to_the_symbol_indent() {
  let site = site(IMPL, GET, Syntax::Rust, Nesting::Nested);
  assert_eq!(
    run(&site, SymbolOp::ReplaceBody("pub fn get(&self) -> u32 {\n    self.value + 1\n}\n".to_owned())),
    "impl Store {\n    /// Reads.\n    #[inline]\n    pub fn get(&self) -> u32 {\n        self.value + 1\n    }\n}\n"
  );
}

#[test]
fn should_keep_a_body_that_already_carries_the_symbol_indent() {
  let site = site(IMPL, GET, Syntax::Rust, Nesting::Nested);
  assert_eq!(
    run(&site, SymbolOp::ReplaceBody("    pub fn get(&self) -> u32 {\n        0\n    }".to_owned())),
    "impl Store {\n    /// Reads.\n    #[inline]\n    pub fn get(&self) -> u32 {\n        0\n    }\n}\n"
  );
}

#[test]
fn should_replace_leading_docs_when_the_new_body_starts_with_a_doc_comment() {
  let site = site(IMPL, GET, Syntax::Rust, Nesting::Nested);
  assert_eq!(
    run(
      &site,
      SymbolOp::ReplaceBody("/// Reads the value.\npub fn get(&self) -> u32 {\n    self.value\n}".to_owned())
    ),
    "impl Store {\n    /// Reads the value.\n    #[inline]\n    pub fn get(&self) -> u32 {\n        self.value\n    }\n}\n"
  );
}

#[test]
fn should_replace_docs_and_attributes_when_the_new_body_restates_both() {
  let site = site(IMPL, GET, Syntax::Rust, Nesting::Nested);
  assert_eq!(
    run(&site, SymbolOp::ReplaceBody("/// New.\n#[cold]\npub fn get(&self) -> u32 {\n    0\n}".to_owned())),
    "impl Store {\n    /// New.\n    #[cold]\n    pub fn get(&self) -> u32 {\n        0\n    }\n}\n"
  );
}

#[test]
fn should_keep_docs_and_attributes_when_the_new_body_starts_with_a_plain_comment() {
  let content = "/// Docs.\n#[tracing::instrument]\npub fn a() {\n  1\n}\n";
  let site = site(content, "pub fn a() {\n  1\n}", Syntax::Rust, Nesting::TopLevel);
  assert_eq!(
    run(&site, SymbolOp::ReplaceBody("// fast path\npub fn a() {\n  2\n}".to_owned())),
    "/// Docs.\n#[tracing::instrument]\n// fast path\npub fn a() {\n  2\n}\n"
  );
}

#[test]
fn should_refuse_a_new_body_that_is_only_docs() {
  let site = site(IMPL, GET, Syntax::Rust, Nesting::Nested);
  assert!(matches!(symbol_edit(&site, &SymbolOp::ReplaceBody("/// Only docs.".to_owned())), Err(EditError::EmptyText)));
}

#[test]
fn should_replace_only_the_attributes_when_the_new_body_starts_with_attributes() {
  let site = site(IMPL, GET, Syntax::Rust, Nesting::Nested);
  assert_eq!(
    run(&site, SymbolOp::ReplaceBody("#[must_use]\npub fn get(&self) -> u32 {\n    self.value\n}".to_owned())),
    "impl Store {\n    /// Reads.\n    #[must_use]\n    pub fn get(&self) -> u32 {\n        self.value\n    }\n}\n"
  );
}

#[test]
fn should_keep_jsdoc_when_a_ts_method_body_repeats_its_decorators() {
  let content = "class C {\n  /** Finds one. */\n  @Get(':id')\n  find() {\n    return 1;\n  }\n}\n";
  let method = "@Get(':id')\n  find() {\n    return 1;\n  }";
  let site = site(content, method, Syntax::TypeScript, Nesting::Nested);
  assert_eq!(
    run(&site, SymbolOp::ReplaceBody("@Get(':id')\nfind() {\n  return 2;\n}".to_owned())),
    "class C {\n  /** Finds one. */\n  @Get(':id')\n  find() {\n    return 2;\n  }\n}\n"
  );
}

#[test]
fn should_refuse_to_replace_docs_when_code_precedes_the_declaration() {
  let content = "/** Docs. */\nexport class A {}\n";
  let site = site(content, "class A {}", Syntax::TypeScript, Nesting::TopLevel);
  assert!(matches!(
    symbol_edit(&site, &SymbolOp::ReplaceBody("/** New. */\nclass A {}".to_owned())),
    Err(EditError::NotLineAligned(_))
  ));
  assert_eq!(
    run(&site, SymbolOp::ReplaceBody("class A { x = 1 }".to_owned())),
    "/** Docs. */\nexport class A { x = 1 }\n"
  );
}

#[test]
fn should_insert_before_the_leading_region_with_a_blank_line_for_top_level_items() {
  let content = "use a;\n\n/// Docs.\n#[inline]\nfn b() {}\n";
  let site = site(content, "fn b() {}", Syntax::Rust, Nesting::TopLevel);
  assert_eq!(
    run(&site, SymbolOp::InsertBefore("fn a() {}".to_owned())),
    "use a;\n\nfn a() {}\n\n/// Docs.\n#[inline]\nfn b() {}\n"
  );
}

#[test]
fn should_insert_nested_items_without_a_blank_line_when_the_symbol_has_none_above() {
  let content = "impl A {\n    fn b() {}\n    fn c() {}\n}\n";
  let before = site(content, "fn c() {}", Syntax::Rust, Nesting::Nested);
  assert_eq!(
    run(&before, SymbolOp::InsertBefore("fn x() {\n    1\n}".to_owned())),
    "impl A {\n    fn b() {}\n    fn x() {\n        1\n    }\n    fn c() {}\n}\n"
  );
  let after = site(content, "fn b() {}", Syntax::Rust, Nesting::Nested);
  assert_eq!(
    run(&after, SymbolOp::InsertAfter("fn x() {}".to_owned())),
    "impl A {\n    fn b() {}\n    fn x() {}\n    fn c() {}\n}\n"
  );
}

#[test]
fn should_insert_nested_items_with_a_blank_line_when_the_symbol_has_one_above() {
  let content = "impl A {\n    fn a() {}\n\n    fn b() {}\n}\n";
  let site = site(content, "fn b() {}", Syntax::Rust, Nesting::Nested);
  assert_eq!(
    run(&site, SymbolOp::InsertAfter("fn c() {}".to_owned())),
    "impl A {\n    fn a() {}\n\n    fn b() {}\n\n    fn c() {}\n}\n"
  );
  assert_eq!(
    run(&site, SymbolOp::InsertBefore("fn x() {}".to_owned())),
    "impl A {\n    fn a() {}\n\n    fn x() {}\n\n    fn b() {}\n}\n"
  );
}

#[test]
fn should_insert_after_a_top_level_item_with_a_blank_line() {
  let content = "export function a() {}\nexport function c() {}\n";
  let site = site(content, "export function a() {}", Syntax::TypeScript, Nesting::TopLevel);
  assert_eq!(
    run(&site, SymbolOp::InsertAfter("export function b() {}\n".to_owned())),
    "export function a() {}\n\nexport function b() {}\nexport function c() {}\n"
  );
}

#[test]
fn should_delete_a_symbol_with_its_docs_and_attributes_and_one_blank_line() {
  let content = "fn a() {}\n\n/// Docs.\n#[inline]\nfn b() {\n    1\n}\n\nfn c() {}\n";
  let site = site(content, "fn b() {\n    1\n}", Syntax::Rust, Nesting::TopLevel);
  assert_eq!(run(&site, SymbolOp::SafeDelete), "fn a() {}\n\nfn c() {}\n");
}

#[test]
fn should_delete_the_last_symbol_and_its_preceding_blank_line() {
  let content = "fn a() {}\n\nfn b() {}\n";
  assert_eq!(run(&site(content, "fn b() {}", Syntax::Rust, Nesting::TopLevel), SymbolOp::SafeDelete), "fn a() {}\n");
  let first = "fn a() {}\n\nfn b() {}\n";
  assert_eq!(run(&site(first, "fn a() {}", Syntax::Rust, Nesting::TopLevel), SymbolOp::SafeDelete), "fn b() {}\n");
}

#[test]
fn should_delete_a_nested_symbol_and_its_trailing_semicolon_line() {
  let content = "class A {\n  // Count.\n  count = 1;\n  name = 'a';\n}\n";
  let site = site(content, "count = 1", Syntax::TypeScript, Nesting::Nested);
  assert_eq!(run(&site, SymbolOp::SafeDelete), "class A {\n  name = 'a';\n}\n");
}

#[test]
fn should_refuse_deleting_a_declaration_that_shares_its_line() {
  let content = "fn a() {} fn b() {}\n";
  assert!(matches!(
    symbol_edit(&site(content, "fn a() {}", Syntax::Rust, Nesting::TopLevel), &SymbolOp::SafeDelete),
    Err(EditError::NotLineAligned(_))
  ));
  assert!(matches!(
    symbol_edit(&site(content, "fn b() {}", Syntax::Rust, Nesting::TopLevel), &SymbolOp::SafeDelete),
    Err(EditError::NotLineAligned(_))
  ));
}

#[test]
fn should_write_inserted_text_with_crlf_endings_after_a_bom() {
  let content = "\u{feff}/// Docs.\r\nfn a() {\r\n}\r\n";
  let site = site(content, "fn a() {\r\n}", Syntax::Rust, Nesting::TopLevel);
  assert_eq!(
    run(&site, SymbolOp::InsertAfter("fn b() {\n}".to_owned())),
    "\u{feff}/// Docs.\r\nfn a() {\r\n}\r\n\r\nfn b() {\r\n}\r\n"
  );
  assert_eq!(
    run(&site, SymbolOp::InsertBefore("fn z() {}".to_owned())),
    "\u{feff}fn z() {}\r\n\r\n/// Docs.\r\nfn a() {\r\n}\r\n"
  );
  assert_eq!(run(&site, SymbolOp::SafeDelete), "\u{feff}");
}

#[test]
fn should_refuse_invalid_spans_and_empty_text() {
  let content = "fn a() {}\n";
  let bad = SymbolSite {
    content,
    span: 4..40,
    floor: 0,
    syntax: Syntax::Rust,
    nesting: Nesting::TopLevel,
  };
  assert!(matches!(
    symbol_edit(&bad, &SymbolOp::SafeDelete),
    Err(EditError::InvalidSpan { start: 4, end: 40, len: 10 })
  ));
  let good = site(content, "fn a() {}", Syntax::Rust, Nesting::TopLevel);
  assert!(matches!(symbol_edit(&good, &SymbolOp::ReplaceBody("\n  \n".to_owned())), Err(EditError::EmptyText)));
}

#[test]
fn should_plan_a_symbol_edit_against_a_hash_bound_span() {
  let workspace = Workspace::new(&["api"]);
  let content = "fn a() {}\n";
  let expected = workspace.write("api", "src/lib.rs", content);
  let symbol = SymbolTarget {
    file: target("api", "src/lib.rs"),
    expected,
    span: 0..9,
    floor: 0,
    syntax: Syntax::Rust,
    nesting: Nesting::TopLevel,
  };
  let planned = plan_symbol_edit(&workspace.scope(), &symbol, &SymbolOp::ReplaceBody("fn a() -> u8 { 1 }".to_owned())).unwrap();
  assert_eq!(planned.plan.files[0].edits, [TextEdit::replace(0..9, "fn a() -> u8 { 1 }")]);
  assert_eq!(
    planned.diff.to_string(),
    "--- api:src/lib.rs\n+++ api:src/lib.rs\n@@ -1,1 +1,1 @@\n-fn a() {}\n+fn a() -> u8 { 1 }\n"
  );
  apply(&planned.plan, &workspace.scope(), &workspace.options()).unwrap();
  assert_eq!(workspace.read("api", "src/lib.rs"), "fn a() -> u8 { 1 }\n");
  let stale = plan_symbol_edit(
    &workspace.scope(),
    &SymbolTarget {
      expected: hash(content),
      ..symbol
    },
    &SymbolOp::SafeDelete,
  )
  .unwrap_err();
  assert!(matches!(&stale, EditError::Stale { paths } if paths == &["api:src/lib.rs"]), "{stale:?}");
}

#[test]
fn should_treat_a_span_ending_in_a_line_terminator_like_one_ending_before_it() {
  let content = "fn a() {}\r\nfn c() {}\r\n";
  let site = SymbolSite {
    content,
    span: 0..11,
    floor: 0,
    syntax: Syntax::Rust,
    nesting: Nesting::Nested,
  };
  assert_eq!(run(&site, SymbolOp::InsertAfter("fn b() {}".to_owned())), "fn a() {}\r\nfn b() {}\r\nfn c() {}\r\n");
  assert_eq!(run(&site, SymbolOp::SafeDelete), "fn c() {}\r\n");
}

#[test]
fn should_keep_a_closing_template_line_above_the_floor() {
  let content = "const t = `\n// end`;\nexport function b() {}\n";
  let start = content.find("export").unwrap();
  let site = SymbolSite {
    content,
    span: start..start + "export function b() {}".len(),
    floor: content.find(";\n").unwrap() + 1,
    syntax: Syntax::TypeScript,
    nesting: Nesting::TopLevel,
  };
  assert_eq!(
    run(&site, SymbolOp::ReplaceBody("/** Docs. */\nexport function b() { return 1; }".to_owned())),
    "const t = `\n// end`;\n/** Docs. */\nexport function b() { return 1; }\n"
  );
}

const CONTROLLER: &str = "@Controller('users')\nexport class UsersController {\n  /** Finds one. */\n  @Get(':id')\n  @UseGuards(AuthGuard)\n  find() {\n    return 1;\n  }\n}\n";
const FIND: &str = "@Get(':id')\n  @UseGuards(AuthGuard)\n  find() {\n    return 1;\n  }";

fn controller_method() -> SymbolSite<'static> {
  site(CONTROLLER, FIND, Syntax::TypeScript, Nesting::Nested)
}

#[test]
fn should_keep_decorators_inside_the_span_when_the_new_body_has_none() {
  let expected = "@Controller('users')\nexport class UsersController {\n  /** Finds one. */\n  @Get(':id')\n  @UseGuards(AuthGuard)\n  find() {\n    return 2;\n  }\n}\n";
  assert_eq!(run(&controller_method(), SymbolOp::ReplaceBody("find() {\n  return 2;\n}".to_owned())), expected);
}

#[test]
fn should_keep_decorators_when_the_new_body_brings_new_docs_or_a_plain_comment() {
  assert_eq!(
    run(&controller_method(), SymbolOp::ReplaceBody("/** New. */\nfind() {\n  return 2;\n}".to_owned())),
    "@Controller('users')\nexport class UsersController {\n  /** New. */\n  @Get(':id')\n  @UseGuards(AuthGuard)\n  find() {\n    return 2;\n  }\n}\n"
  );
  assert_eq!(
    run(&controller_method(), SymbolOp::ReplaceBody("// note\nfind() {\n  return 2;\n}".to_owned())),
    "@Controller('users')\nexport class UsersController {\n  /** Finds one. */\n  @Get(':id')\n  @UseGuards(AuthGuard)\n  // note\n  find() {\n    return 2;\n  }\n}\n"
  );
}

#[test]
fn should_replace_the_decorator_group_when_the_new_body_restates_decorators() {
  assert_eq!(
    run(&controller_method(), SymbolOp::ReplaceBody("@Get(':id')\nfind() {\n  return 2;\n}".to_owned())),
    "@Controller('users')\nexport class UsersController {\n  /** Finds one. */\n  @Get(':id')\n  find() {\n    return 2;\n  }\n}\n"
  );
}

#[test]
fn should_keep_class_decorators_when_the_new_class_has_none() {
  let class = &CONTROLLER[..CONTROLLER.len() - 1];
  let site = site(CONTROLLER, class, Syntax::TypeScript, Nesting::TopLevel);
  assert_eq!(
    run(&site, SymbolOp::ReplaceBody("export class UsersController {}".to_owned())),
    "@Controller('users')\nexport class UsersController {}\n"
  );
}

#[test]
fn should_keep_an_inline_decorator_when_the_new_member_has_none() {
  let content = "class A {\n  @Input() name: string;\n}\n";
  let member = site(content, "@Input() name: string;", Syntax::TypeScript, Nesting::Nested);
  assert_eq!(run(&member, SymbolOp::ReplaceBody("name: number;".to_owned())), "class A {\n  @Input() name: number;\n}\n");
  assert_eq!(
    run(&member, SymbolOp::ReplaceBody("/** Name. */\nname: number;".to_owned())),
    "class A {\n  /** Name. */\n  @Input() name: number;\n}\n"
  );
}

fn rust(content: &str, text: &str) -> String {
  run(&site(content, "fn a() {}", Syntax::Rust, Nesting::TopLevel), SymbolOp::ReplaceBody(text.to_owned()))
}

#[test]
fn should_replace_docs_that_follow_attributes_without_duplicating_them() {
  assert_eq!(
    rust("#[derive(Debug)]\n/// Docs.\nfn a() {}\n", "/// New.\nfn a() {}"),
    "#[derive(Debug)]\n/// New.\nfn a() {}\n"
  );
}

#[test]
fn should_treat_doc_attributes_as_docs() {
  assert_eq!(rust("#[doc = \"Old.\"]\n#[inline]\nfn a() {}\n", "/// New.\nfn a() {}"), "/// New.\n#[inline]\nfn a() {}\n");
  assert_eq!(
    rust("/// Old.\n#[inline]\nfn a() {}\n", "#[doc = \"New.\"]\nfn a() {}"),
    "#[doc = \"New.\"]\n#[inline]\nfn a() {}\n"
  );
}

#[test]
fn should_replace_docs_even_when_the_new_text_puts_attributes_first() {
  assert_eq!(rust("/// Old.\n#[inline]\nfn a() {}\n", "#[cold]\n/// New.\nfn a() {}"), "/// New.\n#[cold]\nfn a() {}\n");
}

#[test]
fn should_keep_plain_comments_above_replaced_docs() {
  assert_eq!(
    rust("// NOTE: keep in sync\n/// Old.\nfn a() {}\n", "/// New.\nfn a() {}"),
    "// NOTE: keep in sync\n/// New.\nfn a() {}\n"
  );
  assert_eq!(
    rust("// NOTE: keep in sync\n/// Old.\nfn a() {}\n", "// NOTE: keep in sync\n/// New.\nfn a() {}"),
    "// NOTE: keep in sync\n/// New.\nfn a() {}\n"
  );
}

#[test]
fn should_not_duplicate_an_attribute_restated_on_the_declaration_line() {
  assert_eq!(rust("#[inline]\nfn a() {}\n", "#[inline] fn a() {}"), "#[inline] fn a() {}\n");
}

fn member(content: &str, declaration: &str, text: &str) -> String {
  run(&site(content, declaration, Syntax::TypeScript, Nesting::Nested), SymbolOp::ReplaceBody(text.to_owned()))
}

#[test]
fn should_keep_a_decorator_whose_arguments_hold_a_comment_with_an_apostrophe() {
  let content = "class A {\n  @Column({\n    // don't change: legacy\n    type: 'text',\n  })\n  name: string;\n}\n";
  let declaration = "@Column({\n    // don't change: legacy\n    type: 'text',\n  })\n  name: string;";
  assert_eq!(
    member(content, declaration, "name: number;"),
    "class A {\n  @Column({\n    // don't change: legacy\n    type: 'text',\n  })\n  name: number;\n}\n"
  );
  assert_eq!(
    member(content, declaration, "/** Doc. */\nname: number;"),
    "class A {\n  /** Doc. */\n  @Column({\n    // don't change: legacy\n    type: 'text',\n  })\n  name: number;\n}\n"
  );
}

#[test]
fn should_keep_decorators_with_regex_block_comment_and_template_arguments() {
  let cases = [
    ("@Matches(/^\\(\\d+$/)", "@Matches(/^\\(\\d+$/)\n  phone: string;"),
    ("@Column({ /* it's ) here */ length: 3 })", "@Column({ /* it's ) here */ length: 3 })\n  phone: string;"),
    ("@Label(`a ) b`)", "@Label(`a ) b`)\n  phone: string;"),
    ("@Split(a / b, (c) / 2)", "@Split(a / b, (c) / 2)\n  phone: string;"),
  ];
  for (decorator, declaration) in cases {
    let content = format!("class A {{\n  {declaration}\n}}\n");
    assert_eq!(
      member(&content, declaration, "phone: number;"),
      format!("class A {{\n  {decorator}\n  phone: number;\n}}\n"),
      "{decorator}"
    );
  }
}

#[test]
fn should_refuse_a_declaration_whose_decorator_never_closes() {
  let content = "class A {\n  @Column({ type: 'text'\n  name: string;\n}\n";
  let broken = site(content, "@Column({ type: 'text'\n  name: string;", Syntax::TypeScript, Nesting::Nested);
  assert!(matches!(
    symbol_edit(&broken, &SymbolOp::ReplaceBody("name: number;".to_owned())),
    Err(EditError::RegionUnparseable("the declaration"))
  ));
  let fine = site(CONTROLLER, FIND, Syntax::TypeScript, Nesting::Nested);
  assert!(matches!(
    symbol_edit(&fine, &SymbolOp::ReplaceBody("@Get(':id'\nfind() {}".to_owned())),
    Err(EditError::RegionUnparseable("the new text"))
  ));
}

#[test]
fn should_keep_a_blank_line_between_a_decorator_and_a_bare_body() {
  let content = "class A {\n  @Get()\n\n  find() {\n    return 1;\n  }\n}\n";
  assert_eq!(
    member(content, "@Get()\n\n  find() {\n    return 1;\n  }", "find() {\n  return 2;\n}"),
    "class A {\n  @Get()\n\n  find() {\n    return 2;\n  }\n}\n"
  );
}

#[test]
fn should_keep_generic_decorator_calls_and_refuse_unclosed_type_arguments() {
  for decorator in ["@X<Foo>()", "@Inject<Map<string, number>>()", "@Pipe<(value: string) => number>(parse)"] {
    let declaration = format!("{decorator}\n  phone: string;");
    let content = format!("class A {{\n  {declaration}\n}}\n");
    assert_eq!(
      member(&content, &declaration, "phone: number;"),
      format!("class A {{\n  {decorator}\n  phone: number;\n}}\n"),
      "{decorator}"
    );
  }
  let content = "class A {\n  @X<Foo\n  phone: string;\n}\n";
  let broken = site(content, "@X<Foo\n  phone: string;", Syntax::TypeScript, Nesting::Nested);
  assert!(matches!(
    symbol_edit(&broken, &SymbolOp::ReplaceBody("phone: number;".to_owned())),
    Err(EditError::RegionUnparseable("the declaration"))
  ));
}
