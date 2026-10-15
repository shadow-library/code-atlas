mod e2e_support;

use std::fs;
use std::path::Path;

use e2e_support::expected::{Atlas, Report, Selector, check_auth, check_blast_radius, check_endpoint_auth, load, new_report, tables};
use e2e_support::{ACME_REPOS, Found, Sandbox, assert_same_answers, entity_refs, fixtures, shorts};
use serde_json::Value;

fn acme_atlas<'a>(sandbox: &'a Sandbox, cwd: &Path) -> Atlas<'a> {
  Atlas {
    sandbox,
    cwd: cwd.to_path_buf(),
    project: "acme".to_owned(),
  }
}

fn endpoint<'a>(api: &'a Value, method: &str, route: &str) -> &'a Value {
  api["endpoints"]["items"]
    .as_array()
    .unwrap()
    .iter()
    .find(|item| item["method"] == method && item["route"] == route)
    .unwrap_or_else(|| panic!("{method} {route} is missing: {api:#}"))
}

fn consumers(item: &Value) -> Vec<String> {
  let mut names: Vec<String> = item["consumers"]
    .as_array()
    .unwrap()
    .iter()
    .map(|consumer| consumer["consumer"]["entity"]["short"].as_str().unwrap().to_owned())
    .collect();
  names.sort();
  names
}

fn used_by(api: &Value, repo: &str, scheme: &str) -> Vec<String> {
  let matrix = api["auth_matrix"].as_array().unwrap().iter().find(|matrix| matrix["repo"] == repo).unwrap();
  let entry = matrix["schemes"].as_array().unwrap().iter().find(|entry| entry["scheme"]["short"] == scheme).unwrap();
  entry["used_by_repos"].as_array().unwrap().iter().map(|repo| repo.as_str().unwrap().to_owned()).collect()
}

fn repos_of(value: &Value) -> Vec<String> {
  let mut repos: Vec<String> = entity_refs(value).into_iter().filter_map(|found| found.repo).collect();
  repos.sort();
  repos.dedup();
  repos
}

