# Operations

All commands below were run against `fixtures/acme` with a scratch `CODE_ATLAS_HOME` at commit `a03744e`.

The language-server, edit-command and parse-budget material was not part of that run; numbers there say where they come from.

## Building and the SQLite flag

```sh
cargo build --release                  # catl, atlas-bench, atlas-eval in target/release
cargo install --path crates/atlas-cli
./scripts/check.sh                     # fmt --check, clippy -D warnings, test --workspace
```

The repository's `.cargo/config.toml` sets `LIBSQLITE3_FLAGS="-USQLITE_ENABLE_MEMORY_MANAGEMENT -DSQLITE_DEFAULT_MEMSTATUS=0"`. Without those flags the bundled SQLite uses one process-wide page cache behind a global mutex, so concurrent queries serialise.

- The flag applies to workspace builds and `cargo install --path crates/atlas-cli`.
- It is not applied by `cargo install --git`. Pass it explicitly: `LIBSQLITE3_FLAGS="-USQLITE_ENABLE_MEMORY_MANAGEMENT -DSQLITE_DEFAULT_MEMSTATUS=0" cargo install --git <url> atlas-cli`.
- Losing it costs throughput, never correctness.
- `catl doctor` reports it under a check called "sqlite build" (ok: "per-connection page caches"; warn when compiled with ENABLE_MEMORY_MANAGEMENT).
- The MCP server uses max(configured readers, cores) read connections; the default is 4 readers.

## Where things live

| Item | Location |
|---|---|
| Registry | `<config dir>/projects.toml` (locked by `projects.toml.lock` while written) |
| Project database | `<data dir>/<project>/atlas.db` (plus `-wal`, `-shm`, `atlas.db.index.lock`) |
| SCIP cache | `<data dir>/<project>/scip-cache/` |
| Repo marker | `.code-atlas.toml` in a repo (`project = "<name>"`), written by `init` |
| Bench corpora | `<data dir>/bench-corpora/` |

The command is `catl`. The home directories, `CODE_ATLAS_HOME`, the `.code-atlas.toml` marker and the MCP server name keep the project name `code-atlas`.

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
| 4 | answered, but nothing matched (read commands only: target not found or ambiguous, run or file not found) |
| 5 | edit refused, nothing written: stale, foreign or unversioned plan, safe-delete blockers, ambiguous target, a missing file or a file given as a symbol target, no or several matches, scope violation, unverified sites, server refusal. Edit commands never exit 4; an endpoint given as an edit target is an invalid target and exits 2 |
| 6 | edit failed and was rolled back, nothing changed |
| 7 | edit partially applied: some files changed and the rollback failed; the output lists them. Never exits 0 |
| 130 | interrupted; a write in progress finishes first so no file is left half written. If the edit then turns out refused, rolled back or partially applied, the exit is 5, 6 or 7 instead; any other outcome exits 130, never 0 |

An applied or dry-run edit whose index freshness is degraded exits 3. `lsp check` exits 1 when a server failed to start or become ready, 3 when a server is ready but reports an unhealthy workspace. `doctor` exits 3 on warnings and 1 on failures. A language server that is unavailable or disabled exits 1 with an install hint.

Verified: `index` and queries on a healthy index exit 0; `symbol Nope` exits 4; running outside any project exits 2; `enrich` with LLM disabled exits 3. Exit codes 5 to 7 are read from the code (`crates/atlas-cli/src/error.rs`) and the `--help` text; they were not re-run for this page.

## CLI reference

Global flags: `-p, --project <NAME>`, `--log-level <FILTER>` (overrides `RUST_LOG`), `--log-format text|json` (affects logs only, not output).

