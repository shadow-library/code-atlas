mod common;

use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{ContentHash, Detector};
use atlas_detect::{DECLARED_ROUTES_VERSION, DeclaredRoutesDetector, DecoratorFamily, DiagnosingDetector, NestEndpointDetector, families_digest};

use common::{Analyzed, analyze, render, run, sorted};

const PINNED: (u32, &str, &str) = (
  5,
  "5c055ee4b8c96ddda27d6e52b5748f56d38252c4e8c1560be0ea9b9d06b0450f",
  "475e911908c1d1890f64a1211c0b554028685e4d6dc68bcbf24cca111474ae4c",
);

fn corpus() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/family-corpus")
}

fn acme() -> DecoratorFamily {
  let text = fs::read_to_string(corpus().join("acme-http.toml")).unwrap();
  let parsed = DecoratorFamily::parse("acme-http", &text).unwrap();
  assert_eq!(parsed.warnings, Vec::<String>::new());
  parsed.family
}

fn acme_server() -> DecoratorFamily {
  let text = fs::read_to_string(corpus().join("acme-server.toml")).unwrap();
  DecoratorFamily::parse("acme-server", &text).unwrap().family
}

fn acme_gate() -> DecoratorFamily {
  let text = fs::read_to_string(corpus().join("acme-gate.toml")).unwrap();
  DecoratorFamily::parse("acme-gate", &text).unwrap().family
}

fn detector() -> DeclaredRoutesDetector {
  DeclaredRoutesDetector::new(vec![acme()])
}

fn corpus_files(repo: &str) -> Vec<(String, Analyzed)> {
  let root = corpus().join(repo).join("src");
  let mut paths: Vec<PathBuf> = fs::read_dir(&root).unwrap().flatten().map(|entry| entry.path()).collect();
  paths.sort();
  paths
    .into_iter()
    .map(|path| {
      let relative = format!("src/{}", path.file_name().unwrap().to_string_lossy());
      let source = fs::read_to_string(&path).unwrap();
      let analyzed = analyze(repo, &relative, &source);
      (relative, analyzed)
    })
    .collect()
}

fn render_corpus(detector: &dyn DiagnosingDetector, repo: &str) -> Vec<String> {
  let mut lines = Vec::new();
  for (_, analyzed) in corpus_files(repo) {
    let (detection, _) = run(detector, &analyzed);
    lines.extend(render(&analyzed.facts, &detection));
  }
  sorted(lines)
}

#[test]
fn should_detect_every_route_of_an_import_anchored_family() {
  let lines = render_corpus(&detector(), "orders");
  assert_eq!(
    lines,
    [
      "endpoint:orders:DELETE /orders/{id} [resolved]",
      "endpoint:orders:DELETE /orders/{id}/purge [resolved]",
      "endpoint:orders:GET /health [syntactic]",
      "endpoint:orders:GET /health/ready [syntactic]",
      "endpoint:orders:GET /orders [resolved]",
      "endpoint:orders:GET /orders/{id} [resolved]",
      "endpoint:orders:POST /orders [resolved]",
      "endpoint:orders:POST /orders/items/{} [heuristic 0.40]",
      "exposes_endpoint HealthController.ready -> endpoint:orders:GET /health/ready [syntactic]",
      "exposes_endpoint HealthController.status -> endpoint:orders:GET /health [syntactic]",
      "exposes_endpoint OrdersController.create -> endpoint:orders:POST /orders [resolved]",
      "exposes_endpoint OrdersController.dynamic -> endpoint:orders:POST /orders/items/{} [heuristic 0.40]",
      "exposes_endpoint OrdersController.get -> endpoint:orders:GET /orders/{id} [resolved]",
      "exposes_endpoint OrdersController.list -> endpoint:orders:GET /orders [resolved]",
      "exposes_endpoint OrdersController.remove -> endpoint:orders:DELETE /orders/{id} [resolved]",
      "exposes_endpoint OrdersController.remove -> endpoint:orders:DELETE /orders/{id}/purge [resolved]",
      "other:http_family_prefix:orders:acme-http:/api/v2 [syntactic]",
      "permission:orders:orders:write [syntactic]",
      "requires_guard endpoint:orders:DELETE /orders/{id} -> ?AdminGuard [resolved]",
      "requires_guard endpoint:orders:DELETE /orders/{id} -> ?SessionGuard [resolved]",
      "requires_guard endpoint:orders:DELETE /orders/{id}/purge -> ?AdminGuard [resolved]",
      "requires_guard endpoint:orders:DELETE /orders/{id}/purge -> ?SessionGuard [resolved]",
      "requires_guard endpoint:orders:GET /orders -> ?SessionGuard [resolved]",
      "requires_guard endpoint:orders:GET /orders/{id} -> ?SessionGuard [resolved]",
      "requires_guard endpoint:orders:POST /orders -> ?SessionGuard [resolved]",
      "requires_guard endpoint:orders:POST /orders/items/{} -> ?SessionGuard [heuristic 0.40]",
      "requires_permission OrdersController.create -> permission:orders:orders:write [syntactic]",
    ]
  );
}

