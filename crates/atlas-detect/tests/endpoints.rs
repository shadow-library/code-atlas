mod common;

use atlas_detect::{AxumEndpointDetector, ExpressEndpointDetector, NestEndpointDetector};
use common::{analyze, lines, render_file, render_repo, render_source, run};

#[test]
fn should_detect_every_acme_api_endpoint_with_its_normalised_route_and_handler() {
  let rendered: Vec<String> = render_repo(&NestEndpointDetector, "acme/acme-api")
    .into_iter()
    .filter(|line| line.starts_with("endpoint:") || line.starts_with("exposes_endpoint"))
    .collect();
  assert_eq!(
    rendered,
    lines(&[
      "endpoint:acme-api:DELETE /users/{id} [syntactic]",
      "endpoint:acme-api:GET /admin/stats [syntactic]",
      "endpoint:acme-api:GET /orders/{id} [resolved]",
      "endpoint:acme-api:GET /users/{id} [syntactic]",
      "endpoint:acme-api:GET /v1/reports/export [syntactic]",
      "endpoint:acme-api:PATCH /users/{id} [syntactic]",
      "endpoint:acme-api:POST /orders [resolved]",
      "endpoint:acme-api:POST /users [syntactic]",
      "exposes_endpoint AdminController.stats -> endpoint:acme-api:GET /admin/stats [syntactic]",
      "exposes_endpoint OrdersController.create -> endpoint:acme-api:POST /orders [resolved]",
      "exposes_endpoint OrdersController.findOne -> endpoint:acme-api:GET /orders/{id} [resolved]",
      "exposes_endpoint ReportsController.export -> endpoint:acme-api:GET /v1/reports/export [syntactic]",
      "exposes_endpoint UsersController.create -> endpoint:acme-api:POST /users [syntactic]",
      "exposes_endpoint UsersController.findOne -> endpoint:acme-api:GET /users/{id} [syntactic]",
      "exposes_endpoint UsersController.remove -> endpoint:acme-api:DELETE /users/{id} [syntactic]",
      "exposes_endpoint UsersController.update -> endpoint:acme-api:PATCH /users/{id} [syntactic]",
    ])
  );
}

#[test]
fn should_attach_class_and_method_guards_to_every_acme_endpoint() {
  let rendered: Vec<String> = render_repo(&NestEndpointDetector, "acme/acme-api")
    .into_iter()
    .filter(|line| line.starts_with("requires_guard"))
    .collect();
  assert_eq!(
    rendered,
    lines(&[
      "requires_guard endpoint:acme-api:DELETE /users/{id} -> ?JwtAuthGuard [syntactic]",
      "requires_guard endpoint:acme-api:DELETE /users/{id} -> ?PermissionsGuard [syntactic]",
      "requires_guard endpoint:acme-api:GET /admin/stats -> ?SessionGuard [syntactic]",
      "requires_guard endpoint:acme-api:GET /orders/{id} -> ?JwtOrApiKeyGuard [resolved]",
      "requires_guard endpoint:acme-api:GET /users/{id} -> ?JwtAuthGuard [syntactic]",
      "requires_guard endpoint:acme-api:GET /users/{id} -> ?PermissionsGuard [syntactic]",
      "requires_guard endpoint:acme-api:GET /v1/reports/export -> ?ApiKeyGuard [syntactic]",
      "requires_guard endpoint:acme-api:PATCH /users/{id} -> ?JwtAuthGuard [syntactic]",
      "requires_guard endpoint:acme-api:PATCH /users/{id} -> ?PermissionsGuard [syntactic]",
      "requires_guard endpoint:acme-api:POST /orders -> ?JwtAuthGuard [resolved]",
      "requires_guard endpoint:acme-api:POST /users -> ?JwtAuthGuard [syntactic]",
      "requires_guard endpoint:acme-api:POST /users -> ?PermissionsGuard [syntactic]",
    ])
  );
}

#[test]
fn should_record_guard_level_and_execution_order() {
  let analyzed = common::fixture_files("acme/acme-api")
    .into_iter()
    .find(|(path, _)| path == "src/users/users.controller.ts")
    .unwrap()
    .1;
  let (detection, _) = run(&NestEndpointDetector, &analyzed);
  let guards: Vec<(String, serde_json::Value, serde_json::Value)> = detection
    .edges
    .iter()
    .filter(|edge| edge.kind.as_str() == "requires_guard" && common::node(&analyzed.facts, &edge.from) == "endpoint:acme-api:GET /users/{id}")
    .map(|edge| (common::node(&analyzed.facts, &edge.to), edge.attrs["level"].clone(), edge.attrs["order"].clone()))
    .collect();
  assert_eq!(
    guards,
    [
      ("?JwtAuthGuard".to_owned(), serde_json::json!("class"), serde_json::json!(0)),
      ("?PermissionsGuard".to_owned(), serde_json::json!("class"), serde_json::json!(1)),
    ]
  );
}

