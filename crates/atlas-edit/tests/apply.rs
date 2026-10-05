mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use atlas_edit::{AppliedChange, EditError, EditLock, EditPlan, FileEdits, PlanViolation, RefusalReason, ResourceOp, TextEdit, apply};
use common::{Workspace, hash, target};

fn edit_plan(files: Vec<FileEdits>) -> EditPlan {
  EditPlan { files, resources: Vec::new() }
}

#[test]
fn should_apply_edits_to_several_files_and_report_each_change() {
  let workspace = Workspace::new(&["api", "web"]);
  let api = workspace.write("api", "src/a.ts", "const a = 1;\nconst b = 2;\n");
  let web = workspace.write("web", "b.ts", "hello world");
  let plan = edit_plan(vec![
    FileEdits::new(target("api", "src/a.ts"), api, vec![TextEdit::replace(19..20, "B"), TextEdit::replace(6..7, "A")]),
    FileEdits::new(target("web", "b.ts"), web, vec![TextEdit::insert(11, "!")]),
  ]);
  let report = apply(&plan, &workspace.scope(), &workspace.options()).unwrap();
  assert_eq!(workspace.read("api", "src/a.ts"), "const A = 1;\nconst B = 2;\n");
  assert_eq!(workspace.read("web", "b.ts"), "hello world!");
  assert_eq!(
    report.changes,
    [
      AppliedChange::Modified {
        file: target("api", "src/a.ts"),
        before: api,
        after: hash("const A = 1;\nconst B = 2;\n"),
        edits: 2,
      },
      AppliedChange::Modified {
        file: target("web", "b.ts"),
        before: web,
        after: hash("hello world!"),
        edits: 1,
      },
    ]
  );
}

#[test]
fn should_refuse_a_plan_when_a_file_changed_since_planning() {
  let workspace = Workspace::new(&["api"]);
  let first = workspace.write("api", "a.ts", "one");
  let second = workspace.write("api", "b.ts", "two");
  let plan = edit_plan(vec![
    FileEdits::new(target("api", "a.ts"), first, vec![TextEdit::replace(0..3, "ONE")]),
    FileEdits::new(target("api", "b.ts"), second, vec![TextEdit::replace(0..3, "TWO")]),
  ]);
  workspace.write("api", "b.ts", "two, edited elsewhere");
  let error = apply(&plan, &workspace.scope(), &workspace.options()).unwrap_err();
  assert!(matches!(&error, EditError::Stale { paths } if paths == &["api:b.ts"]), "{error:?}");
  assert_eq!(workspace.read("api", "a.ts"), "one");
  assert_eq!(workspace.read("api", "b.ts"), "two, edited elsewhere");
}

#[test]
fn should_refuse_the_whole_plan_when_one_file_is_out_of_scope() {
  let workspace = Workspace::new(&["api"]);
  let kept = workspace.write("api", "a.ts", "one");
  let ignored = workspace.write("api", "dist/b.ts", "two");
  let plan = edit_plan(vec![
    FileEdits::new(target("api", "a.ts"), kept, vec![TextEdit::replace(0..3, "ONE")]),
    FileEdits::new(target("api", "dist/b.ts"), ignored, vec![TextEdit::replace(0..3, "TWO")]),
  ]);
  let error = apply(&plan, &workspace.scope(), &workspace.options()).unwrap_err();
  let EditError::OutOfScope { refused } = error else {
    panic!("expected OutOfScope, got {error:?}")
  };
  assert_eq!(refused.len(), 1);
  assert_eq!(refused[0].reason, RefusalReason::DefaultExcluded("dist".to_owned()));
  assert_eq!(workspace.read("api", "a.ts"), "one");
}