| Command | Purpose |
|---|---|
| `project create <NAME> --repo <PATH[=NAME]>...` | register a project; repo names default to a slug of the directory name |
| `project add-repo <PROJECT> <PATH> [--name] [--include GLOB]... [--exclude GLOB]... [--languages rust,ts]`, `remove-repo`, `rename-repo <PROJECT> <OLD> <NEW>`, `set-repo <PROJECT> <REPO> (--include\|--exclude\|--languages\|--clear-filters)` | edit repos (`set-repo` replaces the given filters atomically); removed or renamed repos are dropped or re-keyed on the next query or index |
| `project list [--json]`, `project show <NAME> [--json]` | inspect the registry |
| `project delete <NAME> [--purge-data] [-y]` | unregister; `--purge-data` also deletes the database and caches |
| `project config <PROJECT> [flags]` | show or change settings (below) |
| `init` | write `.code-atlas.toml` and add the current directory as a repo of `-p <project>` |
| `index [--full]` | index now; `--full` re-analyzes every file |
| `status [--json]` | freshness, parse and skip counts, unresolved reasons, edge certainty histogram per kind, SCIP coverage, inferred-fact counts, LLM spend, recent runs |
| `watch` | keep the index fresh in the foreground until Ctrl-C; reloads on registry changes, persists similarity on exit |
| `scip` | run compiler-grade indexers now |
| `enrich [--budget-calls N] [--budget-tokens N] [--summaries] [--dry-run]` | opt-in LLM enrichment; `--dry-run` lists each task's candidates without calling the LLM or writing facts; `--summaries` adds module capability summaries |
| `query overview\|search\|symbol\|callers\|callees\|dependents\|dependencies\|impact\|tests-for\|api\|similar\|unused\|patterns\|context\|explain\|read\|changes` | the graph query surface; all accept `--json`. The CLI keeps `read` and `changes`; MCP folded them into `atlas_symbol mode=source` and `atlas_status since_run` |
| `find <NAME_PATH> [--path P] [--depth N] [--body] [--substring] [--kind K]... [--exclude-kind K]... [--max N]` | name-path symbol search in the graph (no server) |
| `outline <[REPO:]PATH> [--depth N] [--source graph\|lsp]`, `refs <TARGET> [--source lsp\|graph]`, `decl <[REPO:]PATH:LINE:COL>` or `decl --path P --regex R`, `impls <TARGET>`, `diagnostics <[REPO:]PATH> [--lines A-B] [--min-severity error\|warning\|info\|hint]` | `outline` answers from the index by default (`syntactic`, no server started; header says so; `--source lsp` for the exact language-server outline); the others are exact answers from the language servers (`refs --source graph` for graph results); `decl --regex` needs exactly one match in the file |
| `hierarchy <TARGET> [--depth N]` | supertypes and subtypes from the graph |
| `edit rename <TARGET> <NEW> [--cross-repo report\|apply] [--allow-unverified]` | exact rename |
| `edit replace-body\|insert-before\|insert-after <TARGET> (--text T \| --text-file FILE\|-)` | symbol edits (no language server needed) |
| `edit safe-delete <TARGET>` | delete only when nothing references the symbol |
| `edit replace <[REPO:]PATH> --pattern P (--text T \| --text-file F) [--regex] [--all]` | text edit in one file; exactly one match unless `--all` |
| `edit replace-in-files --pattern P (--text T \| --text-file F) [--regex] [--include G]... [--exclude G]... [--occurrence ID]... [--expected-count N]` | text edit across files; a dry run unless `--occurrence` or `--expected-count` is given |
| `edit apply <PLAN.json> [--json]` | apply a plan file written by `--dry-run --plan-out` |
| `lsp check [--json]` | start each language server once and report discovery, versions, readiness time and workspace health |
| `search`, `impact`, `context`, `callers`, `callees`, `symbol`, `api`, `explain`, `read`, `tests-for` | shortcuts for the same `query` subcommands (none for `similar`, `unused`, `patterns`, `changes`, `overview`, `dependents`, `dependencies`) |
| `connect claude [--scope project\|user] [-y]` | wire into Claude Code, see [agent integration](agent-integration.md) |
| `mcp [--watch]` | serve MCP on stdio |
| `debug run <ID>`, `debug explain-context <TASK>`, `debug dump-file <REPO:PATH>` | per-run phase timings and counts, context scoring, raw facts of a file |
| `doctor [--lsp]` | check home, registry, repos, database integrity, index lock, `rust-analyzer`, `typescript-language-server` discovery (one line per candidate), TypeScript 7 repos, `npx`, `claude`, SQLite build; `--lsp` also starts each language server and reports its health (slow) |

