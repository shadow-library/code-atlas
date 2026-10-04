mod e2e_support;

use std::path::PathBuf;

use e2e_support::expected::{Atlas, Selector, check_forbidden, evaluate, load, new_report};
use e2e_support::{Found, Sandbox, entity_refs, fixtures, shorts};

struct Case {
  sandbox: Sandbox,
  repo: PathBuf,
  name: &'static str,
}

impl Case {
  fn indexed(name: &'static str) -> Self {
    let sandbox = Sandbox::new();
    let repo = sandbox.copy_repo("adversarial", name);
    sandbox.create_project(name, std::slice::from_ref(&repo));
    let indexed = sandbox.run(&repo, &["index"]).ok();
    assert!(indexed.stdout.contains("outcome: ok"), "{indexed}");
    Self { sandbox, repo, name }
  }

  fn atlas(&self) -> Atlas<'_> {
    Atlas {
      sandbox: &self.sandbox,
      cwd: self.repo.clone(),
      project: self.name.to_owned(),
    }
  }

  fn assert_expectations(&self) {
    let expected = load(&fixtures().join("adversarial").join(self.name).join("expected.toml"));
    evaluate(&self.atlas(), &expected).assert_clean(self.name);
  }

  fn find(&self, path: &str, symbol: &str) -> Found {
    let selector = Selector::Symbol {
      repo: self.name.to_owned(),
      path: path.to_owned(),
      symbol: Some(symbol.to_owned()),
      suite: None,
    };
    self.atlas().resolve(&selector).unwrap_or_else(|| panic!("{} is not indexed", selector.describe()))
  }
}

#[test]
fn should_meet_the_shadowing_expectations_without_false_callers() {
  let case = Case::indexed("shadowing-ts");
  case.assert_expectations();

  let format = case.find("src/format.ts", "format");
  let callers = case.atlas().answer(&["query", "callers", &format.id, "--min-confidence", "0"]);
  let mut names = shorts(&callers["groups"]);
  names.sort();
  assert_eq!(names, ["renderBlockScoped", "renderImported"], "{callers:#}");
}

#[test]
fn should_resolve_the_reexport_chain_to_the_original_definition() {
  let case = Case::indexed("reexport-chain-ts");
  case.assert_expectations();

  let original = case.find("src/core/impl.ts", "computeTotal");
  let callers = case.atlas().answer(&["query", "callers", &original.id]);
  assert_eq!(shorts(&callers["groups"]), ["checkout"], "{callers:#}");
  assert_eq!(callers["groups"][0]["items"][0]["relation"]["certainty"]["class"], "resolved");
  let decoy = case.find("src/other/total.ts", "total");
  let decoy_callers = case.atlas().answer(&["query", "callers", &decoy.id, "--min-confidence", "0"]);
  assert_eq!(decoy_callers["total"], 0, "{decoy_callers:#}");
}

#[test]
fn should_flag_ambiguous_duplicate_names_instead_of_picking_one() {
  let case = Case::indexed("duplicate-names-rust");
  case.assert_expectations();

  let ambiguous_target = case.atlas().run(&["query", "callers", "parse"]).expect(4);
  assert!(ambiguous_target.stdout.contains("ambiguous target \"parse\": 2 candidates"), "{ambiguous_target}");
  let parse = case.find("src/a.rs", "parse");
  let callers = case.atlas().run(&["query", "callers", &parse.id]).ok();
  assert!(
    callers.stdout.contains("unresolved references named \"parse\" could hide relations (1 ambiguous"),
    "{callers}"
  );
  assert!(callers.stdout.contains("duplicate-names-rust:src/ambiguous.rs:5 parse (ambiguous)"), "{callers}");
  let resolved_callers: Vec<String> = shorts(&case.atlas().answer(&["query", "callers", &parse.id])["groups"]);
  assert_eq!(resolved_callers, ["split_fields"]);
}

#[test]
fn should_record_dynamic_dispatch_as_unresolved_instead_of_linking_it() {
  let case = Case::indexed("dynamic-ts");
  case.assert_expectations();

  let unused = case.atlas().answer(&["query", "unused", "--limit", "100"]);
  let likely: Vec<String> = shorts(&unused["likely_unused"]);
  assert!(likely.is_empty(), "dynamically reachable code reported as likely unused: {likely:?}");
  let stop = entity_refs(&unused["possibly_unused"]).into_iter().find(|found| found.short == "stop");
  assert!(stop.is_some(), "stop should stay a possible, not a certain, unused candidate: {unused:#}");
}

#[test]
#[ignore = "BUG: callers/impact of a function reachable only through a computed member call (table[methodName]()) show no dynamic-dispatch risk caveat"]
fn should_warn_about_dynamic_dispatch_in_callers_and_impact() {
  let case = Case::indexed("dynamic-ts");
  let stop = case.find("src/handlers.ts", "stop");

  let callers = case.atlas().run(&["query", "callers", &stop.id]).ok();
  let impact = case.atlas().run(&["query", "impact", &stop.id]).ok();

  for output in [&callers, &impact] {
    assert!(output.stdout.contains("dynamic"), "no dynamic risk caveat: {output}");
    assert!(output.stdout.contains("src/dispatch.ts"), "the caveat should name the dynamic call site: {output}");
  }
}

