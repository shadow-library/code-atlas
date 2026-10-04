# Code Atlas

Code Atlas indexes a group of Rust and TypeScript/JavaScript repositories (a *project*) into one SQLite knowledge graph and serves it to coding agents over a CLI (`code-atlas`) and an MCP stdio server. The graph holds symbols, calls, references, imports, HTTP endpoints and their cross-repo clients, auth schemes, env/config reads, tests, and a few experimental domains (events, jobs, database tables). Queries answer questions such as "what breaks if I change this", "which tests cover it", "does something like this already exist" and "which repos call this endpoint".

Every fact carries exactly one certainty class and evidence (file, span, content hash, producer). Queries run a freshness barrier first, so an answer reflects the working tree at the time of the call; when the index cannot be brought up to date the answer says `DEGRADED`. Facts that are not exact derivations are tagged and hedged (`possibly calls`, `[heuristic 0.72]`), and an empty result comes with the caveats that could hide relations. LLM use is optional, off by default, and only ever produces facts tagged `inferred`.

Residual freshness risk: with an active watcher the barrier may skip the scan, and a filesystem event that has not been delivered yet can leave an answer stale for up to the 60 s full-scan interval. Without a watcher every call scans.

## Building

```sh
cargo build --release                      # binaries in target/release: code-atlas, atlas-bench, atlas-eval
cargo install --path crates/atlas-cli      # installs code-atlas
```

The repository's `.cargo/config.toml` sets `LIBSQLITE3_FLAGS="-USQLITE_ENABLE_MEMORY_MANAGEMENT -DSQLITE_DEFAULT_MEMSTATUS=0"`. Without it the bundled SQLite uses one process-wide page cache behind a global mutex and concurrent queries serialise. The flag applies to workspace builds and `cargo install --path`, but not to `cargo install --git`; for that, pass it explicitly:

```sh
LIBSQLITE3_FLAGS="-USQLITE_ENABLE_MEMORY_MANAGEMENT -DSQLITE_DEFAULT_MEMSTATUS=0" cargo install --git <url> code-atlas
```

Losing the flag costs throughput, never correctness. `code-atlas doctor` reports the state under the check "sqlite build" (a warning when the build kept memory management). The MCP server uses max(configured readers, cores) read connections (the default is 4 readers).

## Quickstart

Requires a Rust toolchain (edition 2024 in `Cargo.toml`; `rust-toolchain.toml` pins the stable channel). `rust-analyzer`, `npx` and the `claude` CLI are optional (SCIP and `connect claude`).

```sh
cargo build --release
export PATH="$PWD/target/release:$PATH"

code-atlas project create acme \
  --repo fixtures/acme/acme-api --repo fixtures/acme/acme-sdk \
  --repo fixtures/acme/acme-web --repo fixtures/acme/acme-billing
code-atlas -p acme index

code-atlas -p acme query overview
code-atlas -p acme search "user lookup"
code-atlas -p acme impact UsersService.findOne
code-atlas -p acme api
code-atlas -p acme query unused

cd fixtures/acme/acme-api
code-atlas connect claude
```

Inside a registered repo the `-p acme` flag is inferred from the working directory. Set `CODE_ATLAS_HOME=/tmp/atlas-scratch` to keep experiments out of your real registry and databases. `connect claude` writes `.mcp.json` and a guidance block into `AGENTS.md` (or an existing `CLAUDE.md`); `--scope user` registers the server with `claude mcp add-json` instead. Nothing else is needed to use the index: there is no separate reindex step, because every query refreshes it.

Optional accuracy upgrade: `code-atlas -p acme scip` runs rust-analyzer and scip-typescript and promotes matching edges to `compiler_verified`.

## Trust model

A fact may be stated as known only if its certainty is authoritative: `compiler_verified`, `syntactic`, `resolved` or `declared`. Authoritative means "the exact derivation the class names", not "complete": a `resolved` caller list can still miss callers that static analysis cannot see (dynamic dispatch, macros, generated code, reflection, other repos outside the project). `heuristic` facts carry a confidence and a rule name and are leads to verify; `inferred` facts come from an LLM, carry provider, model and rationale, and go stale as soon as their evidence file changes. Absence is never certain: "no callers" and "unused" are reported with the evidence and caveats behind them, and dynamic references that could reach a target are listed instead of ignored. Uncertainty is never silently upgraded; a derived fact is never stronger than the weakest input behind it. Details: [docs/trust-and-limitations.md](docs/trust-and-limitations.md).

