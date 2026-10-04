mod common;

use std::sync::Arc;
use std::thread;

use atlas_core::{
  AnalysisError, AnalysisInput, Diagnostic, ImportFact, Language, LanguageAnalyzer, LanguageFamily, LocalSymbolId, MAX_BODY_TOKENS, MAX_REPORTED_PARSE_ERRORS, NormToken,
  ParseStatus, Producer, RefKind, Severity, Span, UseBinding, UsePath, Visibility, token_code,
};
use atlas_lang_rust::{DEFAULT_MAX_SOURCE_BYTES, RustAnalyzer};
use common::{analyze, file_ref, item, paths_of, strings, symbol_names};

fn use_path(segments: &[&str], binding: UseBinding) -> UsePath {
  UsePath {
    segments: strings(segments),
    binding,
  }
}

fn rust_uses(facts: &atlas_core::FileFacts) -> Vec<(Vec<UsePath>, Visibility, Option<LocalSymbolId>)> {
  facts
    .imports
    .iter()
    .filter_map(|import| match import {
      ImportFact::RustUse { paths, visibility, scope, .. } => Some((paths.clone(), visibility.clone(), *scope)),
      _ => None,
    })
    .collect()
}

#[test]
fn should_flatten_nested_use_trees() {
  let facts = analyze("use a::{b, c::{d as e, *}, self};\nuse ::std::io;\nuse crate::x::{self as y};\nuse {first, second::*};\nuse empty::{};\n");
  assert_eq!(
    rust_uses(&facts),
    [
      (
        vec![
          use_path(&["a", "b"], UseBinding::Name),
          use_path(&["a", "c", "d"], UseBinding::Alias("e".into())),
          use_path(&["a", "c"], UseBinding::Glob),
          use_path(&["a"], UseBinding::Name),
        ],
        Visibility::Private,
        None
      ),
      (vec![use_path(&["std", "io"], UseBinding::Name)], Visibility::Private, None),
      (vec![use_path(&["crate", "x"], UseBinding::Alias("y".into()))], Visibility::Private, None),
      (
        vec![use_path(&["first"], UseBinding::Name), use_path(&["second"], UseBinding::Glob)],
        Visibility::Private,
        None
      ),
    ]
  );
}

#[test]
fn should_mark_visible_uses_as_re_exports() {
  let facts = analyze("pub use inner::Thing;\npub(crate) use inner::*;\nuse inner::Private;\n");
  assert_eq!(facts.imports.iter().map(ImportFact::is_re_export).collect::<Vec<_>>(), [true, true, false]);
  assert_eq!(rust_uses(&facts)[1].0, [use_path(&["inner"], UseBinding::Glob)]);
}

#[test]
fn should_scope_uses_to_the_enclosing_function_or_inline_module() {
  let facts = analyze("use top::Level;\nmod nested {\n  use crate::a::*;\n}\nfn body() {\n  use std::fmt::Write;\n}\n");
  let scopes: Vec<Option<String>> = rust_uses(&facts)
    .into_iter()
    .map(|(_, _, scope)| scope.and_then(|local| facts.symbol(local)).map(|symbol| symbol.name.clone()))
    .collect();
  assert_eq!(scopes, [None, Some("nested".to_owned()), Some("body".to_owned())]);
}

#[test]
fn should_extract_extern_crate_declarations() {
  let source = "extern crate serde;\nextern crate core as std_core;\n";
  let facts = analyze(source);
  assert_eq!(
    facts.imports,
    [
      ImportFact::RustExternCrate {
        name: "serde".into(),
        alias: None,
        span: Span::of_text(source, 0, 19).unwrap(),
      },
      ImportFact::RustExternCrate {
        name: "core".into(),
        alias: Some("std_core".into()),
        span: Span::of_text(source, 20, source.len() - 1).unwrap(),
      },
    ]
  );
}

