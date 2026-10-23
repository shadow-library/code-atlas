mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use atlas_core::{ProjectName, RelPath, RepoName};
use atlas_engine::Engine;
use atlas_project::{AtlasHome, Project, ProjectSettings, RepoConfig};
use atlas_query::{
  AnchorStatus, ContextRequest, DocKind, DocResults, DocsAnswer, DocsRequest, FileNotesRequest, MentionView, NoteStamp, NoteState, NotesRequest, PathRequest, QueryEngine,
  RenderText, Response, SearchRequest, SymbolRequest, Targeted,
};

const REPO: &str = "shelf-web";
const RETRIES: &str = ".code-atlas/notes/retries.md";

fn shelf() -> common::Fixture {
  let scratch = tempfile::tempdir().unwrap();
  let root = scratch.path().join("tree").join(REPO);
  common::copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/docs-corpus").join(REPO), &root);
  let project = Project {
    name: ProjectName::new("shelf").unwrap(),
    settings: ProjectSettings::default(),
    repos: vec![RepoConfig {
      name: RepoName::new(REPO).unwrap(),
      path: root,
      include: Vec::new(),
      exclude: Vec::new(),
      languages: None,
    }],
  };
  let engine = Engine::open(&project, &AtlasHome::rooted_at(scratch.path().join("home")), common::engine_options()).unwrap();
  common::Fixture {
    scratch,
    project,
    query: QueryEngine::new(Arc::new(engine)),
  }
}

fn text<T: RenderText>(fixture: &common::Fixture, response: &Response<T>) -> String {
  fixture.text(response).replace(&fixture.repo_root(REPO).display().to_string(), "<repo>")
}

fn docs(fixture: &common::Fixture, request: DocsRequest) -> DocsAnswer {
  match fixture.query.docs(&request).unwrap().answer {
    Targeted::Found(answer) => answer,
    other => panic!("{request:?} did not resolve: {other:?}"),
  }
}

type Listed = (String, String, DocKind, Option<NoteState>, Option<f64>);

fn listing(answer: &DocsAnswer) -> Vec<Listed> {
  answer
    .sections
    .iter()
    .map(|section| (section.path.clone(), section.heading.clone(), section.kind, section.note_status, section.score))
    .collect()
}

fn note_states(fixture: &common::Fixture) -> Vec<(String, NoteState, Vec<AnchorStatus>)> {
  let notes = fixture.query.notes(&NotesRequest::default()).unwrap().answer.notes;
  notes
    .into_iter()
    .map(|note| (note.path, note.status, note.anchors.iter().map(|anchor| anchor.status).collect()))
    .collect()
}

fn stamp(fixture: &common::Fixture, note: &str) -> NoteStamp {
  let path = fixture.repo_root(REPO).join(note);
  let original = fs::read_to_string(&path).unwrap();
  let stamped = fixture
    .query
    .note_stamp(&RepoName::new(REPO).unwrap(), &RelPath::new(note).unwrap(), &original)
    .unwrap()
    .answer;
  if let NoteStamp::Edits { edits, .. } = &stamped {
    let mut text = original;
    for edit in edits.iter().rev() {
      text.replace_range(edit.start..edit.end, &edit.text);
    }
    fs::write(&path, text).unwrap();
  }
  stamped
}

#[test]
fn should_rank_doc_and_note_sections_for_a_query_and_resolve_their_mentions() {
  let fixture = shelf();

  let answer = docs(
    &fixture,
    DocsRequest {
      query: Some("retries backoff".to_owned()),
      ..DocsRequest::default()
    },
  );

  assert_eq!(
    listing(&answer),
    [
      ("README.md".to_owned(), "Shelf web > Retries".to_owned(), DocKind::Doc, None, Some(2.781)),
      (
        ".code-atlas/notes/retries.md".to_owned(),
        "Retry budget".to_owned(),
        DocKind::Note,
        Some(NoteState::Unstamped),
        Some(2.269)
      ),
      (
        ".code-atlas/notes/client.md".to_owned(),
        "Client construction".to_owned(),
        DocKind::Note,
        Some(NoteState::Unstamped),
        Some(0.763)
      ),
    ]
  );
  let rendered = text(
    &fixture,
    &fixture
      .query
      .docs(&DocsRequest {
        query: Some("retries backoff".to_owned()),
        limit: 1,
        ..DocsRequest::default()
      })
      .unwrap(),
  );
  assert_eq!(
    rendered,
    "fresh run=1 (+0 files, <t>)\ndocs matching \"retries backoff\" (1 of 3 sections)\n  shelf-web:README.md > Shelf web > Retries 5-9 [doc]\n    file: <repo>/README.md\n    `createShelfClient` builds the client; failed reads wait `backoffDelay()` before the next attempt. See [the backoff helper](src/lib/backoff.ts).\n    mentions `createShelfClient` -> createShelfClient function shelf-web:src/lib/shelf-client.ts:6 [heuristic 0.70]\n    mentions `backoffDelay` -> backoffDelay function shelf-web:src/lib/backoff.ts:3 [heuristic 0.70]\n    mentions `src/lib/backoff.ts` -> src/lib/backoff.ts file shelf-web:src/lib/backoff.ts [heuristic 0.70]\n"
  );
}

