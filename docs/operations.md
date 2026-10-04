# Operations

All commands below were run against `fixtures/acme` with a scratch `CODE_ATLAS_HOME` at commit `a03744e`.

## Building and the SQLite flag

```sh
cargo build --release                  # code-atlas, atlas-bench, atlas-eval in target/release
cargo install --path crates/atlas-cli
./scripts/check.sh                     # fmt --check, clippy -D warnings, test --workspace
```

The repository's `.cargo/config.toml` sets `LIBSQLITE3_FLAGS="-USQLITE_ENABLE_MEMORY_MANAGEMENT -DSQLITE_DEFAULT_MEMSTATUS=0"`. Without those flags the bundled SQLite uses one process-wide page cache behind a global mutex, so concurrent queries serialise.

- The flag applies to workspace builds and `cargo install --path crates/atlas-cli`.
- It is not applied by `cargo install --git`. Pass it explicitly: `LIBSQLITE3_FLAGS="-USQLITE_ENABLE_MEMORY_MANAGEMENT -DSQLITE_DEFAULT_MEMSTATUS=0" cargo install --git <url> code-atlas`.
- Losing it costs throughput, never correctness.
- `code-atlas doctor` reports it under a check called "sqlite build" (ok: "per-connection page caches"; warn when compiled with ENABLE_MEMORY_MANAGEMENT).
- The MCP server uses max(configured readers, cores) read connections; the default is 4 readers.

## Where things live

| Item | Location |
|---|---|
| Registry | `<config dir>/projects.toml` (locked by `projects.toml.lock` while written) |
| Project database | `<data dir>/<project>/atlas.db` (plus `-wal`, `-shm`, `atlas.db.index.lock`) |
| SCIP cache | `<data dir>/<project>/scip-cache/` |
| Repo marker | `.code-atlas.toml` in a repo (`project = "<name>"`), written by `init` |
| Bench corpora | `<data dir>/bench-corpora/` |

Resolution order for the home:

1. `CODE_ATLAS_HOME=<root>`: config in `<root>/config`, data in `<root>/data`. Use it for experiments, tests and CI.
2. Otherwise XDG: config `$XDG_CONFIG_HOME/code-atlas` (default `~/.config/code-atlas`), data `$XDG_DATA_HOME/code-atlas` (default `~/.local/share/code-atlas`). Relative or empty XDG values are ignored.

Project resolution (for commands without `-p`): the `.code-atlas.toml` marker found walking up from the working directory; otherwise the single project having a repo that contains the working directory. No match exits 2 with a hint; several matches exit 2 listing the candidates.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | answered with results |
| 1 | runtime error |
| 2 | usage or config error (bad flags, ambiguous or no project, invalid values) |
| 3 | degraded: results were printed but the index is stale, partial or incomplete (also an `enrich` run that skipped tasks) |
| 4 | answered, but nothing matched (target not found or ambiguous, run or file not found) |

Verified: `index` and queries on a healthy index exit 0; `symbol Nope` exits 4; running outside any project exits 2; `enrich` with LLM disabled exits 3.

## CLI reference

Global flags: `-p, --project <NAME>`, `--log-level <FILTER>` (overrides `RUST_LOG`), `--log-format text|json` (affects logs only, not output).

