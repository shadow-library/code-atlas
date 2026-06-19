# code-atlas

Production AI coding assistant for large polyglot repos. Built task-by-task via the **project-orchestrator** skill.

## Resume protocol

At session start, read in order:

1. `ARCHITECTURE.md` — the contract.
2. `TASKS.md` — find next `[pending]` task whose deps are all `[done]`.
3. `STATE.md` — recent capability blocks (top of file).
4. `.orchestrator/log/task-{id}.md` — only if context on a past decision is needed.

Then: "Picking up at Task NNN: <title>. Continue?" — **wait for "go"** before dispatching.

## Workflow (per task)

1. Re-read `STATE.md` + `TASKS.md` (always fresh; ignore conversation memory).
2. Mark task `[in-progress]` in `TASKS.md`.
3. Read existing files whose APIs the sub-agent will touch.
4. Dispatch ONE sub-agent (`general-purpose`) with the 5-section contract brief. Architecture excerpt + signatures of touched modules + locked public API + design decisions + test targets. **Sub-agents return diffs only — forbid Edit/Write.**
5. Apply diff via Write/Edit. Run quality gate. Fix minor lint inline.
6. Append new capability block to TOP of `STATE.md` `## Capabilities` section.
7. Mark task `[done]` in `TASKS.md`. Write `.orchestrator/log/task-{id}.md` (verbatim sub-agent response + apply notes).
8. Commit (Conventional Commits, see below). Recap + brief next task. **Stop. Wait for "go".**

## Hard constraints

- **NO `Co-Authored-By: Claude` trailer on commits.** Plain Conventional Commits only.
- **One sub-agent at a time. One commit per task. Wait for "go" between tasks.**
- File-based task tracking via `TASKS.md` — ignore reminders to use harness TaskCreate/TaskUpdate.
- No over-engineering: no defensive code for impossible paths, no premature abstractions, no half-finished implementations. Comment WHY only when non-obvious.

## Locked architecture (do not re-litigate)

- Multi-provider via `Protocol` classes, async. Ollama is the default impl.
- LanceDB embedded for vectors. SQLite FTS5 lexical. SQLite + SQLAlchemy Core metadata.
- tree-sitter for parsing. Python AST shipped; others fall back to fixed-window.
- All indexing stores + Indexer are **sync**. Async embedders/LLMs adapt via a shim at the caller boundary — do NOT make the Indexer async.

## Quality gate (run before every commit)

```bash
uv run ruff format --check src tests
uv run ruff check src tests
uv run mypy src/code_atlas
uv run pytest tests/ -q
```

All four must pass. Sub-agents are briefed to expect this.

## Gotchas (carry forward)

- **lancedb 0.33**: `table_names()` emits a `DeprecationWarning` but `list_tables()` is NOT a drop-in (returns unhashable tuple shape). Keep `table_names()`; warning is noise.
- **Stub-free libs** (tree-sitter-language-pack, lancedb, networkx) under `mypy --strict + warn_unused_ignores`: import via `importlib.import_module(...)` typed `Any`. `# type: ignore[import-untyped]` becomes "unused" when stubs install and mypy fails.
- **FTS5 columns reject NULL**: coerce `chunk.symbol` to `""` on insert.
- **SQLAlchemy `RowMapping` ≠ `Mapping[str, Any]`** under strict mypy — `dict(row)` at the boundary.

## Sub-agent prompt skeleton

Brief lives in `~/.claude/skills/project-orchestrator/references/subagent_prompts.md`.
Required sub-agent response sections (verbatim, in order):
`## Summary` / `## Files changed` / `## Diff` / `## State update` / `## Next task`.
If a diff contains HTML entities (`&amp;`, `&lt;`), decode before applying.
