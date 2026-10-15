# Code Atlas

Code Atlas indexes a group of Rust and TypeScript/JavaScript repositories (a *project*) into one SQLite knowledge graph and serves it to coding agents over a CLI (`catl`) and an MCP stdio server. Exact IDE operations (references, declaration, implementations, diagnostics, renames, symbol and text edits) run through rust-analyzer and typescript-language-server, with the graph adding cross-repo, HTTP, mock and reuse context. The graph holds symbols, calls, references, imports, HTTP endpoints and their cross-repo clients, auth schemes, env/config reads, tests, and a few experimental domains (events, jobs, database tables). Queries answer questions such as "what breaks if I change this", "which tests cover it", "does something like this already exist" and "which repos call this endpoint".

Every fact carries exactly one certainty class and evidence (file, span, content hash, producer). Queries run a freshness barrier first, so an answer reflects the working tree at the time of the call; when the index cannot be brought up to date the answer says `DEGRADED`. Facts that are not exact derivations are tagged and hedged (`possibly calls`, `[heuristic 0.72]`), and an empty result comes with the caveats that could hide relations. LLM use is optional, off by default, and only ever produces facts tagged `inferred`.

Residual freshness risk: with an active watcher the barrier may skip the scan, and a filesystem event that has not been delivered yet can leave an answer stale for up to the 60 s full-scan interval. Without a watcher every call scans.

## Building

```sh
cargo build --release                      # binaries in target/release: catl, atlas-bench, atlas-eval
cargo install --path crates/atlas-cli      # installs catl
```

The repository's `.cargo/config.toml` sets `LIBSQLITE3_FLAGS="-USQLITE_ENABLE_MEMORY_MANAGEMENT -DSQLITE_DEFAULT_MEMSTATUS=0"`. Without it the bundled SQLite uses one process-wide page cache behind a global mutex and concurrent queries serialise. The flag applies to workspace builds and `cargo install --path`, but not to `cargo install --git`; for that, pass it explicitly:

```sh
LIBSQLITE3_FLAGS="-USQLITE_ENABLE_MEMORY_MANAGEMENT -DSQLITE_DEFAULT_MEMSTATUS=0" cargo install --git <url> atlas-cli
```

Losing the flag costs throughput, never correctness. `catl doctor` reports the state under the check "sqlite build" (a warning when the build kept memory management). The MCP server uses max(configured readers, cores) read connections (the default is 4 readers).

tree-sitter is vendored with one patch that bounds its memory on unterminated generic-angle runs (`vendor/README.md`). Workspace builds, `cargo install --path` and `cargo install --git` all apply it through the workspace's `[patch.crates-io]`; only a crate in another workspace that depends on `atlas-lang-ts` or `atlas-lang-rust` gets upstream tree-sitter.

## Quickstart

Requires a Rust toolchain (edition 2024 in `Cargo.toml`; `rust-toolchain.toml` pins the stable channel). `rust-analyzer`, `npx` and the `claude` CLI are optional (SCIP, `agents sync`). Exact IDE operations need rust-analyzer (`rustup component add rust-analyzer`) for Rust and typescript-language-server with TypeScript 5 for TypeScript; language servers and the TypeScript download through `npx` are **on by default** (`project config <p> --lsp off`, `--lsp-download off`).

```sh
cargo build --release
export PATH="$PWD/target/release:$PATH"

catl project create acme \
  --repo fixtures/acme/acme-api --repo fixtures/acme/acme-sdk \
  --repo fixtures/acme/acme-web --repo fixtures/acme/acme-billing
catl -p acme index

catl -p acme query overview
catl -p acme search "user lookup"
catl -p acme impact UsersService.findOne
catl -p acme api
catl -p acme query unused

catl agents sync          # Claude Code (user scope) and Codex, as config.toml [agents] asks
```

