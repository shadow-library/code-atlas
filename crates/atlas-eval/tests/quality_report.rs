use std::path::{Path, PathBuf};
use std::sync::Arc;

use atlas_eval::quality::{
  ContextReport, Corpus, FixtureGroup, QualityArgs, QualityReport, RenameDocument, RenameOutcome, RenameReport, RepoSource, compare_reports, discover, evaluate_fixture, execute,
  load_truth, read_report, run_quality, run_rename_with, write_rename_report,
};
use atlas_ide::{IdeError, IdeResult, IdeSettings, LaunchRequest, LaunchSpec, Launcher};

fn repo_root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn args(out: Option<PathBuf>, compare: Option<PathBuf>) -> QualityArgs {
  QualityArgs {
    repo_root: Some(repo_root()),
    out,
    compare,
    date: Some("2026-01-01".to_owned()),
    only: vec!["adversarial".to_owned()],
    no_scip: true,
    scip_work: None,
    scip_typescript: false,
    scip_workspace: false,
    scip_only: Vec::new(),
    scip_timeout_secs: 1,
    no_refactors: true,
    no_rename: true,
    rename_only: false,
    ts_lsp: None,
    context_tasks: None,
  }
}

fn adversarial_report() -> QualityReport {
  run_quality(&args(None, None), &repo_root(), "2026-01-01").unwrap()
}

#[test]
fn should_report_every_adversarial_fixture_with_the_documented_structure() {
  let report = adversarial_report();

  let names: Vec<&str> = report.fixtures.iter().map(|fixture| fixture.name.as_str()).collect();
  assert_eq!(
    names,
    [
      "adversarial/broken-syntax",
      "adversarial/computed-routes-ts",
      "adversarial/duplicate-names-rust",
      "adversarial/dynamic-ts",
      "adversarial/generated-code",
      "adversarial/macro-heavy-rust",
      "adversarial/reexport-chain-ts",
      "adversarial/shadowing-ts",
      "adversarial/similar-not-equivalent",
      "adversarial/trait-dispatch-rust",
      "adversarial/ts-overrides",
    ]
  );
  assert_eq!(report.schema_version, 1);
  assert!(
    report
      .fixtures
      .iter()
      .all(|fixture| fixture.files > 0 && fixture.entities_indexed > 0 && fixture.entities.total.expected > 0)
  );
  assert!(report.fixtures.iter().all(|fixture| fixture.entities.total.expected
    == fixture.entities.total.found
      + fixture.entities.total.wrong_certainty
      + fixture.entities.total.wrong_detail
      + fixture.entities.total.missing
      + fixture.entities.total.unbound));
  assert_eq!(
    report.totals.entities.total.expected,
    report.fixtures.iter().map(|fixture| fixture.entities.total.expected).sum::<u64>()
  );
  assert!(report.totals.edge_kinds.contains_key("calls"));
  assert!(report.metrics.contains_key("total/entities/recall"));
  assert!(report.metrics.contains_key("fixture/adversarial/trait-dispatch-rust/edge/calls/recall"));
  assert!(report.oracle.targets.is_empty());
  assert!(report.refactors.scenarios.is_empty());
  assert!(matches!(report.context, ContextReport::NotRequested { .. }));
  assert_eq!(report.rename, RenameReport::NotRequested { reason: "--no-rename".to_owned() });
  assert!(report.metrics.keys().all(|name| !name.starts_with("rename/")));
  assert!(
    report
      .calibration
      .by_class
      .values()
      .all(|cell| cell.judged == cell.correct + cell.overclaimed + cell.underclaimed + cell.false_edges)
  );
}

struct NoServer;

impl Launcher for NoServer {
  fn launch(&self, request: &LaunchRequest<'_>) -> IdeResult<LaunchSpec> {
    Err(IdeError::Unavailable {
      server: request.kind.name().to_owned(),
      reason: "not installed".to_owned(),
      alternative: None,
    })
  }
}

