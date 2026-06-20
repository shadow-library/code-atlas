# Task 034 — Extract shared CLI composition root (refactor)

**Status:** done (Phase 10 follow-up)
**Files:** `src/code_atlas/cli.py`

## Apply notes
- Applied the sub-agent diff as targeted edits against the live file (imports + helper insert, then `ask` and `eval` body rewrites). No deviations from the brief.
- Pure behavior-preserving refactor — `ingest` untouched. Gate clean first-try (no format reflow needed): ruff format ✓, ruff check ✓, mypy ✓ (48 source files), pytest ✓ (318 passed). `code-atlas ask --help` / `eval --help` still render all flags.

## Sub-agent response (verbatim — Summary + State update)

### Summary
Behavior-preserving refactor of `src/code_atlas/cli.py`. Lifted the identical stack-construction and teardown shared by `ask` and `eval` into a `_QAStack` dataclass (with `aclose_providers` / `close_stores` methods) plus a `_build_qa_stack` factory, inserted right after `_StorePaths`. Rewrote both command bodies to use the helper. `ingest` is untouched (its lifecycle genuinely differs). No flag, output, ordering, exit-code, or close-order change.

Imports: added `dataclass` (runtime); added a `TYPE_CHECKING` block importing `EmbeddingProvider, LLMProvider` from `code_atlas.providers.base` (a plain `@dataclass` under `from __future__ import annotations` never resolves annotations at runtime, so `TYPE_CHECKING` is correct and keeps the Protocols out of the runtime import graph). isort ordering preserved.

### State update
Task 034 complete: extracted shared CLI composition root. Added `_QAStack` dataclass (`embedder`, `llm`, `metadata`, `lexical`, `vector`, `agent`; `aclose_providers()` + `close_stores()`) and `_build_qa_stack(settings, paths, repo_id)` factory after `_StorePaths`. `ask` and `eval` build/teardown via the helper, dropping ~12 lines of duplicated construction plus the duplicated teardown loop each. `EmbeddingProvider`/`LLMProvider` under `TYPE_CHECKING`. Pure behavior-preserving refactor: same flags, output, exit codes, single-`asyncio.run` shim, provider/store close order. `ingest` untouched (distinct lifecycle).

## Next task
None outstanding. Optional future: fold `ingest`'s 3-store opening into a smaller shared `_open_stores` helper (lower value — ingest differs in graph + loop + indexer), true incremental SSE streaming, or STATE.md compaction.