#[test]
fn should_flag_an_ambiguous_mention_at_040_and_a_name_that_no_longer_exists_as_drift() {
  let fixture = shelf();

  let answer = docs(
    &fixture,
    DocsRequest {
      query: Some("labels".to_owned()),
      ..DocsRequest::default()
    },
  );

  let [section] = answer.sections.as_slice() else { panic!("{answer:?}") };
  assert_eq!(section.heading, "Shelf web > Labels");
  let views: Vec<(String, Option<String>, usize)> = section
    .mentions
    .iter()
    .map(|mention| match mention {
      MentionView::Resolved { text, certainty, total, .. } => (text.clone(), Some(certainty.tag.clone()), *total),
      MentionView::Drift { text, .. } => (text.clone(), None, 0),
    })
    .collect();
  assert_eq!(
    views,
    [
      ("formatLabel".to_owned(), Some("[heuristic 0.40]".to_owned()), 2),
      ("ShelfFetcher.load".to_owned(), None, 0)
    ]
  );
}

#[test]
fn should_list_the_sections_that_mention_a_target_notes_first_and_skip_the_changelog() {
  let fixture = shelf();

  let symbol = docs(
    &fixture,
    DocsRequest {
      target: Some("backoffDelay".to_owned()),
      ..DocsRequest::default()
    },
  );
  let file = docs(
    &fixture,
    DocsRequest {
      target: Some("shelf-web:src/lib/shelf-client.ts".to_owned()),
      ..DocsRequest::default()
    },
  );
  let notes = docs(
    &fixture,
    DocsRequest {
      target: Some("backoffDelay".to_owned()),
      notes_only: true,
      ..DocsRequest::default()
    },
  );

  assert_eq!(
    listing(&symbol),
    [
      (
        ".code-atlas/notes/retries.md".to_owned(),
        "Retry budget".to_owned(),
        DocKind::Note,
        Some(NoteState::Unstamped),
        None
      ),
      ("README.md".to_owned(), "Shelf web > Retries".to_owned(), DocKind::Doc, None, None),
    ]
  );
  assert_eq!(
    listing(&file).into_iter().map(|(path, heading, ..)| format!("{path} > {heading}")).collect::<Vec<_>>(),
    [
      ".code-atlas/notes/client.md > Client construction",
      "README.md > Shelf web > Retries",
      "docs/design.md > Design > Paging"
    ]
  );
  assert_eq!(listing(&notes).len(), 1);
  assert!(matches!(
    fixture
      .query
      .docs(&DocsRequest {
        target: Some("noSuchHelper".to_owned()),
        ..DocsRequest::default()
      })
      .unwrap()
      .answer,
    Targeted::NotFound { .. }
  ));
}

