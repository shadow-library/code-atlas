mod common;

use atlas_core::{Certainty, CertaintyClass, EdgeKind, EntityId, EntityKey, EntityKind, HttpMethod, Owner, RepoName};
use common::{Indexed, fixtures_root, index_tree, pass_facts, write_tree};
use serde_json::json;

const SHOP_API: &[(&str, &str)] = &[
  (
    "shop-api/package.json",
    r#"{ "name": "shop-api", "version": "1.0.0", "scripts": { "start": "node dist/main.js" }, "dependencies": { "@nestjs/common": "^10.0.0", "@nestjs/core": "^10.0.0" } }"#,
  ),
  (
    "shop-api/src/main.ts",
    r#"import { NestFactory } from '@nestjs/core';
import { AppModule } from './app.module';

async function bootstrap(): Promise<void> {
  const app = await NestFactory.create(AppModule);
  app.setGlobalPrefix('api');
  await app.listen(3000);
}

bootstrap();
"#,
  ),
  ("shop-api/src/app.module.ts", "export class AppModule {}\n"),
  (
    "shop-api/src/throttle.guard.ts",
    r#"import { CanActivate, ExecutionContext, Injectable } from '@nestjs/common';

@Injectable()
export class ThrottleGuard implements CanActivate {
  canActivate(context: ExecutionContext): boolean {
    return context.getType() === 'http';
  }
}
"#,
  ),
  (
    "shop-api/src/users.controller.ts",
    r#"import { Controller, Get, Param, UseGuards } from '@nestjs/common';

import { ThrottleGuard } from './throttle.guard';

@Controller('users')
export class UsersController {
  @UseGuards(ThrottleGuard)
  @Get(':id')
  findOne(@Param('id') id: string) {
    return { id };
  }
}
"#,
  ),
];

const BILLING_API: &[(&str, &str)] = &[
  (
    "billing-api/package.json",
    r#"{ "name": "billing-api", "version": "1.0.0", "scripts": { "start": "node server.js" }, "dependencies": { "express": "^4.0.0" } }"#,
  ),
  (
    "billing-api/src/server.ts",
    r#"import express from 'express';

const app = express();

app.get('/users/:id', (req, res) => {
  res.json({ id: req.params.id });
});

app.all('/ping', (_req, res) => {
  res.send('pong');
});

app.listen(4000);
"#,
  ),
];

const LEDGER: &[(&str, &str)] = &[
  (
    "ledger/Cargo.toml",
    "[package]\nname = \"ledger\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n\n[dependencies]\naxum = \"0.8\"\n",
  ),
  (
    "ledger/src/lib.rs",
    r#"pub mod accounts;

use axum::Router;

pub fn app() -> Router {
  Router::new().nest("/v1", accounts::routes())
}
"#,
  ),
  (
    "ledger/src/accounts.rs",
    r#"use axum::{routing::get, Router};

pub async fn show() -> &'static str {
  "account"
}

pub fn routes() -> Router {
  Router::new().route("/accounts/{id}", get(show))
}
"#,
  ),
];

const SHOP_WEB: &[(&str, &str)] = &[
  ("shop-web/package.json", r#"{ "name": "shop-web", "version": "1.0.0", "dependencies": {} }"#),
  (
    "shop-web/src/api.ts",
    r#"export async function loadShopUser(id: string) {
  return fetch(`${process.env.SHOP_API_URL}/api/users/${id}`);
}

export async function loadAnyUser(base: string, id: string) {
  return fetch(`${base}/users/${id}`);
}

export async function pingAnything() {
  return fetch(`${process.env.BILLING_API_URL}/ping`, { method: 'POST' });
}

export async function loadAccount(id: string) {
  return fetch(`${process.env.LEDGER_URL}/v1/accounts/${id}`);
}
"#,
  ),
];

fn synthetic(scratch: &std::path::Path) -> Indexed {
  let source = scratch.join("source");
  for files in [SHOP_API, BILLING_API, LEDGER, SHOP_WEB] {
    write_tree(&source, files);
  }
  index_tree(scratch, "shop", &source, &["shop-api", "billing-api", "ledger", "shop-web"])
}

fn links(indexed: &Indexed, caller: &str) -> Vec<(String, f32, serde_json::Value)> {
  let facts = pass_facts(&indexed.engine);
  let mut found: Vec<(String, f32, serde_json::Value)> = indexed
    .engine
    .store()
    .read(|tx| -> Result<_, atlas_store::StoreError> {
      let mut found = Vec::new();
      for edge in facts.edges.iter().filter(|edge| edge.kind == EdgeKind::CallsEndpoint) {
        let from = tx.entity(edge.from)?.unwrap();
        if from.name != caller {
          continue;
        }
        let to = tx.entity(edge.to)?.unwrap();
        found.push((to.key.to_string(), edge.certainty.confidence().unwrap().value(), edge.attrs["rule"].clone()));
      }
      Ok(found)
    })
    .unwrap();
  found.sort_by(|left, right| left.0.cmp(&right.0));
  found
}