Every query accepts `--json`. Other flags are per command (`catl query <command> --help` is authoritative):

| Command | Flags |
|---|---|
| `search` | `--kind`, `--repo`, `--limit`, `--include-stale` |
| `symbol` | `--body`, `--kind`, `--repo` |
| `callers`, `callees`, `dependents`, `dependencies`, `impact` | `--depth`, `--limit`, `--min-confidence`, `--kind`, `--repo`; `callers` and `callees` also `--via-http` |
| `tests-for` | `--limit`, `--min-confidence`, `--kind`, `--repo` |
| `api` | `--repo`, `--endpoint "GET /x"`, `--limit`, `--min-confidence` |
| `similar` | target, `--snippet FILE:START-END` or `--code FILE\|- --language rust\|ts\|tsx\|js` (draft code), `--limit`, `--threshold`, `--include-stale`, `--kind`, `--repo` |
| `unused` | `--kind`, `--repo`, `--limit` |
| `patterns` | `--limit`, `--no-excerpts` |
| `context` | `--budget`, `--seed`, `--explain`, `--min-confidence` |
| `explain` | `--kind`, `--repo` |
| `read` | `--signature`, `--context N`, `--kind`, `--repo` |
| `changes` | `--since-run`, `--limit` |

`--snippet` takes a path relative to the working directory (resolved to a file inside a registered repo) or an explicit `repo:path:START-END`.

Every edit subcommand takes `--dry-run` (diffs and a plan id, nothing written), `--plan-out FILE` (with `--dry-run`: write the plan as JSON) and `--json`. A plan file is versioned (`version: 1`); a plan with a missing or different version, another project's plan, or one whose files changed since planning is refused (exit 5). Rename and safe-delete plans are also refused when the index recorded any file change since the plan was made. Targets are names, name paths (`A/b`), keys, `[repo:]path:LINE[:COL]` (1-based line, UTF-8 byte column) or `[repo:]path`.

Quickstart output (acme): `indexed acme: run 1, 58 files updated`; `status` shows `fresh run=1`, `parse ok 58`, `scip: never run`.

## Project config

`catl project config <PROJECT>` prints settings; flags change them (`--reset` restores defaults first):

| Setting | Flag | Default |
|---|---|---|
| `llm.enabled` | `--llm on\|off` | off |
| `llm.provider` | `--llm-provider claude-cli\|anthropic\|openai-compatible\|none` | claude-cli when enabled |
| `llm.model` | `--llm-model <MODEL>` | per-task default |
| `scip.enabled` | `--scip on\|off` | off; when on, SCIP runs automatically while `watch` or `mcp --watch` is active |
| `scip.debounce_secs` | `--scip-debounce <SECS>` | 30; quiet time before an automatic run |
| `max_file_bytes` | `--max-file-bytes <BYTES>` | 1048576 |
| `lsp.enabled` | `--lsp on\|off` | **on**: exact operations may start language servers |
| `lsp.download` | `--lsp-download on\|off` | **on**: a missing typescript-language-server is fetched through `npx` (pinned `typescript-language-server@5` and `typescript@5`) |
| `lsp.typescript_command` | `--lsp-ts-command <WORD>` repeated per argv word (first an absolute path; `--stdio` is added), `--lsp-ts-command-clear` | none |
| `lsp.exclude` | `--lsp-exclude <GLOB>` (repeatable, replaces the list; repo-relative), `--lsp-exclude-clear` | none |
| `lsp.unit` | `--lsp-unit project\|repo` | project: one server per language |
| `lsp.max_servers` | `--lsp-max-servers <N>` | 2 |
| `lsp.idle_secs` | `--lsp-idle-secs <SECS>` | 600; a warm server idle this long is stopped |
| `lsp.ready_timeout_secs` | `--lsp-ready-timeout-secs <SECS>` | 300 |
| `lsp.check_timeout_secs` | `--lsp-check-timeout-secs <SECS>` | 120; how long `lsp check` and flychecks wait |

