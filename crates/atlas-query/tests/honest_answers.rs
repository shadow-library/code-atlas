mod common;

use std::fs;
use std::sync::Arc;

use atlas_core::{Certainty, Confidence, EntityId, EntityKey, ProjectName, RepoName};
use atlas_engine::Engine;
use atlas_project::{AtlasHome, Project, ProjectSettings, RepoConfig};
use atlas_query::{
  Caveat, ContextAnswer, ContextRequest, OmissionReason, QueryEngine, RenderText, SearchAnswer, SearchRequest, SearchRestriction, SymbolRequest, TargetFilter, Targeted, TextOut,
  render_json,
};
use atlas_semantic::RELATION_CAPABILITY_SUMMARY;
use atlas_store::NewInferredFact;

const TASK: &str = "add a DELETE /orders/:id endpoint with api-key auth";

const LABEL: &str = "export function formatLabel(input: string): string {
  let value = input;
  value = value.trim();
  value = value.replace(/\\s+/g, ' ');
  value = value.slice(0, 80);
  value = value.padEnd(4, '.');
  value = value.normalize();
  value = value.replace(/^-/, '');
  value = value.replace(/-$/, '');
  value = value.replace(/_/g, ' ');
  value = value.toUpperCase();
  value = value.concat('');
  value = value.substring(0);
  value = value.trimEnd();
  value = value.trimStart();
  value = value.at(0) === ' ' ? value.slice(1) : value;
  return value;
}
";

const PRICE_RULES: &str = "export function applyPriceRules(cents: number): number {
  return Math.round(cents * 0.9);
}
";

const SHOP_CART: &str = "import { applyPriceRules } from './price-rules';

export function cartTotal(items: number[]): number {
  return applyPriceRules(items.reduce((sum, item) => sum + item, 0));
}
";

const OFFICE_CART: &str = "export function cartTotalReport(items: number[]): string {
  return `cart total ${items.length}`;
}
";

