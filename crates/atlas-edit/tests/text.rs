mod common;

use atlas_edit::{
  Disposition, EditError, NotTextReason, OccurrenceId, Pattern, ReplaceContent, ReplaceInFiles, SkipReason, SkippedFile, TextEdit, apply, replace_content, replace_in_files,
  replace_in_text,
};
use common::{Workspace, hash, target};

fn literal(pattern: &str, text: &str, all: bool) -> ReplaceContent {
  ReplaceContent {
    pattern: Pattern::Literal(pattern.to_owned()),
    text: text.to_owned(),
    all,
  }
}

fn regex(pattern: &str, text: &str, all: bool) -> ReplaceContent {
  ReplaceContent {
    pattern: Pattern::Regex(pattern.to_owned()),
    text: text.to_owned(),
    all,
  }
}

fn replaced(content: &str, request: &ReplaceContent) -> String {
  let plan = replace_in_text(&target("api", "a.ts"), content, request).unwrap();
  plan.plan.files[0].apply_to(content).unwrap()
}

#[test]
fn should_expand_regex_capture_groups() {
  let content = "let total = add(a, b);\nlet diff = sub(c, d);\n";
  assert_eq!(
    replaced(content, &regex(r"(\w+)\((\w+), (?P<second>\w+)\)", "$1(${second}, $2)", true)),
    "let total = add(b, a);\nlet diff = sub(d, c);\n"
  );
}

#[test]
fn should_match_across_lines_and_at_every_line_start() {
  assert_eq!(replaced("start\nmiddle\nend", &regex("start.*end", "all", false)), "all");
  assert_eq!(replaced("fn a\n  fn b\nfn c", &regex("^fn", "pub fn", true)), "pub fn a\n  fn b\npub fn c");
}

#[test]
fn should_keep_dollar_signs_literal_in_literal_mode() {
  assert_eq!(replaced("price", &literal("price", "$1 ${x}", false)), "$1 ${x}");
}

#[test]
fn should_refuse_multiple_matches_without_all() {
  let content = "x = x + 1";
  let error = replace_in_text(&target("api", "a.ts"), content, &literal("x", "y", false)).unwrap_err();
  let EditError::MultipleMatches { count, ids } = error else {
    panic!("expected MultipleMatches, got {error:?}")
  };
  let file = target("api", "a.ts");
  assert_eq!(count, 2);
  assert_eq!(ids, [OccurrenceId::derive(&file, &hash(content), 0, 1), OccurrenceId::derive(&file, &hash(content), 4, 5)]);
  assert_eq!(replaced(content, &literal("x", "y", true)), "y = y + 1");
}

#[test]
fn should_derive_stable_occurrence_ids_from_file_hash_and_offset() {
  let file = target("api", "src/a.ts");
  let id = OccurrenceId::derive(&file, &hash("foo"), 0, 3);
  let mut input = b"api\0src/a.ts\0".to_vec();
  input.extend_from_slice(hash("foo").as_bytes());
  input.extend_from_slice(&0u64.to_le_bytes());
  input.extend_from_slice(&3u64.to_le_bytes());
  assert_eq!(id.as_str(), format!("o-{}", &blake3::hash(&input).to_hex()[..10]));
  assert_eq!(id.as_str(), "o-543ff4ba10");
  assert_eq!(OccurrenceId::derive(&file, &hash("foo"), 0, 3), id);
  assert_ne!(OccurrenceId::derive(&file, &hash("foo "), 0, 3), id);
  assert_ne!(OccurrenceId::derive(&file, &hash("foo"), 1, 3), id);
  assert_ne!(OccurrenceId::derive(&target("web", "src/a.ts"), &hash("foo"), 0, 3), id);
  assert_eq!("o-543ff4ba10".parse::<OccurrenceId>().unwrap(), id);
  for invalid in ["o-0D9E70C5B6", "o-0d9e70c5b", "x-0d9e70c5b6", "o-0d9e70c5b6f"] {
    assert!(matches!(invalid.parse::<OccurrenceId>(), Err(EditError::InvalidOccurrenceId(value)) if value == invalid));
  }
}

#[test]
fn should_report_occurrence_positions() {
  let plan = replace_in_text(&target("api", "a.ts"), "\u{feff}ab\ncab", &literal("b", "B", true)).unwrap();
  let positions: Vec<_> = plan
    .occurrences
    .iter()
    .map(|occurrence| (occurrence.start, occurrence.end, occurrence.line, occurrence.column))
    .collect();
  assert_eq!(positions, [(4, 5, 1, 2), (8, 9, 2, 3)]);
}

