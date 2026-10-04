mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use atlas_core::{EdgeId, EdgeKind, EntityId, EntityKey, EntityKind, RepoName};
use atlas_engine::IndexMode;
use atlas_store::ReadTx;
use common::{Indexed, Selected, acme, classes, computed_routes, edges_of, entries, globex, id, min_confidence, pass_assertion, pass_facts, select, symbol};
use serde_json::json;
use toml::Value;

const LINK_KINDS: [&str; 5] = ["calls_endpoint", "supports_auth", "uses_auth", "implements_auth", "depends_on_package"];
const SCORED_KINDS: [&str; 3] = ["calls_endpoint", "supports_auth", "uses_auth"];
const CONFIDENCE_SLACK: f64 = 1e-6;

fn kind_of(entry: &Value) -> EdgeKind {
  entry.get("kind").and_then(Value::as_str).unwrap().parse().unwrap()
}

fn is_link_kind(entry: &Value) -> bool {
  LINK_KINDS.contains(&kind_of(entry).as_str())
}

fn certainty_failure(label: &str, entry: &Value, certainty: &atlas_core::Certainty) -> Option<String> {
  let allowed = classes(entry);
  if !allowed.is_empty() && !allowed.iter().any(|class| class == certainty.class().as_str()) {
    return Some(format!("{label}: certainty {:?} not in {allowed:?}", certainty));
  }
  let (Some(minimum), Some(confidence)) = (min_confidence(entry), certainty.confidence()) else {
    return None;
  };
  (f64::from(confidence.value()) + CONFIDENCE_SLACK < minimum).then(|| format!("{label}: confidence {} below {minimum}", confidence.value()))
}

fn check_edges(tx: &ReadTx<'_>, expected: &Value, failures: &mut Vec<String>) {
  for entry in entries(expected, "edge").into_iter().filter(|entry| is_link_kind(entry)) {
    let kind = kind_of(entry);
    let (from, to) = (id(&select(tx, &entry["from"])), id(&select(tx, &entry["to"])));
    let label = format!("{kind} {} -> {}", entry["from"], entry["to"]);
    match tx.edge(EdgeId::of(&kind, from, to, "")).unwrap() {
      Some(edge) => failures.extend(certainty_failure(&label, entry, &edge.certainty)),
      None if entry.get("required").and_then(Value::as_bool) == Some(false) => {}
      None => failures.push(format!("{label}: missing")),
    }
  }
}

fn check_forbidden(tx: &ReadTx<'_>, expected: &Value, failures: &mut Vec<String>) {
  for entry in entries(expected, "forbidden_edge").into_iter().filter(|entry| is_link_kind(entry)) {
    let kind = kind_of(entry);
    let from_repo = entry["from"].get("repo").and_then(Value::as_str).unwrap();
    let Some(from) = try_symbol(tx, &entry["from"], from_repo) else { continue };
    let allowed = classes(entry);
    let label = format!("forbidden {kind} {} -> {}", entry["from"], entry["to"]);
    let violating: Vec<_> = edges_of(tx, from, &kind)
      .into_iter()
      .filter(|edge| match select_target(tx, &entry["to"]) {
        Some(Selected::Id(target)) => edge.to == target,
        Some(Selected::Kind(target_kind)) => tx.entity(edge.to).unwrap().is_some_and(|entity| entity.kind == target_kind),
        None => false,
      })
      .filter(|edge| allowed.is_empty() || allowed.iter().any(|class| class == edge.certainty.class().as_str()))
      .collect();
    if !violating.is_empty() {
      failures.push(format!("{label}: found {violating:?}"));
    }
  }
}

fn in_project(tx: &ReadTx<'_>, repo: &str) -> bool {
  !tx.files_in_repo(&RepoName::new(repo).unwrap()).unwrap().is_empty()
}

fn try_symbol(tx: &ReadTx<'_>, selector: &Value, repo: &str) -> Option<EntityId> {
  in_project(tx, repo).then(|| id(&select(tx, selector)))
}

