# Languages and frameworks

Supported file extensions: `rs`; `ts mts cts`; `tsx`; `js mjs cjs`; `jsx` (`Language::from_extension`). `.d.ts` declaration files are recognised as such. Analyzers are tree-sitter based (`tree-sitter-rust`, `tree-sitter-typescript`, `tree-sitter-javascript`). A repo may restrict languages with `languages` in `projects.toml`.

## Depth per language

| Capability | Rust | TypeScript / JavaScript (TSX, JSX) |
|---|---|---|
| Symbols | fn, method, struct, enum, variant, trait, type alias, const/static, field, module, macro (`macro_rules!`), tests | function, method, class, interface, type alias, enum, const/variable, field, namespace, tests and suites |
| Signatures, docs, visibility, modifiers, attributes | yes (`//!` file docs too) | yes (decorators with literal arguments recorded) |
| Imports and exports | `use` trees, globs, `extern crate`, `mod` declarations, `#[path]`, inline modules | ESM `import`/`export`, `export *`, re-export chains, default exports, `require()` as import, barrel files |
| Module system | module tree from each Cargo target root (lib, bins, tests, examples, benches, build script) with rustc mod-rs rules; `cfg`-gated files keep their path and record the cfg text | relative paths, `tsconfig` `paths`/`baseUrl`, package `exports`, workspace packages, dist-to-src mapping (heuristic 0.85 per hop) |
| Call resolution | path calls, method calls on typed receivers (declared types, params, annotated lets, struct literals, newtype extractors), `Self::`, trait-method calls via bounds, `Box`/`Arc`/`Rc` wrappers, field accesses (reads and writes) | scope-based (shadowing aware), namespace-aware (value vs type), `this`/`super`, instance vs static members, overloads resolve to the implementation signature, JSX elements, decorators |
| Dispatch | `dyn Trait` and interface candidates heuristic 0.3; generic bounds resolve to the trait method | interface and subclass override candidates heuristic 0.3 |
| Unresolved | untyped `let x = T::new()` receivers, expression chains, macro-generated names, fn-local items, `self` in items nested in bodies (`Unsupported`) | computed members (`a[b]()`), dynamic `require`/`import()`, rebound `this` and `this: T` annotations inside classes (`Unsupported`, no candidates; outside classes name-only heuristic candidates) |
| Parse-error tolerance | partial parse: facts around errors extracted, up to 8 errors reported | same |
| Similarity and lineage | body fingerprints (normalized token streams, capped at 4,096 tokens) | same |
| Tests | `#[test]`, `rstest`, `test_case`, `<framework>::test` attributes (for example `tokio::test`); `#[cfg(test)]` modules; `tests/` integration directory | `describe`/`it`/`test` calls; frameworks identified: `jest` (globals and `@jest/globals`), `vitest`, `mocha`; `node:test` and `bun:test` recognised as unknown framework; `.spec.`/`.test.` files and `__tests__`/`test`/`tests` directories |
| Compiler-grade verification | rust-analyzer SCIP | scip-typescript SCIP |

`atlas-lang-rust` and `atlas-lang-ts` must never panic on any input; a failure becomes a failed-file record, not a crash. Details of resolver certainty rules are in [knowledge model](knowledge-model.md#certainty-classes) and `crates/atlas-resolve/CLAUDE.md`.

## Frameworks and detectors

Ten detectors run on every analyzed file (`DetectorRegistry::default`). Each emits entities and edges with its own certainty, plus diagnostics (for example `route-dynamic`, `http.dynamic_url`).

| Detector | Detects | Libraries and conventions | Tested on fixtures |
|---|---|---|---|
| `http-endpoints-nest` | endpoints, controller prefixes, global prefix, guards | NestJS (`@Controller`, `@Get/@Post/@Put/@Patch/@Delete/@All/@Head/@Options`, `@UseGuards`, permission decorators whose name ends in `Permission`, `Role` or `Scope`, e.g. `@RequirePermission`) | acme-api |
| `http-endpoints-express` | endpoints, router mounting, mounts across files | `express`, `fastify`, `koa-router`, `@koa/router` registration calls (`get post put patch delete del head options all`) | globex-api, computed-routes-ts (Express); Fastify and Koa in detector unit tests only |
| `http-endpoints-axum` | endpoints, `.nest` composition | `axum` builders; `actix_web` and `rocket` route attributes | acme-billing (axum); actix-web and Rocket in unit tests only |
| `http-clients` | outgoing requests: method, URL pattern, base-URL source, auth hints, headers | global `fetch`, `axios` (methods and config form), Nest `HttpService`, Rust `reqwest` (including `bearer_auth`, `basic_auth`); `new Sdk({ auth })` options | acme-sdk, acme-web, acme-billing |
| `auth` | auth schemes and guards | Nest guards, `@nestjs/passport` (`AuthGuard`, `PassportStrategy`, strategies `passport-jwt` etc.), Nest and Express middleware, axum middleware, Rust extractors; header and cookie reads (`Authorization`, `x-api-key`, cookies) classify `bearer`, `api_key`, `basic`, `session_cookie` | acme-api |
| `config-env` | env var and config reads | `process.env.X`, destructuring, `import.meta.env`, `std::env::var`/`var_os`, `dotenvy`, `ConfigService.get/getOrThrow` | acme, globex |
| `tests` | test to file coverage | naming conventions (`foo.spec.ts` covers `foo.ts`; Rust `tests/` and `_test.rs`), `#[cfg(test)]` modules | acme |
| `events` (experimental) | publishers and subscribers | `EventEmitter`/`EventEmitter2`, receivers named like a bus (`events emitter bus broker queue producer consumer pubsub`) calling `emit publish send` / `on once subscribe listen`; `@OnEvent` from `@nestjs/event-emitter`; Kafka-style `topic` options | acme, globex |
| `jobs` (experimental) | scheduled jobs | `@nestjs/schedule` (`@Cron`, `@Interval`, `@Timeout`), `node-cron`/`cron` `schedule`, top-level `setInterval`, `tokio_cron_scheduler::Job` | unit tests |
| `db` (experimental) | tables, models, reads and writes | TypeORM `@Entity`, `sequelize-typescript` `@Table`, Prisma client model calls, SQL string literals (`select insert update delete with merge replace`), `sqlx` macros and `query_as`-style calls | acme (TypeORM), unit tests |

Certainty highlights: literal routes and explicit table names are `syntactic`; computed routes, guard-to-scheme mapping, Prisma model calls (0.6), TypeORM default table names (0.6) and scheduler calls (0.7) are `heuristic`. Detector output is combined by the link pass into cross-repo facts (`calls_endpoint`, `supports_auth`, `uses_auth`, event producers and consumers), which are always `heuristic`.

## Experimental

Events, jobs and DB detectors tag every entity and edge they emit with `attrs.experimental = true`, and all their facts are heuristic. They assume one event bus and one logical database per project (events, queues and tables are project-scoped). Unmeasured: the quality evaluation checks the acme and globex event and `db_model` expectations (all found) but there is no precision measurement on real code. Treat them as leads.

Also experimental: LLM enrichment (inferred similarity verdicts) and rename lineage confidence calibration. See [trust and limitations](trust-and-limitations.md).

## Not supported

Other languages; GraphQL, gRPC, tRPC and RPC-style clients; HTTP clients other than those listed (`got`, `ky`, `superagent`, `ureq`, `hyper` clients); ORMs not listed (Drizzle, Diesel, SeaORM, Mongoose); message brokers beyond the bus-word heuristic; infrastructure files; environment value files.
