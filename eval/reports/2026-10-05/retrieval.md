# Retrieval report

> Dev set, same authors: the tasks, their ground truth and Code Atlas were written by the same people, and the tasks were used while developing the product. Read these numbers as regression signals for the retrieval surface, not as an unbiased estimate of how it performs on unseen code.

Deterministic and LLM-free: every task's base tree is materialised like the agent harness does it (no setup commands), indexed in process, and queried only with the task prompt, the way a careful agent would before editing. Ground truth is used only to score, and to pick the blast-radius seed symbols.

## Settings

| key | value |
|---|---|
| code atlas revision | 0f3557fefcc02fedbaea4669cc90937a4b263701 |
| context budget (CLI and MCP default) | 6000 |
| search limit per query | 10 |
| searches | the prompt, up to 8 code spans, up to 4 prose phrases |
| similar seeds | up to 5 top function/method search hits |
| impact group limit (visible) | 15 |

## Aggregate (mean over the 11 of 11 tasks that define each metric)

| metric | mean | tasks |
|---|---|---|
| must_change.recall.must_read | 0.464 | 11 |
| must_change.recall.pack | 0.685 | 11 |
| must_change.recall.search | 0.694 | 11 |
| must_change.recall.similar | 0.439 | 11 |
| must_change.recall.any | 0.87 | 11 |
| relevant_files.recall.must_read | 0.386 | 11 |
| relevant_files.recall.pack | 0.535 | 11 |
| relevant_files.recall.search | 0.726 | 11 |
| relevant_files.recall.similar | 0.365 | 11 |
| relevant_files.recall.any | 0.842 | 11 |
| must_consider.recall.must_read | 0.273 | 11 |
| must_consider.recall.pack | 0.364 | 11 |
| must_consider.recall.search | 0.75 | 11 |
| must_consider.recall.similar | 0.341 | 11 |
| must_consider.recall.any | 0.826 | 11 |
| relevant_files.precision.must_read | 0.372 | 11 |
| relevant_files.precision.pack | 0.232 | 11 |
| relevant_files.precision.search | 0.113 | 11 |
| relevant_files.precision.similar | 0.186 | 11 |
| relevant_files.precision.any | 0.097 | 11 |
| reuse_targets.recall.pack | 0.4 | 4 |
| reuse_targets.recall.search | 0.6 | 4 |
| reuse_targets.recall.similar | 0.05 | 4 |
| reuse_targets.recall.any | 0.7 | 4 |
| blast_radius.recall.visible | 1 | 2 |
| blast_radius.recall.full | 1 | 2 |
| pack.bytes | 22438.818 | 11 |
| pack.estimated_tokens | 3370.909 | 11 |

## t00-smoke: Smoke: soft-delete timestamp on the users entity

Categories: smoke.

Index: 65 files in acme-api, acme-sdk, acme-web, acme-billing.
Context pack: 10216 bytes, ~1811 of 6000 tokens; items: must read 11, skim 14, patterns 0, reuse 0, impact 4, tests 5, uncertain 2, omitted 8.

Searches: the full prompt, then `deletedAt`, `User`, `deleted_at`, `UsersService.findOne`, `database column named`, `soft-delete column decorator`, `change minimal`.
Similar seeds: `ts:acme-api:acme-api:src/users/users.service::UsersService::findOne#method` (members 10, shape-only 0), `ts:acme-api:acme-api:src/users/users.controller::UsersController::remove#method` (members 1, shape-only 0), `ts:acme-web:acme-web:src/hooks/useUser::useUser#function` (members 3, shape-only 0), `rust:acme-billing:acme-billing:store::PgInvoiceStore::database_url#method` (members 1, shape-only 0), `ts:acme-api:acme-api:src/users/users.controller::UsersController::update#method` (members 5, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/1) | 0 (0/1) | 1 (1/1) | 0 (0/1) | 1 (1/1) |
| relevant_files recall | 0.5 (1/2) | 0.5 (1/2) | 1 (2/2) | 0.5 (1/2) | 1 (2/2) |
| must_consider recall | 1 (2/2) | 1 (2/2) | 1 (2/2) | 1 (2/2) | 1 (2/2) |
| relevant_files precision | 0.2 | 0.062 | 0.143 | 0.1 | 0.083 |

