# Agent integration

Code Atlas reaches agents two ways: an MCP stdio server (`catl mcp`) and the CLI (`catl ... --json`). Both call the same query layer, so the answers, tags and caveats are identical.

## MCP tools

The server serves one project (`--project`, or inferred from the working directory), the 15 tools below or a [profile](#tool-profiles) of them, and runs the freshness barrier before every call. It is no longer read-only: `atlas_edit` writes files (tool annotation `destructive_hint = true`); the other 14 tools are annotated read-only. Every tool takes an optional `format`: `"text"` (default, compact) or `"json"`. Output is capped at 100,000 bytes (a truncation note says how to narrow the query) and bodies at 400 lines. Server-side call telemetry (calls, errors, latency, bytes, truncations) is part of `atlas_status`.

| Tool | Use it to | Arguments |
|---|---|---|
| `atlas_context` | start a task: ranked, budgeted pack of code to read (full bodies for top items, signatures, one-liners), with the line range of each item, why it is included and what was omitted | `task`, `budget` (500 to 32000 tokens), `seeds`, `known` |
| `atlas_search` | find where a capability lives or whether similar code exists (names, text, structure) | `query`, `kind`, `repo` (a repo, or a directory as `repo:dir` or `dir`), `limit` (1 to 200) |
| `atlas_symbol` | symbol lookup and IDE navigation; `mode` selects the operation (below) | `target`, `mode`, `path`, `depth`, `body`, `substring`, `kinds`, `exclude_kinds`, `max_matches`, `regex`, `source`, `context_lines`, `lines`, `min_severity` |
| `atlas_edit` | exact renames, symbol edits, safe delete, text replacement, plan application (below) | `operation`, `target`, `path`, `new_name`, `text`, `pattern`, `regex`, `all`, `include`, `exclude`, `occurrences`, `expected_count`, `cross_repo`, `allow_unverified`, `plan`, `dry_run` |
| `atlas_callers`, `atlas_callees` | who calls this, what it calls, grouped by repo | `target`, `depth` (1 to 6, default 1), `via_http` (follow HTTP calls across repos) |
| `atlas_impact` | blast radius of changing one or more targets: dependents, endpoints, cross-repo consumers, tests, config, each with the explaining path | `targets`, `depth` (default 3) |
| `atlas_tests_for` | tests relevant to a target (direct, via call graph, via file coverage heuristics) with run commands; when no test reaches a method, a separate hedged method-name fallback lists tests that call that name on an untyped value (not proof of coverage) | `target` |
| `atlas_api` | HTTP endpoints with handlers, consumers across repos, auth supported vs used | `repo`, `endpoint` |
| `atlas_similar` | near-duplicates of a symbol, of a line range or of unsaved draft code, with consumers, tests and a likely shared home | exactly one of `target`, `snippet` (`[repo:]path:START-END`), `code` (with `language`: `rust`, `ts`, `tsx`, `js`) |
| `atlas_unused` | candidate unused functions, endpoints, auth modes, events, config, with evidence | `kind`, `repo` |
| `atlas_patterns` | how this code base does something (exemplars ranked by prevalence) | `topic` |
| `atlas_explain` | provenance of a symbol or relation: certainty, evidence spans, producers, staleness | `target` |
| `atlas_overview` | project map: repos, packages, applications, entrypoints, key modules, APIs, events, patterns | none |
| `atlas_status` | index health: freshness, parse failures, SCIP coverage, LLM status, language servers, server telemetry; with `since_run`, what index runs newer than that run changed (files analysed, deleted, moved, renamed symbols) | `since_run` |

Targets accepted everywhere: a name (`UsersService.findOne`), `[repo:]path[:line]`, an endpoint (`GET /users/{id}`), or a full key; a bare file name (`price-rules.ts`) resolves when exactly one indexed file in scope has it, and otherwise the hint lists up to 3 of them. `atlas_symbol` and `atlas_edit` also take name paths (`UsersService/findOne`, `b[1]`) and `[repo:]path:LINE:COL` positions (1-based line, UTF-8 byte column). A name that is not found or is ambiguous returns candidates or a pointer to `atlas_search`. There is no reindex tool: freshness is automatic, and `atlas_status` with `since_run` confirms what the index saw after an edit.

`atlas_context` states a line range on every item: the symbol span for MUST READ, SKIM and TESTS items (`path:31-35`), and for a one-line mention such as an IMPACT caller the line of the reference that put it in the pack (`path:303 (site)`). An item whose body is not shown (SKIM, and MUST READ items marked "named in the task: read the file directly") also shows up to 5 lines around its key site when the budget allows: the reference site, or the body line that matches the most task or search terms. The pack expands up to three graph hops (calls, references, members, implementations) from its seeds; the enclosing type of every code item, the handler and guards of an endpoint and the guards of an auth scheme come with it without counting as a hop. For an endpoint the task names (or a seed reaches), the pack also follows every decorator on the handler, its parameters and its class into its implementation and adds the functions elsewhere that use the same constant, function or type, usually the framework code that registers the routes, builds their invokers and wraps their replies (`shares ROUTE_METADATA with the route decorator Get of FeedController.sitemap`, `shares INPUT_SLOTS with the parameter decorator Reply of FeedController.feedExport`); those found through the route decorator weigh more than the handler's direct callees, those found through any other decorator less; a function that calls two of the strongest search hits comes in as the code that applies them. A callee shared by many production callers (a route decorator, a logging helper) is scored down by its fan-in, and so is everything reached only through it, and once the named route exists, up to five other routes of its resource or shape are listed under PATTERN instead of seeding the pack. The CLI's `--depth` (0 to 3) narrows the expansion. `known` lists files or symbols the agent already holds: they seed the pack like `seeds` (a file seeds its top-level symbols), but nothing inside them is listed in MUST READ or SKIM; OMITTED says how many relevant items each one held back. OMITTED prints one count per reason on its title line and lists only seed and known diagnostics and each `known` entry with its `(k pack items)` count (`explain` lists every item), and the `~N of B tokens` header is the rendered text pack's characters / 4, which every section, gutter and item line counts against; the `explain` sections are not counted against the budget, so with `explain` the header can exceed it. A `known` entry that matches nothing (`[unknown known]`) or several entities (`[ambiguous known]`, with the candidates) is reported in OMITTED and seeds nothing.

`atlas_read` and `atlas_changes` were removed in this release (16 tools to 15). Calling either returns an error naming the replacement (`atlas_symbol mode=source`, `atlas_status since_run`). `atlas_dependents` and `atlas_dependencies` exist in the CLI (`query dependents`, `query dependencies`) but are not exposed over MCP. The CLI has top-level shortcuts for `search`, `impact`, `context`, `callers`, `callees`, `symbol`, `api`, `explain`, `read` and `tests-for`, and its own `find`, `outline`, `refs`, `decl`, `impls`, `hierarchy`, `diagnostics`, `edit` and `lsp check` ([operations](operations.md#cli-reference)).

### Tool profiles

`[mcp] tools` in `config.toml` (or `CODE_ATLAS_MCP_TOOLS`, or `catl mcp --tools`, which wins) chooses which tools the server lists:

| Profile | Tools |
|---|---|
| `full` (default) | all 15 |
| `core` | `atlas_context`, `atlas_search`, `atlas_symbol`, `atlas_impact`, `atlas_edit`, `atlas_status` |
| `read-only` | `core` without `atlas_edit` |
| a list, e.g. `["atlas_search", "atlas_status"]` (`--tools atlas_search,atlas_status`) | exactly those |

An invalid value from any layer (a typo such as `CODE_ATLAS_MCP_TOOLS=readonly`, an unknown tool name) never falls back to a wider set: `catl mcp` exits 2 naming the layer it came from (`flag:--tools`, `env:CODE_ATLAS_MCP_TOOLS`, `config.toml`), the profiles and the known tools, and `catl doctor` reports `FAIL  mcp tools`. A tool outside the profile is absent from `tools/list`; calling it anyway returns the tool error `<tool> is not in profile <profile>; enable with [mcp] tools`, and a removed tool whose replacement is outside the profile says so without naming it. The server instructions are rendered for the profile and name only its tools, as do the guidance block and the permission rules `agents sync` writes. `agents sync` reads the profile from `config.toml` only, not from `CODE_ATLAS_MCP_TOOLS` in the shell it runs in, because the agent starts the server with its own environment; rerun it after changing the profile (the server reads the setting at start, so the registered command does not change). No tool description names another tool, so a description is correct in every profile; the certainty tags are explained once, in the instructions, and the target syntax in the property descriptions of the tools that take a target.

What a profile costs, measured on the acme fixture with `atlas-eval agent surface --no-sessions --tools <profile>` (UTF-8 bytes of the `instructions` string and of the compact `tools/list` result; characters / 4 is a rough token estimate, not a tokenizer):

| Profile | `instructions` | `tools/list` | total | about tokens |
|---|---:|---:|---:|---:|
| `full` | 1,541 | 12,838 | 14,379 | 3,590 |
| `core` | 1,440 | 7,525 | 8,965 | 2,240 |
| `read-only` | 1,280 | 5,203 | 6,483 | 1,620 |

With Claude Code's tool search on, tool schemas are loaded only when searched, so a profile mainly shortens the instructions and the deferred tool names; it matters most for clients that load every tool upfront.

### `atlas_symbol` modes

Every argument not listed for a mode is rejected with an error that names what the mode takes.

| `mode` | Answer | Takes | Needs a language server |
|---|---|---|---|
| `info` (default) | definition, signature, docs, relation counts | `target`; `body` | no |
| `find` | name-path search (`UsersService/findOne`) | `target`; `path`, `depth`, `body`, `substring`, `kinds`, `exclude_kinds`, `max_matches` | no |
| `outline` | a file's symbols; from the index by default (header says `source=graph`, syntactic), `source=lsp` for the exact language-server outline | `path` or `target`; `depth`, `source` | only with `source=lsp` |
| `source` | code from disk (replaces `atlas_read`) | `target`; `context_lines` (0 to 100) | no |
| `references` | exact references | `target`; `source` | yes, unless `source=graph` |
| `declaration` | exact definition and declaration | `target` (a position), or `path` with `regex` (one capture group around the name) | yes |
| `implementations` | exact implementations of a trait, interface or method | `target` | yes |
| `hierarchy` | supertypes and subtypes | `target`; `depth` | no (graph) |
| `diagnostics` | compiler errors of a file | `path` or `target`; `lines` (`START-END`), `min_severity` | yes |

Exact modes come from rust-analyzer or typescript-language-server. With none available they fail with an install hint and never fall back silently; for `outline` (asked with `source="lsp"`) and `references` the error offers `source="graph"`, which returns graph answers with their usual tags and a caveat that graph references are not exhaustive. `outline` answers from the graph unless asked otherwise: when the graph has no facts for the file (not indexed, excluded, failed to parse) it says so and suggests `source="lsp"`, and a partially parsed file is marked `PARTIAL` in the header; it never falls back silently. Exact answers carry `[compiler]` and the server and version; they are valid for the configuration the server loaded and are never stored in the graph ([knowledge model](knowledge-model.md#certainty-classes)). An answer built from a partial view (server busy, a project that failed to load, diagnostics that did not settle) is marked `PARTIAL: ...` and is never presented as complete.

### `atlas_edit`

`operation` is one of:

- `rename`: exact, in every file the language server sees. Sites the server cannot see (other repos, mocks, HTTP consumers, unresolved or heuristic graph sites) are listed as `not renamed, verify`. Same-repo sites the index resolves to the symbol but the server did not rename are `UNVERIFIED:`; a project the server could not load is `UNLOADED:`. Apply refuses while either is present unless `allow_unverified` is set (check the sites first). `cross_repo=apply` edits only authoritative graph sites whose text checks out on disk; the default is to report them.
- `replace_body`, `insert_before`, `insert_after`: edit a symbol's definition from graph spans bound to the indexed content hash. They need no language server and refuse on an overload set rather than edit one signature. Restate all decorators or attributes of a group, or none: restating one replaces the group.
- `safe_delete`: needs a server; refuses and lists every reference.
- `replace_content` (one file) and `replace_in_files`: literal or regex text edits. A regex is multi-line and `.` matches newlines; use `.*?` or `[^\n]*` for single-line matches. `replace_in_files` without `occurrences` or `expected_count` is always a dry run.
- `apply`: applies a plan by `plan` id.

`dry_run=true` returns the diffs and a plan id; a plan is kept in the server for 15 minutes (32 plans). Writes are atomic with rollback, refused when a file changed since planning, and followed by an index refresh. The outcome lists the files changed and the new native diagnostics in the edited files (checked against a baseline; rust-analyzer reports native errors only, not `cargo check`). A refused, rolled back or partially applied edit is returned as a tool error (`FAILED:` for rolled back, `DEGRADED:` for partially applied, which lists the files still changed). **Re-read any file `atlas_edit` changed before you edit it yourself.**

## Recommended workflow

1. **Orient** in an unfamiliar project with `atlas_overview`.
2. **Start a task** with `atlas_context`. Read what it lists, not the whole repo. Items marked "named in the task: read the file directly" show only an outline and the lines around the key site; for a rename or symbol rewrite use `atlas_edit`.
3. **Before writing new code**, run `atlas_search` and `atlas_similar` to find something to reuse. A hit is a candidate, not proof of equivalence.
4. **Before changing shared code**, run `atlas_impact` on it, and `atlas_tests_for` to choose tests. Treat `UNCERTAIN` entries as work to verify, not as noise.
5. **Edit.** To rename a symbol or rewrite its definition, use `atlas_edit` (exact across files), not find-and-replace; try `dry_run` first on anything wide. For other edits, read the file with your editor's read tool (the harness may require it before an edit). Use `atlas_symbol mode=source` only for code you will not edit yourself. After `atlas_edit` changes a file, re-read it before editing it yourself.
6. **When a relation drives a decision**, run `atlas_explain` on it.
7. **After editing**, re-query or call `atlas_status` with `since_run`. Read the diagnostics in the `atlas_edit` outcome: they cover native language-server errors in the edited files only, not the whole project and not `cargo check`.
8. **Verify before relying on** any `[heuristic]` or `[inferred]` relation, any "none found", and any answer that starts with `DEGRADED`.

## Reading certainty tags

| You see | Meaning | What to do |
|---|---|---|
| `[compiler]`, `[syntactic]`, `[resolved]`, `[declared]`, or no tag on an entity | exact under the class's rules, not exhaustive | rely on it; still read the caveats |
| `[compiler]` on an `atlas_symbol` or `atlas_edit` answer | answered live by a language server (`source = lsp:<server> <version>`), valid for the configuration the server loaded (features, target, loaded projects); never stored in the graph | rely on it unless it says `PARTIAL` or lists sites outside the server's view |
| `PARTIAL: ...` | the server's view was incomplete (busy, a project failed to load, diagnostics did not settle) | treat as a lower bound |
| `outside the language server's view`, `not renamed, verify`, `UNVERIFIED:`, `UNLOADED:` | graph sites the server did not return or rename | check them before relying on or applying a rename |
| `Name kind path [heuristic 0.80]` | the **entity** is uncertain (a tag after the entity name) | verify the entity exists as described |
| `X [entity tag] ([relation tag])`, `verb target [tag]` | entity and relation certainty are separate: parentheses, or a tag after the target of a `verb target` step, tag the relation | read both |
| `possibly calls ... [heuristic 0.72]` | a rule with a confidence; may be wrong | open the source |
| `likely ... [inferred 0.81]` | LLM-derived with rationale (for example a client call linked to an endpoint by `enrich`, shown as a tagged consumer in `api`, `impact`, `unused`, `explain`) | verify; check `stale` markers |
| `[inferred 0.81 stale]`, `[... needs-revalidation]` | evidence changed since the LLM looked | treat as unknown |
| `UNCERTAIN` on an impact entry | every path to it passes through a non-authoritative edge | verify |
| `caveats:` block | unresolved references, dynamic member calls, partial parses, hidden low-confidence relations, skipped files | decides whether "none found" means anything |
| first line `DEGRADED: ...` | index could not be fully refreshed; affected files listed | verify against files |
| header `skipped N files (see status)` | files over the size limit, symlinks or non-UTF-8 paths are not indexed | check `atlas_status` |

Compiler-verified relations render as `[compiler]` everywhere in text output (tool descriptions use the same spelling); the JSON field `certainty.class` is `compiler_verified` and `certainty.authoritative` is a boolean. Two sources produce it: SCIP, ingested into the graph, and live language-server answers, which are not stored. In JSON, every relation carries `certainty` (`class`, `confidence`, `freshness`, `authoritative`, `tag`) and every response carries a `freshness` object. Rules of thumb: an empty result is not proof of absence; "unused" means "no inbound edges found"; a similarity match is a lead.

## Wiring agents: `agents sync`

```sh
catl agents sync                         # what config.toml [agents] asks for (default: Claude Code user scope, plus Codex if ~/.codex exists)
catl agents sync --agent claude --scope local --repo ../api --repo ../web
catl agents sync --dry-run               # show the diffs, write nothing
catl agents status                       # what was written and whether it changed since (exit 3 on drift)
catl agents unsync                       # remove exactly what was recorded
catl agents hooks --json                 # the hook groups sync would write, by event; writes nothing
catl connect claude [--scope project|user] [-y]   # the earlier per-repo command, unchanged (below)
```

**Changed in this version.** `agents sync` is the managed way to wire agents: one cwd-routed user-scope server with no `--project`, permission rules and hooks, a record of what it wrote, drift detection and `unsync`. `connect claude` keeps its earlier behaviour for scripts that call it: project scope by default (`.mcp.json` with `catl mcp --project <p> --watch`, the bare `catl` when `catl` on `PATH` is this binary, otherwise its absolute path with a warning), `--scope user` registers this binary's absolute path with `mcp --watch`, and both write the guidance block into `CLAUDE.md` or `AGENTS.md`. It writes no permission rules or hooks, never removes other entries, and replaces a different existing entry only after showing the diff and asking (`-y`). It now also records what it wrote in `wiring.json`, so `agents status` and `agents unsync` cover it, and `agents sync --migrate` later replaces its entries with the cwd-routed form.

`agents sync` writes one server entry per agent, with the command `catl` (or `[agents] binary`) and the arguments `mcp --watch`. No entry pins a project: the server binds the project of the directory the client starts it in (marker, then registered repo path), so one entry serves every registered repo, and `--watch` keeps answers fresh between calls and runs automatic SCIP when it is on. A `catl` that is not on `PATH`, or is another binary, gets a warning.

| Agent and scope | What is written | Committed? |
|---|---|---|
| Claude Code `user` (default) | `claude mcp add-json --scope user code-atlas {"type":"stdio","command":"catl","args":["mcp","--watch"]}`, plus `env.CODE_ATLAS_HOME` when it is set | no |
| Claude Code `local` | the same through `claude mcp add-json --scope local`, run in each repo root (Claude Code keeps it in `~/.claude.json` under that path) | no |
| Claude Code `project` | `.mcp.json` at each repo root with the bare `catl`, never `CODE_ATLAS_HOME`; only when asked for, with a warning that the file is meant to be committed | yes |
| Codex | `[mcp_servers.code-atlas]` (`command`, `args`, `env`) in `$CODEX_HOME/config.toml` (default `~/.codex/config.toml`), edited in place so comments, key order and other servers stay | no |

- **Repos.** Local and project scope wire `--repo` (repeatable) or, without it, the repo of the working directory; in a git worktree of a registered repo that is the worktree. User scope and Codex need no repo; guidance (below) goes into the same repos.
- **The `claude` CLI** registers user and local entries (the format of `~/.claude.json` is Claude Code's own). Without it, `sync` exits 2, writes nothing and prints the command to run yourself. `CLAUDE_CONFIG_DIR` moves `.claude.json` and `settings.json` the way it does for Claude Code.
- **Every registration is read back.** `claude mcp add-json` is run after `claude mcp remove`; if adding fails, `sync` re-adds the entry that was there before. After the CLI reports success, `sync` reads `.claude.json` again and fails loudly, recording nothing, if the entry is not where it looks: `mcpServers.code-atlas` for user scope, `projects["<repo root>"].mcpServers.code-atlas` for local scope. That second key is what the test double writes (the physical path of the directory `claude` runs in, which is the repo root); the key the real `claude` CLI uses for a repo root that is not a git root was not verified, which is why the read-back exists. When an entry was lost (removed, the re-add failed too), its record is dropped, so the next `sync` creates it again instead of reporting drift.
- **Tests cannot reach your configuration.** When `CARGO_MANIFEST_DIR` is set (inside `cargo test` or `cargo run`), `sync`, `unsync` and `connect` refuse (exit 2) unless `CLAUDE_CONFIG_DIR` and `CODEX_HOME` are set as well.
- **Codex** is wired when `$CODEX_HOME` (or `~/.codex`) exists and `[agents.codex] enabled` is true, and skipped with a note otherwise; `--agent codex` without it exits 2. Verified against the Codex MCP documentation (2026-10): a stdio server is a `[mcp_servers.<name>]` table with `command`, `args`, `env` (plus optional `cwd`, timeouts and tool lists, which `sync` leaves alone), the user file is `~/.codex/config.toml` and `CODEX_HOME` moves it. The documentation does not say which directory Codex starts a server in; the entry sets no `cwd`, so the project is resolved from Codex's working directory. If yours starts servers elsewhere, add `"--project", "<name>"` by hand (then `status` reports it as changed).

**What it records.** `<data dir>/wiring.json` lists every entry `sync` wrote: agent, scope, repo, where (`~/.claude.json` registration, `.mcp.json` member, settings file permission rules and hooks, Codex table, guidance file, git exclude line) and a BLAKE3 hash of the part it owns. The file is version 2 since the `seed` hook (a hooks record now names its event); this `catl` still reads version 1 and writes version 2 on its next change, and an older `catl` refuses version 2 rather than misread it. Only that part is compared, so other servers, hooks or settings in the same file never count as a change. With that record:

- `sync` changes its own unchanged entries without asking, leaves identical ones alone, and refuses (exit 2, nothing written) to overwrite an entry that was changed or removed since it wrote it, or one it did not write, unless `--force`. Adding to a file that already existed asks first (`--yes` skips the question). A failure midway leaves the record matching what was applied, so a rerun picks up where it stopped.
- `sync` also removes what it recorded earlier for the same agent, scope and repos and no longer wants (a hook turned off, guidance switched to `none`, a tracked block `connect` wrote once guidance is `local`), and adds what it wants and did not record yet: rerunning `sync` after an upgrade that added the `seed` hook writes its `UserPromptSubmit` group next to the recorded `PreToolUse` and permission entries and leaves those and every foreign hook alone. A fragment added to a settings file that `sync` created counts as part of that file, so `unsync` still deletes the file once nothing else is left in it.
- `status` lists each recorded entry as `ok`, `changed`, `removed` or `unreadable`.
- `unsync [--agent A] [--scope S] [--repo PATH] [--dry-run] [--force]` removes exactly the recorded entries, refuses changed ones without `--force`, and deletes a file only if `sync` created it and nothing else is left in it.

**Migrating from older wiring.** Entries from an older `catl` have an absolute command or a fixed `--project` (the last connected repo then won everywhere). `sync --migrate` replaces them after showing the diff; with user scope it also offers to remove such local-scope entries in `~/.claude.json`, which would otherwise win over the user-scope server in those repos. Without `--migrate` `sync` refuses to replace them and lists them. A `.mcp.json` with a `code-atlas` server also wins over the user scope; `sync` says so but leaves the committed file alone.

### Permission rules and hooks (Claude Code)

`[agents.claude] hooks` (default `["auto-approve", "seed"]`; `[]` writes none) goes into the settings file of the scope: `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR/settings.json`) for user scope, `.claude/settings.local.json` for local and project scope. Project scope never writes the committed `.claude/settings.json`, because entries there would apply to every teammate. A local settings file is excluded from git like local guidance. Other rules and hooks in the file are kept.

- `auto-approve` is not a hook: it writes the read-only tools of the configured [profile](#tool-profiles) (all 14 with `full`) as exact `permissions.allow` rules (`"mcp__code-atlas__atlas_api"`, ..., `"mcp__code-atlas__atlas_unused"`), never `atlas_edit`. Claude Code then skips the prompt for them without running any process, so a missing or mismatched `catl` cannot block a tool call. Claude Code still applies your deny and ask rules. Every rule naming the `code-atlas` server (`mcp__code-atlas`, `mcp__code-atlas__*`) counts as the fragment `sync` owns, so a rule you add or remove there shows as drift, and `sync` rewrites the list when the tool set changes.
- `remind` is a `PreToolUse` hook on `Grep|Glob|Bash`:

  ```json
  { "matcher": "Grep|Glob|Bash", "hooks": [{ "type": "command", "command": "CODE_ATLAS_HOME=/iso catl hook remind || true", "timeout": 10 }] }
  ```

  (`CODE_ATLAS_HOME=...` only when it is set while syncing.) It answers a `Grep` or `Glob` call, or a `Bash` command that runs `grep`, `rg`, `ag`, `ack` or `git grep`, in a directory of a registered project with `{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"..."}}`: a note that `atlas_search`, `atlas_symbol` or `atlas_context` may answer faster. It makes no permission decision, so the tool runs as usual, and it fires at most once per `session_id` (state in `<data dir>/hooks/reminded/`, entries older than 7 days are dropped).

- `seed` is a `UserPromptSubmit` hook (that event takes no matcher), on by default:

  ```json
  { "hooks": [{ "type": "command", "command": "CODE_ATLAS_HOME=/iso catl hook seed || true", "timeout": 10 }] }
  ```

  It hands the agent the graph's answer before it starts searching: from the full prompt it builds a context pack and adds a short list from it as `{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"..."}}`, which Claude Code puts next to the prompt. When `seed` comes from the built-in default rather than your own `[agents.claude] hooks` list, the `sync` that first writes it says so. To turn it off, set `[agents.claude] hooks = ["auto-approve"]` (or your list without `"seed"`) and rerun `agents sync`, which removes the group it wrote. The context reads:

  ```text
  Code Atlas seed: a short ranked list of the indexed code most relevant to this prompt, from an index that is up to date with the working tree. Read what it names before searching; [heuristic n], [inferred n] and UNCERTAIN mark leads to verify in the source.
  Read first:
  - parse_order function shop:src/lib.rs:5-7  why: named in task as parse_order
  - Order struct shop:src/lib.rs:1-3  why: instantiated by parse_order [resolved]
  - total_cents function shop:src/lib.rs:9-11  why: called by parse_order [resolved]
  More from the code-atlas MCP server: atlas_context (a fuller pack with bodies), atlas_symbol (definitions, references and source), atlas_impact (what a change affects).
  ```

  The pack is the one `atlas_context` builds for the prompt with its default budget (6,000 tokens). The seed lists its MUST READ (`Read first:`), SKIM (`Also relevant:`) and TESTS (`Tests:`) items in rank order, each with its `repo:path:line-range` and why it is there, the way `atlas_context` prints them but without bodies. The list stops at `[hooks] seed_budget` tokens (default 1,200, 200 to 2,000, characters / 4, never more than 9,000 characters, under Claude Code's 10,000-character cap). The bodies of MUST READ items are still read, because their size decides which items are MUST READ; skipping them would make the seed differ from the pack `atlas_context` returns. The last line names only the tools of the configured [profile](#tool-profiles) (resolved from `config.toml`, like `sync`), and is left out when the profile has none of them.

  On a probe on this repository (11 code and 12 chatter prompts from the review, 10 written beforehand for this check, each sent as the first prompt of a session and as a follow-up, 54 cases), it seeded 10 of the 27 code cases and none of the 27 chatter cases: every plain-English follow-up stays silent by design, and the plain first prompts about features this repository does not have (`migrate session storage to redis`) found nothing related.

  The hook never runs an index, never creates a database, and never prepares or collects a worktree overlay. Opening the engine in check-only mode can still write small atomic bookkeeping: restamped file mtimes, adoption of the project's configuration, recovery ledger rows and the similarity blob when the engine drops. Each is a single store transaction or an atomic file replace, and none of it is a fact about the code. It prints nothing when:

  - the event is not `UserPromptSubmit` or lacks `prompt` or an absolute `cwd`, or the input is not JSON;
  - the prompt fails the prompt rule, or is a plain-English follow-up. Lines that are paste markers (`<pasted_content id="…">`, `</pasted_content id="…">`) are dropped first, and a prompt needs at least 16 characters. A **code prompt** has a code-like token: a backtick span, `snake_case`, `camelCase`, `a.b` with parts of two or more characters, `a::b`, `a->b`, a call `name(`, or a path (`a/b/c`, `src/x.rs`); it may seed on any turn. A **plain-English prompt** has none but at least 3 distinct content words (3 or more letters, identifiers split at `_` and case changes, not on a list of about 100 function and chatter words such as `the`, `you`, `thanks`, `please`, `continue`, `commit`, `push`, `next`, `step`); it seeds only when it is the first prompt of its session to qualify. The session is the event's `session_id`, recorded as a marker in `<data dir>/hooks/seeded/` the way `remind` records its own (entries older than 7 days are dropped); an event without one counts as a follow-up. The marker is created only when a seed is about to be printed, with an exclusive create, so a first prompt that stayed silent (stale index, time limit, unrelated pack) leaves the next plain-English prompt of the session eligible, and of two hooks racing on the same session only the one that creates the marker prints. A seeded code prompt creates the marker too (if it is missing), so a plain-English prompt after it is a follow-up. So in an ongoing session `that worked, nice job. anything else left to do?` never seeds, while `why does total_cents round down?` does;
  - the working directory is in no registered project, or is answered from another checkout (a git worktree without an overlay, which `status` marks with a worktree caveat), or its worktree overlay has not been seeded yet (any CLI command or the MCP server started in the worktree seeds it);
  - the project has never been indexed;
  - the index is not up to date with the working tree. The engine is opened with `BarrierMode::CheckOnly`: its barrier stat-scans the repos and answers only when no index run is needed; a changed, added or deleted file, pending work (re-analysis after a configuration or version change, crash recovery, a due parse-budget retry) or a missing repo makes it report the index as not fresh without running anything. With the MCP server's watcher running (`agents sync` always writes `catl mcp --watch`), edits are indexed within about a second, so the next prompt usually finds a clean tree; without it, the next CLI command or MCP call indexes them;
  - the index changed while the pack was built: the query re-read a source file that changed since indexing (a note on the answer), or the last finished index run is no longer the one the barrier saw;
  - an index run started while the pack was built (the `engine:dirty` marker is present when the answer is ready);
  - the pack is empty, or unrelated: no MUST READ or SKIM item is named in the prompt, and none of the top 3 of them is a search hit on at least 2 distinct content words of the prompt. For a code prompt, a top-3 item also passes when a specific token of the prompt names it. A token (a backtick span or a code-like word, backticks and a trailing `()` ignored) is specific when it is a bare identifier with an underscore between letters (`persist_similarity`, `REFRESH_DEFERRED`) or an interior capital (`flushPending`, `UsersService`), and then its lowercase form must equal the item's short name (the last path segment, `Engine::persist_similarity` gives `persist_similarity`); or when it is qualified (`Cache::flush`, `Cache.flush`, `store::Cache::flush`) and its last segment equals the item's and its qualifier agrees with the item's (the last segment may be short then). A single plain word never qualifies, whatever its length: `use `map` instead`, `call `run()` again`, `rename it to `main``, `remove the `fmt` step` and `should it be `default()` instead` stay silent even though items with those names exist. Context marks a bare name that resolves to several symbols (`persist_similarity` is a method of both `Engine` and `Session`) or an all-capitals constant (`REFRESH_DEFERRED`) as a search hit, not as named, so the word gate alone left `what does `persist_similarity` return when nothing changed?` silent. An item below the top 3 never passes this way (`where is `REFRESH_DEFERRED` checked?` ranks the constant eighth on this repository and stays silent). A plain-English prompt never passes it. On this repository, `hmm, try a different approach then` finds `HelloFile::Different` on the single word `different` and stays silent. An earlier version gated on the top PageRank score (at least 0.08); it was dropped, because scores are not calibrated across prompts (the top score of chatter prompts ranged 0.07 to 0.18 here and of code prompts 0.05 to 0.19), so it silenced nothing on a real repository;
  - the time limit passes, or anything fails or panics.

  **Time limit.** The hook answers within 3 s or prints nothing, far under the 10 s `timeout` it is written with (Claude Code's default for this event is 30 s; a timed-out hook's output is discarded and the prompt goes ahead). Binding, opening the engine, the barrier and the query run on a worker thread; the hook waits for it until the limit and then exits, so the prompt is never held longer. Waits for the index lock (when another process's index run is in flight) and SQLite's busy timeout are capped at the time left minus 250 ms, so such a wait ends with a logged reason before the limit. Because the hook never runs an index, an exit at the limit leaves at most the atomic bookkeeping above, never a half-written index.

  **Scaling.** Most of the time goes into loading the engine state when the engine opens, which grows with the number of files: about 0.45 s at 900 files and 1.2 s at 6,200 files (release build), so a project of more than roughly 15,000 to 20,000 files cannot seed within 3 s and the hook stays silent there ("the time limit passed" in its log). A missing or outdated similarity index is rebuilt at open, which adds to that once after an upgrade. A seed that does not come is expected behaviour, not an error: the agent still has the MCP tools.

  The hook never logs or stores the prompt: with `--log-level` or `RUST_LOG` set it logs one line per prompt with the prompt's length in characters, the elapsed milliseconds and the outcome (`seeded the prompt` with the context size, or `no seed` with the reason); without either it writes nothing to stderr.

  Measured with a release build (2026-10, Apple silicon; medians of 10 runs, or of 5 on the large corpus):

  | Case | Hook wall time | Outcome |
  |---|---:|---|
  | code-atlas repository, 886 files, clean tree | 0.52–0.57 s (0.44 s of it loading the engine state) | seeded |
  | fresh clone of a 100,000-line invented corpus (`atlas-bench gen --loc 100000 --repos 2`, 573 files), before `catl index` | 4 ms | silent: never indexed |
  | the same, right after `catl index`, prompt naming a symbol | 0.17–0.18 s | seeded |
  | the same, prompt naming no existing symbol (`fix parseAccount so renderLedger shows the refunded total`) | 0.15 s | seeded: its top items are search hits on `render` and `ledger` |
  | the same, 1 file changed | 0.20 s | silent: not fresh, no run |
  | 1,000,000-line TypeScript corpus (`--lang ts --loc 1000000 --repos 2`, 6,202 files), clean tree | 1.3 s | seeded |
  | the same, 10 or 50 files changed | 1.3–1.4 s | silent: not fresh, no run, no dirty marker; the next `catl status` indexes them |
  | the same, `package.json` changed (pending re-resolve of every file) | 1.4 s | silent: pending work |
  | the same with `catl watch` running, 10 files changed, prompt at once / 1 s / 2 s later | 3.0 s / 1.9 s / 1.35 s | silent at the limit (waited for the watcher's run, then reloaded) / seeded / seeded |


  `catl agents hooks [--binary CMD] [--home DIR] [--json]` prints the groups `sync` would write for the hooks enabled in `config.toml`, keyed by event (`{"PreToolUse": [...], "UserPromptSubmit": [...]}`), so a harness can merge them into its own settings file; it writes nothing.

**Exit status.** Claude Code treats a hook that exits 2 as a block. `catl hook ...` exits 0 whatever happens inside it: an unknown hook name or flag, input that is not the expected JSON, an unreadable home or registry, an error while answering or a panic all end with exit 0 and no output. Only a `catl` that cannot run at all (missing from `PATH`, killed) fails, and the written command ends in `|| true` so the shell turns that into exit 0 too. An older `catl` that predates `catl hook` is covered by that `|| true` as well, not by `catl` itself: it rejects the unknown subcommand with a usage error (exit 2).

Contract verified against the Claude Code hooks reference (code.claude.com/docs/en/hooks, 2026-10) and permissions reference (code.claude.com/docs/en/permissions): input fields `session_id`, `cwd`, `hook_event_name`, `tool_name`, `tool_input`; `additionalContext` is added next to the tool result and leaves the permission flow unchanged; exit 0 with JSON is the structured answer and exit 2 blocks the call; `mcp__<server>__<tool>` names one MCP tool in a permission rule. For `UserPromptSubmit`: the input adds `prompt` (pasted text expanded in place); the event takes no matcher (one given is ignored); `hookSpecificOutput.additionalContext` is added to Claude's context next to the prompt, capped at 10,000 characters (longer output is saved to a file and replaced by a preview); exit 2 blocks the prompt; the default command timeout is 30 s and a timed-out hook's output is discarded while the prompt still reaches Claude. The hook also runs for turns Claude Code starts itself (scheduled tasks, `/loop`, a background subagent reporting back), which the same rules cover.

### Guidance

Claude Code passes the server's `instructions` to the model, so a repo file repeating them is redundant by default. `[agents] guidance` decides whether `sync` writes the block anyway:

| `guidance` | Effect |
|---|---|
| `auto` (default) | nothing for now: Claude Code reads the server instructions; whether Codex shows them to the model is not documented, so set `tracked` for Codex if yours does not |
| `none` | nothing |
| `local` | the block in `CLAUDE.local.md` at the repo root, excluded from git through `info/exclude` of the git common dir, so it works in worktrees and never shows in `git status`. Claude Code only; refused (unless `--force`) in a repo that has an `AGENTS.md` and no `CLAUDE.md`, because a `CLAUDE.local.md` makes Claude Code stop reading `AGENTS.md` by default |
| `tracked` | the block in `CLAUDE.md` or `AGENTS.md` (the first that has it, else the first that exists, else a new `AGENTS.md`), which is usually committed |

The block is a rendering of the server instructions for the configured [profile](#tool-profiles), from one source (`crates/atlas-mcp/src/guidance.rs`); a test checks that this page shows exactly what it renders for `full` (`core` and `read-only` drop the rules and clauses about tools they do not serve):

```markdown
<!-- code-atlas:begin -->
## Code Atlas

This repo is indexed by Code Atlas (project `<project>`), served by the `code-atlas` MCP server. Prefer it to grep when a question spans files or repos.

Code Atlas answers questions about ONE indexed project (a group of repos) from a knowledge graph kept up to date with the working tree.
1. Start a task with atlas_context: a ranked pack of the code to read. A Code Atlas seed in your context is a short list from that pack: read what it names first, and call atlas_context for bodies and more.
2. Before writing new code, run atlas_search and atlas_similar to find code to reuse.
3. Before changing shared code, run atlas_impact on it; use atlas_tests_for to pick tests.
4. Read only the code the results list (atlas_symbol mode=source only for code you will not edit yourself; read files you will edit with your own tool). To rename or rewrite a symbol use atlas_edit (exact across files), not find-and-replace; after it changes a file, re-read that file before editing it yourself.
5. Untagged entities and [resolved]/[syntactic]/[compiler]/[declared] relations are reliable but not exhaustive: read the caveats, and an empty result is not proof of absence. [heuristic] and [inferred] relations are leads: verify them in the source before relying on them. A tag after an entity name tags the entity; one in parentheses or in a `verb target [tag]` step tags the relation.
6. An answer starting with DEGRADED came from an index that could not be fully refreshed: verify it against the files. atlas_explain shows the evidence of a relation.
7. After editing, call atlas_status with since_run or re-query; there is no reindex tool.
8. If a target is not found or is ambiguous, use atlas_search.
<!-- code-atlas:end -->
```

The eval's `standard` and `directed` preambles (`atlas-eval`) are versioned experimental conditions and are deliberately not rendered from this source; changing them is a new preamble version.

The project must exist first (`project create`, or `init` inside a repo that should join an existing project). In a directory that no registered project contains the server still starts but serves only `atlas_status`, which explains how to register the repo. In a git worktree of a registered repo the server answers from a worktree overlay, an index of the worktree seeded from the project's, and `atlas_edit` writes the worktree; start the agent session inside the worktree (see [operations](operations.md#worktrees)). `catl doctor` checks the registry, databases, locks, tool availability and a user-scope entry that still pins `--project`.

## Other agents

The MCP server is vendor-neutral stdio, protocol-compliant with any client that can launch a command:

```json
{ "mcpServers": { "code-atlas": { "command": "/abs/path/catl", "args": ["mcp", "--watch"] } } }
```

Without `--project` the server binds the project of the directory the client starts it in; add `"--project", "acme"` when the client does not start servers in the repo. Add `"env": { "CODE_ATLAS_HOME": "..." }` if the registry is not in the default location. `--watch` keeps the index fresh between calls and enables automatic SCIP (without it each call refreshes the index on demand). If the client does not show MCP server instructions to the model, put the guidance block above into whatever instruction file your agent reads (`AGENTS.md`, rules files, a system prompt). Because `atlas_edit` writes files, clients that gate tools by annotation will treat it as destructive; approve it deliberately. The MCP server keeps language servers warm (rust-analyzer can hold about 2 GB; see [operations](operations.md#language-servers)).

Agents without MCP can shell out. All query commands accept `--json` and exit non-zero as documented in [operations](operations.md#exit-codes); edit commands use exit codes 5 to 7 for refused, rolled-back and partially applied edits:

```sh
catl -p acme impact UsersService.findOne --json
catl -p acme query tests-for UsersService.findOne --json
catl -p acme query similar --snippet src/money.ts:1-7 --json   # path relative to cwd, or repo:path:1-7
catl -p acme refs UsersService.findOne --json                  # exact, from the language server
catl -p acme edit rename UsersService.findOne findUser --dry-run --plan-out plan.json
```

JSON responses are `{ "freshness": {...}, "answer": {...} }`. Check `freshness.state` and exit code 3 before trusting `answer`, and read `answer.caveats`.

## Tips from the smoke run

From the harness smoke run (2026-10-04, `t00-smoke`, one run per condition; see [dogfooding](evaluation/dogfooding.md)):

- An agent called `atlas_read` on a file and then the native `Read` on the same file, because the editor requires a read before an edit. The context was duplicated. Tool descriptions and the context pack said: read files you will edit natively, use the symbol reader only for code you will not edit. `atlas_read` is now `atlas_symbol mode=source` with the same rule, and renames and symbol rewrites go through `atlas_edit` instead of the agent's own tools. This guidance has not been re-measured with agents since the surface changed.
- `atlas_context` bodies of items marked "named in the task: read the file directly" are suppressed for the same reason; they show an outline.
- The atlas run used 15 turns and about 5x the tokens of baseline (including cache reads) on a one-file task, while reading a relevant file the baseline run missed. The pipeline works end to end; cost and benefit on realistic tasks are measured by the A/B harness, not by this single run.
- Both runs failed the same hidden check (a column-name detail): model capability, not the index.
- The index cannot tell you about code it does not model. Read the caveats, then the files.