#[test]
fn should_stamp_a_note_then_report_it_stale_after_the_code_changes_current_after_restamping_and_missing_after_removal() {
  let fixture = shelf();
  let backoff = fixture.repo_root(REPO).join("src/lib/backoff.ts");

  assert!(matches!(stamp(&fixture, ".code-atlas/notes/retries.md"), NoteStamp::Edits { ref missing, .. } if missing.is_empty()));
  assert!(matches!(stamp(&fixture, ".code-atlas/notes/client.md"), NoteStamp::Edits { .. }));
  assert_eq!(
    fs::read_to_string(fixture.repo_root(REPO).join(".code-atlas/notes/client.md")).unwrap(),
    "---\nanchors:\n  - symbol: shelf-web:src/lib/shelf-client.ts#createShelfClient\n    body: 22a9a167e338\n---\n# Client construction\n\nAlways build clients with `createShelfClient` so retries stay configured.\n"
  );
  assert_eq!(
    fs::read_to_string(fixture.repo_root(REPO).join(".code-atlas/notes/retries.md")).unwrap(),
    "---\nanchors:\n  - symbol: shelf-web:src/lib/backoff.ts#backoffDelay\n    body: bb6f402ed561\n---\n# Retry budget\n\n`backoffDelay` caps the wait at thirty seconds; keep the cap when changing the curve.\n"
  );
  assert_eq!(
    note_states(&fixture),
    [
      (".code-atlas/notes/client.md".to_owned(), NoteState::Current, vec![AnchorStatus::Current]),
      (".code-atlas/notes/retries.md".to_owned(), NoteState::Current, vec![AnchorStatus::Current]),
    ]
  );

  fs::write(&backoff, fs::read_to_string(&backoff).unwrap().replace("30000", "60000")).unwrap();
  assert_eq!(
    note_states(&fixture)[1],
    (".code-atlas/notes/retries.md".to_owned(), NoteState::Stale, vec![AnchorStatus::Stale])
  );
  let stale = fixture.query.notes(&NotesRequest { stale_only: true }).unwrap();
  assert_eq!(stale.answer.notes.len(), 1);

  assert!(matches!(stamp(&fixture, ".code-atlas/notes/retries.md"), NoteStamp::Edits { ref edits, .. } if edits.len() == 1));
  assert_eq!(note_states(&fixture)[1].1, NoteState::Current);
  assert!(matches!(stamp(&fixture, ".code-atlas/notes/retries.md"), NoteStamp::Edits { ref edits, .. } if edits.is_empty()));

  fs::write(&backoff, fs::read_to_string(&backoff).unwrap().replace("backoffDelay", "waitFor")).unwrap();
  assert_eq!(
    note_states(&fixture)[1],
    (".code-atlas/notes/retries.md".to_owned(), NoteState::Stale, vec![AnchorStatus::Missing])
  );
  assert!(matches!(stamp(&fixture, ".code-atlas/notes/retries.md"), NoteStamp::Edits { ref missing, .. } if missing == &["shelf-web:src/lib/backoff.ts#backoffDelay".to_owned()]));
}

#[test]
fn should_refuse_to_stamp_a_file_outside_the_notes_directory_or_a_note_that_names_no_code() {
  let fixture = shelf();
  let repo = RepoName::new(REPO).unwrap();

  let outside = fixture.query.note_stamp(&repo, &RelPath::new("README.md").unwrap(), "# x\n").unwrap().answer;
  let unnamed = fixture
    .query
    .note_stamp(&repo, &RelPath::new(".code-atlas/notes/x.md").unwrap(), "# Plans\nNothing here.\n")
    .unwrap()
    .answer;

  assert_eq!(
    outside,
    NoteStamp::Refused {
      reason: "shelf-web:README.md is not a note: notes live under .code-atlas/notes/; nothing was written".to_owned()
    }
  );
  assert_eq!(
    unnamed,
    NoteStamp::Refused {
      reason: "shelf-web:.code-atlas/notes/x.md names no code that resolves to exactly one symbol or file; name it in backticks; nothing was written".to_owned()
    }
  );
}

#[test]
fn should_point_to_notes_and_doc_sections_from_a_symbol_only_when_asked() {
  let fixture = shelf();
  let mut request = SymbolRequest::new("backoffDelay");

  let plain = text(&fixture, &fixture.query.symbol(&request).unwrap());
  request.docs = true;
  let pointed = text(&fixture, &fixture.query.symbol(&request).unwrap());
  let mut unmentioned = SymbolRequest::new("ShelfOptions");
  unmentioned.docs = true;
  let none = text(&fixture, &fixture.query.symbol(&unmentioned).unwrap());

  assert!(!plain.contains("NOTES") && !plain.contains("DOCS"), "{plain}");
  assert!(
    pointed.ends_with(
      "NOTES (1 of 1): notes anchored to it\n  shelf-web:.code-atlas/notes/retries.md > Retry budget [unstamped] on backoffDelay\nDOCS (1 of 1): doc sections that mention it\n  shelf-web:README.md:5 > Shelf web > Retries [heuristic 0.70]\n"
    ),
    "{pointed}"
  );
  assert!(!none.contains("NOTES") && !none.contains("DOCS"), "{none}");
}