Inside a registered repo the `-p acme` flag is inferred from the working directory. Set `CODE_ATLAS_HOME=/tmp/atlas-scratch` to keep experiments out of your real registry and databases. `agents sync` registers one `catl mcp --watch` server with Claude Code (`claude mcp add-json --scope user`) and Codex (`~/.codex/config.toml`) that binds the project of whatever directory the agent starts in, allows the read-only tools without a prompt (`permissions.allow`), and records what it wrote so `agents status` shows drift and `agents unsync` undoes it ([agent integration](docs/agent-integration.md#wiring-agents-agents-sync)). Nothing else is needed to use the index: there is no separate reindex step, because every query refreshes it.

Exact reads and edits (the first call per language starts a server: 2 to 5 s for TypeScript, 20 to 65 s cold for rust-analyzer; the MCP server keeps it warm):

```sh
catl -p acme refs UsersService.findOne
catl -p acme edit rename UsersService.findOne findUser --dry-run --plan-out plan.json
catl -p acme edit apply plan.json
```

Optional accuracy upgrade: `catl -p acme scip` runs rust-analyzer and scip-typescript and promotes matching edges to `compiler_verified`. Cargo workspaces are indexed at the workspace root, so cross-crate references are verified too. With `project config acme --scip on`, SCIP also runs automatically while `watch` or `mcp --watch` is active.

## Trust model

A fact may be stated as known only if its certainty is authoritative: `compiler_verified`, `syntactic`, `resolved` or `declared`. Authoritative means "the exact derivation the class names", not "complete": a `resolved` caller list can still miss callers that static analysis cannot see (dynamic dispatch, macros, generated code, reflection, other repos outside the project). `heuristic` facts carry a confidence and a rule name and are leads to verify; `inferred` facts come from an LLM, carry provider, model and rationale, and go stale as soon as their evidence file changes. Absence is never certain: "no callers" and "unused" are reported with the evidence and caveats behind them, and dynamic references that could reach a target are listed instead of ignored. Uncertainty is never silently upgraded; a derived fact is never stronger than the weakest input behind it. Details: [docs/trust-and-limitations.md](docs/trust-and-limitations.md).

## Evaluation results

Measured 2026-10-04 to 2026-10-06; full results, sources and caveats are in [docs/evaluation/results.md](docs/evaluation/results.md).

- **Knowledge quality.** On the hand-written fixtures, 98/98 expected edges and 184/184 entities were found, with 0 of 63 forbidden edges. Against the compiler (SCIP), `resolved` edges are 97.6% precise over confirmable edges (95.9% all-in), but they reproduce only 53.3% of the compiler's edges.
- **Performance.** This repository (116k LOC) indexes in 1.1 s, and the per-query freshness barrier costs 6 ms (p50). At 1M synthetic LOC, indexing takes 5.6 s with 2.21 GiB peak RSS, and mixed read and write throughput is weak: 8 to 26 qps, with p95 up to 974 ms.
- **Agent A/B (primary).** All 44 runs passed their hidden tests in both conditions (ceiling). The agent made 2 Code Atlas calls in 22 sessions, yet used 18.7% more tokens, because the tool definitions add about 6,300 tokens to every request.
- **Agent A/B (exploratory, directed preamble).** 21 of 22 sessions called `atlas_context`, and they read 23% fewer bytes. Tokens still rose by 24.8% and correctness did not change.
- **IDE operations (2026-10-05/06).** Fixture rename eval: 41/41 edits, 0 `untouched` lines edited (a regression check on the fixtures used to build it, not held-out). On hono 4.13.13, 7 of 10 first-round TypeScript renames broke test compilation until the language server was made to load the right projects; afterwards 19 symbols (plus 2 variants) produced 0 added `tsc` errors (manual reviewer trials, tabulated in `eval/reports/2026-10-06/review-trials.md`). `similar` from draft code found 0 of 16 realistic re-implementations structurally (it finds copies, not re-implementations), and lexical reuse suggestions from task prompts were tried and dropped (about 5% precision). No agent evaluation of `atlas_edit` exists.
- **Conclusion.** These tasks give no evidence that Code Atlas improves an agent's correctness or cost.

## Documentation

| Document | Contents |
|---|---|
| [docs/architecture.md](docs/architecture.md) | crates, pipeline, storage, freshness contract, concurrency, failure handling, project isolation |
| [docs/knowledge-model.md](docs/knowledge-model.md) | entity and edge kinds, keys, certainty classes, evidence, inferred facts, lineage, per-attribute provenance |
| [docs/trust-and-limitations.md](docs/trust-and-limitations.md) | what is deterministic, heuristic or inferred; where conclusions are not authoritative; known limitations; failure modes |
| [docs/agent-integration.md](docs/agent-integration.md) | MCP tools (`atlas_symbol` modes, `atlas_edit`), recommended workflow, reading certainty tags, `agents sync` and `connect claude`, hooks, other agents |
| [docs/languages-and-frameworks.md](docs/languages-and-frameworks.md) | Rust and TS/JS depth, supported frameworks, experimental detectors |
| [docs/extending.md](docs/extending.md) | adding an analyzer, detector, project pass or LLM provider |
| [docs/operations.md](docs/operations.md) | CLI reference, exit codes, home directories, configuration layers (`config.toml` defaults, repo `.code-atlas.toml`, `CODE_ATLAS_*`, `catl config show --origin`), project settings, language servers, parse budget, watch, SCIP, enrichment, doctor |
| [docs/evaluation/methodology.md](docs/evaluation/methodology.md) | quality eval, performance bench, agent A/B harness |
| [docs/evaluation/dogfooding.md](docs/evaluation/dogfooding.md) | dated findings from using Code Atlas on itself |
| [docs/evaluation/results.md](docs/evaluation/results.md) | measured knowledge quality, performance and agent A/B results |
| [fixtures/README.md](fixtures/README.md), [fixtures/expected.schema.md](fixtures/expected.schema.md) | fixture projects and ground-truth format |
| [eval/README.md](eval/README.md), [eval/tasks/README.md](eval/tasks/README.md) | agent A/B harness reference and task catalogue |
| [CLAUDE.md](CLAUDE.md) | contributor rules and engine contracts |

`docs/plan/` holds the original implementation plan (historical) and the approved IDE operations plan.

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
| CLI and MCP server (15 tools) | Implemented | shared query layer; `--json` and MCP `format: "json"`; `atlas_edit` makes the MCP server writable |
| Exact IDE operations (references, declaration, implementations, outline, diagnostics) | Implemented, needs a language server | rust-analyzer and typescript-language-server 5.x (TypeScript 7 refused); answers are live `[compiler]`, never stored; `PARTIAL` when the server's view is incomplete |
| Exact edits (rename, safe delete, symbol and text edits, plan files) | Implemented | atomic with rollback, refuses stale or unverified plans (exit codes 5 to 7); rename determinism is scoped (see limitations); no agent evaluation |
| SCIP ingestion (`compiler_verified`) | Implemented, opt-in | manual `catl scip`; never runs inside a query |
| Near-duplicate and lexical search | Implemented | similarity is evidence, never equivalence |
| Events, jobs, DB tables/queries | Experimental | entities and edges carry an `experimental` attribute (not shown in text output); heuristic only; project-scoped assumptions |
| LLM enrichment (`enrich`: similarity verdicts, links for unmatched client calls, optional capability summaries) | Experimental, off by default | results are `inferred`, tagged, and go stale with their evidence; `enrich --dry-run` previews candidates; no verdicts exist in the published fixtures |
| Rename lineage (`renamed_from`) | Implemented; confidence calibration experimental | heuristic confidence; inferred facts migrate only at 0.85 or higher |
| Quality eval, perf bench, agent A/B harness | Implemented | results in [docs/evaluation/results.md](docs/evaluation/results.md) |
| Other languages (Python, Go, Java, ...) | Unsupported | |
| Automatic SCIP | Implemented, opt-in (default off) | runs only while a watcher is active, after a debounce (default 30 s); explicit `scip` always works |
| Automatic LLM enrichment | Not implemented | `enrich` is always an explicit command |

## Development

```sh
./scripts/check.sh     # fmt --check, clippy -D warnings, test --workspace
```

Rules for contributors (certainty discipline, append-only `atlas-core` contracts, no comments, typed errors) are in [CLAUDE.md](CLAUDE.md).