## t01-cwd-relative-targets: CLI query targets accept paths relative to the working directory

Categories: find-and-extend, reuse.

Index: 491 files in tree.
Context pack: 24995 bytes, ~3502 of 6000 tokens; items: must read 22, skim 14, patterns 0, reuse 0, impact 6, tests 8, uncertain 10, omitted 63.

Searches: the full prompt, then `code-atlas query similar --snippet`, `acme-billing/src`, `--snippet money.rs:1-15`, `acme-api/src/users`, `code-atlas query symbol users.service.ts:18`, `query callers ./users.service.ts:18`, `code-atlas query`, `symbol`, `path relative`, `understands paths relative`, `repository root`, `absolute path`.
Similar seeds: `rust:tree:atlas-cli/bin/atlas-cli:commands/query::run#function` (members 10, shape-only 0), `rust:tree:atlas-query:queries/similar::snippet_target#function` (members 7, shape-only 0), `rust:tree:atlas-resolve/test/acme:::billing#function` (members 10, shape-only 0), `ts:tree:globex-api:src/util/money::formatMoney#function` (members 6, shape-only 0), `ts:tree:acme-web:src/hooks/useMe::useMe#function` (members 4, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) |
| relevant_files recall | 0.2 (1/5) | 0.2 (1/5) | 0.6 (3/5) | 0.4 (2/5) | 0.8 (4/5) |
| must_consider recall | 0 (0/4) | 0.25 (1/4) | 0.5 (2/4) | 0.25 (1/4) | 0.75 (3/4) |
| relevant_files precision | 0.083 | 0.033 | 0.048 | 0.069 | 0.041 |

- relevant_files not retrieved anywhere: `crates/atlas-cli/src/context.rs`
- must_consider not retrieved anywhere: `crates/atlas-cli/src/context.rs`

| reuse target | pack | search | similar |
|---|---|---|---|
| `query::repo_relative` | no | yes | no |

## t02-typed-degraded-cause: Typed cause for degraded freshness reports

Categories: blast-radius.

Index: 491 files in tree.
Context pack: 28961 bytes, ~4077 of 6000 tokens; items: must read 24, skim 14, patterns 0, reuse 1, impact 4, tests 8, uncertain 2, omitted 32.

Searches: the full prompt, then `code-atlas watch`, `FreshnessReport::Degraded`, `cause: DegradedCause`, `reason: String`, `DegradedCause`, `atlas_engine`, `ProjectConfigChanged`, `UpdateFailed { error: String }`, `process changed`, `project configuration`, `project configuration changed`, `free-text reason`.
Similar seeds: `rust:tree:atlas-engine:engine::Engine::barrier#method` (members 3, shape-only 0), `rust:tree:atlas-mcp:options::ServeOptions::with_watch#method` (members 3, shape-only 0), `rust:tree:atlas-query:freshness::Freshness::from_report#method` (members 3, shape-only 0), `rust:tree:atlas-cli/bin/atlas-cli:output::Outcome::degraded_if#method` (members 2, shape-only 0), `rust:tree:atlas-resolve/test/workspace_quality:::reason_label#function` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.5 (3/6) | 1 (6/6) | 0.833 (5/6) | 0.333 (2/6) | 1 (6/6) |
| relevant_files recall | 0.4 (4/10) | 0.7 (7/10) | 0.8 (8/10) | 0.3 (3/10) | 1 (10/10) |
| must_consider recall | 0.167 (1/6) | 0.167 (1/6) | 0.167 (1/6) | 0.333 (2/6) | 0.5 (3/6) |
| relevant_files precision | 0.4 | 0.389 | 0.143 | 0.167 | 0.141 |

- relevant_files unindexable, never in the index (not scored): `CLAUDE.md`
- must_consider not retrieved anywhere: `crates/atlas-cli/src/commands/enrich.rs`, `crates/atlas-cli/src/commands/scip.rs`, `crates/atlas-mcp/src/server.rs`
- must_consider unindexable, never in the index (not scored): `CLAUDE.md`