#[test]
fn should_keep_crlf_line_endings_and_bom() {
  let workspace = Workspace::new(&["api"]);
  let content = "\u{feff}fn a() {\r\n  one();\r\n}\r\n";
  let expected = workspace.write("api", "a.rs", content);
  let request = atlas_edit::ReplaceContent {
    pattern: atlas_edit::Pattern::Literal("  one();".to_owned()),
    text: "  one();\n  two();".to_owned(),
    all: false,
  };
  let planned = atlas_edit::replace_content(&workspace.scope(), &target("api", "a.rs"), &request).unwrap();
  assert_eq!(planned.plan.files[0].expected, expected);
  assert_eq!(planned.plan.files[0].edits, [TextEdit::replace(13..21, "  one();\r\n  two();")]);
  apply(&planned.plan, &workspace.scope(), &workspace.options()).unwrap();
  assert_eq!(workspace.read("api", "a.rs"), "\u{feff}fn a() {\r\n  one();\r\n  two();\r\n}\r\n");
}

#[test]
fn should_refuse_edits_that_touch_the_byte_order_mark() {
  let workspace = Workspace::new(&["api"]);
  let expected = workspace.write("api", "a.ts", "\u{feff}x");
  let plan = edit_plan(vec![FileEdits::new(target("api", "a.ts"), expected, vec![TextEdit::insert(0, "y")])]);
  let error = apply(&plan, &workspace.scope(), &workspace.options()).unwrap_err();
  assert!(matches!(error, EditError::InvalidPlan(PlanViolation::TouchesBom { .. })), "{error:?}");
  assert_eq!(workspace.read("api", "a.ts"), "\u{feff}x");
}

#[test]
fn should_preserve_file_permissions() {
  let workspace = Workspace::new(&["api"]);
  let expected = workspace.write("api", "run.sh", "echo a\n");
  fs::set_permissions(workspace.path("api", "run.sh"), fs::Permissions::from_mode(0o751)).unwrap();
  let plan = edit_plan(vec![FileEdits::new(target("api", "run.sh"), expected, vec![TextEdit::replace(5..6, "b")])]);
  apply(&plan, &workspace.scope(), &workspace.options()).unwrap();
  assert_eq!(workspace.read("api", "run.sh"), "echo b\n");
  assert_eq!(fs::metadata(workspace.path("api", "run.sh")).unwrap().permissions().mode() & 0o777, 0o751);
}

#[test]
fn should_skip_files_whose_edits_change_nothing() {
  let workspace = Workspace::new(&["api"]);
  let expected = workspace.write("api", "a.ts", "same");
  let plan = edit_plan(vec![FileEdits::new(target("api", "a.ts"), expected, vec![TextEdit::replace(0..4, "same")])]);
  assert_eq!(apply(&plan, &workspace.scope(), &workspace.options()).unwrap().changes, []);
}

#[test]
fn should_refuse_invalid_plans_before_touching_files() {
  let workspace = Workspace::new(&["api"]);
  let expected = workspace.write("api", "a.ts", "héllo");
  let file = target("api", "a.ts");
  let violation = |edits: Vec<TextEdit>| {
    let plan = EditPlan {
      files: vec![FileEdits {
        file: file.clone(),
        expected,
        edits,
      }],
      resources: Vec::new(),
    };
    match apply(&plan, &workspace.scope(), &workspace.options()).unwrap_err() {
      EditError::InvalidPlan(violation) => violation,
      other => panic!("expected InvalidPlan, got {other:?}"),
    }
  };
  assert_eq!(
    violation(vec![TextEdit::replace(0..3, "a"), TextEdit::replace(2..4, "b")]),
    PlanViolation::Overlap {
      file: file.clone(),
      first: (0, 3),
      second: (2, 4),
    }
  );
  assert_eq!(
    violation(vec![TextEdit::replace(3..4, "a"), TextEdit::replace(0..1, "b")]),
    PlanViolation::Unsorted { file: file.clone(), index: 1 }
  );
  assert_eq!(
    violation(vec![TextEdit {
      start: 4,
      end: 2,
      new_text: "a".to_owned()
    }]),
    PlanViolation::Inverted {
      file: file.clone(),
      start: 4,
      end: 2
    }
  );
  assert_eq!(
    violation(vec![TextEdit::replace(0..9, "a")]),
    PlanViolation::OutOfBounds {
      file: file.clone(),
      start: 0,
      end: 9,
      len: 6,
    }
  );
  assert_eq!(
    violation(vec![TextEdit::replace(2..3, "e")]),
    PlanViolation::NotCharBoundary { file: file.clone(), offset: 2 }
  );
  assert_eq!(violation(Vec::new()), PlanViolation::NoEdits { file: file.clone() });
  assert!(matches!(
    apply(&EditPlan::default(), &workspace.scope(), &workspace.options()),
    Err(EditError::InvalidPlan(PlanViolation::Empty))
  ));
  assert_eq!(workspace.read("api", "a.ts"), "héllo");
}

