# Agent integration

Code Atlas reaches agents two ways: an MCP stdio server (`code-atlas mcp`) and the CLI (`code-atlas ... --json`). Both call the same query layer, so the answers, tags and caveats are identical.

## MCP tools

The server serves one project (`--project`, or inferred from the working directory), 16 tools, and runs the freshness barrier before every call. Every tool takes an optional `format`: `"text"` (default, compact) or `"json"`. Output is capped at 100,000 bytes (a truncation note says how to narrow the query) and bodies at 400 lines. Server-side call telemetry (calls, errors, latency, bytes, truncations) is part of `atlas_status`.

| Tool | Use it to | Arguments |
|---|---|---|
| `atlas_context` | start a task: ranked, budgeted pack of code to read (full bodies for top items, signatures, one-liners), with why each item is included and what was omitted | `task`, `budget` (500 to 32000 tokens), `seeds` |
| `atlas_search` | find where a capability lives or whether similar code exists (names, text, structure) | `query`, `kind`, `repo`, `limit` (1 to 200) |
| `atlas_symbol` | definition, signature, docs, location, relation counts; body only with `body=true` | `target`, `body` |
| `atlas_callers`, `atlas_callees` | who calls this, what it calls, grouped by repo | `target`, `depth` (1 to 6, default 1) |
| `atlas_impact` | blast radius of changing one or more targets: dependents, endpoints, cross-repo consumers, tests, config, each with the explaining path | `targets`, `depth` (default 3) |
| `atlas_tests_for` | tests relevant to a target (direct, via call graph, via file coverage heuristics) with run commands | `target` |
| `atlas_api` | HTTP endpoints with handlers, consumers across repos, auth supported vs used | `repo`, `endpoint` |
| `atlas_similar` | near-duplicates of a symbol or of a line range, with consumers, tests and a likely shared home | exactly one of `target`, `snippet` (`[repo:]path:START-END`; code text is not accepted) |
| `atlas_unused` | candidate unused functions, endpoints, auth modes, events, config, with evidence | `kind`, `repo` |
| `atlas_patterns` | how this code base does something (exemplars ranked by prevalence) | `topic` |
| `atlas_explain` | provenance of a symbol or relation: certainty, evidence spans, producers, staleness | `target` |
| `atlas_read` | source of exactly one symbol, re-read from disk | `target`, `context_lines` (0 to 100) |
| `atlas_overview` | project map: repos, packages, applications, entrypoints, key modules, APIs, events, patterns | none |
| `atlas_status` | index health: freshness, parse failures, SCIP coverage, LLM status, server telemetry | none |
| `atlas_changes` | what recent index runs changed (files analyzed, deleted, moved, renamed symbols) | `since_run` |

Targets accepted everywhere: a name (`UsersService.findOne`), `[repo:]path[:line]`, an endpoint (`GET /users/{id}`), or a full key. A name that is not found or is ambiguous returns candidates or a pointer to `atlas_search`. There is no reindex tool: freshness is automatic, and `atlas_changes` confirms what the index saw after an edit.

`atlas_dependents` and `atlas_dependencies` exist in the CLI (`query dependents`, `query dependencies`) but are not exposed over MCP at this commit. The CLI has top-level shortcuts for `search`, `impact`, `context`, `callers`, `callees`, `symbol`, `api`, `explain`, `read` and `tests-for`.

## Recommended workflow

1. **Orient** in an unfamiliar project with `atlas_overview`.
2. **Start a task** with `atlas_context`. Read what it lists, not the whole repo. Items marked "likely to edit: read the file directly" show only an outline.
3. **Before writing new code**, run `atlas_search` and `atlas_similar` to find something to reuse. A hit is a candidate, not proof of equivalence.
4. **Before changing shared code**, run `atlas_impact` on it, and `atlas_tests_for` to choose tests. Treat `UNCERTAIN` entries as work to verify, not as noise.
5. **Edit using your own tools.** Read the file you will edit with your editor's read tool (the harness may require it before an edit). Use `atlas_read` only for code you want to view and will not edit.
6. **When a relation drives a decision**, run `atlas_explain` on it.
7. **After editing**, re-query or call `atlas_changes`.
8. **Verify before relying on** any `[heuristic]` or `[inferred]` relation, any "none found", and any answer that starts with `DEGRADED`.

## Reading certainty tags

