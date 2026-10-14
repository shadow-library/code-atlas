# Agent integration

Code Atlas reaches agents two ways: an MCP stdio server (`catl mcp`) and the CLI (`catl ... --json`). Both call the same query layer, so the answers, tags and caveats are identical.

## MCP tools

The server serves one project (`--project`, or inferred from the working directory), 15 tools, and runs the freshness barrier before every call. It is no longer read-only: `atlas_edit` writes files (tool annotation `destructive_hint = true`); the other 14 tools are annotated read-only. Every tool takes an optional `format`: `"text"` (default, compact) or `"json"`. Output is capped at 100,000 bytes (a truncation note says how to narrow the query) and bodies at 400 lines. Server-side call telemetry (calls, errors, latency, bytes, truncations) is part of `atlas_status`.

| Tool | Use it to | Arguments |
|---|---|---|
| `atlas_context` | start a task: ranked, budgeted pack of code to read (full bodies for top items, signatures, one-liners), with why each item is included and what was omitted | `task`, `budget` (500 to 32000 tokens), `seeds` |
| `atlas_search` | find where a capability lives or whether similar code exists (names, text, structure) | `query`, `kind`, `repo`, `limit` (1 to 200) |
| `atlas_symbol` | symbol lookup and IDE navigation; `mode` selects the operation (below) | `target`, `mode`, `path`, `depth`, `body`, `substring`, `kinds`, `exclude_kinds`, `max_matches`, `regex`, `source`, `context_lines`, `lines`, `min_severity` |
| `atlas_edit` | exact renames, symbol edits, safe delete, text replacement, plan application (below) | `operation`, `target`, `path`, `new_name`, `text`, `pattern`, `regex`, `all`, `include`, `exclude`, `occurrences`, `expected_count`, `cross_repo`, `allow_unverified`, `plan`, `dry_run` |
| `atlas_callers`, `atlas_callees` | who calls this, what it calls, grouped by repo | `target`, `depth` (1 to 6, default 1), `via_http` (follow HTTP calls across repos) |
| `atlas_impact` | blast radius of changing one or more targets: dependents, endpoints, cross-repo consumers, tests, config, each with the explaining path | `targets`, `depth` (default 3) |
| `atlas_tests_for` | tests relevant to a target (direct, via call graph, via file coverage heuristics) with run commands | `target` |
| `atlas_api` | HTTP endpoints with handlers, consumers across repos, auth supported vs used | `repo`, `endpoint` |
| `atlas_similar` | near-duplicates of a symbol, of a line range or of unsaved draft code, with consumers, tests and a likely shared home | exactly one of `target`, `snippet` (`[repo:]path:START-END`), `code` (with `language`: `rust`, `ts`, `tsx`, `js`) |
| `atlas_unused` | candidate unused functions, endpoints, auth modes, events, config, with evidence | `kind`, `repo` |
| `atlas_patterns` | how this code base does something (exemplars ranked by prevalence) | `topic` |
| `atlas_explain` | provenance of a symbol or relation: certainty, evidence spans, producers, staleness | `target` |
| `atlas_overview` | project map: repos, packages, applications, entrypoints, key modules, APIs, events, patterns | none |
| `atlas_status` | index health: freshness, parse failures, SCIP coverage, LLM status, language servers, server telemetry; with `since_run`, what index runs newer than that run changed (files analysed, deleted, moved, renamed symbols) | `since_run` |

Targets accepted everywhere: a name (`UsersService.findOne`), `[repo:]path[:line]`, an endpoint (`GET /users/{id}`), or a full key. `atlas_symbol` and `atlas_edit` also take name paths (`UsersService/findOne`, `b[1]`) and `[repo:]path:LINE:COL` positions (1-based line, UTF-8 byte column). A name that is not found or is ambiguous returns candidates or a pointer to `atlas_search`. There is no reindex tool: freshness is automatic, and `atlas_status` with `since_run` confirms what the index saw after an edit.

`atlas_read` and `atlas_changes` were removed in this release (16 tools to 15). Calling either returns an error naming the replacement (`atlas_symbol mode=source`, `atlas_status since_run`). `atlas_dependents` and `atlas_dependencies` exist in the CLI (`query dependents`, `query dependencies`) but are not exposed over MCP. The CLI has top-level shortcuts for `search`, `impact`, `context`, `callers`, `callees`, `symbol`, `api`, `explain`, `read` and `tests-for`, and its own `find`, `outline`, `refs`, `decl`, `impls`, `hierarchy`, `diagnostics`, `edit` and `lsp check` ([operations](operations.md#cli-reference)).

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
2. **Start a task** with `atlas_context`. Read what it lists, not the whole repo. Items marked "likely to edit: read the file directly" show only an outline; for a rename or symbol rewrite use `atlas_edit`.
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

## `connect claude`

```sh
cd <a registered repo>
catl connect claude            # project scope
catl connect claude --scope user
```

- Project scope writes `.mcp.json` at the repo root (`{"mcpServers":{"code-atlas":{"type":"stdio","command":<absolute path to this binary>,"args":["mcp","--project",<name>]}}}`) and adds a marked block (`<!-- code-atlas:begin -->` ... `end`) to `CLAUDE.md` or `AGENTS.md` (the first that exists; otherwise a new `AGENTS.md`). The block tells the agent to start with `context`, run `impact` before changing shared symbols, `search` and `similar` before writing helpers, `explain` for decisions, to use `atlas_edit` to rename or rewrite a symbol (and to re-read a file after `atlas_edit` changed it before editing it itself), to use `atlas_symbol mode=source` only for code it will not edit, to treat heuristic and inferred results as leads, and to verify `DEGRADED` answers. Its meaning matches the MCP server instructions (`crates/atlas-mcp/src/descriptions.rs`).
- User scope registers the server with `claude mcp add-json --scope user code-atlas ...` (needs the `claude` CLI) and updates only the guidance file; `CODE_ATLAS_HOME`, if set, is passed through in the server's `env`.
- `CODE_ATLAS_HOME` is deliberately left out of the project `.mcp.json` because that file is usually committed; a warning says so.
- Existing files are shown as a diff and changed only after confirmation (`--yes` skips the prompt). Re-running is idempotent.

The project must exist first (`project create`, or `init` inside a repo that should join an existing project). `catl doctor` checks the registry, databases, locks and tool availability.

## Other agents

The MCP server is vendor-neutral stdio, protocol-compliant with any client that can launch a command:

```json
{ "mcpServers": { "code-atlas": { "command": "/abs/path/catl", "args": ["mcp", "--project", "acme"] } } }
```

Add `"env": { "CODE_ATLAS_HOME": "..." }` if the registry is not in the default location, and `--watch` to the args to keep the index fresh between calls (otherwise each call refreshes it on demand). Put the guidance from the block above into whatever instruction file your agent reads (`AGENTS.md`, rules files, a system prompt). Because `atlas_edit` writes files, clients that gate tools by annotation will treat it as destructive; approve it deliberately. The MCP server keeps language servers warm (rust-analyzer can hold about 2 GB; see [operations](operations.md#language-servers)).

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
- `atlas_context` bodies for files marked "likely to edit" are suppressed for the same reason; they show an outline.
- The atlas run used 15 turns and about 5x the tokens of baseline (including cache reads) on a one-file task, while reading a relevant file the baseline run missed. The pipeline works end to end; cost and benefit on realistic tasks are measured by the A/B harness, not by this single run.
- Both runs failed the same hidden check (a column-name detail): model capability, not the index.
- The index cannot tell you about code it does not model. Read the caveats, then the files.
