mod common;

use std::path::Path;
use std::sync::Arc;

use atlas_core::{ProjectName, RepoName};
use atlas_engine::{BarrierMode, Engine, EngineOptions};
use atlas_project::{AtlasHome, Project, ProjectSettings, RepoConfig};
use atlas_query::{Caveat, CounterpartSymbol, CounterpartsAnswer, CounterpartsRequest, FreshnessState, ImpactRequest, OnlyThere, QueryEngine, SharedValues, Targeted, render_json};

const REPOS: [&str; 2] = ["notes-web", "notes-server"];

fn notes() -> common::Fixture {
  let scratch = tempfile::tempdir().unwrap();
  let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/counterpart-corpus");
  let repos = REPOS
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
    name: ProjectName::new("notes").unwrap(),
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

fn counterparts(fixture: &common::Fixture, target: &str) -> CounterpartsAnswer {
  match fixture.query.counterparts(&CounterpartsRequest::new(target)).unwrap().answer {
    Targeted::Found(answer) => answer,
    other => panic!("{target} did not resolve: {other:?}"),
  }
}

fn only_there(line: u32, token: &str, text: &str) -> OnlyThere {
  OnlyThere {
    token: token.to_owned(),
    line,
    text: text.to_owned(),
  }
}

fn symbol(name: &str, lines: (u32, u32), shared_lines: usize) -> CounterpartSymbol {
  CounterpartSymbol {
    name: name.to_owned(),
    kind: "constant".to_owned(),
    start_line: lines.0,
    end_line: lines.1,
    shared_lines,
  }
}

#[test]
fn should_find_the_server_op_specs_as_the_counterpart_of_the_client_op_switch() {
  let fixture = notes();

  let answer = counterparts(&fixture, "notes-web:src/ops/op-switch.ts");

  let [counterpart] = answer.counterparts.as_slice() else {
    panic!("expected exactly one counterpart: {answer:?}");
  };
  assert_eq!((counterpart.repo.as_str(), counterpart.path.as_str()), ("notes-server", "src/ops/op-specs.ts"));
  assert_eq!(
    counterpart.absolute_path,
    fixture.repo_root("notes-server").join("src/ops/op-specs.ts").display().to_string()
  );
  assert_eq!(counterpart.score, 31.45);
  assert_eq!(counterpart.certainty.tag, "[heuristic 0.44]");
  assert!(!counterpart.certainty.authoritative);
  assert_eq!(
    counterpart.shared,
    SharedValues {
      count: 14,
      rarest: ["draft_slug", "revision_mark", "section_anchor", "page.archive_draft", "page.create_draft"]
        .map(str::to_owned)
        .to_vec(),
    }
  );
  assert_eq!(counterpart.symbols, [symbol("DECLARED_OP_SPECS", (6, 20), 12), symbol("WIRE_FIELDS", (22, 22), 1)]);
  assert_eq!(
    counterpart.only_there,
    [
      only_there(
        14,
        "page.merge_drafts",
        "'page.merge_drafts': { required: { source_slug: 'string', target_slug: 'string' }, optional: {} },"
      ),
      only_there(17, "tag", "'page.tag_note': { required: { draft_slug: 'string', tag: 'string' }, optional: {} },"),
      only_there(
        22,
        "merge_receipt",
        "export const WIRE_FIELDS = ['draft_slug', 'section_anchor', 'revision_mark', 'merge_receipt'];"
      ),
    ]
  );
  assert_eq!(answer.caveats, []);
}

#[test]
fn should_never_offer_a_file_of_the_same_package_and_rank_other_repos_by_shared_rare_values() {
  let fixture = notes();

  let answer = counterparts(&fixture, "notes-server:src/ops/op-specs.ts");

  let found: Vec<(&str, &str, f64, &str)> = answer
    .counterparts
    .iter()
    .map(|counterpart| (counterpart.repo.as_str(), counterpart.path.as_str(), counterpart.score, counterpart.certainty.tag.as_str()))
    .collect();
  assert_eq!(
    found,
    [
      ("notes-web", "src/ops/op-switch.ts", 31.45, "[heuristic 0.44]"),
      ("notes-web", "src/ops/op-labels.ts", 23.75, "[heuristic 0.37]")
    ]
  );
  assert!(answer.counterparts.iter().all(|counterpart| counterpart.repo != "notes-server"));
}

#[test]
fn should_find_none_for_common_values_too_few_rare_ones_or_a_test_file() {
  let fixture = notes();

  for target in ["notes-web:src/ui/format.ts", "notes-web:src/api/routes.ts", "notes-web:src/main.ts"] {
    let answer = counterparts(&fixture, target);
    assert_eq!((answer.counterparts.len(), answer.caveats.len()), (0, 0), "{target}");
  }
  let test = counterparts(&fixture, "notes-web:src/ops/op-switch.test.ts");
  assert_eq!(test.counterparts, []);
  assert_eq!(
    test.caveats,
    [Caveat::Note {
      message: "notes-web:src/ops/op-switch.test.ts is a test, generated, unreadable or oversized file, or not Rust or TypeScript: it has no counterparts".to_owned(),
    }]
  );
}

#[test]
fn should_answer_for_a_symbol_from_its_file() {
  let fixture = notes();

  let answer = counterparts(&fixture, "pinNote");

  assert_eq!(answer.file.path, "src/ops/op-switch.ts");
  assert_eq!(answer.counterparts.len(), 1);
}

#[test]
fn should_render_the_counterparts_as_text() {
  let fixture = notes();
  let root = fixture.repo_root("notes-server");

  let text = fixture.text(&fixture.query.counterparts(&CounterpartsRequest::new("notes-web:src/ops/op-switch.ts")).unwrap());

  assert_eq!(
    text.lines().skip(1).collect::<Vec<_>>().join("\n"),
    format!(
      "counterparts of notes-web:src/ops/op-switch.ts (1)
  notes-server:src/ops/op-specs.ts [heuristic 0.44] score=31.45
    file: {}
    shared values (14), rarest: 'draft_slug', 'revision_mark', 'section_anchor', 'page.archive_draft', 'page.create_draft'
    most shared in DECLARED_OP_SPECS 6-20, WIRE_FIELDS 22
    only on that side, next to shared values:
      14  'page.merge_drafts': {{ required: {{ source_slug: 'string', target_slug: 'string' }}, optional: {{}} }},
      17  'page.tag_note': {{ required: {{ draft_slug: 'string', tag: 'string' }}, optional: {{}} }},
      22  export const WIRE_FIELDS = ['draft_slug', 'section_anchor', 'revision_mark', 'merge_receipt'];",
      root.join("src/ops/op-specs.ts").display()
    )
  );
}

#[test]
fn should_add_counterparts_to_the_impact_of_a_file_only_when_asked() {
  let fixture = notes();
  let impact = |target: &str, counterparts: bool| {
    let request = ImpactRequest {
      counterparts,
      ..ImpactRequest::new([target])
    };
    let response = fixture.query.impact(&request).unwrap();
    let Targeted::Found(answer) = &response.answer else { panic!("{target}") };
    (answer.counterparts.len(), fixture.text(&response), render_json(&response).unwrap())
  };

  let (asked, text, json) = impact("notes-web:src/ops/op-switch.ts", true);
  let (plain, plain_text, plain_json) = impact("notes-web:src/ops/op-switch.ts", false);
  let (symbol, symbol_text, _) = impact("pinNote", true);

  assert_eq!((asked, plain, symbol), (1, 0, 0));
  assert!(
    text.contains("COUNTERPARTS (1): files in other repos or packages that share contract values with it\n  notes-server:src/ops/op-specs.ts [heuristic 0.44] score=31.45\n"),
    "{text}"
  );
  assert!(json.contains("\"counterparts\": ["), "{json}");
  assert!(!plain_text.contains("COUNTERPARTS") && !symbol_text.contains("COUNTERPARTS"), "{plain_text}\n{symbol_text}");
  assert!(!plain_json.contains("\"counterparts\""), "{plain_json}");
}

#[test]
fn should_rate_a_file_sharing_many_rare_values_with_a_higher_confidence() {
  let fixture = notes();

  let answer = counterparts(&fixture, "notes-web:src/sync/sync-jobs.ts");

  let [counterpart] = answer.counterparts.as_slice() else {
    panic!("expected exactly one counterpart: {answer:?}");
  };
  assert_eq!((counterpart.repo.as_str(), counterpart.path.as_str()), ("notes-server", "src/jobs/sync-registry.ts"));
  assert_eq!((counterpart.score, counterpart.certainty.tag.as_str()), (71.82, "[heuristic 0.64]"));
  assert_eq!(
    counterpart.shared,
    SharedValues {
      count: 28,
      rarest: ["conflict_hint", "outline_cursor", "replica_epoch", "shelf_token", "sync.chill_outline"]
        .map(str::to_owned)
        .to_vec(),
    }
  );
  assert_eq!(counterpart.symbols, [symbol("SYNC_FIELDS", (1, 28), 26)]);
  assert_eq!(
    counterpart.only_there,
    [
      only_there(6, "sync.hydrate_outline", "'sync.hydrate_outline': ['outline_cursor', 'hydration_window'],"),
      only_there(11, "sync.evict_shelf", "'sync.evict_shelf': ['shelf_token', 'eviction_reason'],"),
    ]
  );
}

#[test]
fn should_note_that_counterparts_were_not_computed_when_freshness_is_required_and_the_index_is_stale() {
  let fixture = notes();
  fixture.query.counterparts(&CounterpartsRequest::new("notes-web:src/sync/sync-jobs.ts")).unwrap();
  let engine = Engine::open(
    &fixture.project,
    &AtlasHome::rooted_at(fixture.scratch.path().join("home")),
    EngineOptions {
      barrier: BarrierMode::CheckOnly,
      ..common::engine_options()
    },
  )
  .unwrap();
  std::fs::write(fixture.repo_root("notes-web").join("src/ui/format.ts"), "export const CHANGED = 1;\n").unwrap();
  let checked = QueryEngine::new(Arc::new(engine));
  let mut request = CounterpartsRequest::new("notes-web:src/sync/sync-jobs.ts");
  request.require_fresh = true;

  let response = checked.counterparts(&request).unwrap();

  assert!(matches!(response.freshness.state, FreshnessState::Degraded { .. }), "{:?}", response.freshness);
  let Targeted::Found(answer) = response.answer else { panic!("{:?}", response.answer) };
  assert_eq!(answer.counterparts, []);
  assert_eq!(
    answer.caveats,
    [Caveat::Note {
      message: "the index is not fresh, so the counterparts were not computed".to_owned(),
    }]
  );
}