#[test]
fn should_accept_adjacent_edits_and_two_insertions_at_one_offset() {
  let workspace = Workspace::new(&["api"]);
  let expected = workspace.write("api", "a.ts", "abc");
  let plan = edit_plan(vec![FileEdits::new(
    target("api", "a.ts"),
    expected,
    vec![
      TextEdit::insert(1, "1"),
      TextEdit::insert(1, "2"),
      TextEdit::replace(1..2, "B"),
      TextEdit::replace(2..3, "C"),
    ],
  )]);
  apply(&plan, &workspace.scope(), &workspace.options()).unwrap();
  assert_eq!(workspace.read("api", "a.ts"), "a12BC");
}

#[test]
fn should_apply_content_edits_then_rename_create_and_delete() {
  let workspace = Workspace::new(&["api"]);
  let old = workspace.write("api", "src/old.rs", "pub fn old() {}\n");
  let gone = workspace.write("api", "src/gone.rs", "x");
  let plan = EditPlan {
    files: vec![FileEdits::new(target("api", "src/old.rs"), old, vec![TextEdit::replace(7..10, "new")])],
    resources: vec![
      ResourceOp::Rename {
        from: target("api", "src/old.rs"),
        to: target("api", "src/new.rs"),
        expected: old,
      },
      ResourceOp::Create {
        file: target("api", "src/created.rs"),
        content: "fn created() {}\n".to_owned(),
      },
      ResourceOp::Delete {
        file: target("api", "src/gone.rs"),
        expected: gone,
      },
    ],
  };
  let report = apply(&plan, &workspace.scope(), &workspace.options()).unwrap();
  assert_eq!(workspace.read("api", "src/new.rs"), "pub fn new() {}\n");
  assert_eq!(workspace.read("api", "src/created.rs"), "fn created() {}\n");
  assert_eq!(fs::metadata(workspace.path("api", "src/created.rs")).unwrap().permissions().mode() & 0o777, 0o644);
  assert!(!workspace.exists("api", "src/old.rs"));
  assert!(!workspace.exists("api", "src/gone.rs"));
  assert_eq!(
    report.changes,
    [
      AppliedChange::Modified {
        file: target("api", "src/old.rs"),
        before: old,
        after: hash("pub fn new() {}\n"),
        edits: 1,
      },
      AppliedChange::Renamed {
        from: target("api", "src/old.rs"),
        to: target("api", "src/new.rs"),
      },
      AppliedChange::Created {
        file: target("api", "src/created.rs"),
        after: hash("fn created() {}\n"),
      },
      AppliedChange::Deleted {
        file: target("api", "src/gone.rs"),
        before: gone,
      },
    ]
  );
}