Blast radius: `impact` on `rust:tree:atlas-engine:report::FreshnessReport::Degraded#variant` (symbols in `crates/atlas-engine/src/report.rs` that `reference.patch` changes) against the 5 other must_change files. Precision is against must_change only, so it is low by design; recall is the score.

| impact view | recall | hits | files retrieved | precision |
|---|---|---|---|---|
| visible (16 items collapsed) | 1 | 5/5 | 28 | 0.179 |
| full | 1 | 5/5 | 34 | 0.147 |

## t03-nested-router-links: Planted bug: routes of Express routers nested across files are never composed or linked

Categories: planted-bug, dependency-trace.

Index: 491 files in tree.
Context pack: 16417 bytes, ~2156 of 6000 tokens; items: must read 3, skim 26, patterns 5, reuse 2, impact 13, tests 2, uncertain 9, omitted 35.

Searches: the full prompt, then `GET /api/orders/{id}`, `GET /{id}`, `fetch(`, `calls_endpoint`, `app.use('/health', healthRouter)`, `health.ts`, `Bug report`, `Code Atlas stopped linking`, `web client`, `Express routers nested`.
Similar seeds: `ts:tree:globex-api:src/routes/orders.routes::registerOrderRoutes#function` (members 7, shape-only 0), `rust:tree:acme-billing:acme_client::AcmeApiClient::get_order#method` (members 8, shape-only 0), `ts:tree:@acme/sdk:src/client::AcmeClient::getOrder#method` (members 10, shape-only 0), `rust:tree:atlas-detect:detectors/clients::fetch_request#function` (members 10, shape-only 0), `rust:tree:atlas-link:http::calls_endpoint#function` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/1) | 1 (1/1) | 0 (0/1) | 0 (0/1) | 1 (1/1) |
| relevant_files recall | 0.25 (1/4) | 0.75 (3/4) | 0.5 (2/4) | 0.5 (2/4) | 0.75 (3/4) |
| must_consider recall | 0 (0/2) | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) |
| relevant_files precision | 0.333 | 0.097 | 0.035 | 0.074 | 0.037 |

- relevant_files not retrieved anywhere: `crates/atlas-detect/tests/endpoints.rs`
- must_consider not retrieved anywhere: `crates/atlas-detect/tests/endpoints.rs`

## t04-detector-import-gates: Refactor: one shared place for detector import checks

Categories: refactor.

Index: 491 files in tree.
Context pack: 32814 bytes, ~5128 of 6000 tokens; items: must read 20, skim 20, patterns 3, reuse 0, impact 6, tests 8, uncertain 4, omitted 38.

Searches: the full prompt, then `atlas_core::ImportFact`, `import`, `require()`, `use`, `crates/atlas-detect/src/detectors/`, `ImportFact`, `extern crate`, `Tech debt`, `several framework detectors decide`, `path-qualified references`, `logic exists`.
Similar seeds: `rust:tree:atlas-query:queries/context::route_mentions#function` (members 10, shape-only 0), `rust:tree:atlas-resolve:output::Emitter::ts_import#method` (members 10, shape-only 0), `ts:tree:acme-api:src/auth/require-permission.decorator::RequirePermission#function` (members 8, shape-only 0), `rust:tree:atlas-resolve:surface::FileSurface::uses_in#method` (members 1, shape-only 1), `rust:tree:atlas-detect:registry::DetectorRegistry::run_all#method` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.8 (4/5) | 0.8 (4/5) | 1 (5/5) | 0.6 (3/5) | 1 (5/5) |
| relevant_files recall | 0.778 (7/9) | 0.778 (7/9) | 1 (9/9) | 0.333 (3/9) | 1 (9/9) |
| must_consider recall | 0.667 (2/3) | 0.667 (2/3) | 1 (3/3) | 0 (0/3) | 1 (3/3) |
| relevant_files precision | 0.7 | 0.35 | 0.184 | 0.111 | 0.132 |

## t05-mcp-dependency-tools: Expose the dependents/dependencies traversals as MCP tools

Categories: architecture-addition, test-discovery.

Index: 491 files in tree.
Context pack: 19093 bytes, ~2804 of 6000 tokens; items: must read 12, skim 19, patterns 0, reuse 5, impact 0, tests 8, uncertain 1, omitted 5.

