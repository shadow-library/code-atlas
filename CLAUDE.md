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
- `atlas-lsp` — blocking stdio JSON-RPC client, LSP types, server processes and pool, document sync (core only, no project or engine)
- `atlas-edit` — edit plans, path-guarded atomic application with rollback, text operations, symbol regions (no LSP, no engine)
- `atlas-ide` — `IdeService`: exact operations over language servers plus graph extras, shared by CLI and MCP
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
- `ReceiverHint::Declared { type_text, binding }` (Rust) replaces `TypeHint` when the head is a param or annotated `let` whose declared type is a generic parameter in scope or a `Box`/`Arc`/`Rc` wrapper, because the `TypeHint` path would drop the bounds or the inner type. `type_text` is the whitespace-normalised declared type. A shadowing binding without an annotation is `Expr`. Bindings inside fn or impl items nested in a body keep `TypeHint`/`Expr`, because their references are attributed to the outer symbol, whose generics differ.
- Rust field accesses (reads and writes, not method callees) are `ValueRef`s whose `path` is the `self` or identifier head plus the field chain, with `receiver` describing `path[0]`; path `ValueRef`s have no receiver. An accessed tuple field (`.0`) or an expression head emits nothing, and at most 4 spans are kept per (symbol, path, receiver).
- Inside fn, impl or trait items nested in a body (attributed to the outer symbol), Rust `self` heads are `Expr("self")` and a `Self` path head is rewritten to `(Self)`, so the resolver never binds them to the outer impl: `(Self)` paths are `Dynamic`, and `self` receivers are unknown.
- `ReferenceFact.origin` = `MacroTokenScan` also covers macro arguments that only parse piece by piece, except for known expression-macro families (std format/assert, `tracing`/`log` level macros, `anyhow!`/`bail!`/`ensure!`), which keep `Syntax`.
- `NodeRef::Unresolved { path, from }` (detector output only): the engine resolves `path` in the owner file with `atlas-resolve::resolve_path_in_file`, scoped to `from`. The stored edge keeps the weaker of the detector's and the resolution's certainty; an unresolvable path drops the edge and is counted.
- TS detector paths `[T.., "prototype", m]` resolve `m` on the instance side of class or interface `T`, following `extends`; any other kind of `T` is NotFound. A static member is `[T, m]`. Rust detectors never emit this form.
- `test-mocks` emits `references` edges from test code to the mocked member, `heuristic(0.6, "jest-mock-member")`. Attrs: `mock` (`ATTR_MOCK`) is `replaced` for a `jest.fn()`/`vi.fn()` property, a `createMock`/`jest.Mocked` binding or a `mockImplementation`/`mockReturnValue`/`mockResolvedValue`/`mockRejectedValue`/`mockReturnThis` setup (also chained on `spyOn`), and `spied` otherwise; `mock_type` (`ATTR_MOCK_TYPE`) says how `T` was inferred; `mock_site` names the pattern.
- Queries never treat an edge carrying `mock` (on the edge or any `site_attrs` entry) as production usage or as exercising its target, whatever its certainty: in `tests-for` only a mock edge on a test's own reached path yields `spies on it`/`mocks it`; a mock in its suite, file or called helper adds a note to a real reach, or alone yields the hedged `its scope mocks it` naming that scope.
- Dangling targets: the engine drops detector edges whose `NodeRef::Key` target entity does not exist after the run (e.g. `covers_file` to a guessed sibling file) and counts them. Detector edges may repeat (kind, from, to) with different spans; the engine merges them into one edge with one evidence per span.
- Detector-only edge kinds: `makes_request` (caller → `ClientCall`), `implements_auth` (guard/strategy/middleware → `AuthScheme`), `requires_guard` (endpoint → guard, usually `Unresolved`, attrs `level`/`order`). `atlas-link` consumes them to derive `calls_endpoint`, `supports_auth` and `uses_auth`.
- Endpoint certainty covers the route and mount chain only; handler resolution lives on the `exposes_endpoint` edge, which is dropped if its `Unresolved` handler cannot be resolved. Keys never include a Nest global prefix (emitted as `other:http_global_prefix:<repo>:<prefix>`); routes on a router not mounted in the same file carry `route_scope = "router_relative"` (Express adds `router`), and cross-file mounts are custom `mounts` edges to the mounted router (attrs `mount_prefix`, `mounted_on`, `mounted_on_scope`).
- Detector attrs for `atlas-link`/queries: `makes_request` edges carry per-call-site `base_source`, `auth_hint`, `auth_hint_source`; `new Sdk({ auth: { type } })` yields a `references` edge to the class with `uses_auth_option`; composite guards emit `references` edges with `delegates_to`; env reads in test code carry `in_test = true`. Fully dynamic client URLs produce no `ClientCall`, only an `http.dynamic_url` diagnostic with the call span.

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

