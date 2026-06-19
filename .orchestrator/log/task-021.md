# Task 021 — Citation hydration + Reranker Protocol

**Status:** done
**Sub-agent:** general-purpose (agentId a6f49c6ea9db94d92)

## Files changed
- src/code_atlas/retrieval/citation.py (new)
- src/code_atlas/retrieval/reranker.py (new)
- src/code_atlas/retrieval/__init__.py (modified)
- tests/unit/retrieval/test_citation.py (new)
- tests/unit/retrieval/test_reranker.py (new)

## Sub-agent response (verbatim)

### Summary
Add `to_citation(chunk)` helper that projects `CodeChunk` to `Citation` with snippet truncation. Add `Reranker` Protocol plus `PassthroughReranker` no-op default. Re-export from `retrieval/__init__.py` maintaining RUF022 sort order.

### State update
- New `code_atlas.retrieval.citation` (`to_citation` + `DEFAULT_SNIPPET_MAX_CHARS=800`).
- New `code_atlas.retrieval.reranker` (`Reranker` Protocol + `PassthroughReranker`).
- `PassthroughReranker.rerank` shallow-copies via `list(results)`, uses `del query` to silence unused-arg lint.
- Snippet is raw prefix — no ellipsis. Negative `max_snippet_chars` raises ValueError; zero allowed.

### Next task
Wire `Reranker` into a retrieval entrypoint (Retriever facade running hybrid → rerank → citation projection), inject PassthroughReranker as default.

## Apply notes

- Diff was clean. Applied via Write (new files) + Edit (__init__.py).
- Quality gate clean first-try — no post-write fixes needed (ruff format check, ruff lint, mypy strict, all 230 pytests passed).
- Did NOT create `tests/unit/retrieval/__init__.py` (sub-agent obeyed the brief's explicit "do not create" note this time).
- Acceptance verified locally:
  - `uv run ruff format --check src tests` → 50 files already formatted
  - `uv run ruff check src tests` → All checks passed
  - `uv run mypy src/code_atlas` → Success (31 source files, strict)
  - `uv run pytest tests/ -q` → 230 passed (220 prior + 10 new — 6 citation + 4 reranker)
- Sub-agent's "Next task" suggestion is reasonable (a facade combining hybrid+rerank+citation), but it doesn't match the planned Task 022 in TASKS.md which is the agent-side `Toolbox` (open_file / find_symbol / list_callers / list_callees). The retrieval surface as it stands is composable enough that callers (CLI, API, QAAgent) can wire HybridRetriever → Reranker → to_citation themselves without a facade.
