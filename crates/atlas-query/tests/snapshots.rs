mod common;

use atlas_query::{
  ApiRequest, ChangesRequest, ContextRequest, ExplainRequest, ImpactRequest, PatternsRequest, ReadRequest, RenderText, Response, SearchRequest, SimilarRequest, SymbolRequest,
  TestsForRequest, TraversalKind, TraverseRequest, UnusedRequest, render_json,
};
use common::Fixture;
use serde::Serialize;

fn text<T: RenderText>(fixture: &Fixture, response: atlas_query::QueryResult<Response<T>>) -> String {
  fixture.text(&response.unwrap())
}

fn json<T: Serialize>(response: atlas_query::QueryResult<Response<T>>) -> String {
  let mut response = response.unwrap();
  response.freshness.duration_ms = 0;
  render_json(&response).unwrap()
}

fn traverse(fixture: &Fixture, kind: TraversalKind, target: &str, depth: u32) -> String {
  text(
    fixture,
    fixture.query.traverse(
      kind,
      &TraverseRequest {
        depth,
        ..TraverseRequest::new(target)
      },
    ),
  )
}

fn battery(fixture: &Fixture) -> Vec<(&'static str, String)> {
  let query = &fixture.query;
  query.status().unwrap();
  vec![
    ("overview", text(fixture, query.overview())),
    ("search_cents", text(fixture, query.search(&SearchRequest::new("convert currency string to cents")))),
    (
      "symbol_body",
      text(
        fixture,
        query.symbol(&SymbolRequest {
          body: true,
          ..SymbolRequest::new("UsersService.findOne")
        }),
      ),
    ),
    ("symbol_ambiguous", text(fixture, query.symbol(&SymbolRequest::new("findOne")))),
    ("callers_depth_2", traverse(fixture, TraversalKind::Callers, "UsersService.findOne", 2)),
    ("callees_depth_2", traverse(fixture, TraversalKind::Callees, "OrdersController.findOne", 2)),
    ("dependents", traverse(fixture, TraversalKind::Dependents, "toCents", 1)),
    ("dependencies", traverse(fixture, TraversalKind::Dependencies, "UsersController", 1)),
    ("impact_users_find_one", text(fixture, query.impact(&ImpactRequest::new(["UsersService.findOne"])))),
    ("impact_trait_method", text(fixture, query.impact(&ImpactRequest::new(["InvoiceStore::insert"])))),
    ("tests_for", text(fixture, query.tests_for(&TestsForRequest::new("UsersService")))),
    ("api", text(fixture, query.api(&ApiRequest::default()))),
    ("similar", text(fixture, query.similar(&SimilarRequest::new("toCents")))),
    ("unused", text(fixture, query.unused(&UnusedRequest::default()))),
    ("patterns_endpoint", text(fixture, query.patterns(&PatternsRequest::new("endpoint")))),
    ("patterns_test", text(fixture, query.patterns(&PatternsRequest::new("test")))),
    (
      "context",
      text(fixture, query.context(&ContextRequest::new("add a DELETE /orders/:id endpoint with api-key auth"))),
    ),
    ("explain", text(fixture, query.explain(&ExplainRequest::new("UsersController.findOne")))),
    ("read", text(fixture, query.read_symbol(&ReadRequest::new("toCents")))),
    ("changes", text(fixture, query.changes(&ChangesRequest::default()))),
    ("json_impact", json(query.impact(&ImpactRequest::new(["toCents"])))),
    (
      "json_api",
      json(query.api(&ApiRequest {
        endpoint: Some("GET /users/:id".to_owned()),
        ..ApiRequest::default()
      })),
    ),
    (
      "json_context",
      json(query.context(&ContextRequest {
        budget: 1_500,
        ..ContextRequest::new("convert currency string to cents")
      })),
    ),
    ("json_ambiguous", json(query.callers(&TraverseRequest::new("findOne")))),
    ("json_explain", json(query.explain(&ExplainRequest::new("GET /orders/:id")))),
  ]
}

#[test]
fn should_match_golden_snapshots() {
  let fixture = common::acme();

  for (name, output) in battery(&fixture) {
    insta::assert_snapshot!(name, output);
  }
}

#[test]
fn should_produce_byte_identical_output_for_the_same_index_and_query() {
  let first = common::acme();
  let second = common::acme();

  let left = battery(&first);
  let right = battery(&second);
  let again = battery(&first);

  assert_eq!(left, right);
  assert_eq!(left, again);
}
