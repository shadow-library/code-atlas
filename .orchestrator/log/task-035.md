# Task 035 — True incremental SSE streaming (`QAAgent.ask_stream`)

**Status:** done (Phase 10 follow-up)
**Files:** `src/code_atlas/agent/qa.py`, `src/code_atlas/agent/__init__.py`, `src/code_atlas/api/routes.py`, `tests/integration/agent/test_qa.py`, `tests/unit/api/test_app.py`

## Apply notes
- Applied the sub-agent diff verbatim as targeted edits across all five files. No deviations.
- Gate clean first-try (no format reflow / no post-write fixes): ruff format ✓, ruff check ✓, mypy ✓ (48 source files), pytest ✓ (321 passed, +3).
- Supersedes the Task 025 SSE-replay deferral. `/ask/stream` now streams real LLM tokens; `ask` (non-streaming) untouched.

## Design highlights
- `ask_stream` drives `chat_stream`, yields `StreamEvent(type="token")` per `content_delta`, terminal `StreamEvent(type="done", answer=...)`. Usage summed across turns (final `done` chunk carries it).
- Token deltas stream every turn; Ollama tool-call turns have empty content so only the final turn surfaces. Terminal `done` Answer is authoritative.
- `_event_stream` rewritten with SSE-correct multi-line `data:` framing (`_sse_data`) + `event: error` on `CodeAtlasError`.

## Sub-agent response (verbatim — Summary + State update)

### Summary
Adds a true incremental SSE streaming path. New module-level frozen dataclass `StreamEvent` and `QAAgent.ask_stream` async generator mirror `ask`'s retrieval + bounded tool-use loop, but drive `chat_stream` and `yield` content-delta tokens live, then emit a terminal `done` event carrying the authoritative `Answer`. `ask` is untouched. The SSE `_event_stream` is rewritten to consume `ask_stream` with correct multi-line `data:` framing (via a `_sse_data` helper), a terminal `event: done`, and an `event: error` for `CodeAtlasError`. Tests add a `StreamingFakeLLM` plus three `ask_stream` cases, and the API fake gains an `ask_stream` generator.

### State update
- `QAAgent.ask_stream(question) -> AsyncIterator[StreamEvent]` mirrors `ask` (retrieval + bounded tool-use loop) but drives `chat_stream`, yielding token deltas live, then a terminal `done` with the full Answer. Tool-call turns accumulate `tool_call_delta`/usage/`finish_reason`; the final turn's content is the authoritative `Answer.text`. `ask` unchanged.
- `StreamEvent` frozen dataclass (`type: Literal["token","done"]`, `text=""`, `answer: Answer | None`), exported from `code_atlas.agent.qa` and `code_atlas.agent`.
- `_event_stream` now consumes `ask_stream`: `_sse_data` frames multi-line token text as one SSE event; terminal `event: done` carries `answer.model_dump_json()`; `CodeAtlasError` surfaces as `event: error`.
- Tests: `StreamingFakeLLM` (canned per-turn chunk lists, `chat` raises) + single-turn, tool-then-answer, blank-question cases; API `FakeAgent.ask_stream` keeps `/ask/stream` working and `test_ask_stream_sse` green.
- Carry-forward: an async generator's leading `raise` surfaces on first `__anext__` (tests force iteration via list-comprehension).

## Next task
None outstanding. Possible future: stream tool-call activity as SSE progress events; or a CLI `ask --stream` flag reusing `ask_stream`.
