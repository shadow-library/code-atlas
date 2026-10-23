mod common;

use std::path::Path;
use std::sync::Arc;

use atlas_core::{ProjectName, RepoName};
use atlas_engine::Engine;
use atlas_project::{AtlasHome, Project, ProjectSettings, RepoConfig};
use atlas_query::{AlsoChangeAnswer, AlsoChangeRequest, IndexState, QueryEngine, ReuseAnswer, ReuseReason, ReuseRequest, UnitStatus, render_text};

const BACKOFF_BODY: &str = "  const low = 250;
  const high = 30_000;
  if (typeof problem === 'object' && problem !== null && 'retryAfter' in problem) {
    const wanted = Number((problem as { retryAfter: unknown }).retryAfter);
    if (Number.isFinite(wanted) && wanted > 0) {
      return Math.min(wanted * 1000, high);
    }
  }
  const doubled = low * 2 ** Math.max(0, count - 1);
  return Math.min(doubled, high);
";

fn corpus(group: &str, repos: &[&str]) -> common::Fixture {
  let scratch = tempfile::tempdir().unwrap();
  let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join(group);
  let repos = repos
    .iter()
    .map(|name| {
      let root = scratch.path().join("tree").join(name);
      common::copy_dir(&corpus.join(name), &root);
      RepoConfig {
        name: RepoName::new(*name).unwrap(),
        path: root,
        include: Vec::new(),
        exclude: Vec::new(),
        languages: None,
      }
    })
    .collect();
  let project = Project {
    name: ProjectName::new("shelf").unwrap(),
    settings: ProjectSettings::default(),
    repos,
  };
  let engine = Engine::open(&project, &AtlasHome::rooted_at(scratch.path().join("home")), common::engine_options()).unwrap();
  common::Fixture {
    scratch,
    project,
    query: QueryEngine::new(Arc::new(engine)),
  }
}

fn shelf() -> common::Fixture {
  corpus("reuse-corpus", &["shelf-app"])
}

fn check(fixture: &common::Fixture, file: &str, text: Option<String>, lines: Option<(u32, u32)>) -> ReuseAnswer {
  let request = ReuseRequest {
    repo_path: file.to_owned(),
    text,
    lines,
  };
  fixture.query.reuse_check(&request).unwrap().answer
}

fn backoff_copy(name: &str) -> String {
  format!("export function {name}(problem: unknown, count: number): number {{\n{BACKOFF_BODY}}}\n")
}

fn found(answer: &ReuseAnswer) -> Vec<(String, String, String, String)> {
  answer
    .units
    .iter()
    .flat_map(|unit| {
      unit
        .candidates
        .iter()
        .map(|candidate| (unit.name.clone(), candidate.entity.short.clone(), candidate.certainty.tag.clone(), candidate.reason.text()))
    })
    .collect()
}

#[test]
fn should_split_new_text_into_function_units_of_at_least_twelve_tokens() {
  let fixture = shelf();
  let text = format!("{}\nexport function tiny(): number {{\n  return 1;\n}}\n", backoff_copy("pauseAfterFailure"));

  let answer = check(&fixture, "shelf-app:src/ui/retry-badge.ts", Some(text), None);

  let units: Vec<(&str, UnitStatus, u32, u32)> = answer.units.iter().map(|unit| (unit.name.as_str(), unit.status, unit.start_line, unit.end_line)).collect();
  assert_eq!(units, [("pauseAfterFailure", UnitStatus::New, 1, 12)]);
  assert_eq!(answer.index_state, IndexState::NotIndexed);
}

#[test]
fn should_find_an_existing_near_duplicate_of_new_code_capped_at_two_per_function() {
  let fixture = shelf();

  let answer = check(&fixture, "shelf-app:src/ui/retry-badge.ts", Some(backoff_copy("pauseAfterFailure")), None);

  let near = |candidate: &str| {
    (
      "pauseAfterFailure".to_owned(),
      candidate.to_owned(),
      "[heuristic 1.00]".to_owned(),
      "near duplicate 1.00".to_owned(),
    )
  };
  assert_eq!(found(&answer), [near("waitBeforeRetry"), near("requeueDelay")]);
  let candidate = &answer.units[0].candidates[0];
  assert_eq!(candidate.reason, ReuseReason::NearDuplicate { jaccard: 1.0 });
  assert!(!candidate.certainty.authoritative);
}

#[test]
fn should_name_a_helper_of_the_same_package_whose_name_shares_two_parts() {
  let fixture = shelf();
  let text = "export function retryDelayFromResponse(response: Response): number | undefined {\n  const value = response.headers.get('Retry-After');\n  return value === null ? undefined : Number(value) * 1000;\n}\n";

  let answer = check(&fixture, "shelf-app:src/ui/retry-badge.ts", Some(text.to_owned()), None);

  assert_eq!(
    found(&answer),
    [(
      "retryDelayFromResponse".to_owned(),
      "retryDelayFromHeader".to_owned(),
      "[heuristic 0.50]".to_owned(),
      "same name parts: delay, retry".to_owned()
    )]
  );
  let candidate = &answer.units[0].candidates[0];
  assert_eq!((candidate.start_line, candidate.end_line), (14, 20));
  assert_eq!(
    candidate.signature.as_deref(),
    Some("function retryDelayFromHeader(header: string | undefined): number | undefined")
  );
}

#[test]
fn should_not_offer_a_helper_the_new_code_already_calls() {
  let fixture = shelf();
  let text = "import { retryDelayFromHeader } from '../lib/backoff';\n\nexport function retryDelayFromResponse(response: Response): number | undefined {\n  const value = response.headers.get('Retry-After');\n  return retryDelayFromHeader(value ?? undefined);\n}\n";

  let answer = check(&fixture, "shelf-app:src/ui/retry-badge.ts", Some(text.to_owned()), None);

  assert_eq!(answer.units.len(), 1);
  assert_eq!(found(&answer), []);
}