#[test]
fn should_skip_and_write_a_standalone_rename_report_when_no_server_is_found() {
  let scratch = tempfile::tempdir().unwrap();
  let groups = discover(&repo_root().join("fixtures"), &["trait-dispatch-rust".to_owned()]).unwrap();

  let rename = run_rename_with(&groups, Arc::new(NoServer), IdeSettings::default()).unwrap();
  let document = RenameDocument {
    date: "2026-01-01".to_owned(),
    rename,
  };
  let (markdown, json) = write_rename_report(&document, &scratch.path().join("rename")).unwrap();

  let reread: RenameDocument = serde_json::from_str(&std::fs::read_to_string(json).unwrap()).unwrap();
  assert_eq!(reread, document);
  let RenameReport::Run { scenarios, aggregate, .. } = reread.rename else {
    panic!("expected a rename run");
  };
  assert_eq!(scenarios.len(), 1);
  assert!(matches!(&scenarios[0].outcome, RenameOutcome::Skipped { reason } if reason.starts_with("no language server: ")));
  assert_eq!((aggregate.skipped, aggregate.edit.expected), (1, 0));
  let text = std::fs::read_to_string(markdown).unwrap();
  assert!(text.starts_with("# Code Atlas rename evaluation\n\nDate: 2026-01-01.\n"), "{text}");
}

#[test]
fn should_score_store_get_with_the_real_rust_analyzer_when_asked_to() {
  if std::env::var_os("ATLAS_TEST_RENAME_REAL").is_none() {
    eprintln!("SKIPPED should_score_store_get_with_the_real_rust_analyzer_when_asked_to: set ATLAS_TEST_RENAME_REAL=1");
    return;
  }
  let scratch = tempfile::tempdir().unwrap();
  let mut options = args(Some(scratch.path().join("rename")), None);
  options.rename_only = true;
  options.no_rename = false;
  options.only = vec!["trait-dispatch-rust".to_owned()];

  let outcome = execute(&options).unwrap();

  let document: RenameDocument = serde_json::from_str(&std::fs::read_to_string(&outcome.json).unwrap()).unwrap();
  let RenameReport::Run { scenarios, .. } = document.rename else {
    panic!("expected a rename run");
  };
  match &scenarios[0].outcome {
    RenameOutcome::Scored { score, .. } => assert_eq!((score.edit.correct, score.edit.expected, score.edit.predicted), (7, 7, 7)),
    other => panic!("store-get was not scored: {other:?}"),
  }
}

#[test]
fn should_refuse_rename_only_together_with_no_rename() {
  let mut options = args(None, None);
  options.rename_only = true;

  assert!(execute(&options).unwrap_err().to_string().contains("--rename-only conflicts with --no-rename"));
}

#[test]
fn should_produce_an_identical_report_on_every_run() {
  let first = serde_json::to_string(&adversarial_report()).unwrap();
  let second = serde_json::to_string(&adversarial_report()).unwrap();

  assert_eq!(first, second);
}

#[test]
fn should_write_markdown_and_json_and_compare_a_report_with_itself() {
  let scratch = tempfile::tempdir().unwrap();
  let base = scratch.path().join("reports").join("quality");

  let first = execute(&args(Some(base.clone()), None)).unwrap();
  let markdown = std::fs::read_to_string(&first.markdown).unwrap();
  let again = execute(&args(Some(scratch.path().join("again").join("quality.md")), Some(first.json.clone()))).unwrap();

  for heading in [
    "# Code Atlas knowledge-quality report",
    "## Headline per fixture",
    "## Edge kinds (all fixtures)",
    "## Certainty calibration",
    "## Ground-truth binding errors",
    "## Refactor robustness",
  ] {
    assert!(markdown.contains(heading), "missing {heading}");
  }
  let reread = read_report(&first.json).unwrap();
  assert_eq!(compare_reports(&reread, &reread).regressed.len(), 0);
  assert_eq!(again.regressions, 0);
  assert!(again.comparison.unwrap().contains("0 regressed, 0 improved"));
  assert!(std::fs::read_to_string(&again.markdown).unwrap().contains("## Comparison with the previous report"));
}

#[test]
fn should_run_every_refactor_scenario_and_record_its_checks() {
  let report = atlas_eval::quality::run_refactors(&repo_root().join("fixtures").join("acme")).unwrap();

  let names: Vec<&str> = report.scenarios.iter().map(|scenario| scenario.name.as_str()).collect();
  assert_eq!(
    names,
    [
      "rename_symbol_and_callers",
      "move_file_and_fix_imports",
      "extract_function",
      "delete_function_and_its_calls",
      "change_route_literal",
      "swap_guard"
    ]
  );
  let again = atlas_eval::quality::run_refactors(&repo_root().join("fixtures").join("acme")).unwrap();
  assert_eq!(serde_json::to_string(&report).unwrap(), serde_json::to_string(&again).unwrap());
  for scenario in &report.scenarios {
    assert!(scenario.error.is_none(), "{}: {:?}", scenario.name, scenario.error);
    for required in [
      "incremental index equals a full index (digest)",
      "no stale deterministic fact survives",
      "inferred facts on changed evidence are stale",
    ] {
      assert!(scenario.checks.iter().any(|check| check.name == required), "{} lacks `{required}`", scenario.name);
    }
    assert_eq!(scenario.passed, scenario.checks.iter().all(|check| check.passed));
  }
}