#[test]
fn should_compose_nest_global_prefixes_and_axum_nesting() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = synthetic(scratch.path());
  let facts = pass_facts(&indexed.engine);
  let mut composed: Vec<(String, String, String)> = facts
    .entities
    .iter()
    .filter(|entity| entity.kind == EntityKind::ApiEndpoint && entity.attrs.contains_key("composed_from"))
    .map(|entity| (entity.key.to_string(), entity.attrs["composed_from"].as_str().unwrap().to_owned(), entity.certainty.tag()))
    .collect();
  composed.sort();
  assert_eq!(
    composed,
    [
      (
        "endpoint:ledger:GET /v1/accounts/{id}".to_owned(),
        "endpoint:ledger:GET /accounts/{id}".to_owned(),
        "[heuristic 0.60]".to_owned()
      ),
      (
        "endpoint:shop-api:GET /api/users/{id}".to_owned(),
        "endpoint:shop-api:GET /users/{id}".to_owned(),
        "[heuristic 0.50]".to_owned()
      ),
    ]
  );
}

#[test]
fn should_follow_base_evidence_to_the_named_repo() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = synthetic(scratch.path());
  let shop = links(&indexed, "loadShopUser");
  assert_eq!(shop.len(), 1, "{shop:?}");
  assert_eq!(shop[0].0, "endpoint:shop-api:GET /api/users/{id}");
  assert!(shop[0].1 > 0.8, "{shop:?}");
  let ledger = links(&indexed, "loadAccount");
  assert_eq!(ledger.iter().map(|link| link.0.as_str()).collect::<Vec<_>>(), ["endpoint:ledger:GET /v1/accounts/{id}"]);
}

#[test]
fn should_not_offer_base_path_prefix_candidates_when_a_full_match_exists() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = synthetic(scratch.path());
  let any = links(&indexed, "loadAnyUser");
  assert_eq!(
    any.iter().map(|link| (link.0.as_str(), link.2.clone())).collect::<Vec<_>>(),
    [("endpoint:billing-api:GET /users/{id}", json!("path-template"))]
  );
}

#[test]
fn should_record_guards_without_known_auth_semantics() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = synthetic(scratch.path());
  let facts = pass_facts(&indexed.engine);
  let endpoint = facts
    .entities
    .iter()
    .find(|entity| entity.key.to_string() == "endpoint:shop-api:GET /api/users/{id}")
    .unwrap();
  assert_eq!(endpoint.attrs["unclassified_guards"], json!(["ThrottleGuard"]));
  assert_eq!(endpoint.attrs["auth_requirement"], json!({ "all_of": ["unknown_guard:ThrottleGuard"] }));
  assert!(facts.edges.iter().all(|edge| edge.kind != EdgeKind::SupportsAuth));
}

#[test]
fn should_match_any_method_routes_at_reduced_confidence() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = synthetic(scratch.path());
  let ping = links(&indexed, "pingAnything");
  assert_eq!(ping.len(), 1, "{ping:?}");
  assert_eq!(ping[0].0, "endpoint:billing-api:ANY /ping");
  assert!(ping[0].1 < 0.75, "{ping:?}");
}

const GATE_API: &[(&str, &str)] = &[
  (
    "gate-api/package.json",
    r#"{ "name": "gate-api", "version": "1.0.0", "scripts": { "start": "node dist/main.js" }, "dependencies": { "@nestjs/common": "^10.0.0", "@nestjs/core": "^10.0.0" } }"#,
  ),
  ("gate-api/src/main.ts", "import { NestFactory } from '@nestjs/core';\n\nNestFactory.create(AppModule);\n"),
  (
    "gate-api/src/guards.ts",
    r#"import { CanActivate, Injectable, SetMetadata } from '@nestjs/common';

export const RequirePermission = (permission: string) => SetMetadata('permission', permission);

@Injectable()
export class TenantGuard implements CanActivate {
  canActivate(): boolean {
    return true;
  }
}

@Injectable()
export class PermissionsGuard implements CanActivate {
  canActivate(): boolean {
    return true;
  }
}
"#,
  ),
  (
    "gate-api/src/a.controller.ts",
    r#"import { Controller, Get, UseGuards } from '@nestjs/common';

import { PermissionsGuard, RequirePermission, TenantGuard } from './guards';

@Controller('a')
export class AController {
  @UseGuards(TenantGuard)
  @Get('x')
  tenant() {
    return 1;
  }

  @UseGuards(PermissionsGuard)
  @RequirePermission('a:read')
  @Get('y')
  permitted() {
    return 2;
  }
}
"#,
  ),
];