#[test]
fn should_link_custom_permission_decorators_to_their_handlers() {
  let rendered: Vec<String> = render_file(&NestEndpointDetector, "acme/acme-api", "src/users/users.controller.ts")
    .into_iter()
    .filter(|line| line.starts_with("permission:") || line.starts_with("requires_permission"))
    .collect();
  assert_eq!(
    rendered,
    lines(&[
      "permission:acme-api:users:delete [syntactic]",
      "permission:acme-api:users:read [syntactic]",
      "permission:acme-api:users:write [syntactic]",
      "requires_permission UsersController.create -> permission:acme-api:users:write [syntactic]",
      "requires_permission UsersController.findOne -> permission:acme-api:users:read [syntactic]",
      "requires_permission UsersController.remove -> permission:acme-api:users:delete [syntactic]",
      "requires_permission UsersController.update -> permission:acme-api:users:write [syntactic]",
    ])
  );
}

#[test]
fn should_record_nest_endpoint_parameters_headers_and_body_type() {
  let source = "import { Body, Controller, Headers, Param, Put } from '@nestjs/common';\n\
@Controller({ path: 'items' })\n\
export class ItemsController {\n\
  @Put(':id')\n\
  replace(@Param('id') id: string, @Headers('x-request-id') requestId: string, @Body() body: ReplaceItem) {}\n\
}\n";
  let analyzed = analyze("shop", "src/items.controller.ts", source);
  let (detection, _) = run(&NestEndpointDetector, &analyzed);
  let endpoint = &detection.entities[0];
  assert_eq!(endpoint.key.to_string(), "endpoint:shop:PUT /items/{id}");
  assert_eq!(endpoint.attrs["params"], serde_json::json!(["id"]));
  assert_eq!(endpoint.attrs["headers"], serde_json::json!(["x-request-id"]));
  assert_eq!(endpoint.attrs["body_type"], serde_json::json!("ReplaceItem"));
  assert_eq!(endpoint.attrs["handler"], serde_json::json!("ItemsController.replace"));
}

#[test]
fn should_refuse_non_literal_controller_prefixes() {
  let source = "import { Controller, Get } from '@nestjs/common';\n\
const BASE = process.env.PREFIX ?? 'x';\n\
let MUTABLE = 'm';\n\
@Controller(BASE)\n\
export class A { @Get('a') a() {} }\n\
@Controller(MUTABLE)\n\
export class B { @Get('b') b() {} }\n\
@Controller(`${BASE}/c`)\n\
export class C { @Get() c() {} }\n";
  let analyzed = analyze("shop", "src/a.controller.ts", source);
  let (detection, diagnostics) = run(&NestEndpointDetector, &analyzed);
  assert_eq!(
    common::render(&analyzed.facts, &detection),
    lines(&[
      "endpoint:shop:GET /{}/c [heuristic 0.40]",
      "exposes_endpoint C.c -> endpoint:shop:GET /{}/c [heuristic 0.40]",
    ])
  );
  let codes: Vec<&str> = diagnostics.iter().map(|diagnostic| diagnostic.code.as_str()).collect();
  assert_eq!(codes, ["route-dynamic", "route-dynamic", "route-partially-dynamic"]);
}

#[test]
fn should_ignore_route_decorators_that_are_not_imported_from_nestjs() {
  let source = "import { Controller, Get } from 'routing-controllers';\n@Controller('a')\nexport class A { @Get('b') b() {} }\n";
  assert!(render_source(&NestEndpointDetector, "shop", "src/a.ts", source).is_empty());
}

#[test]
fn should_record_a_global_prefix_set_on_the_nest_application() {
  let source = "import { NestFactory } from '@nestjs/core';\n\
async function bootstrap() {\n  const app = await NestFactory.create(AppModule);\n  app.setGlobalPrefix('api/v1');\n  other.setGlobalPrefix('nope');\n}\n";
  assert_eq!(
    render_source(&NestEndpointDetector, "shop", "src/main.ts", source),
    lines(&["other:http_global_prefix:shop:/api/v1 [syntactic]"])
  );
}