#[test]
fn should_fingerprint_bodies_with_abstracted_identifiers_and_literals() {
  let facts = analyze("fn a(x: u8) -> u8 { let y = x + 1; // note\n  helper(y, \"one\") }\nfn b(p: u8) -> u8 { let q = p + 2; helper(q, \"two\") }\nfn c(x: u8) -> u8 { x * 1 }\n");
  let fingerprint = |name: &str| item(&facts, &[name]).body.unwrap();
  let a = fingerprint("a");
  assert_eq!(a.normalized_tokens, fingerprint("b").normalized_tokens);
  assert_ne!(a.normalized_tokens, fingerprint("c").normalized_tokens);
  assert_eq!((a.token_count, a.line_count), (15, 2));
  let identifier = token_code(NormToken::Identifier);
  assert_eq!(
    a.normalized_tokens[..6],
    [
      token_code(NormToken::Punctuation("{")),
      token_code(NormToken::Keyword("let")),
      identifier,
      token_code(NormToken::Punctuation("=")),
      identifier,
      token_code(NormToken::Punctuation("+")),
    ]
  );
  assert_eq!(a.normalized_tokens[6], token_code(NormToken::NumberLiteral));
  assert_eq!(a.normalized_tokens[12], token_code(NormToken::StringLiteral));
}

#[test]
fn should_cap_fingerprint_tokens_but_count_every_token() {
  let source = format!("fn long() {{ {} }}\n", "call(); ".repeat(2_000));
  let body = item(&analyze(&source), &["long"]).body.unwrap();
  assert_eq!(body.normalized_tokens.len(), MAX_BODY_TOKENS);
  assert_eq!(body.token_count, 8_002);
}

#[test]
fn should_report_partial_parses_and_keep_extracting_well_formed_items() {
  let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/adversarial/broken-syntax/crate/src/lib.rs")).unwrap();
  let facts = analyze(&source);
  let missing = source.find("1 + ;").unwrap() + 3;
  assert_eq!(
    facts.parse_status,
    ParseStatus::Partial {
      error_count: 1,
      first_errors: vec![atlas_core::ParseErrorFact {
        span: Span::of_text(&source, missing, missing).unwrap(),
        message: "missing `identifier`".into(),
      }],
    }
  );
  assert_eq!(
    symbol_names(&facts),
    [
      "before#function",
      "Early#struct",
      "Early::run#method",
      "middle#function",
      "after#function",
      "Late#struct",
      "Late::go#method"
    ]
  );
  assert_eq!(paths_of(&facts, RefKind::Call), ["before", "before", "after"]);
}

#[test]
fn should_cap_reported_parse_errors() {
  let source = "fn ok() {}\n".to_owned() + &"fn broken( {}\n".repeat(20) + "struct Still;\n";
  let facts = analyze(&source);
  let ParseStatus::Partial { error_count, first_errors } = &facts.parse_status else {
    panic!("expected a partial parse")
  };
  assert!(*error_count as usize > MAX_REPORTED_PARSE_ERRORS);
  assert_eq!(first_errors.len(), MAX_REPORTED_PARSE_ERRORS);
  assert_eq!(item(&facts, &["ok"]).name, "ok");
}

#[test]
fn should_reject_sources_above_the_size_limit() {
  let source = "fn a() {}\n".repeat(10);
  let file = file_ref("src/lib.rs", &source);
  let analyzer = RustAnalyzer::new().with_max_source_bytes(50);
  assert_eq!(
    analyzer.analyze(&AnalysisInput { file: &file, source: &source }),
    Err(AnalysisError::SourceTooLarge { bytes: 100, limit: 50 })
  );
  let oversized = "a".repeat(DEFAULT_MAX_SOURCE_BYTES + 1);
  assert_eq!(
    RustAnalyzer::new().analyze(&AnalysisInput { file: &file, source: &oversized }),
    Err(AnalysisError::SourceTooLarge {
      bytes: DEFAULT_MAX_SOURCE_BYTES + 1,
      limit: DEFAULT_MAX_SOURCE_BYTES
    })
  );
}