#[test]
fn should_source_guard_classification_attrs_as_heuristic_while_the_endpoint_stays_syntactic() {
  let scratch = tempfile::tempdir().unwrap();
  let source = scratch.path().join("source");
  write_tree(&source, GATE_API);
  let indexed = index_tree(scratch.path(), "gate", &source, &["gate-api"]);
  let attrs = ["auth_requirement", "auth_combinator", "unclassified_guards", "authorization_guards"];
  for (route, requirement) in [
    ("/a/x", json!({ "all_of": ["unknown_guard:TenantGuard"] })),
    ("/a/y", json!({ "all_of": ["authorization_only:PermissionsGuard"] })),
  ] {
    let key = EntityKey::Endpoint {
      repo: RepoName::new("gate-api").unwrap(),
      method: HttpMethod::Get,
      route: route.to_owned(),
    };
    let merged = indexed.engine.store().read(|tx| tx.entity_with_sources(EntityId::of(&key))).unwrap().unwrap();
    assert_eq!(merged.entity.attrs["auth_requirement"], requirement, "{route}");
    assert_eq!(merged.entity.certainty, Certainty::Syntactic, "{route}");
    for attr in attrs {
      let certainty = &merged.attr_sources[attr].certainty;
      assert_eq!(merged.attr_sources[attr].owner, Owner::Project { pass: "link".to_owned() }, "{route} {attr}");
      assert_eq!(certainty.class(), CertaintyClass::Heuristic, "{route} {attr}");
      assert_eq!(certainty.confidence().map(|confidence| confidence.value()), Some(0.5), "{route} {attr}");
    }
  }
}

fn me_fetch(name: &str, prefix: &str) -> String {
  format!(
    "import {{ getToken }} from '{prefix}api/session';\nimport {{ API_URL }} from '{prefix}config';\n\nexport function {name}() {{\n  const token = getToken();\n  return fetch(`${{API_URL}}/users/me`, {{ headers: {{ Authorization: `Bearer ${{token}}` }} }});\n}}\n"
  )
}

#[test]
fn should_not_derive_auth_usage_from_test_code_in_root_test_dirs_or_e2e_specs() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = index_tree(scratch.path(), "acme", &fixtures_root().join("acme"), &["acme-api", "acme-sdk", "acme-web", "acme-billing"]);
  let web = scratch.path().join("tree/acme");
  let files = [
    ("acme-web/test/me.e2e-spec.ts".to_owned(), me_fetch("rootTestProbe", "../src/")),
    ("acme-web/src/hooks/me.e2e-spec.ts".to_owned(), me_fetch("e2eSpecProbe", "../")),
    ("acme-web/src/hooks/meProbe.ts".to_owned(), me_fetch("productionProbe", "../")),
  ];
  let files: Vec<(&str, &str)> = files.iter().map(|(path, content)| (path.as_str(), content.as_str())).collect();
  write_tree(&web, &files);
  assert!(indexed.engine.ensure_fresh().unwrap().is_fresh());
  let facts = pass_facts(&indexed.engine);

  let users: Vec<String> = indexed
    .engine
    .store()
    .read(|tx| -> Result<Vec<String>, atlas_store::StoreError> {
      let mut names = Vec::new();
      for edge in facts.edges.iter().filter(|edge| edge.kind == EdgeKind::UsesAuth) {
        if let Some(entity) = tx.entity(edge.from)?.filter(|entity| entity.name.ends_with("Probe")) {
          names.push(entity.name);
        }
      }
      Ok(names)
    })
    .unwrap();

  assert_eq!(users, vec!["productionProbe".to_owned()]);
}

fn order_probe(module: &str, attribute: &str, name: &str) -> String {
  format!(
    "\n{attribute}mod {module} {{\n  pub async fn {name}(http: &reqwest::Client, base_url: &str, api_key: &str) -> reqwest::Result<reqwest::Response> {{\n    let url = format!(\"{{}}/orders/{{}}\", base_url, \"o-1\");\n    http.get(url).header(\"x-api-key\", api_key).send().await\n  }}\n}}\n"
  )
}

