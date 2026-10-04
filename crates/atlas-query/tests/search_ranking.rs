mod common;

use std::sync::Arc;

use atlas_core::{ProjectName, RepoName};
use atlas_engine::Engine;
use atlas_project::{AtlasHome, Project, ProjectSettings, RepoConfig};
use atlas_query::{QueryEngine, SearchHitView, SearchRequest};

const PACKAGE: &str = "{\n  \"name\": \"ranking\",\n  \"private\": true\n}\n";
const SOURCE: &str = "export function parseMoney(raw: string): number {\n  return Number(raw);\n}\n\nexport function parse(raw: string): string {\n  return raw;\n}\n\nexport function formatMoney(value: number): string {\n  return String(value);\n}\n";

fn names(hits: &[SearchHitView]) -> Vec<&str> {
  hits.iter().map(|hit| hit.entity.short.as_str()).collect()
}

#[test]
fn should_not_treat_a_single_token_name_hit_as_an_exact_match_for_a_multi_word_query() {
  let fixture = common::single_repo("ranking", &[("package.json", PACKAGE), ("src/money.ts", SOURCE)]);

  let answer = fixture.query.search(&SearchRequest::new("parse money amount")).unwrap().answer;

  assert!(!names(&answer.exact_name).contains(&"parse"), "{answer:#?}");
  assert!(!names(&answer.exact_name).contains(&"formatMoney"), "{answer:#?}");
}

#[test]
fn should_list_a_name_that_covers_every_query_term_as_an_exact_match() {
  let fixture = common::single_repo("ranking", &[("package.json", PACKAGE), ("src/money.ts", SOURCE)]);

  let answer = fixture.query.search(&SearchRequest::new("parse money")).unwrap().answer;

  assert_eq!(names(&answer.exact_name), ["parseMoney"], "{answer:#?}");
}

#[test]
fn should_rank_hits_covering_more_query_terms_above_single_token_name_hits() {
  let fixture = common::single_repo("ranking", &[("package.json", PACKAGE), ("src/money.ts", SOURCE)]);

  let answer = fixture.query.search(&SearchRequest::new("parse money amount")).unwrap().answer;

  let ranked: Vec<&str> = names(&answer.exact_name).into_iter().chain(names(&answer.strong)).chain(names(&answer.related)).collect();
  let position = |name: &str| {
    ranked
      .iter()
      .position(|candidate| *candidate == name)
      .unwrap_or_else(|| panic!("{name} missing from {ranked:?}"))
  };
  assert!(position("parseMoney") < position("parse"), "{ranked:?}");
  assert!(position("parseMoney") < position("formatMoney"), "{ranked:?}");
}

#[test]
fn should_not_put_clean_variants_above_change_detection_code_in_the_workspace_index() {
  let scratch = tempfile::tempdir().unwrap();
  let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
  let repos = ["atlas-project", "atlas-engine"]
    .iter()
    .map(|name| RepoConfig {
      name: RepoName::new(*name).unwrap(),
      path: root.join(name),
      include: vec!["src/**".to_owned()],
      exclude: Vec::new(),
      languages: None,
    })
    .collect();
  let project = Project {
    name: ProjectName::new("workspace").unwrap(),
    settings: ProjectSettings::default(),
    repos,
  };
  let engine = Engine::open(&project, &AtlasHome::rooted_at(scratch.path().join("home")), common::engine_options()).unwrap();
  let query = QueryEngine::new(Arc::new(engine));

  let answer = query.search(&SearchRequest::new("detect racily clean files")).unwrap().answer;

  assert!(answer.exact_name.is_empty(), "{:?}", names(&answer.exact_name));
  let ranked: Vec<&str> = names(&answer.strong).into_iter().chain(names(&answer.related)).collect();
  let position = |name: &str| {
    ranked
      .iter()
      .position(|candidate| *candidate == name)
      .unwrap_or_else(|| panic!("{name} missing from {ranked:?}"))
  };
  assert!(position("detect_changes") < position("Plan::Clean"), "{ranked:?}");
  assert!(position("Tree::detect") < position("Plan::Clean"), "{ranked:?}");
}

const CENTS: &str = "export function toCents(value: string): number {\n  return Number(value) * 100;\n}\n\nexport function amountCents(value: number): number {\n  return value;\n}\n\nexport class UsersService {\n  findOne(id: string): string {\n    return id;\n  }\n}\n";

#[test]
fn should_require_exact_name_hits_to_stay_within_the_query_terms() {
  let fixture = common::single_repo("cents", &[("package.json", PACKAGE), ("src/cents.ts", CENTS)]);

  let answer = fixture.query.search(&SearchRequest::new("to cents")).unwrap().answer;

  assert_eq!(names(&answer.exact_name), ["toCents"], "{answer:#?}");
}

#[test]
fn should_match_the_qualified_name_as_an_exact_hit() {
  let fixture = common::single_repo("cents", &[("package.json", PACKAGE), ("src/cents.ts", CENTS)]);

  for query in ["UsersService.findOne", "UsersService findOne"] {
    let answer = fixture.query.search(&SearchRequest::new(query)).unwrap().answer;

    assert!(names(&answer.exact_name).contains(&"UsersService.findOne"), "{query}: {answer:#?}");
  }
}