Changing `max_file_bytes` changes the project fingerprint; running engines reload. A long-lived MCP server adopts changed `lsp` settings without a restart. Per-repo `include`, `exclude` (gitignore-style globs) and `languages` (`rust`, `ts`) are set with `project add-repo` / `project set-repo` or by editing `projects.toml`:

```toml
[[project.repo]]
name = "acme-api"
path = "/abs/path/acme-api"
exclude = ["**/*.generated.ts"]
languages = ["typescript"]
```

The registry denies unknown fields. Default excluded directories: `node_modules target dist build .git .next coverage vendor` (an `include` pattern naming one re-enables it); hidden files and `.gitignore`d files are skipped; symlinks and files above `max_file_bytes` are skipped and listed in `status`.

## Language servers

Exact reads (`outline --source lsp`, `refs`, `decl`, `impls`, `diagnostics`) and exact edits (`rename`, `safe-delete`) come from rust-analyzer and typescript-language-server. Their answers are never stored in the graph and carry `[compiler]` with the server and version.

**Defaults and consent.** `lsp.enabled` and `lsp.download` are both on by default (decision 2026-10-05), so the first exact TypeScript call in a project with no installed server downloads packages through `npx`. `project config <p> --lsp off` disables servers (exact operations then fail with a message saying so), and `--lsp-download off` stops downloads. rust-analyzer is never downloaded: install it with `rustup component add rust-analyzer`.

**Discovery order.**

- rust-analyzer: `PATH`, then `rustup which rust-analyzer`.
- typescript-language-server: the configured `lsp.typescript_command`, `PATH`, `<repo root>/node_modules/.bin` for each TypeScript repo root, the npx cache (`npx --no-install`), then the download (only after `npm ping` answers). Each candidate is probed with `--version`; a broken candidate is skipped with its reason, except that a configured `lsp.typescript_command` that fails stops discovery rather than falling through to another server. `doctor` and `lsp check` print one line per step.
- tsserver: the first repo root whose `node_modules/typescript` is below version 7 and has `lib/tsserver.js`; otherwise the TypeScript bundled with the server, if it can be verified below 7.
- **TypeScript 7 is refused.** It ships no `tsserver.js`. A server reporting TypeScript 7 or later is retired and the call fails as unavailable (the unit refuses for 60 s without spawning); install `typescript@5` beside the server or use the bundled one. `doctor` warns for each repo whose `node_modules` holds TypeScript 7.

**Lifecycle.**

- MCP (`catl mcp`): servers are started by the first exact call, kept warm and shared across calls, at most `lsp.max_servers` (2: one per language by default), the least recently used evicted over the cap, idle ones stopped after `lsp.idle_secs`. There is no prewarm, so the first call per language pays the cold start. A server that crashes is restarted on the next call; three failures within 10 minutes pause that server for 10 minutes. Servers are stopped on SIGTERM and SIGINT.
- CLI: one-shot. `outline --source lsp`, `refs`, `decl`, `impls`, `diagnostics`, `edit rename`, `edit safe-delete` and `lsp check` start a server, wait for readiness, answer and stop it. `edit replace-body`, `insert-before` and `insert-after` start none (they cross-check against a server only when one is already warm, which a one-shot process never has). Cold start, measured: 2 to 5 s for TypeScript on fixtures; 20 to 65 s for rust-analyzer (22 s for a 2-crate workspace, 64 s for `acme-billing`, from the `--help` text), longer on large workspaces. Measured in review, not in a checked-in report: about 58 s cold and 6 s warm on this workspace, with about 2.1 GB resident. Use the MCP server, not repeated CLI calls, for interactive work.
- Servers sync to the working tree before every call from the index run ledger. An unknown change set (a gap in the ledger, more than 1,000 changed files) restarts the server.