#[test]
fn should_not_derive_auth_usage_from_rust_items_inside_a_cfg_test_module_of_any_name() {
  let scratch = tempfile::tempdir().unwrap();
  let indexed = index_tree(scratch.path(), "acme", &fixtures_root().join("acme"), &["acme-api", "acme-sdk", "acme-web", "acme-billing"]);
  let client = scratch.path().join("tree/acme/acme-billing/src/acme_client.rs");
  let mut source = std::fs::read_to_string(&client).unwrap();
  source.push_str(&order_probe("fixtures", "#[cfg(test)]\n", "fixture_probe"));
  source.push_str(&order_probe("probes", "", "production_probe"));
  std::fs::write(&client, source).unwrap();
  assert!(indexed.engine.ensure_fresh().unwrap().is_fresh());
  let facts = pass_facts(&indexed.engine);

  let users: Vec<String> = indexed
    .engine
    .store()
    .read(|tx| -> Result<Vec<String>, atlas_store::StoreError> {
      let mut names = Vec::new();
      for edge in facts.edges.iter().filter(|edge| edge.kind == EdgeKind::UsesAuth) {
        if let Some(entity) = tx.entity(edge.from)?.filter(|entity| entity.name.ends_with("_probe")) {
          names.push(entity.name);
        }
      }
      Ok(names)
    })
    .unwrap();

  assert_eq!(users, vec!["production_probe".to_owned()]);
}

const MIXED_API: &[(&str, &str)] = &[
  ("mixed-api/package.json", r#"{ "name": "mixed-api", "version": "1.0.0" }"#),
  (
    "mixed-api/src/main.ts",
    r#"import { AcmeServer } from '@acme/http';

class AppModule {}

async function boot(): Promise<void> {
  const app = await AcmeServer.create(AppModule);
  app.setGlobalPrefix('api');
}

boot();
"#,
  ),
  (
    "mixed-api/src/nest-main.ts",
    r#"import { NestFactory } from '@nestjs/core';

class NestModule {}

async function bootstrap(): Promise<void> {
  const app = await NestFactory.create(NestModule);
  app.setGlobalPrefix('api');
}

bootstrap();
"#,
  ),
  (
    "mixed-api/src/orders.controller.ts",
    r#"import { Api, Read } from '@acme/http';

@Api('orders')
export class OrdersController {
  @Read(':id')
  find() {
    return null;
  }
}
"#,
  ),
  (
    "mixed-api/src/health.controller.ts",
    r#"import { Controller, Get } from '@nestjs/common';

@Controller('health')
export class HealthController {
  @Get()
  check() {
    return 'ok';
  }
}
"#,
  ),
];

#[test]
fn should_compose_a_nest_prefix_and_a_family_prefix_of_the_same_value_each_onto_its_own_routes() {
  let scratch = tempfile::tempdir().unwrap();
  let source = scratch.path().join("source");
  write_tree(&source, MIXED_API);
  let family = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n[global_prefix]\nfactory = \"AcmeServer.create\"\nmethod = \"setGlobalPrefix\"\n";
  let mut project = atlas_project::Project {
    name: atlas_core::ProjectName::new("mixed").unwrap(),
    settings: atlas_project::ProjectSettings::default(),
    repos: vec![atlas_project::RepoConfig {
      name: RepoName::new("mixed-api").unwrap(),
      path: source.join("mixed-api"),
      include: Vec::new(),
      exclude: Vec::new(),
      languages: None,
    }],
  };
  project.settings.detect.route_families = vec!["acme-http".to_owned()];
  project.settings.detect.families = vec![atlas_detect::DecoratorFamily::parse("acme-http", family).unwrap().family];
  let engine = atlas_engine::Engine::open(
    &project,
    &atlas_project::AtlasHome::rooted_at(scratch.path().join("home")),
    atlas_engine::EngineOptions::default(),
  )
  .unwrap();
  assert!(engine.ensure_fresh().unwrap().is_fresh());

  let mut composed: Vec<(String, String, String)> = pass_facts(&engine)
    .entities
    .iter()
    .filter(|entity| entity.kind == EntityKind::ApiEndpoint && entity.attrs.contains_key("composed_from"))
    .map(|entity| {
      (
        entity.key.to_string(),
        entity.attrs["composed_from"].as_str().unwrap().to_owned(),
        entity.attrs["rule"].as_str().unwrap().to_owned(),
      )
    })
    .collect();
  composed.sort();

  assert_eq!(
    composed,
    [
      (
        "endpoint:mixed-api:GET /api/health".to_owned(),
        "endpoint:mixed-api:GET /health".to_owned(),
        "nest-global-prefix".to_owned()
      ),
      (
        "endpoint:mixed-api:GET /api/orders/{id}".to_owned(),
        "endpoint:mixed-api:GET /orders/{id}".to_owned(),
        "declared-global-prefix".to_owned()
      ),
    ]
  );
}