Searches: the full prompt, then `code-atlas query dependents <target>`, `code-atlas query dependencies <target>`, `atlas_callers`, `atlas_callees`, `atlas_dependents`, `atlas_dependencies`, `target`, `depth`, `Agents connected through`, `MCP server`, `follow imports`, `type usage`.
Similar seeds: `rust:tree:atlas-mcp:server::AtlasServer::callees#method` (members 10, shape-only 0), `rust:tree:atlas-query:queries/traverse::QueryEngine::dependents#method` (members 8, shape-only 0), `rust:tree:atlas-query:queries/traverse::QueryEngine::dependencies#method` (members 8, shape-only 0), `rust:tree:atlas-mcp:server::AtlasServer::callers#method` (members 10, shape-only 0), `rust:tree:atlas-query:queries/traverse::QueryEngine::callees#method` (members 9, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) |
| relevant_files recall | 0.571 (4/7) | 0.571 (4/7) | 0.714 (5/7) | 0.571 (4/7) | 0.857 (6/7) |
| must_consider recall | 0.5 (2/4) | 0.5 (2/4) | 0.75 (3/4) | 0.5 (2/4) | 1 (4/4) |
| relevant_files precision | 0.8 | 0.286 | 0.096 | 0.571 | 0.097 |

- must_change not retrieved anywhere: `crates/atlas-mcp/tests/protocol.rs`
- relevant_files not retrieved anywhere: `crates/atlas-mcp/tests/protocol.rs`

| reuse target | pack | search | similar |
|---|---|---|---|
| `QueryEngine::dependents` | yes | yes | no |
| `QueryEngine::dependencies` | no | yes | no |
| `render::targeted` | yes | no | yes |
| `arguments::depth` | yes | no | no |
| `arguments::TraverseArgs` | no | no | no |

## t06-repo-file-size-limit: Per-repository max_file_bytes override

Categories: config-impact, blast-radius.

Index: 491 files in tree.
Context pack: 18511 bytes, ~2706 of 6000 tokens; items: must read 21, skim 17, patterns 0, reuse 1, impact 0, tests 8, uncertain 0, omitted 9.

Searches: the full prompt, then `max_file_bytes`, `[project.settings]`, `projects.toml`, `[[project.repo]]`, `Per-repository file size limit`, `mixed projects`, `large hand-maintained TypeScript files`, `want indexed`.
Similar seeds: `rust:tree:atlas-cli/bin/atlas-cli:commands/project::apply_settings#function` (members 8, shape-only 0), `rust:tree:atlas-lang-rust:analyzer::RustAnalyzer::with_max_source_bytes#method` (members 1, shape-only 0), `rust:tree:atlas-project:registry::Registry::set_settings#method` (members 10, shape-only 0), `rust:tree:atlas-scip/test/rust_analyzer:::expected#function` (members 6, shape-only 0), `rust:tree:atlas-link:facts::Facts::in_project#method` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.182 (2/11) | 0.364 (4/11) | 0.545 (6/11) | 0.273 (3/11) | 0.818 (9/11) |
| relevant_files recall | 0.429 (3/7) | 0.571 (4/7) | 0.714 (5/7) | 0.286 (2/7) | 0.857 (6/7) |
| must_consider recall | 0.25 (1/4) | 0.25 (1/4) | 0.75 (3/4) | 0.25 (1/4) | 0.75 (3/4) |
| relevant_files precision | 0.333 | 0.222 | 0.122 | 0.118 | 0.1 |

- must_change not retrieved anywhere: `crates/atlas-query/tests/perf.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`
- relevant_files not retrieved anywhere: `crates/atlas-engine/src/engine.rs`
- must_consider not retrieved anywhere: `crates/atlas-engine/src/engine.rs`

| reuse target | pack | search | similar |
|---|---|---|---|
| `discovery::discover` | no | no | no |

Blast radius: `impact` on `rust:tree:atlas-project:registry::RepoConfig#struct`, `rust:tree:atlas-project:registry::build_repo#function` (symbols in `crates/atlas-project/src/registry.rs` that `reference.patch` changes) against the 10 other must_change files. Precision is against must_change only, so it is low by design; recall is the score.

