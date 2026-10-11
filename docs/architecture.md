# Architecture

Code Atlas is a Cargo workspace of 20 product crates plus the unpublished test crate `atlas-test-lsp`. One *project* (a named set of repos) owns one SQLite database. The engine keeps that database equal to the working tree; the query layer reads it; the CLI and MCP server are thin front ends over the query layer.

## Crates

| Crate | Responsibility |
|---|---|
| `atlas-core` | ids, entity and edge keys, kinds, `Certainty`/`Evidence`/`Freshness`, the `FileFacts` IR, the `LanguageAnalyzer`, `Detector` and `LlmProvider` traits, errors. Append-only contracts |
| `atlas-store` | SQLite schema and migrations, per-owner fact storage, merged views, traversal primitives, run ledger, inferred facts, LLM cache and call ledger, derived blobs |
| `atlas-project` | project registry (`projects.toml`), cwd to project inference, `.code-atlas.toml` markers, file discovery, content hashing and stat cache, `Cargo.toml`/`package.json`/`tsconfig` manifests and package graphs |
| `atlas-lang-rust`, `atlas-lang-ts` | tree-sitter analyzers producing `FileFacts`. Depend on `atlas-core` only, never panic on any input |
| `atlas-resolve` | project-wide symbol table; Rust module tree and `use` resolution; TS import, export and re-export resolution; call and reference targets with certainty |
| `atlas-detect` | framework detectors (endpoints, clients, auth, env/config, tests, events, jobs, db) over `FileFacts` plus source text |
| `atlas-link` | cross-repo pass: package dependencies, route composition, API to client matching, auth support and use, events, config |
| `atlas-similarity` | MinHash/LSH structural near-duplicates and field-weighted BM25 lexical search |
| `atlas-scip` | runs rust-analyzer and scip-typescript, ingests SCIP as `compiler_verified` edges, reconciles with our edges |
| `atlas-semantic` | `LlmProvider` backends (claude CLI, Anthropic HTTP, OpenAI-compatible), prompts, budget, call cache, inferred facts, revalidation |
| `atlas-engine` | full and incremental pipeline, freshness barrier, lineage, locks, watcher, SCIP and enrich orchestration, `ProjectPass` |
| `atlas-query` | the query surface shared by CLI and MCP: context packs, impact, callers, API, unused, similar, patterns, explain, status, and the graph-side symbol modes (find, hierarchy, source, graph outline and references) |
| `atlas-lsp` | blocking stdio JSON-RPC client for language servers, hand-written LSP types, server processes and pool, document sync, TypeScript server discovery. Depends on `atlas-core` only |
| `atlas-edit` | edit plans, path-guarded atomic application with rollback, text operations, symbol regions. No LSP, no engine; depends on `atlas-core` and `atlas-project` |
| `atlas-ide` | `IdeService`: exact operations over language servers (references, declaration, implementations, outline, diagnostics, rename, safe delete) plus graph extras, symbol and text edits, plan files. Shared by CLI and MCP |
| `atlas-mcp` | MCP stdio server (15 tools) |
| `atlas-cli` | the `catl` binary |
| `atlas-test-lsp` | unpublished (`publish = false`) test crate that owns the only fake language-server binary; tests locate it with `atlas_test_lsp::fake_lsp_path()`; it is a dev-dependency only, so no product build includes it |
| `atlas-bench`, `atlas-eval` | performance harness and evaluation harness (quality and agent A/B) |

Dependency direction: `core` at the bottom; analyzers and `atlas-lsp` depend on `core` only; `engine` depends on everything below it; `query` on `engine`; `atlas-ide` on `query`, `engine`, `atlas-lsp`, `atlas-edit` and `atlas-scip` (to locate rust-analyzer); `mcp` and `cli` on `query` and `atlas-ide`. `atlas-link` cannot implement `ProjectPass` itself because `atlas-engine` depends on it; the engine registers it as the pass `link`.

## Data flow

```
 working tree (repos of one project)
        |
        |  discover (ignore rules, default excludes, size limit, symlinks)
        v
 +---------------------- ensure_fresh() barrier (every query, every index run) ----------------------+
 | scan   stat every file, hash only changed or racily-clean files (stat cache)                      |
 | plan   added / modified / deleted / moved files, package-graph changes                            |
 | analyze (parallel)  LanguageAnalyzer -> FileFacts ; Detectors -> Detection ; generated-file flag  |
 | symbols  ProjectSymbolTable upsert/refresh (Rust module tree, TS exports) -> relocations, deltas  |
 | resolve  atlas-resolve: references -> edges with certainty, UnresolvedRefs                        |
 | existence  detector edges to non-existent targets are dropped and counted                         |
 | persist  one owner per file: entities + edges + unresolved + artifact blob (batched write txs)    |
 | dependents  re-resolve files whose looked-up names changed, until converged                       |
 | similarity  in-memory MinHash and BM25 index updated                                              |
 | lineage  renamed_from edges from disappeared/appeared symbols                                     |
 | passes  structure -> link -> registered passes (each replaces its Project owner wholesale)        |
 | finish  run ledger, freshness header                                                              |
 +---------------------------------------------------------------------------------------------------+
        |                                            ^
        v                                            | out of band, never inside the barrier
 SQLite (WAL) ---- atlas-query ---- CLI / MCP     catl scip  -> scip:<repo>:<path> owners
        |                                          catl enrich -> inferred facts (LLM)
        +-- file owners, project-pass owners, SCIP owners, inferred facts, blobs, run ledger
```