#[test]
fn should_record_family_attrs_on_each_endpoint() {
  let (_, analyzed) = corpus_files("orders").into_iter().find(|(path, _)| path == "src/orders.controller.ts").unwrap();
  let (detection, _) = run(&detector(), &analyzed);
  let attrs: Vec<String> = detection
    .entities
    .iter()
    .map(|entity| format!("{} {}", entity.key, serde_json::to_string(&entity.attrs).unwrap()))
    .collect();
  assert_eq!(
    attrs,
    [
      r#"endpoint:orders:GET /orders {"controller":"OrdersController","framework":"acme-http","handler":"OrdersController.list","route_source":"constant"}"#,
      r#"endpoint:orders:GET /orders/{id} {"controller":"OrdersController","framework":"acme-http","handler":"OrdersController.get","headers":["x-tenant"],"params":["id"],"route_source":"constant"}"#,
      r#"endpoint:orders:POST /orders {"body_type":"CreateOrder","controller":"OrdersController","framework":"acme-http","handler":"OrdersController.create","route_source":"constant"}"#,
      r#"permission:orders:orders:write {"decorator":"RequirePermission"}"#,
      r#"endpoint:orders:DELETE /orders/{id} {"controller":"OrdersController","framework":"acme-http","handler":"OrdersController.remove","params":["id"],"route_source":"constant"}"#,
      r#"endpoint:orders:DELETE /orders/{id}/purge {"controller":"OrdersController","framework":"acme-http","handler":"OrdersController.remove","params":["id"],"route_source":"constant"}"#,
      r#"endpoint:orders:POST /orders/items/{} {"controller":"OrdersController","framework":"acme-http","handler":"OrdersController.dynamic","route_source":"dynamic"}"#,
    ]
  );
}

#[test]
fn should_ignore_lookalike_decorators_from_other_packages_or_local_definitions() {
  assert_eq!(render_corpus(&detector(), "lookalike"), Vec::<String>::new());
}

#[test]
fn should_leave_nest_detection_to_the_nest_detector() {
  let lines = render_corpus(&NestEndpointDetector, "lookalike");
  assert_eq!(
    lines,
    [
      "endpoint:lookalike:GET /nest [syntactic]",
      "exposes_endpoint NestController.list -> endpoint:lookalike:GET /nest [syntactic]"
    ]
  );
}

#[test]
fn should_version_the_detector_by_its_families() {
  let text = fs::read_to_string(corpus().join("acme-http.toml")).unwrap();
  let one = detector();
  let changed = DeclaredRoutesDetector::new(vec![
    DecoratorFamily::parse("acme-http", &text.replace("Remove = \"DELETE\"", "Remove = \"PUT\""))
      .unwrap()
      .family,
  ]);
  let second = DecoratorFamily::parse("acme-rpc", &text).unwrap().family;

  assert_eq!(one.producer(), detector().producer());
  assert_eq!(one.producer().name, "atlas-detect.http-endpoints-declared");
  assert_ne!(one.producer().version, changed.producer().version);
  assert_ne!(one.producer().version, DeclaredRoutesDetector::new(vec![acme(), second]).producer().version);
  assert_eq!(DeclaredRoutesDetector::new(vec![acme(), acme()]).producer(), one.producer());
}

#[test]
fn should_change_declared_route_output_over_the_corpus_only_with_a_version_bump() {
  let mut inputs = Vec::new();
  let mut outputs = Vec::new();
  for (family, repos) in [
    (acme(), ["imports", "lookalike", "orders"].as_slice()),
    (acme_server(), ["projects"].as_slice()),
    (acme_gate(), ["gate"].as_slice()),
  ] {
    let detector = DeclaredRoutesDetector::new(vec![family.clone()]);
    inputs.extend(families_digest(std::slice::from_ref(&family)).to_string().into_bytes());
    for (repo, path, analyzed) in repos
      .iter()
      .flat_map(|repo| corpus_files(repo).into_iter().map(move |(path, analyzed)| (*repo, path, analyzed)))
    {
      inputs.extend(serde_json::to_vec(&analyzed.facts).unwrap());
      inputs.push(b'\n');
      let (detection, diagnostics) = run(&detector, &analyzed);
      let mut diagnostics: Vec<String> = diagnostics.iter().map(|diagnostic| serde_json::to_string(diagnostic).unwrap()).collect();
      diagnostics.sort();
      outputs.extend(format!("{repo}\0{path}\0").into_bytes());
      outputs.extend(serde_json::to_vec(&detection).unwrap());
      outputs.extend(serde_json::to_vec(&diagnostics).unwrap());
      outputs.push(b'\n');
    }
  }
  let actual = (DECLARED_ROUTES_VERSION, ContentHash::of(&inputs).to_string(), ContentHash::of(&outputs).to_string());
  let advice = match actual.1 == PINNED.1 {
    false => "the family corpus, the fixture family or the analyzers changed, so re-pin",
    true if actual.0 == PINNED.0 => "the declared-routes output over unchanged inputs changed: bump DECLARED_ROUTES_VERSION, then re-pin",
    true => "DECLARED_ROUTES_VERSION changed: re-pin",
  };
  assert_eq!((actual.0, actual.1.as_str(), actual.2.as_str()), PINNED, "{advice}");
}