#[test]
fn should_fuse_doc_sections_into_search_results_and_answer_only_docs_when_asked() {
  let fixture = shelf();
  let mut request = SearchRequest::new("backoff");
  request.docs = DocResults::Fused;

  let fused = fixture.query.search(&request).unwrap().answer;
  request.docs = DocResults::Only;
  let only = fixture.query.search(&request).unwrap().answer;
  request.docs = DocResults::Off;
  let off = fixture.query.search(&request).unwrap().answer;

  let ranked: Vec<(String, Option<usize>)> = fused.docs.iter().map(|section| (section.path.clone(), section.rank)).collect();
  assert_eq!(ranked, [("README.md".to_owned(), Some(2)), (".code-atlas/notes/retries.md".to_owned(), Some(4))]);
  assert_eq!(fused.strong.len() + fused.related.len(), 3);
  assert_eq!(only.strong.len() + only.related.len() + only.exact_name.len(), 0);
  assert_eq!(only.docs.iter().map(|section| section.rank).collect::<Vec<_>>(), [Some(1), Some(2)]);
  assert!(off.docs.is_empty());
}

#[test]
fn should_list_notes_anchored_to_a_must_read_item_as_skim_pointers() {
  let fixture = shelf();

  let pack = text(&fixture, &fixture.query.context(&ContextRequest::new("change the backoffDelay curve")).unwrap());

  assert!(
    pack.ends_with("SKIM (1)\n  note shelf-web:.code-atlas/notes/retries.md > Retry budget [unstamped] on backoffDelay\n"),
    "{pack}"
  );
}

#[test]
fn should_append_the_doc_map_to_the_overview_only_with_docs() {
  let fixture = shelf();

  let plain = text(&fixture, &fixture.query.overview().unwrap());
  let mapped = text(&fixture, &fixture.query.overview_with(true).unwrap());

  assert!(!plain.contains("\ndocs\n"), "{plain}");
  assert!(
    mapped.ends_with(
      "docs\n  shelf-web: 2 mapped docs, 2 notes (0 stale)\n    README.md\n      Shelf web\n      Shelf web > Retries\n      Shelf web > Labels\n    docs/design.md\n      Design\n      Design > Paging\n"
    ),
    "{mapped}"
  );
}

#[test]
fn should_find_the_cheapest_path_between_two_symbols_within_the_depth() {
  let fixture = shelf();

  let found = text(&fixture, &fixture.query.path(&PathRequest::new("loadWithBackoff", "ShelfClient")).unwrap());
  let mut shallow = PathRequest::new("loadWithBackoff", "ShelfClient");
  shallow.max_depth = 1;
  let none = fixture.query.path(&shallow).unwrap().answer;

  assert_eq!(
    found.split_once('\n').unwrap().1,
    "path between loadWithBackoff function shelf-web:src/lib/backoff.ts:7 and ShelfClient class shelf-web:src/lib/shelf-client.ts:10: 2 hops, cost 2\n  loadWithBackoff function shelf-web:src/lib/backoff.ts:7\n  calls createShelfClient function shelf-web:src/lib/shelf-client.ts:6 ([resolved])\n  instantiates ShelfClient class shelf-web:src/lib/shelf-client.ts:10 ([resolved])\n"
  );
  let Targeted::Found(none) = none else { panic!("{none:?}") };
  assert_eq!((none.cost, none.steps.len()), (None, 0));
  assert!(fixture.query.path(&PathRequest { max_depth: 13, ..shallow }).is_err());
}

#[test]
fn should_answer_the_notes_anchored_to_a_file_with_their_anchored_sections() {
  let fixture = shelf();
  let request = |file: &str| FileNotesRequest {
    file: file.to_owned(),
    require_fresh: true,
  };

  let Targeted::Found(backoff) = fixture.query.notes_for_file(&request("shelf-web:src/lib/backoff.ts")).unwrap().answer else {
    panic!("backoff.ts did not resolve")
  };
  let Targeted::Found(format) = fixture.query.notes_for_file(&request("shelf-web:src/ui/format.ts")).unwrap().answer else {
    panic!("format.ts did not resolve")
  };

  let [note] = backoff.notes.as_slice() else { panic!("{backoff:?}") };
  assert_eq!((note.note.path.as_str(), note.note.status), (".code-atlas/notes/retries.md", NoteState::Unstamped));
  assert_eq!(
    note.sections.iter().map(|section| (section.heading.as_str(), section.text.as_str())).collect::<Vec<_>>(),
    [("Retry budget", "`backoffDelay` caps the wait at thirty seconds; keep the cap when changing the curve.")]
  );
  assert!(format.notes.is_empty());
}