#[test]
fn should_refuse_bad_patterns_and_empty_matches() {
  let file = target("api", "a.ts");
  assert!(matches!(replace_in_text(&file, "abc", &literal("z", "y", false)), Err(EditError::NoMatch { path }) if path == "api:a.ts"));
  assert!(matches!(replace_in_text(&file, "abc", &literal("", "y", false)), Err(EditError::EmptyPattern)));
  assert!(matches!(replace_in_text(&file, "abc", &regex("(", "y", false)), Err(EditError::InvalidRegex { pattern, .. }) if pattern == "("));
  assert!(matches!(
    replace_in_text(&file, "abc", &regex("z*", "y", true)),
    Err(EditError::EmptyMatch { offset: 0, .. })
  ));
}

#[test]
fn should_plan_replace_content_against_the_file_on_disk() {
  let workspace = Workspace::new(&["api"]);
  let expected = workspace.write("api", "a.ts", "const a = 1;\n");
  let plan = replace_content(&workspace.scope(), &target("api", "a.ts"), &literal("1", "2", false)).unwrap();
  assert_eq!(plan.plan.files[0].expected, expected);
  assert_eq!(plan.plan.files[0].edits, [TextEdit::replace(10..11, "2")]);
  apply(&plan.plan, &workspace.scope(), &workspace.options()).unwrap();
  assert_eq!(workspace.read("api", "a.ts"), "const a = 2;\n");
  workspace.write("api", "bin.ts", "a\0b");
  assert!(matches!(
    replace_content(&workspace.scope(), &target("api", "bin.ts"), &literal("a", "b", false)),
    Err(EditError::NotText {
      reason: NotTextReason::Binary,
      ..
    })
  ));
}

fn search(text: &str) -> ReplaceInFiles {
  ReplaceInFiles {
    pattern: Pattern::Literal("foo".to_owned()),
    text: text.to_owned(),
    include: Vec::new(),
    exclude: Vec::new(),
    occurrences: None,
    expected_count: None,
    dry_run: false,
  }
}

fn fixture() -> Workspace {
  let mut workspace = Workspace::new(&["api", "web"]);
  workspace.project.settings.max_file_bytes = 64;
  workspace.project.repos[1].exclude = vec!["legacy/".to_owned()];
  workspace.write("api", "src/a.ts", "foo foo\n");
  workspace.write("api", "src/b.rs", "fn foo() {}\n");
  workspace.write("api", "node_modules/pkg/x.ts", "foo");
  workspace.write("api", ".gitignore", "ignored.ts\n");
  workspace.write("api", "ignored.ts", "foo");
  workspace.write("api", ".hidden.ts", "foo");
  workspace.write("api", "image.png", b"foo\0\x89PNG".as_slice());
  workspace.write("api", "latin1.ts", b"foo \xe9".as_slice());
  workspace.write("api", "big.ts", "foo ".repeat(20));
  workspace.write("web", "c.ts", "foo");
  workspace.write("web", "legacy/old.ts", "foo");
  workspace
}

fn found(plan: &atlas_edit::FilesPlan) -> Vec<(String, usize)> {
  plan.text.occurrences.iter().map(|occurrence| (occurrence.file.to_string(), occurrence.start)).collect()
}

#[test]
fn should_treat_a_call_without_selection_as_a_dry_run() {
  let workspace = fixture();
  let plan = replace_in_files(&workspace.scope(), &search("bar")).unwrap();
  assert_eq!(plan.disposition, Disposition::ImplicitDryRun);
  assert_eq!(plan.total_matches, 4);
  assert_eq!(
    found(&plan),
    [
      ("api:src/a.ts".to_owned(), 0),
      ("api:src/a.ts".to_owned(), 4),
      ("api:src/b.rs".to_owned(), 3),
      ("web:c.ts".to_owned(), 0)
    ]
  );
  let explicit = replace_in_files(&workspace.scope(), &ReplaceInFiles { dry_run: true, ..search("bar") }).unwrap();
  assert_eq!(explicit.disposition, Disposition::DryRun);
}

#[test]
fn should_skip_binary_non_utf8_and_oversized_files() {
  let workspace = fixture();
  let plan = replace_in_files(&workspace.scope(), &search("bar")).unwrap();
  assert_eq!(
    plan.skipped,
    [
      SkippedFile {
        file: target("api", "big.ts"),
        reason: SkipReason::TooLarge { size: 80, limit: 64 },
      },
      SkippedFile {
        file: target("api", "image.png"),
        reason: SkipReason::Binary,
      },
      SkippedFile {
        file: target("api", "latin1.ts"),
        reason: SkipReason::NotUtf8,
      },
    ]
  );
}

