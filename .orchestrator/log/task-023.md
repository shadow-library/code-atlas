# Task 023 — Prompts + QA agent (closes Phase 6)

**Status:** done
**Deps:** 020, 021, 022, 019 (all done)
**Files:** src/code_atlas/agent/prompts.py (new), src/code_atlas/agent/qa.py (new), src/code_atlas/agent/__init__.py (modified), tests/integration/agent/test_qa.py (new)

## Apply notes

- Applied sub-agent diff verbatim across 4 files.
- One post-write fix: `uv run ruff format` collapsed a multi-line `messages.append(ChatMessage(role="tool", ...))` call onto a single line (fits within 120 cols). No logic change.
- Quality gate green: `ruff format --check` (56 files), `ruff check` (clean), `mypy src/code_atlas` (35 source files, no issues), `pytest` **256 passed** (4 new). Warnings are pre-existing noise (lancedb `table_names()` deprecation, pytest-asyncio `get_event_loop_policy` deprecation).

## Key decisions (locked)

- **No repo_id filter on retrieval.** v1 scopes one indexed repo per agent instance; stores hold a single repo. `Toolbox` binds repo_id privately with no public accessor, and `tools.py` was out of scope — so QAAgent does not filter retrieval by repo_id.
- **No reranker** in QAAgent — constructor signature is locked to `(retriever, llm, toolbox, max_tool_iters, retrieval_k)`; reranker integration deferred.
- **Citations from retrieved chunks** (deduped by path+start+end), not parsed from answer text — guarantees grounding inputs and satisfies "≥1 citation matching the retrieved chunk".
- **Answer.text never empty** — falls back to `DECLINE_MESSAGE` on blank model content.
- Bounded loop `range(max_tool_iters + 1)`: ≤ max_tool_iters tool-execution rounds + a final answer call; `iteration == max_tool_iters` breaks without executing more tools.
- `_run_tool` swallows `AgentError` into `{"error": ...}` so tool failures feed back to the LLM rather than aborting the loop.
- `assert response is not None` for mypy strict narrowing (loop always runs ≥1×).

## Carry-forward gotchas (reaffirmed)

- Integration test uses **file-backed** SQLite (`sqlite:///{tmp_path}/meta.sqlite`), NOT `:memory:`, because `HybridRetriever` calls `MetadataStore.get_many` via `asyncio.to_thread` (worker thread would see an empty `:memory:` DB).
- SymbolGraph nodes keyed by `(path, name)` — distinct symbols need distinct paths.

---

## Verbatim sub-agent response

## Summary

Implemented Task 023, closing Phase 6: the Q&A orchestrator. Added `agent/prompts.py` with the system prompt (encoding the three grounding rules), `DECLINE_MESSAGE`, and the `format_context` / `format_user_prompt` renderers; added `agent/qa.py` with `QAAgent` that retrieves, drives a bounded tool-use loop over the `Toolbox`, dedups citations from the retrieved chunks, accumulates token usage, and returns a non-empty `Answer`. Wired `QAAgent` into `agent/__init__.py` and added a self-contained integration test exercising a real `HybridRetriever` + file-backed `MetadataStore` + real `SymbolGraph` + real `Toolbox` against a fake LLM.

## Files changed

- `src/code_atlas/agent/prompts.py` (new)
- `src/code_atlas/agent/qa.py` (new)
- `src/code_atlas/agent/__init__.py` (modified)
- `tests/integration/agent/test_qa.py` (new)

## State update

### Task 023 — Prompts + QA agent (closes Phase 6) [done]

- Built `agent/prompts.py`: `SYSTEM_PROMPT` encodes the 3 grounding rules (cite `path:start-end`, never invent paths, decline when insufficient); `DECLINE_MESSAGE` fallback; `format_context` (header `[path:start-end] symbol=...` + content, blank-line separated; empty → short notice); `format_user_prompt` (question + "Context:" section).
- Built `agent/qa.py` `QAAgent` (kw-only ctor: retriever, llm, toolbox, max_tool_iters=4, retrieval_k=8; validates >=1). `ask()`: guards empty question -> AgentError, times via perf_counter, retrieves (no repo_id filter — v1 single repo per agent), system+user messages, bounded tool-use loop over `range(max_tool_iters+1)`, accumulates token usage, dedups citations from retrieved chunks via `to_citation` (by path/start/end, first-seen order), returns non-empty `Answer` (falls back to DECLINE_MESSAGE on blank content).
- `_run_tool` traces the call and swallows `AgentError` into `{"error": ...}` so tool failures feed back to the LLM rather than aborting the loop. Assistant tool-call turn appended as `ChatMessage(role="assistant", content=...)` before tool-result messages (ChatMessage has no tool_calls field — acceptable for v1).
- Wired `QAAgent` into `agent/__init__.py` (`__all__ = ["QAAgent", "ToolResult", "Toolbox"]`).
- Decisions: no repo_id filter on retrieval; no reranker (deferred); citations from retrieved chunks (not parsed from answer text) to guarantee grounding; `json.dumps` for tool-result content; `assert response is not None` for mypy narrowing.
- Gotcha reaffirmed: integration test uses file-backed SQLite (not `:memory:`) because HybridRetriever calls the store via `asyncio.to_thread`; distinct symbols need distinct paths (graph nodes keyed by `(path, name)`).
- Tests: 4 integration tests — grounded answer + token total, tool executed & fed back, decline on blank content, empty-question raises. No `__init__.py` under tests (pythonpath=src). `asyncio_mode="auto"`, so async tests run without explicit markers.

## Next task

Task 024 — CLI.