#[test]
fn should_reject_non_rust_files_and_describe_itself() {
  let analyzer = RustAnalyzer::new();
  let file = file_ref("src/index.ts", "");
  assert_eq!(
    analyzer.analyze(&AnalysisInput { file: &file, source: "" }),
    Err(AnalysisError::UnsupportedLanguage(Language::TypeScript))
  );
  assert_eq!(analyzer.language_family(), LanguageFamily::Rust);
  assert_eq!(analyzer.producer(), Producer::new("atlas-lang-rust", 2));
}

#[test]
fn should_analyze_empty_and_whitespace_only_files() {
  for source in ["", "\n\n  \t", "// only a comment\n"] {
    let facts = analyze(source);
    assert_eq!((facts.parse_status, facts.symbols.len(), facts.references.len()), (ParseStatus::Ok, 0, 0));
  }
}

#[test]
fn should_survive_deeply_nested_syntax_and_report_the_depth_limit() {
  let sources = [
    format!("fn parens() {{ let x = {}1{}; }}\n", "(".repeat(20_000), ")".repeat(20_000)),
    format!("fn chain() {{ start(){}; }}\n", ".next()".repeat(20_000)),
    format!("{}fn deep() {{}}{}\n", "mod m { ".repeat(5_000), " }".repeat(5_000)),
    format!("fn types() {{ let v: {}u8{} = f(); }}\n", "Vec<".repeat(5_000), ">".repeat(5_000)),
    format!("fn sums() {{ g({}\"b\"); }}\n", "\"a\" + ".repeat(20_000)),
    format!("#[x({}y{})]\nfn attributed() {{}}\n", "a(".repeat(5_000), ")".repeat(5_000)),
    format!("fn macros() {{ m!({}{}); }}\n", "n!(".repeat(5_000), ")".repeat(5_000)),
  ];
  let depth_limit = Diagnostic {
    severity: Severity::Warning,
    code: "rust.depth_limit".into(),
    message: "syntax nested beyond the analyzer depth limit was skipped".into(),
    span: None,
  };
  for source in &sources {
    let facts = analyze(source);
    assert!(!facts.symbols.is_empty());
    assert!(facts.diagnostics.is_empty() || facts.diagnostics == [depth_limit.clone()]);
  }
  assert_eq!(analyze(&sources[0]).diagnostics, [depth_limit]);
}

#[test]
fn should_analyze_concurrently_with_a_parser_per_thread() {
  let analyzer = Arc::new(RustAnalyzer::new());
  let handles: Vec<_> = (0..8)
    .map(|index| {
      let analyzer = Arc::clone(&analyzer);
      thread::spawn(move || {
        let source = format!("pub fn worker_{index}() {{ helper_{index}(); }}\n");
        let file = file_ref("src/lib.rs", &source);
        (0..50)
          .map(|_| analyzer.analyze(&AnalysisInput { file: &file, source: &source }).unwrap().references[0].path.clone())
          .collect::<Vec<_>>()
      })
    })
    .collect();
  for (index, handle) in handles.into_iter().enumerate() {
    let paths = handle.join().unwrap();
    assert!(paths.iter().all(|path| *path == [format!("helper_{index}")]));
  }
}

#[test]
fn should_handle_multibyte_text_in_spans_and_summaries() {
  let source = "/// Grüße ✓\nfn grüß(name: &str) { println!(\"héllo {} ✓\", name); }\n";
  let facts = analyze(source);
  let function = item(&facts, &["grüß"]);
  assert_eq!(function.doc.as_deref(), Some("Grüße ✓"));
  let start = source.find("grüß").unwrap();
  assert_eq!(function.name_span, Span::of_text(source, start, start + "grüß".len()).unwrap());
  assert_eq!(facts.references[0].args[0], atlas_core::ArgSummary::Str("héllo {} ✓".into()));
}