fn select_target(tx: &ReadTx<'_>, selector: &Value) -> Option<Selected> {
  match selector.get("repo").and_then(Value::as_str) {
    Some(repo) if !in_project(tx, repo) => None,
    _ => Some(select(tx, selector)),
  }
}

fn check_unmatched(tx: &ReadTx<'_>, expected: &Value, failures: &mut Vec<String>) {
  for entry in entries(expected, "unmatched_call") {
    let caller = symbol(tx, &entry["from"]);
    let linked = edges_of(tx, caller.id, &EdgeKind::CallsEndpoint);
    if !linked.is_empty() {
      failures.push(format!("unmatched call {} has calls_endpoint {linked:?}", entry["from"]));
    }
    let requests = edges_of(tx, caller.id, &EdgeKind::MakesRequest);
    let method = entry["method"].as_str().unwrap();
    let marked = requests.iter().any(|request| {
      let client = tx.entity(request.to).unwrap().unwrap();
      matches!(&client.key, EntityKey::ClientCall { method: client_method, .. } if client_method.as_str() == method)
        && pass_assertion(tx, client.id).is_some_and(|annotated| annotated.attrs.get("unmatched") == Some(&json!(true)))
    });
    if !marked {
      failures.push(format!("unmatched call {} has no client call marked unmatched", entry["from"]));
    }
  }
}

fn check_auth(tx: &ReadTx<'_>, expected: &Value, failures: &mut Vec<String>) {
  for entry in entries(expected, "auth") {
    let api_repo = RepoName::new(entry["api_repo"].as_str().unwrap()).unwrap();
    let supported: BTreeSet<String> = entry["supported"].as_array().unwrap().iter().map(|value| value.as_str().unwrap().to_owned()).collect();
    let found: BTreeSet<String> = tx
      .entities_by_kind(&EntityKind::AuthScheme, &Default::default())
      .unwrap()
      .into_iter()
      .filter(|scheme| scheme.repo.as_ref() == Some(&api_repo))
      .map(|scheme| scheme.name)
      .collect();
    if found != supported {
      failures.push(format!("auth {api_repo}: supported {found:?} != {supported:?}"));
    }
    let used = entry["used"].as_table().unwrap();
    for scheme in &supported {
      let key = EntityKey::AuthScheme {
        repo: api_repo.clone(),
        scheme: scheme.clone(),
      };
      let Some(summary) = tx.entity(EntityId::of(&key)).unwrap().filter(|merged| merged.attrs.contains_key("consumers")) else {
        failures.push(format!("auth {key}: no link summary"));
        continue;
      };
      let mut consumers: Vec<String> = used
        .get(scheme)
        .and_then(Value::as_array)
        .map(|repos| repos.iter().map(|repo| repo.as_str().unwrap().to_owned()).collect())
        .unwrap_or_default();
      consumers.sort();
      if summary.attrs["consumers"] != json!(consumers) || summary.attrs["used"] != json!(!consumers.is_empty()) {
        failures.push(format!("auth {key}: summary {:?} expected consumers {consumers:?}", summary.attrs));
      }
    }
  }
}

fn check_endpoint_auth(tx: &ReadTx<'_>, expected: &Value, api_repo: &str, failures: &mut Vec<String>) {
  for entry in entries(expected, "endpoint_auth") {
    let selector: Value = toml::toml! { kind = "api_endpoint" }.into();
    let mut selector = selector;
    let table = selector.as_table_mut().unwrap();
    table.insert("repo".into(), Value::String(api_repo.to_owned()));
    table.insert("method".into(), entry["method"].clone());
    table.insert("route".into(), entry["route"].clone());
    let endpoint = id(&select(tx, &selector));
    let edges = edges_of(tx, endpoint, &EdgeKind::SupportsAuth);
    let schemes: BTreeSet<String> = edges.iter().map(|edge| tx.entity(edge.to).unwrap().unwrap().name).collect();
    let wanted: BTreeSet<String> = entry["schemes"].as_array().unwrap().iter().map(|value| value.as_str().unwrap().to_owned()).collect();
    let label = format!("endpoint_auth {} {}", entry["method"], entry["route"]);
    if schemes != wanted {
      failures.push(format!("{label}: schemes {schemes:?} != {wanted:?}"));
    }
    for edge in &edges {
      if edge.attrs.get("combinator") != Some(&json!(entry["mode"].as_str().unwrap())) {
        failures.push(format!("{label}: combinator {:?}", edge.attrs.get("combinator")));
      }
      failures.extend(certainty_failure(&label, entry, &edge.certainty));
    }
  }
}