| Command | Purpose |
|---|---|
| `project create <NAME> --repo <PATH[=NAME]>...` | register a project; repo names default to a slug of the directory name |
| `project add-repo <PROJECT> <PATH> [--name]`, `remove-repo`, `rename-repo <PROJECT> <OLD> <NEW>` | edit repos; removed or renamed repos are dropped or re-keyed on the next query or index |
| `project list [--json]`, `project show <NAME> [--json]` | inspect the registry |
| `project delete <NAME> [--purge-data] [-y]` | unregister; `--purge-data` also deletes the database and caches |
| `project config <PROJECT> [flags]` | show or change settings (below) |
| `init` | write `.code-atlas.toml` and add the current directory as a repo of `-p <project>` |
| `index [--full]` | index now; `--full` re-analyzes every file |
| `status [--json]` | freshness, parse and skip counts, unresolved reasons, edge certainty histogram per kind, SCIP coverage, inferred-fact counts, LLM spend, recent runs |
| `watch` | keep the index fresh in the foreground until Ctrl-C; reloads on registry changes, persists similarity on exit |
| `scip` | run compiler-grade indexers now |
| `enrich [--budget-calls N] [--budget-tokens N]` | opt-in LLM enrichment |
| `query overview\|search\|symbol\|callers\|callees\|dependents\|dependencies\|impact\|tests-for\|api\|similar\|unused\|patterns\|context\|explain\|read\|changes` | the query surface; all accept `--json` |
| `search`, `impact`, `context`, `callers`, `symbol`, `api`, `explain` | shortcuts for the same `query` subcommands (no shortcuts for `callees`, `similar`, `unused`, `tests-for`, `read`, `changes`, `overview`) |
| `connect claude [--scope project\|user] [-y]` | wire into Claude Code, see [agent integration](agent-integration.md) |
| `mcp [--watch]` | serve MCP on stdio |
| `debug run <ID>`, `debug explain-context <TASK>`, `debug dump-file <REPO:PATH>` | per-run phase timings and counts, context scoring, raw facts of a file |
| `doctor` | check home, registry, repos, database integrity, index lock, `rust-analyzer`, `npx`, `claude`, SQLite build |

Every query accepts `--json`. Other flags are per command (`code-atlas query <command> --help` is authoritative):

| Command | Flags |
|---|---|
| `search` | `--kind`, `--repo`, `--limit`, `--include-stale` |
| `symbol` | `--body`, `--kind`, `--repo` |
| `callers`, `callees`, `dependents`, `dependencies`, `impact` | `--depth`, `--limit`, `--min-confidence`, `--kind`, `--repo` |
| `tests-for` | `--limit`, `--min-confidence`, `--kind`, `--repo` |
| `api` | `--repo`, `--endpoint "GET /x"`, `--limit`, `--min-confidence` |
| `similar` | target or `--snippet FILE:START-END`, `--limit`, `--threshold`, `--include-stale`, `--kind`, `--repo` |
| `unused` | `--kind`, `--repo`, `--limit` |
| `patterns` | `--limit`, `--no-excerpts` |
| `context` | `--budget`, `--seed`, `--explain`, `--min-confidence` |
| `explain` | `--kind`, `--repo` |
| `read` | `--signature`, `--context N`, `--kind`, `--repo` |
| `changes` | `--since-run`, `--limit` |

`--snippet` takes a path relative to the working directory (resolved to a file inside a registered repo) or an explicit `repo:path:START-END`.

Quickstart output (acme): `indexed acme: run 1, 58 files updated`; `status` shows `fresh run=1`, `parse ok 58`, `scip: never run`.

## Project config

`code-atlas project config <PROJECT>` prints settings; flags change them (`--reset` restores defaults first):

| Setting | Flag | Default |
|---|---|---|
| `llm.enabled` | `--llm on\|off` | off |
| `llm.provider` | `--llm-provider claude-cli\|anthropic\|openai-compatible\|none` | claude-cli when enabled |
| `llm.model` | `--llm-model <MODEL>` | per-task default |
| `scip.enabled` | `--scip on\|off` | on (not read by anything, see below) |
| `scip.debounce_secs` | `--scip-debounce <SECS>` | 30 (not read by anything) |
| `max_file_bytes` | `--max-file-bytes <BYTES>` | 1048576 |

Changing `max_file_bytes` changes the project fingerprint; running engines reload. Per-repo `include`, `exclude` (glob patterns) and `languages` can only be set by editing `projects.toml`:

```toml
[[project.repo]]
name = "acme-api"
path = "/abs/path/acme-api"
exclude = ["**/*.generated.ts"]
languages = ["typescript"]
```

