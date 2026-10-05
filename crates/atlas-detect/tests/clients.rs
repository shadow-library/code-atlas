mod common;

use atlas_core::{Detection, EdgeKind};
use atlas_detect::HttpClientDetector;
use common::{analyze, lines, render_file, render_repo, render_source, run};
use serde_json::{Value, json};

fn request_attrs(detection: &Detection, client: &str) -> Value {
  let edge = detection
    .edges
    .iter()
    .find(|edge| edge.kind == EdgeKind::MakesRequest && matches!(&edge.to, atlas_core::NodeRef::Key(key) if key.to_string() == client))
    .unwrap_or_else(|| panic!("no makes_request edge to {client}"));
  serde_json::to_value(&edge.attrs).unwrap()
}

fn fixture_detection(repo_dir: &str, path: &str) -> Detection {
  let analyzed = common::fixture_files(repo_dir).into_iter().find(|(file, _)| file == path).unwrap().1;
  run(&HttpClientDetector, &analyzed).0
}

#[test]
fn should_detect_the_acme_sdk_fetch_calls_with_patterns_and_methods() {
  assert_eq!(
    render_file(&HttpClientDetector, "acme/acme-sdk", "src/client.ts"),
    lines(&[
      "client:acme-sdk:GET {}/orders/{} [heuristic 0.70]",
      "client:acme-sdk:GET {}/users/{} [heuristic 0.70]",
      "client:acme-sdk:PATCH {}/users/{} [heuristic 0.70]",
      "client:acme-sdk:POST {}/orders [heuristic 0.70]",
      "makes_request AcmeClient.createOrder -> client:acme-sdk:POST {}/orders [heuristic 0.70]",
      "makes_request AcmeClient.getOrder -> client:acme-sdk:GET {}/orders/{} [heuristic 0.70]",
      "makes_request AcmeClient.getUser -> client:acme-sdk:GET {}/users/{} [heuristic 0.70]",
      "makes_request AcmeClient.updateUser -> client:acme-sdk:PATCH {}/users/{} [heuristic 0.70]",
    ])
  );
}

#[test]
fn should_summarise_auth_headers_built_by_a_same_class_helper() {
  let detection = fixture_detection("acme/acme-sdk", "src/client.ts");
  let attrs = request_attrs(&detection, "client:acme-sdk:GET {}/users/{}");
  assert_eq!(attrs["auth_hint"], json!(["api_key:x-api-key", "bearer"]));
  assert_eq!(attrs["auth_hint_source"], json!("AcmeClient.headers"));
  assert_eq!(attrs["base_source"], json!("field"));
  assert_eq!(attrs["method_source"], json!("default"));
  assert_eq!(request_attrs(&detection, "client:acme-sdk:PATCH {}/users/{}")["method_source"], json!("literal"));
}

#[test]
fn should_detect_acme_web_direct_fetches_and_the_sdk_auth_option() {
  assert_eq!(
    render_repo(&HttpClientDetector, "acme/acme-web"),
    lines(&[
      "client:acme-web:GET {}/health [heuristic 0.70]",
      "client:acme-web:GET {}/users/me [heuristic 0.70]",
      "makes_request checkHealth -> client:acme-web:GET {}/health [heuristic 0.70]",
      "makes_request useMe -> client:acme-web:GET {}/users/me [heuristic 0.70]",
      "references acme -> ?AcmeClient [heuristic 0.60]",
    ])
  );
  let me = fixture_detection("acme/acme-web", "src/hooks/useMe.ts");
  let attrs = request_attrs(&me, "client:acme-web:GET {}/users/me");
  assert_eq!(attrs["auth_hint"], json!(["bearer"]));
  assert_eq!(attrs["base_source"], json!("ident:API_URL"));
  let health = fixture_detection("acme/acme-web", "src/api/health.ts");
  assert_eq!(request_attrs(&health, "client:acme-web:GET {}/health").get("auth_hint"), None);
  let option = fixture_detection("acme/acme-web", "src/api/client.ts");
  assert_eq!(option.edges[0].attrs["uses_auth_option"], json!("bearer"));
  assert_eq!(option.edges[0].attrs["option"], json!("auth.type"));
}

#[test]
fn should_detect_the_acme_billing_reqwest_call_through_a_local_format_binding() {
  assert_eq!(
    render_repo(&HttpClientDetector, "acme/acme-billing"),
    lines(&[
      "client:acme-billing:GET {}/orders/{} [heuristic 0.70]",
      "makes_request AcmeApiClient::get_order -> client:acme-billing:GET {}/orders/{} [heuristic 0.70]",
    ])
  );
  let attrs = request_attrs(&fixture_detection("acme/acme-billing", "src/acme_client.rs"), "client:acme-billing:GET {}/orders/{}");
  assert_eq!(attrs["auth_hint"], json!(["api_key:x-api-key"]));
  assert_eq!(attrs["base_source"], json!("field"));
  assert_eq!(attrs["library"], json!("reqwest"));
}