#[test]
fn should_answer_every_query_kind_over_a_freshly_indexed_acme_project() {
  let sandbox = Sandbox::new();
  let api_repo = sandbox.acme().join("acme-api");

  let indexed = sandbox.run(&api_repo, &["index"]).ok();
  assert!(indexed.stdout.starts_with("indexed acme: run 1,"), "{indexed}");
  assert!(indexed.stdout.contains("outcome: ok"), "{indexed}");

  let overview = sandbox.run(&api_repo, &["query", "overview", "--json"]).ok().json();
  assert_eq!(overview["freshness"]["state"], "fresh");
  assert_eq!(overview["freshness"]["files_updated"], 0, "the explicit index left work for the barrier");
  let answer = &overview["answer"];
  let repos: Vec<&str> = answer["repos"].as_array().unwrap().iter().map(|repo| repo["repo"].as_str().unwrap()).collect();
  assert_eq!(repos, ACME_REPOS);
  let apis = answer["apis"].as_array().unwrap();
  let summary = |api: &serde_json::Value| (api["repo"].clone(), api["endpoints"].clone(), api["auth_schemes"].clone());
  assert_eq!(
    apis.iter().map(summary).collect::<Vec<_>>(),
    [
      ("acme-api".into(), 8.into(), serde_json::json!(["api_key", "bearer", "session_cookie"])),
      ("acme-billing".into(), 2.into(), serde_json::json!([]))
    ]
  );
  let scheme_tags: Vec<(&str, &str)> = apis[0]["auth_scheme_certainty"]
    .as_object()
    .unwrap()
    .iter()
    .map(|(name, certainty)| (name.as_str(), certainty["tag"].as_str().unwrap()))
    .collect();
  assert_eq!(
    scheme_tags,
    [("api_key", "[heuristic 0.80]"), ("bearer", "[heuristic 0.85]"), ("session_cookie", "[heuristic 0.80]")]
  );
  assert!(apis[1].get("auth_scheme_certainty").is_none(), "{apis:?}");
  let events = answer["events"].as_array().unwrap();
  assert_eq!(
    (
      events.len(),
      events[0]["event"].as_str(),
      events[0]["publishers"].as_u64(),
      events[0]["certainty"]["tag"].as_str()
    ),
    (1, Some("order.created"), Some(1), Some("[heuristic 0.80]"))
  );

  let search = sandbox.run(&api_repo, &["search", "formatMoney", "--json"]).ok().answer();
  let exact: Vec<Found> = entity_refs(&search["exact_name"]);
  assert_eq!(exact.len(), 1, "{search:#}");
  assert_eq!((exact[0].short.as_str(), exact[0].path.as_deref()), ("formatMoney", Some("src/common/money.ts")));

  let impact = sandbox.run(&api_repo, &["impact", "formatMoney", "--json"]).ok().answer();
  let direct: Vec<String> = shorts(&impact["direct"]);
  assert_eq!(
    direct,
    [
      "OrdersService.findOne",
      "ReportsService.exportCsv",
      "src/orders/orders.service.ts",
      "src/reports/reports.service.ts"
    ]
  );
  assert_eq!(shorts(&impact["endpoints"]), ["GET /orders/{id}", "GET /v1/reports/export"]);
  let mut consumers_reached = shorts(&impact["consumers"]);
  consumers_reached.sort();
  assert_eq!(consumers_reached, ["AcmeApiClient::get_order", "AcmeClient.getOrder"]);
  assert_eq!(
    impact["consumers"]["uncertain"], 2,
    "cross-repo consumers are reached through heuristic calls_endpoint edges"
  );

  let api = sandbox.run(&api_repo, &["api", "--json"]).ok().answer();
  let routes: Vec<String> = api["endpoints"]["items"]
    .as_array()
    .unwrap()
    .iter()
    .map(|item| format!("{} {}", item["method"].as_str().unwrap(), item["route"].as_str().unwrap()))
    .collect();
  assert_eq!(
    routes,
    [
      "GET /admin/stats",
      "POST /orders",
      "GET /orders/{id}",
      "POST /users",
      "DELETE /users/{id}",
      "GET /users/{id}",
      "PATCH /users/{id}",
      "GET /v1/reports/export",
      "POST /invoices",
      "GET /invoices/{id}"
    ]
  );
  assert_eq!(consumers(endpoint(&api, "GET", "/orders/{id}")), ["AcmeApiClient::get_order", "AcmeClient.getOrder"]);
  assert_eq!(consumers(endpoint(&api, "GET", "/users/{id}")), ["AcmeClient.getUser", "useMe"]);
  assert_eq!(used_by(&api, "acme-api", "bearer"), ["acme-sdk", "acme-web"]);
  assert_eq!(used_by(&api, "acme-api", "api_key"), ["acme-billing", "acme-sdk"]);
  assert!(used_by(&api, "acme-api", "session_cookie").is_empty());

  let context = sandbox
    .run(
      &api_repo,
      &["context", "fix the lookup of a single user by id in UsersService.findOne", "--budget", "3000", "--json"],
    )
    .ok()
    .answer();
  let must_read = shorts(&context["must_read"]);
  assert_eq!(must_read[0], "UsersService.findOne", "{context:#}");
  assert!(must_read.contains(&"UsersController.findOne".to_owned()), "{must_read:?}");
  assert!(shorts(&context["skim"]).contains(&"GET /users/{id}".to_owned()), "{context:#}");
  assert!(
    shorts(&context["tests"]).contains(&"UsersService.findOne.should return the user when it exists".to_owned()),
    "{context:#}"
  );
  assert!(repos_of(&context).iter().all(|repo| repo.starts_with("acme-")), "{context:#}");

  let held = sandbox
    .run(
      &api_repo,
      &[
        "context",
        "fix the lookup of a single user by id in UsersService.findOne",
        "--known",
        "UsersService.findOne",
        "--depth",
        "2",
        "--json",
      ],
    )
    .ok()
    .answer();
  assert!(!shorts(&held["must_read"]).contains(&"UsersService.findOne".to_owned()), "{held:#}");
  let held_rows: Vec<&str> = held["omitted"]["items"]
    .as_array()
    .unwrap()
    .iter()
    .filter(|item| item["reason"] == "held")
    .map(|item| item["label"].as_str().unwrap())
    .collect();
  assert_eq!(
    held_rows,
    ["UsersService.findOne method acme-api:src/users/users.service.ts:18 as UsersService.findOne (1 pack item)"],
    "{held:#}"
  );
  sandbox.run(&api_repo, &["context", "fix the lookup", "--depth", "4"]).expect(2);
}