fn check_endpoints(tx: &ReadTx<'_>, expected: &Value, failures: &mut Vec<String>) {
  for entry in entries(expected, "entity").into_iter().filter(|entry| entry["kind"].as_str() == Some("api_endpoint")) {
    let endpoint = id(&select(tx, entry));
    match tx.entity(endpoint).unwrap() {
      Some(entity) => failures.extend(certainty_failure(&format!("endpoint {} {}", entry["method"], entry["route"]), entry, &entity.certainty)),
      None => failures.push(format!("endpoint {} {} missing", entry["method"], entry["route"])),
    }
  }
}

fn check_all(indexed: &Indexed, api_repo: Option<&str>) {
  let mut failures = Vec::new();
  indexed
    .engine
    .store()
    .read(|tx| -> Result<(), atlas_store::StoreError> {
      check_endpoints(tx, &indexed.expected, &mut failures);
      check_edges(tx, &indexed.expected, &mut failures);
      check_forbidden(tx, &indexed.expected, &mut failures);
      check_unmatched(tx, &indexed.expected, &mut failures);
      check_auth(tx, &indexed.expected, &mut failures);
      if let Some(api_repo) = api_repo {
        check_endpoint_auth(tx, &indexed.expected, api_repo, &mut failures);
      }
      Ok(())
    })
    .unwrap();
  assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

struct Score {
  expected: usize,
  emitted: usize,
  true_positives: usize,
}

fn score(indexed: &Indexed) -> BTreeMap<&'static str, Score> {
  let emitted = pass_facts(&indexed.engine).edges;
  indexed
    .engine
    .store()
    .read(|tx| -> Result<_, atlas_store::StoreError> {
      let mut table = BTreeMap::new();
      for kind_name in SCORED_KINDS {
        let kind: EdgeKind = kind_name.parse().unwrap();
        let expected: BTreeSet<(EntityId, EntityId)> = entries(&indexed.expected, "edge")
          .into_iter()
          .filter(|entry| kind_of(entry) == kind && entry.get("required").and_then(Value::as_bool) != Some(false))
          .map(|entry| (id(&select(tx, &entry["from"])), id(&select(tx, &entry["to"]))))
          .collect();
        let produced: BTreeSet<(EntityId, EntityId)> = emitted.iter().filter(|edge| edge.kind == kind).map(|edge| (edge.from, edge.to)).collect();
        table.insert(
          kind_name,
          Score {
            expected: expected.len(),
            emitted: produced.len(),
            true_positives: expected.intersection(&produced).count(),
          },
        );
      }
      Ok(table)
    })
    .unwrap()
}

#[test]
fn should_satisfy_every_acme_link_expectation() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = acme(scratch.path());
  check_all(&indexed, Some("acme-api"));
}

#[test]
fn should_score_full_precision_and_recall_on_acme_link_edges() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = acme(scratch.path());
  let table = score(&indexed);
  eprintln!("| kind | expected | emitted | tp | precision | recall |");
  for (kind, row) in &table {
    let precision = row.true_positives as f64 / row.emitted.max(1) as f64;
    let recall = row.true_positives as f64 / row.expected.max(1) as f64;
    eprintln!("| {kind} | {} | {} | {} | {precision:.2} | {recall:.2} |", row.expected, row.emitted, row.true_positives);
  }
  for (kind, row) in table {
    assert_eq!((row.true_positives, row.emitted), (row.expected, row.expected), "{kind}");
  }
}