#[test]
fn should_prefer_a_call_chain_to_a_shortcut_through_the_file_that_holds_both_ends() {
  let fixture = common::single_repo(
    "chain",
    &[
      ("package.json", "{ \"name\": \"chain\", \"version\": \"1.0.0\" }\n"),
      (
        "src/steps.ts",
        "export function firstStep(): number {\n  return secondStep();\n}\n\nfunction secondStep(): number {\n  return thirdStep();\n}\n\nfunction thirdStep(): number {\n  return lastStep();\n}\n\nexport function lastStep(): number {\n  return 1;\n}\n",
      ),
    ],
  );

  let path = text(&fixture, &fixture.query.path(&PathRequest::new("firstStep", "lastStep")).unwrap());

  assert_eq!(
    path.split_once('\n').unwrap().1,
    "path between firstStep function chain:src/steps.ts:1 and lastStep function chain:src/steps.ts:13: 3 hops, cost 3\n  firstStep function chain:src/steps.ts:1\n  calls secondStep function chain:src/steps.ts:5 ([resolved])\n  calls thirdStep function chain:src/steps.ts:9 ([resolved])\n  calls lastStep function chain:src/steps.ts:13 ([resolved])\n"
  );
}

#[test]
fn should_fuse_only_sections_holding_every_query_term_or_one_in_their_heading_and_render_them_compactly() {
  let fixture = shelf();
  let search = |text: &str, docs: DocResults| {
    let mut request = SearchRequest::new(text);
    request.docs = docs;
    fixture.query.search(&request).unwrap()
  };

  let budget = search("backoff budget", DocResults::Fused);
  let unmatched = search("zebra backoff", DocResults::Fused);
  let only = search("zebra backoff", DocResults::Only);

  let paths = |answer: &atlas_query::SearchAnswer| answer.docs.iter().map(|section| section.path.clone()).collect::<Vec<_>>();
  assert_eq!(paths(&budget.answer), [RETRIES]);
  assert!(budget.answer.docs[0].mentions.is_empty());
  let rendered = text(&fixture, &budget);
  let section = rendered.split("docs and notes (1)\n").nth(1).unwrap();
  assert_eq!(
    section,
    "  <repo>/.code-atlas/notes/retries.md:5-7 > Retry budget [note unstamped]\n    `backoffDelay` caps the wait at thirty seconds; keep the cap when changing the curve.\n"
  );
  assert!(unmatched.answer.docs.is_empty());
  assert_eq!(paths(&only.answer), ["README.md", RETRIES]);
  assert!(!only.answer.docs[0].mentions.is_empty());
}

#[test]
fn should_list_at_most_three_notes_with_their_anchors_and_the_totals() {
  let fixture = shelf();
  let notes = fixture.repo_root(REPO).join(".code-atlas/notes");
  for index in 0..4 {
    fs::write(
      notes.join(format!("client-{index}.md")),
      format!("# Client {index}\n\n`ShelfOptions`, `createShelfClient`, `ShelfClient` and `ShelfClient.fetchShelf` work together.\n"),
    )
    .unwrap();
  }
  let mut request = atlas_query::ImpactRequest::new(["shelf-web:src/lib/shelf-client.ts"]);
  request.docs = true;

  let impact = text(&fixture, &fixture.query.impact(&request).unwrap());

  let notes_section: Vec<&str> = impact.lines().skip_while(|line| !line.starts_with("NOTES")).take(4).collect();
  assert_eq!(
    notes_section,
    [
      "NOTES (3 of 5): notes anchored to it",
      "  shelf-web:.code-atlas/notes/client-0.md > Client 0 [unstamped] on ShelfOptions, createShelfClient, ShelfClient (+1) [heuristic 0.70]",
      "  shelf-web:.code-atlas/notes/client-1.md > Client 1 [unstamped] on ShelfOptions, createShelfClient, ShelfClient (+1) [heuristic 0.70]",
      "  shelf-web:.code-atlas/notes/client-2.md > Client 2 [unstamped] on ShelfOptions, createShelfClient, ShelfClient (+1) [heuristic 0.70]",
    ]
  );
  assert!(impact.contains("DOCS (2 of 2): doc sections that mention it\n"), "{impact}");
}