#[test]
fn should_detect_computed_route_client_calls_and_report_fully_dynamic_urls() {
  assert_eq!(
    render_repo(&HttpClientDetector, "adversarial/computed-routes-ts"),
    lines(&[
      "client:computed-routes-ts:GET /api/items/{} [syntactic]",
      "client:computed-routes-ts:GET /api/status [syntactic]",
      "client:computed-routes-ts:POST /api/items [syntactic]",
      "makes_request createItem -> client:computed-routes-ts:POST /api/items [syntactic]",
      "makes_request getItemConcat -> client:computed-routes-ts:GET /api/items/{} [resolved]",
      "makes_request getItemTemplate -> client:computed-routes-ts:GET /api/items/{} [syntactic]",
      "makes_request getStatus -> client:computed-routes-ts:GET /api/status [syntactic]",
    ])
  );
  let analyzed = common::fixture_files("adversarial/computed-routes-ts")
    .into_iter()
    .find(|(path, _)| path == "src/client.ts")
    .unwrap()
    .1;
  let (_, diagnostics) = run(&HttpClientDetector, &analyzed);
  let reported: Vec<(&str, u32, &str)> = diagnostics
    .iter()
    .map(|diagnostic| (diagnostic.code.as_str(), diagnostic.span.unwrap().start_line, diagnostic.message.as_str()))
    .collect();
  assert_eq!(
    reported,
    [("http.dynamic_url", 20, "axios call in getDynamic has a fully dynamic URL and was not analysed")]
  );
}

#[test]
fn should_not_create_a_shared_entity_for_fetches_of_dynamic_urls() {
  let source = "export function load(url: string, base: string, path: string) {\n  fetch(url);\n  fetch(`${base}${path}`);\n  fetch(`${base}/`);\n}\n";
  assert!(render_source(&HttpClientDetector, "web", "src/load.ts", source).is_empty());
  let analyzed = analyze("web", "src/load.ts", source);
  let (_, diagnostics) = run(&HttpClientDetector, &analyzed);
  assert_eq!(diagnostics.iter().filter(|diagnostic| diagnostic.code == "http.dynamic_url").count(), 3);
}

#[test]
fn should_expand_one_level_same_file_wrappers_at_their_literal_call_sites() {
  let source = "const KEY = 'k';\n\
export class ShopClient {\n\
  constructor(private readonly baseUrl: string) {}\n\
  private request(method: string, path: string) {\n\
    return fetch(`${this.baseUrl}${path}`, { method, headers: { 'x-api-key': KEY } });\n\
  }\n\
  getItem(id: string) { return this.request('GET', `/items/${id}`); }\n\
  deleteItem(id: string) { return this.request('DELETE', '/items/' + id); }\n\
  anything(path: string) { return this.request(verb(), path); }\n\
}\n";
  assert_eq!(
    render_source(&HttpClientDetector, "sdk", "src/client.ts", source),
    lines(&[
      "client:sdk:DELETE {}/items/{} [heuristic 0.60]",
      "client:sdk:GET {}/items/{} [heuristic 0.60]",
      "makes_request ShopClient.deleteItem -> client:sdk:DELETE {}/items/{} [heuristic 0.60]",
      "makes_request ShopClient.getItem -> client:sdk:GET {}/items/{} [heuristic 0.60]",
    ])
  );
  let analyzed = analyze("sdk", "src/client.ts", source);
  let (detection, _) = run(&HttpClientDetector, &analyzed);
  let attrs = request_attrs(&detection, "client:sdk:GET {}/items/{}");
  assert_eq!(attrs["auth_hint"], json!(["api_key:x-api-key"]));
  assert_eq!(attrs["auth_hint_source"], json!("ShopClient.request"));
}

#[test]
fn should_keep_a_wrapper_request_when_its_callers_add_no_literal_url_text() {
  let source = "import { Controller, Get, Param } from '@nestjs/common';\n\
function relayEcho(id: string) {\n\
  return fetch('/echo/' + id);\n\
}\n\
@Controller('echo')\n\
export class EchoController {\n\
  @Get(':id')\n\
  echo(@Param('id') id: string) {\n\
    return relayEcho(id);\n\
  }\n\
}\n";
  assert_eq!(
    render_source(&HttpClientDetector, "api", "src/echo.controller.ts", source),
    lines(&["client:api:GET /echo/{} [resolved]", "makes_request relayEcho -> client:api:GET /echo/{} [resolved]"])
  );
}