| impact view | recall | hits | files retrieved | precision |
|---|---|---|---|---|
| visible (119 items collapsed) | 1 | 10/10 | 25 | 0.4 |
| full | 1 | 10/10 | 40 | 0.25 |

## t07-static-precompressed-quality: serveStatic precompressed variants ignore Accept-Encoding quality values

Categories: reuse-trap.

Index: 486 files in tree.
Context pack: 28628 bytes, ~4671 of 6000 tokens; items: must read 30, skim 7, patterns 0, reuse 0, impact 0, tests 8, uncertain 0, omitted 35.

Searches: the full prompt, then `serveStatic({ precompressed: true })`, `Accept-Encoding: gzip;q=1.0, br;q=0.5`, `app.js.gz`, `app.js.br`, `app.js`, `Accept-Encoding: br;q=0.2, gzip;q=0.8`, `Accept-*`, `q=0`, `precompressed variant`, `client lists`, `bare token`, `Real browsers`.
Similar seeds: `ts:tree:hono:src/middleware/serve-static/index::serveStatic#function` (members 9, shape-only 0), `ts:tree:@hono/cloudflare-workers:src/serve-static::serveStatic#function` (members 10, shape-only 0), `ts:tree:hono:benchmarks/http-server/benchmark::getAppTemplate#function` (members 1, shape-only 0), `ts:tree:hono:src/utils/encode.test::str2UInt8Array#function` (members 1, shape-only 0), `ts:tree:hono:src/utils/accept::parseAccept#function` (members 7, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) |
| relevant_files recall | 0.4 (2/5) | 0.4 (2/5) | 0.6 (3/5) | 0.4 (2/5) | 0.6 (3/5) |
| must_consider recall | 0.167 (1/6) | 0.167 (1/6) | 0.833 (5/6) | 0.667 (4/6) | 0.833 (5/6) |
| relevant_files precision | 0.182 | 0.143 | 0.058 | 0.1 | 0.045 |

- relevant_files not retrieved anywhere: `src/middleware/serve-static/index.test.ts`, `src/utils/compress.ts`
- must_consider not retrieved anywhere: `src/middleware/serve-static/index.test.ts`

| reuse target | pack | search | similar |
|---|---|---|---|
| `src/utils/accept::parseAccept` | yes | yes | no |

## t08-cookie-samesite-default: Default SameSite=Lax for cookies created by the cookie helper

Categories: test-discovery, cross-package.

Index: 486 files in tree.
Context pack: 26501 bytes, ~4230 of 6000 tokens; items: must read 19, skim 19, patterns 0, reuse 0, impact 1, tests 8, uncertain 0, omitted 35.