#[test]
fn should_reproduce_the_acme_endpoints_auth_matrix_consumers_and_blast_radius() {
  let sandbox = Sandbox::new();
  let root = sandbox.acme();
  let atlas = acme_atlas(&sandbox, &root.join("acme-api"));
  let expected = load(&fixtures().join("acme/expected.toml"));
  let mut report = new_report();

  check_auth(&atlas, &expected, &mut report);
  check_endpoint_auth(&atlas, &expected, &mut report);
  check_cross_repo_edges(&atlas, &expected, &mut report);
  check_blast_radius(&atlas, &expected, &mut report);

  report.assert_clean("acme");
}

fn check_cross_repo_edges(atlas: &Atlas<'_>, expected: &toml::Value, report: &mut Report) {
  for edge in tables(expected, "edge") {
    let kind = edge["kind"].as_str().unwrap();
    if !matches!(kind, "calls_endpoint" | "uses_auth") {
      continue;
    }
    let (from, to) = (Selector::field(edge, "from"), Selector::field(edge, "to"));
    let relations = atlas.edges(&from, &to, kind).unwrap_or_default();
    match relations.as_slice() {
      [] => report.violations.push(format!("{kind} {} -> {} is missing", from.describe(), to.describe())),
      found if found.iter().any(|relation| relation.class != "heuristic") => {
        report
          .violations
          .push(format!("{kind} {} -> {} is not heuristic: {found:?}", from.describe(), to.describe()))
      }
      _ => {}
    }
  }
}

#[test]
fn should_keep_acme_and_globex_isolated_even_with_a_shared_repo() {
  let sandbox = Sandbox::new();
  let acme = sandbox.acme();
  let globex_repos: Vec<_> = ["globex-api", "globex-worker"]
    .iter()
    .map(|repo| sandbox.copy_repo("globex", repo))
    .chain([acme.join("acme-sdk")])
    .collect();
  sandbox.create_project("globex", &globex_repos);
  let api_repo = acme.join("acme-api");
  let globex_api = sandbox.work().join("globex/globex-api");
  sandbox.run(&api_repo, &["index"]).ok();
  sandbox.run(&globex_api, &["index"]).ok();

  let acme_queries: [&[&str]; 9] = [
    &["search", "formatMoney"],
    &["search", "UsersService"],
    &["query", "symbol", "UsersService"],
    &["api", "--endpoint", "GET /users/:id"],
    &["impact", "formatMoney"],
    &["impact", "UsersService.findOne"],
    &["callers", "to_cents"],
    &["search", "DATABASE_URL", "--kind", "env_var"],
    &["query", "overview"],
  ];
  let globex_expected = load(&fixtures().join("globex/expected.toml"));
  let acme_traps = isolation_traps(&globex_expected, "acme", &ACME_REPOS);
  for query in acme_queries {
    let mut args = query.to_vec();
    args.push("--json");
    let run = sandbox.run(&api_repo, &args);
    assert!([0, 3].contains(&run.code), "{run}");
    let answer = run.answer();
    let leaked: Vec<String> = repos_of(&answer).into_iter().filter(|repo| !repo.starts_with("acme-")).collect();
    assert!(leaked.is_empty(), "{query:?} from acme returned {leaked:?}");
    for found in entity_refs(&answer) {
      assert!(!acme_traps.iter().any(|trap| trap.matches(&found)), "{query:?} returned isolation trap {found:?}");
    }
  }

  let globex_answer = sandbox.run(&globex_api, &["impact", "formatMoney", "--json"]).ok().answer();
  assert_eq!(
    repos_of(&globex_answer),
    ["acme-sdk", "globex-api"],
    "acme-sdk is a globex repo too, so its client may consume globex endpoints"
  );
  let mut globex_consumers = shorts(&globex_answer["consumers"]);
  globex_consumers.sort();
  assert_eq!(
    globex_consumers,
    ["AcmeClient.createOrder", "AcmeClient.getOrder"],
    "both routes share the registerOrderRoutes handler"
  );
  let globex_search = sandbox.run(&globex_api, &["search", "UsersService", "--json"]).ok().answer();
  assert!(
    repos_of(&globex_search)
      .iter()
      .all(|repo| repo != "acme-api" && repo != "acme-web" && repo != "acme-billing"),
    "{globex_search:#}"
  );

  let sdk = acme.join("acme-sdk");
  let ambiguous = sandbox.run(&sdk, &["query", "symbol", "toCents"]).expect(2);
  assert!(ambiguous.stderr.contains("acme") && ambiguous.stderr.contains("globex"), "{ambiguous}");
  let in_acme = sandbox.run(&sdk, &["-p", "acme", "impact", "toCents", "--json"]).ok().answer();
  let in_globex = sandbox.run(&sdk, &["-p", "globex", "impact", "toCents", "--json"]).ok().answer();
  assert!(shorts(&in_acme["direct"]).contains(&"OrderForm".to_owned()), "{in_acme:#}");
  assert_eq!(
    repos_of(&in_globex),
    ["acme-sdk"],
    "the shared repo must not drag acme consumers into globex: {in_globex:#}"
  );

  let money = sdk.join("src/money.ts");
  let mut source = fs::read_to_string(&money).unwrap();
  source.push_str("\nexport function fromCents(cents: number): string {\n  return (cents / 100).toFixed(2);\n}\n");
  fs::write(&money, source).unwrap();
  for project in ["acme", "globex"] {
    let found = sandbox.run(&sdk, &["-p", project, "query", "symbol", "fromCents"]).ok();
    assert!(found.stdout.contains("fromCents function acme-sdk:src/money.ts:"), "{project}: {found}");
  }
  assert!(sandbox.db("acme").exists() && sandbox.db("globex").exists());
  assert_ne!(sandbox.db("acme"), sandbox.db("globex"));
}

