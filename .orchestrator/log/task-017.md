# Task 017 — Provider Protocols + registry

**Status:** done
**Sub-agent:** general-purpose (agentId a1192228e39f62e62)

## Files changed
- src/code_atlas/providers/__init__.py (new)
- src/code_atlas/providers/base.py (new)
- src/code_atlas/providers/registry.py (new)
- tests/unit/providers/test_registry.py (new)

## Sub-agent response (verbatim)

### Summary
Seed providers subsystem: EmbeddingProvider/LLMProvider Protocols, frozen pydantic records (ChatMessage, ToolSpec, ToolCall, ChatResponse, ChatChunk), name-keyed registry with make_embedding/make_llm resolvers and error wrapping. Ship 10 unit tests.

### State update
- Task 017 done. New pkg `code_atlas.providers`. Layering clean: depends on domain/errors/utils/config only.
- `base.py` ships Protocols `EmbeddingProvider` + `LLMProvider` (async) + 5 frozen pydantic records. `ChatRole` Literal. `chat_stream` typed as method returning `AsyncIterator[ChatChunk]` per Protocol+async-gen idiom.
- `registry.py` two module dicts. `register_*` whitespace name → `ProviderError`; re-register silently overwrites. `make_*` missing → `ProviderError` with `{"name", "available": sorted(...)}`. Factory exception wrapped, original in `__cause__`. Info-log on register, warn-log on factory fail.
- Registry ships empty — concrete providers register on import in Task 018/019.
- 10 unit tests with autouse `clear_registry` fixture.

### Next task
Task 018: implement `OllamaEmbeddingProvider` (httpx async, batched `/api/embed`, auto-registers as `"ollama"`).

## Apply notes

- Diff clean. Applied via Write for all four new files.
- Quality gate first-try clean — no post-write fixes needed.
- Acceptance verified locally:
  - `uv run ruff format --check src tests` → 39 files already formatted
  - `uv run ruff check src tests` → All checks passed
  - `uv run mypy src/code_atlas` → Success (25 source files, strict)
  - `uv run pytest tests/ -q` → 184 passed (172 prior + 12 new; 10 explicitly written + 2 pytest parametrized expansions)
- Sub-agent's "Next task" matches the planned Task 018 (Ollama embedding provider).
- Side commit this turn: `CLAUDE.md` (session resume guide) shipped as a separate `docs:` commit before the Task 017 commit. It's not part of any planned task; it's project documentation requested by the user.