#[test]
fn should_detect_globex_express_routes_on_a_typed_app_parameter() {
  assert_eq!(
    render_repo(&ExpressEndpointDetector, "globex/globex-api"),
    lines(&[
      "endpoint:globex-api:GET /orders/{id} [syntactic]",
      "endpoint:globex-api:GET /users/{id} [syntactic]",
      "endpoint:globex-api:POST /orders [syntactic]",
      "exposes_endpoint registerOrderRoutes -> endpoint:globex-api:GET /orders/{id} [syntactic]",
      "exposes_endpoint registerOrderRoutes -> endpoint:globex-api:POST /orders [syntactic]",
      "exposes_endpoint registerUserRoutes -> endpoint:globex-api:GET /users/{id} [syntactic]",
      "requires_guard endpoint:globex-api:GET /orders/{id} -> ?requireGlobexKey [syntactic]",
      "requires_guard endpoint:globex-api:GET /users/{id} -> ?requireGlobexKey [syntactic]",
      "requires_guard endpoint:globex-api:POST /orders -> ?requireGlobexKey [syntactic]",
    ])
  );
}

#[test]
fn should_resolve_computed_routes_without_inventing_dynamic_ones() {
  assert_eq!(
    render_repo(&ExpressEndpointDetector, "adversarial/computed-routes-ts"),
    lines(&[
      "endpoint:computed-routes-ts:GET /api/status [resolved]",
      "endpoint:computed-routes-ts:GET /legacy/ping [resolved]",
      "endpoint:computed-routes-ts:GET /{id} [heuristic 0.50]",
      "endpoint:computed-routes-ts:POST / [heuristic 0.50]",
      "exposes_endpoint file:computed-routes-ts:src/routes/items.ts -> endpoint:computed-routes-ts:GET /{id} [heuristic 0.50]",
      "exposes_endpoint file:computed-routes-ts:src/routes/items.ts -> endpoint:computed-routes-ts:POST / [heuristic 0.50]",
      "exposes_endpoint file:computed-routes-ts:src/server.ts -> endpoint:computed-routes-ts:GET /api/status [resolved]",
      "exposes_endpoint file:computed-routes-ts:src/server.ts -> endpoint:computed-routes-ts:GET /legacy/ping [resolved]",
      "mounts file:computed-routes-ts:src/routes/index.ts -> ?itemsRouter [heuristic 0.60]",
      "mounts file:computed-routes-ts:src/server.ts -> ?apiRouter [heuristic 0.60]",
    ])
  );
  let analyzed = common::fixture_files("adversarial/computed-routes-ts")
    .into_iter()
    .find(|(path, _)| path == "src/server.ts")
    .unwrap()
    .1;
  let (_, diagnostics) = run(&ExpressEndpointDetector, &analyzed);
  assert_eq!(diagnostics.iter().map(|diagnostic| diagnostic.code.as_str()).collect::<Vec<_>>(), ["route-dynamic"]);
}

#[test]
fn should_mark_unmounted_router_routes_as_router_relative() {
  let analyzed = common::fixture_files("adversarial/computed-routes-ts")
    .into_iter()
    .find(|(path, _)| path == "src/routes/items.ts")
    .unwrap()
    .1;
  let (detection, _) = run(&ExpressEndpointDetector, &analyzed);
  for endpoint in &detection.entities {
    assert_eq!(endpoint.attrs["route_scope"], serde_json::json!("router_relative"));
    assert_eq!(endpoint.attrs["router"], serde_json::json!("itemsRouter"));
  }
}

#[test]
fn should_compose_router_prefixes_mounted_in_the_same_file() {
  let source = "import express, { Router } from 'express';\n\
import { list } from './handlers';\n\
const app = express();\n\
const users = Router();\n\
const admin = express.Router();\n\
users.get('/:id', auth, list);\n\
admin.route('/stats').get((req, res) => res.json({}));\n\
users.use('/admin', admin);\n\
app.use('/api/users', users);\n\
function auth(req, res, next) { next(); }\n";
  assert_eq!(
    render_source(&ExpressEndpointDetector, "shop", "src/app.ts", source),
    lines(&[
      "endpoint:shop:GET /api/users/admin/stats [resolved]",
      "endpoint:shop:GET /api/users/{id} [resolved]",
      "exposes_endpoint ?list -> endpoint:shop:GET /api/users/{id} [resolved]",
      "exposes_endpoint file:shop:src/app.ts -> endpoint:shop:GET /api/users/admin/stats [resolved]",
      "requires_guard endpoint:shop:GET /api/users/{id} -> auth [resolved]",
    ])
  );
}

