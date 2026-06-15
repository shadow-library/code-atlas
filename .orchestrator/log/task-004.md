# Task 004 — Errors module (typed exception hierarchy)

**Status:** done
**Sub-agent:** general-purpose (agentId aa10615d65ae5b73b)

## Files changed
- src/code_atlas/errors.py (new)
- tests/unit/test_errors.py (new)

## Sub-agent response (verbatim)

### Summary
Added typed exception hierarchy module `src/code_atlas/errors.py` with `CodeAtlasError` base (carrying optional context dict, custom `__str__`/`__repr__`) and eight subclasses (`ConfigError`, `IngestionError`, `IndexingError`, `RepositoryNotIndexed` extending `IndexingError`, `ProviderError`, `RetrievalError`, `AgentError`, `EvaluationError`). Added matching unit tests covering str/repr formatting, inheritance, context propagation across raise/except, per-instance default dicts, and repr-form of context in str.

### State update
Task 004 done. Two new files: `src/code_atlas/errors.py` (base + 8 subclasses, `__all__` sorted) and `tests/unit/test_errors.py` (8 tests). `RepositoryNotIndexed` extends `IndexingError`. Context dict per-instance via `dict(context) if context else {}`. No `__init__.py` in tests tree.

### Next task
Task 005 per `TASKS.md` — caller to dispatch.

## Apply notes

- HTML-entity encoding in transcript again: `-&gt;` and `&amp;` decoded to `->` and `&`.
- Two new files written via Write (mkdir tests/unit first). Content matches diff bytes.
- Acceptance verified locally:
  - `uv run ruff format --check src tests` → OK
  - `uv run ruff check src tests` → All checks passed
  - `uv run mypy src/code_atlas` → Success (2 files, strict)
  - `uv run pytest tests/unit/test_errors.py -q` → 8 passed
- The local venv ended up on Python 3.14, producing a pytest-asyncio deprecation warning unrelated to our code. CI matrix (3.11 / 3.12) will not surface this. Not a blocker.