fn copy_dir(from: &Path, to: &Path) {
  std::fs::create_dir_all(to).unwrap();
  for entry in std::fs::read_dir(from).unwrap() {
    let entry = entry.unwrap();
    let target = to.join(entry.file_name());
    match entry.file_type().unwrap().is_dir() {
      true => copy_dir(&entry.path(), &target),
      false => drop(std::fs::copy(entry.path(), target).unwrap()),
    }
  }
}

#[test]
fn should_detect_a_missing_entity_and_a_forbidden_edge_in_a_mutated_fixture() {
  let scratch = tempfile::tempdir().unwrap();
  let fixtures = scratch.path().join("fixtures");
  copy_dir(
    &repo_root().join("fixtures").join("adversarial").join("shadowing-ts"),
    &fixtures.join("adversarial").join("shadowing-ts"),
  );
  let render = fixtures.join("adversarial/shadowing-ts/src/render.ts");
  let source = std::fs::read_to_string(&render).unwrap();
  let mutated = source
    .replace("  const format = (value: string) => value.toUpperCase();\n", "")
    .replace("renderParam(", "renderParameter(");
  std::fs::write(&render, mutated).unwrap();
  let groups = vec![FixtureGroup {
    name: "adversarial/shadowing-ts".to_owned(),
    truth: load_truth(&repo_root().join("fixtures/adversarial/shadowing-ts/expected.toml")).unwrap(),
    repos: vec![RepoSource {
      name: "shadowing-ts".to_owned(),
      dir: fixtures.join("adversarial/shadowing-ts"),
    }],
  }];
  let corpus = Corpus::index(groups).unwrap();
  let group = &corpus.groups[0];

  let report = evaluate_fixture(&group.name, &group.truth, corpus.project(&group.truth.project.name).unwrap(), &corpus).unwrap();

  assert_eq!(report.entities.total.missing, 1);
  assert!(report.forbidden.violations >= 1, "{:?}", report.forbidden);
  assert!(report.findings.iter().any(|finding| finding.section == "forbidden_edge"));
  assert!(report.findings.iter().any(|finding| finding.section == "entity" && finding.label.contains("renderParam")));
}

#[test]
fn should_score_context_selection_for_fixture_tasks_and_skip_unsupported_ones() {
  let scratch = tempfile::tempdir().unwrap();
  let tasks = scratch.path().join("tasks");
  std::fs::create_dir_all(tasks.join("t01-user-field")).unwrap();
  std::fs::create_dir_all(tasks.join("t02-own-repo")).unwrap();
  std::fs::write(
    tasks.join("t01-user-field").join("task.toml"),
    r#"id = "t01-user-field"
title = "x"
repo = { fixture = "acme" }
prompt = "Add a deletedAt column to the User entity and return it from UsersService.findOne"
[ground_truth]
relevant_files = ["acme-api/src/users/user.entity.ts", "acme-api/src/users/users.service.ts"]
relevant_symbols = ["UsersService.findOne"]
"#,
  )
  .unwrap();
  std::fs::write(
    tasks.join("t02-own-repo").join("task.toml"),
    "id = \"t02-own-repo\"\ntitle = \"y\"\nrepo = \"self\"\nprompt = \"p\"\n[ground_truth]\nrelevant_files = [\"crates/x.rs\"]\n",
  )
  .unwrap();
  let mut request = args(None, None);
  request.only = vec!["acme".to_owned()];
  request.context_tasks = Some(tasks);

  let report = run_quality(&request, &repo_root(), "2026-01-01").unwrap();

  let ContextReport::Scored { tasks, skipped, aggregate } = report.context else {
    panic!("context tasks were given")
  };
  assert_eq!(tasks.len(), 1);
  assert_eq!(tasks[0].id, "t01-user-field");
  let combined = &tasks[0].combined;
  assert_eq!(combined.files.relevant, 2);
  assert_eq!(combined.symbols.relevant, 1);
  assert!(combined.files.hits <= combined.files.retrieved.min(2));
  assert_eq!(aggregate.combined_files.tasks, 1);
  assert_eq!(skipped.len(), 1);
  assert_eq!(skipped[0].id, "t02-own-repo");
  assert!(report.metrics.contains_key("context/combined/files/recall"));
}