#[test]
fn should_synthesize_wrapper_calls_only_for_callers_that_specialise_the_url() {
  let source = "function load(path: string) {\n\
  return fetch('/api' + path);\n\
}\n\
export function users() { return load('/users'); }\n\
export function anything(path: string) { return load(path); }\n";
  assert_eq!(
    render_source(&HttpClientDetector, "web", "src/api.ts", source),
    lines(&[
      "client:web:GET /api/users [heuristic 0.60]",
      "client:web:GET /api{} [resolved]",
      "makes_request load -> client:web:GET /api{} [resolved]",
      "makes_request users -> client:web:GET /api/users [heuristic 0.60]",
    ])
  );
}

#[test]
fn should_detect_axios_instances_config_calls_and_nest_http_service() {
  let source = "import axios from 'axios';\n\
import { HttpService } from '@nestjs/axios';\n\
const api = axios.create({ baseURL: 'https://api.shop.test/v2' });\n\
const envApi = axios.create({ baseURL: process.env.SHOP_API });\n\
export function list(token: string) { return api.get('/items', { headers: { Authorization: `Bearer ${token}` } }); }\n\
export function create(item: unknown) { return envApi.post('/items', item, { withCredentials: true }); }\n\
export function remove(id: string) { return axios({ method: 'delete', url: `/items/${id}` }); }\n\
export class Payments {\n  constructor(private readonly http: HttpService) {}\n  charge() { return this.http.post('https://pay.test/charges', {}, { headers: { 'X-Api-Key': 'k' } }); }\n}\n";
  assert_eq!(
    render_source(&HttpClientDetector, "shop", "src/api.ts", source),
    lines(&[
      "client:shop:DELETE /items/{} [syntactic]",
      "client:shop:GET https://api.shop.test/v2/items [syntactic]",
      "client:shop:POST https://pay.test/charges [syntactic]",
      "client:shop:POST {}/items [heuristic 0.70]",
      "makes_request Payments.charge -> client:shop:POST https://pay.test/charges [syntactic]",
      "makes_request create -> client:shop:POST {}/items [heuristic 0.70]",
      "makes_request list -> client:shop:GET https://api.shop.test/v2/items [syntactic]",
      "makes_request remove -> client:shop:DELETE /items/{} [syntactic]",
    ])
  );
  let analyzed = analyze("shop", "src/api.ts", source);
  let (detection, _) = run(&HttpClientDetector, &analyzed);
  let listed = request_attrs(&detection, "client:shop:GET https://api.shop.test/v2/items");
  assert_eq!(listed["base_source"], json!("literal:https://api.shop.test"));
  assert_eq!(listed["auth_hint"], json!(["bearer"]));
  let created = request_attrs(&detection, "client:shop:POST {}/items");
  assert_eq!(created["base_source"], json!("env:SHOP_API"));
  assert_eq!(created["auth_hint"], json!(["cookie:*"]));
  assert_eq!(request_attrs(&detection, "client:shop:POST https://pay.test/charges")["library"], json!("nest_http"));
}

#[test]
fn should_detect_reqwest_free_functions_typed_clients_and_auth_builders() {
  let source = "use reqwest::Client;\n\
pub async fn ping() { reqwest::get(\"https://status.test/ping\").await; }\n\
pub async fn push(client: &Client, base: &str, token: &str) {\n  client.request(reqwest::Method::PUT, format!(\"{}/items/{}\", base, 1)).bearer_auth(token).send().await;\n}\n\
pub async fn local() {\n  let http = Client::new();\n  let url = \"https://x.test/a\";\n  http.delete(url).basic_auth(\"u\", Some(\"p\")).send().await;\n}\n";
  assert_eq!(
    render_source(&HttpClientDetector, "svc", "src/lib.rs", source),
    lines(&[
      "client:svc:DELETE https://x.test/a [heuristic 0.70]",
      "client:svc:GET https://status.test/ping [syntactic]",
      "client:svc:PUT {}/items/{} [heuristic 0.70]",
      "makes_request local -> client:svc:DELETE https://x.test/a [heuristic 0.70]",
      "makes_request ping -> client:svc:GET https://status.test/ping [syntactic]",
      "makes_request push -> client:svc:PUT {}/items/{} [heuristic 0.70]",
    ])
  );
  let analyzed = analyze("svc", "src/lib.rs", source);
  let (detection, _) = run(&HttpClientDetector, &analyzed);
  assert_eq!(request_attrs(&detection, "client:svc:PUT {}/items/{}")["auth_hint"], json!(["bearer"]));
  assert_eq!(request_attrs(&detection, "client:svc:PUT {}/items/{}")["base_source"], json!("param"));
  assert_eq!(request_attrs(&detection, "client:svc:DELETE https://x.test/a")["auth_hint"], json!(["basic"]));
}