#[test]
fn should_keep_the_users_me_call_a_weak_heuristic_and_check_health_unmatched() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = acme(scratch.path());
  indexed
    .engine
    .store()
    .read(|tx| -> Result<(), atlas_store::StoreError> {
      let use_me = symbol(tx, &toml::toml! { repo = "acme-web" path = "src/hooks/useMe.ts" symbol = "useMe" }.into());
      let links = edges_of(tx, use_me.id, &EdgeKind::CallsEndpoint);
      assert_eq!(links.len(), 1);
      assert_eq!(tx.entity(links[0].to)?.unwrap().key.to_string(), "endpoint:acme-api:GET /users/{id}");
      assert_eq!(links[0].attrs["rule"], json!("literal-on-param-segment"));
      let confidence = links[0].certainty.confidence().unwrap().value();
      assert!((0.4..0.5).contains(&confidence), "{confidence}");
      let health = symbol(tx, &toml::toml! { repo = "acme-web" path = "src/api/health.ts" symbol = "checkHealth" }.into());
      assert!(edges_of(tx, health.id, &EdgeKind::CallsEndpoint).is_empty());
      let client = EntityId::of(&EntityKey::ClientCall {
        repo: RepoName::new("acme-web").unwrap(),
        method: atlas_core::HttpMethod::Get,
        target: "{}/health".to_owned(),
      });
      let marked = pass_assertion(tx, client).unwrap();
      assert_eq!(marked.attrs["unmatched_reason"], json!("no_matching_endpoint"));
      Ok(())
    })
    .unwrap();
}

#[test]
fn should_derive_any_of_auth_for_the_composite_guard() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = acme(scratch.path());
  indexed
    .engine
    .store()
    .read(|tx| -> Result<(), atlas_store::StoreError> {
      let endpoint = EntityId::of(&EntityKey::Endpoint {
        repo: RepoName::new("acme-api").unwrap(),
        method: atlas_core::HttpMethod::Get,
        route: "/orders/{id}".to_owned(),
      });
      let mut edges: Vec<(String, serde_json::Value, serde_json::Value)> = edges_of(tx, endpoint, &EdgeKind::SupportsAuth)
        .into_iter()
        .map(|edge| {
          (
            tx.entity(edge.to).unwrap().unwrap().name,
            edge.attrs["via_guards"].clone(),
            edge.attrs["requirement"].clone(),
          )
        })
        .collect();
      edges.sort_by(|left, right| left.0.cmp(&right.0));
      assert_eq!(
        edges,
        [
          ("api_key".to_owned(), json!(["JwtOrApiKeyGuard -> ApiKeyGuard"]), json!({ "any_of": ["api_key", "bearer"] })),
          ("bearer".to_owned(), json!(["JwtOrApiKeyGuard -> JwtAuthGuard"]), json!({ "any_of": ["api_key", "bearer"] })),
        ]
      );
      Ok(())
    })
    .unwrap();
}

#[test]
fn should_label_permission_guards_as_authorization_only() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = acme(scratch.path());
  let facts = pass_facts(&indexed.engine);
  let endpoint = facts.entities.iter().find(|entity| entity.key.to_string() == "endpoint:acme-api:GET /users/{id}").unwrap();
  assert_eq!(endpoint.attrs["auth_requirement"], json!({ "all_of": ["bearer", "authorization_only:PermissionsGuard"] }));
  assert_eq!(endpoint.attrs["authorization_guards"], json!(["PermissionsGuard"]));
  assert_eq!(endpoint.attrs["unclassified_guards"], json!([]));
}

