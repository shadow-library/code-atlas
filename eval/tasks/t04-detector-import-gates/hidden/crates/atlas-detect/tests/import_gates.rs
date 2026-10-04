mod common;

use atlas_detect::{AuthSchemeDetector, AxumEndpointDetector, DiagnosingDetector, ExpressEndpointDetector, HttpClientDetector, NestEndpointDetector};
use common::{analyze, lines, render_source};

const TS_SOURCES: [(&str, &str); 12] = [
  (
    "esm-express",
    "import express from 'express';\n\nconst app = express();\napp.get('/a', (_req, res) => res.send('a'));\n",
  ),
  (
    "require-express",
    "const express = require('express');\n\nconst app = express();\napp.get('/a', (_req, res) => res.send('a'));\n",
  ),
  ("esm-fastify", "import Fastify from 'fastify';\n\nconst app = Fastify();\napp.get('/a', async () => 'a');\n"),
  (
    "require-koa-router",
    "const Router = require('@koa/router');\n\nconst router = new Router();\nrouter.get('/a', (ctx) => {});\n",
  ),
  (
    "esm-nest",
    "import { Controller, Get } from '@nestjs/common';\n\n@Controller('a')\nexport class AController {\n  @Get()\n  list() {\n    return [];\n  }\n}\n",
  ),
  (
    "require-nest",
    "const { Controller, Get } = require('@nestjs/common');\n\n@Controller('a')\nexport class AController {\n  @Get()\n  list() {\n    return [];\n  }\n}\n",
  ),
  ("nest-lookalike", "import { Controller } from 'nestjs-extra';\n\nexport const controller = Controller;\n"),
  (
    "esm-axios",
    "import axios from 'axios';\n\nexport async function load(id: string) {\n  return axios.get(`/users/${id}`);\n}\n",
  ),
  (
    "esm-nest-axios",
    "import { HttpService } from '@nestjs/axios';\n\nexport class Api {\n  constructor(private readonly http: HttpService) {}\n}\n",
  ),
  (
    "require-axios",
    "const axios = require('axios');\n\nexport async function load(id: string) {\n  return axios.get(`/users/${id}`);\n}\n",
  ),
  ("fetch-only", "export async function ping() {\n  return fetch('/ping');\n}\n"),
  (
    "unrelated",
    "import { format } from 'date-fns';\n\nexport const today = () => format(new Date(), 'yyyy');\n",
  ),
];

const RUST_SOURCES: [(&str, &str); 10] = [
  ("use-axum", "use axum::Router;\n\npub fn app() -> Router {\n  Router::new()\n}\n"),
  ("path-axum", "pub fn app() -> axum::Router {\n  axum::Router::new()\n}\n"),
  ("extern-axum", "extern crate axum;\n\npub fn app() {}\n"),
  ("use-actix", "use actix_web::get;\n\n#[get(\"/a\")]\nasync fn a() -> &'static str {\n  \"a\"\n}\n"),
  ("use-rocket", "use rocket::get;\n\n#[get(\"/a\")]\nfn a() -> &'static str {\n  \"a\"\n}\n"),
  (
    "use-reqwest",
    "use reqwest::Client;\n\npub async fn load(client: &Client) {\n  let _ = client.get(\"http://api/users\").send().await;\n}\n",
  ),
  ("path-reqwest", "pub async fn load() {\n  let _ = reqwest::get(\"http://api/users\").await;\n}\n"),
  ("extern-reqwest", "extern crate reqwest;\n\npub fn load() {}\n"),
  ("grouped-use", "use {axum::Router, serde::Deserialize};\n\npub fn app() -> Router {\n  Router::new()\n}\n"),
  ("unrelated", "use serde::Deserialize;\n\n#[derive(Deserialize)]\npub struct Config {\n  pub port: u16,\n}\n"),
];

fn gates(detector: &dyn DiagnosingDetector, sources: &[(&str, &str)], extension: &str) -> Vec<String> {
  sources
    .iter()
    .filter(|(name, source)| detector.applies_to(&analyze("repo", &format!("src/{name}.{extension}"), source).facts))
    .map(|(name, _)| (*name).to_owned())
    .collect()
}

#[test]
fn should_gate_the_express_detector_on_esm_and_require_imports_of_server_packages() {
  assert_eq!(
    gates(&ExpressEndpointDetector, &TS_SOURCES, "ts"),
    ["esm-express", "require-express", "esm-fastify", "require-koa-router"]
  );
  assert_eq!(gates(&ExpressEndpointDetector, &RUST_SOURCES, "rs"), Vec::<String>::new());
}

