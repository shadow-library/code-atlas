mod common;

use atlas_detect::AuthSchemeDetector;
use common::{lines, render_repo, render_source};

#[test]
fn should_classify_the_three_acme_guard_implementations() {
  assert_eq!(
    render_repo(&AuthSchemeDetector, "acme/acme-api"),
    lines(&[
      "auth:acme-api:api_key [heuristic 0.80]",
      "auth:acme-api:bearer [heuristic 0.85]",
      "auth:acme-api:session_cookie [heuristic 0.80]",
      "implements_auth ApiKeyGuard -> auth:acme-api:api_key [heuristic 0.80]",
      "implements_auth JwtAuthGuard -> auth:acme-api:bearer [heuristic 0.85]",
      "implements_auth SessionGuard -> auth:acme-api:session_cookie [heuristic 0.80]",
      "references JwtOrApiKeyGuard -> ?ApiKeyGuard [heuristic 0.70]",
      "references JwtOrApiKeyGuard -> ?JwtAuthGuard [heuristic 0.70]",
    ])
  );
}

#[test]
fn should_record_the_header_and_cookie_each_scheme_reads() {
  let files = common::fixture_files("acme/acme-api");
  let attrs_of = |path: &str| {
    let analyzed = &files.iter().find(|(file, _)| file == path).unwrap().1;
    let (detection, _) = common::run(&AuthSchemeDetector, analyzed);
    serde_json::to_value(&detection.entities[0].attrs).unwrap()
  };
  assert_eq!(
    attrs_of("src/auth/api-key.guard.ts"),
    serde_json::json!({ "guard_kind": "nest_guard", "header": "x-api-key" })
  );
  assert_eq!(
    attrs_of("src/auth/session.guard.ts"),
    serde_json::json!({ "guard_kind": "nest_guard", "cookie": "acme_session" })
  );
}

#[test]
fn should_classify_the_globex_express_middleware() {
  assert_eq!(
    render_repo(&AuthSchemeDetector, "globex/globex-api"),
    lines(&[
      "auth:globex-api:api_key [heuristic 0.80]",
      "implements_auth requireGlobexKey -> auth:globex-api:api_key [heuristic 0.80]",
    ])
  );
}

#[test]
fn should_map_passport_guards_and_strategies_to_schemes() {
  let source = "import { Injectable } from '@nestjs/common';\n\
import { AuthGuard, PassportStrategy } from '@nestjs/passport';\n\
import { Strategy } from 'passport-jwt';\n\
import { HeaderAPIKeyStrategy } from 'passport-headerapikey';\n\
@Injectable() export class JwtGuard extends AuthGuard('jwt') {}\n\
@Injectable() export class JwtStrategy extends PassportStrategy(Strategy) { validate(p: unknown) { return p; } }\n\
@Injectable() export class KeyStrategy extends PassportStrategy(HeaderAPIKeyStrategy, 'api-key') {}\n\
@Injectable() export class LocalGuard extends AuthGuard('local') {}\n\
export class Base extends Mixin('jwt') {}\n";
  assert_eq!(
    render_source(&AuthSchemeDetector, "api", "src/auth.ts", source),
    lines(&[
      "auth:api:api_key [heuristic 0.75]",
      "auth:api:bearer [heuristic 0.75]",
      "implements_auth JwtGuard -> auth:api:bearer [heuristic 0.75]",
      "implements_auth JwtStrategy -> auth:api:bearer [heuristic 0.75]",
      "implements_auth KeyStrategy -> auth:api:api_key [heuristic 0.75]",
    ])
  );
}

#[test]
fn should_classify_axum_middleware_and_extractors() {
  let source = "use axum::{extract::{FromRequestParts, Request}, http::header::AUTHORIZATION, middleware::Next};\n\
pub struct Bearer(String);\n\
impl<S: Send + Sync> FromRequestParts<S> for Bearer {\n  type Rejection = ();\n  async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ()> {\n    parts.headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix(\"Bearer \")).map(|t| Bearer(t.into())).ok_or(())\n  }\n}\n\
pub async fn require_key(request: Request, next: Next) -> Response {\n  if request.headers().get(\"x-service-key\").is_none() { return reject(); }\n  next.run(request).await\n}\n\
pub async fn not_auth(request: Request, next: Next) -> Response { next.run(request).await }\n";
  assert_eq!(
    render_source(&AuthSchemeDetector, "svc", "src/auth.rs", source),
    lines(&[
      "auth:svc:api_key [heuristic 0.80]",
      "auth:svc:bearer [heuristic 0.85]",
      "implements_auth Bearer -> auth:svc:bearer [heuristic 0.85]",
      "implements_auth require_key -> auth:svc:api_key [heuristic 0.80]",
    ])
  );
}

#[test]
fn should_not_classify_client_code_or_permission_guards_as_auth_schemes() {
  let source = "export class ApiClient {\n  headers() { return { Authorization: `Bearer ${this.token}`, 'x-api-key': this.key }; }\n}\n\
export function handler(req, res) { res.cookie('sid', '1'); }\n";
  assert!(render_source(&AuthSchemeDetector, "web", "src/client.ts", source).is_empty());
}
