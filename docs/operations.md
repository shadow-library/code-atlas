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
| `mcp [--watch]` | serve MCP on stdio for the project of `-p` or the working directory; where no project resolves it serves only `atlas_status`, which explains how to register the repo |
| `debug run <ID>`, `debug explain-context <TASK>`, `debug dump-file <REPO:PATH>` | per-run phase timings and counts, context scoring, raw facts of a file |
| `doctor [--lsp]` | check home, `config.toml`, registry, the project resolution trace, ignored configuration values, a user-scope Claude Code entry that still pins `--project`, repos, database integrity, index lock, `rust-analyzer`, `typescript-language-server` discovery (one line per candidate), TypeScript 7 repos, `npx`, `claude`, SQLite build; `--lsp` also starts each language server and reports its health (slow) |

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

Worktrees are not indexed yet. A directory inside a git worktree of a registered repo, inside the repo (`.worktrees/<name>`, `.claude/worktrees/<name>`) or outside it (`../repo-feature`), resolves to the project that indexes the main checkout. Code Atlas detects the worktree from its `.git` file (`gitdir:` and `commondir`, no `git` process), so it never treats it as the main checkout:

- Queries still answer, but from the main checkout: the CLI prints `worktree caveat: answers describe the main checkout at <main>, not the git worktree at <worktree> ...` on stderr, and every MCP answer ends with that line (a `caveat` field in JSON).
- Every edit (`catl edit ...`, including dry runs and `edit apply`, and MCP `atlas_edit`) is refused with a message naming both paths, because it would write into the main checkout. The CLI exits 5 and writes nothing. Edits from the main checkout itself are unaffected.
- The same holds for a worktree of a git repo that sits inside a registered folder (a registered parent directory that is not itself a checkout, or an independent repo nested in a registered repo): any linked worktree strictly below a registered repo path binds as a worktree, unless a repo is registered at or inside the worktree itself.
- A `.git` file below a registered repo root that does not resolve (dangling `gitdir`, deleted admin directory, unreadable file) is treated as a broken worktree: answers carry a caveat and edits are refused with a message saying the worktree could not be resolved (`git worktree repair` fixes it). A worktree admin directory that only lacks its `commondir` file is still recognised.
- When the indexed checkout is itself a linked worktree (a bare repository with several worktrees), messages call it the indexed checkout rather than the main checkout.
- A project that registers the worktree's own path wins over the main project there, and `agents sync` (or `connect claude`) run in a worktree writes into the worktree; files it excludes from git go into the common `.git/info/exclude`.
- Limitation: the MCP server decides its binding once, from the directory it starts in. An agent whose server started in the main checkout and that then moves into a worktree (or subagents that share the parent session's servers) is not protected: answers carry no caveat and `atlas_edit` writes into the main checkout. Start the agent session inside the worktree. The CLI resolves on every call, so it has no such gap.
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

- **Fact keys** change what is indexed: `max_file_bytes`, `detect.route_families` (with the definitions of the families it names) and each repo's `include`, `exclude` and `languages`. They stop at the repo layer and enter the project fingerprint, so changing them makes every engine re-adopt the project and re-index what changed. Environment variables and flags never change them (`CODE_ATLAS_MAX_FILE_BYTES` is ignored with a warning), so two processes with different environments cannot write different facts into one database.
- **A repository file is data only.** `.code-atlas.toml` may set `project`, `[index] include`, `exclude`, `languages` (for its own repo), `[detect] route_families` (added to the project's list) and `max_file_bytes` (for the whole project, applied only when every repo that sets it agrees). It can only name a route family; the definition always comes from the user's config directory. Executables (`lsp.typescript_command`, `agents.binary`), credential settings (`llm.api_key_env`, `llm.openai_base_url`) and machine settings (`lsp.*`, `scip.*`, `llm.*`, `mcp.*`) are ignored there with a warning, because cloning a repository must never run its command or redirect a key.

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
tools = "full"                     # stored and validated; not acted on yet

[agents]                           # what `catl agents sync` writes
binary = "catl"                    # command agents start; an absolute path only in this file
guidance = "auto"                  # "auto" | "none" | "local" (CLAUDE.local.md, git-excluded) | "tracked" (CLAUDE.md / AGENTS.md)

[agents.claude]
enabled = true
scope = "user"                     # "user" | "local" | "project" (.mcp.json, committed)
hooks = ["auto-approve"]           # "auto-approve": permissions.allow for the read-only tools; "remind": once-per-session nudge on grep; [] for none

[agents.codex]
enabled = true                     # only when $CODEX_HOME or ~/.codex exists

[worktrees]                        # validated; not acted on yet
mode = "overlay"
gc_after_days = 14
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

A route family describes a decorator-based HTTP framework that Code Atlas does not compile in (only NestJS is built in). Declare it once per user in `<config dir>/detectors/<name>.toml` (under `CODE_ATLAS_HOME/config` when that is set) and enable it where it is used. The file name is the family name: a lowercase letter followed by lowercase letters, digits, `-` or `_`; the names of built-in frameworks (`nestjs`, `express`, `fastify`, `koa`, `axum`, `actix`, `rocket`) are refused. Matching is described in [languages and frameworks](languages-and-frameworks.md#declared-route-families).

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

[guards]
decorators = ["Guarded"]             # each argument becomes a requires_guard edge

[params]                             # optional; recorded as endpoint attrs
path = ["PathParam"]                 # -> params
headers = ["Header"]                 # -> headers
body = ["Payload"]                   # -> body_type (the parameter's declared type)

[global_prefix]                      # optional
factory = "AcmeServer.create"        # const app = await AcmeServer.create(...)
method = "setGlobalPrefix"           # app.setGlobalPrefix('v2')
```

`packages`, `[controller] decorators` and `[routes.methods]` are required. Each decorator name has one role. Unknown keys are ignored with a warning (`detectors/acme-http.toml: unknown key client ignored`), so a file written for a newer `catl` still loads. Invalid files:

- a file whose name is not a family name (`Acme Http.toml`) is skipped with a warning;
- an invalid value, a missing required key or broken syntax in a family that no project enables is skipped with a warning naming the file and the key;
- the same in a family that some project enables is an error naming the file and the key (`invalid config file .../detectors/acme-http.toml: routes.methods.Read: expected an HTTP method ...`, exit 2). It stops every command, including a running MCP server's next call, until the file is fixed or the family is no longer enabled.

`packages` holds import specifiers compared as written:

- A decorator counts only when it is imported from a listed specifier exactly. A file importing `@acme/http/testing` is examined when `@acme/http` is listed, but its decorators count only if `@acme/http/testing` is listed too.
- Decorators re-exported through a barrel inside the repository (`import { Api } from './http'`) do not match; list the barrel's specifier if every file writes it the same way.
- A relative specifier in `packages` is matched literally, so it only matches files that spell the import the same way.

Enable a family with the fact key `detect.route_families`:

- for every project: `config.toml` `[defaults.detect] route_families = ["acme-http"]`;
- for one project: `projects.toml` `[project.settings.detect] route_families = ["acme-http"]` (edit the file; `project config` has no flag for it);
- from a repository: `.code-atlas.toml` `[detect] route_families = ["acme-http"]`, which adds to the project's list.

The effective list is the project value plus every repo file's list; it applies to all repos of the project, which is harmless because matching is anchored on imports of the family's packages. Environment variables cannot set it (`CODE_ATLAS_DETECT_ROUTE_FAMILIES` is ignored with a warning). An enabled name without a file is ignored with a warning and reported by `status`. `catl config show --origin` lists `detect.route_families` with every layer that contributed; `doctor` lists the declared families.

The enabled names and the definitions they resolve to enter the project fingerprint and the version of the `http-endpoints-declared` detector. Editing a family file, or enabling or disabling one, therefore re-analyses every file of the project on the next query (once), and a long-lived MCP server reloads at its next call instead of re-analysing files back and forth with a CLI that reads the new definition. `catl watch` does not re-read family files on its own: it picks up an edit only after another process (any `catl` command or the MCP server) has re-adopted the project; restart it otherwise. Projects that enable no family keep their fingerprint and detector list unchanged.

A family's global prefix is stored as `other:http_family_prefix:<repo>:<family>:<prefix>`, separate from the NestJS `other:http_global_prefix:<repo>:<prefix>`, so a NestJS app and a family app in one repo with the same prefix value keep one prefix each.

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