fn shop_tree() -> common::Fixture {
  common::single_repo(
    "tree",
    &[
      ("package.json", r#"{ "name": "tree" }"#),
      ("apps/web-shop/src/price-rules.ts", PRICE_RULES),
      ("apps/web-shop/src/cart-total.ts", SHOP_CART),
      ("apps/back-office/src/cart-total.ts", OFFICE_CART),
    ],
  )
}

fn two_shops() -> common::Fixture {
  let scratch = tempfile::tempdir().unwrap();
  let repos = ["web-shop", "back-office"]
    .iter()
    .map(|name| {
      let root = scratch.path().join("tree").join(name);
      fs::create_dir_all(root.join("src")).unwrap();
      fs::write(root.join("package.json"), format!(r#"{{ "name": "{name}" }}"#)).unwrap();
      fs::write(root.join("src/price-rules.ts"), PRICE_RULES).unwrap();
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
    name: ProjectName::new("shops").unwrap(),
    settings: ProjectSettings::default(),
    repos,
  };
  let engine = Engine::open(&project, &AtlasHome::rooted_at(scratch.path().join("home")), common::engine_options()).unwrap();
  let query = QueryEngine::new(Arc::new(engine));
  common::Fixture { scratch, project, query }
}

fn pack_text(answer: &ContextAnswer) -> String {
  let mut out = TextOut::default();
  answer.render_text(&mut out);
  out.finish()
}

fn omitted_block(text: &str) -> Vec<String> {
  let mut lines = text.lines().skip_while(|line| !line.starts_with("OMITTED"));
  let Some(title) = lines.next() else { return Vec::new() };
  std::iter::once(title).chain(lines.take_while(|line| line.starts_with("  "))).map(str::to_owned).collect()
}

fn over_budget(answer: &ContextAnswer) -> bool {
  answer
    .caveats
    .iter()
    .any(|caveat| matches!(caveat, Caveat::Note { message } if message.starts_with("mandatory mentions exceed the budget")))
}

fn assert_honest(answer: &ContextAnswer) -> usize {
  let chars = pack_text(answer).chars().count();
  assert_eq!(answer.estimated_tokens, chars.div_ceil(4));
  assert!(
    over_budget(answer) || chars as f64 <= answer.budget as f64 * 4.0 * 1.05,
    "{chars} chars for {} tokens",
    answer.budget
  );
  chars
}

#[test]
fn should_render_search_hits_without_ranking_diagnostics_and_keep_them_in_json() {
  let fixture = common::acme();

  let response = fixture.query.search(&SearchRequest::new("convert currency string to cents")).unwrap();
  let text = fixture.text(&response);
  let json = render_json(&response).unwrap();

  let hit = text.lines().position(|line| line.starts_with("  toCents function acme-sdk:src/money.ts:1 score=")).unwrap();
  assert_eq!(text.lines().nth(hit + 1), Some("    sig function toCents(amount: string): number"));
  assert_eq!(text.lines().filter(|line| line.trim_start().starts_with("matched ")).count(), 0);
  assert!(!text.contains("expanded "), "{text}");
  assert!(json.contains("\"matched_fields\": [\n"), "{json}");
  assert!(json.contains("\"name: cents\""), "{json}");
}

#[test]
fn should_count_omitted_items_per_reason_on_one_line() {
  let fixture = common::acme();

  let answer = fixture.query.context(&ContextRequest::new(TASK)).unwrap().answer;

  assert_eq!(
    omitted_block(&pack_text(&answer)),
    ["OMITTED (16): 11 low relevance; 5 shown inside an enclosing MUST READ body (scores with explain=true)"]
  );
  assert_eq!(answer.omitted.total, 16);
  assert_eq!(answer.omitted.items.len(), 16);
}

#[test]
fn should_keep_listing_seed_diagnostics_under_the_collapsed_counts() {
  let fixture = common::acme();
  let request = ContextRequest {
    seeds: vec!["UnknownWidget".to_owned()],
    budget: 1_500,
    ..ContextRequest::new(TASK)
  };

  let answer = fixture.query.context(&request).unwrap().answer;

  assert_eq!(
    omitted_block(&pack_text(&answer)),
    [
      "OMITTED (32): 16 budget; 11 low relevance; 4 shown inside an enclosing MUST READ body (scores with explain=true)",
      "  UnknownWidget: no entity matches UnknownWidget; try a search for UnknownWidget [unknown seed]",
    ]
  );
}

#[test]
fn should_list_every_omitted_item_when_explaining() {
  let fixture = common::acme();
  let request = ContextRequest {
    explain: true,
    ..ContextRequest::new(TASK)
  };

  let answer = fixture.query.context(&request).unwrap().answer;
  let block = omitted_block(&pack_text(&answer));

  assert_eq!(block.len(), 17);
  assert_eq!(block[0], "OMITTED (16)");
  assert_eq!(
    block[1],
    "  ApiKeyGuard.canActivate method acme-api:src/auth/api-key.guard.ts:8 [shown inside an enclosing MUST READ body]"
  );
  assert_eq!(block.iter().filter(|line| line.ends_with("[low relevance]")).count(), 11);
  assert_eq!(answer.estimated_tokens, pack_text(&answer).chars().count().div_ceil(4));
}

#[test]
fn should_list_the_confidence_floor_with_its_relation_count() {
  let fixture = common::acme();
  let request = ContextRequest {
    min_confidence: Some(0.9),
    ..ContextRequest::new(TASK)
  };

  let answer = fixture.query.context(&request).unwrap().answer;
  let title = omitted_block(&pack_text(&answer)).remove(0);

  let floor = answer.omitted.items.iter().find(|item| item.reason == OmissionReason::BelowConfidenceFloor).unwrap();
  assert!(title.contains(&format!("; {}", floor.label)) || title.contains(&format!(": {}", floor.label)), "{title}");
  assert!(floor.label.ends_with("relations below confidence 0.90"), "{}", floor.label);
}

#[test]
fn should_estimate_a_small_pack_by_its_rendered_size() {
  let fixture = common::single_repo("web", &[("package.json", r#"{ "name": "web" }"#), ("src/label.ts", LABEL)]);

  let answer = fixture.query.context(&ContextRequest::new("make formatLabel stop calling toUpperCase")).unwrap().answer;

  assert_eq!(assert_honest(&answer), 460);
  assert_eq!((answer.estimated_tokens, answer.budget), (115, 6_000));
  assert!(!over_budget(&answer));
}

#[test]
fn should_fill_a_budget_bound_pack_up_to_its_rendered_budget() {
  let fixture = common::acme();
  let request = ContextRequest {
    budget: 1_500,
    ..ContextRequest::new(TASK)
  };

  let answer = fixture.query.context(&request).unwrap().answer;

  let chars = assert_honest(&answer);
  assert!(chars <= 6_000, "{chars}");
  assert!(!over_budget(&answer));
  assert_eq!(answer.omitted_by_reason.get(&OmissionReason::Budget), Some(&16));
  assert!(!answer.must_read.is_empty());
}

#[test]
fn should_state_the_rendered_size_when_mandatory_mentions_exceed_the_budget() {
  let fixture = common::acme();
  let request = ContextRequest {
    budget: 300,
    ..ContextRequest::new(TASK)
  };

  let answer = fixture.query.context(&request).unwrap().answer;

  let chars = assert_honest(&answer);
  assert!(chars > 1_200, "{chars}");
  assert_eq!(
    answer.caveats.first(),
    Some(&Caveat::Note {
      message: format!(
        "mandatory mentions exceed the budget ({} > 300 tokens); uncertain and impact items are never dropped silently",
        answer.estimated_tokens
      ),
    })
  );
  assert_eq!((answer.impact.len(), answer.tests.len(), answer.uncertain.len()), (6, 0, 1));
  assert!(answer.must_read.is_empty() && answer.skim.is_empty());
}

#[test]
fn should_resolve_a_bare_file_name_that_names_one_indexed_file() {
  let fixture = shop_tree();

  let response = fixture.query.symbol(&SymbolRequest::new("price-rules.ts")).unwrap();
  let request = ContextRequest {
    seeds: vec!["price-rules.ts".to_owned()],
    ..ContextRequest::new("round the discounted price")
  };
  let answer = fixture.query.context(&request).unwrap().answer;

  let Targeted::Found(found) = response.answer else { panic!("{:?}", response.answer) };
  assert_eq!(found.entity.key, "file:tree:apps/web-shop/src/price-rules.ts");
  assert_eq!(answer.omitted_by_reason.get(&OmissionReason::UnknownSeed), None);
  assert!(answer.must_read.iter().any(|item| item.entity.short == "applyPriceRules"), "{answer:#?}");
}

#[test]
fn should_list_the_files_a_bare_name_matches_when_several_do() {
  let fixture = shop_tree();

  let response = fixture.query.symbol(&SymbolRequest::new("cart-total.ts")).unwrap();
  let request = ContextRequest {
    seeds: vec!["cart-total.ts".to_owned()],
    ..ContextRequest::new("sum the cart")
  };
  let answer = fixture.query.context(&request).unwrap().answer;

  let hint = "cart-total.ts matches 2 indexed files: tree:apps/back-office/src/cart-total.ts, tree:apps/web-shop/src/cart-total.ts";
  assert_eq!(
    response.answer,
    Targeted::NotFound {
      spec: "cart-total.ts".to_owned(),
      hint: hint.to_owned()
    }
  );
  let unknown: Vec<&str> = answer
    .omitted
    .items
    .iter()
    .filter(|item| item.reason == OmissionReason::UnknownSeed)
    .map(|item| item.label.as_str())
    .collect();
  assert_eq!(unknown, [format!("cart-total.ts: {hint}")]);
}

#[test]
fn should_scope_a_bare_file_name_to_the_named_repo() {
  let fixture = two_shops();

  let everywhere = fixture.query.symbol(&SymbolRequest::new("price-rules.ts")).unwrap();
  let scoped = fixture.query.symbol(&SymbolRequest::new("back-office:price-rules.ts")).unwrap();

  assert_eq!(
    everywhere.answer,
    Targeted::NotFound {
      spec: "price-rules.ts".to_owned(),
      hint: "price-rules.ts matches 2 indexed files: back-office:src/price-rules.ts, web-shop:src/price-rules.ts".to_owned()
    }
  );
  let Targeted::Found(found) = scoped.answer else { panic!("{:?}", scoped.answer) };
  assert_eq!(found.entity.key, "file:back-office:src/price-rules.ts");
}

#[test]
fn should_restrict_search_to_a_directory_given_as_the_repo() {
  let fixture = shop_tree();

  let relative = fixture.query.search_scope("apps/web-shop").unwrap();
  let qualified = fixture.query.search_scope("tree:apps/back-office/").unwrap();
  let unknown = fixture.query.search_scope("apps/warehouse").unwrap();
  let repo = fixture.query.search_scope("tree").unwrap();

  let in_shop = SearchRestriction {
    repo: "tree".to_owned(),
    path_prefix: Some("apps/web-shop/".to_owned()),
  };
  assert_eq!(relative, Some(in_shop.clone()));
  assert_eq!(
    qualified,
    Some(SearchRestriction {
      repo: "tree".to_owned(),
      path_prefix: Some("apps/back-office/".to_owned()),
    })
  );
  assert_eq!(unknown, None);
  assert_eq!(
    repo,
    Some(SearchRestriction {
      repo: "tree".to_owned(),
      path_prefix: None,
    })
  );
  let request = SearchRequest {
    repo: Some(in_shop.repo),
    path_prefix: in_shop.path_prefix,
    limit: 1,
    ..SearchRequest::new("cart total report")
  };
  let response = fixture.query.search(&request).unwrap();
  let text = fixture.text(&response);
  let shown: Vec<&str> = response
    .answer
    .exact_name
    .iter()
    .chain(&response.answer.strong)
    .chain(&response.answer.related)
    .map(|hit| hit.entity.key.as_str())
    .collect();
  assert_eq!(text.lines().nth(1), Some("search \"cart total report\" in tree:apps/web-shop/: 1 hits"));
  assert_eq!(shown, ["ts:tree:tree:apps/web-shop/src/cart-total::cartTotal#function"]);
}

#[test]
fn should_charge_the_held_omissions_before_filling_a_budget_bound_pack() {
  let fixture = common::acme();
  let request = ContextRequest {
    known: vec!["acme-api:src/orders/orders.controller.ts".to_owned(), "acme-api:src/auth/api-key.guard.ts".to_owned()],
    budget: 1_500,
    ..ContextRequest::new(TASK)
  };

  let answer = fixture.query.context(&request).unwrap().answer;

  assert_eq!(assert_honest(&answer), 5_987);
  assert_eq!(answer.estimated_tokens, 1_497);
  assert!(!over_budget(&answer), "{:?}", answer.caveats);
  assert_eq!(
    omitted_block(&pack_text(&answer)),
    [
      "OMITTED (28): 15 low relevance; 9 budget; 2 shown inside an enclosing MUST READ body (scores with explain=true)",
      "  src/auth/api-key.guard.ts file acme-api:src/auth/api-key.guard.ts as acme-api:src/auth/api-key.guard.ts (4 pack items) [already held by the caller]",
      "  src/orders/orders.controller.ts file acme-api:src/orders/orders.controller.ts as acme-api:src/orders/orders.controller.ts (7 pack items) [already held by the caller]",
    ]
  );
}

#[test]
fn should_state_the_header_estimate_in_the_over_budget_caveat_when_explaining() {
  let fixture = common::acme();
  let request = ContextRequest {
    budget: 300,
    explain: true,
    ..ContextRequest::new(TASK)
  };

  let answer = fixture.query.context(&request).unwrap().answer;

  assert_eq!(answer.estimated_tokens, pack_text(&answer).chars().count().div_ceil(4));
  assert_eq!(
    answer.caveats.first(),
    Some(&Caveat::Note {
      message: format!(
        "mandatory mentions exceed the budget ({} > 300 tokens); uncertain and impact items are never dropped silently",
        answer.estimated_tokens
      ),
    })
  );
}

#[test]
fn should_resolve_a_bare_file_name_in_the_repo_the_filter_names() {
  let fixture = two_shops();
  let request = SymbolRequest {
    filter: TargetFilter {
      kinds: Vec::new(),
      repo: Some("web-shop".to_owned()),
    },
    ..SymbolRequest::new("price-rules.ts")
  };

  let response = fixture.query.symbol(&request).unwrap();

  let Targeted::Found(found) = response.answer else { panic!("{:?}", response.answer) };
  assert_eq!(found.entity.key, "file:web-shop:src/price-rules.ts");
}

fn plant_capability(fixture: &common::Fixture, key: &str) {
  let fact = NewInferredFact {
    subjects: vec![EntityId::of(&key.parse::<EntityKey>().unwrap())],
    relation: RELATION_CAPABILITY_SUMMARY.to_owned(),
    payload: serde_json::json!({ "summary": "formatLabel tidies labels", "capabilities": ["formatLabel"] }),
    certainty: Certainty::Inferred {
      confidence: Confidence::new(0.9).unwrap(),
      provider: "fake".to_owned(),
      model: "fake-model".to_owned(),
      rationale: "planted by the test".to_owned(),
    },
    evidence: Vec::new(),
    created_run: None,
    llm_call: None,
  };
  fixture.query.engine().store().write(|tx| tx.insert_inferred_fact(&fact)).unwrap();
}

#[test]
fn should_keep_structural_and_inferred_hits_inside_the_searched_directory() {
  let fixture = common::single_repo(
    "tree",
    &[
      ("package.json", r#"{ "name": "tree" }"#),
      ("apps/web-shop/src/label.ts", LABEL),
      ("apps/back-office/src/title.ts", &LABEL.replace("formatLabel", "tidyTitle")),
    ],
  );
  let label = "ts:tree:tree:apps/web-shop/src/label::formatLabel#function";
  let title = "ts:tree:tree:apps/back-office/src/title::tidyTitle#function";
  fixture.query.engine().ensure_fresh().unwrap();
  plant_capability(&fixture, label);
  plant_capability(&fixture, title);
  let scope = fixture.query.search_scope("apps/web-shop").unwrap().unwrap();
  let scoped = SearchRequest {
    repo: Some(scope.repo),
    path_prefix: scope.path_prefix,
    ..SearchRequest::new("formatLabel")
  };

  let everywhere = fixture.query.search(&SearchRequest::new("formatLabel")).unwrap().answer;
  let inside = fixture.query.search(&scoped).unwrap().answer;

  let keys = |answer: &SearchAnswer| -> (Vec<String>, Vec<String>) {
    (
      answer.structurally_similar.iter().map(|hit| hit.entity.key.clone()).collect(),
      answer.inferred_capabilities.iter().map(|hit| hit.entity.key.clone()).collect(),
    )
  };
  assert_eq!(keys(&everywhere), (vec![title.to_owned()], vec![title.to_owned(), label.to_owned()]));
  assert_eq!(keys(&inside), (Vec::new(), vec![label.to_owned()]));
}