#[test]
fn should_refuse_resource_operations_onto_existing_files_without_writing() {
  let workspace = Workspace::new(&["api"]);
  let edited = workspace.write("api", "edited.ts", "a");
  let from = workspace.write("api", "from.ts", "from");
  workspace.write("api", "taken.ts", "taken");
  let plan = EditPlan {
    files: vec![FileEdits::new(target("api", "edited.ts"), edited, vec![TextEdit::replace(0..1, "b")])],
    resources: vec![ResourceOp::Rename {
      from: target("api", "from.ts"),
      to: target("api", "taken.ts"),
      expected: from,
    }],
  };
  let error = apply(&plan, &workspace.scope(), &workspace.options()).unwrap_err();
  assert!(matches!(&error, EditError::AlreadyExists { path } if path == "api:taken.ts"), "{error:?}");
  let create = EditPlan {
    files: Vec::new(),
    resources: vec![ResourceOp::Create {
      file: target("api", "taken.ts"),
      content: String::new(),
    }],
  };
  assert!(matches!(apply(&create, &workspace.scope(), &workspace.options()), Err(EditError::AlreadyExists { .. })));
  let missing = EditPlan {
    files: Vec::new(),
    resources: vec![ResourceOp::Delete {
      file: target("api", "missing.ts"),
      expected: from,
    }],
  };
  assert!(matches!(apply(&missing, &workspace.scope(), &workspace.options()), Err(EditError::Missing { path }) if path == "api:missing.ts"));
  let stale_delete = EditPlan {
    files: Vec::new(),
    resources: vec![ResourceOp::Delete {
      file: target("api", "taken.ts"),
      expected: from,
    }],
  };
  assert!(matches!(apply(&stale_delete, &workspace.scope(), &workspace.options()), Err(EditError::Stale { paths }) if paths == ["api:taken.ts"]));
  assert_eq!(
    (workspace.read("api", "edited.ts"), workspace.read("api", "from.ts"), workspace.read("api", "taken.ts")),
    ("a".to_owned(), "from".to_owned(), "taken".to_owned())
  );
}

#[test]
fn should_reject_conflicting_resource_operations() {
  let workspace = Workspace::new(&["api"]);
  let a = workspace.write("api", "a.ts", "a");
  let b = workspace.write("api", "b.ts", "b");
  let edits = |file: &str, expected| FileEdits::new(target("api", file), expected, vec![TextEdit::insert(0, "x")]);
  let conflict = |files: Vec<FileEdits>, resources: Vec<ResourceOp>| match apply(&EditPlan { files, resources }, &workspace.scope(), &workspace.options()) {
    Err(EditError::InvalidPlan(PlanViolation::ResourceConflict { path, reason })) => (path.to_string(), reason),
    other => panic!("expected a resource conflict, got {other:?}"),
  };
  let rename = |from: &str, to: &str, expected| ResourceOp::Rename {
    from: target("api", from),
    to: target("api", to),
    expected,
  };
  assert_eq!(
    conflict(Vec::new(), vec![rename("a.ts", "c.ts", a), rename("b.ts", "c.ts", b)]),
    ("api:c.ts".to_owned(), "appears in more than one resource operation")
  );
  assert_eq!(
    conflict(vec![edits("b.ts", b)], vec![rename("a.ts", "b.ts", a)]),
    ("api:b.ts".to_owned(), "a rename destination cannot also be edited")
  );
  assert_eq!(
    conflict(
      vec![edits("a.ts", a)],
      vec![ResourceOp::Delete {
        file: target("api", "a.ts"),
        expected: a,
      }]
    ),
    ("api:a.ts".to_owned(), "a deleted file cannot also be edited")
  );
  assert_eq!(
    conflict(
      vec![edits("a.ts", a)],
      vec![ResourceOp::Create {
        file: target("api", "a.ts"),
        content: String::new(),
      }]
    ),
    ("api:a.ts".to_owned(), "a created file cannot also be edited")
  );
  assert_eq!(
    conflict(Vec::new(), vec![rename("a.ts", "a.ts", a)]),
    ("api:a.ts".to_owned(), "renames a file onto itself (case-only renames are not supported)")
  );
  assert_eq!(
    conflict(vec![edits("a.ts", a)], vec![rename("a.ts", "c.ts", b)]),
    ("api:a.ts".to_owned(), "the rename and the content edits expect different contents")
  );
  assert_eq!((workspace.read("api", "a.ts"), workspace.read("api", "b.ts")), ("a".to_owned(), "b".to_owned()));
}

#[test]
fn should_time_out_while_another_holder_keeps_the_edit_lock() {
  let workspace = Workspace::new(&["api"]);
  let expected = workspace.write("api", "a.ts", "a");
  let plan = edit_plan(vec![FileEdits::new(target("api", "a.ts"), expected, vec![TextEdit::replace(0..1, "b")])]);
  let mut options = workspace.options();
  options.lock_timeout = Duration::from_millis(50);
  let mut holder = EditLock::open(&options.lock_path).unwrap();
  let error = holder.hold(Duration::from_secs(1), || apply(&plan, &workspace.scope(), &options).unwrap_err()).unwrap();
  assert!(matches!(&error, EditError::LockTimeout { path, .. } if path == &options.lock_path), "{error:?}");
  assert_eq!(workspace.read("api", "a.ts"), "a");
  apply(&plan, &workspace.scope(), &options).unwrap();
  assert_eq!(workspace.read("api", "a.ts"), "b");
}