#[test]
fn should_detect_fastify_and_koa_router_registrations() {
  let fastify = "import Fastify from 'fastify';\nconst server = Fastify();\nserver.get('/health', async () => ({ ok: true }));\n";
  assert_eq!(
    render_source(&ExpressEndpointDetector, "shop", "src/server.ts", fastify),
    lines(&[
      "endpoint:shop:GET /health [syntactic]",
      "exposes_endpoint file:shop:src/server.ts -> endpoint:shop:GET /health [syntactic]"
    ])
  );
  let koa = "import Router from '@koa/router';\nconst router = new Router();\nexport function routes() { router.post('/items', create); }\n";
  assert_eq!(
    render_source(&ExpressEndpointDetector, "shop", "src/routes.ts", koa),
    lines(&["endpoint:shop:POST /items [syntactic]", "exposes_endpoint ?create -> endpoint:shop:POST /items [resolved]"])
  );
}

#[test]
fn should_detect_the_acme_billing_axum_router() {
  assert_eq!(
    render_repo(&AxumEndpointDetector, "acme/acme-billing"),
    lines(&[
      "endpoint:acme-billing:GET /invoices/{id} [syntactic]",
      "endpoint:acme-billing:POST /invoices [syntactic]",
      "exposes_endpoint ?create_invoice -> endpoint:acme-billing:POST /invoices [resolved]",
      "exposes_endpoint ?get_invoice -> endpoint:acme-billing:GET /invoices/{id} [resolved]",
    ])
  );
}

#[test]
fn should_follow_axum_method_chains_nesting_and_services() {
  let source = "use axum::{routing::{get, post}, Router};\n\
use crate::other::routes as other_routes;\n\
const ORDERS: &str = \"/orders/:id\";\n\
pub fn app() -> Router {\n  Router::new()\n    .route(ORDERS, get(show).post(create).layer(TraceLayer::new()))\n    .route(\"/health\", get(|| async { \"ok\" }))\n    .route_service(\"/static\", ServeDir::new(\"assets\"))\n    .nest(\"/api\", api())\n    .nest(\"/ext\", other_routes())\n}\n\
fn api() -> Router { Router::new().route(\"/items\", axum::routing::delete(remove_item)) }\n\
fn show() {}\n";
  assert_eq!(
    render_source(&AxumEndpointDetector, "billing", "src/app.rs", source),
    lines(&[
      "endpoint:billing:ANY /static [syntactic]",
      "endpoint:billing:DELETE /api/items [resolved]",
      "endpoint:billing:GET /health [syntactic]",
      "endpoint:billing:GET /orders/{id} [resolved]",
      "endpoint:billing:POST /orders/{id} [resolved]",
      "exposes_endpoint ?create -> endpoint:billing:POST /orders/{id} [resolved]",
      "exposes_endpoint ?remove_item -> endpoint:billing:DELETE /api/items [resolved]",
      "exposes_endpoint app -> endpoint:billing:ANY /static [syntactic]",
      "exposes_endpoint app -> endpoint:billing:GET /health [syntactic]",
      "exposes_endpoint show -> endpoint:billing:GET /orders/{id} [resolved]",
      "mounts app -> ?other_routes [heuristic 0.60]",
    ])
  );
}

#[test]
fn should_detect_actix_and_rocket_route_attributes() {
  let source = "use actix_web::{get as fetch, post};\n\
#[fetch(\"/users/{id:\\\\d+}\")]\nasync fn user() {}\n\
#[post(\"/users\")]\nasync fn create() {}\n\
#[actix_web::route(\"/both\", method = \"GET\", method = \"HEAD\")]\nasync fn both() {}\n\
#[get(\"/not-actix\")]\nfn unrelated() {}\n";
  assert_eq!(
    render_source(&AxumEndpointDetector, "svc", "src/main.rs", source),
    lines(&[
      "endpoint:svc:GET /both [syntactic]",
      "endpoint:svc:GET /users/{id} [syntactic]",
      "endpoint:svc:HEAD /both [syntactic]",
      "endpoint:svc:POST /users [syntactic]",
      "exposes_endpoint both -> endpoint:svc:GET /both [syntactic]",
      "exposes_endpoint both -> endpoint:svc:HEAD /both [syntactic]",
      "exposes_endpoint create -> endpoint:svc:POST /users [syntactic]",
      "exposes_endpoint user -> endpoint:svc:GET /users/{id} [syntactic]",
    ])
  );
}

#[test]
fn should_treat_routes_on_a_router_parameter_as_router_relative() {
  let source = "use axum::Router;\nuse axum::routing::get;\npub fn add(router: Router<AppState>) -> Router<AppState> { router.route(\"/x\", get(x)) }\n";
  assert_eq!(
    render_source(&AxumEndpointDetector, "svc", "src/lib.rs", source),
    lines(&["endpoint:svc:GET /x [heuristic 0.50]", "exposes_endpoint ?x -> endpoint:svc:GET /x [heuristic 0.50]"])
  );
}
