# Agent integration

Code Atlas reaches agents two ways: an MCP stdio server (`catl mcp`) and the CLI (`catl ... --json`). Both call the same query layer, so the answers, tags and caveats are identical.

## MCP tools

The server serves one project (`--project`, or inferred from the working directory), the 15 tools below or a [profile](#tool-profiles) of them, and runs the freshness barrier before every call. It is no longer read-only: `atlas_edit` writes files (tool annotation `destructive_hint = true`); the other 14 tools are annotated read-only. Every tool takes an optional `format`: `"text"` (default, compact) or `"json"`. Output is capped at 100,000 bytes (a truncation note says how to narrow the query) and bodies at 400 lines. Server-side call telemetry (calls, errors, latency, bytes, truncations) is part of `atlas_status`.

| Tool | Use it to | Arguments |
|---|---|---|
| `atlas_context` | start a task: ranked, budgeted pack of code to read (full bodies for top items, signatures, one-liners), with the line range of each item, why it is included and what was omitted | `task`, `budget` (500 to 32000 tokens), `seeds`, `known` |
| `atlas_search` | find where a capability lives or whether similar code exists (names, text, structure) | `query`, `kind`, `repo`, `limit` (1 to 200) |
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

Targets accepted everywhere: a name (`UsersService.findOne`), `[repo:]path[:line]`, an endpoint (`GET /users/{id}`), or a full key. `atlas_symbol` and `atlas_edit` also take name paths (`UsersService/findOne`, `b[1]`) and `[repo:]path:LINE:COL` positions (1-based line, UTF-8 byte column). A name that is not found or is ambiguous returns candidates or a pointer to `atlas_search`. There is no reindex tool: freshness is automatic, and `atlas_status` with `since_run` confirms what the index saw after an edit.

`atlas_context` states a line range on every item: the symbol span for MUST READ, SKIM and TESTS items (`path:31-35`), and for a one-line mention such as an IMPACT caller the line of the reference that put it in the pack (`path:303 (site)`). An item whose body is not shown (SKIM, and MUST READ items marked "named in the task: read the file directly") also shows up to 5 lines around its key site when the budget allows: the reference site, or the body line that matches the most task or search terms. The pack expands up to three graph hops (calls, references, members, implementations) from its seeds; the enclosing type of every code item, the handler and guards of an endpoint and the guards of an auth scheme come with it without counting as a hop. The CLI's `--depth` (0 to 3) narrows the expansion. `known` lists files or symbols the agent already holds: they seed the pack like `seeds` (a file seeds its top-level symbols), but nothing inside them is listed in MUST READ or SKIM; OMITTED says how many relevant items each one held back. A `known` entry that matches nothing (`[unknown known]`) or several entities (`[ambiguous known]`, with the candidates) is reported in OMITTED and seeds nothing.

`atlas_read` and `atlas_changes` were removed in this release (16 tools to 15). Calling either returns an error naming the replacement (`atlas_symbol mode=source`, `atlas_status since_run`). `atlas_dependents` and `atlas_dependencies` exist in the CLI (`query dependents`, `query dependencies`) but are not exposed over MCP. The CLI has top-level shortcuts for `search`, `impact`, `context`, `callers`, `callees`, `symbol`, `api`, `explain`, `read` and `tests-for`, and its own `find`, `outline`, `refs`, `decl`, `impls`, `hierarchy`, `diagnostics`, `edit` and `lsp check` ([operations](operations.md#cli-reference)).

### Tool profiles

`[mcp] tools` in `config.toml` (or `CODE_ATLAS_MCP_TOOLS`, or `catl mcp --tools`, which wins) chooses which tools the server lists:

| Profile | Tools |
|---|---|
| `full` (default) | all 15 |
| `core` | `atlas_context`, `atlas_search`, `atlas_symbol`, `atlas_impact`, `atlas_edit`, `atlas_status` |
| `read-only` | `core` without `atlas_edit` |
| a list, e.g. `["atlas_search", "atlas_status"]` (`--tools atlas_search,atlas_status`) | exactly those |

An invalid value from any layer (a typo such as `CODE_ATLAS_MCP_TOOLS=readonly`, an unknown tool name) never falls back to a wider set: `catl mcp` exits 2 naming the layer it came from (`flag:--tools`, `env:CODE_ATLAS_MCP_TOOLS`, `config.toml`), the profiles and the known tools, and `catl doctor` reports `FAIL  mcp tools`. A tool outside the profile is absent from `tools/list`; calling it anyway returns the tool error `<tool> is not in profile <profile>; enable with [mcp] tools`, and a removed tool whose replacement is outside the profile says so without naming it. The server instructions are rendered for the profile and name only its tools, as do the guidance block and the permission rules `agents sync` writes. `agents sync` reads the profile from `config.toml` only, not from `CODE_ATLAS_MCP_TOOLS` in the shell it runs in, because the agent starts the server with its own environment; rerun it after changing the profile (the server reads the setting at start, so the registered command does not change). No tool description names another tool, so a description is correct in every profile; the certainty tags and target syntax are explained once, in the instructions.

What a profile costs, measured on the acme fixture with `atlas-eval agent surface --no-sessions --tools <profile>` (UTF-8 bytes of the `instructions` string and of the compact `tools/list` result; characters / 4 is a rough token estimate, not a tokenizer):

| Profile | `instructions` | `tools/list` | total | about tokens |
|---|---:|---:|---:|---:|
| `full` | 1,405 | 12,838 | 14,243 | 3,560 |
| `core` | 1,304 | 7,525 | 8,829 | 2,210 |
| `read-only` | 1,144 | 5,203 | 6,347 | 1,590 |

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

**What it records.** `<data dir>/wiring.json` lists every entry `sync` wrote: agent, scope, repo, where (`~/.claude.json` registration, `.mcp.json` member, settings file permission rules and hooks, Codex table, guidance file, git exclude line) and a BLAKE3 hash of the part it owns. Only that part is compared, so other servers, hooks or settings in the same file never count as a change. With that record:

- `sync` changes its own unchanged entries without asking, leaves identical ones alone, and refuses (exit 2, nothing written) to overwrite an entry that was changed or removed since it wrote it, or one it did not write, unless `--force`. Adding to a file that already existed asks first (`--yes` skips the question). A failure midway leaves the record matching what was applied, so a rerun picks up where it stopped.
- `sync` also removes what it recorded earlier for the same agent, scope and repos and no longer wants (a hook turned off, guidance switched to `none`, a tracked block `connect` wrote once guidance is `local`).
- `status` lists each recorded entry as `ok`, `changed`, `removed` or `unreadable`.
- `unsync [--agent A] [--scope S] [--repo PATH] [--dry-run] [--force]` removes exactly the recorded entries, refuses changed ones without `--force`, and deletes a file only if `sync` created it and nothing else is left in it.

**Migrating from older wiring.** Entries from an older `catl` have an absolute command or a fixed `--project` (the last connected repo then won everywhere). `sync --migrate` replaces them after showing the diff; with user scope it also offers to remove such local-scope entries in `~/.claude.json`, which would otherwise win over the user-scope server in those repos. Without `--migrate` `sync` refuses to replace them and lists them. A `.mcp.json` with a `code-atlas` server also wins over the user scope; `sync` says so but leaves the committed file alone.

### Permission rules and the remind hook (Claude Code)

`[agents.claude] hooks` (default `["auto-approve"]`; `[]` writes neither) goes into the settings file of the scope: `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR/settings.json`) for user scope, `.claude/settings.local.json` for local and project scope. Project scope never writes the committed `.claude/settings.json`, because entries there would apply to every teammate. A local settings file is excluded from git like local guidance. Other rules and hooks in the file are kept.

- `auto-approve` is not a hook: it writes the read-only tools of the configured [profile](#tool-profiles) (all 14 with `full`) as exact `permissions.allow` rules (`"mcp__code-atlas__atlas_api"`, ..., `"mcp__code-atlas__atlas_unused"`), never `atlas_edit`. Claude Code then skips the prompt for them without running any process, so a missing or mismatched `catl` cannot block a tool call. Claude Code still applies your deny and ask rules. Every rule naming the `code-atlas` server (`mcp__code-atlas`, `mcp__code-atlas__*`) counts as the fragment `sync` owns, so a rule you add or remove there shows as drift, and `sync` rewrites the list when the tool set changes.
- `remind` is a `PreToolUse` hook on `Grep|Glob|Bash`:

  ```json
  { "matcher": "Grep|Glob|Bash", "hooks": [{ "type": "command", "command": "CODE_ATLAS_HOME=/iso catl hook remind || true", "timeout": 10 }] }
  ```

  (`CODE_ATLAS_HOME=...` only when it is set while syncing.) It answers a `Grep` or `Glob` call, or a `Bash` command that runs `grep`, `rg`, `ag`, `ack` or `git grep`, in a directory of a registered project with `{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"..."}}`: a note that `atlas_search`, `atlas_symbol` or `atlas_context` may answer faster. It makes no permission decision, so the tool runs as usual, and it fires at most once per `session_id` (state in `<data dir>/hooks/reminded/`, entries older than 7 days are dropped).

**Exit status.** Claude Code treats a hook that exits 2 as a block. `catl hook ...` exits 0 whatever happens inside it: an unknown hook name or flag (an older or newer `catl`), input that is not the expected JSON, an unreadable home or registry, an error while answering or a panic all end with exit 0 and no output. Only a `catl` that cannot run at all (missing from `PATH`, killed) fails, and the written command ends in `|| true` so the shell turns that into exit 0 too.

Contract verified against the Claude Code hooks reference (code.claude.com/docs/en/hooks, 2026-10) and permissions reference (code.claude.com/docs/en/permissions): input fields `session_id`, `cwd`, `hook_event_name`, `tool_name`, `tool_input`; `additionalContext` is added next to the tool result and leaves the permission flow unchanged; exit 0 with JSON is the structured answer and exit 2 blocks the call; `mcp__<server>__<tool>` names one MCP tool in a permission rule.

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
1. Start a task with atlas_context: a ranked pack of the code to read.
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

The project must exist first (`project create`, or `init` inside a repo that should join an existing project). In a directory that no registered project contains the server still starts but serves only `atlas_status`, which explains how to register the repo. In a git worktree of a registered repo, worktrees are not indexed yet: answers describe the main checkout with a `worktree caveat:` line and `atlas_edit` refuses; start the agent session inside the worktree (see [operations](operations.md#worktrees)). `catl doctor` checks the registry, databases, locks, tool availability and a user-scope entry that still pins `--project`.

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
