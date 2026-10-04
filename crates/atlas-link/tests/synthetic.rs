mod common;

use atlas_core::{EdgeKind, EntityKind};
use common::{Indexed, index_tree, pass_facts, write_tree};
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
        "[heuristic 0.85]".to_owned()
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