`.cargo/config.toml` builds the bundled SQLite without `SQLITE_ENABLE_MEMORY_MANAGEMENT`, which removes its global page-cache mutex; builds from outside the workspace keep it (slower, still correct) and `doctor` warns.

## Engine contracts (`atlas-engine`)

### Freshness

- `Engine::ensure_fresh()` is the barrier every query runs first. It stat-scans every repo and runs an incremental update when anything changed. With a healthy watcher it skips the scan only if no event arrived since the last scan and that scan is younger than `full_scan_interval` (60 s). Events mark the watcher dirty inside the notify callback. A watcher error, overflow or rescan event forces the next barrier to scan, and a dead worker or channel makes every barrier scan. Residual risk: an edit whose event has not been delivered yet.
- The barrier returns `FreshnessReport::Fresh(header)` or `Degraded { failed_files, reason }`, and queries must surface `Degraded`. It is degraded in these cases:
  - the update failed (store error, lock timeout, a panic inside the update);
  - a file could not be read or analysed during this update;
  - a project pass failed or panicked (the run is `partial`);
  - a repo root is unavailable;
  - another engine adopted a different project configuration.

  Facts of failed or removed files are deleted, never kept stale. `header.files_failed_analysis` keeps counting files that failed in earlier runs.
- Scans pass the start time of the scan that produced the stored stamps as `previous_scan_ns` (persisted per repo in the `engine:meta` blob); an unknown time hashes everything.
- Concurrency:
  - Barriers in one process are single-flight: a caller joins only a flight launched after it arrived (ticket `launched + 1`), so it never receives a scan that started before its call. Queries run after the barrier on the store's read-connection pool, outside the engine lock; one query uses one read transaction. `Engine::skip_barrier` / `QueryEngine::without_barrier` exist for benchmarks only and must never be reachable from the CLI or MCP.
  - Across processes, `<db>.index.lock` is held for a whole run, and the store write lock only per persist batch. After taking the index lock, the engine always re-reads the persisted generation (the last finished run in `engine:meta`). If it moved, the engine reloads its state and re-plans from a fresh scan.
  - The `engine:dirty` blob exists while a run is in flight. A barrier that sees it waits for the index lock. A dirty marker found while holding the lock means a crashed run: the engine reloads (ignoring the persisted similarity index), marks the orphaned `barrier`/`index`/`edit` runs `failed`, recomputes SCIP lag, and the next run re-resolves every file. If dependency rounds do not converge, the run stays `partial` and keeps the dirty marker, so the next run re-resolves everything.
- `Engine::write_then_refresh(files, write)` holds the engine mutex and `<db>.index.lock` (barrier order) across `write` and an incremental refresh run (trigger `edit`), so no barrier in any process indexes a half-applied write. The refresh always scans and re-hashes every path in `files` whatever its stat says (mtime-preserving writers, clock skew), also when `write` errors or panics (the panic resumes after the locks are released). It returns `Err` without calling `write` only for an unknown repo, a lock timeout or a failed sync; `write` must not call back into the engine.
  - `<db>.edit.lock` (atlas-edit) is taken only inside the `write` closure, so the order is engine mutex → index lock → edit lock. Taking the edit lock first and then running a barrier deadlocks.
  - Another process's barrier waits on the index lock for the whole write plus refresh, and returns `Degraded` (lock timeout) if that outlasts `lock_timeout`.