const LOCK_HOLDER_ENV: &str = "ATLAS_EDIT_TEST_LOCK_HOLDER";

#[test]
fn should_time_out_while_another_process_holds_the_edit_lock() {
  if let Some(lock_path) = std::env::var_os(LOCK_HOLDER_ENV) {
    let lock_path = std::path::PathBuf::from(lock_path);
    let mut lock = EditLock::open(&lock_path).unwrap();
    lock
      .hold(Duration::from_secs(5), || {
        fs::write(lock_path.with_extension("held"), "").unwrap();
        std::thread::sleep(Duration::from_millis(400));
      })
      .unwrap();
    return;
  }
  let workspace = Workspace::new(&["api"]);
  let expected = workspace.write("api", "a.ts", "a");
  let plan = edit_plan(vec![FileEdits::new(target("api", "a.ts"), expected, vec![TextEdit::replace(0..1, "b")])]);
  let mut options = workspace.options();
  options.lock_timeout = Duration::from_millis(50);
  let mut child = std::process::Command::new(std::env::current_exe().unwrap())
    .args(["--exact", "should_time_out_while_another_process_holds_the_edit_lock", "--nocapture"])
    .env(LOCK_HOLDER_ENV, &options.lock_path)
    .stdout(std::process::Stdio::null())
    .spawn()
    .unwrap();
  let held = options.lock_path.with_extension("held");
  while !held.exists() {
    std::thread::sleep(Duration::from_millis(5));
  }
  assert!(matches!(apply(&plan, &workspace.scope(), &options), Err(EditError::LockTimeout { .. })));
  assert!(child.wait().unwrap().success());
  apply(&plan, &workspace.scope(), &options).unwrap();
  assert_eq!(workspace.read("api", "a.ts"), "b");
}

#[test]
fn should_refuse_case_aliases_of_protected_and_ignored_files() {
  let workspace = Workspace::new(&["api"]);
  let config = workspace.write("api", ".git/config", "[core]\n");
  workspace.write("api", ".gitignore", "secret.env\n");
  let secret = workspace.write("api", "secret.env", "TOKEN=a\n");
  let foo = workspace.write("api", "Foo.ts", "aaaa");
  let git = apply(
    &edit_plan(vec![FileEdits::new(target("api", ".GIT/config"), config, vec![TextEdit::insert(7, "x")])]),
    &workspace.scope(),
    &workspace.options(),
  );
  assert!(matches!(git, Err(EditError::OutOfScope { .. })), "{git:?}");
  let case_insensitive = workspace.path("api", "foo.ts").exists();
  if case_insensitive {
    let ignored = apply(
      &edit_plan(vec![FileEdits::new(target("api", "Secret.env"), secret, vec![TextEdit::replace(6..7, "b")])]),
      &workspace.scope(),
      &workspace.options(),
    );
    assert!(
      matches!(&ignored, Err(EditError::OutOfScope { refused }) if refused[0].reason == RefusalReason::Gitignored),
      "{ignored:?}"
    );
    let double = apply(
      &edit_plan(vec![
        FileEdits::new(target("api", "Foo.ts"), foo, vec![TextEdit::replace(0..1, "X")]),
        FileEdits::new(target("api", "foo.ts"), foo, vec![TextEdit::replace(3..4, "Y")]),
      ]),
      &workspace.scope(),
      &workspace.options(),
    );
    assert!(matches!(double, Err(EditError::InvalidPlan(PlanViolation::DuplicateFile { .. }))), "{double:?}");
    let case_only = EditPlan {
      files: Vec::new(),
      resources: vec![ResourceOp::Rename {
        from: target("api", "Foo.ts"),
        to: target("api", "foo.ts"),
        expected: foo,
      }],
    };
    assert!(matches!(
      apply(&case_only, &workspace.scope(), &workspace.options()),
      Err(EditError::InvalidPlan(PlanViolation::ResourceConflict { .. }))
    ));
  }
  assert_eq!(
    (workspace.read("api", ".git/config"), workspace.read("api", "secret.env"), workspace.read("api", "Foo.ts")),
    ("[core]\n".to_owned(), "TOKEN=a\n".to_owned(), "aaaa".to_owned())
  );
}