Searches: the full prompt, then `hono/cookie`, `setCookie`, `setSignedCookie`, `generateCookie`, `generateSignedCookie`, `deleteCookie`, `SameSite`, `Set-Cookie`, `Cookies created through`, `attribute unless`, `caller passes`, `Browsers disagree`.
Similar seeds: `ts:tree:hono:src/utils/cookie::serializeSigned#function` (members 10, shape-only 0), `ts:tree:hono:src/helper/cookie/index::getCookie#function` (members 1, shape-only 0), `ts:tree:hono:src/helper/cookie/index::setCookie#function` (members 5, shape-only 0), `ts:tree:hono:src/helper/cookie/index::setSignedCookie#function` (members 8, shape-only 0), `ts:tree:hono:src/helper/cookie/index::generateCookie#function` (members 6, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.125 (1/8) | 0.625 (5/8) | 0.5 (4/8) | 0.125 (1/8) | 0.75 (6/8) |
| relevant_files recall | 0.222 (2/9) | 0.667 (6/9) | 0.556 (5/9) | 0.222 (2/9) | 0.778 (7/9) |
| must_consider recall | 0.25 (1/4) | 0.25 (1/4) | 1 (4/4) | 0.25 (1/4) | 1 (4/4) |
| relevant_files precision | 0.286 | 0.4 | 0.143 | 0.5 | 0.159 |

- must_change not retrieved anywhere: `adapters/aws-lambda/src/handler.test.ts`, `adapters/aws-lambda/test/index.test.ts`
- relevant_files not retrieved anywhere: `adapters/aws-lambda/src/handler.test.ts`, `adapters/aws-lambda/test/index.test.ts`

## t09-lambda-text-content-types: Lambda adapters base64-encode textual responses such as text/markdown and text/event-stream

Categories: cross-package, consistency.

Index: 486 files in tree.
Context pack: 32075 bytes, ~5003 of 6000 tokens; items: must read 32, skim 4, patterns 0, reuse 10, impact 0, tests 8, uncertain 10, omitted 67.

Searches: the full prompt, then `Content-Type: text/markdown`, `text/event-stream`, `text/calendar`, `application/javascript`, `bodyEncoding: 'base64'`, `text/*`, `application/x-www-form-urlencoded`, `charset`, `come back base64-encoded`, `AWS Lambda deployment`, `API Gateway REST`, `media types configured shows`.
Similar seeds: `ts:tree:@hono/aws-lambda:src/handler::EventProcessor::createResult#method` (members 10, shape-only 0), `ts:tree:hono:src/helper/ssg/ssg::parseResponseContent#function` (members 10, shape-only 0), `ts:tree:hono:src/helper/streaming/text::streamText#function` (members 5, shape-only 0), `ts:tree:hono:src/context::Context::text#method` (members 2, shape-only 0), `ts:tree:hono:src/jsx/dom/hooks/index::useFormStatus#function` (members 4, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.75 (3/4) | 1 (4/4) | 1 (4/4) | 0.75 (3/4) | 1 (4/4) |
| relevant_files recall | 0.375 (3/8) | 0.5 (4/8) | 1 (8/8) | 0.375 (3/8) | 1 (8/8) |
| must_consider recall | 0 (0/4) | 0 (0/4) | 1 (4/4) | 0 (0/4) | 1 (4/4) |
| relevant_files precision | 0.273 | 0.235 | 0.163 | 0.167 | 0.136 |

## t10-auth-challenge-quoting: WWW-Authenticate challenges do not escape backslashes in quoted-string parameters

Categories: cross-cutting, consistency.

Index: 486 files in tree.
Context pack: 8616 bytes, ~992 of 6000 tokens; items: must read 2, skim 6, patterns 0, reuse 0, impact 0, tests 8, uncertain 0, omitted 3.

Searches: the full prompt, then `bearerAuth({ token, realm: 'C:\\vault' })`, `WWW-Authenticate: Bearer realm="C:\vault"`, `C:vault`, `invalidToken: { wwwAuthenticateHeader: { error: 'invalid_token', error_description: 'bad "token"' } }`, `WWW-Authenticate`, `pnpm`, `npx pnpm@12.6.0 ...`, `./node_modules/.bin`, `clients receive`, `unterminated quoted-string`, `Double quotes`, `cut short`.
Similar seeds: `ts:tree:hono:src/middleware/bearer-auth/index::bearerAuth#function` (members 1, shape-only 0), `ts:tree:hono:src/utils/jwt/jwt::decodeHeader#function` (members 2, shape-only 0), `ts:tree:hono:src/utils/ipaddr::parseIPv6HexCode#function` (members 10, shape-only 0), `ts:tree:@hono/cloudflare-workers:src/serve-static-module::module#function` (members 8, shape-only 0), `ts:tree:hono:src/helper/websocket/index::createWSMessageEvent#function` (members 2, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.25 (1/4) | 0.25 (1/4) | 0.25 (1/4) | 0.25 (1/4) | 0.5 (2/4) |
| relevant_files recall | 0.125 (1/8) | 0.25 (2/8) | 0.5 (4/8) | 0.125 (1/8) | 0.625 (5/8) |
| must_consider recall | 0 (0/4) | 0.25 (1/4) | 0.75 (3/4) | 0 (0/4) | 0.75 (3/4) |
| relevant_files precision | 0.5 | 0.333 | 0.111 | 0.071 | 0.098 |

- must_change not retrieved anywhere: `src/middleware/jwk/jwk.ts`, `src/middleware/jwt/jwt.ts`
- relevant_files not retrieved anywhere: `src/middleware/basic-auth/index.test.ts`, `src/middleware/jwk/jwk.ts`, `src/middleware/jwt/jwt.ts`
- must_consider not retrieved anywhere: `src/middleware/basic-auth/index.test.ts`