#[test]
fn should_expose_pass_attrs_through_the_merged_entity_view() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = acme(scratch.path());
  indexed
    .engine
    .store()
    .read(|tx| -> Result<(), atlas_store::StoreError> {
      let key = EntityKey::Endpoint {
        repo: RepoName::new("acme-api").unwrap(),
        method: atlas_core::HttpMethod::Get,
        route: "/users/{id}".to_owned(),
      };
      let merged = tx.entity_by_key(&key).unwrap().unwrap();
      assert_eq!(merged.attrs["auth_requirement"], json!({ "all_of": ["bearer", "authorization_only:PermissionsGuard"] }));
      let detector = tx
        .entity_assertions(merged.id)
        .unwrap()
        .into_iter()
        .find(|assertion| !assertion.fact.attrs.contains_key("auth_requirement"))
        .unwrap();
      assert_eq!(detector.fact.certainty, merged.certainty);
      Ok(())
    })
    .unwrap();
}

#[test]
fn should_expose_the_sdk_one_hop_path_from_web_callers_to_the_endpoint() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = acme(scratch.path());
  indexed
    .engine
    .store()
    .read(|tx| -> Result<(), atlas_store::StoreError> {
      let use_user = symbol(tx, &toml::toml! { repo = "acme-web" path = "src/hooks/useUser.ts" symbol = "useUser" }.into());
      let get_user = symbol(tx, &toml::toml! { repo = "acme-sdk" path = "src/client.ts" symbol = "AcmeClient.getUser" }.into());
      assert!(edges_of(tx, use_user.id, &EdgeKind::Calls).iter().any(|edge| edge.to == get_user.id));
      assert_eq!(edges_of(tx, get_user.id, &EdgeKind::MakesRequest).len(), 1);
      let endpoints: Vec<String> = edges_of(tx, get_user.id, &EdgeKind::CallsEndpoint)
        .iter()
        .map(|edge| tx.entity(edge.to).unwrap().unwrap().key.to_string())
        .collect();
      assert_eq!(endpoints, ["endpoint:acme-api:GET /users/{id}"]);
      assert!(edges_of(tx, use_user.id, &EdgeKind::CallsEndpoint).is_empty());
      Ok(())
    })
    .unwrap();
}

#[test]
fn should_link_cross_repo_packages_and_annotate_events() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = acme(scratch.path());
  let facts = pass_facts(&indexed.engine);
  let web = EntityId::of(&EntityKey::Repository {
    repo: RepoName::new("acme-web").unwrap(),
  });
  let sdk = EntityId::of(&EntityKey::Repository {
    repo: RepoName::new("acme-sdk").unwrap(),
  });
  let repo_edges: Vec<_> = facts.edges.iter().filter(|edge| edge.kind == EdgeKind::DependsOnPackage && edge.from == web).collect();
  assert_eq!(repo_edges.len(), 1);
  assert_eq!((repo_edges[0].to, repo_edges[0].certainty.clone()), (sdk, atlas_core::Certainty::Resolved));
  assert_eq!(repo_edges[0].attrs["to_package"], json!("pkg:npm:@acme/sdk"));
  let package_edge = facts
    .edges
    .iter()
    .find(|edge| edge.kind == EdgeKind::DependsOnPackage && edge.attrs.get("cross_repo") == Some(&json!(true)))
    .unwrap();
  assert_eq!(package_edge.attrs["target_repo"], json!("acme-sdk"));
  assert_eq!(package_edge.certainty, atlas_core::Certainty::Syntactic);
  let event = facts.entities.iter().find(|entity| entity.kind == EntityKind::Event).unwrap();
  assert_eq!(event.name, "order.created");
  assert_eq!(
    (
      &event.attrs["producers"],
      &event.attrs["consumers"],
      &event.attrs["producer_repos"],
      &event.attrs["consumer_repos"],
      &event.attrs["cross_repo"],
    ),
    (&json!(1), &json!(1), &json!(["acme-api"]), &json!(["acme-billing"]), &json!(true))
  );
  assert!(!event.attrs.contains_key("orphan"));
  let status = facts.entities.iter().find(|entity| entity.kind.as_str() == "link_status").unwrap();
  assert_eq!(status.attrs["unsupported"], json!(["env definition files"]));
  assert_eq!((&status.attrs["client_calls"], &status.attrs["client_calls_unmatched"]), (&json!(7), &json!(1)));
}