#[test]
fn should_refuse_read_only_files() {
  let workspace = Workspace::new(&["api"]);
  let edited = workspace.write("api", "ro.ts", "const a = 1;\n");
  let deleted = workspace.write("api", "gone.ts", "x");
  let renamed = workspace.write("api", "from.ts", "y");
  for file in ["ro.ts", "gone.ts", "from.ts"] {
    fs::set_permissions(workspace.path("api", file), fs::Permissions::from_mode(0o444)).unwrap();
  }
  let plans = [
    edit_plan(vec![FileEdits::new(target("api", "ro.ts"), edited, vec![TextEdit::replace(10..11, "2")])]),
    EditPlan {
      files: Vec::new(),
      resources: vec![ResourceOp::Delete {
        file: target("api", "gone.ts"),
        expected: deleted,
      }],
    },
    EditPlan {
      files: Vec::new(),
      resources: vec![ResourceOp::Rename {
        from: target("api", "from.ts"),
        to: target("api", "to.ts"),
        expected: renamed,
      }],
    },
  ];
  let refused: Vec<String> = plans
    .iter()
    .map(|plan| match apply(plan, &workspace.scope(), &workspace.options()) {
      Err(EditError::ReadOnly { path }) => path,
      other => panic!("expected ReadOnly, got {other:?}"),
    })
    .collect();
  assert_eq!(refused, ["api:ro.ts", "api:gone.ts", "api:from.ts"]);
  assert_eq!(workspace.read("api", "ro.ts"), "const a = 1;\n");
  assert!(workspace.exists("api", "gone.ts") && workspace.exists("api", "from.ts") && !workspace.exists("api", "to.ts"));
}

fn temp_names(directory: &std::path::Path) -> Vec<String> {
  let mut names: Vec<String> = fs::read_dir(directory)
    .unwrap()
    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
    .filter(|name| name.starts_with(".tmp") || name.starts_with(atlas_edit::TEMP_PREFIX))
    .collect();
  names.sort();
  names
}

#[test]
fn should_name_temporary_files_and_sweep_them_after_a_killed_apply() {
  const CHILD_ENV: &str = "ATLAS_EDIT_TEST_CRASH_DIR";
  const FILES: usize = 400;
  let plan_for = |workspace: &Workspace| {
    let files = (0..FILES)
      .map(|index| {
        let name = format!("src/f{index:05}.ts");
        FileEdits::new(target("api", &name), hash(&workspace.read("api", &name)), vec![TextEdit::replace(0..3, "NEW")])
      })
      .collect();
    edit_plan(files)
  };
  if let Some(base) = std::env::var_os(CHILD_ENV) {
    let workspace = Workspace::at(std::path::PathBuf::from(base), &["api"]);
    apply(&plan_for(&workspace), &workspace.scope(), &workspace.options()).unwrap();
    return;
  }
  let workspace = Workspace::new(&["api"]);
  for index in 0..FILES {
    workspace.write("api", &format!("src/f{index:05}.ts"), "old line\n");
  }
  let mut child = std::process::Command::new(std::env::current_exe().unwrap())
    .args(["--exact", "should_name_temporary_files_and_sweep_them_after_a_killed_apply", "--nocapture"])
    .env(CHILD_ENV, &workspace.base)
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .spawn()
    .unwrap();
  while !workspace.read("api", "src/f00000.ts").starts_with("NEW") && child.try_wait().unwrap().is_none() {
    std::thread::yield_now();
  }
  child.kill().unwrap();
  child.wait().unwrap();
  let source = workspace.path("api", "src");
  let leftovers = temp_names(&source);
  let old = (0..FILES).filter(|index| workspace.read("api", &format!("src/f{index:05}.ts")) == "old line\n").count();
  assert!(leftovers.iter().all(|name| name.starts_with(atlas_edit::TEMP_PREFIX)), "{leftovers:?}");
  assert_eq!(leftovers.len(), old);
  let current = workspace.read("api", "src/f00000.ts");
  let report = apply(
    &edit_plan(vec![FileEdits::new(target("api", "src/f00000.ts"), hash(&current), vec![TextEdit::replace(0..3, "TOP")])]),
    &workspace.scope(),
    &workspace.options(),
  )
  .unwrap();
  assert_eq!(report.swept.len(), leftovers.len());
  assert_eq!(temp_names(&source), Vec::<String>::new());
}