#[test]
fn should_gate_the_nest_detector_on_esm_imports_from_the_nestjs_scope_only() {
  assert_eq!(gates(&NestEndpointDetector, &TS_SOURCES, "ts"), ["esm-nest", "esm-nest-axios"]);
  assert_eq!(gates(&NestEndpointDetector, &RUST_SOURCES, "rs"), Vec::<String>::new());
}

#[test]
fn should_gate_the_http_client_detector_on_fetch_esm_axios_and_reqwest_paths() {
  assert_eq!(gates(&HttpClientDetector, &TS_SOURCES, "ts"), ["esm-axios", "esm-nest-axios", "fetch-only"]);
  assert_eq!(gates(&HttpClientDetector, &RUST_SOURCES, "rs"), ["use-reqwest", "path-reqwest"]);
}

#[test]
fn should_gate_the_axum_detector_on_server_crate_uses_and_paths_but_not_extern_crate() {
  assert_eq!(gates(&AxumEndpointDetector, &TS_SOURCES, "ts"), Vec::<String>::new());
  assert_eq!(
    gates(&AxumEndpointDetector, &RUST_SOURCES, "rs"),
    ["use-axum", "path-axum", "use-actix", "use-rocket", "grouped-use"]
  );
}

const MIDDLEWARE_BODY: &str = "export function guard(req, res, next) {\n  const header = req.headers.authorization;\n  if (!header || !header.startsWith('Bearer ')) {\n    return res.status(401).end();\n  }\n  next();\n}\n";

#[test]
fn should_recognise_express_shaped_middleware_imported_with_esm_or_require() {
  for import in [
    "import express from 'express';",
    "const express = require('express');",
    "import Koa from 'koa';",
    "const fastify = require('fastify');",
  ] {
    assert_eq!(
      render_source(&AuthSchemeDetector, "api", "src/guard.ts", &format!("{import}\n\n{MIDDLEWARE_BODY}")),
      lines(&["auth:api:bearer [heuristic 0.85]", "implements_auth guard -> auth:api:bearer [heuristic 0.85]"]),
      "{import}"
    );
  }
  assert_eq!(
    render_source(
      &AuthSchemeDetector,
      "api",
      "src/guard.ts",
      &format!("import {{ Router }} from 'other';\n\n{MIDDLEWARE_BODY}")
    ),
    Vec::<String>::new()
  );
}

const EXTRACTOR_BODY: &str = "pub struct Bearer(String);\n\nimpl<S: Send + Sync> FromRequestParts<S> for Bearer {\n  type Rejection = ();\n  async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ()> {\n    parts.headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix(\"Bearer \")).map(|t| Bearer(t.into())).ok_or(())\n  }\n}\n";

#[test]
fn should_recognise_rust_extractors_of_crates_brought_in_by_use_or_extern_crate() {
  for import in [
    "use axum::extract::FromRequestParts;",
    "extern crate axum;",
    "use actix_web::FromRequest;",
    "extern crate actix_web;",
  ] {
    assert_eq!(
      render_source(&AuthSchemeDetector, "svc", "src/auth.rs", &format!("{import}\n\n{EXTRACTOR_BODY}")),
      lines(&["auth:svc:bearer [heuristic 0.85]", "implements_auth Bearer -> auth:svc:bearer [heuristic 0.85]"]),
      "{import}"
    );
  }
  assert_eq!(
    render_source(&AuthSchemeDetector, "svc", "src/auth.rs", &format!("use serde::Deserialize;\n\n{EXTRACTOR_BODY}")),
    Vec::<String>::new()
  );
}

#[test]
fn should_detect_axios_calls_only_through_esm_imports_of_axios() {
  let call = "\n\nexport async function load(id: string) {\n  await fetch('/ping');\n  return axios.get(`/users/${id}`);\n}\n";
  assert_eq!(
    render_source(&HttpClientDetector, "web", "src/api.ts", &format!("import axios from 'axios';{call}")),
    lines(&[
      "client:web:GET /ping [syntactic]",
      "client:web:GET /users/{} [syntactic]",
      "makes_request load -> client:web:GET /ping [syntactic]",
      "makes_request load -> client:web:GET /users/{} [syntactic]",
    ])
  );
  assert_eq!(
    render_source(&HttpClientDetector, "web", "src/api.ts", &format!("const axios = require('axios');{call}")),
    lines(&["client:web:GET /ping [syntactic]", "makes_request load -> client:web:GET /ping [syntactic]"])
  );
}
