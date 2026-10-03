# Code Atlas

Code Atlas indexes a group of Rust and TypeScript repositories into one SQLite knowledge graph per project and serves it to coding agents over a CLI (`code-atlas`) and an MCP stdio server.
Every fact carries provenance (certainty + evidence), and no query answers from an index older than the working tree.

## Crates (`crates/`)

- `atlas-core` — ids, keys, kinds, certainty/evidence, the `FileFacts` IR, analyzer/detector/LLM traits, errors
- `atlas-store` — SQLite schema, migrations, per-file fact ownership, traversal primitives, run ledger
- `atlas-project` — project registry, cwd inference, discovery, hashing, stat cache, manifests
- `atlas-lang-rust` / `atlas-lang-ts` — tree-sitter analyzers producing `FileFacts` (depend on core only)
- `atlas-resolve` — Rust module/use and TS import resolution, call targets with certainty
- `atlas-detect` — framework detectors: endpoints, clients, auth, env/config, tests, events, jobs, db
- `atlas-scip` — SCIP generation/ingestion that upgrades edges to `compiler_verified`
- `atlas-similarity` — MinHash/LSH near-duplicates and BM25 search
- `atlas-semantic` — `LlmProvider` backends, call cache and ledger, inferred facts
- `atlas-link` — cross-repo links: packages, API↔client, auth, events, config
- `atlas-engine` — full/incremental pipeline, lineage, freshness barrier, watcher
- `atlas-query` — query surface shared by CLI and MCP
- `atlas-mcp`, `atlas-cli` (binary `code-atlas`), `atlas-bench`, `atlas-eval`

The plan lives in `docs/plan/implementation-plan.md`.

## Rules

- Never present heuristic or inferred knowledge as certain. Every edge and derived entity carries exactly one `Certainty`. Query output may state a fact as known only if `Certainty::is_authoritative()` is true; every other fact carries its tag (`[heuristic 0.72]`, `[inferred 0.81 stale]`).
- `Certainty::is_llm_derived()` (only `Inferred`) separates LLM facts, which can go stale, from file-owned reproducible facts. Uncertainty is never silently upgraded.
- `atlas-core` contracts are append-only: add items or `#[non_exhaustive]` variants, never change existing signatures or key formats. `EntityId` values are pinned by tests; changing key rendering is a breaking change.
- Each language analyzer depends on `atlas-core` alone and must never panic on any input.

## Knowledge-model contracts

- Store writes take `(Owner, RunId, entities, edges, unresolved)`; owner and run are not fields on the DTOs.
- `EdgeId` discriminator is `""` by default: one edge per (kind, from, to) with merged evidence. Use a non-empty discriminator only when parallel edges mean different things, e.g. two `calls_endpoint` edges from one function to one endpoint with different auth headers (`"auth:bearer"`, `"auth:api-key"`).
- `FileFacts::symbol_keys()` returns keys positionally parallel to `symbols`. `~n` disambiguators are per file in source order, so reordering overloads changes keys (accepted). Cross-file key collisions (e.g. Rust `#[path]` aliasing) are handled by multi-owner entity semantics in the store.
- `Entity::from_symbol()` is the only way to turn a `SymbolFact` into an `Entity` (`Syntactic`, attrs under the `ATTR_*` names).
- `FileFacts.impls` holds one `ImplFact` per `impl` block, including empty marker impls and impls nested in fn bodies (`scope` = enclosing symbol); the resolver derives cross-file `implements` edges from it. `FileFacts.file_doc` carries file-level `//!` / `/*! */` docs.
- `ReferenceFact.origin` is `Syntax` by default. `MacroTokenScan` refs come from a token scan of macro bodies that do not parse as expressions; the resolver must cap their certainty at `heuristic`, never `resolved`.
- Fields added to existing IR structs use `#[serde(default, skip_serializing_if = ...)]` so serialized output is unchanged when they are empty or default.
- Unresolved refs are stored for display and debugging only; re-resolution re-analyzes the owner file, so the resolver never depends on stored refs alone.
- Scoping: env vars (`env:<repo>:<NAME>`), config keys, endpoints, auth schemes, permissions and jobs are repo-scoped. Events, queues and db tables are project-scoped, which assumes one logical database and one event bus per project.
- DB models: a table is an `EntityKey::DbTable`; a model class stays a symbol entity with its normal kind (class/struct) and gets a `maps_to_table` edge to the table. `EntityKind::DbModel` exists only for `Other` keys; detectors use the convention above.
- Client URL templates and base-URL composition are resolved by `atlas-link`; detectors record the raw `ArgSummary` pattern.
- TS member-chain references: `path` always starts with the chain head (`this`, `super`, a name, or `(expr)` for any other expression; computed members are `[expr]` segments) and `receiver` describes `path[0]`: `This`, `Identifier` for a non-local name, `TypeHint`/`Expr` for a local binding or expression. A head marked `TypeHint`/`Expr` is never resolved as an import or module symbol.
- `ReceiverHint::TypeHint` = declared type only (annotation, param type, struct literal, allowlisted newtype extractor such as axum `State(s): State<T>`), never inferred from a call such as `T::new()`; the resolver may infer from a constructor's signature with `heuristic` certainty.
- `ReferenceFact.origin` = `MacroTokenScan` also covers macro arguments that only parse piece by piece, except for known expression-macro families (std format/assert, `tracing`/`log` level macros, `anyhow!`/`bail!`/`ensure!`), which keep `Syntax`.

## Conventions

- Rust 2024, 2-space indent, 180 columns (`rustfmt.toml`).
- No comments unless the code cannot say it (external behaviour, non-obvious constraint, a bug the shape prevents). No doc blocks restating names, no dead code, no TODOs.
- Typed errors via `thiserror` per crate; no `unwrap`/`expect` on fallible runtime paths outside tests.
- No `println!`/`eprintln!` in library code; use `tracing` with structured fields.
- Model states as enums / discriminated unions, never parallel booleans. Prefer early returns.
- Tests are named `should_*` and assert exact values.
- New third-party dependencies go in `[workspace.dependencies]` and need a stated justification.

## Checks

```
./scripts/check.sh   # fmt --check, clippy -D warnings, test --workspace
```
