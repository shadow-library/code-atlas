mod common;

use std::fs;

use atlas_core::{Certainty, Confidence, EntityId, EntityKey, Evidence, Producer, RelPath, RepoName, Span};
use atlas_engine::{EngineOptions, FaultInjection};
use atlas_query::{FreshnessState, SimilarRequest, SimilaritySignal, Targeted, TraverseRequest};
use atlas_store::NewInferredFact;

fn first_line(text: &str) -> &str {
  text.lines().next().unwrap()
}

fn append(path: &std::path::Path, text: &str) {
  let mut content = fs::read_to_string(path).unwrap();
  content.push_str(text);
  fs::write(path, content).unwrap();
}

#[test]
fn should_report_the_incremental_update_in_the_freshness_header_after_an_edit() {
  let fixture = common::acme();
  let before = fixture.query.callers(&TraverseRequest::new("UsersService.findOne")).unwrap();
  append(&fixture.repo_root("acme-api").join("src/users/users.service.ts"), "\nexport const touched = true;\n");

  let after = fixture.query.callers(&TraverseRequest::new("UsersService.findOne")).unwrap();

  assert_eq!(first_line(&fixture.text(&before)), "fresh run=1 (+58 files, <t>)");
  assert_eq!(first_line(&fixture.text(&after)), "fresh run=2 (+1 files, <t>)");
  let Targeted::Found(answer) = after.answer else { panic!("target must still resolve") };
  assert_eq!(answer.total, 4);
}

#[test]
fn should_note_skipped_files_in_the_freshness_header() {
  let fixture = common::acme();
  fs::write(fixture.repo_root("acme-api").join("src/huge.ts"), "x".repeat(1024 * 1024 + 1)).unwrap();

  let response = fixture.query.callers(&TraverseRequest::new("UsersService.findOne")).unwrap();

  assert_eq!(response.freshness.skipped_files, 1);
  assert!(
    first_line(&fixture.text(&response)).ends_with(" | skipped 1 files (see status)"),
    "{}",
    fixture.text(&response)
  );
  let clean = common::acme().query.callers(&TraverseRequest::new("UsersService.findOne")).unwrap();
  assert_eq!(clean.freshness.skipped_files, 0);
}

#[test]
fn should_mark_answers_degraded_and_name_the_failed_file_under_an_injected_failure() {
  let options = EngineOptions {
    fault_injection: FaultInjection::panic_when(|repo, path| repo.as_str() == "acme-api" && path.as_str() == "src/users/users.service.ts"),
    ..common::engine_options()
  };
  let fixture = common::fixture_with("acme", &common::ACME_REPOS, options);

  let response = fixture.query.callers(&TraverseRequest::new("UsersService.findOne")).unwrap();

  let FreshnessState::Degraded { failed_files, .. } = &response.freshness.state else {
    panic!("expected a degraded header: {:?}", response.freshness)
  };
  assert_eq!(failed_files.len(), 1);
  let text = fixture.text(&response);
  let mut lines = text.lines();
  let header = lines.next().unwrap();
  assert!(header.starts_with("DEGRADED: "), "{header}");
  assert!(header.ends_with(" run=1 (+58 files, <t>) | 1 files failed analysis"), "{header}");
  assert!(lines.next().unwrap().starts_with("  affected acme-api:src/users/users.service.ts: "), "{text}");
  assert!(
    !matches!(&response.answer, Targeted::Found(answer) if answer.target.kind == "method"),
    "facts of the failed file must be gone"
  );
}

fn inferred_equivalence(fixture: &common::Fixture) {
  let engine = fixture.query.engine();
  let to_cents = EntityId::of(&"ts:acme-sdk:@acme/sdk:src/money::toCents#function".parse::<EntityKey>().unwrap());
  let parse_money = EntityId::of(&"ts:acme-web:acme-web:src/utils/currency::parseMoney#function".parse::<EntityKey>().unwrap());
  let repo = RepoName::new("acme-web").unwrap();
  let path = RelPath::new("src/utils/currency.ts").unwrap();
  let hash = engine.store().read(|tx| tx.file(&repo, &path)).unwrap().unwrap().content_hash;
  let fact = NewInferredFact {
    subjects: vec![to_cents, parse_money],
    relation: "equivalent_to".to_owned(),
    payload: serde_json::json!({ "verdict": "equivalent", "rationale": "both parse a decimal string into integer cents", "behavioural_differences": [] }),
    certainty: Certainty::Inferred {
      confidence: Confidence::new(0.9).unwrap(),
      provider: "fake".to_owned(),
      model: "fake-model".to_owned(),
      rationale: "both parse a decimal string into integer cents".to_owned(),
    },
    evidence: vec![Evidence {
      repo,
      path,
      span: Span::from_zero_based_rows(0, 10, 0, 0, 0, 10),
      content_hash: hash,
      producer: Producer::new("test", 1),
      note: None,
    }],
    created_run: None,
    llm_call: None,
  };
  engine.store().write(|tx| tx.insert_inferred_fact(&fact)).unwrap();
}

fn parse_money(answer: &Targeted<atlas_query::SimilarAnswer>) -> &atlas_query::ClusterMember {
  let Targeted::Found(answer) = answer else { panic!("toCents must resolve") };
  answer.members.iter().find(|member| member.entity.short == "parseMoney").unwrap()
}

#[test]
fn should_hide_stale_inferred_facts_by_default_and_flag_them_when_requested() {
  let fixture = common::acme();
  fixture.query.similar(&SimilarRequest::new("toCents")).unwrap();
  inferred_equivalence(&fixture);

  let current = fixture.query.similar(&SimilarRequest::new("toCents")).unwrap();
  append(&fixture.repo_root("acme-web").join("src/utils/currency.ts"), "\nexport const touched = true;\n");
  let hidden = fixture.query.similar(&SimilarRequest::new("toCents")).unwrap();
  let shown = fixture
    .query
    .similar(&SimilarRequest {
      include_stale: true,
      ..SimilarRequest::new("toCents")
    })
    .unwrap();

  assert_eq!(
    parse_money(&current.answer).verdict,
    "inferred equivalent (0.90): both parse a decimal string into integer cents"
  );
  assert!(!parse_money(&hidden.answer).signals.iter().any(|signal| matches!(signal, SimilaritySignal::Inferred { .. })));
  assert!(fixture.text(&hidden).contains("inferred facts hidden: 1 stale (set include_stale=true)"));
  assert!(first_line(&fixture.text(&hidden)).ends_with("| 1 stale inferred facts (hidden unless include_stale=true)"));
  let stale = parse_money(&shown.answer);
  assert_eq!(
    stale.verdict,
    "inferred equivalent (0.90) [stale: acme-web:src/utils/currency.ts changed]: both parse a decimal string into integer cents"
  );
  assert!(fixture.text(&shown).contains("inferred [inferred 0.90 stale]"));
}
