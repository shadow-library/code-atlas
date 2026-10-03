mod common;

use atlas_detect::ConfigEnvDetector;
use common::{analyze, lines, render_repo, render_source, run};

#[test]
fn should_detect_acme_api_env_and_config_reads() {
  assert_eq!(
    render_repo(&ConfigEnvDetector, "acme/acme-api"),
    lines(&[
      "config:acme-api:API_KEYS [heuristic 0.80]",
      "env:acme-api:DATABASE_URL [syntactic]",
      "env:acme-api:JWT_SECRET [syntactic]",
      "env:acme-api:PORT [syntactic]",
      "reads_config ApiKeyGuard.canActivate -> config:acme-api:API_KEYS [heuristic 0.80]",
      "reads_env AuthModule -> env:acme-api:JWT_SECRET [syntactic]",
      "reads_env JwtAuthGuard.canActivate -> env:acme-api:JWT_SECRET [syntactic]",
      "reads_env bootstrap -> env:acme-api:PORT [syntactic]",
      "reads_env databaseConfig -> env:acme-api:DATABASE_URL [syntactic]",
    ])
  );
}

#[test]
fn should_detect_env_reads_in_acme_web_and_acme_billing() {
  assert_eq!(
    render_repo(&ConfigEnvDetector, "acme/acme-web"),
    lines(&["env:acme-web:VITE_API_URL [syntactic]", "reads_env API_URL -> env:acme-web:VITE_API_URL [syntactic]"])
  );
  assert_eq!(
    render_repo(&ConfigEnvDetector, "acme/acme-billing"),
    lines(&[
      "env:acme-billing:ACME_API_URL [syntactic]",
      "env:acme-billing:BILLING_API_KEY [syntactic]",
      "env:acme-billing:DATABASE_URL [syntactic]",
      "reads_env AcmeApiClient::from_env -> env:acme-billing:ACME_API_URL [syntactic]",
      "reads_env AcmeApiClient::from_env -> env:acme-billing:BILLING_API_KEY [syntactic]",
      "reads_env main -> env:acme-billing:DATABASE_URL [syntactic]",
    ])
  );
}

#[test]
fn should_detect_globex_env_reads_attributed_to_module_constants() {
  assert_eq!(
    render_repo(&ConfigEnvDetector, "globex/globex-api"),
    lines(&[
      "env:globex-api:DATABASE_URL [syntactic]",
      "env:globex-api:GLOBEX_KEY [syntactic]",
      "env:globex-api:PORT [syntactic]",
      "reads_env DATABASE_URL -> env:globex-api:DATABASE_URL [syntactic]",
      "reads_env PORT -> env:globex-api:PORT [syntactic]",
      "reads_env requireGlobexKey -> env:globex-api:GLOBEX_KEY [syntactic]",
    ])
  );
  assert_eq!(
    render_repo(&ConfigEnvDetector, "globex/globex-worker"),
    lines(&["env:globex-worker:DATABASE_URL [syntactic]", "reads_env main -> env:globex-worker:DATABASE_URL [syntactic]"])
  );
}

#[test]
fn should_read_destructured_and_indexed_env_names_and_flag_dynamic_ones() {
  let source = "const { HOST, PORT: port } = process.env;\nexport function key(name: string) { return process.env['API_KEY'] ?? process.env[name]; }\n";
  let analyzed = analyze("svc", "src/env.ts", source);
  let (detection, diagnostics) = run(&ConfigEnvDetector, &analyzed);
  assert_eq!(
    common::sorted(common::render(&analyzed.facts, &detection)),
    lines(&[
      "env:svc:API_KEY [syntactic]",
      "env:svc:HOST [syntactic]",
      "env:svc:PORT [syntactic]",
      "reads_env HOST -> env:svc:HOST [syntactic]",
      "reads_env port -> env:svc:PORT [syntactic]",
      "reads_env key -> env:svc:API_KEY [syntactic]",
    ])
  );
  assert_eq!(diagnostics.iter().map(|diagnostic| diagnostic.code.as_str()).collect::<Vec<_>>(), ["env-dynamic-name"]);
}

#[test]
fn should_detect_rust_env_functions_and_macros_but_not_dynamic_names() {
  let source = "use std::env;\nuse std::env::var_os;\n\
pub fn load(name: &str) {\n  let a = env::var(\"A\");\n  let b = var_os(\"B\");\n  let c = env!(\"C\");\n  let d = option_env!(\"D\");\n  let e = std::env::var(name);\n  let f = dotenvy::var(\"F\");\n}\n";
  let analyzed = analyze("svc", "src/config.rs", source);
  let (detection, diagnostics) = run(&ConfigEnvDetector, &analyzed);
  let edges: Vec<String> = common::render(&analyzed.facts, &detection)
    .into_iter()
    .filter(|line| line.starts_with("reads_env"))
    .collect();
  assert_eq!(
    common::sorted(edges),
    lines(&[
      "reads_env load -> env:svc:A [syntactic]",
      "reads_env load -> env:svc:B [syntactic]",
      "reads_env load -> env:svc:C [syntactic]",
      "reads_env load -> env:svc:D [syntactic]",
      "reads_env load -> env:svc:F [syntactic]",
    ])
  );
  assert_eq!(diagnostics.len(), 1);
}

#[test]
fn should_ignore_config_getters_on_unrelated_receivers() {
  let source = "export function f(cache: Map<string, string>) { return cache.get('API_KEYS') ?? settings.get('X'); }\n";
  assert!(render_source(&ConfigEnvDetector, "svc", "src/f.ts", source).is_empty());
}

#[test]
fn should_not_report_env_writes_or_deletes_as_reads() {
  let source = "export function setup() {\n  process.env.NODE_ENV = 'test';\n  process.env['X'] = '1';\n  delete process.env.Y;\n  process.env.Z ??= 'z';\n  process.env.W += 'w';\n  if (process.env.A === 'b' || process.env['B'] == 'c') {}\n}\n";
  assert_eq!(
    render_source(&ConfigEnvDetector, "svc", "src/setup.ts", source),
    lines(&[
      "env:svc:A [syntactic]",
      "env:svc:B [syntactic]",
      "reads_env setup -> env:svc:A [syntactic]",
      "reads_env setup -> env:svc:B [syntactic]"
    ])
  );
}

#[test]
fn should_mark_env_reads_inside_test_code() {
  let in_test = |repo: &str, path: &str, source: &str| -> Vec<(String, bool)> {
    let analyzed = analyze(repo, path, source);
    let (detection, _) = run(&ConfigEnvDetector, &analyzed);
    let mut marked: Vec<(String, bool)> = detection
      .edges
      .iter()
      .map(|edge| (common::node(&analyzed.facts, &edge.to), edge.attrs.get("in_test") == Some(&serde_json::json!(true))))
      .collect();
    marked.sort();
    marked
  };
  assert_eq!(
    in_test("svc", "src/users.spec.ts", "describe('x', () => { process.env.SPEC; });\n"),
    [("env:svc:SPEC".to_owned(), true)]
  );
  let rust = "pub fn load() { std::env::var(\"PROD\"); }\n#[cfg(test)]\nmod tests {\n  fn helper() { std::env::var(\"HELPER\"); }\n  #[test]\n  fn reads() { std::env::var(\"CASE\"); }\n}\n";
  assert_eq!(
    in_test("svc", "src/lib.rs", rust),
    [("env:svc:CASE".to_owned(), true), ("env:svc:HELPER".to_owned(), true), ("env:svc:PROD".to_owned(), false)]
  );
}