## Documentation

| Document | Contents |
|---|---|
| [docs/architecture.md](docs/architecture.md) | crates, pipeline, storage, freshness contract, concurrency, failure handling, project isolation |
| [docs/knowledge-model.md](docs/knowledge-model.md) | entity and edge kinds, keys, certainty classes, evidence, inferred facts, lineage, per-attribute provenance |
| [docs/trust-and-limitations.md](docs/trust-and-limitations.md) | what is deterministic, heuristic or inferred; where conclusions are not authoritative; known limitations; failure modes |
| [docs/agent-integration.md](docs/agent-integration.md) | MCP tools, recommended workflow, reading certainty tags, `connect claude`, other agents |
| [docs/languages-and-frameworks.md](docs/languages-and-frameworks.md) | Rust and TS/JS depth, supported frameworks, experimental detectors |
| [docs/extending.md](docs/extending.md) | adding an analyzer, detector, project pass or LLM provider |
| [docs/operations.md](docs/operations.md) | CLI reference, exit codes, home directories, project settings, watch, SCIP, enrichment, doctor |
| [docs/evaluation/methodology.md](docs/evaluation/methodology.md) | quality eval, performance bench, agent A/B harness |
| [docs/evaluation/dogfooding.md](docs/evaluation/dogfooding.md) | dated findings from using Code Atlas on itself |
| `docs/evaluation/results.md` | measured results (to be added) |
| [fixtures/README.md](fixtures/README.md), [fixtures/expected.schema.md](fixtures/expected.schema.md) | fixture projects and ground-truth format |
| [eval/README.md](eval/README.md), [eval/tasks/README.md](eval/tasks/README.md) | agent A/B harness reference and task catalogue |
| [CLAUDE.md](CLAUDE.md) | contributor rules and engine contracts |

`docs/plan/` is the original implementation plan and is historical.

## Status

State as of this documentation pass. "Implemented" means covered by tests and the fixture evaluation; it does not mean complete or error-free (see the limitations document).

| Area | Status | Notes |
|---|---|---|
| Rust and TS/JS/TSX/JSX analysis (tree-sitter) | Implemented | symbols, imports/exports, references, impls, test markers, body fingerprints |
| Name, module and import resolution | Implemented | `resolved` for unique results under emulated language rules; `heuristic` candidates for dispatch, macros, name-only matches |
| Incremental indexing, freshness barrier, watcher | Implemented | incremental result equals a full index (tested with random edit sequences) |
| HTTP endpoints: NestJS, Express/Fastify/Koa router, axum/actix-web/Rocket | Implemented | fixtures cover NestJS, Express, axum; Fastify, Koa, actix-web, Rocket only have unit tests |
| HTTP clients: fetch, axios, reqwest, Nest `HttpService` | Implemented | base-URL and route matching is heuristic, never above `heuristic` |
| Cross-repo linking: packages, API to client, auth, env/config | Implemented | link-derived facts are `heuristic` unless backed by manifests |
| Auth schemes, permissions, env/config reads, tests (jest, vitest, `#[test]`) | Implemented | guard to scheme mapping is name- and library-based |
| CLI and MCP server (16 tools) | Implemented | shared query layer; `--json` and MCP `format: "json"` |
| SCIP ingestion (`compiler_verified`) | Implemented, opt-in | manual `code-atlas scip`; never runs inside a query |
| Near-duplicate and lexical search | Implemented | similarity is evidence, never equivalence |
| Events, jobs, DB tables/queries | Experimental | entities and edges carry an `experimental` attribute (not shown in text output); heuristic only; project-scoped assumptions |
| LLM enrichment (`enrich`, inferred similarity verdicts) | Experimental, off by default | only similarity-pair classification is wired into `enrich`; no verdicts exist in the published fixtures |
| Rename lineage (`renamed_from`) | Implemented; confidence calibration experimental | heuristic confidence; inferred facts migrate only at 0.85 or higher |
| Quality eval, perf bench, agent A/B harness | Implemented | results to be published in `docs/evaluation/results.md` |
| Other languages (Python, Go, Java, ...) | Unsupported | |
| Automatic SCIP or LLM runs on file change | Not implemented | `scip.enabled` and `scip.debounce_secs` settings exist but nothing reads them |

## Development

```sh
./scripts/check.sh     # fmt --check, clippy -D warnings, test --workspace
```

Rules for contributors (certainty discipline, append-only `atlas-core` contracts, no comments, typed errors) are in [CLAUDE.md](CLAUDE.md).