#[test]
fn should_exclude_the_symbols_of_the_same_file_that_the_checked_lines_overlap() {
  let fixture = shelf();

  let answer = check(&fixture, "shelf-app:src/lib/backoff.ts", None, Some((1, 12)));

  assert_eq!(answer.index_state, IndexState::Current);
  assert_eq!(answer.units.len(), 1);
  assert_eq!(answer.units[0].status, UnitStatus::OnDisk);
  let names: Vec<String> = found(&answer).into_iter().map(|(_, candidate, _, _)| candidate).collect();
  assert_eq!(names.len(), 2);
  assert!(!names.contains(&"waitBeforeRetry".to_owned()), "{names:?}");
}

#[test]
fn should_report_only_matches_a_changed_function_did_not_have_before() {
  let fixture = shelf();
  let file = fixture.repo_root("shelf-app").join("src/lib/backoff.ts");
  let before = std::fs::read_to_string(&file).unwrap();
  let after = before.replace("  const ceiling = 30_000;\n", "  const ceiling = 30_000;\n  const floor = 0;\n");

  let changed = check(&fixture, "shelf-app:src/lib/backoff.ts", Some(after), None);
  let same = check(&fixture, "shelf-app:src/lib/backoff.ts", Some(before), None);

  let units: Vec<(&str, UnitStatus)> = changed.units.iter().map(|unit| (unit.name.as_str(), unit.status)).collect();
  assert_eq!(units, [("waitBeforeRetry", UnitStatus::Changed)]);
  assert_eq!(found(&changed), []);
  assert!(same.units.is_empty());
  assert!(
    render_text(
      &fixture
        .query
        .reuse_check(&ReuseRequest {
          repo_path: "shelf-app:src/lib/backoff.ts".to_owned(),
          text: None,
          lines: Some((14, 20))
        })
        .unwrap()
    )
    .contains("(1 function checked, 0 candidates)")
  );
}

#[test]
fn should_cap_the_answer_at_three_candidates() {
  let fixture = shelf();
  let text = format!("{}\n{}", backoff_copy("pauseAfterFailure"), backoff_copy("holdAfterError"));

  let answer = check(&fixture, "shelf-app:src/ui/retry-badge.ts", Some(text), None);

  let counts: Vec<usize> = answer.units.iter().map(|unit| unit.candidates.len()).collect();
  assert_eq!(counts, [2, 1]);
  assert_eq!(answer.candidates(), 3);
}

fn also_change(fixture: &common::Fixture, file: &str) -> AlsoChangeAnswer {
  fixture.query.also_change(&AlsoChangeRequest::new([file])).unwrap().answer
}

fn listed(answer: &AlsoChangeAnswer) -> Vec<(String, f64, String, String)> {
  answer
    .files
    .iter()
    .map(|file| (format!("{}:{}", file.repo, file.path), file.score, file.reason.clone(), file.certainty.tag.clone()))
    .collect()
}

#[test]
fn should_rank_the_call_chain_above_unrelated_files_and_leave_out_tests_and_generated_files() {
  let fixture = shelf();

  let answer = also_change(&fixture, "shelf-app:src/api/client.ts");

  assert_eq!(
    listed(&answer),
    [
      (
        "shelf-app:src/state/shelf-store.ts".to_owned(),
        1.0,
        "calls changed function fetchShelf".to_owned(),
        "[resolved]".to_owned()
      ),
      (
        "shelf-app:src/lib/backoff.ts".to_owned(),
        0.36,
        "called by changed function fetchShelf".to_owned(),
        "[resolved]".to_owned()
      ),
      (
        "shelf-app:src/ui/shelf-view.ts".to_owned(),
        0.27,
        "via shelf-app:src/state/shelf-store.ts, 2 hops from a changed file".to_owned(),
        "[resolved]".to_owned()
      ),
    ]
  );
  assert_eq!(answer.caveats, []);
}

#[test]
fn should_boost_a_counterpart_of_the_changed_file_above_its_graph_neighbours() {
  let fixture = corpus("reuse-corpus", &["shelf-app", "shelf-server"]);

  let answer = also_change(&fixture, "shelf-app:src/sync/shelf-sync.ts");

  let row = |file: &str, reason: &str, tag: &str| (file.to_owned(), 1.0, reason.to_owned(), tag.to_owned());
  assert_eq!(
    listed(&answer),
    [
      row("shelf-server:src/sync/sync-registry.ts", "same contract values", "[heuristic 0.65]"),
      row("shelf-app:src/lib/legacy-backoff.ts", "called by changed function describeSync", "[resolved]"),
      row("shelf-app:src/ui/count-label.ts", "called by changed function describeSync", "[resolved]"),
    ]
  );
}

#[test]
fn should_seed_nothing_from_a_file_that_is_not_indexed() {
  let fixture = shelf();

  let answer = also_change(&fixture, "shelf-app:src/ui/missing.ts");

  assert!(answer.files.is_empty());
  assert_eq!(
    render_text(&fixture.query.also_change(&AlsoChangeRequest::new(["shelf-app:src/ui/missing.ts"])).unwrap())
      .lines()
      .skip(1)
      .collect::<Vec<_>>(),
    [
      "also change with shelf-app:src/ui/missing.ts (0)",
      "  none: no other file is linked to these in the graph",
      "caveats:",
      "  shelf-app:src/ui/missing.ts is not indexed, so it seeds nothing",
    ]
  );
}