| You see | Meaning | What to do |
|---|---|---|
| `[compiler]`, `[syntactic]`, `[resolved]`, `[declared]`, or no tag on an entity | exact under the class's rules, not exhaustive | rely on it; still read the caveats |
| `Name kind path [heuristic 0.80]` | the **entity** is uncertain (a tag after the entity name) | verify the entity exists as described |
| `X [entity tag] ([relation tag])`, `verb target [tag]` | entity and relation certainty are separate: parentheses, or a tag after the target of a `verb target` step, tag the relation | read both |
| `possibly calls ... [heuristic 0.72]` | a rule with a confidence; may be wrong | open the source |
| `likely ... [inferred 0.81]` | LLM-derived with rationale (for example a client call linked to an endpoint by `enrich`, shown as a tagged consumer in `api`, `impact`, `unused`, `explain`) | verify; check `stale` markers |
| `[inferred 0.81 stale]`, `[... needs-revalidation]` | evidence changed since the LLM looked | treat as unknown |
| `UNCERTAIN` on an impact entry | every path to it passes through a non-authoritative edge | verify |
| `caveats:` block | unresolved references, dynamic member calls, partial parses, hidden low-confidence relations, skipped files | decides whether "none found" means anything |
| first line `DEGRADED: ...` | index could not be fully refreshed; affected files listed | verify against files |
| header `skipped N files (see status)` | files over the size limit, symlinks or non-UTF-8 paths are not indexed | check `atlas_status` |

Compiler-verified relations render as `[compiler]` everywhere in text output (tool descriptions use the same spelling); the JSON field `certainty.class` is `compiler_verified` and `certainty.authoritative` is a boolean. In JSON, every relation carries `certainty` (`class`, `confidence`, `freshness`, `authoritative`, `tag`) and every response carries a `freshness` object. Rules of thumb: an empty result is not proof of absence; "unused" means "no inbound edges found"; a similarity match is a lead.

## `connect claude`

```sh
cd <a registered repo>
code-atlas connect claude            # project scope
code-atlas connect claude --scope user
```

- Project scope writes `.mcp.json` at the repo root (`{"mcpServers":{"code-atlas":{"type":"stdio","command":<absolute path to this binary>,"args":["mcp","--project",<name>]}}}`) and adds a marked block (`<!-- code-atlas:begin -->` ... `end`) to `CLAUDE.md` or `AGENTS.md` (the first that exists; otherwise a new `AGENTS.md`). The block tells the agent to start with `context`, run `impact` before changing shared symbols, `search` and `similar` before writing helpers, `explain` for decisions, to treat heuristic and inferred results as leads, and to verify `DEGRADED` answers.
- User scope registers the server with `claude mcp add-json --scope user code-atlas ...` (needs the `claude` CLI) and updates only the guidance file; `CODE_ATLAS_HOME`, if set, is passed through in the server's `env`.
- `CODE_ATLAS_HOME` is deliberately left out of the project `.mcp.json` because that file is usually committed; a warning says so.
- Existing files are shown as a diff and changed only after confirmation (`--yes` skips the prompt). Re-running is idempotent.

The project must exist first (`project create`, or `init` inside a repo that should join an existing project). `code-atlas doctor` checks the registry, databases, locks and tool availability.

## Other agents

The MCP server is vendor-neutral stdio, protocol-compliant with any client that can launch a command:

```json
{ "mcpServers": { "code-atlas": { "command": "/abs/path/code-atlas", "args": ["mcp", "--project", "acme"] } } }
```

Add `"env": { "CODE_ATLAS_HOME": "..." }` if the registry is not in the default location, and `--watch` to the args to keep the index fresh between calls (otherwise each call refreshes it on demand). Put the guidance from the block above into whatever instruction file your agent reads (`AGENTS.md`, rules files, a system prompt).

Agents without MCP can shell out. All query commands accept `--json` and exit non-zero as documented in [operations](operations.md#exit-codes):

```sh
code-atlas -p acme impact UsersService.findOne --json
code-atlas -p acme query tests-for UsersService.findOne --json
code-atlas -p acme query similar --snippet src/money.ts:1-7 --json   # path relative to cwd, or repo:path:1-7
```

JSON responses are `{ "freshness": {...}, "answer": {...} }`. Check `freshness.state` and exit code 3 before trusting `answer`, and read `answer.caveats`.

## Tips from the smoke run

From the harness smoke run (2026-10-04, `t00-smoke`, one run per condition; see [dogfooding](evaluation/dogfooding.md)):

- An agent called `atlas_read` on a file and then the native `Read` on the same file, because the editor requires a read before an edit. The context was duplicated. Tool descriptions and the context pack now say: read files you will edit natively, use `atlas_read` only to view code you will not edit.
- `atlas_context` bodies for files marked "likely to edit" are suppressed for the same reason; they show an outline.
- The atlas run used 15 turns and about 5x the tokens of baseline (including cache reads) on a one-file task, while reading a relevant file the baseline run missed. The pipeline works end to end; cost and benefit on realistic tasks are measured by the A/B harness, not by this single run.
- Both runs failed the same hidden check (a column-name detail): model capability, not the index.
- The index cannot tell you about code it does not model. Read the caveats, then the files.