#[test]
fn should_satisfy_globex_expectations_and_never_link_across_projects() {
  let scratch = tempfile::tempdir().unwrap();
  let acme = acme(scratch.path());
  let globex = globex(scratch.path());
  check_all(&globex, None);
  for (indexed, repos) in [
    (&acme, &["acme-api", "acme-sdk", "acme-web", "acme-billing"][..]),
    (&globex, &["globex-api", "globex-worker"][..]),
  ] {
    let facts = pass_facts(&indexed.engine);
    let owners: BTreeSet<String> = indexed
      .engine
      .store()
      .read(|tx| -> Result<BTreeSet<String>, atlas_store::StoreError> {
        let mut found = BTreeSet::new();
        for edge in &facts.edges {
          for end in [edge.from, edge.to] {
            if let Some(repo) = tx.entity(end)?.and_then(|entity| entity.repo) {
              found.insert(repo.to_string());
            }
          }
        }
        Ok(found)
      })
      .unwrap();
    assert!(owners.iter().all(|repo| repos.contains(&repo.as_str())), "{owners:?}");
  }
  let globex_facts = pass_facts(&globex.engine);
  assert!(
    globex_facts
      .edges
      .iter()
      .all(|edge| edge.kind != EdgeKind::CallsEndpoint && edge.kind != EdgeKind::UsesAuth)
  );
  let event = globex_facts.entities.iter().find(|entity| entity.kind == EntityKind::Event).unwrap();
  assert_eq!(event.attrs["cross_repo"], json!(true));
  assert!(!event.attrs.contains_key("orphan"));
}

#[test]
fn should_compose_mounted_express_routes_and_match_clients_against_them() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = computed_routes(scratch.path());
  check_all(&indexed, None);
  let facts = pass_facts(&indexed.engine);
  let mut composed: Vec<(String, String)> = facts
    .entities
    .iter()
    .filter(|entity| entity.kind == EntityKind::ApiEndpoint)
    .map(|entity| (entity.key.to_string(), entity.attrs["composed_from"].as_str().unwrap().to_owned()))
    .collect();
  composed.sort();
  assert_eq!(
    composed,
    [
      (
        "endpoint:computed-routes-ts:GET /api/items/{id}".to_owned(),
        "endpoint:computed-routes-ts:GET /{id}".to_owned()
      ),
      ("endpoint:computed-routes-ts:POST /api/items".to_owned(), "endpoint:computed-routes-ts:POST /".to_owned()),
    ]
  );
  let exposures = facts.edges.iter().filter(|edge| edge.kind == EdgeKind::ExposesEndpoint).count();
  assert_eq!(exposures, 2);
}

#[test]
fn should_produce_identical_facts_on_every_run() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = acme(scratch.path());
  let first = pass_facts(&indexed.engine);
  let (once, twice, elapsed) = indexed
    .engine
    .store()
    .read(|tx| -> Result<_, atlas_link::LinkError> {
      let started = Instant::now();
      let once = atlas_link::link(tx, &indexed.project)?;
      let elapsed = started.elapsed();
      Ok((once, atlas_link::link(tx, &indexed.project)?, elapsed))
    })
    .unwrap();
  eprintln!("link pass on acme took {elapsed:?}");
  assert_eq!(serde_json::to_string(&once.edges).unwrap(), serde_json::to_string(&twice.edges).unwrap());
  assert_eq!(serde_json::to_string(&once.entities).unwrap(), serde_json::to_string(&twice.entities).unwrap());
  indexed.engine.index(IndexMode::Full).unwrap();
  assert_eq!(pass_facts(&indexed.engine), first);
}