**Settings that matter.**

- `lsp.exclude` drops matching Cargo projects from rust-analyzer's `linkedProjects` and refuses exact operations on files under them (for TypeScript it only refuses files, since workspace folders are repo roots). A repository that contains fixture crates needs it: rust-analyzer reported `health = error` on a full copy of this repository until the fixtures were excluded. For this repository: `catl project config <p> --lsp-exclude 'fixtures/**' --lsp-exclude 'crates/*/tests/pin-corpus/**' --lsp-exclude 'eval/**'`. While a server reports `health = error`, reads are answered `PARTIAL` with its message and edits refuse.
- `lsp.unit = repo` runs one server per repo instead of per language.

**Side effects and limits.**

- rust-analyzer can rewrite `Cargo.lock`. Code Atlas does not restore it; the answer carries a warning naming the file (`was rewritten by rust-analyzer since it started`), unlike SCIP, which restores it.
- rust-analyzer builds the workspace (build scripts, proc macros) in a cargo target directory outside the repo, so a first call is slow and uses disk.
- Edit results report native diagnostics only: for Rust, rust-analyzer's own errors with `cargo check` not run.
- Manifest edits (`Cargo.toml`, `package.json`, `tsconfig*.json`) are not in the engine's change set. atlas-ide compares the root and package manifests on every sync and restarts or caveats the server; this is a workaround, tracked as ENGINE-1.

**Checking.** `catl doctor` shows discovery without starting anything (it does not probe the npm registry); `catl doctor --lsp` and `catl lsp check` start each server once and report versions, readiness time and workspace health; `atlas_status` lists running servers with phase, uptime, idle time and resident memory.

## Parse budget

tree-sitter error recovery can loop forever on some malformed Rust and TSX inputs. Every analysis therefore runs under a budget of 2 s plus 1 s per 100 KiB of source, capped at 30 s, charged in the analysing thread's CPU time (a 10x wall-clock backstop applies, and a parse that makes no progress for 2 s of CPU is stopped early). A file that overruns becomes a failed file (`parse budget exceeded (...)`), its facts are removed and the run is `DEGRADED`.

Because overruns depend on machine load, they are retried: once in the same run at 2x the budget (serially, when at most `available_parallelism` files failed), then in later runs after 1 minute, 10 minutes and 1 hour at 4x, 8x and 16x (capped at 30 s). After that the file waits for a change. `status` lists each budget-failed file with its next retry.

Latency costs: a run that first meets K hung files adds about 6 to 10 s to the query that triggers it for small files, whatever K is (review, 24 hung files: first query 100 s under the first revision with uncapped serial retries, 4.6 s with the final capped retries). A due retry adds up to about 10 s to the barrier that runs it (a hang stops at the 8 s stall limit; a file that keeps advancing may use up to 30 s of CPU). Under extreme starvation the wall time grows with the slowdown, up to the 10x backstop: an escalated retry of a large file can take up to 300 s. These review measurements are not in a checked-in report.

## Watch and freshness

You never need `watch` for correctness: every query runs the freshness barrier. `watch` (and `mcp --watch`) uses filesystem events to make the barrier cheap, with a 60 s full-scan safety interval and a 200 ms debounce. Run `watch` when you want updates indexed in the background; it does not run SCIP or enrichment.

## SCIP

`catl scip` runs `rust-analyzer scip` once per Cargo workspace (at the workspace root; standalone crates per package) and `npx @sourcegraph/scip-typescript` per npm package, ingests the results as `compiler_verified` edges, and prints per-package counts (`acme-api:package.json [scip-typescript] ingested 32 files, 118 edges`). Afterwards `status` shows `scip: current (47/58 files covered)` and edge histograms gain `compiler_verified` counts (acme: `calls: compiler_verified 63, heuristic 10, resolved 65`).

- Never inside a query. By default it only runs on `catl scip`; any file re-indexed later is marked lagging until the next run. With `project config <p> --scip on`, a watcher (`watch`, `mcp --watch`) schedules a run once no file event has arrived for `scip.debounce_secs`, on its own thread, never blocking queries. One SCIP run per project at a time (`atlas.db.scip.lock`; explicit runs wait, automatic runs skip, `status` shows "in progress"). An unavailable indexer pauses automatic runs for an hour; a failed package is held while its inputs are unchanged (up to an hour).
- Needs `rust-analyzer` (PATH or `rustup which`) and `npx` (cached package or `npm ping`); `doctor` checks both. Default timeout 10 minutes per package; rust-analyzer needs about 2 GB RSS and builds the workspace.
- Runners leave no files in the repo (`CARGO_TARGET_DIR` is redirected into the cache; `Cargo.lock` is restored; missing `tsconfig.json` is inferred via a symlink overlay).
- Results are cached under `scip-cache/` per indexer and root, keyed by indexer version and input file hashes; the newest 2 entries per root are kept. An index whose inputs changed during the run is returned but never cached.
- Workspace-wide indexing on this repository took 62 s instead of 666 s and produced 2082 cross-crate `compiler_verified` calls instead of 0 (one measurement from the commit message).

## LLM enrichment, settings and costs

Disabled by default. Enable per project:

```sh
catl project config acme --llm on --llm-provider claude-cli      # uses the local `claude` CLI session
catl project config acme --llm on --llm-provider anthropic       # needs ANTHROPIC_API_KEY
catl project config acme --llm on --llm-provider openai-compatible --llm-model <model>   # needs OPENAI_BASE_URL (and OPENAI_API_KEY)
catl -p acme enrich --budget-calls 50 --budget-tokens 200000
```

- `enrich` runs, within one shared budget: LLM link tasks for client calls the link pass could not match (candidates are same-method endpoints), similarity classification of structural pairs and of lexical candidates (shared name words, signatures, BM25 neighbours), optional capability summaries (`--summaries`), and revalidation of stale facts. Results are stored as `inferred` facts; a link goes stale when its client file or candidate set changes. Nothing else is sent to a provider. `--dry-run` prints what each task would see.
- Defaults per run: 50 calls, 200,000 input tokens, concurrency 4; the dry run prints the split (links up to 1/5, structural pairs next (no fixed share), lexical pairs up to 2/5, summaries up to 1/5 when enabled, revalidation the rest). Calls are skipped (and reported) once the budget is exhausted. Responses are cached by prompt hash, so repeated runs on unchanged code make no calls.
- Default models: `claude-sonnet-5-5` for similarity, `claude-haiku-4-5-20251001` for lighter tasks; `--llm-model` overrides all tasks. The `claude-cli` provider runs `claude -p` without tools, settings or session persistence.
- Cost: `status` shows `llm calls N tokens in X out Y cost ~$Z`. The estimate uses a built-in price table of unverified list prices for Sonnet and Haiku families; models outside the table have no estimate (tokens are still counted). The `claude-cli` provider consumes the user's Claude subscription or API usage; treat the figure as a guide.
- With the provider unavailable or disabled, `enrich` skips tasks, the run is recorded `partial`, and the exit code is 3. Inferred facts older than their evidence show as `stale`.

## Doctor

`catl doctor` prints one line per check (`ok`, `warn`, `fail`) and a summary (`0 failures, 0 warnings`): home paths, registry parse, whether the working directory is in a project, every repo path, database integrity (`PRAGMA integrity_check`), index lock state, `rust-analyzer`, `typescript-language-server` discovery and TypeScript 7 repos, `npx`, `claude` versions, and the SQLite build flags. With `--lsp` it also starts each language server of the project in scope and reports its readiness and health. It exits 3 when there are warnings and 1 on failures. Run it first when something looks wrong; follow with `status` and `debug run <id>`.

## Logging

Logs go to stderr. `--log-level info` or `RUST_LOG=atlas_engine=debug`; `--log-format json` for structured logs. The MCP server writes logs to stderr only, never stdout.