- Project configuration: `engine:project` holds a fingerprint of the repos (name, path, include/exclude, languages), `max_file_bytes` and the store `SCHEMA_VERSION`, so an engine built for another schema degrades instead of writing. `Engine::open` and `Engine::reload_project` adopt their configuration. Any other engine whose fingerprint differs reports `Degraded("project configuration changed; reopen the engine")` and neither purges nor indexes. Long-lived servers must call `reload_project` when the registry changes. Always open an engine with the full registry `Project`: opening with a subset adopts it and purges every other repo. The transaction that begins a run records the configured repos in `engine:repos` before any file owner is written. On the next run, repos from `engine:repos`, Repository entities and meta that are absent from the adopted configuration are purged, even when the previous run crashed. Stale entries are cleared only after a run finishes.
- SCIP and LLM enrichment never run inside the barrier. Every file owner written by a run, whether its content changed or it was only re-resolved, loses its `scip:` owner. It is added to `engine:scip.lagging` and stamped in `dropped_in` in the same transaction, whether or not SCIP has run, and `header.scip` reports the lag once SCIP has run.
- The similarity index is updated in memory every run and persisted on `Engine` drop, after `similarity_persist_idle` in the watcher, or via `persist_similarity()`. Long-lived processes, the MCP server in particular, must call `persist_similarity()` on SIGTERM/SIGINT, because signals skip drops. The blob is stamped with its generation; a stale or missing blob is rebuilt from the artifacts at open.
- `Engine::scip` syncs the generation before planning. It ingests a file only when the stored artifact, the hash the indexer was given and the disk agree; any other file stays lagging. Before writing `scip:` owners under the index lock, it re-checks the hash in the files table. If the persisted generation moved since the jobs were built, it also re-reads the repo's graph fingerprint and the lag set: a changed fingerprint leaves the whole package lagging, and a file whose `dropped_in` stamp (the run that last dropped its `scip:` owner) differs from the job's snapshot is skipped. Deleted files are removed from `engine:scip.lagging` when the run removes them, and ingest drops every lagging entry without a file record.
- After dependency rounds, a run also drops every `scip:` owner with an edge into an entity the run removed (dependency tracking cannot see edges only SCIP produced) and marks its file lagging with `dropped_in`, like a re-resolved file.
- SCIP indexing units: a Cargo workspace member is indexed with its whole workspace (one rust-analyzer run at the `[workspace]` root, `package_dir` = workspace dir), so cross-crate references map to project definitions; standalone crates and npm packages run per package. With one workspace-wide index every member is re-ingested on each run; this is safe because each file is still gated on its own hash, its `dropped_in` stamp and an unchanged `(len, mtime)` between indexer start and ingest (catches A→B→A edits), and a file that changed mid-run contributes no definitions, so references into it are dropped rather than mis-mapped. The index is decoded and mapped outside the engine mutex; only the final write holds it with the index lock.
- Automatic SCIP: when `EngineOptions::auto_scip` has indexers and a watcher runs, the watcher schedules `Engine::scip` (ledger trigger `scip-auto`) on its own thread once `scip.enabled` is set (opt-in, default off: `code-atlas project config <p> --scip on`; explicit `scip` ignores the setting), no file event arrived for `scip.debounce_secs`, no refresh is pending, and lagging files exist that a ready indexer covers. It never blocks barriers and never marks them `Degraded`. One run at a time per project: `<db>.scip.lock` is held by every SCIP run (explicit runs wait for it, automatic runs skip when it is busy) and is what `status` probes for "in progress". After an attempt it waits for the persisted generation to move before trying again. A package whose indexer failed or timed out is skipped while its inputs (file hashes plus graph fingerprint) are unchanged, for at most `AutoScipOptions::failure_retry` (1 h). The hold fingerprint covers parsed manifests and tsconfigs through the graph fingerprint but not lockfiles, `rust-toolchain*` or `.cargo/config*`, which the cache key does include: fixing a failure by changing only those waits for the hold to expire or a source edit (or a restart). When the backlog is empty or entirely held, the generation is recorded as attempted so the scheduler does not rebuild jobs every tick. A run cancelled at any point is recorded `partial`. An `Unavailable` indexer warns once per process and pauses for `AutoScipOptions::unavailable_retry` (1 h) or until restart. Dropping the `WatchHandle` cancels the run (the indexer process group is killed) and joins it. The last automatic run and backoffs are persisted in `engine:scip.auto`; `status` hides backoffs past their `retry_at_ms`.