#[test]
fn should_refuse_when_expected_count_differs() {
  let workspace = fixture();
  let error = replace_in_files(
    &workspace.scope(),
    &ReplaceInFiles {
      expected_count: Some(3),
      ..search("bar")
    },
  )
  .unwrap_err();
  assert!(matches!(error, EditError::ExpectedCountMismatch { expected: 3, found: 4 }), "{error:?}");
  let plan = replace_in_files(
    &workspace.scope(),
    &ReplaceInFiles {
      expected_count: Some(4),
      ..search("bar")
    },
  )
  .unwrap();
  assert_eq!(plan.disposition, Disposition::Apply);
  apply(&plan.text.plan, &workspace.scope(), &workspace.options()).unwrap();
  assert_eq!(
    (workspace.read("api", "src/a.ts"), workspace.read("api", "src/b.rs"), workspace.read("web", "c.ts")),
    ("bar bar\n".to_owned(), "fn bar() {}\n".to_owned(), "bar".to_owned())
  );
  assert_eq!(workspace.read("web", "legacy/old.ts"), "foo");
}

#[test]
fn should_apply_only_selected_occurrences() {
  let workspace = fixture();
  let dry = replace_in_files(&workspace.scope(), &search("bar")).unwrap();
  let selected = vec![dry.text.occurrences[1].id.clone(), dry.text.occurrences[3].id.clone()];
  let plan = replace_in_files(
    &workspace.scope(),
    &ReplaceInFiles {
      occurrences: Some(selected.clone()),
      ..search("bar")
    },
  )
  .unwrap();
  assert_eq!(plan.disposition, Disposition::Apply);
  assert_eq!(plan.total_matches, 4);
  assert_eq!(plan.text.occurrences.iter().map(|occurrence| occurrence.id.clone()).collect::<Vec<_>>(), selected);
  apply(&plan.text.plan, &workspace.scope(), &workspace.options()).unwrap();
  assert_eq!(
    (workspace.read("api", "src/a.ts"), workspace.read("api", "src/b.rs"), workspace.read("web", "c.ts")),
    ("foo bar\n".to_owned(), "fn foo() {}\n".to_owned(), "bar".to_owned())
  );
}

#[test]
fn should_refuse_unknown_or_empty_occurrence_selections() {
  let workspace = fixture();
  let stale: OccurrenceId = "o-0000000000".parse().unwrap();
  let error = replace_in_files(
    &workspace.scope(),
    &ReplaceInFiles {
      occurrences: Some(vec![stale.clone()]),
      ..search("bar")
    },
  )
  .unwrap_err();
  assert!(matches!(&error, EditError::UnknownOccurrences { ids } if ids == &[stale]), "{error:?}");
  assert!(matches!(
    replace_in_files(
      &workspace.scope(),
      &ReplaceInFiles {
        occurrences: Some(Vec::new()),
        ..search("bar")
      }
    ),
    Err(EditError::EmptySelection)
  ));
}

#[test]
fn should_filter_files_with_repo_prefixed_globs() {
  let workspace = fixture();
  let only_web = replace_in_files(
    &workspace.scope(),
    &ReplaceInFiles {
      include: vec!["web:*.ts".to_owned()],
      ..search("bar")
    },
  )
  .unwrap();
  assert_eq!(found(&only_web), [("web:c.ts".to_owned(), 0)]);
  let without_rust = replace_in_files(
    &workspace.scope(),
    &ReplaceInFiles {
      exclude: vec!["*.rs".to_owned(), "web:c.ts".to_owned()],
      ..search("bar")
    },
  )
  .unwrap();
  assert_eq!(found(&without_rust), [("api:src/a.ts".to_owned(), 0), ("api:src/a.ts".to_owned(), 4)]);
  let error = replace_in_files(
    &workspace.scope(),
    &ReplaceInFiles {
      include: vec!["nope:*.ts".to_owned()],
      ..search("bar")
    },
  )
  .unwrap_err();
  assert!(matches!(&error, EditError::UnknownRepo(name) if name == "nope"), "{error:?}");
}

