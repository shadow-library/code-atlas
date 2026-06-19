# Task 019 — Ollama LLM provider (chat + stream + tools)

**Status:** done
**Sub-agent:** general-purpose (agentId a66739594ca8b1db7)

## Files changed
- src/code_atlas/providers/ollama_llm.py (new)
- src/code_atlas/providers/__init__.py (modified — re-export class)
- tests/unit/providers/test_ollama_llm.py (new)

## Sub-agent response (verbatim)

### Summary
Add `OllamaLLMProvider` doing async chat + NDJSON streaming + tool calls hitting Ollama `/api/chat`. Auto-register as `"ollama"` in LLM registry. Mirror Task 018 ownership pattern (`_owns_client`), validation idiom, and error wrapping. 14 mock-transport tests.

### State update
- `_emit_line` helper handles the four streaming cases: content-only intermediate lines, tool-call-only lines, content+tool combos, and the final `done: true` line (folds last tool call into the terminal chunk).
- Constructor validates `model` non-empty, `temperature` in `[0, 2]`, `max_tokens >= 1`. Empty messages raise `ProviderError`.
- Streaming wraps the whole `async with stream()` block in try/except — captures both pre-stream HTTP failures and mid-stream disconnects.
- Providers exposed: `OllamaEmbeddingProvider` + `OllamaLLMProvider`. Registry has both `"ollama"` factories.

### Next task
Wire end-to-end retrieval → chat: thin agent/answerer that pulls candidates from indexer, formats prompts with citations, calls `make_llm(settings).chat(...)`.

## Apply notes

- Diff was clean (no HTML entities). Applied via Edit (two separate edits on __init__.py to keep the imports + __all__ entries alphabetically sorted) and Write (two new files).
- Quality gate clean first try — no post-write fixes needed.
- Acceptance verified locally:
  - `uv run ruff format --check src tests` → 43 files already formatted
  - `uv run ruff check src tests` → All checks passed
  - `uv run mypy src/code_atlas` → Success (27 source files, strict)
  - `uv run pytest tests/ -q` → 208 passed (194 prior + 14 new)
- Sub-agent's "Next task" maps to Phase 5 (Retrieval), specifically TASKS.md Task 020 (hybrid retrieval). The actual ordering will land hybrid retrieval first, then the QA agent on top. Sub-agent's intuition is correct but the canonical task is already on the backlog.