The registry denies unknown fields. Default excluded directories: `node_modules target dist build .git .next coverage vendor` (an `include` pattern naming one re-enables it); hidden files and `.gitignore`d files are skipped; symlinks and files above `max_file_bytes` are skipped and listed in `status`.

## Watch and freshness

You never need `watch` for correctness: every query runs the freshness barrier. `watch` (and `mcp --watch`) uses filesystem events to make the barrier cheap, with a 60 s full-scan safety interval and a 200 ms debounce. Run `watch` when you want updates indexed in the background; it does not run SCIP or enrichment.

## SCIP

`code-atlas scip` runs `rust-analyzer scip` per Cargo package and `npx @sourcegraph/scip-typescript` per npm package, ingests the results as `compiler_verified` edges, and prints per-package counts (`acme-api:package.json [scip-typescript] ingested 32 files, 118 edges`). Afterwards `status` shows `scip: current (47/58 files covered)` and edge histograms gain `compiler_verified` counts (acme: `calls: compiler_verified 63, heuristic 10, resolved 65`).

- Never automatic and never inside a query. Any file re-indexed later is marked lagging until the next `scip` run.
- Needs `rust-analyzer` (PATH or `rustup which`) and `npx` (cached package or `npm ping`); `doctor` checks both. Default timeout 10 minutes per package; rust-analyzer needs about 2 GB RSS and builds the workspace.
- Runners leave no files in the repo (`CARGO_TARGET_DIR` is redirected into the cache; `Cargo.lock` is restored; missing `tsconfig.json` is inferred via a symlink overlay).
- Results are cached by indexer version and input file hashes under `scip-cache/`; the cache is never garbage-collected.
- The `scip.enabled` and `scip.debounce_secs` settings are currently inert: `code-atlas scip` ran with `--scip off` set.

## LLM enrichment, settings and costs

Disabled by default. Enable per project:

```sh
code-atlas project config acme --llm on --llm-provider claude-cli      # uses the local `claude` CLI session
code-atlas project config acme --llm on --llm-provider anthropic       # needs ANTHROPIC_API_KEY
code-atlas project config acme --llm on --llm-provider openai-compatible --llm-model <model>   # needs OPENAI_BASE_URL (and OPENAI_API_KEY)
code-atlas -p acme enrich --budget-calls 50 --budget-tokens 200000
```

- `enrich` classifies candidate similarity pairs (structural clusters with Jaccard 0.6 or more, name-related) and re-runs stale facts whose evidence changed. Results are stored as `inferred` facts. Nothing else is sent to a provider; capability summaries and client-call linking are not wired in.
- Defaults per run: 50 calls, 200,000 input tokens, concurrency 4. Calls are skipped (and reported) once the budget is exhausted. Responses are cached by prompt hash, so repeated runs on unchanged code make no calls.
- Default models: `claude-sonnet-5-5` for similarity, `claude-haiku-4-5-20251001` for lighter tasks; `--llm-model` overrides all tasks. The `claude-cli` provider runs `claude -p` without tools, settings or session persistence.
- Cost: `status` shows `llm calls N tokens in X out Y cost ~$Z`. The estimate uses a built-in price table of unverified list prices for Sonnet and Haiku families; models outside the table have no estimate (tokens are still counted). The `claude-cli` provider consumes the user's Claude subscription or API usage; treat the figure as a guide.
- With the provider unavailable or disabled, `enrich` skips tasks, the run is recorded `partial`, and the exit code is 3. Inferred facts older than their evidence show as `stale`.

## Doctor

`code-atlas doctor` prints one line per check (`ok`, `warn`, `fail`) and a summary (`0 failures, 0 warnings`): home paths, registry parse, whether the working directory is in a project, every repo path, database integrity (`PRAGMA integrity_check`), index lock state, `rust-analyzer`, `npx`, `claude` versions, and the SQLite build flags. Run it first when something looks wrong; follow with `status` and `debug run <id>`.

## Logging

Logs go to stderr. `--log-level info` or `RUST_LOG=atlas_engine=debug`; `--log-format json` for structured logs. The MCP server writes logs to stderr only, never stdout.