#[test]
fn should_read_the_last_option_key_from_inline_and_same_file_const_objects_and_cap_keys_a_spread_may_override() {
  let (_, analyzed) = corpus_files("imports").into_iter().next().unwrap();
  let (detection, diagnostics) = run(&detector(), &analyzed);
  assert_eq!(
    sorted(render(&analyzed.facts, &detection)),
    [
      "endpoint:imports:DELETE /imports [syntactic]",
      "endpoint:imports:GET /imports/status/{jobId} [resolved]",
      "endpoint:imports:PATCH /imports/touch [syntactic]",
      "endpoint:imports:POST /imports/archive [heuristic 0.50]",
      "endpoint:imports:POST /imports/run [resolved]",
      "endpoint:imports:PUT /imports/resume/{jobId} [heuristic 0.80]",
      "endpoint:imports:PUT /imports/retry/{jobId} [heuristic 0.80]",
      "exposes_endpoint ImportsController.archive -> endpoint:imports:POST /imports/archive [heuristic 0.50]",
      "exposes_endpoint ImportsController.cancelAll -> endpoint:imports:DELETE /imports [syntactic]",
      "exposes_endpoint ImportsController.retry -> endpoint:imports:PUT /imports/resume/{jobId} [heuristic 0.80]",
      "exposes_endpoint ImportsController.retry -> endpoint:imports:PUT /imports/retry/{jobId} [heuristic 0.80]",
      "exposes_endpoint ImportsController.run -> endpoint:imports:POST /imports/run [resolved]",
      "exposes_endpoint ImportsController.status -> endpoint:imports:GET /imports/status/{jobId} [resolved]",
      "exposes_endpoint ImportsController.touch -> endpoint:imports:PATCH /imports/touch [syntactic]",
    ]
  );
  let route_sources: Vec<String> = detection.entities.iter().map(|entity| format!("{} {}", entity.key, entity.attrs["route_source"])).collect();
  assert_eq!(
    route_sources,
    [
      "endpoint:imports:POST /imports/run \"constant\"",
      "endpoint:imports:GET /imports/status/{jobId} \"constant\"",
      "endpoint:imports:PUT /imports/retry/{jobId} \"literal\"",
      "endpoint:imports:PUT /imports/resume/{jobId} \"literal\"",
      "endpoint:imports:DELETE /imports \"literal\"",
      "endpoint:imports:PATCH /imports/touch \"literal\"",
      "endpoint:imports:POST /imports/archive \"literal\"",
    ]
  );
  let messages: Vec<(&str, &str)> = diagnostics.iter().map(|diagnostic| (diagnostic.code.as_str(), diagnostic.message.as_str())).collect();
  assert_eq!(
    messages,
    [
      (
        "route-options-unresolved",
        "@Route on ImportsController.exported takes options that are neither an object literal nor a const object literal declared in this file"
      ),
      (
        "route-options-unresolved",
        "@Route on ImportsController.noMethod has no HTTP method that can be determined statically under `method`"
      ),
      (
        "route-options-spread",
        "@Route on ImportsController.archive takes `method`, `path` from options a spread may override"
      ),
    ]
  );
}

#[test]
fn should_survive_a_deeply_nested_option_const_on_a_two_mebibyte_stack() {
  let depth = 20_000;
  let source = format!(
    "import {{ Api, Route }} from '@acme/http';\n\nconst DEEP = {{ method: 'GET', path: 'deep', extra: {}0{} }};\n\n@Api('deep')\nexport class DeepController {{\n  @Route(DEEP)\n  read() {{\n    return 0;\n  }}\n}}\n",
    "{ a: ".repeat(depth),
    " }".repeat(depth)
  );
  let analyzed = analyze("deep", "src/deep.controller.ts", &source);
  let lines = std::thread::Builder::new()
    .stack_size(2 * 1024 * 1024)
    .spawn(move || {
      let (detection, _) = run(&detector(), &analyzed);
      sorted(render(&analyzed.facts, &detection))
    })
    .unwrap()
    .join()
    .unwrap();
  assert_eq!(
    lines,
    [
      "endpoint:deep:GET /deep/deep [resolved]",
      "exposes_endpoint DeepController.read -> endpoint:deep:GET /deep/deep [resolved]",
    ]
  );
}