### Incremental ≡ full

- A file owner's facts are a pure function of its persisted artifact (`artifact:<repo>:<path>` blob: `FileFacts`, detector outputs, package context, generated flag), the symbol table, and the set of entities asserted by file owners. Never put run ids, timings or absolute paths into file- or pass-owned facts.
- The re-resolve set is the changed files, plus resolver relocations and `affected_files`, plus files whose detector `Key` targets appeared or disappeared, plus the safety nets `owners_referencing` (collected ids) and `unresolved_by_names` (created symbols). Any package-graph change re-resolves every file of the project.
- A detector `Key` target exists only if a file owner (including manifest owners) asserts it. Entities owned by project passes or SCIP never keep a detector edge alive.
- `renamed_from` edges (owner `Project { pass: "lineage" }`) are history and cannot be derived from the tree; digest comparisons import that owner first.
  - Identical bodies need at least 12 tokens. Across files that are not renamed, they need 30.
  - Bodies under 30 tokens whose names share no meaningful token are capped at confidence 0.6.
  - Inferred facts migrate only for lineage with confidence ≥ 0.85.
- `tests/incremental_equivalence.rs` and `tests/regressions.rs` (seeded random edit sequences and batches against a fresh full index) must keep passing.
- Any change to a producer's output must bump that producer's version (and its pin); the engine re-analyses or re-resolves on mismatch. Producers and their pins (`tests/version_pin.rs`):
  - the analyzers (`ANALYZER_VERSION`, re-analyses that language);
  - each detector (its `detector!` line; the engine compares the whole detector list, so one bump re-analyses every file);
  - the resolver (`RESOLVER_VERSION`, re-resolves every file);
  - project passes (`ProjectPass::version`: `STRUCTURE_VERSION`, `atlas_link::LINK_VERSION`; `engine:meta` stores `pass_versions`, and a mismatch triggers a run that re-runs the passes only). Their pin is in `atlas-engine`.

  A pin is `(version, inputs digest, output digest)` over `fixtures/` plus the crate's `tests/pin-corpus/` (resolver, detectors). Inputs cover only what the producer reads (language files, manifests, upstream facts), so eval ground truth and READMEs never trip a pin; a changed inputs digest only needs a re-pin. `.gitattributes` keeps fixtures and corpora byte-exact.

### `ProjectPass` (for `atlas-link` and later passes)

- `trait ProjectPass { fn name(&self) -> &'static str; fn version(&self) -> u32 { 1 } fn run(&self, tx: &ReadTx<'_>, context: &PassContext<'_>) -> Result<PassOutput, PassError>; }`, with `PassOutput { entities, edges }`. `structure` and `link` are built in; extra passes go through `EngineOptions::passes`; every engine that opens a database must register the same passes.
- Passes run on every index run, after all file and manifest owners are committed: `structure` and `link` first, then the registered passes in order. Each output replaces the owner `Project { pass: name }` wholesale.
- A pass must be a deterministic function of file-owned facts, `PassContext` (project, package graphs, manifest hashes) and earlier passes. It must not read its own previous output or the `lineage` and `scip:*` owners.
- An error or panic in a pass removes its facts and makes the run partial. The names `repos`, `structure`, `link`, `lineage` and the prefix `scip:` are reserved, and `Engine::open` rejects them and duplicate names.
