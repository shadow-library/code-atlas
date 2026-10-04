mod common;

use atlas_query::ApiRequest;

const PACKAGE: &str = "{\n  \"name\": \"orphan\",\n  \"private\": true,\n  \"dependencies\": { \"express\": \"^4.18.0\" }\n}\n";
const ROUTER: &str =
  "import { Router } from 'express';\n\nexport const widgetsRouter = Router();\n\nwidgetsRouter.get('/:id', (req, res) => {\n  res.json({ id: req.params.id });\n});\n";

#[test]
fn should_hide_router_relative_endpoints_superseded_by_composed_routes() {
  let fixture = common::fixture("adversarial", &["computed-routes-ts"]);

  let response = fixture.query.api(&ApiRequest::default()).unwrap();
  let text = fixture.text(&response);

  assert_eq!(response.answer.endpoints.total, 4, "{text}");
  assert!(response.answer.router_fragments.is_empty(), "{text}");
  assert!(!text.contains("router fragments"), "{text}");
  let routes: Vec<(&str, &str)> = response.answer.endpoints.items.iter().map(|view| (view.method.as_str(), view.route.as_str())).collect();
  assert_eq!(
    routes,
    [("POST", "/api/items"), ("GET", "/api/items/{id}"), ("GET", "/api/status"), ("GET", "/legacy/ping")]
  );
}

#[test]
fn should_list_unmounted_router_fragments_apart_from_the_endpoint_totals() {
  let fixture = common::single_repo("orphan", &[("package.json", PACKAGE), ("src/widgets.ts", ROUTER)]);

  let response = fixture.query.api(&ApiRequest::default()).unwrap();
  let text = fixture.text(&response);

  assert_eq!(response.answer.endpoints.total, 0, "{text}");
  assert_eq!(response.answer.router_fragments.total, 1, "{text}");
  assert!(text.contains("endpoints (0)\nrouter fragments (not mounted in this project) (1)\n  GET /{id}"), "{text}");
}

#[test]
fn should_not_report_superseded_router_fragments_as_unused_endpoints() {
  let fixture = common::fixture("adversarial", &["computed-routes-ts"]);

  let answer = fixture.query.unused(&atlas_query::UnusedRequest::default()).unwrap().answer;

  let routes: Vec<&str> = answer
    .possibly_unused
    .items
    .iter()
    .chain(&answer.likely_unused.items)
    .map(|candidate| candidate.entity.short.as_str())
    .collect();
  assert!(!routes.contains(&"GET /{id}") && !routes.contains(&"POST /"), "{routes:?}");
}

#[test]
fn should_mark_unmounted_router_fragments_as_possibly_unused_with_the_mount_caveat() {
  let fixture = common::single_repo("orphan", &[("package.json", PACKAGE), ("src/widgets.ts", ROUTER)]);

  let answer = fixture.query.unused(&atlas_query::UnusedRequest::default()).unwrap().answer;

  let fragment = answer
    .possibly_unused
    .items
    .iter()
    .find(|candidate| candidate.entity.short == "GET /{id}")
    .expect("fragment listed");
  assert_eq!(fragment.evidence[0], "router fragment; mount not detected");
}

#[test]
fn should_count_unmounted_router_fragments_separately_in_the_overview() {
  let fixture = common::single_repo("orphan", &[("package.json", PACKAGE), ("src/widgets.ts", ROUTER)]);

  let response = fixture.query.overview().unwrap();
  let text = fixture.text(&response);

  assert!(text.contains("orphan: 0 endpoints (+1 unmounted router fragments)"), "{text}");
}