#[test]
fn should_keep_computed_routes_heuristic_and_invent_no_routes() {
  let case = Case::indexed("computed-routes-ts");
  case.assert_expectations();

  let routes: Vec<String> = case
    .atlas()
    .endpoints()
    .iter()
    .map(|item| format!("{} {}", item["method"].as_str().unwrap(), item["route"].as_str().unwrap()))
    .collect();
  assert!(!routes.iter().any(|route| route.contains("reports")), "a route was invented for buildPath(): {routes:?}");
  for item in case.atlas().endpoints() {
    let route = item["route"].as_str().unwrap();
    if route.starts_with("/api/items") {
      assert_eq!(item["certainty"]["class"], "heuristic", "{item:#}");
    }
  }
}

#[test]
#[ignore = "BUG: `api` lists router-relative fragments (GET /{id}, POST /) of a router mounted elsewhere as endpoints of the server"]
fn should_not_list_router_relative_fragments_as_server_endpoints() {
  let case = Case::indexed("computed-routes-ts");

  let mut routes: Vec<String> = case
    .atlas()
    .endpoints()
    .iter()
    .map(|item| format!("{} {}", item["method"].as_str().unwrap(), item["route"].as_str().unwrap()))
    .collect();
  routes.sort();

  assert_eq!(routes, ["GET /api/items/{id}", "GET /api/status", "GET /legacy/ping", "POST /api/items"]);
}

#[test]
fn should_never_present_similar_but_different_functions_as_equivalent() {
  let case = Case::indexed("similar-not-equivalent");
  case.assert_expectations();

  for (path, symbol) in [("src/rounding.ts", "roundHalfUp"), ("src/sums.ts", "isEvenMod"), ("src/user-input.ts", "normalize")] {
    let found = case.find(path, symbol);
    let similar = case.atlas().run(&["query", "similar", &found.id, "--threshold", "0"]).ok();
    for line in similar.stdout.lines().filter(|line| line.trim_start().starts_with("verdict:")) {
      assert!(!line.contains("equivalent") || line.contains("not equivalent"), "{symbol}: {line}\n{similar}");
    }
  }
}

#[test]
fn should_index_broken_syntax_partially_and_say_so() {
  let case = Case::indexed("broken-syntax");
  case.assert_expectations();

  let status = case.atlas().answer(&["status"]);
  assert_eq!(status["files_by_parse_status"], serde_json::json!({ "partial": 2 }), "{status:#}");
  let before = case.find("src/broken.ts", "before");
  let callers = case.atlas().run(&["query", "callers", &before.id]).ok();
  assert!(
    callers.stdout.contains("files in scope parsed partially or failed; their relations may be missing"),
    "{callers}"
  );
  assert!(callers.stdout.contains("broken-syntax:src/broken.ts partial"), "{callers}");
}

#[test]
fn should_never_present_macro_generated_items_as_resolved() {
  let case = Case::indexed("macro-heavy-rust");
  case.assert_expectations();

  for generated in ["add", "sub"] {
    let output = case.atlas().run(&["query", "symbol", generated]).expect(4);
    assert!(output.stdout.contains("not found"), "{output}");
  }
  let manhattan = case.find("src/lib.rs", "Point::manhattan");
  let callees = case.atlas().answer(&["query", "callees", &manhattan.id, "--min-confidence", "0"]);
  let certain = callees["groups"]
    .as_array()
    .into_iter()
    .flatten()
    .flat_map(|group| group["items"].as_array().into_iter().flatten())
    .filter(|item| item["relation"]["certainty"]["authoritative"] == true)
    .count();
  assert_eq!(certain, 0, "macro-generated calls presented as certain: {callees:#}");
}

#[test]
#[ignore = "BUG: calls through a `T: Store` bound and through `Box<dyn Store>` reach the trait method only as [heuristic 0.70]; expected.toml requires resolved"]
fn should_meet_the_trait_dispatch_expectations() {
  Case::indexed("trait-dispatch-rust").assert_expectations();
}

#[test]
fn should_never_resolve_trait_dispatch_to_a_concrete_impl() {
  let case = Case::indexed("trait-dispatch-rust");
  let expected = load(&fixtures().join("adversarial").join(case.name).join("expected.toml"));
  let mut report = new_report();

  check_forbidden(&case.atlas(), &expected, &mut report);

  report.assert_clean(case.name);
  let read_mem = case.find("src/lib.rs", "read_mem");
  let callees = case.atlas().answer(&["query", "callees", &read_mem.id]);
  assert_eq!(shorts(&callees["groups"]), ["MemStore::<Store>::get"], "{callees:#}");
  assert_eq!(callees["groups"][0]["items"][0]["relation"]["certainty"]["class"], "resolved");
}

#[test]
fn should_meet_the_generated_code_expectations() {
  Case::indexed("generated-code").assert_expectations();
}