#[test]
fn should_sweep_stale_temporary_files_across_the_project() {
  let workspace = Workspace::new(&["api"]);
  fs::write(&workspace.options().lock_path, "").unwrap();
  let prefix = atlas_edit::temp_prefix(&workspace.options().lock_path);
  workspace.write("api", &format!("src/deep/{prefix}abc123"), "stale");
  workspace.write("api", &format!(".config/{prefix}def456"), "stale");
  workspace.write("api", &format!("node_modules/pkg/{prefix}ghi789"), "untouched");
  workspace.write("api", "src/.atlas-edit.0000ffff.abc", "another lock");
  workspace.write("api", "src/.atlas-editor.ts", "not a temp");
  let mut swept = atlas_edit::sweep_stale_temps(&workspace.scope(), &workspace.options()).unwrap();
  swept.sort();
  assert_eq!(
    swept,
    [
      workspace.path("api", &format!(".config/{prefix}def456")),
      workspace.path("api", &format!("src/deep/{prefix}abc123"))
    ]
  );
  assert!(workspace.exists("api", &format!("node_modules/pkg/{prefix}ghi789")));
  assert!(workspace.exists("api", "src/.atlas-edit.0000ffff.abc") && workspace.exists("api", "src/.atlas-editor.ts"));
}

#[test]
fn should_rename_and_create_on_a_filesystem_without_hard_links() {
  let Some(mount) = std::env::var_os("ATLAS_EDIT_TEST_EXFAT_DIR") else {
    eprintln!("SKIPPED: set ATLAS_EDIT_TEST_EXFAT_DIR to a mounted exFAT volume");
    return;
  };
  let base = std::path::PathBuf::from(mount).join("atlas-edit-test");
  let _ = fs::remove_dir_all(&base);
  fs::create_dir_all(&base).unwrap();
  let workspace = Workspace::at(base.clone(), &["api"]);
  let a = workspace.write("api", "a.ts", "export const a = 1;\n");
  let b = workspace.write("api", "b.ts", "import { a } from './a';\n");
  workspace.write("api", "taken.ts", "taken");
  let plan = EditPlan {
    files: vec![FileEdits::new(target("api", "b.ts"), b, vec![TextEdit::replace(19..22, "./c")])],
    resources: vec![
      ResourceOp::Rename {
        from: target("api", "a.ts"),
        to: target("api", "c.ts"),
        expected: a,
      },
      ResourceOp::Create {
        file: target("api", "d.ts"),
        content: "d".to_owned(),
      },
    ],
  };
  apply(&plan, &workspace.scope(), &workspace.options()).unwrap();
  assert_eq!(
    (workspace.read("api", "b.ts"), workspace.read("api", "c.ts"), workspace.read("api", "d.ts")),
    ("import { a } from './c';\n".to_owned(), "export const a = 1;\n".to_owned(), "d".to_owned())
  );
  assert!(!workspace.exists("api", "a.ts"));
  let clobber = EditPlan {
    files: Vec::new(),
    resources: vec![ResourceOp::Rename {
      from: target("api", "c.ts"),
      to: target("api", "taken.ts"),
      expected: a,
    }],
  };
  assert!(matches!(apply(&clobber, &workspace.scope(), &workspace.options()), Err(EditError::AlreadyExists { .. })));
  assert_eq!(workspace.read("api", "taken.ts"), "taken");
  fs::remove_dir_all(&base).unwrap();
}