Language servers sit beside this flow, not in it. `atlas-ide` runs the barrier first, syncs a server (one per language by default, started by the first exact call, kept warm under MCP and one-shot under the CLI) to the working tree from the run ledger, then asks it. Exact answers are rendered `[compiler]` and are never written to the store. Edits go through `Engine::write_then_refresh`, which holds the engine mutex and the index lock across the atomic write and an incremental refresh, so no barrier in any process indexes a half-applied edit. Lock order is IDE edit mutex, barrier, language-server unit mutex, then engine mutex, index lock and `<db>.edit.lock`. See `crates/atlas-ide/CLAUDE.md` and the [operations](operations.md#language-servers) page.

Pipeline phases as reported by `catl debug run <id>`: `plan, analyze, symbols, load, resolve, manifests, existence, lineage_inputs, encode, persist, dependents, similarity, lineage, passes, finish`.

## Storage

One database per project at `<data dir>/<project>/atlas.db` (see [operations](operations.md) for the data dir). SQLite in WAL mode, `synchronous = NORMAL`, foreign keys on; readers are `query_only`. Schema version is stored in `user_version` (currently 2); migrations run at open, under the index lock, so an older binary's in-flight run finishes first. Assertion rows use a compact encoding (values derivable from the owner or the key are elided and rehydrated on read; legacy v1 rows stay readable). A migrated database keeps its file size until a re-index or `VACUUM`, and the migration overwrites the project fingerprint so an older engine still running degrades instead of writing. Per-file artifacts are zlib-compressed. Each read connection has a 16 MiB page cache (capped at 512 MiB across readers); mmap is off. Bulk loads drop secondary indexes while the store is empty and rebuild them before follow-up rounds.

| Tables | Purpose |
|---|---|
| `owners` | one row per fact owner: `file {repo, path}` or `project {pass}` (including `scip:<repo>:<path>`, `lineage`, `structure`, `link`, `repos`) |
| `entities`, `entity_owners` | entity identity and a per-owner assertion (certainty, attrs, location) |
| `edges`, `edge_owners` | edge identity (kind, from, to, discriminator) and per-owner assertions with evidence |
| `unresolved_refs` | references that did not resolve, with reason; stored for display and debugging only |
| `files` | per-file record: content hash, size, mtime, inode, parse status, analyzer, indexed run |
| `index_runs`, `run_fact_deltas` | run ledger: trigger, outcome, phase timings, counts, change set, per-owner fact deltas |
| `inferred_facts`, `inferred_subjects`, `inferred_evidence` | LLM-derived facts with evidence and freshness |
| `llm_cache`, `llm_calls` | prompt cache and call ledger (tokens, cost estimate, model) |
| `derived_blobs` | engine state: per-file artifacts (`artifact:<repo>:<path>`), `engine:meta`, `engine:dirty`, `engine:project`, `engine:repos`, `engine:scip.lagging`, similarity index |

Key properties:

- **Ownership.** Every fact is written by exactly one owner, and an owner is replaced wholesale. Re-analyzing a file replaces its owner; deleting the file removes it. No fact outlives the file that produced it.
- **Multi-owner entities.** The same entity key can be asserted by several owners (a file entity by the file and by a pass; a Rust `#[path]` alias by two files). The merged view takes certainty, kind-specific fields and evidence from the winning assertion (strongest certainty, ties to the smallest owner) and unions attrs. `entity_with_sources` returns per-attribute provenance; see the [knowledge model](knowledge-model.md).
- **Dangling edges.** Edge targets may be absent (a dependency not yet indexed); the engine re-resolves through `owners_referencing` and drops dangling detector edges.
- **Artifacts.** Each file's `FileFacts`, detector outputs, package context and generated flag are persisted as a blob, so re-resolution after another file changes does not re-parse.

## Freshness contract

`Engine::ensure_fresh()` runs before every query. It stat-scans each repo, hashes files whose stamp changed (and files modified close to the previous scan, to catch racily-clean files), and runs an incremental update if anything differs. The result is `Fresh(header)` or `Degraded { failed_files, reason }`; queries must surface `Degraded` as a `DEGRADED:` first line (CLI exit code 3). With a healthy watcher the barrier skips the scan only if no event arrived since the last scan and that scan is younger than 60 seconds; a watcher error, overflow, rescan event or dead worker forces a scan on every barrier. Residual risk: an edit whose filesystem event has not been delivered yet.

Degraded cases: the update failed (store error, lock timeout, panic); a file could not be read or analyzed; a project pass failed (the run is `partial`); a repo root is unavailable; another engine adopted a different project configuration. Facts of failed or removed files are deleted, never kept stale.

SCIP and LLM enrichment never run inside the barrier (automatic SCIP runs on the watcher's own thread, and never makes the barrier degraded). Any file re-written by a run loses its `scip:` owner and is recorded as lagging, so SCIP edges never describe an older version of a file. Inferred facts become `stale` when any evidence file's hash changes.

**Incremental equals full.** A file owner's facts are a pure function of its persisted artifact, the symbol table and the set of entities asserted by file owners. Run ids, timings and absolute paths are never part of file- or pass-owned facts. `tests/incremental_equivalence.rs` and `tests/regressions.rs` replay seeded random edit sequences and compare the fact-set digest against a fresh full index. The one exception is `renamed_from` lineage, which is history and cannot be derived from the tree.

## Concurrency

| Scope | Mechanism |
|---|---|
| Within a process | concurrent barriers coalesce into single-flight scans (a barrier joins a flight launched after its arrival, so it never sees a scan older than its call); queries run outside the engine lock on a pool of read-only connections (the MCP server uses max(configured readers, cores); default 4) |
| Across processes | `<db>.index.lock` is held for a whole index run; the store write lock only per persist batch. After taking the lock the engine re-reads the persisted generation and reloads and re-plans if another process moved it |
| In-flight marker | `engine:dirty` exists while a run is in flight; a barrier that sees it first tries the index lock without waiting: if the lock is free the previous run crashed and the barrier recovers; if it is busy, a run is live and the barrier waits for the lock (up to the lock timeout) |
| Store | one writer at a time; re-entering the store from a thread already holding a transaction fails with `StoreError::Reentrant` |
| Watcher | notify events mark the watcher dirty and are debounced (200 ms); similarity is persisted after 30 s idle |
| Edits and language servers | one edit per process at a time (dry runs included); a per-server mutex serialises requests; `<db>.edit.lock` is taken only inside the write; at most `lsp.max_servers` (2) servers per process |
| Registry | `projects.toml` is read-modify-written under `projects.toml.lock` |

Several processes (CLI, MCP server, `watch`) can use one project at once. The MCP server serves one project; it reloads the registry when the project configuration changes and persists the similarity index on SIGTERM and SIGINT.

## Failure handling

| Failure | Behaviour |
|---|---|
| File unreadable, not UTF-8, or analyzer error | file recorded as failed; its facts are removed; run reports `Degraded` with the file and reason |
| Syntax errors | tree-sitter partial parse: facts around the error are extracted, file marked `partial` with up to 8 reported errors; surfaced as a caveat on queries that touch it |
| File above `max_file_bytes` (default 1 MiB), symlink, non-UTF-8 path | skipped, listed in `status` and as `skipped N files` in the query header; not an error |
| Analyzer or pass panic | caught; the file or pass fails, the run is `partial` |
| Analysis exceeds its parse budget | file recorded as failed (`parse budget exceeded`), its facts removed, run `Degraded`; retried in the same run and with backoff in later runs (see [operations](operations.md#parse-budget)) |
| Language server missing, disabled, crashed or not ready | exact operations fail with a typed error and an install hint, or answer `PARTIAL`; the index is unaffected; three failures within 10 minutes pause the server for 10 minutes |
| Edit write fails | rolled back (exit 6), or reported as partially applied with the files left changed (exit 7); the index is refreshed in both cases. There is no journal for a crash mid-write; the next apply sweeps leftover temp files |
| Crash mid-run | `engine:dirty` found on next lock: orphaned runs marked `failed`, state reloaded, every file re-resolved |
| Dependency rounds do not converge | run stays `partial` and keeps the dirty marker; next run re-resolves everything |
| Registry changed under a long-lived engine | `Degraded("project configuration changed; reopen the engine")`; `watch` and the MCP server reload |
| Repo root missing | `Degraded`; other repos still index |
| SCIP indexer missing, timeout (10 min), network failure | reported per indexing unit as unavailable or failed; the index is unaffected. Automatic runs back off for an hour after an unavailable indexer and hold a failed package until its inputs change |
| LLM provider unavailable or over budget | tasks skipped; the enrich run is `partial`; CLI exit code 3 |

Failure modes and how they surface to users are catalogued in [trust and limitations](trust-and-limitations.md#failure-modes-and-how-they-surface).

## Project isolation

- One database file per project. A query can only reach the project it was opened for; the MCP server is started with one `--project`.
- Entity keys embed the repo (`ts:acme-api:...`, `endpoint:acme-api:GET /users/{id}`), so equal names in different repos never merge.
- Repo-scoped: env vars, config keys, endpoints, auth schemes, permissions, jobs. Project-scoped: events, queues, DB tables, which assumes one logical database and one event bus per project.
- Cross-repo linking happens only within a project. The `globex` fixture project repeats the names, routes and env vars of `acme` on purpose; the evaluation fails if a query scoped to one project returns an entity of the other.
- Opening an engine with a subset of the registry adopts that subset and purges the other repos' facts, so callers must always open with the full registered project.