#[test]
fn should_render_hunks_with_occurrence_ids() {
  let workspace = Workspace::new(&["api"]);
  let content = (1..=12)
    .map(|line| if line == 2 || line == 11 { format!("foo {line} foo") } else { format!("line {line}") })
    .collect::<Vec<_>>()
    .join("\n")
    + "\n";
  workspace.write("api", "a.ts", &content);
  let plan = replace_in_files(&workspace.scope(), &search("bar")).unwrap();
  let ids: Vec<&str> = plan.text.occurrences.iter().map(|occurrence| occurrence.id.as_str()).collect();
  let expected = format!(
    "--- api:a.ts\n+++ api:a.ts\n@@ -1,5 +1,5 @@\n line 1\n-foo 2 foo  [{} {}]\n+bar 2 bar\n line 3\n line 4\n line 5\n@@ -8,5 +8,5 @@\n line 8\n line 9\n line 10\n-foo 11 foo  [{} {}]\n+bar 11 bar\n line 12\n",
    ids[0], ids[1], ids[2], ids[3]
  );
  assert_eq!(plan.text.diffs[0].to_string(), expected);
}

#[test]
fn should_detect_the_dominant_line_ending_and_normalize_inserted_text() {
  use atlas_edit::LineEnding;
  assert_eq!(LineEnding::dominant("a\r\nb\r\nc\n"), LineEnding::CrLf);
  assert_eq!(LineEnding::dominant("a\r\nb\n"), LineEnding::Lf);
  assert_eq!(LineEnding::dominant("no newline"), LineEnding::Lf);
  assert_eq!(LineEnding::CrLf.normalize("a\nb\r\nc"), "a\r\nb\r\nc");
  assert_eq!(LineEnding::Lf.normalize("a\r\nb\nc"), "a\nb\nc");
}

#[test]
fn should_refuse_replacements_that_name_missing_capture_groups() {
  let file = target("api", "a.ts");
  let error = replace_in_text(&file, "foo\n", &regex("(foo)", "$1_bar", true)).unwrap_err();
  assert!(matches!(&error, EditError::UnknownCaptureGroup { reference } if reference == "1_bar"), "{error:?}");
  assert!(error.to_string().contains("${1}"), "{error}");
  let error = replace_in_text(&file, "price\n", &regex("price", "costs $5", true)).unwrap_err();
  assert!(matches!(&error, EditError::UnknownCaptureGroup { reference } if reference == "5"), "{error:?}");
  assert!(matches!(
    replace_in_text(&file, "x", &regex("x", "${missing}", true)),
    Err(EditError::UnknownCaptureGroup { reference }) if reference == "missing"
  ));
  assert_eq!(replaced("foo\n", &regex("(foo)", "${1}_bar", true)), "foo_bar\n");
  assert_eq!(replaced("price\n", &regex("price", "costs $$5", true)), "costs $5\n");
  assert_eq!(replaced("price\n", &regex("price", "costs $", true)), "costs $\n");
}

#[test]
fn should_match_line_anchors_and_multi_line_literals_in_crlf_files() {
  let content = "let a = 1;\r\nlet b = 2;\r\n";
  assert_eq!(replaced(content, &regex(r"= (\d);$", "= ${1}0;", true)), "let a = 10;\r\nlet b = 20;\r\n");
  assert_eq!(replaced(content, &literal("1;\nlet", "1;\nconst", false)), "let a = 1;\r\nconst b = 2;\r\n");
}

#[test]
fn should_let_dot_match_newlines_like_serena() {
  let content = "import a from 'a';\nimport b from 'b';\n";
  assert_eq!(replaced(content, &regex(r"^import a (.*)$", "X", false)), "X");
  assert_eq!(
    replaced(content, &regex(r"^import a ([^\n]*)$", "import A $1", false)),
    "import A from 'a';\nimport b from 'b';\n"
  );
}

#[test]
fn should_search_files_of_nested_repos_once() {
  let mut workspace = Workspace::new(&["outer"]);
  let inner = workspace.root("outer").join("pkg");
  std::fs::create_dir_all(&inner).unwrap();
  workspace.project.repos.push(atlas_project::RepoConfig {
    name: atlas_core::RepoName::new("inner").unwrap(),
    path: inner,
    include: Vec::new(),
    exclude: Vec::new(),
    languages: None,
  });
  workspace.write("outer", "pkg/a.ts", "const foo = 1;\n");
  workspace.write("outer", "b.ts", "foo\n");
  let plan = replace_in_files(
    &workspace.scope(),
    &ReplaceInFiles {
      expected_count: Some(2),
      ..search("bar")
    },
  )
  .unwrap();
  assert_eq!(found(&plan), [("outer:b.ts".to_owned(), 0), ("inner:a.ts".to_owned(), 6)]);
  apply(&plan.text.plan, &workspace.scope(), &workspace.options()).unwrap();
  assert_eq!(workspace.read("outer", "pkg/a.ts"), "const bar = 1;\n");
}