fn isolation_traps(expected: &toml::Value, project: &str, own_repos: &[&str]) -> Vec<Selector> {
  tables(expected, "isolation")
    .into_iter()
    .filter(|rule| rule["project"].as_str() == Some(project))
    .flat_map(|rule| rule["must_not_return"].as_array().unwrap().iter().map(Selector::parse))
    .filter(|selector| selector.repo().is_none_or(|repo| !own_repos.contains(&repo)))
    .collect()
}

#[test]
fn should_drop_and_restore_cross_repo_links_when_the_sdk_leaves_and_rejoins() {
  let sandbox = Sandbox::new();
  let root = sandbox.acme();
  let api_repo = root.join("acme-api");
  sandbox.run(&api_repo, &["index"]).ok();
  let api_before = sandbox.run(&api_repo, &["api", "--json"]).ok().answer();
  let impact_before = sandbox.run(&api_repo, &["impact", "OrdersService.findOne", "--json"]).ok().answer();
  assert!(consumers(endpoint(&api_before, "GET", "/users/{id}")).contains(&"AcmeClient.getUser".to_owned()));
  assert!(shorts(&impact_before["consumers"]).contains(&"AcmeClient.getOrder".to_owned()));

  sandbox.run(&api_repo, &["project", "remove-repo", "acme", "acme-sdk"]).ok();

  let api_without = sandbox.run(&api_repo, &["api", "--json"]).ok().answer();
  assert!(!repos_of(&api_without).contains(&"acme-sdk".to_owned()), "{api_without:#}");
  assert_eq!(consumers(endpoint(&api_without, "GET", "/users/{id}")), ["useMe"]);
  assert_eq!(consumers(endpoint(&api_without, "PATCH", "/users/{id}")), Vec::<String>::new());
  assert_eq!(used_by(&api_without, "acme-api", "bearer"), ["acme-web"]);
  assert_eq!(used_by(&api_without, "acme-api", "api_key"), ["acme-billing"]);
  let impact_without = sandbox.run(&api_repo, &["impact", "OrdersService.findOne", "--json"]).ok().answer();
  assert!(!repos_of(&impact_without).contains(&"acme-sdk".to_owned()), "{impact_without:#}");
  assert_same_answers(
    &sandbox,
    "acme",
    &[&["api"], &["impact", "OrdersService.findOne"], &["query", "overview"]],
    "after removing acme-sdk",
  );

  sandbox.run(&api_repo, &["project", "add-repo", "acme", root.join("acme-sdk").to_str().unwrap()]).ok();

  assert_eq!(sandbox.run(&api_repo, &["api", "--json"]).ok().answer(), api_before);
  assert_eq!(sandbox.run(&api_repo, &["impact", "OrdersService.findOne", "--json"]).ok().answer(), impact_before);
}
