# Fixtures

Test data for analyzers, linker, query engine and quality evaluation. These are not workspace crates: every Rust fixture has its own empty `[workspace]` table. Ground truth lives in `expected.toml` files, format in `expected.schema.md`.

| Fixture | Languages | What it tests |
|---|---|---|
| `acme/acme-api` | TS (NestJS) | controllers and literal/const routes, three auth schemes (bearer, `x-api-key`, `acme_session` cookie), `@RequirePermission`, events, env/config reads, TypeORM entities, jest tests, unused code |
| `acme/acme-sdk` | TS | fetch client with templated URLs and `auth` union, barrel re-exports, vitest tests |
| `acme/acme-web` | TSX | cross-repo `file:` dependency, SDK calls through an imported instance, a direct fetch that matches `/users/:id` only at runtime (`/users/me`), an unmatched `/health` call, a semantically equivalent duplicate of `toCents` |
| `acme/acme-billing` | Rust (axum, reqwest) | route registration, outgoing HTTP with `x-api-key`, env reads, event subscriber, trait + `dyn` dispatch, unit and integration tests; passes `cargo check --all-targets` |
| `globex/globex-api` | TS (Express) | project isolation: same routes, `UsersService`, `formatMoney`, `DATABASE_URL` as acme |
| `globex/globex-worker` | Rust | project isolation: `to_cents` with different semantics, its own `order.created` consumer; passes `cargo check` |
| `adversarial/shadowing-ts` | TS | scope-based resolution beats name matching |
| `adversarial/reexport-chain-ts` | TS | three-barrel re-export chain with alias resolves to the original definition |
| `adversarial/duplicate-names-rust` | Rust | same-name functions, explicit-over-glob, genuine glob ambiguity |
| `adversarial/trait-dispatch-rust` | Rust | generic bound, `impl Trait`, `dyn`, concrete receivers: resolved vs heuristic edges |
| `adversarial/dynamic-ts` | TS | computed member calls, `require(var)`, template `import()`, `eval`: unresolved, no fabricated edges |
| `adversarial/computed-routes-ts` | TS (Express, axios) | router mounting, constant propagation into routes, concatenated URLs, fully dynamic routes |
| `adversarial/generated-code` | TS, Rust | `@generated` file flagging, `include!(concat!(env!("OUT_DIR"), ...))` stays unresolved |
| `adversarial/similar-not-equivalent` | TS | same shape different behaviour, same name different behaviour, different shape same behaviour |
| `adversarial/broken-syntax` | TS, Rust | partial-parse extraction around a syntax error |
| `adversarial/ts-overrides` | TS (jest) | method-level `implements` from implementations and overrides (`via` interface/class), heritage labelled `implements`/`extends` instead of `references`, calls through an interface |
| `adversarial/macro-heavy-rust` | Rust | `macro_rules!`-generated functions are never resolved |
