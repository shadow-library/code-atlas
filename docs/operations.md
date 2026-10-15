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

tree-sitter is vendored in `vendor/tree-sitter` with one patch (`vendor/tree-sitter.patch`, procedure in `vendor/README.md`) that keeps an unterminated `sql<a><a>…` at end of file from allocating quadratic memory. Workspace builds, `cargo install --path` and `cargo install --git` apply it; only a crate in another workspace that depends on the analyzers gets upstream tree-sitter.

## Where things live

| Item | Location |
|---|---|
| Global defaults | `<config dir>/config.toml` (optional; see [Configuration layers](#configuration-layers)) |
| Registry | `<config dir>/projects.toml` (locked by `projects.toml.lock` while written) |
| Route families | `<config dir>/detectors/<name>.toml` (optional; see [Route families](#route-families)) |
| Project database | `<data dir>/<project>/atlas.db` (plus `-wal`, `-shm`, `atlas.db.index.lock`) |
| SCIP cache | `<data dir>/<project>/scip-cache/` |
| Repo marker and repo layer | `.code-atlas.toml` in a repo (`project = "<name>"` pin, optional `[index]` and `[detect]` keys), pin written by `init` |
| Bench corpora | `<data dir>/bench-corpora/` |
| Agent wiring record | `<data dir>/wiring.json` (what `agents sync` wrote; `wiring.json.lock` while it runs) |
| Hook state | `<data dir>/hooks/reminded/` (one empty file per session the `remind` hook answered; dropped after 7 days) |

The command is `catl`. The home directories, `CODE_ATLAS_HOME`, the `.code-atlas.toml` marker and the MCP server name keep the project name `code-atlas`.

Resolution order for the home:

1. `CODE_ATLAS_HOME=<root>`: config in `<root>/config`, data in `<root>/data`. Use it for experiments, tests and CI.
2. Otherwise XDG: config `$XDG_CONFIG_HOME/code-atlas` (default `~/.config/code-atlas`), data `$XDG_DATA_HOME/code-atlas` (default `~/.local/share/code-atlas`). Relative or empty XDG values are ignored.

Project resolution (for commands without `-p`): the nearest `.code-atlas.toml` with a `project` key, walking up from the working directory (a marker without one pins nothing and the walk continues); otherwise the single project having a repo that contains the working directory. No match exits 2 with a hint; several matches exit 2 listing the candidates. `doctor` prints each step of this resolution.

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

Verified: `index` and queries on a healthy index exit 0; `symbol Nope` exits 4; running outside any project exits 2 (except `mcp`, which then serves only `atlas_status`); `enrich` with LLM disabled exits 3. Exit codes 5 to 7 are read from the code (`crates/atlas-cli/src/error.rs`) and the `--help` text; they were not re-run for this page.

## CLI reference

Global flags: `-p, --project <NAME>`, `--log-level <FILTER>` (overrides `RUST_LOG`), `--log-format text|json` (affects logs only, not output).

| Command | Purpose |
|---|---|
| `project create <NAME> --repo <PATH[=NAME]>...` | register a project; repo names default to a slug of the directory name |
| `project add-repo <PROJECT> <PATH> [--name] [--include GLOB]... [--exclude GLOB]... [--languages rust,ts]`, `remove-repo`, `rename-repo <PROJECT> <OLD> <NEW>`, `set-repo <PROJECT> <REPO> (--include\|--exclude\|--languages\|--clear-filters)` | edit repos (`set-repo` replaces the given filters atomically); removed or renamed repos are dropped or re-keyed on the next query or index |
| `project list [--json]`, `project show <NAME> [--json]` | inspect the registry |
| `project delete <NAME> [--purge-data] [-y]` | unregister; `--purge-data` also deletes the database and caches |
| `project config <PROJECT> [flags]` | show the effective settings or change the ones stored in `projects.toml` (below) |
| `config show [--effective] [--origin] [--json]` | print configuration keys of the project in scope (`-p` or the working directory; global keys only outside every project): by default only keys set somewhere, `--effective` all of them, `--origin` with the layer each value came from |
| `init` | write `.code-atlas.toml` and add the current directory as a repo of `-p <project>` (created if missing) |
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
| `agents sync [--agent claude\|codex]... [--scope user\|local\|project] [--repo PATH]... [--dry-run] [--force] [--migrate] [-y]` | wire Claude Code and Codex as `config.toml [agents]` asks (`catl mcp --watch`, no fixed project), plus hooks and guidance, and record it in `wiring.json`; refuses to overwrite entries changed since (exit 2) unless `--force`; see [agent integration](agent-integration.md#wiring-agents-agents-sync) |
| `agents status [--json]` | each recorded entry as `ok`, `changed`, `removed` or `unreadable`; exits 3 on drift |
| `agents unsync [--agent A]... [--scope S] [--repo PATH]... [--dry-run] [--force] [-y]` | remove exactly what `agents sync` recorded |
| `connect claude [--scope project\|user] [-y]` | the earlier per-repo wiring, unchanged: project scope (default) writes `.mcp.json` with `catl mcp --project <p> --watch`, user scope registers this binary's absolute path; both add the guidance block; recorded in `wiring.json` but no permission rules or hooks |
| `hook remind` | answer a Claude Code `PreToolUse` hook from the JSON on stdin (written by `agents sync`); exits 0 whatever happens, including an unknown hook name |
| `mcp [--watch] [--tools PROFILE]` | serve MCP on stdio for the project of `-p` or the working directory; where no project resolves it serves only `atlas_status`, which explains how to register the repo. `--tools` (`core`, `read-only`, `full` or `name,name`) overrides `[mcp] tools`; see [tool profiles](agent-integration.md#tool-profiles) |
| `debug run <ID>`, `debug explain-context <TASK>`, `debug dump-file <REPO:PATH>` | per-run phase timings and counts, context scoring, raw facts of a file |
| `doctor [--lsp]` | check home, `config.toml`, registry, the project resolution trace, ignored configuration values, a user-scope Claude Code entry that still pins `--project`, repos, database integrity, index lock, `rust-analyzer`, `typescript-language-server` discovery (one line per candidate), TypeScript 7 repos, `npx`, `claude`, SQLite build, the `[mcp] tools` profile `catl mcp` would serve (FAIL when it would refuse to start); `--lsp` also starts each language server and reports its health (slow) |

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
| `context` | `--budget`, `--seed`, `--known`, `--depth` (0 to 3 graph hops from the seeds, default 3), `--explain`, `--min-confidence` |
| `explain` | `--kind`, `--repo` |
| `read` | `--signature`, `--context N`, `--kind`, `--repo` |
| `changes` | `--since-run`, `--limit` |

`--snippet` takes a path relative to the working directory (resolved to a file inside a registered repo) or an explicit `repo:path:START-END`.

Every edit subcommand takes `--dry-run` (diffs and a plan id, nothing written), `--plan-out FILE` (with `--dry-run`: write the plan as JSON) and `--json`. A plan file is versioned (`version: 1`); a plan with a missing or different version, another project's plan, or one whose files changed since planning is refused (exit 5). Rename and safe-delete plans are also refused when the index recorded any file change since the plan was made. Targets are names, name paths (`A/b`), keys, `[repo:]path:LINE[:COL]` (1-based line, UTF-8 byte column) or `[repo:]path`.

Quickstart output (acme): `indexed acme: run 1, 58 files updated`; `status` shows `fresh run=1`, `parse ok 58`, `scip: never run`.

### Worktrees

Code Atlas detects a git worktree of a registered repo from its `.git` file (`gitdir:` and `commondir`, no `git` process), whether it sits inside the repo (`.worktrees/<name>`, `.claude/worktrees/<name>`) or outside it (`../repo-feature`), and never treats it as the main checkout. What happens there is set by `[worktrees] mode` in `config.toml`:

| Mode | In a worktree |
|---|---|
| `overlay` (default) | answers come from the worktree's own index, without a caveat, and edits write the worktree |
| `main` | answers come from the main checkout with a `worktree caveat:` line, and every edit is refused |
| `off` | Code Atlas does not serve the worktree: commands exit 2, and `catl mcp` serves only `atlas_status`, which says why |

**Overlays.** The first command (or MCP server start) in a worktree derives an overlay project, shown as `<project>@<worktree directory>` (for example `shop@feature`): the same repo names, settings and repo files, except that each repo registered inside the checkout the worktree belongs to (its root or any subdirectory) is read from the same place in the worktree; the project's other repos stay on their own checkouts. The worktree's own `.code-atlas.toml` applies. Overlays are state, not configuration: they are recorded in `<data dir>/worktrees.json`, never in `projects.toml`, and `project config`, `project show` and `--project` do not know them.

- Seeding. The overlay's database (`<data dir>/_worktrees/<project>.<worktree>-<hash>/atlas.db`) starts as a copy of the project's database, taken with SQLite's online backup API while holding the project's index lock, so it never contains half a run; a project that was never indexed gives an empty overlay. Seeding needs at least twice the project database's size free on the data directory's filesystem; otherwise the worktree is served as in `main` mode with a warning. The copy is written next to its final name and renamed into place only when complete, so an interrupted seed leaves no overlay, and the next use seeds again. The first query then runs a normal incremental update: every file of the worktree's repos is hashed once (the stored stat stamps describe the main checkout's files), but only files whose content differs from the main checkout are analysed again. `catl status` and MCP `atlas_status` in a worktree name the overlay, its project, the repos read from the worktree and when it was seeded, and their hints name the project, not the overlay.
- SCIP and similarity. SCIP facts and the similarity index are copied with the database. SCIP facts of files changed in the worktree lag until `catl scip` (or automatic SCIP under `watch`) runs there; `status` says so.
- Edits. `catl edit ...` and `atlas_edit` write only the worktree's files (and the files of the project's other repos, which are not part of the worktree; the edit output marks each of those `(outside the worktree: <checkout>)`, and JSON gives it as `outside_worktree`). Overlay edits take the project's own edit lock, so edits from the main checkout and from any of its worktrees run one at a time and never overwrite each other's files in a shared repo. Edits stay refused in `main` mode, for a `.git` entry that does not resolve (a broken worktree: `git worktree repair` fixes it), and where no overlay can be derived, with a warning that says why: the worktree belongs to a repo inside a registered folder that is not itself a checkout (or to an independent repo nested in a registered repo), a registered directory does not exist in the worktree (an untracked directory, or one the worktree's branch lacks), or the overlay could not be prepared (the project index stayed locked longer than the lock timeout, the project database could not be copied, or `worktrees.json` is unreadable or from a newer `catl`).
- Independence. Each overlay has its own database and index lock, so the project and its overlays index concurrently; two worktrees of one project get two overlays.
- Cleanup. While a command or MCP server uses an overlay it holds a shared lock on it. At most once an hour, preparing an overlay or running a command in a registered project also removes every unused overlay whose worktree is gone (deleted directory, `git worktree remove` or `prune`, a `.git` file that no longer resolves), whose project is no longer registered, or that has not been used for `gc_after_days` (default 14). Only the overlay's directory under `_worktrees` and its entry in `worktrees.json` are removed, never a project's database. `catl project list` lists each project's overlays with their state (`ready`, `not seeded`, `worktree gone`) and last use; `catl status` in the main checkout lists them too (a JSON `overlays` field; `overlay` inside a worktree).
- Cost. An overlay takes one copy of the project's database. Measured with a release build on an Apple-silicon laptop: for the `acme` fixture (4 repos, 87 source files, 1.2 MB database) the copy took 7 ms and the first command in the worktree 125 ms (a second one 42 ms); for an invented checkout with a Rust crate and a TypeScript package (3,001 files, 63 MB database) the copy took 95 ms, the first command 0.72 s including re-hashing all 3,001 files and analysing the one that differed, and later commands cost the same as in the main checkout (about 0.5 s). Deleting `_worktrees/<name>` by hand is safe while no process uses it.
- A project that registers the worktree's own path wins over the main project there, and `agents sync` (or `connect claude`) run in a worktree writes into the worktree; files it excludes from git go into the common `.git/info/exclude`.
- Limitation: the MCP server decides its binding once, from the directory it starts in. An agent whose server started in the main checkout and that then moves into a worktree (or subagents that share the parent session's servers) is not protected: answers describe the main checkout without a caveat and `atlas_edit` writes into the main checkout. Start the agent session inside the worktree. The CLI resolves on every call, so it has no such gap. A server started in a worktree follows registry changes by deriving its overlay again; if that is no longer possible (the repo or the worktree went away), its tools fail until it is restarted.
- The watcher ignores `.worktrees` and `.claude/worktrees` under a repo, so work in a worktree does not wake the main project's index. Submodules are separate checkouts, not worktrees.

## Configuration layers

Every setting resolves through these layers, later ones winning:

1. built-in defaults;
2. `<config dir>/config.toml` (optional, per user): `[defaults]` for every project, plus `[mcp]`, `[agents]` (read by `agents sync` only, never from the environment or a repository file) and `[worktrees]`;
3. `projects.toml`: the project's own `[project.settings]` and repo filters;
4. `<repo>/.code-atlas.toml` of each registered repo (committable);
5. environment variables `CODE_ATLAS_<KEY>`, the key upper-cased with dots as underscores (`CODE_ATLAS_LSP_ENABLED=false`, `CODE_ATLAS_MCP_WATCH=true`; lists as TOML arrays, `CODE_ATLAS_LSP_EXCLUDE='["fixtures/**"]'`);
6. command-line flags (`mcp --watch` / `--no-watch`).

Two rules cut across the order:

- **Fact keys** change what is indexed: `max_file_bytes`, `detect.route_families` (with the definitions of the families it names), `resolve.workspace_dist` and each repo's `include`, `exclude` and `languages`. They stop at the repo layer and enter the project fingerprint, so changing them makes every engine re-adopt the project and re-index what changed. Environment variables and flags never change them (`CODE_ATLAS_MAX_FILE_BYTES` is ignored with a warning), so two processes with different environments cannot write different facts into one database.
- **A repository file is data only.** `.code-atlas.toml` may set `project`, `[index] include`, `exclude`, `languages` (for its own repo), `[detect] route_families` (added to the project's list), `[resolve] workspace_dist` and `max_file_bytes` (both for the whole project, applied only when every repo that sets them agrees). It can only name a route family; the definition always comes from the user's config directory. Executables (`lsp.typescript_command`, `agents.binary`), credential settings (`llm.api_key_env`, `llm.openai_base_url`) and machine settings (`lsp.*`, `scip.*`, `llm.*`, `mcp.*`) are ignored there with a warning, because cloning a repository must never run its command or redirect a key.

List values combine instead of replacing: a repo's effective `exclude` is its `projects.toml` globs, then `[defaults] exclude`, then the repo file's `[index] exclude`; `include` likewise without the defaults. A repo file's `languages` replaces the registry's.

`config.toml` rejects invalid values of known keys, broken syntax and a `version` other than 1 with an error naming the file (exit 2). Unknown keys and tables there are ignored with a warning, so a file written for a newer `catl` still loads in an older one or in a running MCP server. Without the file everything behaves as before it existed. `llm.api_key_env` must be a variable name and `llm.openai_base_url` an `http(s)://` URL without `user:password@`; neither error echoes the value, in case a key was pasted there. A `.code-atlas.toml` is lenient: unknown keys (for example keys a newer `catl` writes) and invalid values, including an empty `languages = []`, are ignored with a warning, and only broken TOML syntax or a non-string `project` is an error. A marker above or below a repo root only pins `project`; its other keys are ignored with a warning. Warnings print once per command on stderr, in the MCP server log, and in `doctor`.

```toml
# ~/.config/code-atlas/config.toml
version = 1

[defaults]
max_file_bytes = 2097152
exclude = ["**/*.gen.ts"]          # appended to every repo's exclude

[defaults.lsp]
max_servers = 3

[defaults.scip]
enabled = true

[defaults.llm]
provider = "openai-compatible"
openai_base_url = "http://localhost:8080/v1"   # wins over OPENAI_BASE_URL
api_key_env = "LOCAL_LLM_KEY"                  # names the variable; never the key itself

[mcp]
watch = true                       # what `catl mcp` does without --watch / --no-watch
max_output_bytes = 100000
max_body_lines = 400
tools = "full"                     # "full" | "core" | "read-only" | a list of tool names; what `catl mcp` serves without --tools

[agents]                           # what `catl agents sync` writes
binary = "catl"                    # command agents start; an absolute path only in this file
guidance = "auto"                  # "auto" | "none" | "local" (CLAUDE.local.md, git-excluded) | "tracked" (CLAUDE.md / AGENTS.md)

[agents.claude]
enabled = true
scope = "user"                     # "user" | "local" | "project" (.mcp.json, committed)
hooks = ["auto-approve"]           # "auto-approve": permissions.allow for the read-only tools of [mcp] tools; "remind": once-per-session nudge on grep; [] for none

[agents.codex]
enabled = true                     # only when $CODEX_HOME or ~/.codex exists

[worktrees]
mode = "overlay"                   # "overlay" | "main" (main checkout, edits refused) | "off"
gc_after_days = 14                 # remove overlays unused this long
```

```toml
# <repo>/.code-atlas.toml
project = "acme"

[index]
exclude = ["apps/legacy/**"]
```

`catl config show --origin` prints each key with its layer (`built-in`, `config.toml`, `projects.toml`, `<repo>/.code-atlas.toml`, `env:<VAR>`, `flag:<FLAG>`; list keys name every contributing layer):

```
projects.toml                       scip.debounce_secs = 10
env:CODE_ATLAS_LSP_ENABLED          lsp.enabled = false
config.toml + app/.code-atlas.toml  repo.app.exclude = ["**/*.gen.ts", "legacy/**"]
```

**Sparse project settings.** `projects.toml` keeps its format, and every project written before `config.toml` existed stores the built-in defaults explicitly. A stored value equal to the built-in default therefore counts as unset, so a `[defaults]` value reaches it. To keep a built-in value against a different global default, set it with `project config` (for example `--scip off` while `[defaults.scip] enabled = true`): the key is then listed in `pinned = [...]` under `[project.settings]`, which a `catl` older than this layer cannot read. Optional values (`llm.provider`, `llm.model`, `lsp.typescript_command`) cannot be pinned back to unset. `--reset` forgets the project's stored settings and pins.

**Writes keep comments.** `project` commands edit `projects.toml` in place: comments, key order, CRLF line endings and untouched projects are kept, and only changed values are rewritten. `init` adds or changes the `project` key of an existing `.code-atlas.toml` and keeps the rest of it. Each edited text is parsed back and compared with the intended content; if they differ, the file is written as a fresh serialisation without comments and a warning is logged.

**Reloading.** The CLI reads every layer on each command. The MCP server re-reads the project layers before each tool call and reloads when the effective project changed, so an edited `config.toml [defaults]`, registry or repo file applies at the next call; `watch` reloads when another process has re-adopted the project. `[mcp]` keys and `CODE_ATLAS_MCP_*` are read once when `catl mcp` starts; restart the server to change them.

## Route families

A route family describes a decorator-based HTTP framework that Code Atlas does not compile in (only NestJS is built in), the typed client a web app calls it with, or both. Declare it once per user in `<config dir>/detectors/<name>.toml` (under `CODE_ATLAS_HOME/config` when that is set) and enable it where it is used. The file name is the family name: a lowercase letter followed by lowercase letters, digits, `-` or `_`; the names of built-in frameworks and client libraries (`nestjs`, `express`, `fastify`, `koa`, `axum`, `actix`, `rocket`, `fetch`, `axios`, `nest_http`, `openapi-fetch`, `reqwest`) are refused, because a family's name is the `framework` of its endpoints and the `library` of its client calls. Matching is described in [languages and frameworks](languages-and-frameworks.md#declared-route-families).

```toml
# ~/.config/code-atlas/detectors/acme-http.toml  (an invented framework)
version = 1                          # optional; only 1 is understood
packages = ["@acme/http"]            # decorators count only when imported from these

[controller]
decorators = ["Api"]                 # class decorators that make a controller
path = { argument = 0, key = "path" }  # default: argument 0, or its `path` key when it is an object literal

[routes]
path = { argument = 0, key = "path" }  # same default as the controller

[routes.methods]                     # method decorator = HTTP method (GET POST PUT PATCH DELETE HEAD OPTIONS TRACE CONNECT ANY)
Read = "GET"
Write = "POST"
Remove = "DELETE"

[routes.options]                     # optional; decorators whose first argument is an options object
decorators = ["Route"]               # @Route({ method: 'POST', path: 'run' }) or @Route(RUN_OPTIONS)
method = "method"                    # optional; the key holding the HTTP method (default as shown)
path = "path"                        # optional; the key holding the path (default as shown)

[guards]
decorators = ["Guarded", "Authenticated"]    # guard classes in the arguments, or the decorator itself (below)
auth_schemes = { Authenticated = "bearer" }  # optional; the scheme a decorator guard implements: bearer, api_key, session_cookie or basic

[permissions]                        # optional
decorators = ["Permit"]              # each argument (or array item) becomes a requires_permission edge

[params]                             # optional; recorded as endpoint attrs
path = ["PathParam"]                 # -> params
headers = ["Header"]                 # -> headers
body = ["Payload"]                   # -> body_type (the parameter's declared type)

[global_prefix]                      # optional
factory = "AcmeServer.create"        # const app = await AcmeServer.create(...)
method = "setGlobalPrefix"           # app.setGlobalPrefix('v2')
```

`packages`, `[controller] decorators` and `[routes.methods]` or `[routes.options]` are required, unless the file declares only a `[client]` (below). Each decorator name has one role. Unknown keys are ignored with a warning (`detectors/acme-http.toml: unknown key server ignored`), so a file written for a newer `catl` still loads. Invalid files:

- a file whose name is not a family name (`Acme Http.toml`) is skipped with a warning;
- an invalid value, a missing required key or broken syntax in a family that no project enables is skipped with a warning naming the file and the key;
- the same in a family that some project enables is an error naming the file and the key (`invalid config file .../detectors/acme-http.toml: routes.methods.Read: expected an HTTP method ...`, exit 2). It stops every command, including a running MCP server's next call, until the file is fixed or the family is no longer enabled.

Guards and permissions:

- A `[guards]` decorator whose arguments name guard classes (`@Guarded(SessionGuard)`, `@Guarded(new AdminGuard())`, `@Guarded(guards.SessionGuard)` through a namespace import) gives one `requires_guard` edge per class, resolved like a NestJS `@UseGuards` argument. Only a PascalCase name counts as a class. A binding declared in the same file counts only when it is a class, or a constant initialised by a class expression or by `new` or a call of a PascalCase name other than a built-in value type (`AuthGuard('jwt')` counts, `Object.freeze(...)`, `String(...)` and `new Map()` do not), so `@Guarded(ADMIN_ROLE)`, `@Guarded(Role.Admin)`, `@Guarded(sessionGuard)` and such same-file values never yield a guard edge. The detector cannot see other files: an imported PascalCase name (`@Guarded(AdminRole)`) or a PascalCase member of a namespace import (`@Guarded(roles.Admin)`) is taken as a guard class even when it is a value, and its edge then resolves to that value.
- A `[guards]` decorator without such an argument (`@Authenticated()`, `@Auth({ roles: ['admin'], optional: true })`, `@Auth(ADMIN_ROLE)`) is the guard itself: the edge targets `other:guard_decorator:<repo>:<family>:<decorator>` (the exported name, so aliases share it). Its object arguments are recorded on the edge as `options` and its other arguments as `arguments`, with string constants of the same file read and any other name written `{"binding": "Role.Admin"}`; values that cannot be read statically (calls, spreads) are left out. Options next to a guard class are not recorded.
- `auth_schemes` maps such a decorator to an auth scheme: the detector then records `implements_auth` from the decorator guard to `auth:<repo>:<scheme>` (`syntactic`, `guard_kind = "declared_decorator"`), and the link pass derives `supports_auth` for its endpoints and `uses_auth` for clients sending a matching credential, as for guard classes. A decorator guard without a scheme stays an unclassified guard.
- A `[permissions]` decorator's string literal is the permission (`syntactic`); a string constant or a template of constants declared in the same file is read (`resolved`, attr `constant`). The values of imported constants are not read, because the analyzer stores no constant values: any other constant (imported, an enum member) keeps its name as the permission (`permission:<repo>:PROJECTS_READ`, `heuristic 0.60`, rule `permission-constant-unread`, `value_source = "unread_constant"`) and gets a `references` edge (`permission_constant = true`) to the constant, which the engine resolves through imports. The name is the exported one whatever the import style: an alias (`PROJECTS_READ as PR`) is mapped back, a namespace qualifier (`P.PROJECTS_READ`) is dropped, and the qualifier of a named enum or object import stays (`Scope.Admin`). One permission can therefore appear twice: under its value where the constant is declared in the same file or the literal is written, and under the constant's name elsewhere. Other arguments give the `permission-not-literal` diagnostic. Decorators not listed keep the name rule NestJS uses (`Require(s)Permission`, `Role`, `Scope`, string literals only).
- A decorator has one role, so listing one in both `[guards]` and `[permissions]` is an error. Guard and permission decorators match only when imported from `packages`, like every other decorator; list the package that exports them.

An option-route decorator reads its first argument: an object literal, or the name of a `const` object literal declared at the top level of the same file (`as const` and `satisfies` are allowed). As in JavaScript, the last entry with a key wins, and a spread before it is overridden; a spread after it (or a spread with no such entry) may override the method or path at runtime, so the endpoint is capped at `heuristic 0.50` (rule `option-route-spread`) and an Info diagnostic `route-options-spread` names the keys. Objects nested deeper than 32 levels are not read beyond that depth. Its `method` value is a string in any case, a string constant of the same file (`resolved`), or a member such as `Verb.POST`, read by its last name (`heuristic 0.80`, rule `option-route-method-member`); its `path` value follows the rules of a route path (string, template, string constant, array), and a missing path means the controller path alone. Routes read from a `const` object are `resolved`, like routes built from constants. An imported constant cannot be read by a per-file detector: the decorator then yields no endpoint and a `route-options-unresolved` warning, as does an options object without a readable method.

`packages` holds import specifiers compared as written:

- A decorator counts only when it is imported from a listed specifier exactly. A file importing `@acme/http/testing` is examined when `@acme/http` is listed, but its decorators count only if `@acme/http/testing` is listed too.
- Decorators re-exported through a barrel inside the repository (`import { Api } from './http'`) do not match; list the barrel's specifier if every file writes it the same way.
- A relative specifier in `packages` is matched literally, so it only matches files that spell the import the same way.

Enable a family with the fact key `detect.route_families`:

- for every project: `config.toml` `[defaults.detect] route_families = ["acme-http"]`;
- for one project: `projects.toml` `[project.settings.detect] route_families = ["acme-http"]` (edit the file; `project config` has no flag for it);
- from a repository: `.code-atlas.toml` `[detect] route_families = ["acme-http"]`, which adds to the project's list for the whole project, not only for that repository.

The effective list is the project value plus every repo file's list; it applies to all repos of the project, which is harmless because matching is anchored on imports of the family's packages. Environment variables cannot set it (`CODE_ATLAS_DETECT_ROUTE_FAMILIES` is ignored with a warning). An enabled name without a file is ignored with a warning and reported by `status`. `catl config show --origin` lists `detect.route_families` with every layer that contributed; `doctor` lists the declared families.

The enabled names and the definitions they resolve to enter the project fingerprint and the versions of the `http-endpoints-declared` and `http-clients-declared` detectors. Editing a family file, or enabling or disabling one, therefore re-analyses every file of the project on the next query (once), and a long-lived MCP server reloads at its next call instead of re-analysing files back and forth with a CLI that reads the new definition. `catl watch` does not re-read family files on its own: it picks up an edit only after another process (any `catl` command or the MCP server) has re-adopted the project; restart it otherwise. Projects that enable no family keep their fingerprint and detector list unchanged.

A family's global prefix is stored as `other:http_family_prefix:<repo>:<family>:<prefix>`, separate from the NestJS `other:http_global_prefix:<repo>:<prefix>`, so a NestJS app and a family app in one repo with the same prefix value keep one prefix each.

### Typed clients

A `[client]` section describes how a web app calls such a server: a client binding imported from a module, or created by calling a factory imported from one, whose methods take the request path. `openapi-fetch` (`createClient<paths>()`, methods `GET` to `OPTIONS`) is built in and needs no file. A file may hold only a `[client]` section, for a client whose server is not a decorator framework.

```toml
# ~/.config/code-atlas/detectors/acme-books.toml  (an invented client)
[client]
modules = ["@acme/books-client"]       # import specifiers, compared as written (also tsconfig path aliases)
files = ["apps/web/src/api/*.ts"]      # repository-relative globs of in-repo modules imported with ./ or ../
exports = ["booksApi", "default"]      # optional; imported names that are clients (default: every name except the factories)
factories = ["createBooksClient"]      # optional; a binding initialised by calling one of these is a client ("default" for a default export)
methods = ["get", "post", "delete"]    # optional; method names, each an HTTP method in any case (default: GET..OPTIONS in upper and lower case)
path = { argument = 0, key = "path" }  # optional; the path argument, or that key of an object-literal argument (default as shown)
style = "openapi"                      # optional; "openapi" for /books/{id}, "express" for /books/:id
target_package = "@acme/books-api"     # optional; the package whose endpoints these calls target
base = "/api"                          # optional; the base of calls whose client has none of its own
```

`modules` or `files` is required. A relative import (`./api/client`, `../api/client.js`) is resolved against the importing file and matched against `files` with the usual source extensions and `index` files; `*` and `?` match within one path segment and `**` across segments. A call matches when its receiver is a named, aliased, default or namespace-member import from a listed module (`booksApi.get(...)`, `clients.booksApi.get(...)`), a namespace import of such a module when `exports` is not set (`api.get(...)` for `import * as api`), or a local initialised by a listed factory (`const api = createBooksClient({ baseUrl })`). A parameter or local that shadows the import, a rebinding (`const c = booksApi`), destructured methods and class fields are not followed. A factory's `baseUrl` or `baseURL` option becomes the request's base: a literal value is prefixed to the URL without a trailing `/`, a value starting with an environment variable or identifier is recorded as `base_source` (`env:X`, `ident:x`, or `expr` for other expressions such as `import.meta.env.X` inside a template), and a static suffix such as the `/api` of ``${process.env.X}/api/`` stays a path prefix. `headers` options on the factory or the call become auth hints. The certainty is `syntactic` for a literal path, as for `fetch` and `axios`.

A client imported from another file is matched only through `files` or `modules`, so its call records only the path written at the call (`booksApi.get('/books/{id}')` gives `GET /books/{}`), not the creating file's base URL or generated types. For the built-in shape the creating file supplies them: an exported constant initialised by `openapi-fetch`'s `createClient` (`export const booksApi = createClient<paths>({ baseUrl: '/api/' })`) is recorded as an `other:typed_client:<repo>:<path>#<export>` entity with its `base`, `base_source`, `types_module` and `types_path`. When a `files` client is imported by a relative specifier, the call records `client_module` (the resolved module path without extension) and `client_export`, and the link pass joins the two: it matches `<base><path>` against routes, inherits the definition's `base_source` and types for placement, and names the definition in `client_defined_in` on the `calls_endpoint` edge. An exported constant initialised by one of the section's own `factories` is recorded the same way, with its `baseUrl`. Not joined: clients imported through `modules` (package specifiers and tsconfig path aliases cannot be resolved to a file per file), `export default createClient(...)`, `export { api }` lists and re-exports through a barrel (a client defined in a directory's `index` file is found through the directory itself).

`base` is a static base path (`/api/v1`) or origin (`https://api.acme.test/v1`); a trailing `/` is dropped. A call whose client gets no base from a factory option or a surface (below) records it as `client_base`, and the link pass matches `<base><path>` for a path starting with `/`, unless the call is joined to a client definition with a base, which wins. The `client:` key keeps the path as written at the call; the `calls_endpoint` edge records the base it used in `client_base`.

#### Client surfaces

Some transports create one client per API surface from a map of bases and export the members:

```ts
// apps/web/src/transport.ts
import { createApiClient } from '@acme/http-kit';
export const apiClient = createApiClient({ surfaces: { v1: '/api/v1', auth: '/api/auth' }, ssr });
export const APIRequest = apiClient.v1;
```

```toml
[client]
files = ["apps/web/src/transport.ts"]
exports = ["APIRequest", "apiClient"]
methods = ["get", "post", "put", "patch", "delete"]

[client.surfaces]
modules = ["@acme/http-kit"]           # import specifiers the factory comes from, compared as written
factory = "createApiClient"            # the factory's export name ("default" for a default export)
option = "surfaces"                    # the factory option holding { name: base }
containers = ["apiClient"]             # optional; follow members only of these exports
```

A binding initialised by calling the factory (named, aliased or default import from a listed module) is a container, and its member `<name>` is a client with the base mapped to `<name>`. The map is an object literal in the call or a top-level `const` object literal of the same file; computed keys and spreads are skipped, and each value is evaluated like `baseUrl` (a literal is a base, a leading environment variable or identifier becomes `base_source`). In the transport file, an exported container is recorded as one `other:typed_client:<repo>:<path>#<container>.<name>` definition per surface, an exported `const X = container.name` (optionally `as T`) as `#X`, and calls such as `apiClient.v1.get(...)` or `APIRequest.get(...)` in that file get the base directly. In other files, `APIRequest.get('/admin/users').query(q).execute()` (chained builders are matched on their first call) records `client_export = APIRequest`, and with `[client.surfaces]` declared a call on a member of an imported client export, `apiClient.auth.get('/session')` after `import { apiClient } from './transport'` or `transport.apiClient.auth.get(...)` through a namespace import, records `client_export = apiClient.auth`; the link pass joins both to their definitions, so they match `/api/v1/admin/users` and `/api/auth/session`. With `containers` set, only members of the listed exports are followed (`APIRequest.cache.get(...)` is then not a client call); without it, a member of any client export is, and one that finds no definition (`APIRequest.cache`, or `apiClient.v2` when `v2` is not a surface) keeps only its written path: the section's `base` is not applied to it. Without `[client.surfaces]`, members of imports are not followed. Not recognised: `createApiClient(...).v1` without a binding, destructuring (`const { v1 } = apiClient`) and computed members (`apiClient['v1']`).

A URL with no literal text after a leading `/` (`APIRequest.get(`/${id}`)`) is kept for a call that will be joined to a definition or has a `base`, because the base supplies its static segments; other fully dynamic URLs are still dropped with the `http.dynamic_url` diagnostic.

Whole path segments in the declared style (`{id}`, or `:id` for `express`) are rewritten to the `{}` placeholder the link pass matches against routes, and the path as written is kept in the `path_template` attr of the `makes_request` edge. Partial segments (`{name}.json`) and the query string stay literal. `target_package` is copied to every call as an attr and steers link placement (below).

Definitions are recorded only for files a family owns: with `files` set, the files its globs match (importing one of its `modules` does not make a file its own); without `files`, every file, at a lower rank. When several enabled families share one factory (one per web app, each with its own `files`), each records its own transport file only, and a broad family without `files` yields to them. If two families of the same rank claim one definition (two globs matching one file, or two families without `files`), none records it and the file carries a `client-definition-conflict` warning (its `detector_diagnostics` attribute) and `status` warns for each tied family with the number of definitions; narrow their `files`.

The `[client]` section and its enabling follow the rules of the whole file: it is enabled through `detect.route_families` under the file's name, its definition enters the project fingerprint, and it runs as the detector `http-clients-declared`, whose version is a digest of `DECLARED_CLIENTS_VERSION` and every enabled client section, registered only when an enabled family has one. `status` adds the number of client calls each family matched and warns when a client section matched none.

### Several apps in one repository

Endpoint keys are scoped by repository, not by package, so two apps of one monorepo that expose the same route (`GET /health`) share one endpoint entity; key formats never change. The link pass therefore works with packages, the nearest `package.json` or `Cargo.toml` above a file:

- in a repository with more than one manifest, each endpoint and client call records the `packages` its asserting files belong to (two entries for a route both apps expose); a route composed from a global prefix carries only the packages it shares with that prefix;
- when one of an endpoint's packages declares a global prefix, only prefixes of the endpoint's packages are composed onto it and other packages' prefixes no longer are; when none does, every prefix of the repo applies, as before, so an endpoint never loses all of its compositions;
- a client call prefers endpoints of one package, in this order: the package its generated types are imported from (`createClient<paths>()` with `paths` imported from `@acme/books-api/...` or from a relative path inside that package), the client section's `target_package`, and, for a relative URL, the caller's own package. Each step applies only when a matching endpoint lives in that package; otherwise the call is placed across all matching endpoints as before. The `calls_endpoint` edge records `client_package`, `endpoint_packages`, `target_package` and `package_rule` (`types-package`, `configured-package`, `same-package`).

A shared route stays one endpoint with both handlers' `exposes_endpoint` edges, so `callers --via-http` of either handler lists every client of the route; read `target_package` on the link to see which app a call was attributed to.

Limits: generated types that live in the web app itself (`apps/web/src/generated/books.ts`) name the web package, so they narrow placement only if the web package exposes the route; otherwise the next rule applies. A repository whose apps share one root manifest, with no `package.json` or `Cargo.toml` per app, has a single package, so no package placement happens there and calls are placed by route and repository as before.

`status` prints `route family acme-http: 3 controllers, 12 routes` for each enabled family (controllers that expose at least one route), and a warning for a family that matched nothing or has no file:

```
route family acme-http: 3 controllers, 12 routes
warning: route family acme-rpc matched no controllers or routes; check its packages and decorator names
warning: route family acme-htp is enabled but not declared: /home/me/.config/code-atlas/detectors/acme-htp.toml does not exist
```

## Project config

`catl project config <PROJECT>` prints the effective settings; flags change the values stored in `projects.toml` and print what changed in the effective settings. A flag whose key an environment variable or a repo file overrides prints a warning. `--reset` forgets the stored values first:

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

`resolve.workspace_dist` declares where a workspace package's build output maps back to its sources when no tsconfig does (typically a build that passes `outDir` on the command line):

```toml
# config.toml (every project)
[defaults.resolve]
workspace_dist = [{ dist = "dist", source = "src" }]

# projects.toml (one project)
[project.settings.resolve]
workspace_dist = [{ dist = "dist", source = "src" }]

# .code-atlas.toml (a repository)
[resolve]
workspace_dist = [{ dist = "dist", source = "src" }]
```

Both directories are relative to the package root, with no `.`, `..` or empty segments; each `dist` is listed once and none lies inside another. A package entry that lies under `<package>/<dist>` and does not exist is mapped to `<package>/<source>` with the emitted-extension rules (`.js`/`.d.ts` to `.ts`, `.tsx`, `.js`, `.jsx`, an extensionless entry or directory to `index`), and the import is `resolved` instead of the heuristic `dist-to-src` when exactly one source file exists across the mappings (several candidates stay `dist-to-src`). A tsconfig that declares the mapping takes precedence; a mapping whose source file does not exist stays `dist-to-src`. It is a fact key: it enters the project fingerprint, changes the package graph (so every file of the project is re-resolved), and environment variables and flags never set it. The mapping applies to the whole project, whichever repository's file declares it (as for route families); two repository files that declare different lists conflict and both are ignored with a warning that names the files, not the values.

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

`llm.openai_base_url` and `llm.api_key_env` (in `config.toml [defaults.llm]` or a project's `[project.settings.llm]`) replace `OPENAI_BASE_URL` and the key variable (`OPENAI_API_KEY`, or `ANTHROPIC_API_KEY` for `anthropic`); the key itself is only ever read from the environment.

- `enrich` runs, within one shared budget: LLM link tasks for client calls the link pass could not match (candidates are same-method endpoints), similarity classification of structural pairs and of lexical candidates (shared name words, signatures, BM25 neighbours), optional capability summaries (`--summaries`), and revalidation of stale facts. Results are stored as `inferred` facts; a link goes stale when its client file or candidate set changes. Nothing else is sent to a provider. `--dry-run` prints what each task would see.
- Defaults per run: 50 calls, 200,000 input tokens, concurrency 4; the dry run prints the split (links up to 1/5, structural pairs next (no fixed share), lexical pairs up to 2/5, summaries up to 1/5 when enabled, revalidation the rest). Calls are skipped (and reported) once the budget is exhausted. Responses are cached by prompt hash, so repeated runs on unchanged code make no calls.
- Default models: `claude-sonnet-5-5` for similarity, `claude-haiku-4-5-20251001` for lighter tasks; `--llm-model` overrides all tasks. The `claude-cli` provider runs `claude -p` without tools, settings or session persistence.
- Cost: `status` shows `llm calls N tokens in X out Y cost ~$Z`. The estimate uses a built-in price table of unverified list prices for Sonnet and Haiku families; models outside the table have no estimate (tokens are still counted). The `claude-cli` provider consumes the user's Claude subscription or API usage; treat the figure as a guide.
- With the provider unavailable or disabled, `enrich` skips tasks, the run is recorded `partial`, and the exit code is 3. Inferred facts older than their evidence show as `stale`.

## Doctor

`catl doctor` prints one line per check (`ok`, `warn`, `fail`) and a summary (`0 failures, 0 warnings`): home paths, `config.toml` parse, registry parse, whether the working directory is in a project with the resolution trace (`-p` is not consulted here), ignored configuration values, a user-scope Claude Code `code-atlas` entry that still pins `--project` (read from `$CLAUDE_CONFIG_DIR/.claude.json` or `~/.claude.json`, never written), every repo path, database integrity (`PRAGMA integrity_check`), index lock state, `rust-analyzer`, `typescript-language-server` discovery and TypeScript 7 repos, `npx`, `claude` versions, and the SQLite build flags. With `--lsp` it also starts each language server of the project in scope and reports its readiness and health. It exits 3 when there are warnings and 1 on failures. Run it first when something looks wrong; follow with `status` and `debug run <id>`.

## Logging

Logs go to stderr. `--log-level info` or `RUST_LOG=atlas_engine=debug`; `--log-format json` for structured logs. The MCP server writes logs to stderr only, never stdout.
